use std::collections::HashSet;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use warpui::EntityId;

use super::*;
use crate::app_state::CliAgentResumeDescriptor;
use crate::pane_group::PaneId;
use crate::terminal::agent_session_history::PastAgentSession;
use crate::terminal::agent_sessions_index::{DetachedInput, build_rows};
use crate::terminal::cli_agent::CLIAgent;
use crate::terminal::detached_sessions::DetachedListError;
use crate::workspace::PaneViewLocator;
use crate::workspace::agent_session_actions::AgentSessionAction;

fn claude(id: &str, cwd: &str) -> CliAgentResumeDescriptor {
    CliAgentResumeDescriptor {
        agent: CLIAgent::Claude,
        session_id: Some(id.into()),
        cwd: cwd.into(),
        title: Some(format!("conversation {id}")),
        captured_unix_seconds: None,
    }
}

fn pane(tab_index: usize, cwd: &str) -> PaneAgentInput {
    PaneAgentInput {
        locator: PaneViewLocator {
            pane_group_id: EntityId::from_usize(tab_index + 1),
            pane_id: PaneId::dummy_pane_id(),
        },
        tab_index,
        tab_title: format!("tab {tab_index}"),
        pane_number: 1,
        cwd: Some(PathBuf::from(cwd)),
        running: None,
        pending: None,
    }
}

fn past(id: &str, cwd: &str, age_secs: u64) -> PastAgentSession {
    PastAgentSession {
        agent: CLIAgent::Claude,
        session_id: id.into(),
        cwd: cwd.into(),
        canonical_cwd: None,
        title: Some(format!("past {id}")),
        modified: SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000 - age_secs),
    }
}

fn detached(id: u64, title: &str) -> DetachedInput {
    DetachedInput {
        session_id: warp_core::SessionId::from(id),
        title: title.into(),
        cwd: "/elsewhere".into(),
        detached_for_seconds: id,
    }
}

fn kinds(rows: &[ViewRow]) -> Vec<String> {
    rows.iter()
        .map(|row| match row {
            ViewRow::TabHeader { title, .. } => format!("tab:{title}"),
            ViewRow::Pane { input, .. } => format!("pane:{}", input.tab_index),
            ViewRow::PastToggle {
                count, expanded, ..
            } => format!("toggle:{count}:{expanded}"),
            ViewRow::Past { row, .. } => {
                format!("past:{}", row.session_id.clone().unwrap_or_default())
            }
            ViewRow::DetachedHeader { count } => format!("detached-header:{count}"),
            ViewRow::Detached { row } => {
                format!("detached:{}", row.title.clone().unwrap_or_default())
            }
            ViewRow::SupervisorUnreachable => "unreachable".to_owned(),
        })
        .collect()
}

/// `view_rows` over a snapshot, with folder membership worked out as the view does on open.
fn layout(
    panes: &[PaneAgentInput],
    rows: &[AgentSessionRow],
    expanded_folders: &ExpandedFolders,
    query: &str,
) -> Vec<ViewRow> {
    view_rows(
        panes,
        rows,
        &past_by_folder(panes, rows),
        expanded_folders,
        query,
    )
}

fn pane_action(rows: &[ViewRow], tab_index: usize) -> ViewRowAction {
    rows.iter()
        .find_map(|row| match row {
            ViewRow::Pane { input, action, .. } => {
                (input.tab_index == tab_index).then(|| action.clone())
            }
            ViewRow::TabHeader { .. }
            | ViewRow::PastToggle { .. }
            | ViewRow::Past { .. }
            | ViewRow::DetachedHeader { .. }
            | ViewRow::Detached { .. }
            | ViewRow::SupervisorUnreachable => None,
        })
        .expect("the pane has a row")
}

