use super::*;
use crate::terminal::cli_agent_sessions::event::{
    CLI_AGENT_NOTIFICATION_SENTINEL, CLIAgentEventSource, CLIAgentEventType,
};

#[test]
fn codex_parses_any_text_as_stop() {
    let event = CodexSessionHandler::parse_osc9_text("Agent turn complete").unwrap();
    assert_eq!(event.event, CLIAgentEventType::Stop);
    assert_eq!(event.agent, CLIAgent::Codex);
    assert_eq!(event.payload.query.as_deref(), Some("Agent turn complete"));
}

#[test]
fn codex_body_becomes_query() {
    let event =
        CodexSessionHandler::parse_osc9_text("I've updated the README with the new instructions.")
            .unwrap();
    assert_eq!(event.event, CLIAgentEventType::Stop);
    assert_eq!(
        event.payload.query.as_deref(),
        Some("I've updated the README with the new instructions.")
    );
}

#[test]
fn codex_approval_text_still_becomes_stop() {
    let event =
        CodexSessionHandler::parse_osc9_text("Approval requested: rm -rf /tmp/foo").unwrap();
    assert_eq!(event.event, CLIAgentEventType::Stop);
    assert_eq!(
        event.payload.query.as_deref(),
        Some("Approval requested: rm -rf /tmp/foo")
    );
}

#[test]
fn codex_ignores_empty_body() {
    assert!(CodexSessionHandler::parse_osc9_text("").is_none());
    assert!(CodexSessionHandler::parse_osc9_text("   ").is_none());
}

#[test]
fn codex_try_parse_ignores_titled_notifications() {
    let mut handler = CodexSessionHandler;
    assert!(
        handler
            .try_parse(Some("some-title"), "Agent turn complete", false)
            .is_none()
    );
}

#[test]
fn codex_try_parse_handles_osc9() {
    let mut handler = CodexSessionHandler;
    let event = handler
        .try_parse(None, "Agent turn complete", false)
        .unwrap();
    assert_eq!(event.event, CLIAgentEventType::Stop);
}

#[test]
fn codex_try_parse_ignores_osc9_when_plugin_already_active() {
    let _guard = FeatureFlag::CodexPlugin.override_enabled(true);
    let mut handler = CodexSessionHandler;
    let body = r#"{"v":1,"agent":"codex","event":"permission_request","summary":"Approve?","tool_name":"Bash"}"#;

    let event = handler
        .try_parse(Some(CLI_AGENT_NOTIFICATION_SENTINEL), body, false)
        .unwrap();

    assert_eq!(event.event, CLIAgentEventType::PermissionRequest);
    // Once the session is rich, OSC 9 fallback is dropped.
    assert!(
        handler
            .try_parse(None, "Agent turn complete", true)
            .is_none()
    );
}

#[test]
fn codex_try_parse_ignores_structured_event_without_codex_plugin() {
    let _guard = FeatureFlag::CodexPlugin.override_enabled(false);
    let mut handler = CodexSessionHandler;
    let body = r#"{"v":1,"agent":"codex","event":"permission_request","summary":"Approve?","tool_name":"Bash"}"#;

    assert!(
        handler
            .try_parse(Some(CLI_AGENT_NOTIFICATION_SENTINEL), body, false)
            .is_none()
    );
    assert!(
        handler
            .try_parse(None, "Agent turn complete", false)
            .is_some()
    );
}

#[test]
fn codex_try_parse_ignores_other_structured_agents() {
    let mut handler = CodexSessionHandler;
    let body = r#"{"v":1,"agent":"claude","event":"stop"}"#;

    assert!(
        handler
            .try_parse(Some(CLI_AGENT_NOTIFICATION_SENTINEL), body, false)
            .is_none()
    );
    assert!(
        handler
            .try_parse(None, "Agent turn complete", false)
            .is_some()
    );
}

#[test]
fn auggie_is_supported() {
    assert!(is_agent_supported(&CLIAgent::Auggie));
}

#[test]
fn auggie_default_handler_skips_session_start() {
    let mut handler = DefaultSessionListener;
    let event = CLIAgentEvent {
        source: CLIAgentEventSource::RichPlugin,
        v: 1,
        agent: CLIAgent::Auggie,
        event: CLIAgentEventType::SessionStart,
        session_id: None,
        cwd: None,
        project: None,
        payload: CLIAgentEventPayload::default(),
    };
    assert!(handler.handle_event(event).is_none());
}

#[test]
fn auggie_default_handler_forwards_stop() {
    let mut handler = DefaultSessionListener;
    let event = CLIAgentEvent {
        source: CLIAgentEventSource::RichPlugin,
        v: 1,
        agent: CLIAgent::Auggie,
        event: CLIAgentEventType::Stop,
        session_id: None,
        cwd: None,
        project: None,
        payload: CLIAgentEventPayload::default(),
    };
    assert!(handler.handle_event(event).is_some());
}

