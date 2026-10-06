//! `<warp binary> agent-ctrl move-all --under <folder>` -- moving every local agent session in a
//! folder to AgentCtrl from a shell.
//!
//! Lives in the app rather than in `managed_agent_sessions` because the sessions are the running
//! windows' to give up: each move goes to the instance holding the pane over
//! [`crate::remote_control`], exactly as confirming the move in that pane would.

use std::collections::HashMap;
use std::fmt;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use instant::Instant;
use itertools::Itertools;
use managed_agent_sessions::{list_repositories, normalize_repo_url};
use warp_cli::managed_sessions::{ManagedEnvironment, ManagedSessionsArgs, ManagedSessionsCommand};

use crate::remote_control::address;
use crate::remote_control::client::{self, Instance};
use crate::remote_control::protocol::{PROTOCOL_VERSION, PaneSummary, Request, Response};
use crate::terminal::CLIAgent;
use crate::terminal::agent_session_handoff::repo_url_for;
use crate::terminal::agent_session_history;

const POLL_INTERVAL: Duration = Duration::from_secs(2);

const MOVE_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// How many `none` answers, or failures to reach the instance at all, in a row end a move as
/// lost. A single `none` can race the instance recording a move it has just accepted.
const MAX_CONSECUTIVE_LOST: usize = 3;

const LOG_FILE_NAME: &str = "agentctrl-move-all.log";

/// One agent session that will be moved, and where it was found to be running.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PaneSource {
    pub instance_pid: u32,
    pub terminal_view_id: u64,
    pub agent: CLIAgent,
    pub session_id: String,
    pub cwd: PathBuf,
    pub recovered_from: SourceOrigin,
}

/// Which report of a pane's directory [`PaneSource::cwd`] came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SourceOrigin {
    PaneCwd,
    HookCwd,
    SessionHistory,
}

impl fmt::Display for SourceOrigin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            SourceOrigin::PaneCwd => "pane",
            SourceOrigin::HookCwd => "agent hook",
            SourceOrigin::SessionHistory => "session history",
        })
    }
}

/// Why a pane is left where it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Skip {
    NoAgentSession,
    NoDirectory,
    OutsideFolder,
    /// The repository's URL, as its remote names it.
    RepoNotConfigured(String),
    NotAGitRepo,
}

impl fmt::Display for Skip {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Skip::NoAgentSession => f.write_str("no agent session"),
            Skip::NoDirectory => f.write_str("no directory"),
            Skip::OutsideFolder => f.write_str("outside the folder"),
            Skip::RepoNotConfigured(url) => write!(f, "repo not configured: {url}"),
            Skip::NotAGitRepo => f.write_str("not a git repository"),
        }
    }
}

/// How one move ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MoveOutcome {
    Done { job_id: String },
    Failed { reason: String },
}

impl fmt::Display for MoveOutcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MoveOutcome::Done { job_id } => write!(f, "moved: {job_id}"),
            MoveOutcome::Failed { reason } => write!(f, "failed: {reason}"),
        }
    }
}

/// A parsed `agent-ctrl move-all`.
#[derive(Debug, Clone)]
pub(crate) struct MoveAllRequest {
    pub under: PathBuf,
    pub yes: bool,
    pub environment: ManagedEnvironment,
}

/// The `move-all` request in `args`, which this module runs rather than `managed_agent_sessions`.
pub(crate) fn move_all_request(args: &ManagedSessionsArgs) -> Option<MoveAllRequest> {
    match &args.command {
        ManagedSessionsCommand::MoveAll {
            under,
            yes,
            environment,
        } => Some(MoveAllRequest {
            under: under.clone(),
            yes: *yes,
            environment: *environment,
        }),
        ManagedSessionsCommand::List
        | ManagedSessionsCommand::Start { .. }
        | ManagedSessionsCommand::Repos { .. }
        | ManagedSessionsCommand::Open { .. }
        | ManagedSessionsCommand::Attach { .. }
        | ManagedSessionsCommand::Input { .. }
        | ManagedSessionsCommand::Close { .. }
        | ManagedSessionsCommand::Stop { .. } => None,
    }
}

/// The agent whose first command prefix is `label`, as [`PaneSummary::agent`] names it.
fn agent_from_label(label: &str) -> Option<CLIAgent> {
    enum_iterator::all::<CLIAgent>().find(|agent| agent.command_prefixes().first() == Some(&label))
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.filter(|value| !value.trim().is_empty())
}

