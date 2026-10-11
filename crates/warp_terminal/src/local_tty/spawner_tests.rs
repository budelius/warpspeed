use anyhow::anyhow;
use warp_core::SessionId;

use super::skip_ended_session;

#[test]
fn a_session_that_has_ended_does_not_stop_the_handover() {
    let result = skip_ended_session(SessionId::from(7u64), anyhow!("was not found"), |_| {
        Ok(false)
    });

    assert!(result.is_ok());
}

#[test]
fn a_session_the_supervisor_no_longer_knows_does_not_stop_the_handover() {
    let result = skip_ended_session(SessionId::from(7u64), anyhow!("was not found"), |_| {
        Err(anyhow!("Terminal session SessionId(7) was not found"))
    });

    assert!(result.is_ok());
}

#[test]
fn a_running_session_that_could_not_be_handed_over_fails_the_handover() {
    let result = skip_ended_session(SessionId::from(7u64), anyhow!("pipe broken"), |_| Ok(true));

    assert_eq!(result.unwrap_err().to_string(), "pipe broken");
}