#[test]
fn pi_is_supported() {
    assert!(is_agent_supported(&CLIAgent::Pi));
}

#[test]
fn oh_my_pi_is_supported() {
    assert!(is_agent_supported(&CLIAgent::OhMyPi));
}

#[test]
fn pi_default_handler_skips_session_start() {
    let mut handler = DefaultSessionListener;
    let event = CLIAgentEvent {
        source: CLIAgentEventSource::RichPlugin,
        v: 1,
        agent: CLIAgent::Pi,
        event: CLIAgentEventType::SessionStart,
        session_id: None,
        cwd: None,
        project: None,
        payload: CLIAgentEventPayload::default(),
    };
    assert!(handler.handle_event(event).is_none());
}

#[test]
fn pi_default_handler_forwards_stop() {
    let mut handler = DefaultSessionListener;
    let event = CLIAgentEvent {
        source: CLIAgentEventSource::RichPlugin,
        v: 1,
        agent: CLIAgent::Pi,
        event: CLIAgentEventType::Stop,
        session_id: None,
        cwd: None,
        project: None,
        payload: CLIAgentEventPayload::default(),
    };
    assert!(handler.handle_event(event).is_some());
}

#[test]
fn droid_is_supported() {
    assert!(is_agent_supported(&CLIAgent::Droid));
}

#[test]
fn droid_default_handler_skips_session_start() {
    let mut handler = DefaultSessionListener;
    let event = CLIAgentEvent {
        source: CLIAgentEventSource::RichPlugin,
        v: 1,
        agent: CLIAgent::Droid,
        event: CLIAgentEventType::SessionStart,
        session_id: None,
        cwd: None,
        project: None,
        payload: CLIAgentEventPayload::default(),
    };
    assert!(handler.handle_event(event).is_none());
}

#[test]
fn droid_default_handler_forwards_stop() {
    let mut handler = DefaultSessionListener;
    let event = CLIAgentEvent {
        source: CLIAgentEventSource::RichPlugin,
        v: 1,
        agent: CLIAgent::Droid,
        event: CLIAgentEventType::Stop,
        session_id: None,
        cwd: None,
        project: None,
        payload: CLIAgentEventPayload::default(),
    };
    assert!(handler.handle_event(event).is_some());
}

#[test]
fn droid_default_handler_forwards_permission_request() {
    let mut handler = DefaultSessionListener;
    let event = CLIAgentEvent {
        source: CLIAgentEventSource::RichPlugin,
        v: 1,
        agent: CLIAgent::Droid,
        event: CLIAgentEventType::PermissionRequest,
        session_id: None,
        cwd: None,
        project: None,
        payload: CLIAgentEventPayload::default(),
    };
    assert!(handler.handle_event(event).is_some());
}

#[test]
fn warp_tui_notifications_are_supported() {
    assert!(is_agent_supported(&CLIAgent::WarpTui));
    let mut handler = create_handler(&CLIAgent::WarpTui).expect("should create handler");
    let stop_body = r#"{"v":1,"agent":"warp-tui","event":"stop","session_id":"sess-42"}"#;
    let parsed_stop = handler
        .try_parse(Some(CLI_AGENT_NOTIFICATION_SENTINEL), stop_body, false)
        .expect("should parse stop");
    assert_eq!(parsed_stop.agent, CLIAgent::WarpTui);
    assert_eq!(parsed_stop.event, CLIAgentEventType::Stop);
    assert!(handler.handle_event(parsed_stop).is_some());
}

#[test]
fn oh_my_pi_end_to_end_parsing_and_handling() {
    let mut handler = create_handler(&CLIAgent::OhMyPi).expect("should create handler");

    // Test session_start payload: proves SessionStart is skipped
    let start_body = r#"{"v":1,"agent":"omp","event":"session_start"}"#;
    let parsed_start = handler
        .try_parse(Some(CLI_AGENT_NOTIFICATION_SENTINEL), start_body, false)
        .expect("should successfully parse session_start payload");
    assert_eq!(parsed_start.agent, CLIAgent::OhMyPi);
    assert_eq!(parsed_start.event, CLIAgentEventType::SessionStart);
    assert!(handler.handle_event(parsed_start).is_none());

    // Test stop payload: proves Stop forwards with CLIAgent::OhMyPi
    let stop_body = r#"{"v":1,"agent":"omp","event":"stop"}"#;
    let parsed_stop = handler
        .try_parse(Some(CLI_AGENT_NOTIFICATION_SENTINEL), stop_body, false)
        .expect("should successfully parse stop payload");
    assert_eq!(parsed_stop.agent, CLIAgent::OhMyPi);
    assert_eq!(parsed_stop.event, CLIAgentEventType::Stop);

    let handled_stop = handler
        .handle_event(parsed_stop)
        .expect("should forward stop event");
    assert_eq!(handled_stop.agent, CLIAgent::OhMyPi);
    assert_eq!(handled_stop.event, CLIAgentEventType::Stop);
}