/// Whether `path` is `folder` or inside it.
fn is_under(path: &Path, folder: &Path) -> bool {
    // Windows paths compare case-insensitively, and a pane reports its directory with whichever
    // separators and drive-letter case the shell used.
    #[cfg(windows)]
    {
        let normalized =
            |path: &Path| PathBuf::from(path.to_string_lossy().replace('/', "\\").to_lowercase());
        normalized(path).starts_with(normalized(folder))
    }
    #[cfg(not(windows))]
    {
        path.starts_with(folder)
    }
}

/// Decides which panes' agent sessions to move and why the others stay, in the order the moves
/// run: by instance, then by tab.
pub(crate) fn plan_moves(
    panes: &[(u32, PaneSummary)],
    under: &Path,
    history: &dyn Fn(CLIAgent, &str) -> Option<PathBuf>,
    configured: &[String],
    repo_url_of: &dyn Fn(&Path) -> Option<String>,
) -> (Vec<PaneSource>, Vec<(PaneSummary, Skip)>) {
    let mut ordered = panes.iter().collect::<Vec<_>>();
    ordered.sort_by_key(|(pid, pane)| (*pid, pane.tab_index));

    let mut sources = Vec::new();
    let mut skipped = Vec::new();
    for (pid, pane) in ordered {
        match plan_one(*pid, pane, under, history, configured, repo_url_of) {
            Ok(source) => sources.push(source),
            Err(skip) => skipped.push((pane.clone(), skip)),
        }
    }
    (sources, skipped)
}

fn plan_one(
    instance_pid: u32,
    pane: &PaneSummary,
    under: &Path,
    history: &dyn Fn(CLIAgent, &str) -> Option<PathBuf>,
    configured: &[String],
    repo_url_of: &dyn Fn(&Path) -> Option<String>,
) -> Result<PaneSource, Skip> {
    let agent = non_empty(pane.agent.as_deref())
        .and_then(agent_from_label)
        .ok_or(Skip::NoAgentSession)?;
    let session_id = non_empty(pane.agent_session_id.as_deref()).ok_or(Skip::NoAgentSession)?;
    let terminal_view_id = pane.terminal_view_id.ok_or(Skip::NoAgentSession)?;

    let (cwd, recovered_from) = non_empty(Some(pane.working_directory.as_str()))
        .map(|cwd| (PathBuf::from(cwd), SourceOrigin::PaneCwd))
        .or_else(|| {
            non_empty(pane.agent_cwd.as_deref())
                .map(|cwd| (PathBuf::from(cwd), SourceOrigin::HookCwd))
        })
        .or_else(|| {
            history(agent, session_id)
                .filter(|cwd| !cwd.as_os_str().is_empty())
                .map(|cwd| (cwd, SourceOrigin::SessionHistory))
        })
        .ok_or(Skip::NoDirectory)?;

    if !is_under(&cwd, under) {
        return Err(Skip::OutsideFolder);
    }
    let repo_url = repo_url_of(&cwd).ok_or(Skip::NotAGitRepo)?;
    if !configured.contains(&normalize_repo_url(&repo_url)) {
        return Err(Skip::RepoNotConfigured(repo_url));
    }

    Ok(PaneSource {
        instance_pid,
        terminal_view_id,
        agent,
        session_id: session_id.to_owned(),
        cwd,
        recovered_from,
    })
}

/// A status request failed because the instance is no longer running, as opposed to answering
/// slowly.
#[derive(Debug)]
pub(crate) struct InstanceGone(pub String);

