use std::collections::HashMap;
use std::ffi::OsString;
use std::fs::File;
use std::io::Read;
use std::os::fd::{AsFd, FromRawFd};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

use instant::Instant;
use warp_core::SessionId;

use super::*;
use crate::SizeInfo;
use crate::local_tty::PtyOptions;
use crate::local_tty::server::{api, protocol};
use crate::local_tty::shell::{DirectShellStarter, ShellStarter};
use crate::shell::ShellType;

fn request(supervisor: &mut Supervisor, message: api::Message) -> api::Message {
    let (client, server) = UnixStream::pair().unwrap();
    protocol::send_message(client.as_fd(), message, None).unwrap();
    supervisor.handle_connection(server).unwrap();
    protocol::receive_message(client.as_fd()).unwrap().unwrap()
}

fn shell_options() -> PtyOptions {
    PtyOptions {
        size: SizeInfo::new_without_font_metrics(24, 80),
        window_id: None,
        shell_starter: ShellStarter::Direct(DirectShellStarter::new_for_test(
            ShellType::Bash,
            PathBuf::from("/bin/sh"),
            vec![
                OsString::from("-c"),
                OsString::from("printf detached-output; sleep 10"),
            ],
        )),
        start_dir: None,
        env_vars: HashMap::new(),
        enable_ssh_wrapper: false,
        reuse_ssh_control_master: false,
        shell_debug_mode: false,
        honor_ps1: false,
        node_version_chip_enabled: false,
        close_fds: true,
    }
}

#[test]
fn detached_output_is_replayed_when_session_reattaches() {
    let output_directory = tempfile::tempdir().unwrap();
    let mut supervisor = Supervisor::new(output_directory.path().to_path_buf());
    let session_id = SessionId::from(42);

    let response = request(
        &mut supervisor,
        api::Message::SpawnShellRequest {
            options: Box::new(shell_options()),
            persistent_session_id: Some(session_id),
            client_pid: std::process::id(),
        },
    );
    let api::Message::SpawnShellResponse {
        spawn_result: api::Result::Ok(spawned),
    } = response
    else {
        panic!("terminal supervisor did not spawn the test shell");
    };
    drop(unsafe { File::from_raw_fd(spawned.leader_fd) });
    supervisor
        .sessions
        .get_mut(&session_id)
        .unwrap()
        .attached_client_pid = None;

    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        supervisor.maintain_sessions();
        let output_len = supervisor.sessions[&session_id]
            .detached_output
            .as_file()
            .metadata()
            .unwrap()
            .len();
        if output_len > 0 {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "shell produced no detached output"
        );
        std::thread::sleep(Duration::from_millis(10));
    }

    let response = request(
        &mut supervisor,
        api::Message::PrepareForTerminationRequest {
            client_pid: std::process::id(),
            persistent_session_ids: vec![session_id],
        },
    );
    assert!(matches!(
        response,
        api::Message::PrepareForTerminationResponse
    ));
    let response = request(
        &mut supervisor,
        api::Message::KillSessionRequest {
            session_id,
            client_pid: std::process::id(),
        },
    );
    assert!(matches!(
        response,
        api::Message::KillSessionResponse { error_msg: None }
    ));
    assert!(supervisor.sessions.contains_key(&session_id));

    let response = request(
        &mut supervisor,
        api::Message::AttachSessionRequest {
            session_id,
            client_pid: std::process::id(),
        },
    );
    let api::Message::AttachSessionResponse {
        attach_result: api::Result::Ok(attached),
    } = response
    else {
        panic!("terminal supervisor did not reattach the test shell");
    };
    drop(unsafe { File::from_raw_fd(attached.leader_fd) });
    let mut replay = unsafe { File::from_raw_fd(attached.replay_fd.unwrap()) };
    let mut output = String::new();
    replay.read_to_string(&mut output).unwrap();
    assert_eq!(output, "detached-output");

    let pid = supervisor.sessions[&session_id].child.id();
    let response = request(
        &mut supervisor,
        api::Message::KillChildRequest {
            pid,
            client_pid: std::process::id(),
        },
    );
    assert!(matches!(
        response,
        api::Message::KillChildResponse { error_msg: None }
    ));
}
