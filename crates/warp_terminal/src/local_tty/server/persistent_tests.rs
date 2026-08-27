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

#[test]
fn socket_directory_is_tightened_when_it_is_group_or_world_writable() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("supervisor");
    std::fs::create_dir(&directory).unwrap();
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o777)).unwrap();

    prepare_socket_directory(&directory).unwrap();

    let mode = std::fs::symlink_metadata(&directory).unwrap().mode();
    assert_eq!(
        mode & 0o777,
        0o700,
        "a directory anyone could write into must be tightened before the socket is bound"
    );
}

#[test]
fn socket_directory_is_created_private() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("nested").join("supervisor");

    prepare_socket_directory(&directory).unwrap();

    let mode = std::fs::symlink_metadata(&directory).unwrap().mode();
    assert_eq!(mode & 0o077, 0, "the socket directory must not be readable or writable by others");
}

#[test]
fn binding_refuses_a_path_that_is_not_a_socket() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("not-a-socket");
    std::fs::write(&path, b"").unwrap();

    let error = bind_listener(&path).unwrap_err();

    assert!(
        error.to_string().contains("is not a socket"),
        "expected a refusal, got: {error:#}"
    );
    assert!(path.exists(), "a path we do not recognise must not be unlinked");
}

#[test]
fn binding_refuses_to_displace_a_live_supervisor() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("supervisor.sock");
    let _live = UnixListener::bind(&path).unwrap();

    let error = bind_listener(&path).unwrap_err();

    assert!(
        error.to_string().contains("already listening"),
        "expected a refusal, got: {error:#}"
    );
}

#[test]
fn binding_replaces_a_stale_socket() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("supervisor.sock");
    // A socket file with nobody accepting on it, exactly what a crashed
    // supervisor leaves behind.
    drop(UnixListener::bind(&path).unwrap());

    let listener = bind_listener(&path).unwrap();

    assert!(listener.local_addr().is_ok());
    let mode = std::fs::symlink_metadata(&path).unwrap().mode();
    assert_eq!(
        mode & 0o077,
        0,
        "the socket must be private from the moment it exists, not chmod-ed afterwards"
    );
}

#[test]
fn spooled_output_is_capped_and_keeps_the_most_recent_bytes() {
    let root = tempfile::tempdir().unwrap();
    let mut session = SupervisedSession {
        child: std::process::Command::new("/bin/sh")
            .args(["-c", "sleep 10"])
            .spawn()
            .unwrap(),
        leader: File::open("/dev/null").unwrap(),
        attached_client_pid: None,
        preserve_for_client_pid: None,
        detached_output: tempfile::NamedTempFile::new_in(root.path()).unwrap(),
        spooled_output_bytes: 0,
    };

    // Write past the cap in chunks, with a recognisable marker at the very end.
    let chunk = vec![b'a'; 64 * 1024];
    let written = MAX_SPOOLED_OUTPUT_BYTES + (1024 * 1024);
    let mut total = 0u64;
    while total < written {
        session.spool_output(&chunk).unwrap();
        total += chunk.len() as u64;
    }
    session.spool_output(b"TAIL-MARKER").unwrap();

    let length = session
        .detached_output
        .as_file()
        .metadata()
        .unwrap()
        .len();
    assert!(
        length <= MAX_SPOOLED_OUTPUT_BYTES,
        "spool grew to {length} bytes, past the {MAX_SPOOLED_OUTPUT_BYTES} cap"
    );

    let mut contents = Vec::new();
    let mut file = session.detached_output.reopen().unwrap();
    file.read_to_end(&mut contents).unwrap();
    assert!(
        contents.ends_with(b"TAIL-MARKER"),
        "compaction must keep the most recent output, which is what a replay wants"
    );
    assert!(
        contents.starts_with(SPOOL_TRUNCATION_NOTICE),
        "a truncated replay must say so rather than silently starting mid-stream"
    );

    let _ = session.child.kill();
    let _ = session.child.wait();
}

#[test]
fn a_preserved_session_can_still_be_killed_afterwards() {
    let output_directory = tempfile::tempdir().unwrap();
    let mut supervisor = Supervisor::new(output_directory.path().to_path_buf());
    let session_id = SessionId::from(43);
    let client_pid = std::process::id();

    let response = request(
        &mut supervisor,
        api::Message::SpawnShellRequest {
            options: Box::new(shell_options()),
            persistent_session_id: Some(session_id),
            client_pid,
        },
    );
    let api::Message::SpawnShellResponse {
        spawn_result: api::Result::Ok(spawned),
    } = response
    else {
        panic!("expected the shell to spawn");
    };
    drop(unsafe { File::from_raw_fd(spawned.leader_fd) });

    // The client announces it is going away and wants the session kept.
    request(
        &mut supervisor,
        api::Message::PrepareForTerminationRequest {
            client_pid,
            persistent_session_ids: vec![session_id],
        },
    );

    // The kill that accompanies that shutdown detaches rather than killing.
    assert_eq!(supervisor.kill_session(session_id, client_pid), None);
    assert!(
        supervisor.sessions.contains_key(&session_id),
        "the preserved session must survive its client's shutdown"
    );

    // A later kill must actually kill. Before the mark was consumed this
    // returned success forever and the shell could never be terminated.
    assert_eq!(supervisor.kill_session(session_id, client_pid), None);
    assert!(
        !supervisor.sessions.contains_key(&session_id),
        "a second kill must terminate the session rather than silently succeeding"
    );
}