impl fmt::Display for InstanceGone {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for InstanceGone {}

/// Asks for a move's step every `interval` until it is done or failed, or `timeout` has passed.
///
/// `call` answers with the step's wire form (see [`Response::MoveState`]). Errors other than
/// [`InstanceGone`] are retried until the timeout without counting against the move.
pub(crate) fn poll_until_terminal(
    call: &mut dyn FnMut() -> Result<String>,
    timeout: Duration,
    interval: Duration,
) -> MoveOutcome {
    let started = Instant::now();
    let mut untracked = 0;
    let mut gone = 0;
    loop {
        match call() {
            Ok(step) => {
                gone = 0;
                if let Some(job_id) = step.strip_prefix("done:") {
                    return MoveOutcome::Done {
                        job_id: job_id.to_owned(),
                    };
                }
                if let Some(reason) = step.strip_prefix("failed:") {
                    return MoveOutcome::Failed {
                        reason: reason.to_owned(),
                    };
                }
                if step == "none" {
                    untracked += 1;
                    if untracked >= MAX_CONSECUTIVE_LOST {
                        return MoveOutcome::Failed {
                            reason: "the instance has no record of this move".to_owned(),
                        };
                    }
                } else {
                    untracked = 0;
                }
            }
            Err(error) => {
                if let Some(InstanceGone(reason)) = error.downcast_ref::<InstanceGone>() {
                    gone += 1;
                    if gone >= MAX_CONSECUTIVE_LOST {
                        return MoveOutcome::Failed {
                            reason: reason.clone(),
                        };
                    }
                }
            }
        }
        if started.elapsed() >= timeout {
            return MoveOutcome::Failed {
                reason: "timed out".to_owned(),
            };
        }
        std::thread::sleep(interval);
    }
}

/// An instance whose panes were not considered, and why.
struct SkippedInstance {
    pid: u32,
    reason: String,
}

/// The instances that answered with their panes, and those that could not be asked.
///
/// Reads the markers itself rather than taking [`crate::remote_control::client::live_instances`],
/// because an instance on an older protocol has to be reported rather than silently dropped: only
/// windows on this build can carry out a move.
fn discover() -> (Vec<(Instance, Vec<PaneSummary>)>, Vec<SkippedInstance>) {
    let current_channel = format!("-v{PROTOCOL_VERSION}-");
    let mut answered = Vec::new();
    let mut skipped = Vec::new();
    for marker in address::read_markers() {
        let pid = marker.pid;
        let is_current = marker.address.contains(&current_channel);
        let Ok(instance) = Instance::connect(marker) else {
            if is_current {
                address::forget(pid);
            }
            continue;
        };
        if !is_current {
            skipped.push(SkippedInstance {
                pid,
                reason: "instance on an older protocol".to_owned(),
            });
            continue;
        }
        match instance.call(Request::ListPanes) {
            Ok(Response::Panes { panes }) => answered.push((instance, panes)),
            Ok(other) => skipped.push(SkippedInstance {
                pid,
                reason: format!("unexpected answer to a pane listing: {other:?}"),
            }),
            Err(error) => skipped.push(SkippedInstance {
                pid,
                reason: format!("{error:#}"),
            }),
        }
    }
    (answered, skipped)
}

/// Where each agent's past sessions ran, read once per agent.
struct SessionHistory {
    by_agent: std::cell::RefCell<HashMap<CLIAgent, HashMap<String, PathBuf>>>,
}

impl SessionHistory {
    fn new() -> Self {
        Self {
            by_agent: Default::default(),
        }
    }

