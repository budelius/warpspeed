use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use warpui::{App, EntityId};

use super::score_row;
use crate::pane_group::PaneId;
use crate::search::QueryFilter;
use crate::search::command_palette::CommandPaletteMixer;
use crate::search::command_palette::agent_sessions::DataSource;
use crate::search::command_palette::agent_sessions::data_source::{
    AgentSessionSection, ZeroStateSlot, section_for, zero_state_slots,
};
use crate::search::command_palette::mixer::CommandPaletteItemAction;
use crate::search::data_source::{Query, QueryResult};
use crate::terminal::agent_session_history::{PastAgentSession, ResolvedPath};
use crate::terminal::agent_sessions_index::{AgentSessionRow, AgentSessionState, PaneAgentInput};
use crate::terminal::cli_agent::CLIAgent;
use crate::workspace::PaneViewLocator;
use crate::workspace::agent_session_actions::AgentSessionAction;

fn locator(index: usize) -> PaneViewLocator {
    PaneViewLocator {
        pane_group_id: EntityId::from_usize(index + 1),
        pane_id: PaneId::dummy_pane_id(),
    }
}

fn row(title: Option<&str>, cwd: &str, state: AgentSessionState) -> AgentSessionRow {
    AgentSessionRow {
        agent: Some(CLIAgent::Claude),
        session_id: Some("3f2a9c1e-7b44-4d0e-9a51-0c8e2b6d1f77".into()),
        title: title.map(str::to_owned),
        cwd: cwd.into(),
        canonical_cwd: None,
        last_used: None,
        preview: Vec::new(),
        state,
    }
}

fn past_row(title: &str, cwd: &str) -> AgentSessionRow {
    row(Some(title), cwd, AgentSessionState::Past)
}

#[test]
fn score_row_matches_the_title() {
    let row = past_row("refactor the parser", "/work/other");
    assert!(score_row(&row, "parser").is_some());
}

#[test]
fn score_row_matches_a_folder_segment() {
    let row = past_row("untitled", "/work/warp-agent-sessions-picker");
    assert!(score_row(&row, "picker").is_some());
}

#[test]
fn score_row_matches_the_agent_name() {
    let row = past_row("untitled", "/work/other");
    assert!(score_row(&row, "claude").is_some());
}

#[test]
fn score_row_matches_a_session_id_prefix() {
    let row = past_row("untitled", "/work/other");
    assert!(score_row(&row, "3f2a9c").is_some());
}

#[test]
fn score_row_is_none_without_a_match() {
    let row = past_row("untitled", "/work/other");
    assert_eq!(score_row(&row, "zzqx"), None);
}

#[test]
fn rows_fall_into_this_pane_running_elsewhere_detached_and_past() {
    let focused = locator(0);
    let focused_folder = ResolvedPath::new(Path::new("/work/here"));
    let section = |row: &AgentSessionRow| section_for(row, Some(focused), Some(&focused_folder));

    let here = row(None, "/x", AgentSessionState::Running { locator: focused });
    let pending_here = row(None, "/x", AgentSessionState::Pending { locator: focused });
    let elsewhere = row(
        None,
        "/x",
        AgentSessionState::Running {
            locator: locator(1),
        },
    );
    let pending_elsewhere = row(
        None,
        "/x",
        AgentSessionState::Pending {
            locator: locator(1),
        },
    );
    let detached = row(
        None,
        "/work/here",
        AgentSessionState::Detached {
            session_id: warp_core::SessionId::from(7),
        },
    );
    let past_here = past_row("a", "/work/here/crate");
    let past_elsewhere = past_row("b", "/work/there");

    assert_eq!(section(&here), AgentSessionSection::ThisPane);
    assert_eq!(section(&pending_here), AgentSessionSection::ThisPane);
    assert_eq!(section(&elsewhere), AgentSessionSection::RunningElsewhere);
    assert_eq!(
        section(&pending_elsewhere),
        AgentSessionSection::RunningElsewhere
    );
    assert_eq!(section(&detached), AgentSessionSection::Detached);
    assert_eq!(section(&past_here), AgentSessionSection::ThisPane);
    assert_eq!(section(&past_elsewhere), AgentSessionSection::Past);
}