#[test]
fn panes_are_grouped_under_their_tab_headers_in_tab_order() {
    let first = pane(0, "/a");
    let second = PaneAgentInput {
        tab_title: "tab 0".into(),
        ..pane(0, "/b")
    };
    let third = pane(1, "/c");
    let panes = vec![first, second, third];
    let rows = build_rows(&panes, &[], &[]);

    let view = layout(&panes, &rows, &ExpandedFolders::new(), "");

    assert_eq!(
        kinds(&view),
        vec!["tab:tab 0", "pane:0", "pane:0", "tab:tab 1", "pane:1"]
    );
}

#[test]
fn each_pane_gets_the_action_its_state_calls_for() {
    let running = PaneAgentInput {
        running: Some(claude("r", "/r")),
        ..pane(0, "/r")
    };
    let pending = PaneAgentInput {
        pending: Some(claude("p", "/p")),
        ..pane(1, "/p")
    };
    let shell = pane(2, "/s");
    let panes = vec![running.clone(), pending.clone(), shell.clone()];
    let rows = build_rows(&panes, &[], &[]);

    let view = layout(&panes, &rows, &ExpandedFolders::new(), "");

    assert_eq!(
        pane_action(&view, 0),
        ViewRowAction::Act(AgentSessionAction::Focus(running.locator))
    );
    assert_eq!(
        pane_action(&view, 1),
        ViewRowAction::Act(AgentSessionAction::Start(pending.locator))
    );
    assert_eq!(
        pane_action(&view, 2),
        ViewRowAction::ResumeInFolder {
            folder: PathBuf::from("/s"),
            target: shell.locator,
        }
    );
}

#[test]
fn a_pane_without_a_folder_has_no_action() {
    let shell = PaneAgentInput {
        cwd: None,
        ..pane(0, "/unused")
    };
    let panes = vec![shell];
    let rows = build_rows(&panes, &[], &[]);

    let view = layout(&panes, &rows, &ExpandedFolders::new(), "");

    assert_eq!(pane_action(&view, 0), ViewRowAction::None);
}

#[test]
fn past_rows_appear_only_when_their_folder_is_expanded() {
    let panes = vec![pane(0, "/proj")];
    let history = vec![
        past("top", "/proj", 1),
        past("nested", "/proj/sub", 2),
        past("other", "/elsewhere", 3),
    ];
    let rows = build_rows(&panes, &[], &history);

    let collapsed = layout(&panes, &rows, &ExpandedFolders::new(), "");
    assert_eq!(
        kinds(&collapsed),
        vec!["tab:tab 0", "pane:0", "toggle:2:false"]
    );

    let expanded = ExpandedFolders::from([(0, PathBuf::from("/proj"))]);
    let open = layout(&panes, &rows, &expanded, "");
    assert_eq!(
        kinds(&open),
        vec![
            "tab:tab 0",
            "pane:0",
            "toggle:2:true",
            "past:top",
            "past:nested"
        ]
    );
}

#[test]
fn a_query_filters_rows_but_keeps_their_tab_header() {
    let alpha = PaneAgentInput {
        running: Some(claude("alpha", "/alpha")),
        ..pane(0, "/alpha")
    };
    let beta = PaneAgentInput {
        running: Some(claude("beta", "/beta")),
        tab_title: "tab 0".into(),
        ..pane(0, "/beta")
    };
    let gamma = pane(1, "/gamma");
    let panes = vec![alpha, beta, gamma];
    let rows = build_rows(&panes, &[], &[]);

    let view = layout(&panes, &rows, &ExpandedFolders::new(), "beta");

    assert_eq!(kinds(&view), vec!["tab:tab 0", "pane:0"]);
    assert!(matches!(
        &view[1],
        ViewRow::Pane { input, .. } if input.cwd.as_deref() == Some(std::path::Path::new("/beta"))
    ));
}

