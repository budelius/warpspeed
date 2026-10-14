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
        session_uuid_hex: None,
        latest_user_prompt: None,
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
        last_prompt: None,
        modified: SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000 - age_secs),
        preview: Vec::new(),
    }
}

fn detached(id: u64, title: &str) -> DetachedInput {
    DetachedInput {
        session_id: warp_core::SessionId::from(id),
        title: title.into(),
        cwd: "/elsewhere".into(),
        detached_for_seconds: id,
        session_uuid_hex: String::new(),
    }
}

fn managed(job_id: &str, state: &str) -> RunSummary {
    RunSummary {
        job_id: job_id.to_owned(),
        state: state.to_owned(),
        agent_name: "staging-claude-interactive-rw".to_owned(),
        repo_url: Some("https://gitlab.com/acme/warp.git".to_owned()),
        branch: Some("feat/sessions".to_owned()),
        interactive: true,
        last_activity_at: None,
        node_name: None,
        paused_at: None,
        pause_allowed: Some(true),
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
            ViewRow::ManagedHeader => "managed-header".to_owned(),
            ViewRow::Managed { run } => format!("managed:{}", run.job_id),
            ViewRow::ManagedError(_) => "managed-error".to_owned(),
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
            | ViewRow::ManagedHeader
            | ViewRow::Managed { .. }
            | ViewRow::ManagedError(_)
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
        ViewRowAction::ChooseSession {
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
fn folders_with_past_sessions_are_expanded_when_the_view_opens() {
    let panes = vec![
        same_tab(0, "/proj"),
        same_tab(0, "/proj"),
        pane(1, "/proj"),
        pane(2, "/empty"),
    ];
    let history = vec![past("top", "/proj", 1)];
    let rows = build_rows(&panes, &[], &history);
    let membership = past_by_folder(&panes, &rows);

    let expanded = folders_with_past(&panes, &membership);

    assert_eq!(
        expanded,
        ExpandedFolders::from([(0, PathBuf::from("/proj")), (1, PathBuf::from("/proj")),])
    );
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
fn exact_full_word_queries_exclude_partial_matches() {
    let exact = PaneAgentInput {
        tab_title: "ME".into(),
        ..pane(0, "/projects/me")
    };
    let partial = PaneAgentInput {
        tab_title: "memo".into(),
        ..pane(1, "/projects/memo")
    };
    let panes = vec![exact, partial];
    let rows = build_rows(&panes, &[], &[]);

    let view = layout(&panes, &rows, &ExpandedFolders::new(), "me");

    assert_eq!(kinds(&view), vec!["tab:ME", "pane:0"]);
}

#[test]
fn fuzzy_queries_still_match_when_there_is_no_exact_word() {
    let matching = PaneAgentInput {
        tab_title: "backend".into(),
        ..pane(0, "/projects/backend")
    };
    let panes = vec![matching];
    let rows = build_rows(&panes, &[], &[]);

    let view = layout(&panes, &rows, &ExpandedFolders::new(), "bcknd");

    assert_eq!(kinds(&view), vec!["tab:backend", "pane:0"]);
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
fn managed_runs_are_searchable_and_keep_state_specific_actions() {
    let runs = [
        managed("job-running", "running"),
        managed("job-paused", "paused"),
    ];

    let rows = managed_view_rows(&runs, None, "paused");

    assert_eq!(kinds(&rows), ["managed-header", "managed:job-paused"]);
    let ViewRow::Managed { run } = &rows[1] else {
        panic!("the matching run should be listed");
    };
    assert_eq!(managed_action_label(&run.state), "Resume & attach");
    assert_eq!(managed_action_label("completed"), "View transcript");
    assert_eq!(managed_action_label("running"), "Attach here");
}

#[test]
fn managed_browser_lists_other_projects_and_nodes_without_a_local_pane() {
    let mut run = managed("job-remote", "paused");
    run.node_name = Some("distant-node".to_owned());
    run.repo_url = Some("https://git.example/other-project.git".to_owned());

    assert_eq!(
        kinds(&managed_view_rows(&[run.clone()], None, "")),
        ["managed-header", "managed:job-remote"]
    );
    assert_eq!(
        kinds(&managed_view_rows(&[run], None, "distant-node")),
        ["managed-header", "managed:job-remote"]
    );
}

#[test]
fn managed_session_details_include_last_activity_age_and_date() {
    let mut run = managed("job-running", "running");
    run.last_activity_at = Some("2020-01-15T12:00:00Z".to_owned());

    let (_, detail) = preview_heading(&ViewRow::Managed { run });
    let detail = detail.expect("a managed session has details");

    assert!(detail.contains("Remote AgentCtrl"));
    assert!(detail.contains("last active"));
    assert!(detail.contains("2020-01-15"));
}

#[test]
fn ending_a_process_requires_two_activations_and_switching_processes_rearms() {
    let mut pending = None;

    assert!(!confirm_process_end(&mut pending, 42));
    assert_eq!(pending, Some(42));
    assert!(!confirm_process_end(&mut pending, 43));
    assert_eq!(pending, Some(43));
    assert!(confirm_process_end(&mut pending, 43));
    assert_eq!(pending, None);
}

#[test]
fn local_session_details_include_last_activity_age_and_date() {
    let rows = build_rows(&[], &[], &[past("old", "/proj", 5)]);
    let row = rows.first().expect("the past session is indexed").clone();

    let (_, detail) = preview_heading(&ViewRow::Past {
        row,
        target: PaneViewLocator {
            pane_group_id: EntityId::from_usize(1),
            pane_id: PaneId::dummy_pane_id(),
        },
    });
    let detail = detail.expect("a local session has details");

    assert!(detail.contains("Local"));
    assert!(detail.contains("last active"));
    assert!(detail.contains("1970-"));
}

#[test]
fn an_external_process_names_its_agent_build_and_saved_pane() {
    let scanned = ScannedAgent {
        agent: CLIAgent::Claude,
        session_id: "session".into(),
        cwd: Some("/work/repo".into()),
        pid: 42,
        started_at_unix_seconds: 123,
        warp_session_uuid: Some("pane".into()),
        placement:
            crate::terminal::cli_agent_sessions::agent_process_scan::Placement::HeldByOtherBuild {
                build: "997f1e1c".into(),
                supervisor_pid: 12,
                saved_in: Some(SavedPane {
                    session_name: "M3RP".into(),
                    tab_title: "WA3P".into(),
                }),
            },
        process_count: 1,
    };
    let mut history = past("session", "/work/repo", 5);
    history.last_prompt = Some("Fix the reboot path".into());
    let rows = build_rows_with_scanned(&[], &[], &[history], &[scanned]);
    let row = rows.first().expect("the process is indexed").clone();

    let view_row = ViewRow::Detached { row: row.clone() };
    let (title, detail) = preview_heading(&view_row);

    assert_eq!(endable_process_pid(&row), Some(42));
    assert_eq!(
        preview_action(&view_row, None).map(|(_, label)| label),
        Some("End process…")
    );
    assert_eq!(
        preview_action(&view_row, Some(42)).map(|(_, label)| label),
        Some("Confirm end")
    );
    assert_eq!(title, "Claude Code · Fix the reboot path");
    let detail = detail.expect("an external process has details");
    assert!(detail.contains("Local"));
    assert!(detail.contains("997f1e1c"));
    assert!(detail.contains("M3RP / WA3P"));
    assert!(process_warning(&row).is_some_and(|warning| warning.contains("PID 42")));
}

#[test]
fn a_managed_error_stays_visible_without_runs() {
    let rows = managed_view_rows(&[], Some("sign in required"), "");
    assert_eq!(kinds(&rows), ["managed-header", "managed-error"]);
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
            | ViewRow::ManagedHeader
            | ViewRow::Managed { .. }
            | ViewRow::ManagedError(_)
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
        vec![
            ("top".into(), last.locator),
            ("nested".into(), last.locator)
        ],
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
    let expanded =
        ExpandedFolders::from([(0, PathBuf::from("/proj")), (1, PathBuf::from("/proj"))]);
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
    assert!(
        matches!(&after[old_after], ViewRow::Past { row, .. } if row.session_id.as_deref() == Some("old"))
    );
}
