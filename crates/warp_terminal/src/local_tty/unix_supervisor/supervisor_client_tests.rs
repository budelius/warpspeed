use std::collections::HashMap;
use std::ffi::OsString;
use std::path::PathBuf;
use std::time::Duration;

use instant::Instant;
use serial_test::serial;

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

/// Starts the daemon in this process, once, and waits until it answers.
///
/// Once, because a second attempt races: it flips the process umask inside
/// `bind_listener` before finding the socket already served, and every other
/// test asserting on a file mode is running alongside it.
static DAEMON: std::sync::Once = std::sync::Once::new();

fn start_daemon() {
    DAEMON.call_once(|| {
        std::thread::spawn(|| {
            if let Err(error) = super::super::supervisor_daemon::run() {
                eprintln!("supervisor exited: {error:#}");
            }
        });
    });

    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(mut transport) = UnixSocketTransport::connect(&socket_path()) {
            transport.send(&Message::Ping).unwrap();
            assert!(matches!(
                transport.receive_timeout(CONTROL_TIMEOUT).unwrap(),
                Message::Pong { .. }
            ));
            transport.send(&Message::ResponseReceived).unwrap();
            return;
        }
        assert!(Instant::now() < deadline, "the supervisor never started");
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// The property the whole feature rests on: the shell a client lets go of is
/// the same process it gets back, and the screen it handed over comes with it.
#[test]
#[serial]
fn a_client_reattaches_the_same_process_through_the_daemon() {
    start_daemon();
    let client = UnixTerminalSupervisorClient::new().unwrap();
    let session_id = SessionId::from(u64::MAX - 2);

    let first = client
        .spawn(session_id, &shell_options(&["-c", "sleep 60"]))
        .expect("the supervisor must start a shell");
    // Releasing this process's copy of the lent descriptor; the supervisor
    // keeps its own.
    nix::unistd::close(first.raw as std::os::fd::RawFd).unwrap();

    let snapshot = ScreenSnapshot {
        columns: 100,
        rows: 30,
        repaint: b"\x1b[H\x1b[2Jclaude".to_vec(),
    };
    let metadata = SessionMetadata {
        shell: "sh".to_owned(),
        working_directory: "/tmp".to_owned(),
        title: "Claude Code".to_owned(),
    };
    client
        .detach(session_id, snapshot.clone(), metadata.clone())
        .expect("detaching must succeed");

    // The listing is what the detached-session picker is built from, so the
    // metadata a client detached with has to survive the round trip.
    let listed = client
        .list_sessions()
        .unwrap()
        .into_iter()
        .find(|session| session.session_id == session_id)
        .expect("a detached session must appear in the listing");
    assert_eq!(listed.metadata, metadata);
    assert!(!listed.attached);

    let attached = client.attach(session_id).expect("attaching must succeed");
    let second = attached.endpoint.expect("attach must lend an endpoint");

    assert_eq!(second.pid, first.pid, "the shell must be the same process");
    assert_eq!(
        attached.snapshot, snapshot,
        "the screen handed over at detach must come back on attach"
    );
    assert!(client.is_running(session_id).unwrap());

    nix::unistd::close(second.raw as std::os::fd::RawFd).unwrap();
    client.kill(session_id).unwrap();
}

/// The lent descriptor has to be a working terminal in *this* process, not
/// merely a plausible number: everything the pane types goes through it.
#[test]
#[serial]
fn the_lent_descriptor_is_a_terminal_this_process_can_use() {
    start_daemon();
    let client = UnixTerminalSupervisorClient::new().unwrap();
    let session_id = SessionId::from(u64::MAX - 5);

    let endpoint = client
        .spawn(session_id, &shell_options(&["-c", "cat"]))
        .expect("the supervisor must start a shell");
    let fd = endpoint.raw as std::os::fd::RawFd;

    assert!(
        nix::unistd::isatty(fd).unwrap_or(false),
        "the lent descriptor must be a terminal"
    );
    nix::unistd::write(fd, b"round-trip\n").expect("writing to the lent terminal must succeed");

    let deadline = Instant::now() + Duration::from_secs(10);
    let mut seen = String::new();
    let mut buffer = [0u8; 4096];
    while Instant::now() < deadline && !seen.contains("round-trip") {
        match nix::unistd::read(fd, &mut buffer) {
            Ok(0) | Err(_) => std::thread::sleep(Duration::from_millis(10)),
            Ok(read) => seen.push_str(&String::from_utf8_lossy(&buffer[..read])),
        }
    }

    nix::unistd::close(fd).unwrap();
    client.kill(session_id).unwrap();
    assert!(seen.contains("round-trip"), "read {seen:?}");
}

/// A session another client holds must not be handed to a second one: two
/// readers on one terminal would each see half the output.
#[test]
#[serial]
fn a_session_that_is_already_attached_cannot_be_attached_again() {
    start_daemon();
    let client = UnixTerminalSupervisorClient::new().unwrap();
    let session_id = SessionId::from(u64::MAX - 6);
    let endpoint = client
        .spawn(session_id, &shell_options(&["-c", "sleep 60"]))
        .expect("the supervisor must start a shell");

    let second = client.attach(session_id);

    nix::unistd::close(endpoint.raw as std::os::fd::RawFd).unwrap();
    client.kill(session_id).unwrap();
    assert!(second.is_err(), "a second attach must be refused");
}