#[test]
fn a_query_shows_matching_past_rows_without_expanding() {
    let panes = vec![pane(0, "/proj")];
    let history = vec![past("needle", "/proj", 1), past("hay", "/proj", 2)];
    let rows = build_rows(&panes, &[], &history);

    let view = layout(&panes, &rows, &ExpandedFolders::new(), "needle");

    assert_eq!(
        kinds(&view),
        vec!["tab:tab 0", "pane:0", "toggle:1:true", "past:needle"]
    );
}

#[test]
fn the_detached_section_comes_last() {
    let panes = vec![pane(0, "/a"), pane(1, "/b")];
    let rows = build_rows(&panes, &[detached(1, "one"), detached(2, "two")], &[]);

    let view = layout(&panes, &rows, &ExpandedFolders::new(), "");

    assert_eq!(
        kinds(&view),
        vec![
            "tab:tab 0",
            "pane:0",
            "tab:tab 1",
            "pane:1",
            "detached-header:2",
            "detached:one",
            "detached:two"
        ]
    );
}

#[test]
fn an_unreachable_supervisor_is_one_row_in_the_detached_section() {
    assert_eq!(
        kinds(&supervisor_rows(&DetachedListError::Unreachable(
            "refused".into()
        ))),
        vec!["detached-header:0", "unreachable"]
    );
    assert!(supervisor_rows(&DetachedListError::Unsupported).is_empty());
}

#[test]
fn view_rows_takes_folder_membership_from_the_snapshot_not_the_paths() {
    let panes = vec![pane(0, "/proj")];
    let history = vec![past("near", "/proj", 1), past("far", "/elsewhere", 2)];
    let rows = build_rows(&panes, &[], &history);
    let far = rows
        .iter()
        .position(|row| row.session_id.as_deref() == Some("far"))
        .expect("the far session is a row");
    let membership = PastByFolder::from([(PathBuf::from("/proj"), vec![far])]);
    let expanded = ExpandedFolders::from([(0, PathBuf::from("/proj"))]);

    let view = view_rows(&panes, &rows, &membership, &expanded, "");

    assert_eq!(
        kinds(&view),
        vec!["tab:tab 0", "pane:0", "toggle:1:true", "past:far"]
    );
}

#[test]
fn past_by_folder_counts_subfolders_and_skips_other_folders() {
    let panes = vec![pane(0, "/proj"), pane(1, "/proj"), pane(2, "")];
    let history = vec![
        past("top", "/proj", 1),
        past("nested", "/proj/sub", 2),
        past("other", "/elsewhere", 3),
    ];
    let rows = build_rows(&panes, &[], &history);

    let membership = past_by_folder(&panes, &rows);

    assert_eq!(
        membership.len(),
        1,
        "one entry per distinct, non-empty folder"
    );
    let ids: Vec<&str> = membership[&PathBuf::from("/proj")]
        .iter()
        .filter_map(|index| rows[*index].session_id.as_deref())
        .collect();
    assert_eq!(ids, vec!["top", "nested"]);
}

fn same_tab(tab_index: usize, cwd: &str) -> PaneAgentInput {
    PaneAgentInput {
        tab_title: format!("tab {tab_index}"),
        ..pane(tab_index, cwd)
    }
}

fn past_targets(rows: &[ViewRow]) -> Vec<(String, PaneViewLocator)> {
    rows.iter()
        .filter_map(|row| match row {
            ViewRow::Past { row, target } => Some((row.session_id.clone()?, *target)),
            ViewRow::TabHeader { .. }
            | ViewRow::Pane { .. }
            | ViewRow::PastToggle { .. }
            | ViewRow::DetachedHeader { .. }
            | ViewRow::Detached { .. }
            | ViewRow::SupervisorUnreachable => None,
        })
        .collect()
}

