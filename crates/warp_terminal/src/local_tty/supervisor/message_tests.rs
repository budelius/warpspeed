use std::path::PathBuf;

use warp_core::SessionId;

use super::{SessionBootstrap, SessionMetadata};
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

/// A session the supervisor recovered from a client that died, or one handed over by a pane that
/// never attached it, carries no metadata. Reading its default `input_editor_active: false` as
/// "a program is running" opened an unnamed block that no prompt ever closed.
#[test]
fn an_adopted_shell_with_no_handover_is_at_its_prompt_unless_it_runs_a_program() {
    let unknown = SessionMetadata::default();

    assert!(unknown.adopted_shell_is_at_prompt(false));
    assert!(!unknown.adopted_shell_is_at_prompt(true));
}

#[test]
fn an_adopted_shell_is_where_its_handover_says_unless_it_runs_a_program() {
    let handed_over = |input_editor_active| SessionMetadata {
        input_editor_active,
        bootstrap: Some(SessionBootstrap::default()),
        ..SessionMetadata::default()
    };

    assert!(handed_over(true).adopted_shell_is_at_prompt(false));
    assert!(!handed_over(false).adopted_shell_is_at_prompt(false));
    assert!(!handed_over(true).adopted_shell_is_at_prompt(true));
}
