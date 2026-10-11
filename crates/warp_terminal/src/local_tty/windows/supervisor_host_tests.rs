use std::collections::HashMap;
use std::ffi::OsString;
use std::os::windows::io::FromRawHandle;
use std::path::PathBuf;

use mio::windows::NamedPipe;
use mio::{Interest, Poll, Token};
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

fn register(endpoint: Endpoint) {
    let mut pipe = unsafe { NamedPipe::from_raw_handle(endpoint.raw as usize as *mut _) };
    let poll = Poll::new().expect("a completion port should be creatable");
    poll.registry()
        .register(&mut pipe, Token(0), Interest::READABLE)
        .expect("each attachment must have a fresh IOCP-compatible pipe object");
}

#[test]
fn repeated_attachments_use_fresh_pipes_and_preserve_the_child() {
    let mut host = WindowsSessionHost::default();
    let session_id = SessionId::from(u64::MAX - 2);
    let first = host
        .spawn(session_id, powershell_options(), std::process::id())
        .expect("supervisor host should spawn PowerShell");
    let pid = first.pid;
    register(first);
    host.detach(session_id).unwrap();

    for _ in 0..20 {
        let endpoint = host
            .lend_endpoint(session_id, std::process::id())
            .expect("a fresh relay should be attachable");
        assert_eq!(endpoint.pid, pid);
        register(endpoint);
        host.detach(session_id).unwrap();
        assert!(host.is_running(session_id));
    }

    host.kill(session_id).unwrap();
}

#[test]
fn host_owns_process_after_lending_endpoint() {
    let mut host = WindowsSessionHost::default();
    let session_id = SessionId::from(u64::MAX - 1);

    let endpoint = host
        .spawn(session_id, powershell_options(), std::process::id())
        .expect("supervisor host should spawn PowerShell");

    assert!(endpoint.pid > 0);
    assert!(host.is_running(session_id));
    assert!(host.has_sessions());

    let _ = unsafe { CloseHandle(HANDLE(endpoint.raw as usize as *mut _)) };
    host.kill(session_id)
        .expect("supervisor host should terminate PowerShell");
    assert!(!host.has_sessions());
}
