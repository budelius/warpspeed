use std::path::PathBuf;

use super::*;
use crate::app_state::{
    CliAgentResumeDescriptor, LeafSnapshot, PaneNodeSnapshot, TerminalPaneSnapshot,
};
use crate::terminal::cli_agent::CLIAgent;

fn terminal_pane(
    uuid: u8,
    descriptor: Option<CliAgentResumeDescriptor>,
) -> (PaneNodeSnapshot, TerminalPaneSnapshot) {
    let terminal = TerminalPaneSnapshot {
        uuid: vec![uuid],
        persistent_session_id: None,
        session_holder: None,
        cli_agent_resume_descriptor: descriptor,
        managed_session_job_id: None,
        cwd: None,
        shell_launch_data: None,
        is_active: false,
        is_read_only: false,
        input_config: None,
        llm_model_override: None,
        active_profile_id: None,
        conversation_ids_to_restore: Vec::new(),
        active_conversation_id: None,
    };
    (
        PaneNodeSnapshot::Leaf(LeafSnapshot {
            is_focused: false,
            custom_vertical_tabs_title: None,
            contents: LeafContents::Terminal(terminal.clone()),
        }),
        terminal,
    )
}

fn claude_descriptor() -> CliAgentResumeDescriptor {
    CliAgentResumeDescriptor {
        agent: CLIAgent::Claude,
        session_id: Some("abc-123".to_owned()),
        cwd: PathBuf::from("/work/warp"),
        title: Some("fix the installer".to_owned()),
        captured_unix_seconds: Some(1_767_225_600),
    }
}

fn tab(root: PaneNodeSnapshot, custom_title: Option<&str>) -> TabSnapshot {
    TabSnapshot {
        custom_title: custom_title.map(str::to_owned),
        root,
        default_directory_color: None,
        selected_color: Default::default(),
        left_panel: None,
        right_panel: None,
        group_id: None,
        pinned: false,
    }
}

/// A pane with no agent contributes nothing, so the registry is a list of sessions rather than a
/// list of panes with mostly-empty entries.
#[test]
fn panes_without_an_agent_are_not_recorded() {
    let (node, _) = terminal_pane(1, None);
    let records = collect_from_tabs(vec![tab(node, None)]);
    assert!(records.is_empty());
}

#[test]
fn a_claude_pane_is_recorded_with_its_resume_command() {
    let (node, _) = terminal_pane(0xab, Some(claude_descriptor()));
    let records = collect_from_tabs(vec![tab(node, Some("warp"))]);

    assert_eq!(records.len(), 1);
    let record = &records[0];
    assert_eq!(record.agent, "Claude Code");
    assert_eq!(record.session_id.as_deref(), Some("abc-123"));
    assert_eq!(
        record.resume_command.as_deref(),
        Some("claude --resume abc-123")
    );
    assert_eq!(record.pane_uuid, "ab");
    assert_eq!(record.tab_title.as_deref(), Some("warp"));
    assert_eq!(record.title.as_deref(), Some("fix the installer"));
}

/// An agent Warp never got a session ID for still has to appear. Its absence is the answer to why
/// that session did not come back, and a registry that omits it answers nothing.
#[test]
fn an_agent_without_a_session_id_is_still_recorded() {
    let descriptor = CliAgentResumeDescriptor {
        session_id: None,
        ..claude_descriptor()
    };
    let (node, _) = terminal_pane(1, Some(descriptor));
    let records = collect_from_tabs(vec![tab(node, None)]);

    assert_eq!(records.len(), 1);
    assert_eq!(records[0].session_id, None);
    assert_eq!(records[0].resume_command, None);

    let markdown = render_markdown(&records);
    assert!(
        markdown.contains("none was reported"),
        "the markdown must explain the missing session ID, not omit the session: {markdown}"
    );
}

/// An agent whose CLI cannot be told to open a session by ID is recorded without a command rather
/// than with a guessed one -- a wrong command starts a fresh session and scrolls the ID away.
#[test]
fn an_agent_with_no_verified_resume_syntax_gets_no_command() {
    let descriptor = CliAgentResumeDescriptor {
        agent: CLIAgent::Gemini,
        ..claude_descriptor()
    };
    let (node, _) = terminal_pane(1, Some(descriptor));
    let records = collect_from_tabs(vec![tab(node, None)]);

    assert_eq!(records[0].session_id.as_deref(), Some("abc-123"));
    assert_eq!(records[0].resume_command, None);
}

/// Split panes nest, and every terminal in the tree counts.
#[test]
fn every_pane_in_a_split_is_visited() {
    use crate::app_state::{BranchSnapshot, PaneFlex, SplitDirection};

    let (first, _) = terminal_pane(1, Some(claude_descriptor()));
    let (second, _) = terminal_pane(2, Some(claude_descriptor()));
    let (third, _) = terminal_pane(3, None);
    let inner = PaneNodeSnapshot::Branch(BranchSnapshot {
        direction: SplitDirection::Vertical,
        children: vec![(PaneFlex(0.5), second), (PaneFlex(0.5), third)],
    });
    let root = PaneNodeSnapshot::Branch(BranchSnapshot {
        direction: SplitDirection::Horizontal,
        children: vec![(PaneFlex(0.5), first), (PaneFlex(0.5), inner)],
    });

    let records = collect_from_tabs(vec![tab(root, None)]);
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].pane_uuid, "01");
    assert_eq!(records[1].pane_uuid, "02");
}

/// The registry is rewritten in place on every save, so a run with fewer sessions than the last
/// must not leave the old ones behind for the user to chase.
#[test]
fn writing_replaces_the_previous_registry() {
    let dir = tempfile::tempdir().expect("temp dir");

    let (node, _) = terminal_pane(1, Some(claude_descriptor()));
    let records = collect_from_tabs(vec![tab(node, None)]);
    write_to_dir(dir.path(), &records).expect("first write");
    let json = std::fs::read_to_string(dir.path().join(JSON_FILE_NAME)).expect("json written");
    assert!(json.contains("abc-123"));

    write_to_dir(dir.path(), &[]).expect("second write");
    let json = std::fs::read_to_string(dir.path().join(JSON_FILE_NAME)).expect("json written");
    assert!(
        !json.contains("abc-123"),
        "a stale session must not survive a save that no longer has it: {json}"
    );
    let markdown =
        std::fs::read_to_string(dir.path().join(MARKDOWN_FILE_NAME)).expect("markdown written");
    assert!(markdown.contains("No pane was running a CLI agent."));

    assert!(
        !dir.path().join("agent-sessions.tmp").exists(),
        "the temporary file must not be left behind"
    );
}

fn collect_from_tabs(tabs: Vec<TabSnapshot>) -> Vec<AgentSessionRecord> {
    let mut records = Vec::new();
    for (tab_index, tab) in tabs.iter().enumerate() {
        collect_node(0, tab_index, tab, &tab.root, &mut records);
    }
    records
}
