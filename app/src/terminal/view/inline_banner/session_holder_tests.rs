use warp_core::SessionId;

use super::*;
use crate::app_state::SessionHolder;

fn restore(verdict: HeldVerdict) -> WithheldRestore {
    WithheldRestore {
        command: "claude --resume conversation".to_owned(),
        holder: Some(SessionHolder {
            channel: "old-supervisor".to_owned(),
            version: "old-build".to_owned(),
            pid: 42,
        }),
        session_id: Some(SessionId::from(7)),
        agent: None,
        verdict,
    }
}

#[test]
fn attach_requires_a_known_detached_supervisor_session() {
    let mut held = restore(HeldVerdict::Detached);
    assert_eq!(
        SessionHolderBannerState::new(0, held.clone()).offers_attach(),
        cfg!(windows)
    );
    held.holder = None;
    assert!(!SessionHolderBannerState::new(0, held).offers_attach());
    let mut held = restore(HeldVerdict::Detached);
    held.session_id = None;
    assert!(!SessionHolderBannerState::new(0, held).offers_attach());
    assert!(!SessionHolderBannerState::new(0, restore(HeldVerdict::Attached)).offers_attach());
    assert!(!SessionHolderBannerState::new(0, restore(HeldVerdict::HeldByUnknown)).offers_attach());
}

#[test]
fn plain_shell_recovery_does_not_promise_to_execute_an_empty_command() {
    let mut held = restore(HeldVerdict::Detached);
    held.command.clear();
    let banner = SessionHolderBannerState::new(0, held);
    assert!(banner.body().contains("leaves a fresh shell here"));
    assert!(!banner.body().contains("``"));
}

#[cfg(windows)]
#[test]
fn attach_progress_never_says_the_old_session_is_being_ended() {
    let mut banner = SessionHolderBannerState::new(0, restore(HeldVerdict::Detached));
    banner.set_attaching();
    assert!(banner.in_flight());
    assert_eq!(banner.body(), "Connecting to the existing session…");
    banner.set_attach_error("Supervisor unavailable".to_owned());
    assert!(!banner.in_flight());
    assert_eq!(banner.body(), "Attaching failed: Supervisor unavailable");
    assert!(banner.offers_attach());
    banner.set_in_flight();
    assert_eq!(banner.body(), "Ending the session there…");
}