#[test]
fn past_rows_are_not_this_pane_without_a_focused_folder() {
    let past = past_row("a", "/work/here");
    assert_eq!(
        section_for(&past, Some(locator(0)), None),
        AgentSessionSection::Past
    );
}

#[test]
fn zero_state_lists_sections_bottom_up_with_a_separator_after_each() {
    let sections = [
        AgentSessionSection::Past,
        AgentSessionSection::ThisPane,
        AgentSessionSection::Detached,
        AgentSessionSection::RunningElsewhere,
        AgentSessionSection::ThisPane,
    ];

    // The palette renders results bottom-up, so the last slot is the topmost on screen.
    assert_eq!(
        zero_state_slots(&sections),
        vec![
            ZeroStateSlot::Row(0),
            ZeroStateSlot::Separator(AgentSessionSection::Past),
            ZeroStateSlot::Row(2),
            ZeroStateSlot::Separator(AgentSessionSection::Detached),
            ZeroStateSlot::Row(3),
            ZeroStateSlot::Separator(AgentSessionSection::RunningElsewhere),
            ZeroStateSlot::Row(4),
            ZeroStateSlot::Row(1),
            ZeroStateSlot::Separator(AgentSessionSection::ThisPane),
        ]
    );
}

#[test]
fn section_titles() {
    assert_eq!(AgentSessionSection::ThisPane.title(), "This pane");
    assert_eq!(
        AgentSessionSection::RunningElsewhere.title(),
        "Running elsewhere"
    );
    assert_eq!(AgentSessionSection::Detached.title(), "Detached");
    assert_eq!(AgentSessionSection::Past.title(), "Past");
}

fn pane(index: usize, cwd: &str) -> PaneAgentInput {
    PaneAgentInput {
        locator: locator(index),
        tab_index: index,
        tab_title: format!("tab {index}"),
        pane_number: 1,
        cwd: Some(PathBuf::from(cwd)),
        running: None,
        pending: None,
    }
}

fn past_session(id: &str, title: &str, cwd: &str) -> PastAgentSession {
    PastAgentSession {
        agent: CLIAgent::Claude,
        session_id: id.into(),
        cwd: cwd.into(),
        canonical_cwd: None,
        title: Some(title.into()),
        modified: SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000),
        preview: Vec::new(),
    }
}

fn run_agents_query(
    snapshot: (
        Vec<PaneAgentInput>,
        Option<PaneViewLocator>,
        Vec<PastAgentSession>,
    ),
    text: &str,
    check: impl FnOnce(&[QueryResult<CommandPaletteItemAction>]) + 'static,
) {
    let text = text.to_owned();
    App::test((), |mut app| async move {
        let (panes, focused, past) = snapshot;
        let data_source = app.add_model(|_| DataSource::new());
        data_source.update(&mut app, |source, _| {
            source.set_snapshot(panes, Vec::new(), focused, &past);
        });

        let mixer = app.add_model(|_| CommandPaletteMixer::new());
        mixer.update(&mut app, |mixer, ctx| {
            mixer.add_sync_source(data_source.clone(), [QueryFilter::AgentSessions]);
            mixer.run_query(
                Query {
                    filters: HashSet::from([QueryFilter::AgentSessions]),
                    text,
                },
                ctx,
            );
        });

        app.read(|app| check(mixer.as_ref(app).results()));
    })
}

#[test]
fn mixer_returns_agent_sessions_under_the_agents_filter() {
    let here = pane(0, "/work/here");
    let focused = here.locator;
    run_agents_query(
        (
            vec![here],
            Some(focused),
            vec![
                past_session("11111111-aaaa", "fix the login flow", "/work/there"),
                past_session("22222222-bbbb", "write release notes", "/work/there"),
            ],
        ),
        "login",
        move |results| {
            assert_eq!(results.len(), 1);
            match results[0].accept_result() {
                CommandPaletteItemAction::AgentSession {
                    action:
                        AgentSessionAction::Resume {
                            session_id, target, ..
                        },
                } => {
                    assert_eq!(session_id, "11111111-aaaa");
                    assert_eq!(target, Some(focused));
                }
                other => panic!("expected a resume action, got {other:?}"),
            }
        },
    );
}