#[test]
fn kimi_and_deepseek_harness_are_supported() {
    assert!(is_agent_supported(&CLIAgent::Kimi));
    assert!(is_agent_supported(&CLIAgent::DeepSeekHarness));
    assert!(create_handler(&CLIAgent::Kimi).is_some());
    assert!(create_handler(&CLIAgent::DeepSeekHarness).is_some());
}

#[test]
fn deepseek_harness_end_to_end_parsing_and_handling() {
    let mut handler = create_handler(&CLIAgent::DeepSeekHarness).expect("should create handler");
    let start = handler
        .try_parse(
            Some(CLI_AGENT_NOTIFICATION_SENTINEL),
            r#"{"v":1,"agent":"dsh","event":"session_start","session_id":"0b6c1234-ffff-4a4a-8b8b-0123456789ab"}"#,
            false,
        )
        .expect("session_start parses");
    assert_eq!(start.agent, CLIAgent::DeepSeekHarness);
    assert!(handler.handle_event(start).is_none());

    let stop = handler
        .try_parse(
            Some(CLI_AGENT_NOTIFICATION_SENTINEL),
            r#"{"v":1,"agent":"dsh","event":"stop"}"#,
            false,
        )
        .expect("stop parses");
    let handled = handler.handle_event(stop).expect("stop is forwarded");
    assert_eq!(handled.agent, CLIAgent::DeepSeekHarness);
    assert_eq!(handled.event, CLIAgentEventType::Stop);
}

#[test]
fn kimi_osc9_titles_map_to_their_events() {
    let done = KimiSessionHandler::parse_osc9_text("Kimi Code task complete: Fix the reboot bug")
        .expect("task complete parses");
    assert_eq!(done.agent, CLIAgent::Kimi);
    assert_eq!(done.event, CLIAgentEventType::Stop);
    assert_eq!(done.payload.query.as_deref(), Some("Fix the reboot bug"));
    assert_eq!(done.source, CLIAgentEventSource::KimiOsc9Fallback);

    let approval = KimiSessionHandler::parse_osc9_text("Kimi Code approval required: Shell")
        .expect("approval parses");
    assert_eq!(approval.event, CLIAgentEventType::PermissionRequest);
    assert_eq!(approval.payload.tool_name.as_deref(), Some("Shell"));
    assert_eq!(approval.payload.summary.as_deref(), Some("Shell"));

    let question =
        KimiSessionHandler::parse_osc9_text("Kimi Code needs your answer: Which branch?")
            .expect("question parses");
    assert_eq!(question.event, CLIAgentEventType::QuestionAsked);
    assert_eq!(question.payload.summary.as_deref(), Some("Which branch?"));

    let untitled = KimiSessionHandler::parse_osc9_text("Kimi Code task complete")
        .expect("a title alone parses");
    assert_eq!(untitled.event, CLIAgentEventType::Stop);
    assert_eq!(
        untitled.payload.query.as_deref(),
        Some("Kimi Code task complete")
    );
    assert!(KimiSessionHandler::parse_osc9_text("   ").is_none());
}

#[test]
fn kimi_handler_takes_hook_events_and_forwards_session_start() {
    let mut handler = create_handler(&CLIAgent::Kimi).expect("should create handler");
    let start = handler
        .try_parse(
            Some(CLI_AGENT_NOTIFICATION_SENTINEL),
            r#"{"v":1,"agent":"kimi","event":"session_start","session_id":"session_0b6c1234-ffff-4a4a-8b8b-0123456789ab","plugin_version":"1.0.0"}"#,
            false,
        )
        .expect("session_start parses");
    let forwarded = handler.handle_event(start).expect("session_start is forwarded");
    assert_eq!(forwarded.event, CLIAgentEventType::SessionStart);
    assert_eq!(
        forwarded.session_id.as_deref(),
        Some("session_0b6c1234-ffff-4a4a-8b8b-0123456789ab")
    );
}

#[test]
fn kimi_handler_ignores_other_agents_and_osc9_once_hooks_report() {
    let mut handler = create_handler(&CLIAgent::Kimi).expect("should create handler");
    assert!(
        handler
            .try_parse(
                Some(CLI_AGENT_NOTIFICATION_SENTINEL),
                r#"{"v":1,"agent":"claude","event":"stop"}"#,
                false,
            )
            .is_none()
    );
    assert!(
        handler
            .try_parse(Some("some-title"), "Kimi Code task complete: x", false)
            .is_none()
    );
    assert!(
        handler
            .try_parse(None, "Kimi Code task complete: x", false)
            .is_some()
    );
    assert!(
        handler
            .try_parse(None, "Kimi Code task complete: x", true)
            .is_none()
    );
}
