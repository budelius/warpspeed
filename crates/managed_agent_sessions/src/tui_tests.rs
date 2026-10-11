use super::*;
use crate::STAGING_ZITADEL;

fn test_tui() -> ManagedSessionsTui {
    static PROFILE: EnvironmentProfile = EnvironmentProfile {
        alias: "staging",
        base_url: "http://localhost",
        fleet_id: "fleet",
        zitadel: &STAGING_ZITADEL,
    };
    let (updates_tx, updates_rx) = mpsc::channel();
    ManagedSessionsTui::new(
        &PROFILE,
        "job-123".to_owned(),
        SessionDescriptor {
            protocol_version: 1,
            environment: "staging".to_owned(),
            fleet_id: "fleet".to_owned(),
            job_id: "job-123".to_owned(),
            display_name: "Test run".to_owned(),
            agent_name: "test-agent".to_owned(),
            state: "running".to_owned(),
            capabilities: vec!["watch".to_owned(), "stop".to_owned()],
            next_cursor: None,
        },
        Vec::new(),
        updates_tx,
        updates_rx,
    )
}

fn event(event_type: &str, content: &str) -> RunEvent {
    RunEvent {
        event_id: "event-1".to_owned(),
        job_id: "job-123".to_owned(),
        sequence: 7,
        timestamp: "2026-01-01T00:00:00Z".to_owned(),
        event_type: event_type.to_owned(),
        content: content.to_owned(),
        redacted: false,
        truncated: false,
    }
}

fn render_lines(tui: &mut ManagedSessionsTui, width: u16, height: u16) -> Vec<String> {
    let backend = ratatui::backend::TestBackend::new(width, height);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal.draw(|frame| tui.draw(frame)).unwrap();
    let buffer = terminal.backend().buffer();
    (0..height)
        .map(|y| {
            (0..width)
                .filter_map(|x| buffer.cell((x, y)))
                .map(|cell| cell.symbol())
                .collect()
        })
        .collect()
}

#[test]
fn full_screen_contains_header_transcript_composer_and_controls() {
    let mut tui = test_tui();
    tui.events.push(event("assistant", "ready"));
    tui.status = StatusMessage {
        text: "Live".to_owned(),
        tone: StatusTone::Success,
    };

    let lines = render_lines(&mut tui, 80, 18);

    assert!(lines[0].starts_with("┌ AgentCtrl "));
    assert!(lines[1].contains("Test run  running"));
    assert!(lines[4].starts_with("┌ Conversation · 1 events "));
    assert!(lines.iter().any(|line| line.contains("AGENT  #7")));
    assert!(lines[14].starts_with("┌ Message "));
    assert!(lines[15].starts_with("│ Type a follow-up message…"));
    assert!(lines[17].contains("Enter send"));
    assert!(lines[17].contains("/help commands"));
    assert!(lines[17].contains("Ctrl-O shell"));
}

#[test]
fn composer_recognizes_run_controls_and_local_terminal_commands() {
    assert_eq!(
        parse_composer_command("/resume"),
        ComposerCommand::Action(SessionAction::Resume)
    );
    assert_eq!(
        parse_composer_command("/pause"),
        ComposerCommand::Action(SessionAction::Pause)
    );
    assert_eq!(
        parse_composer_command("/ssh user@example.com -t tmux attach"),
        ComposerCommand::LocalShell(Some("ssh user@example.com -t tmux attach".to_owned()))
    );
    assert_eq!(
        parse_composer_command("!tmux new -A -s work"),
        ComposerCommand::LocalShell(Some("tmux new -A -s work".to_owned()))
    );
    assert_eq!(
        parse_composer_command("//resume"),
        ComposerCommand::Send("/resume".to_owned())
    );
}

#[test]
fn status_events_update_pause_and_resume_controls() {
    let mut tui = test_tui();

    tui.update_run_state_from_events(&[event("status", "paused")]);
    assert_eq!(tui.descriptor.state, "paused");
    assert!(
        tui.descriptor
            .capabilities
            .iter()
            .any(|value| value == "resume")
    );

    tui.update_run_state_from_events(&[event("status", "resumed")]);
    assert_eq!(tui.descriptor.state, "running");
    assert!(
        tui.descriptor
            .capabilities
            .iter()
            .any(|value| value == "pause")
    );
}

#[test]
fn resume_on_running_session_is_handled_without_sending_agent_input() {
    let mut tui = test_tui();
    tui.insert_text("/resume");

    tui.submit_input();

    assert_eq!(tui.status.text, "Run is already running");
    assert!(tui.input.is_empty());
}

#[test]
fn help_lists_shell_ssh_tmux_and_run_controls() {
    let mut tui = test_tui();
    tui.help_visible = true;

    let lines = render_lines(&mut tui, 80, 24);

    assert!(lines.iter().any(|line| line.contains("AgentCtrl commands")));
    assert!(lines.iter().any(|line| line.contains("/pause  /resume")));
    assert!(lines.iter().any(|line| line.contains("/ssh <args>")));
    assert!(lines.iter().any(|line| line.contains("/tmux <args>")));
}

#[test]
fn local_shell_command_returns_to_the_tui_process() {
    let command = if cfg!(windows) { "exit 0" } else { "true" };

    let status = run_local_shell(Some(command)).unwrap();

    assert!(status.success());
}

#[test]
fn input_editing_keeps_cursor_on_utf8_boundaries() {
    let mut tui = test_tui();
    tui.insert_text("aé🦀z");

    tui.move_cursor_left();
    tui.backspace();
    tui.insert_char('!');

    assert_eq!(tui.input, "aé!z");
    assert_eq!(tui.input_cursor, "aé!".len());
}

#[test]
fn pasted_input_replaces_terminal_control_characters() {
    let mut tui = test_tui();

    tui.insert_text("hello\u{1b}[31m");

    assert_eq!(tui.input, "hello�[31m");
}

#[test]
fn transcript_distinguishes_agent_user_and_tool_events() {
    let lines = transcript_lines(
        &[
            event("assistant", "ready"),
            event("user_input", "continue"),
            event("tool_use", "Bash"),
        ],
        false,
    );
    let rendered: Vec<_> = lines.iter().map(ToString::to_string).collect();

    assert_eq!(
        rendered,
        vec![
            "AGENT  #7   ",
            "  ready",
            "",
            "YOU  #7   ",
            "  continue",
            "",
            "TOOL  #7   ",
            "  Bash",
            "",
        ]
    );
}

#[test]
fn transcript_replaces_terminal_control_characters() {
    let lines = transcript_lines(&[event("stderr", "bad\u{1b}[31m\tvalue")], false);
    let rendered: Vec<_> = lines.iter().map(ToString::to_string).collect();

    assert_eq!(rendered[1], "  bad�[31m value");
    assert!(!rendered[1].contains('\u{1b}'));
}