    fn cwd_of(&self, agent: CLIAgent, session_id: &str) -> Option<PathBuf> {
        self.by_agent
            .borrow_mut()
            .entry(agent)
            .or_insert_with(|| {
                // `find_all` lists newest first, and the newest record of a session wins.
                let mut cwds = HashMap::new();
                for session in agent_session_history::find_all(agent) {
                    if !session.cwd.as_os_str().is_empty() {
                        cwds.entry(session.session_id).or_insert(session.cwd);
                    }
                }
                cwds
            })
            .get(session_id)
            .cloned()
    }
}

/// Runs `agent-ctrl move-all` and exits.
pub(crate) fn run(request: MoveAllRequest) -> Result<()> {
    let under = std::path::absolute(&request.under)
        .with_context(|| format!("Cannot resolve {}", request.under.display()))?;

    let configured = list_repositories(request.environment)
        .context("Could not list the repositories configured on AgentCtrl")?
        .iter()
        .map(|repository| normalize_repo_url(&repository.url))
        .collect::<Vec<_>>();

    let (instances, skipped_instances) = discover();
    let panes = instances
        .iter()
        .flat_map(|(instance, panes)| panes.iter().map(|pane| (instance.pid(), pane.clone())))
        .collect::<Vec<_>>();
    let titles = panes
        .iter()
        .filter_map(|(pid, pane)| Some(((*pid, pane.terminal_view_id?), pane.title.clone())))
        .collect::<HashMap<_, _>>();

    let history = SessionHistory::new();
    let (sources, skipped) = plan_moves(
        &panes,
        &under,
        &|agent, session_id| history.cwd_of(agent, session_id),
        &configured,
        &|cwd| repo_url_for(cwd).ok().map(|(_, url)| url),
    );

    let title_of = |source: &PaneSource| {
        titles
            .get(&(source.instance_pid, source.terminal_view_id))
            .cloned()
            .unwrap_or_default()
    };
    print!(
        "{}",
        format_plan(&under, &sources, &title_of, &skipped, &skipped_instances)
    );

    if sources.is_empty() {
        println!("Nothing to move.");
        return Ok(());
    }
    if !request.yes && !confirm(sources.len())? {
        println!("Nothing moved.");
        return Ok(());
    }

    let instances = instances
        .into_iter()
        .map(|(instance, _)| (instance.pid(), instance))
        .collect::<HashMap<_, _>>();
    let mut results = Vec::new();
    for source in &sources {
        println!(
            "Moving {} ({}) in {}…",
            title_of(source),
            source.agent.display_name(),
            source.cwd.display()
        );
        let outcome = match instances.get(&source.instance_pid) {
            Some(instance) => move_one(instance, source.terminal_view_id),
            None => MoveOutcome::Failed {
                reason: format!("instance {} is gone", source.instance_pid),
            },
        };
        println!("  {outcome}");
        results.push((source, outcome));
    }

    let summary = format_results(&results, &title_of);
    print!("{summary}");
    if let Err(error) = append_to_log(&under, &summary) {
        eprintln!("Could not write the move log: {error:#}");
    }

    let failed = results
        .iter()
        .filter(|(_, outcome)| matches!(outcome, MoveOutcome::Failed { .. }))
        .count();
    if failed > 0 {
        bail!("{failed} of {} moves failed", results.len());
    }
    Ok(())
}

fn move_one(instance: &Instance, terminal_view_id: u64) -> MoveOutcome {
    match instance.call(Request::MoveSessionToAgentCtrl { terminal_view_id }) {
        Ok(Response::MoveStarted) => {}
        Ok(Response::Failed { error }) => return MoveOutcome::Failed { reason: error },
        Ok(other) => {
            return MoveOutcome::Failed {
                reason: format!("unexpected answer to a move: {other:?}"),
            };
        }
        Err(error) => {
            return MoveOutcome::Failed {
                reason: format!("{error:#}"),
            };
        }
    }
    let pid = instance.pid();
    let mut status = || match instance.call(Request::MoveStatus { terminal_view_id }) {
        Ok(Response::MoveState { step }) => Ok(step),
        Ok(Response::Failed { error }) => bail!(error),
        Ok(other) => bail!("unexpected answer to a move status: {other:?}"),
        Err(error) if !client::is_listening(pid) => {
            Err(InstanceGone(format!("instance {pid} stopped answering: {error:#}")).into())
        }
        Err(error) => Err(error),
    };
    poll_until_terminal(&mut status, MOVE_TIMEOUT, POLL_INTERVAL)
}

fn confirm(count: usize) -> Result<bool> {
    print!("Move {count} sessions? [y/N] ");
    std::io::stdout().flush()?;
    let mut answer = String::new();
    std::io::stdin().lock().read_line(&mut answer)?;
    Ok(matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

fn format_plan(
    under: &Path,
    sources: &[PaneSource],
    title_of: &dyn Fn(&PaneSource) -> String,
    skipped: &[(PaneSummary, Skip)],
    skipped_instances: &[SkippedInstance],
) -> String {
    let mut out = format!(
        "Agent sessions under {} to move to AgentCtrl:\n",
        under.display()
    );
    if sources.is_empty() {
        out.push_str("  (none)\n");
    }
    for source in sources {
        out.push_str(&format!(
            "  {:<8} {:<30} {:<12} {} (from {})\n",
            source.instance_pid,
            title_of(source),
            source.agent.display_name(),
            source.cwd.display(),
            source.recovered_from
        ));
    }
    if !skipped.is_empty() || !skipped_instances.is_empty() {
        out.push_str("Skipped:\n");
    }
    for instance in skipped_instances {
        out.push_str(&format!(
            "  instance {:<8} {}\n",
            instance.pid, instance.reason
        ));
    }
    for (pane, skip) in skipped {
        out.push_str(&format!("  {:<39} {skip}\n", pane.title));
    }
    out
}

fn format_results(
    results: &[(&PaneSource, MoveOutcome)],
    title_of: &dyn Fn(&PaneSource) -> String,
) -> String {
    let rows = results
        .iter()
        .map(|(source, outcome)| {
            format!(
                "  {:<30} {:<40} {:<12} {outcome}",
                title_of(source),
                source.cwd.display(),
                source.agent.display_name()
            )
        })
        .join("\n");
    format!("Results:\n{rows}\n")
}

fn append_to_log(under: &Path, summary: &str) -> Result<()> {
    let path = warp_core::paths::state_dir().join(LOG_FILE_NAME);
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("Failed to open {}", path.display()))?;
    writeln!(
        file,
        "{} move-all --under {}\n{summary}",
        chrono::Local::now().to_rfc3339(),
        under.display()
    )
    .with_context(|| format!("Failed to write {}", path.display()))
}

#[cfg(test)]
#[path = "agent_ctrl_fleet_cli_tests.rs"]
mod tests;
