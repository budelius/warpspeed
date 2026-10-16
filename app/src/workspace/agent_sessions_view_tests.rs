use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use warpui::EntityId;

use super::*;
use crate::app_state::CliAgentResumeDescriptor;
use crate::pane_group::PaneId;
use crate::terminal::agent_session_history::PastAgentSession;
use crate::terminal::agent_sessions_index::{DetachedInput, build_rows, build_rows_with_scanned};
use crate::terminal::cli_agent::CLIAgent;
use crate::terminal::cli_agent_sessions::agent_process_scan::{Placement, SavedPane, ScannedAgent};
use crate::terminal::detached_sessions::DetachedListError;
use crate::workspace::PaneViewLocator;
use crate::workspace::agent_session_actions::AgentSessionAction;

fn locator(index: usize) -> PaneViewLocator {
    PaneViewLocator {
        pane_group_id: EntityId::from_usize(index + 1),
        pane_id: PaneId::dummy_pane_id(),
    }
}

fn descriptor(id: &str, cwd: &str) -> CliAgentResumeDescriptor {
    CliAgentResumeDescriptor {
        agent: CLIAgent::Claude,
        session_id: Some(id.into()),
        cwd: cwd.into(),
        title: Some(format!("conversation {id}")),
        captured_unix_seconds: None,
    }
}

fn pane(index: usize, cwd: &str) -> PaneAgentInput {
    PaneAgentInput {
        locator: locator(index),
        tab_index: index,
        tab_title: format!("tab {index}"),
        pane_number: 1,
        cwd: Some(PathBuf::from(cwd)),
        session_uuid_hex: None,
        latest_user_prompt: None,
        running: None,
        pending: None,
    }
}

fn running_pane(index: usize, id: &str, cwd: &str) -> PaneAgentInput {
    PaneAgentInput {
        running: Some(descriptor(id, cwd)),
        ..pane(index, cwd)
    }
}

fn past_at(id: &str, cwd: &str, modified: SystemTime) -> PastAgentSession {
    PastAgentSession {
        agent: CLIAgent::Claude,
        session_id: id.into(),
        cwd: cwd.into(),
        canonical_cwd: None,
        title: Some(format!("past {id}")),
        last_prompt: None,
        modified,
        preview: Vec::new(),
    }
}

fn past(id: &str, cwd: &str, age_secs: u64) -> PastAgentSession {
    past_at(
        id,
        cwd,
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000 - age_secs),
    )
}

fn detached(id: u64, title: &str, cwd: &str) -> DetachedInput {
    DetachedInput {
        session_id: warp_core::SessionId::from(id),
        title: title.into(),
        cwd: cwd.into(),
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
            ViewRow::SectionHeader { section, count, .. } => {
                format!("section:{section:?}:{count}")
            }
            ViewRow::DateHeader(date) => format!("date:{date}"),
            ViewRow::Local { row, .. } => format!(
                "local:{}",
                row.session_id
                    .clone()
                    .or_else(|| row.title.clone())
                    .unwrap_or_default()
            ),
            ViewRow::Managed { run } => format!("managed:{}", run.job_id),
            ViewRow::ManagedError(_) => "managed-error".to_owned(),
            ViewRow::SupervisorUnreachable => "unreachable".to_owned(),
        })
        .collect()
}

fn layout(
    panes: &[PaneAgentInput],
    detached: &[DetachedInput],
    history: &[PastAgentSession],
    target: Option<PaneViewLocator>,
    filter: SessionFilter,
    query: &str,
) -> Vec<ViewRow> {
    let rows = build_rows(panes, detached, history);
    view_rows(panes, &rows, target, filter, query)
}

#[test]
fn all_sessions_promotes_the_current_directory_first() {
    let target = pane(0, "/work/current");
    let elsewhere = running_pane(1, "running-elsewhere", "/work/elsewhere");
    let panes = vec![target.clone(), elsewhere];
    let history = vec![
        past("current-past", "/work/current/subdir", 1),
        past("other-past", "/another/project", 2),
    ];

    let rows = layout(
        &panes,
        &[detached(7, "detached shell", "/detached")],
        &history,
        Some(target.locator),
        SessionFilter::All,
        "",
    );

    let kinds = kinds(&rows);
    assert_eq!(kinds[0], "section:CurrentDirectory:1");
    assert_eq!(kinds[1], "local:current-past");
    assert_eq!(kinds[2], "section:RunningAndDetached:2");
    assert_eq!(kinds[3], "local:running-elsewhere");
    assert_eq!(kinds[4], "local:detached shell");
    assert_eq!(kinds[5], "section:Past:1");
    assert!(kinds[6].starts_with("date:"));
    assert_eq!(kinds[7], "local:other-past");
}

