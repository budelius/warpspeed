use std::collections::HashMap;
use std::ffi::OsString;
use std::path::PathBuf;
use std::time::Duration;

use instant::Instant;

use super::*;
use crate::SizeInfo;
use crate::local_tty::shell::{DirectShellStarter, ShellStarter};
use crate::shell::ShellType;

fn shell_options(args: &[&str]) -> PtyOptions {
    PtyOptions {
        size: SizeInfo::new_without_font_metrics(24, 80),
        window_id: None,
        shell_starter: ShellStarter::Direct(DirectShellStarter::new_for_test(
            ShellType::Bash,
            PathBuf::from("/bin/sh"),
            args.iter().map(OsString::from).collect(),
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
fn the_host_owns_the_process_after_lending_its_endpoint() {
    let mut host = UnixSessionHost::default();
    let session_id = SessionId::from(u64::MAX - 1);

    let endpoint = host
        .spawn(
            session_id,
            shell_options(&["-c", "sleep 30"]),
            std::process::id(),
        )
        .expect("the host must start a shell");

    assert!(endpoint.pid > 0);
    assert!(endpoint.raw >= 0, "the lent endpoint must be a descriptor");
    assert!(host.is_running(session_id));
    assert!(host.has_sessions());

    host.kill(session_id).expect("the host must end the shell");
    assert!(!host.has_sessions());
}

/// The whole point of the host: it keeps reading once the client lets go, so a
/// detached shell is not blocked on a terminal nobody is draining.
#[test]
fn a_detached_session_keeps_producing_output() {
    let mut host = UnixSessionHost::default();
    let session_id = SessionId::from(u64::MAX - 3);
    host.spawn(
        session_id,
        shell_options(&["-c", "echo still-running; sleep 30"]),
        std::process::id(),
    )
    .expect("the host must start a shell");
    host.detach(session_id).expect("detaching must succeed");

    let deadline = Instant::now() + Duration::from_secs(10);
    let mut seen = String::new();
    let mut buffer = [0u8; 4096];
    while Instant::now() < deadline && !seen.contains("still-running") {
        let read = host
            .read_output(session_id, &mut buffer)
            .expect("reading must not fail");
        if read == 0 {
            std::thread::sleep(Duration::from_millis(10));
            continue;
        }
        seen.push_str(&String::from_utf8_lossy(&buffer[..read]));
    }

    host.kill(session_id).expect("the host must end the shell");
    assert!(seen.contains("still-running"), "read {seen:?}");
}

/// A client that dies without detaching must not leave its session looking
/// attached forever -- nothing could ever reattach to it.
/// `std::process::Command` is disallowed for flashing a console on Windows;
/// this module is Unix-only.
#[allow(clippy::disallowed_types)]
#[test]
fn a_session_whose_client_died_is_reported_as_orphaned() {
    let mut host = UnixSessionHost::default();
    let session_id = SessionId::from(u64::MAX - 4);
    let mut client = std::process::Command::new("true")
        .spawn()
        .expect("`true` must be runnable");
    let client_pid = client.id();
    host.spawn(session_id, shell_options(&["-c", "sleep 30"]), client_pid)
        .expect("the host must start a shell");
    client.wait().expect("the client must be reapable");

    let orphaned = host.take_orphaned_sessions();

    // Reported once, then left alone: a second sweep must not re-detach a
    // session the supervisor has already taken over.
    let second_sweep = host.take_orphaned_sessions();
    host.kill(session_id).expect("the host must end the shell");
    assert_eq!(orphaned, vec![session_id]);
    assert!(second_sweep.is_empty());
}

#[test]
fn a_session_that_was_never_spawned_cannot_be_attached_or_resized() {
    let mut host = UnixSessionHost::default();
    let unknown = SessionId::from(1234u64);

    assert!(host.lend_endpoint(unknown, std::process::id()).is_err());
    assert!(host.resize(unknown, 80, 24).is_err());
    assert!(!host.is_running(unknown));
}
