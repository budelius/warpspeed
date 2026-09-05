use std::collections::HashMap;
use std::ffi::OsString;
use std::path::PathBuf;
use std::time::Duration;

use instant::Instant;
use serial_test::serial;
use windows::Win32::Foundation::{CloseHandle, HANDLE};

use super::*;
use crate::SizeInfo;
use crate::local_tty::shell::{DirectShellStarter, ShellStarter};
use crate::shell::ShellType;

fn powershell_options() -> PtyOptions {
    PtyOptions {
        size: SizeInfo::new_without_font_metrics(24, 80),
        window_id: None,
        shell_starter: ShellStarter::Direct(DirectShellStarter::new_for_test(
            ShellType::PowerShell,
            PathBuf::from(r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe"),
            vec![
                OsString::from("-NoProfile"),
                OsString::from("-Command"),
                OsString::from("Start-Sleep -Seconds 30"),
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
#[serial]
fn client_reattaches_same_process_through_daemon() {
    std::thread::spawn(|| super::super::supervisor_daemon::run().unwrap());
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(mut transport) = PipeTransport::connect(&supervisor_pipe_name()) {
            transport.send(&Message::Ping).unwrap();
            assert!(matches!(
                transport.receive_timeout(CONTROL_TIMEOUT).unwrap(),
                Message::Pong { .. }
            ));
            transport.send(&Message::ResponseReceived).unwrap();
            break;
        }
        assert!(Instant::now() < deadline, "supervisor pipe did not start");
        std::thread::sleep(Duration::from_millis(25));
    }

    let client = WindowsTerminalSupervisorClient::new().unwrap();
    let session_id = SessionId::from(u64::MAX - 2);
    let first = client.spawn(session_id, &powershell_options()).unwrap();
    let _ = unsafe { CloseHandle(HANDLE(first.raw as usize as *mut _)) };

    let snapshot = ScreenSnapshot {
        columns: 100,
        rows: 30,
        repaint: b"\x1b[H\x1b[2Jclaude".to_vec(),
    };
    let metadata = SessionMetadata {
        shell: "pwsh".to_owned(),
        working_directory: r"C:\src".to_owned(),
        title: "Claude Code".to_owned(),
    };
    client
        .detach(session_id, snapshot.clone(), metadata.clone())
        .unwrap();

    // The listing is what the detached-session picker is built from, so the
    // metadata a client detached with has to survive the round trip.
    let listed = client
        .list_sessions()
        .unwrap()
        .into_iter()
        .find(|session| session.session_id == session_id)
        .expect("a detached session must appear in the listing");
    assert_eq!(listed.metadata, metadata);

    let attached = client.attach(session_id).unwrap();
    let second = attached.endpoint.expect("attach should lend an endpoint");

    assert_eq!(second.pid, first.pid);
    assert_eq!(
        attached.snapshot, snapshot,
        "the screen handed over at detach must come back on attach"
    );
    assert!(client.is_running(session_id).unwrap());

    let _ = unsafe { CloseHandle(HANDLE(second.raw as usize as *mut _)) };
    client.kill(session_id).unwrap();
}