#[test]
fn empty_shells_are_not_session_rows_or_nested_picker_actions() {
    let target = pane(0, "/work/current");

    let rows = layout(
        std::slice::from_ref(&target),
        &[],
        &[past("elsewhere", "/work/elsewhere", 1)],
        Some(target.locator),
        SessionFilter::All,
        "",
    );

    assert_eq!(
        kinds(&rows)
            .into_iter()
            .filter(|kind| kind.starts_with("local:"))
            .collect::<Vec<_>>(),
        ["local:elsewhere"]
    );
}

#[test]
fn every_past_session_is_available_and_resumes_in_the_opening_pane() {
    let target = pane(0, "/work/current");
    let rows = layout(
        std::slice::from_ref(&target),
        &[],
        &[past("far-away", "/unrelated/repository", 1)],
        Some(target.locator),
        SessionFilter::All,
        "",
    );
    let ViewRow::Local {
        row,
        target: actual_target,
    } = rows
        .iter()
        .find(|row| matches!(row, ViewRow::Local { .. }))
        .expect("the global past session is listed")
    else {
        panic!("expected a local session row");
    };

    assert_eq!(*actual_target, Some(target.locator));
    assert!(matches!(
        action_for(row, *actual_target),
        Some(AgentSessionAction::Resume {
            target: Some(actual),
            ..
        }) if actual == target.locator
    ));
}

#[test]
fn filters_include_all_sessions_of_their_kind() {
    let target = running_pane(0, "current-running", "/work/current");
    let elsewhere = running_pane(1, "other-running", "/work/elsewhere");
    let panes = vec![target.clone(), elsewhere];
    let history = vec![
        past("current-past", "/work/current", 1),
        past("other-past", "/work/elsewhere", 2),
    ];

    let current = layout(
        &panes,
        &[],
        &history,
        Some(target.locator),
        SessionFilter::CurrentDirectory,
        "",
    );
    let running = layout(
        &panes,
        &[],
        &history,
        Some(target.locator),
        SessionFilter::RunningAndDetached,
        "",
    );
    let past = layout(
        &panes,
        &[],
        &history,
        Some(target.locator),
        SessionFilter::Past,
        "",
    );

    assert_eq!(
        kinds(&current),
        [
            "section:CurrentDirectory:2",
            "local:current-running",
            "local:current-past"
        ]
    );
    assert_eq!(
        kinds(&running),
        [
            "section:RunningAndDetached:2",
            "local:current-running",
            "local:other-running"
        ]
    );
    assert_eq!(
        kinds(&past)
            .into_iter()
            .filter(|kind| !kind.starts_with("date:"))
            .collect::<Vec<_>>(),
        ["section:Past:2", "local:current-past", "local:other-past"]
    );
}

#[test]
fn past_sessions_are_grouped_under_calendar_dates() {
    let first_day = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let previous_day = first_day - Duration::from_secs(86_400);
    let history = vec![
        past_at("newer", "/a", first_day),
        past_at("same-day", "/b", first_day - Duration::from_secs(60)),
        past_at("previous-day", "/c", previous_day),
    ];

    let rows = layout(&[], &[], &history, None, SessionFilter::Past, "");
    let date_headers = rows
        .iter()
        .filter(|row| matches!(row, ViewRow::DateHeader(_)))
        .count();

    assert_eq!(date_headers, 2);
    assert_eq!(
        kinds(&rows)
            .into_iter()
            .filter(|kind| kind.starts_with("local:"))
            .collect::<Vec<_>>(),
        ["local:newer", "local:same-day", "local:previous-day"]
    );
}

#[test]
fn search_finds_sessions_outside_the_current_directory() {
    let target = pane(0, "/work/current");
    let history = vec![
        past("needle", "/unrelated/repository", 1),
        past("haystack", "/work/current", 2),
    ];

    let rows = layout(
        std::slice::from_ref(&target),
        &[],
        &history,
        Some(target.locator),
        SessionFilter::All,
        "needle",
    );

    assert_eq!(
        kinds(&rows)
            .into_iter()
            .filter(|kind| kind.starts_with("local:"))
            .collect::<Vec<_>>(),
        ["local:needle"]
    );
}

