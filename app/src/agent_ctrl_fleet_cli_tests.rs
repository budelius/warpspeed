use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::time::Duration;

use clap::Parser;

use super::*;

const CONFIGURED: &str = "https://gitea.example.com/acme/warp";

struct PaneSpec {
    tab_index: usize,
    terminal_view_id: u64,
    agent: Option<&'static str>,
    agent_session_id: Option<&'static str>,
    working_directory: &'static str,
    agent_cwd: Option<&'static str>,
}

impl Default for PaneSpec {
    fn default() -> Self {
        Self {
            tab_index: 0,
            terminal_view_id: 1,
            agent: Some("claude"),
            agent_session_id: Some("session-1"),
            working_directory: "/work/warp",
            agent_cwd: None,
        }
    }
}

fn pane(spec: PaneSpec) -> PaneSummary {
    PaneSummary {
        session_id: Some(7),
        window: "window-1".to_owned(),
        tab_index: spec.tab_index,
        title: format!("pane {}", spec.terminal_view_id),
        shell: "pwsh".to_owned(),
        working_directory: spec.working_directory.to_owned(),
        has_running_agent: spec.agent.is_some(),
        is_busy: true,
        survives_quit: true,
        terminal_view_id: Some(spec.terminal_view_id),
        agent: spec.agent.map(str::to_owned),
        agent_session_id: spec.agent_session_id.map(str::to_owned),
        agent_cwd: spec.agent_cwd.map(str::to_owned),
        transcript_path: None,
    }
}

fn no_history(_: CLIAgent, _: &str) -> Option<PathBuf> {
    None
}

/// Every directory under `/work` is a clone of the configured repository.
fn configured_repo(path: &Path) -> Option<String> {
    path.starts_with("/work")
        .then(|| format!("{CONFIGURED}.git"))
}

fn plan(
    panes: &[(u32, PaneSummary)],
    history: &dyn Fn(CLIAgent, &str) -> Option<PathBuf>,
    repo_url_of: &dyn Fn(&Path) -> Option<String>,
) -> (Vec<PaneSource>, Vec<(PaneSummary, Skip)>) {
    plan_moves(
        panes,
        Path::new("/work"),
        history,
        &[CONFIGURED.to_owned()],
        repo_url_of,
    )
}

#[test]
fn pane_working_directory_comes_first() {
    let panes = [(
        10,
        pane(PaneSpec {
            agent_cwd: Some("/work/other"),
            ..Default::default()
        }),
    )];
    let history = |_: CLIAgent, _: &str| Some(PathBuf::from("/work/history"));
    let (sources, skipped) = plan(&panes, &history, &configured_repo);

    assert!(skipped.is_empty(), "{skipped:?}");
    assert_eq!(
        sources,
        vec![PaneSource {
            instance_pid: 10,
            terminal_view_id: 1,
            agent: CLIAgent::Claude,
            session_id: "session-1".to_owned(),
            cwd: PathBuf::from("/work/warp"),
            recovered_from: SourceOrigin::PaneCwd,
        }]
    );
}

#[test]
fn hook_cwd_is_used_when_the_pane_has_no_directory() {
    let panes = [(
        10,
        pane(PaneSpec {
            working_directory: "",
            agent_cwd: Some("/work/hooked"),
            ..Default::default()
        }),
    )];
    let history = |_: CLIAgent, _: &str| Some(PathBuf::from("/work/history"));
    let (sources, _) = plan(&panes, &history, &configured_repo);

    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].cwd, PathBuf::from("/work/hooked"));
    assert_eq!(sources[0].recovered_from, SourceOrigin::HookCwd);
}

#[test]
fn session_history_is_used_when_neither_pane_nor_hook_has_a_directory() {
    let panes = [(
        10,
        pane(PaneSpec {
            agent: Some("pi"),
            working_directory: "",
            agent_cwd: Some(""),
            ..Default::default()
        }),
    )];
    let history = |agent: CLIAgent, session_id: &str| {
        (agent == CLIAgent::Pi && session_id == "session-1").then(|| PathBuf::from("/work/history"))
    };
    let (sources, _) = plan(&panes, &history, &configured_repo);

    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].agent, CLIAgent::Pi);
    assert_eq!(sources[0].cwd, PathBuf::from("/work/history"));
    assert_eq!(sources[0].recovered_from, SourceOrigin::SessionHistory);
}

