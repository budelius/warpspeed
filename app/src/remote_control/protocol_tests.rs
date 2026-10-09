use serde::Serialize;
use serde::de::DeserializeOwned;

use super::*;

const TOKEN: &str = "0123456789abcdef0123456789abcdef";

/// Round-trips `value` through bincode, the encoding the channel uses, and compares the debug
/// form, since the protocol types deliberately do not implement `PartialEq`.
fn assert_round_trips<T: Serialize + DeserializeOwned + std::fmt::Debug>(value: T) {
    let bytes = bincode::serialize(&value).expect("serializes");
    let decoded: T = bincode::deserialize(&bytes).expect("deserializes");
    assert_eq!(format!("{decoded:?}"), format!("{value:?}"));
}

#[test]
fn protocol_version_is_three() {
    assert_eq!(PROTOCOL_VERSION, 3);
}

#[test]
fn cli_agent_event_round_trips() {
    let request = Request::CliAgentEvent {
        token: AgentEventToken::from(TOKEN.to_owned()),
        notification: r#"{"v":1,"agent":"kimi","event":"stop"}"#.to_owned(),
    };
    let bytes = bincode::serialize(&request).expect("serializes");
    let decoded: Request = bincode::deserialize(&bytes).expect("deserializes");
    let Request::CliAgentEvent {
        token,
        notification,
    } = decoded
    else {
        panic!("expected a CLI agent event, got {decoded:?}");
    };
    assert_eq!(token.as_str(), TOKEN);
    assert_eq!(notification, r#"{"v":1,"agent":"kimi","event":"stop"}"#);
}

#[test]
fn cli_agent_event_token_is_left_out_of_debug_output() {
    let request = Request::CliAgentEvent {
        token: AgentEventToken::from(TOKEN.to_owned()),
        notification: r#"{"v":1,"agent":"kimi","event":"stop"}"#.to_owned(),
    };
    let debug = format!("{request:?}");
    assert!(!debug.contains(TOKEN), "{debug}");
    assert!(debug.contains("CliAgentEvent"), "{debug}");
    assert!(!format!("{request:#?}").contains(TOKEN));
}

#[test]
fn agent_event_token_serializes_as_a_plain_string() {
    assert_eq!(
        bincode::serialize(&AgentEventToken::from(TOKEN.to_owned())).unwrap(),
        bincode::serialize(TOKEN).unwrap()
    );
}

#[test]
fn move_requests_round_trip() {
    assert_round_trips(Request::MoveSessionToAgentCtrl {
        terminal_view_id: 42,
        cwd: None,
    });
    assert_round_trips(Request::MoveSessionToAgentCtrl {
        terminal_view_id: 42,
        cwd: Some(r"T:\projects\warp".to_owned()),
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
            pushed_branch: None,
        });
    }
    assert_round_trips(Response::MoveState {
        step: "done:job-1".to_owned(),
        pushed_branch: Some("gitea/agentctrl/handoff-1".to_owned()),
    });
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
        agent_not_resumed: false,
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
                ..pane.clone()
            },
            PaneSummary {
                working_directory: String::new(),
                has_running_agent: false,
                agent_not_resumed: true,
                ..pane
            },
        ],
    });
}
