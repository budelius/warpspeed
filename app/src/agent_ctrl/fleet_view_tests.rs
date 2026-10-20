use super::*;

fn row(state: &str, pause_allowed: bool) -> FleetRow {
    FleetRow {
        job_id: "job-1".to_owned(),
        state: state.to_owned(),
        agent: "staging-claude-interactive-rw".to_owned(),
        node_name: Some("agent-ctrl-node-0".to_owned()),
        pause_allowed,
        resume_allowed: false,
        branch: None,
        branch_url: None,
        last_activity: None,
    }
}

#[test]
fn running_rows_offer_pause_and_paused_rows_offer_resume() {
    assert_eq!(pause_control(&row("running", true)), PauseControl::Pause);
    assert_eq!(pause_control(&row("paused", true)), PauseControl::Resume);
    assert_eq!(
        pause_control(&row("running", false)),
        PauseControl::Unavailable
    );
    assert_eq!(
        pause_control(&row("waiting_input", true)),
        PauseControl::Unavailable
    );
    assert_eq!(
        pause_control(&row("completed", true)),
        PauseControl::Unavailable
    );
}