#[test]
fn a_pane_with_no_directory_anywhere_is_skipped() {
    let panes = [(
        10,
        pane(PaneSpec {
            working_directory: "",
            ..Default::default()
        }),
    )];
    let (sources, skipped) = plan(&panes, &no_history, &configured_repo);

    assert!(sources.is_empty());
    assert_eq!(skipped.len(), 1);
    assert_eq!(skipped[0].1, Skip::NoDirectory);
}

#[test]
fn panes_outside_the_folder_are_skipped() {
    let panes = [(
        10,
        pane(PaneSpec {
            working_directory: "/elsewhere/warp",
            ..Default::default()
        }),
    )];
    let anywhere = |_: &Path| Some(format!("{CONFIGURED}.git"));
    let (sources, skipped) = plan(&panes, &no_history, &anywhere);

    assert!(sources.is_empty());
    assert_eq!(skipped[0].1, Skip::OutsideFolder);
}

#[test]
fn a_repository_agent_ctrl_does_not_know_is_skipped_with_its_url() {
    let panes = [(10, pane(PaneSpec::default()))];
    let unknown = |_: &Path| Some("https://gitea.example.com/acme/other.git".to_owned());
    let (sources, skipped) = plan(&panes, &no_history, &unknown);

    assert!(sources.is_empty());
    assert_eq!(
        skipped[0].1,
        Skip::RepoNotConfigured("https://gitea.example.com/acme/other.git".to_owned())
    );
    assert_eq!(
        skipped[0].1.to_string(),
        "repo not configured: https://gitea.example.com/acme/other.git"
    );
}

#[test]
fn a_directory_outside_any_repository_is_skipped() {
    let panes = [(10, pane(PaneSpec::default()))];
    let (sources, skipped) = plan(&panes, &no_history, &|_: &Path| None);

    assert!(sources.is_empty());
    assert_eq!(skipped[0].1, Skip::NotAGitRepo);
}

#[test]
fn panes_without_an_agent_or_its_session_id_are_skipped() {
    let panes = [
        (
            10,
            pane(PaneSpec {
                agent: None,
                ..Default::default()
            }),
        ),
        (
            10,
            pane(PaneSpec {
                terminal_view_id: 2,
                agent_session_id: None,
                ..Default::default()
            }),
        ),
        (
            10,
            pane(PaneSpec {
                terminal_view_id: 3,
                agent: Some("not-an-agent"),
                ..Default::default()
            }),
        ),
    ];
    let (sources, skipped) = plan(&panes, &no_history, &configured_repo);

    assert!(sources.is_empty());
    assert_eq!(
        skipped.iter().map(|(_, skip)| skip).collect::<Vec<_>>(),
        vec![
            &Skip::NoAgentSession,
            &Skip::NoAgentSession,
            &Skip::NoAgentSession
        ]
    );
    assert_eq!(Skip::NoAgentSession.to_string(), "no agent session");
}

#[test]
fn moves_are_ordered_by_instance_then_tab() {
    let at = |tab_index, terminal_view_id| {
        pane(PaneSpec {
            tab_index,
            terminal_view_id,
            ..Default::default()
        })
    };
    let panes = [
        (30, at(0, 1)),
        (20, at(2, 2)),
        (20, at(0, 3)),
        (30, at(1, 4)),
        (20, at(1, 5)),
    ];
    let (sources, _) = plan(&panes, &no_history, &configured_repo);

    assert_eq!(
        sources
            .iter()
            .map(|source| (source.instance_pid, source.terminal_view_id))
            .collect::<Vec<_>>(),
        vec![(20, 3), (20, 5), (20, 2), (30, 1), (30, 4)]
    );
}

fn poll(steps: &[&str], timeout: Duration) -> (MoveOutcome, usize) {
    let remaining = RefCell::new(steps.iter().map(|step| step.to_string()));
    let mut calls = 0;
    let mut call = || {
        calls += 1;
        Ok(remaining
            .borrow_mut()
            .next()
            .unwrap_or_else(|| "cutting_over".to_owned()))
    };
    let outcome = poll_until_terminal(&mut call, timeout, Duration::ZERO);
    (outcome, calls)
}

