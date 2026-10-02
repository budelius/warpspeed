use std::path::PathBuf;

use warp_core::SessionId;

use super::SessionBootstrap;
use crate::shell::{ShellLaunchData, ShellType};

#[test]
fn an_orphaned_local_shell_is_announced_under_its_supervisor_session_id() {
    let launch = ShellLaunchData::MSYS2 {
        executable_path: PathBuf::from(r"C:\Program Files\Git\usr\bin\bash.exe"),
        shell_type: ShellType::Bash,
    };

    let bootstrap = SessionBootstrap::for_orphaned_local_shell(
        SessionId::from(1258271190020384652u64),
        &launch,
        "budel".to_owned(),
        "workstation".to_owned(),
    )
    .expect("a local shell can be announced");

    assert_eq!(bootstrap.hook_session_id, 1258271190020384652);
    assert_eq!(
        ShellType::from_name(&bootstrap.shell),
        Some(ShellType::Bash)
    );
    assert_eq!(bootstrap.hostname, "workstation");
    assert!(!bootstrap.is_subshell);
}

#[test]
fn a_shell_in_wsl_is_not_guessed() {
    let launch = ShellLaunchData::WSL {
        distro: "Ubuntu-24.04".to_owned(),
    };

    assert!(
        SessionBootstrap::for_orphaned_local_shell(
            SessionId::from(1u64),
            &launch,
            "budel".to_owned(),
            "workstation".to_owned(),
        )
        .is_none()
    );
}
