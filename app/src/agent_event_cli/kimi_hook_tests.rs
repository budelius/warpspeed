use std::fs;

use serde_json::{Value, json};

use super::to_notification;
use crate::terminal::CLIAgent;
use crate::terminal::cli_agent_sessions::event::{
    CLI_AGENT_NOTIFICATION_SENTINEL, CLIAgentEvent, CLIAgentEventType, parse_event,
};

const SESSION_ID: &str = "session_0b6c1234-ffff-4a4a-8b8b-0123456789ab";

/// A hook payload as Kimi writes it to the hook's stdin, with `extra` merged in.
fn hook(name: &str, extra: Value) -> Value {
    let mut payload = json!({
        "hook_event_name": name,
        "session_id": SESSION_ID,
        "cwd": r"C:\Users\me\proj",
        "client_type": "kimi_code_cli",
        "session_title": "Fix the build",
    });
    payload
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    payload
}

/// What Warp makes of the notification a hook produces, as if it had arrived over OSC 777.
fn parsed(payload: &Value) -> CLIAgentEvent {
    let notification = to_notification(payload, Some("1.0.0"), None).expect("the hook is tracked");
    parse_event(
        Some(CLI_AGENT_NOTIFICATION_SENTINEL),
        &serde_json::to_string(&notification).unwrap(),
    )
    .expect("Warp parses the notification")
}

#[test]
fn session_start_reports_the_session_its_directory_and_the_hooks_version() {
    let home = tempfile::tempdir().unwrap();
    let dir = home
        .path()
        .join("sessions")
        .join("wd_proj_4045ff9b01fe")
        .join(SESSION_ID);
    fs::create_dir_all(&dir).unwrap();
    let line = json!({"sessionId": SESSION_ID, "sessionDir": dir, "workDir": "C:/Users/me/proj"});
    fs::write(home.path().join("session_index.jsonl"), format!("{line}\n")).unwrap();
    let payload = hook(
        "SessionStart",
        json!({"source": "startup", "model": "kimi-k2", "profile": "default"}),
    );

    let notification = to_notification(&payload, Some("1.0.0"), Some(home.path())).unwrap();

    assert_eq!(notification.agent.as_deref(), Some("kimi"));
    assert_eq!(notification.event, "session_start");
    assert_eq!(notification.session_id.as_deref(), Some(SESSION_ID));
    assert_eq!(notification.cwd.as_deref(), Some(r"C:\Users\me\proj"));
    assert_eq!(
        notification.transcript_path,
        Some(dir.display().to_string())
    );
    assert_eq!(notification.plugin_version.as_deref(), Some("1.0.0"));
    let event = parsed(&payload);
    assert_eq!(event.agent, CLIAgent::Kimi);
    assert_eq!(event.event, CLIAgentEventType::SessionStart);
}

#[test]
fn session_start_without_an_index_finds_the_directory_by_kimis_naming() {
    let home = tempfile::tempdir().unwrap();
    let dir = home
        .path()
        .join("sessions")
        .join("wd_proj_4045ff9b01fe")
        .join(SESSION_ID);
    fs::create_dir_all(&dir).unwrap();

    let notification = to_notification(
        &hook("SessionStart", json!({"source": "resume"})),
        None,
        Some(home.path()),
    )
    .unwrap();

    assert_eq!(
        notification.transcript_path,
        Some(dir.display().to_string())
    );
}

#[test]
fn session_start_omits_a_directory_that_does_not_exist() {
    let home = tempfile::tempdir().unwrap();
    let notification = to_notification(
        &hook("SessionStart", json!({"source": "startup"})),
        None,
        Some(home.path()),
    )
    .unwrap();
    assert_eq!(notification.transcript_path, None);
}