#[test]
fn polling_stops_at_done_with_the_job_id() {
    let (outcome, calls) = poll(
        &[
            "preparing",
            "importing",
            "waiting_for_resume",
            "done:job-42",
        ],
        Duration::from_secs(60),
    );
    assert_eq!(
        outcome,
        MoveOutcome::Done {
            job_id: "job-42".to_owned()
        }
    );
    assert_eq!(calls, 4);
}

#[test]
fn polling_stops_at_failed_keeping_colons_in_the_reason() {
    let (outcome, _) = poll(
        &["preparing", "failed:push failed: remote: denied"],
        Duration::from_secs(60),
    );
    assert_eq!(
        outcome,
        MoveOutcome::Failed {
            reason: "push failed: remote: denied".to_owned()
        }
    );
}

#[test]
fn polling_gives_up_after_the_timeout() {
    let (outcome, _) = poll(&["preparing"], Duration::from_millis(50));
    assert_eq!(
        outcome,
        MoveOutcome::Failed {
            reason: "timed out".to_owned()
        }
    );
}

#[test]
fn polling_fails_once_the_instance_stops_answering() {
    let mut call =
        || -> anyhow::Result<String> { Err(InstanceGone("pipe closed".to_owned()).into()) };
    let outcome = poll_until_terminal(&mut call, Duration::from_secs(60), Duration::ZERO);
    let MoveOutcome::Failed { reason } = outcome else {
        panic!("expected a failure, got {outcome:?}");
    };
    assert!(reason.contains("pipe closed"), "{reason}");
}

#[test]
fn polling_fails_when_the_move_is_no_longer_tracked() {
    let (outcome, _) = poll(
        &["preparing", "none", "none", "none"],
        Duration::from_secs(60),
    );
    assert!(matches!(outcome, MoveOutcome::Failed { .. }), "{outcome:?}");
}

#[test]
fn move_all_is_routed_to_this_module() {
    let args = warp_cli::Args::try_parse_from([
        "warpspeed",
        "agent-ctrl",
        "move-all",
        "--under",
        "/work",
        "--yes",
    ])
    .unwrap();
    let Some(warp_cli::Command::AgentCtrl(managed)) = args.command() else {
        panic!("expected an agent-ctrl command");
    };
    let request = move_all_request(managed).expect("move-all reaches the fleet driver");
    assert_eq!(request.under, PathBuf::from("/work"));
    assert!(request.yes);

    let args = warp_cli::Args::try_parse_from(["warpspeed", "agent-ctrl", "list"]).unwrap();
    let Some(warp_cli::Command::AgentCtrl(managed)) = args.command() else {
        panic!("expected an agent-ctrl command");
    };
    assert!(move_all_request(managed).is_none());
}

#[test]
fn polling_retries_slow_answers_without_giving_up_the_move() {
    let mut answers = (0..5)
        .map(|_| Err(anyhow::anyhow!("did not answer within 20 seconds")))
        .chain([Ok("done:job-1".to_owned())]);
    let mut call = || answers.next().expect("polling stops at done");
    let outcome = poll_until_terminal(&mut call, Duration::from_secs(60), Duration::ZERO);
    assert_eq!(
        outcome,
        MoveOutcome::Done {
            job_id: "job-1".to_owned()
        }
    );
}

#[test]
fn polling_forgives_none_answers_that_a_real_step_interrupts() {
    let (outcome, _) = poll(
        &["none", "preparing", "none", "none", "done:job-1"],
        Duration::from_secs(60),
    );
    assert_eq!(
        outcome,
        MoveOutcome::Done {
            job_id: "job-1".to_owned()
        }
    );
}

#[test]
fn polling_follows_a_move_whose_pane_was_closed_to_its_outcome() {
    let (outcome, _) = poll(
        &["preparing", "cutting_over", "done:job-1"],
        Duration::from_secs(60),
    );
    assert_eq!(
        outcome,
        MoveOutcome::Done {
            job_id: "job-1".to_owned()
        }
    );
}

#[test]
fn an_empty_history_directory_counts_as_no_directory() {
    let panes = [(
        10,
        pane(PaneSpec {
            working_directory: "",
            ..Default::default()
        }),
    )];
    let history = |_: CLIAgent, _: &str| Some(PathBuf::new());
    let (sources, skipped) = plan(&panes, &history, &configured_repo);

    assert!(sources.is_empty());
    assert_eq!(skipped[0].1, Skip::NoDirectory);
}
