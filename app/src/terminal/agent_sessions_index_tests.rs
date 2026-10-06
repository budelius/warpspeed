use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use warpui::EntityId;

use super::*;
use crate::pane_group::PaneId;
use crate::terminal::cli_agent::CLIAgent;

fn claude(id: &str, cwd: &str) -> CliAgentResumeDescriptor {
    CliAgentResumeDescriptor {
        agent: CLIAgent::Claude,
        session_id: Some(id.into()),
        cwd: cwd.into(),
        title: None,
        captured_unix_seconds: None,
    }
}

fn claude_without_id(cwd: &str) -> CliAgentResumeDescriptor {
    CliAgentResumeDescriptor {
        session_id: None,
        ..claude("", cwd)
    }
}

fn pane_at(index: usize, cwd: &str) -> PaneAgentInput {
    PaneAgentInput {
        locator: PaneViewLocator {
            pane_group_id: EntityId::from_usize(index + 1),
            pane_id: PaneId::dummy_pane_id(),
        },
        tab_index: index,
        tab_title: format!("tab {index}"),
        cwd: Some(PathBuf::from(cwd)),
        running: None,
        pending: None,
    }
}

fn past_session_at(agent: CLIAgent, id: &str, cwd: &str, age_secs: u64) -> PastAgentSession {
    PastAgentSession {
        agent,
        session_id: id.into(),
        cwd: cwd.into(),
        title: None,
        modified: SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000 - age_secs),
    }
}

fn past_session(agent: CLIAgent, id: &str, cwd: &str) -> PastAgentSession {
    past_session_at(agent, id, cwd, 0)
}

fn detached(id: u64, title: &str, cwd: &str, seconds: u64) -> DetachedInput {
    DetachedInput {
        session_id: warp_core::SessionId::from(id),
        title: title.into(),
        cwd: cwd.into(),
        detached_for_seconds: seconds,
    }
}

#[test]
fn a_running_conversation_appears_once_as_running() {
    let pane = PaneAgentInput {
        running: Some(claude("a", "/p")),
        ..pane_at(0, "/p")
    };
    let past = vec![past_session(CLIAgent::Claude, "a", "/p")];
    let rows = build_rows(&[pane], &[], &past);
    assert_eq!(rows.len(), 1);
    assert!(matches!(rows[0].state, AgentSessionState::Running { .. }));
}

#[test]
fn a_pending_restore_outranks_its_past_entry() {
    let pane = PaneAgentInput {
        pending: Some(claude("b", "/p")),
        ..pane_at(0, "/p")
    };
    let past = vec![past_session(CLIAgent::Claude, "b", "/p")];
    let rows = build_rows(&[pane], &[], &past);
    assert_eq!(rows.len(), 1);
    assert!(matches!(rows[0].state, AgentSessionState::Pending { .. }));
}

#[test]
fn past_sessions_follow_live_ones_newest_first() {
    let pane = PaneAgentInput {
        running: Some(claude("live", "/p")),
        ..pane_at(0, "/p")
    };
    let past = vec![
        past_session_at(CLIAgent::Claude, "old", "/p", 500),
        past_session_at(CLIAgent::Claude, "new", "/p", 10),
    ];
    let rows = build_rows(&[pane], &[], &past);
    let ids: Vec<_> = rows.iter().map(|row| row.session_id.as_deref()).collect();
    assert_eq!(ids, [Some("live"), Some("new"), Some("old")]);
    assert_eq!(rows[1].state, AgentSessionState::Past);
}

#[test]
fn an_agent_without_a_session_id_is_kept_per_pane() {
    let panes = [
        PaneAgentInput {
            running: Some(claude_without_id("/p")),
            ..pane_at(0, "/p")
        },
        PaneAgentInput {
            running: Some(claude_without_id("/p")),
            ..pane_at(1, "/p")
        },
    ];
    let rows = build_rows(&panes, &[], &[]);
    assert_eq!(rows.len(), 2);
}

#[test]
fn live_rows_keep_tab_order_with_running_and_pending_interleaved() {
    let panes = [
        PaneAgentInput {
            pending: Some(claude("p", "/p")),
            ..pane_at(0, "/p")
        },
        PaneAgentInput {
            running: Some(claude("r", "/p")),
            ..pane_at(1, "/p")
        },
    ];
    let rows = build_rows(&panes, &[], &[]);
    let ids: Vec<_> = rows.iter().map(|row| row.session_id.as_deref()).collect();
    assert_eq!(ids, [Some("p"), Some("r")]);
}

#[test]
fn a_running_pane_outranks_its_own_pending_restore() {
    let pane = PaneAgentInput {
        running: Some(claude("a", "/p")),
        pending: Some(claude("a", "/p")),
        ..pane_at(0, "/p")
    };
    let rows = build_rows(&[pane], &[], &[]);
    assert_eq!(rows.len(), 1);
    assert!(matches!(rows[0].state, AgentSessionState::Running { .. }));
}

#[test]
fn detached_sessions_come_after_live_ones_most_recently_detached_first() {
    let pane = PaneAgentInput {
        running: Some(claude("a", "/p")),
        ..pane_at(0, "/p")
    };
    let detached = [detached(1, "one", "/x", 900), detached(2, "two", "/y", 30)];
    let rows = build_rows(&[pane], &detached, &[]);
    assert_eq!(rows.len(), 3);
    assert_eq!(
        rows[1].state,
        AgentSessionState::Detached {
            session_id: warp_core::SessionId::from(2)
        }
    );
    assert_eq!(rows[1].agent, None);
    assert_eq!(rows[1].title.as_deref(), Some("two"));
    assert!(matches!(rows[2].state, AgentSessionState::Detached { .. }));
}

#[test]
fn a_past_session_merges_into_a_detached_shell_with_the_same_title_and_a_cwd_within_it() {
    let mut past = past_session(CLIAgent::Claude, "a", "/work/repo/sub");
    past.title = Some("fix the bug".into());
    let rows = build_rows(&[], &[detached(1, "fix the bug", "/work/repo", 5)], &[past]);
    assert_eq!(rows.len(), 1);
    assert!(matches!(rows[0].state, AgentSessionState::Detached { .. }));
    assert_eq!(rows[0].agent, Some(CLIAgent::Claude));
    assert_eq!(rows[0].session_id.as_deref(), Some("a"));
}

#[test]
fn a_past_session_with_another_title_stays_separate_from_a_detached_shell() {
    let mut past = past_session(CLIAgent::Claude, "a", "/work/repo");
    past.title = Some("something else".into());
    let rows = build_rows(&[], &[detached(1, "fix the bug", "/work/repo", 5)], &[past]);
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[1].state, AgentSessionState::Past);
}
