use std::path::{Path, PathBuf};

use warpui::EntityId;

use super::*;
use crate::pane_group::PaneId;
use crate::terminal::agent_sessions_index::AgentSessionState;
use crate::terminal::shell::ShellType;

/// Each call makes a distinct pane: `dummy_pane_id` allocates a fresh entity.
fn locator(index: usize) -> PaneViewLocator {
    PaneViewLocator {
        pane_group_id: EntityId::from_usize(index),
        pane_id: PaneId::dummy_pane_id(),
    }
}

fn row(agent: Option<CLIAgent>, state: AgentSessionState) -> AgentSessionRow {
    AgentSessionRow {
        agent,
        session_id: Some("abc".into()),
        title: None,
        last_prompt: None,
        cwd: PathBuf::from("T:/p/x"),
        canonical_cwd: None,
        last_used: None,
        preview: Vec::new(),
        process_count: 0,
        supervisor: None,
        state,
    }
}

#[test]
fn resume_command_line_uses_each_shells_separator() {
    let cwd = Path::new("T:/p/x");
    assert_eq!(
        resume_command_line(cwd, "pi --session 1", ShellKind::Posix),
        "cd T:/p/x && pi --session 1"
    );
    assert_eq!(
        resume_command_line(cwd, "pi --session 1", ShellKind::PowerShell),
        "Set-Location -LiteralPath 'T:/p/x' -ErrorAction Stop; pi --session 1"
    );
    assert_eq!(
        resume_command_line(cwd, "pi --session 1", ShellKind::Fish),
        "cd T:/p/x; and pi --session 1"
    );
}

#[test]
fn a_path_with_spaces_is_escaped() {
    let cwd = Path::new("T:/my dir");
    assert_eq!(
        resume_command_line(cwd, "claude --resume 1", ShellKind::Posix),
        "cd T:/my\\ dir && claude --resume 1"
    );
    assert_eq!(
        resume_command_line(cwd, "claude --resume 1", ShellKind::Fish),
        "cd T:/my\\ dir; and claude --resume 1"
    );
    assert_eq!(
        resume_command_line(cwd, "claude --resume 1", ShellKind::PowerShell),
        "Set-Location -LiteralPath 'T:/my dir' -ErrorAction Stop; claude --resume 1"
    );
}

#[test]
fn powershell_takes_brackets_literally_and_doubles_single_quotes() {
    assert_eq!(
        resume_command_line(
            Path::new("T:/a[1]/it's"),
            "claude --resume 1",
            ShellKind::PowerShell
        ),
        "Set-Location -LiteralPath 'T:/a[1]/it''s' -ErrorAction Stop; claude --resume 1"
    );
}

#[test]
fn each_action_has_its_label() {
    let locator = locator(1);
    assert_eq!(AgentSessionAction::Focus(locator).label(), "Focus");
    assert_eq!(AgentSessionAction::Start(locator).label(), "Start");
    assert_eq!(
        AgentSessionAction::Attach {
            session_id: warp_core::SessionId::from(1),
            holder: None,
            agent: None,
        }
        .label(),
        "Attach"
    );
    assert_eq!(
        AgentSessionAction::Resume {
            agent: CLIAgent::Claude,
            session_id: "abc".into(),
            cwd: PathBuf::from("T:/p/x"),
            target: None,
        }
        .label(),
        "Resume"
    );
}

#[test]
fn each_shell_type_maps_to_its_kind() {
    assert_eq!(ShellKind::from(ShellType::Zsh), ShellKind::Posix);
    assert_eq!(ShellKind::from(ShellType::Bash), ShellKind::Posix);
    assert_eq!(ShellKind::from(ShellType::Fish), ShellKind::Fish);
    assert_eq!(
        ShellKind::from(ShellType::PowerShell),
        ShellKind::PowerShell
    );
}

#[test]
fn each_state_maps_to_its_action() {
    let (running_pane, pending_pane, target) = (locator(1), locator(2), locator(9));
    let running = row(
        Some(CLIAgent::Claude),
        AgentSessionState::Running {
            locator: running_pane,
        },
    );
    assert_eq!(
        action_for(&running, Some(target)),
        Some(AgentSessionAction::Focus(running_pane))
    );

    let pending = row(
        Some(CLIAgent::Claude),
        AgentSessionState::Pending {
            locator: pending_pane,
        },
    );
    assert_eq!(
        action_for(&pending, Some(target)),
        Some(AgentSessionAction::Start(pending_pane))
    );

    let session_id = warp_core::SessionId::from(7);
    let holder = SessionHolder {
        channel: "old-supervisor".into(),
        version: "old-build".into(),
        pid: 42,
    };
    let detached = row(
        Some(CLIAgent::Pi),
        AgentSessionState::Detached {
            session_id,
            holder: Some(holder.clone()),
        },
    );
    assert_eq!(
        action_for(&detached, Some(target)),
        Some(AgentSessionAction::Attach {
            session_id,
            holder: Some(holder),
            agent: Some(HeldAgent {
                agent: CLIAgent::Pi,
                session_id: "abc".into(),
            }),
        })
    );

    let past = row(Some(CLIAgent::Claude), AgentSessionState::Past);
    assert_eq!(
        action_for(&past, Some(target)),
        Some(AgentSessionAction::Resume {
            agent: CLIAgent::Claude,
            session_id: "abc".into(),
            cwd: PathBuf::from("T:/p/x"),
            target: Some(target),
        })
    );
}

#[test]
fn a_past_row_without_a_resume_command_has_no_action() {
    let gemini = row(Some(CLIAgent::Gemini), AgentSessionState::Past);
    assert_eq!(action_for(&gemini, Some(locator(9))), None);

    let without_id = AgentSessionRow {
        session_id: None,
        ..row(Some(CLIAgent::Claude), AgentSessionState::Past)
    };
    assert_eq!(action_for(&without_id, Some(locator(9))), None);

    let without_agent = row(None, AgentSessionState::Past);
    assert_eq!(action_for(&without_agent, None), None);
}

#[test]
fn a_past_row_with_an_unsafe_session_id_has_no_action() {
    for session_id in ["", "abc; rm -rf ~", "../abc", "a b", "$(whoami)"] {
        let unsafe_row = AgentSessionRow {
            session_id: Some(session_id.into()),
            ..row(Some(CLIAgent::Claude), AgentSessionState::Past)
        };
        assert_eq!(action_for(&unsafe_row, None), None, "{session_id:?}");
    }
    let uuid_row = AgentSessionRow {
        session_id: Some("0f8e-41_ab".into()),
        ..row(Some(CLIAgent::Claude), AgentSessionState::Past)
    };
    assert!(action_for(&uuid_row, None).is_some());
}

#[test]
fn resume_directory_is_the_conversations_folder_or_nothing() {
    let exists = |path: &Path| path != Path::new("T:/gone");
    assert_eq!(
        resume_directory(Path::new("T:/p/x"), exists),
        Some(PathBuf::from("T:/p/x"))
    );
    assert_eq!(resume_directory(Path::new("T:/gone"), exists), None);
    assert_eq!(resume_directory(Path::new(""), exists), None);
}
