use std::collections::HashMap;
use std::ffi::OsString;
use std::path::PathBuf;

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
