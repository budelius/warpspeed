use std::collections::HashMap;
use std::ffi::OsString;
use std::path::PathBuf;
use std::time::Duration;

use instant::Instant;

use super::*;
use crate::SizeInfo;
use crate::local_tty::shell::{DirectShellStarter, ShellStarter};
use crate::shell::ShellType;

/// A shell running `args`, with everything else at its least surprising value.
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

/// Reads until `needle` shows up or the deadline passes, so the assertion does
/// not depend on how the shell's output happens to be chunked.
fn read_until(pty: &mut SupervisedPty, needle: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut seen = String::new();
    let mut buffer = [0u8; 4096];
    while Instant::now() < deadline {
        let read = pty.read(&mut buffer).expect("reading must not fail");
        if read == 0 {
            std::thread::sleep(Duration::from_millis(10));
            continue;
        }
        seen.push_str(&String::from_utf8_lossy(&buffer[..read]));
        if seen.contains(needle) {
            return seen;
        }
    }
    seen
}

#[test]
fn a_supervised_pty_runs_a_shell_and_produces_its_output() {
    let mut pty = SupervisedPty::spawn(shell_options(&["-c", "echo supervised-hello"]))
        .expect("the shell must start");

    let output = read_until(&mut pty, "supervised-hello");

    assert!(output.contains("supervised-hello"), "read {output:?}");
}

/// The supervisor's loop is single-threaded: a terminal with nothing to say
/// must not park it.
#[test]
fn reading_an_idle_pty_returns_nothing_instead_of_blocking() {
    let mut pty =
        SupervisedPty::spawn(shell_options(&["-c", "sleep 30"])).expect("the shell must start");
    // Let the shell get past whatever it prints on startup.
    let _ = read_until(&mut pty, "\u{0}");

    let mut buffer = [0u8; 1024];
    let started = Instant::now();
    let read = pty.read(&mut buffer).expect("reading must not fail");

    assert_eq!(read, 0);
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "an idle read took {:?}",
        started.elapsed()
    );
}

#[test]
fn a_supervised_pty_reports_when_its_shell_exits() {
    let mut pty = SupervisedPty::spawn(shell_options(&["-c", "exit 0"])).expect("shell must start");

    let deadline = Instant::now() + Duration::from_secs(10);
    while pty.is_running() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }

    assert!(!pty.is_running());
}

#[test]
fn killing_a_supervised_pty_ends_its_shell() {
    let mut pty = SupervisedPty::spawn(shell_options(&["-c", "sleep 60"])).expect("shell starts");
    assert!(pty.is_running());

    pty.kill().expect("killing must succeed");

    assert!(!pty.is_running());
}

/// The leader is what gets lent to a client, so it has to be a real descriptor
/// this process owns rather than a copy that dies with a temporary.
#[test]
fn a_supervised_pty_lends_a_usable_leader_descriptor() {
    let pty = SupervisedPty::spawn(shell_options(&["-c", "sleep 30"])).expect("shell starts");

    let fd = pty.leader_fd();

    assert!(fd >= 0);
    assert!(
        nix::fcntl::fcntl(fd, nix::fcntl::F_GETFD).is_ok(),
        "the lent descriptor must still be open in this process"
    );
}