#[test]
fn a_folders_past_is_listed_once_per_tab_under_its_last_pane() {
    let first = same_tab(0, "/proj");
    let other = same_tab(0, "/other");
    let last = same_tab(0, "/proj");
    let panes = vec![first, other, last.clone()];
    let history = vec![past("top", "/proj", 1), past("nested", "/proj/sub", 2)];
    let rows = build_rows(&panes, &[], &history);

    let collapsed = layout(&panes, &rows, &ExpandedFolders::new(), "");
    assert_eq!(
        kinds(&collapsed),
        vec!["tab:tab 0", "pane:0", "pane:0", "pane:0", "toggle:2:false"]
    );

    let expanded = ExpandedFolders::from([(0, PathBuf::from("/proj"))]);
    let open = layout(&panes, &rows, &expanded, "");
    assert_eq!(
        kinds(&open),
        vec![
            "tab:tab 0",
            "pane:0",
            "pane:0",
            "pane:0",
            "toggle:2:true",
            "past:top",
            "past:nested"
        ]
    );
    assert_eq!(
        past_targets(&open),
        vec![("top".into(), last.locator), ("nested".into(), last.locator)],
        "past rows resume in the pane they are listed under"
    );
}

#[test]
fn expanding_a_folder_in_one_tab_leaves_the_same_folder_in_another_collapsed() {
    let panes = vec![pane(0, "/proj"), pane(1, "/proj")];
    let history = vec![past("top", "/proj", 1)];
    let rows = build_rows(&panes, &[], &history);

    let expanded = ExpandedFolders::from([(1, PathBuf::from("/proj"))]);
    let view = layout(&panes, &rows, &expanded, "");

    assert_eq!(
        kinds(&view),
        vec![
            "tab:tab 0",
            "pane:0",
            "toggle:1:false",
            "tab:tab 1",
            "pane:1",
            "toggle:1:true",
            "past:top"
        ]
    );
}

#[test]
fn a_query_lists_matching_past_rows_once_per_tab_and_folder() {
    let panes = vec![same_tab(0, "/proj"), same_tab(0, "/proj"), pane(1, "/proj")];
    let history = vec![past("needle", "/proj", 1), past("hay", "/proj", 2)];
    let rows = build_rows(&panes, &[], &history);

    let view = layout(&panes, &rows, &ExpandedFolders::new(), "needle");

    assert_eq!(
        kinds(&view),
        vec![
            "tab:tab 0",
            "pane:0",
            "toggle:1:true",
            "past:needle",
            "tab:tab 1",
            "pane:1",
            "toggle:1:true",
            "past:needle"
        ]
    );
}

#[test]
fn every_row_has_its_own_key_and_keeps_it_when_rows_are_added_before_it() {
    let running = PaneAgentInput {
        running: Some(claude("r", "/proj")),
        ..same_tab(0, "/proj")
    };
    let panes = vec![running, same_tab(0, "/proj"), pane(1, "/proj")];
    let history = vec![past("old", "/proj", 5)];
    let detached_sessions = [detached(1, "one"), detached(2, "two")];
    let expanded = ExpandedFolders::from([(0, PathBuf::from("/proj")), (1, PathBuf::from("/proj"))]);
    let rows = build_rows(&panes, &detached_sessions, &history);
    let before = layout(&panes, &rows, &expanded, "");

    let keys: Vec<RowKey> = before.iter().map(ViewRow::key).collect();
    let unique: HashSet<&RowKey> = keys.iter().collect();
    assert_eq!(unique.len(), keys.len(), "no two rows share a key");

    let newer_history = vec![past("new", "/proj", 1), past("old", "/proj", 5)];
    let rows = build_rows(&panes, &detached_sessions, &newer_history);
    let after = layout(&panes, &rows, &expanded, "");
    let old_before = before
        .iter()
        .position(|row| matches!(row, ViewRow::Past { row, .. } if row.session_id.as_deref() == Some("old")))
        .expect("the old session is listed");
    let old_after = after
        .iter()
        .position(|row| row.key() == before[old_before].key())
        .expect("the old session keeps its key");
    assert_ne!(old_before, old_after, "the new session moved it down");
    assert!(matches!(&after[old_after], ViewRow::Past { row, .. } if row.session_id.as_deref() == Some("old")));
}
