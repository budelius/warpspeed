use warp_cli::agent_event::AgentEventFormat;

use super::notification_json;
use crate::remote_control::protocol::MAX_CLI_AGENT_EVENT_BYTES;
use crate::terminal::CLIAgent;
use crate::terminal::cli_agent_sessions::event::{
    CLI_AGENT_NOTIFICATION_SENTINEL, CLIAgentEventType, parse_event,
};

#[test]
fn protocol_input_passes_through_unchanged_in_meaning() {
    let input = r#"{"v":1,"agent":"dsh","event":"tool_complete","session_id":"session-1","tool_name":"bash","tool_input":{"command":"ls"},"plugin_version":"0.1.0"}"#;

    let json = notification_json(AgentEventFormat::Protocol, input, None).expect("valid input");

    let event = parse_event(Some(CLI_AGENT_NOTIFICATION_SENTINEL), &json).expect("Warp parses it");
    assert_eq!(event.agent, CLIAgent::DeepSeekHarness);
    assert_eq!(event.event, CLIAgentEventType::ToolComplete);
    assert_eq!(event.session_id.as_deref(), Some("session-1"));
    assert_eq!(event.payload.tool_input_preview.as_deref(), Some("ls"));
    assert_eq!(event.payload.plugin_version.as_deref(), Some("0.1.0"));
}

#[test]
fn kimi_hook_input_is_mapped_with_its_hooks_version() {
    let input = r#"{"hook_event_name":"Stop","session_id":"session_x","cwd":"/p","client_type":"kimi_code_cli","stop_hook_active":false}"#;

    let json = notification_json(AgentEventFormat::KimiHook, input, Some("1.0.0")).unwrap();

    let event = parse_event(Some(CLI_AGENT_NOTIFICATION_SENTINEL), &json).unwrap();
    assert_eq!(event.agent, CLIAgent::Kimi);
    assert_eq!(event.event, CLIAgentEventType::Stop);
    assert_eq!(event.payload.plugin_version.as_deref(), Some("1.0.0"));
}

#[test]
fn malformed_or_untracked_input_sends_nothing() {
    assert!(notification_json(AgentEventFormat::Protocol, "not json", None).is_none());
    assert!(notification_json(AgentEventFormat::Protocol, r#"{"v":1}"#, None).is_none());
    assert!(notification_json(AgentEventFormat::KimiHook, "{", None).is_none());
    assert!(
        notification_json(
            AgentEventFormat::KimiHook,
            r#"{"hook_event_name":"PreToolUse","session_id":"s"}"#,
            None
        )
        .is_none()
    );
}

#[test]
fn oversized_text_is_cut_to_fit_the_limit() {
    let query = format!("Fix the build{}", "x".repeat(200 * 1024));
    let input =
        serde_json::json!({"v": 1, "agent": "dsh", "event": "prompt_submit", "query": query})
            .to_string();

    let json = notification_json(AgentEventFormat::Protocol, &input, None).expect("still sent");

    assert!(json.len() <= MAX_CLI_AGENT_EVENT_BYTES, "{}", json.len());
    let event = parse_event(Some(CLI_AGENT_NOTIFICATION_SENTINEL), &json).unwrap();
    assert_eq!(event.event, CLIAgentEventType::PromptSubmit);
    assert!(event.payload.query.unwrap().starts_with("Fix the build"));
}

#[test]
fn oversized_tool_input_is_left_out() {
    let input = serde_json::json!({
        "v": 1,
        "agent": "dsh",
        "event": "tool_complete",
        "tool_name": "write",
        "tool_input": {"content": "y".repeat(200 * 1024)}
    })
    .to_string();

    let json = notification_json(AgentEventFormat::Protocol, &input, None).expect("still sent");

    assert!(json.len() <= MAX_CLI_AGENT_EVENT_BYTES, "{}", json.len());
    let event = parse_event(Some(CLI_AGENT_NOTIFICATION_SENTINEL), &json).unwrap();
    assert_eq!(event.payload.tool_name.as_deref(), Some("write"));
    assert_eq!(event.payload.tool_input_preview, None);
}

#[test]
fn a_notification_that_cannot_be_cut_to_fit_sends_nothing() {
    let input = serde_json::json!({
        "v": 1,
        "agent": "dsh",
        "event": "stop",
        "session_id": "s".repeat(100 * 1024)
    })
    .to_string();
    assert!(notification_json(AgentEventFormat::Protocol, &input, None).is_none());
}
