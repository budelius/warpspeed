//! The managed runs on AgentCtrl staging and the repositories it can start runs on, refreshed
//! while a fleet view is attached.
use std::time::Duration;

use managed_agent_sessions::{RepositoryInfo, RunSummary, normalize_repo_url};
use warp_cli::managed_sessions::ManagedEnvironment;
use warpui::{Entity, ModelContext, SingletonEntity};

use crate::terminal::CLIAgent;

/// The heading of the runs that carry no repository.
pub const NO_REPOSITORY: &str = "(no repository)";

const REFRESH_INTERVAL: Duration = Duration::from_secs(15);

/// One fetch of the dashboard's runs and repositories.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FleetSnapshot {
    pub runs: Vec<RunSummary>,
    pub repos: Vec<RepositoryInfo>,
}

/// The dashboard calls the model makes, all blocking and run off the main thread.
#[derive(Clone, Copy)]
pub struct FleetBackend {
    pub list: fn() -> anyhow::Result<FleetSnapshot>,
    /// Starts a run of an AgentCtrl profile (first argument) on a repository URL (second),
    /// returning its job id.
    pub start: fn(&str, &str) -> anyhow::Result<String>,
    pub stop: fn(&str) -> anyhow::Result<()>,
}

impl FleetBackend {
    pub fn staging() -> Self {
        Self {
            list: || {
                Ok(FleetSnapshot {
                    runs: managed_agent_sessions::list_runs(ManagedEnvironment::Staging)?,
                    repos: managed_agent_sessions::list_repositories(ManagedEnvironment::Staging)?,
                })
            },
            start: |profile, repo_url| {
                managed_agent_sessions::start_run_on_repo(
                    ManagedEnvironment::Staging,
                    profile,
                    repo_url,
                    None,
                    None,
                )
            },
            stop: |job_id| {
                managed_agent_sessions::stop_run(ManagedEnvironment::Staging, job_id, false)
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FleetGroup {
    pub repo_name: String,
    /// `None` for the runs under [`NO_REPOSITORY`].
    pub repo_url: Option<String>,
    pub rows: Vec<FleetRow>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FleetRow {
    pub job_id: String,
    pub state: String,
    pub agent: String,
    pub branch: Option<String>,
    /// The branch's page on the repository's web UI, for hosts whose layout is known.
    pub branch_url: Option<String>,
    pub last_activity: Option<String>,
}

impl FleetRow {
    pub fn is_finished(&self) -> bool {
        is_finished_state(&self.state)
    }
}

pub enum FleetModelEvent {
    Updated,
}

pub struct FleetModel {
    backend: FleetBackend,
    runs: Vec<RunSummary>,
    repos: Vec<RepositoryInfo>,
    last_error: Option<String>,
    refreshing: bool,
    attached_views: usize,
    /// Whether a refresh timer is pending, so re-attaching within an interval does not start a
    /// second polling chain.
    refresh_scheduled: bool,
}

impl FleetModel {
    pub fn new() -> Self {
        Self::with_backend(FleetBackend::staging())
    }

    pub fn with_backend(backend: FleetBackend) -> Self {
        Self {
            backend,
            runs: Vec::new(),
            repos: Vec::new(),
            last_error: None,
            refreshing: false,
            attached_views: 0,
            refresh_scheduled: false,
        }
    }

    pub fn repos(&self) -> &[RepositoryInfo] {
        &self.repos
    }

    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    pub fn is_refreshing(&self) -> bool {
        self.refreshing
    }

    pub fn groups(&self) -> Vec<FleetGroup> {
        group_runs(&self.runs, &self.repos)
    }

    /// Counts a fleet view as showing the runs; the first one starts polling.
    pub fn attach_view(&mut self, ctx: &mut ModelContext<Self>) {
        self.attached_views += 1;
        if self.attached_views == 1 {
            self.refresh(ctx);
            self.schedule_refresh(ctx);
        }
    }

    pub fn detach_view(&mut self) {
        self.attached_views = self.attached_views.saturating_sub(1);
    }

    pub fn refresh(&mut self, ctx: &mut ModelContext<Self>) {
        if self.refreshing {
            return;
        }
        self.refreshing = true;
        ctx.emit(FleetModelEvent::Updated);
        let list = self.backend.list;
        ctx.spawn(
            async move { blocking::unblock(list).await },
            |me, result, ctx| me.apply_refresh(result, ctx),
        );
    }

    fn apply_refresh(
        &mut self,
        result: anyhow::Result<FleetSnapshot>,
        ctx: &mut ModelContext<Self>,
    ) {
        self.refreshing = false;
        match result {
            Ok(snapshot) => {
                self.runs = snapshot.runs;
                self.repos = snapshot.repos;
                self.last_error = None;
            }
            Err(error) => {
                log::warn!("Refreshing the AgentCtrl fleet failed: {error:#}");
                self.last_error = Some(format!("Could not refresh the fleet: {error:#}"));
            }
        }
        ctx.emit(FleetModelEvent::Updated);
    }

    /// Starts `agent` on `repo_url`, then refreshes.
    pub fn start(&mut self, repo_url: String, agent: CLIAgent, ctx: &mut ModelContext<Self>) {
        let Some(profile) = agent_ctrl_profile(agent) else {
            log::error!("{agent:?} has no AgentCtrl profile to start");
            return;
        };
        let start = self.backend.start;
        ctx.spawn(
            async move { blocking::unblock(move || start(profile, &repo_url)).await },
            |me, result, ctx| {
                if let Err(error) = result {
                    me.last_error = Some(start_failure_message(&error));
                    ctx.emit(FleetModelEvent::Updated);
                }
                me.refresh(ctx);
            },
        );
    }

    /// Asks AgentCtrl to stop `job_id` gracefully, then refreshes.
    pub fn stop(&mut self, job_id: String, ctx: &mut ModelContext<Self>) {
        let stop = self.backend.stop;
        ctx.spawn(
            async move { blocking::unblock(move || stop(&job_id)).await },
            |me, result, ctx| {
                if let Err(error) = result {
                    me.last_error = Some(format!("Could not stop the run: {error:#}"));
                    ctx.emit(FleetModelEvent::Updated);
                }
                me.refresh(ctx);
            },
        );
    }

    fn schedule_refresh(&mut self, ctx: &mut ModelContext<Self>) {
        if self.refresh_scheduled || !should_poll(self.attached_views) {
            return;
        }
        self.refresh_scheduled = true;
        ctx.spawn(
            async { warpui::r#async::Timer::after(REFRESH_INTERVAL).await },
            |me, _, ctx| {
                me.refresh_scheduled = false;
                if should_poll(me.attached_views) {
                    me.refresh(ctx);
                    me.schedule_refresh(ctx);
                }
            },
        );
    }
}

impl Default for FleetModel {
    fn default() -> Self {
        Self::new()
    }
}

impl Entity for FleetModel {
    type Event = FleetModelEvent;
}

impl SingletonEntity for FleetModel {}

/// Whether the fleet is polled while `attached_views` fleet views show it.
pub(crate) fn should_poll(attached_views: usize) -> bool {
    attached_views > 0
}

pub fn is_finished_state(state: &str) -> bool {
    matches!(state, "completed" | "failed" | "stopped" | "interrupted")
}

#[cfg(feature = "local_fs")]
fn agent_ctrl_profile(agent: CLIAgent) -> Option<&'static str> {
    crate::terminal::agent_session_handoff::agent_ctrl_profile(agent)
}

#[cfg(not(feature = "local_fs"))]
fn agent_ctrl_profile(agent: CLIAgent) -> Option<&'static str> {
    log::warn!("Starting {agent:?} on AgentCtrl needs local filesystem support");
    None
}

#[cfg(feature = "local_fs")]
fn start_failure_message(error: &anyhow::Error) -> String {
    crate::terminal::agent_session_handoff::start_agent_failure_message(error)
}

#[cfg(not(feature = "local_fs"))]
fn start_failure_message(error: &anyhow::Error) -> String {
    format!("Could not start the agent on AgentCtrl: {error:#}")
}

/// `runs` grouped by repository, the groups sorted by name with [`NO_REPOSITORY`] last and the
/// live runs of each group before the finished ones.
pub(crate) fn group_runs(runs: &[RunSummary], repos: &[RepositoryInfo]) -> Vec<FleetGroup> {
    let mut groups: Vec<FleetGroup> = Vec::new();
    let mut unattributed = Vec::new();
    for run in runs {
        let Some(repo_url) = run.repo_url.as_deref().filter(|url| !url.trim().is_empty()) else {
            unattributed.push(fleet_row(run, None));
            continue;
        };
        let repo_url = normalize_repo_url(repo_url);
        let row = fleet_row(run, Some(&repo_url));
        match groups
            .iter_mut()
            .find(|group| group.repo_url.as_deref() == Some(repo_url.as_str()))
        {
            Some(group) => group.rows.push(row),
            None => groups.push(FleetGroup {
                repo_name: repository_name(&repo_url, repos),
                repo_url: Some(repo_url),
                rows: vec![row],
            }),
        }
    }
    groups.sort_by(|a, b| a.repo_name.cmp(&b.repo_name));
    if !unattributed.is_empty() {
        groups.push(FleetGroup {
            repo_name: NO_REPOSITORY.to_owned(),
            repo_url: None,
            rows: unattributed,
        });
    }
    for group in &mut groups {
        group.rows.sort_by_key(FleetRow::is_finished);
    }
    groups
}

fn fleet_row(run: &RunSummary, repo_url: Option<&str>) -> FleetRow {
    FleetRow {
        job_id: run.job_id.clone(),
        state: run.state.clone(),
        agent: run.agent_name.clone(),
        branch: run.branch.clone(),
        branch_url: repo_url
            .zip(run.branch.as_deref())
            .and_then(|(repo_url, branch)| branch_web_url(repo_url, branch)),
        last_activity: run.last_activity_at.clone(),
    }
}

/// AgentCtrl's name for `repo_url`, else its path on the host.
fn repository_name(repo_url: &str, repos: &[RepositoryInfo]) -> String {
    repos
        .iter()
        .find(|repo| normalize_repo_url(&repo.url) == repo_url)
        .map(|repo| repo.name.clone())
        .or_else(|| {
            repo_url
                .split_once("://")
                .and_then(|(_, rest)| rest.split_once('/'))
                .map(|(_, path)| path.to_owned())
                .filter(|path| !path.is_empty())
        })
        .unwrap_or_else(|| repo_url.to_owned())
}

/// The page of `branch` on the web UI of `repo_url`, for the Gitea at `git-dev.obachan.dev` and
/// for `gitlab.com`.
pub fn branch_web_url(repo_url: &str, branch: &str) -> Option<String> {
    if branch.is_empty() {
        return None;
    }
    let normalized = normalize_repo_url(repo_url);
    let (_, rest) = normalized.split_once("://")?;
    let (authority, path) = rest.split_once('/')?;
    let host = authority.split(':').next().unwrap_or(authority);
    if path.is_empty() {
        return None;
    }
    let branch = branch
        .split('/')
        .map(|segment| urlencoding::encode(segment).into_owned())
        .collect::<Vec<_>>()
        .join("/");
    match host {
        "git-dev.obachan.dev" => Some(format!("https://{host}/{path}/src/branch/{branch}")),
        "gitlab.com" => Some(format!("https://{host}/{path}/-/tree/{branch}")),
        _ => None,
    }
}

#[cfg(test)]
#[path = "fleet_model_tests.rs"]
mod tests;