#[test]
fn exact_full_word_queries_exclude_partial_matches() {
    let history = vec![
        past("exact", "/projects/me", 1),
        past("partial", "/memo", 2),
    ];

    let rows = layout(&[], &[], &history, None, SessionFilter::Past, "me");

    assert_eq!(
        kinds(&rows)
            .into_iter()
            .filter(|kind| kind.starts_with("local:"))
            .collect::<Vec<_>>(),
        ["local:exact"]
    );
}

#[test]
fn managed_runs_are_searchable_and_keep_state_specific_actions() {
    let runs = [
        managed("job-running", "running"),
        managed("job-paused", "paused"),
    ];

    let rows = managed_view_rows(&runs, None, "paused");

    assert_eq!(kinds(&rows), ["section:AgentCtrl:1", "managed:job-paused"]);
    let ViewRow::Managed { run } = &rows[1] else {
        panic!("the matching run should be listed");
    };
    assert_eq!(managed_action_label(&run.state), "Resume & attach");
    assert_eq!(managed_action_label("completed"), "View transcript");
    assert_eq!(managed_action_label("running"), "Attach here");
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
fn a_managed_error_stays_visible_without_runs() {
    let rows = managed_view_rows(&[], Some("sign in required"), "");

    assert_eq!(kinds(&rows), ["section:AgentCtrl:0", "managed-error"]);
}

#[test]
fn an_unreachable_supervisor_is_visible_in_the_running_section() {
    assert_eq!(
        kinds(&supervisor_rows(&DetachedListError::Unreachable(
            "refused".into()
        ))),
        ["section:RunningAndDetached:0", "unreachable"]
    );
    assert!(supervisor_rows(&DetachedListError::Unsupported).is_empty());
}

#[test]
fn filter_counts_are_independent_of_the_all_sessions_deduplication() {
    let target = running_pane(0, "running", "/work/current");
    let panes = vec![target.clone()];
    let rows = build_rows(
        &panes,
        &[detached(1, "shell", "/work/current")],
        &[past("past", "/work/current", 1)],
    );
    let (_, current_folder) = focused_folder(&panes, Some(target.locator)).unwrap();

    assert_eq!(
        filter_count(SessionFilter::All, &rows, 2, Some(&current_folder)),
        5
    );
    assert_eq!(
        filter_count(
            SessionFilter::CurrentDirectory,
            &rows,
            2,
            Some(&current_folder)
        ),
        3
    );
    assert_eq!(
        filter_count(
            SessionFilter::RunningAndDetached,
            &rows,
            2,
            Some(&current_folder)
        ),
        2
    );
    assert_eq!(
        filter_count(SessionFilter::AgentCtrl, &rows, 2, Some(&current_folder)),
        2
    );
    assert_eq!(
        filter_count(SessionFilter::Past, &rows, 2, Some(&current_folder)),
        1
    );
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
fn an_external_process_names_its_agent_build_and_saved_pane() {
    let scanned = ScannedAgent {
        agent: CLIAgent::Claude,
        session_id: "session".into(),
        cwd: Some("/work/repo".into()),
        pid: 42,
        started_at_unix_seconds: 123,
        warp_session_uuid: Some("pane".into()),
        placement: Placement::HeldByOtherBuild {
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
    let view_row = ViewRow::Local {
        row: row.clone(),
        target: None,
    };

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
    assert!(detail.contains("997f1e1c"));
    assert!(detail.contains("M3RP / WA3P"));
    assert!(process_warning(&row).is_some_and(|warning| warning.contains("PID 42")));
}

#[test]
fn local_session_details_include_last_activity_age_and_date() {
    let rows = build_rows(&[], &[], &[past("old", "/proj", 5)]);
    let row = rows.first().expect("the past session is indexed").clone();

    let (_, detail) = preview_heading(&ViewRow::Local { row, target: None });
    let detail = detail.expect("a local session has details");

    assert!(detail.contains("Local"));
    assert!(detail.contains("last active"));
    assert!(detail.contains("1970-"));
}

#[test]
fn row_keys_stay_stable_when_a_newer_session_is_inserted() {
    let older = past("older", "/work", 10);
    let old_rows = layout(
        &[],
        &[],
        std::slice::from_ref(&older),
        None,
        SessionFilter::Past,
        "",
    );
    let old_key = old_rows
        .iter()
        .find(|row| matches!(row, ViewRow::Local { .. }))
        .map(ViewRow::key)
        .unwrap();

    let new_rows = layout(
        &[],
        &[],
        &[past("newer", "/work", 1), older],
        None,
        SessionFilter::Past,
        "",
    );

    assert!(new_rows.iter().any(|row| row.key() == old_key));
}