#[test]
fn zero_state_query_has_a_separator_per_section() {
    run_agents_query(
        (
            vec![pane(0, "/work/here")],
            Some(locator(0)),
            vec![past_session(
                "11111111-aaaa",
                "fix the login flow",
                "/work/there",
            )],
        ),
        "",
        |results| {
            let labels: Vec<String> = results
                .iter()
                .map(|result| result.accessibility_label())
                .collect();
            assert_eq!(labels.len(), 2, "{labels:?}");
            assert!(
                labels.iter().any(|label| label == "Section: Past"),
                "{labels:?}"
            );
        },
    );
}

#[test]
fn rows_without_an_action_are_disabled() {
    run_agents_query(
        (
            Vec::new(),
            None,
            vec![past_session("not a safe id", "bad id", "/work/there")],
        ),
        "bad",
        |results| {
            assert_eq!(results.len(), 1);
            assert!(results[0].is_disabled());
            assert!(matches!(
                results[0].accept_result(),
                CommandPaletteItemAction::NoOp
            ));
        },
    );
}

/// The palette shows results reversed, so the last result is the topmost row on screen.
fn on_screen_titles(results: &[QueryResult<CommandPaletteItemAction>]) -> Vec<String> {
    results
        .iter()
        .rev()
        .map(|result| result.accessibility_label())
        .collect()
}

#[test]
fn zero_state_shows_newest_past_session_first_under_its_header() {
    let mut newer = past_session("11111111-aaaa", "newer", "/work/there");
    newer.modified += Duration::from_secs(60);
    let older = past_session("22222222-bbbb", "older", "/work/there");
    run_agents_query((Vec::new(), None, vec![older, newer]), "", |results| {
        assert_eq!(
            on_screen_titles(results),
            vec![
                "Section: Past",
                "Agent session: newer",
                "Agent session: older"
            ]
        );
    });
}

#[test]
fn search_results_keep_display_order_on_equal_scores() {
    let mut newer = past_session("11111111-aaaa", "deploy newer", "/work/there");
    newer.modified += Duration::from_secs(60);
    let older = past_session("22222222-bbbb", "deploy older", "/work/there");
    run_agents_query(
        (Vec::new(), None, vec![older, newer]),
        "deploy",
        |results| {
            assert_eq!(results[0].score(), results[1].score());
            assert_eq!(
                on_screen_titles(results),
                vec!["Agent session: deploy newer", "Agent session: deploy older"]
            );
        },
    );
}

#[test]
fn search_results_put_the_best_match_on_top() {
    let mut scattered = past_session("11111111-aaaa", "one large drawer error", "/work/there");
    scattered.modified += Duration::from_secs(60);
    let exact = past_session("22222222-bbbb", "older", "/work/there");
    run_agents_query(
        (Vec::new(), None, vec![exact, scattered]),
        "older",
        |results| {
            assert_eq!(
                on_screen_titles(results),
                vec![
                    "Agent session: older",
                    "Agent session: one large drawer error"
                ]
            );
        },
    );
}

#[test]
fn a_pending_row_reads_as_pending_not_running() {
    use crate::search::command_palette::agent_sessions::search::RowMatch;
    use crate::search::command_palette::agent_sessions::search_item::AgentSessionSearchItem;

    let pending = row(
        None,
        "/x",
        AgentSessionState::Pending {
            locator: locator(1),
        },
    );
    let running = row(
        None,
        "/x",
        AgentSessionState::Running {
            locator: locator(1),
        },
    );
    let start = AgentSessionSearchItem::new(
        pending,
        Some(AgentSessionAction::Start(locator(1))),
        RowMatch::default(),
    );
    let focus = AgentSessionSearchItem::new(
        running,
        Some(AgentSessionAction::Focus(locator(1))),
        RowMatch::default(),
    );

    assert_eq!(start.trailing_text(), "○ pending · Start");
    assert_eq!(focus.trailing_text(), "● running · Focus");
    assert_eq!(
        AgentSessionSearchItem::new(past_row("a", "/x"), None, RowMatch::default()).trailing_text(),
        ""
    );
}
