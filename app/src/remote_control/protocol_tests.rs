use serde::Serialize;
use serde::de::DeserializeOwned;

use super::*;

/// Round-trips `value` through bincode, the encoding the channel uses, and compares the debug
/// form, since the protocol types deliberately do not implement `PartialEq`.
fn assert_round_trips<T: Serialize + DeserializeOwned + std::fmt::Debug>(value: T) {
    let bytes = bincode::serialize(&value).expect("serializes");
    let decoded: T = bincode::deserialize(&bytes).expect("deserializes");
    assert_eq!(format!("{decoded:?}"), format!("{value:?}"));
}

#[test]
fn protocol_version_is_two() {
    assert_eq!(PROTOCOL_VERSION, 2);
}

#[test]
fn move_requests_round_trip() {
    assert_round_trips(Request::MoveSessionToAgentCtrl {
        terminal_view_id: 42,
    });
    assert_round_trips(Request::MoveStatus {
        terminal_view_id: u64::MAX,
    });
}

#[test]
fn move_responses_round_trip() {
    assert_round_trips(Response::MoveStarted);
    for step in [
        "preparing",
        "importing",
        "waiting_for_resume",
        "cutting_over",
        "done:job-1",
        "failed:the remote rejected the push",
        "none",
    ] {
        assert_round_trips(Response::MoveState {
            step: step.to_owned(),
        });
    }
}

#[test]
fn pane_summary_with_agent_details_round_trips() {
    let pane = PaneSummary {
        session_id: Some(7),
        window: "1".to_owned(),
        tab_index: 0,
        title: "Fix the build".to_owned(),
        shell: "pwsh".to_owned(),
        working_directory: "/srv/app".to_owned(),
        has_running_agent: true,
        is_busy: false,
        survives_quit: true,
        terminal_view_id: Some(1234),
        agent: Some("claude".to_owned()),
        agent_session_id: Some("0b6c1234-ffff-4a4a-8b8b-0123456789ab".to_owned()),
        agent_cwd: Some("/srv/app/sub".to_owned()),
        transcript_path: Some("/home/ben/.claude/projects/-srv-app/x.jsonl".to_owned()),
    };
    assert_round_trips(Response::Panes {
        panes: vec![
            pane.clone(),
            PaneSummary {
                terminal_view_id: None,
                agent: None,
                agent_session_id: None,
                agent_cwd: None,
                transcript_path: None,
                ..pane
            },
        ],
    });
}