#[test]
fn prompt_submit_joins_the_prompts_text_parts_and_leaves_the_cwd_out() {
    let event = parsed(&hook(
        "UserPromptSubmit",
        json!({
            "prompt": [
                {"type": "text", "text": "Fix the build"},
                {"type": "image_url", "image_url": {"url": "data:image/png;base64,AAAA"}},
                {"type": "text", "text": "and the tests"}
            ],
            "is_steer": false
        }),
    ));
    assert_eq!(event.event, CLIAgentEventType::PromptSubmit);
    assert_eq!(
        event.payload.query.as_deref(),
        Some("Fix the build\nand the tests")
    );
    assert_eq!(event.cwd, None);
}

#[test]
fn permission_request_carries_the_tool_and_a_summary() {
    let event = parsed(&hook(
        "PermissionRequest",
        json!({
            "id": "approval_1",
            "agent_id": "main",
            "turn_id": 3,
            "tool_call_id": "call_1",
            "tool_name": "Shell",
            "action": "run command",
            "display": {"kind": "command", "command": "cargo test", "description": "Run the tests"},
            "tool_input": {"command": "cargo test"}
        }),
    ));
    assert_eq!(event.event, CLIAgentEventType::PermissionRequest);
    assert_eq!(event.payload.tool_name.as_deref(), Some("Shell"));
    assert_eq!(event.payload.summary.as_deref(), Some("Run the tests"));
    assert_eq!(
        event.payload.tool_input_preview.as_deref(),
        Some("cargo test")
    );
}

#[test]
fn permission_summary_falls_back_through_the_display_to_the_action() {
    let file = parsed(&hook(
        "PermissionRequest",
        json!({
            "tool_name": "WriteFile",
            "action": "write file",
            "display": {"kind": "file_io", "operation": "write", "path": "src/main.rs"},
            "tool_input": {"path": "src/main.rs"}
        }),
    ));
    assert_eq!(file.payload.summary.as_deref(), Some("src/main.rs"));
    let todo = parsed(&hook(
        "PermissionRequest",
        json!({"tool_name": "Todo", "action": "update todos", "display": {"kind": "todo_list", "items": []}}),
    ));
    assert_eq!(todo.payload.summary.as_deref(), Some("update todos"));
}

#[test]
fn permission_result_tool_use_stop_and_interrupt_map_directly() {
    assert_eq!(
        parsed(&hook(
            "PermissionResult",
            json!({"tool_name": "Shell", "decision": "approved"})
        ))
        .event,
        CLIAgentEventType::PermissionReplied
    );
    let tool = parsed(&hook(
        "PostToolUse",
        json!({"tool_name": "Shell", "tool_input": {"command": "ls"}, "tool_call_id": "call_2", "tool_output": "a\nb"}),
    ));
    assert_eq!(tool.event, CLIAgentEventType::ToolComplete);
    assert_eq!(tool.payload.tool_name.as_deref(), Some("Shell"));
    assert_eq!(
        parsed(&hook("Stop", json!({"stop_hook_active": false}))).event,
        CLIAgentEventType::Stop
    );
    assert_eq!(
        parsed(&hook(
            "Interrupt",
            json!({"turn_id": 3, "reason": "cancelled"})
        ))
        .event,
        CLIAgentEventType::IdlePrompt
    );
}

#[test]
fn stop_failure_carries_the_error() {
    let event = parsed(&hook(
        "StopFailure",
        json!({"error_type": "RateLimitError", "error_message": "429 Too Many Requests"}),
    ));
    assert_eq!(event.event, CLIAgentEventType::StopFailure);
    assert_eq!(event.payload.error_type.as_deref(), Some("RateLimitError"));
    assert_eq!(
        event.payload.response.as_deref(),
        Some("429 Too Many Requests")
    );
}

#[test]
fn untracked_hooks_send_nothing() {
    for name in [
        "SessionEnd",
        "PreToolUse",
        "PostToolUseFailure",
        "UserPromptQueued",
        "TurnStarted",
        "SessionHeartbeat",
        "SubagentStart",
        "SubagentStop",
        "TaskStarted",
        "PreCompact",
        "PostCompact",
        "Notification",
    ] {
        assert!(
            to_notification(&hook(name, json!({})), None, None).is_none(),
            "{name}"
        );
    }
    assert!(to_notification(&json!({"session_id": SESSION_ID}), None, None).is_none());
}
