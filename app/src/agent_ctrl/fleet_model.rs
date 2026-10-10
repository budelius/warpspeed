//! The managed runs on AgentCtrl staging and the repositories it can start runs on, refreshed
//! while a consuming view is attached.
use std::collections::{HashMap, HashSet};
use std::time::Duration;

use managed_agent_sessions::{
    RepositoryInfo, RunPreview, RunSummary, is_paused_state, normalize_repo_url,
};
use warp_cli::managed_sessions::ManagedEnvironment;
use warpui::{Entity, EntityId, ModelContext, SingletonEntity};

use crate::terminal::CLIAgent;

/// The agents a fleet or pane start offers, with their button labels.
pub(crate) const STARTABLE_AGENTS: [(CLIAgent, &str); 5] = [
    (CLIAgent::Claude, "Claude"),
    (CLIAgent::Pi, "Pi"),
    (CLIAgent::Codex, "Codex"),
    (CLIAgent::Kimi, "Kimi"),
    (CLIAgent::DeepSeekHarness, "DeepSeek"),
];

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
    pub pause: fn(&str) -> anyhow::Result<()>,
    pub resume: fn(&str) -> anyhow::Result<()>,
    pub resume_for_attach: fn(&str) -> anyhow::Result<()>,
    pub preview: fn(&str) -> anyhow::Result<RunPreview>,
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
            pause: |job_id| {
                managed_agent_sessions::pause_run(ManagedEnvironment::Staging, job_id).map(drop)
            },
            resume: |job_id| {
                managed_agent_sessions::resume_run(ManagedEnvironment::Staging, job_id).map(drop)
            },
            resume_for_attach: |job_id| {
                managed_agent_sessions::resume_run_and_wait(
                    ManagedEnvironment::Staging,
                    job_id,
                    Duration::from_secs(60),
                )
            },
            preview: |job_id| {
                managed_agent_sessions::fetch_run_preview(ManagedEnvironment::Staging, job_id, 12)
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
    /// The AgentCtrl node the run is on, or last ran on while paused.
    pub node_name: Option<String>,
    /// Whether the dashboard reports that the run can be paused now.
    pub pause_allowed: bool,
    pub branch: Option<String>,
    /// The branch's page on the repository's web UI, for hosts whose layout is known.
    pub branch_url: Option<String>,
    pub last_activity: Option<String>,
}

impl FleetRow {
    pub fn is_finished(&self) -> bool {
        is_finished_state(&self.state)
    }

    pub fn is_paused(&self) -> bool {
        is_paused_state(&self.state)
    }

    /// Only a run the dashboard reports as pausable has an agent session AgentCtrl can resume.
    pub fn can_pause(&self) -> bool {
        self.pause_allowed && self.state == "running"
    }

    pub fn can_resume(&self) -> bool {
        self.is_paused()
    }

    fn display_rank(&self) -> u8 {
        if self.is_finished() {
            2
        } else if self.is_paused() {
            1
        } else {
            0
        }
    }
}

pub enum FleetModelEvent {
    Updated,
    PreviewUpdated,
    ResumeForAttachSucceeded { job_id: String, requester: EntityId },
}

pub struct FleetModel {
    backend: FleetBackend,
    runs: Vec<RunSummary>,
    repos: Vec<RepositoryInfo>,
    /// Why the last refresh failed; cleared by the next successful one.
    last_error: Option<String>,
    /// Why the last start, stop, pause or resume failed; kept across refreshes until the next
    /// such action, or until dismissed.
    action_error: Option<String>,
    refreshing: bool,
    /// Set when a refresh is asked for while one is in flight, so the newer state is fetched once
    /// it lands.
    refresh_again: bool,
    previews: HashMap<String, Result<RunPreview, String>>,
    previewing: HashSet<String>,
    attached_views: HashSet<EntityId>,
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
            action_error: None,
            refreshing: false,
            refresh_again: false,
            previews: HashMap::new(),
            previewing: HashSet::new(),
            attached_views: HashSet::new(),
            refresh_scheduled: false,
        }
    }

    pub fn repos(&self) -> &[RepositoryInfo] {
        &self.repos
    }

    pub fn runs(&self) -> &[RunSummary] {
        &self.runs
    }

    pub fn preview(&self, job_id: &str) -> Option<&Result<RunPreview, String>> {
        self.previews.get(job_id)
    }

    pub fn is_previewing(&self, job_id: &str) -> bool {
        self.previewing.contains(job_id)
    }

    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    pub fn action_error(&self) -> Option<&str> {
        self.action_error.as_deref()
    }

    pub fn dismiss_action_error(&mut self, ctx: &mut ModelContext<Self>) {
        if self.action_error.take().is_some() {
            ctx.emit(FleetModelEvent::Updated);
        }
    }

    pub fn set_action_error(&mut self, error: String, ctx: &mut ModelContext<Self>) {
        self.action_error = Some(error);
        ctx.emit(FleetModelEvent::Updated);
    }

    pub fn attached_view_count(&self) -> usize {
        self.attached_views.len()
    }

    pub fn is_refreshing(&self) -> bool {
        self.refreshing
    }

    pub fn groups(&self) -> Vec<FleetGroup> {
        group_runs(&self.runs, &self.repos)
    }

    /// Counts `view_id` as showing the runs; the first one starts polling. Attaching an attached
    /// view again changes nothing.
    pub fn attach_view(&mut self, view_id: EntityId, ctx: &mut ModelContext<Self>) {
        if self.attached_views.insert(view_id) && self.attached_views.len() == 1 {
            self.refresh(ctx);
            self.schedule_refresh(ctx);
        }
    }

    /// Stops counting `view_id`. Panes are detached more than once on close (hidden for undo,
    /// then closed), so detaching a detached view changes nothing.
    pub fn detach_view(&mut self, view_id: EntityId) {
        self.attached_views.remove(&view_id);
    }

    pub fn request_preview(&mut self, job_id: String, force: bool, ctx: &mut ModelContext<Self>) {
        if self.previewing.contains(&job_id) || (!force && self.previews.contains_key(&job_id)) {
            return;
        }
        self.previewing.insert(job_id.clone());
        ctx.emit(FleetModelEvent::PreviewUpdated);
        let preview = self.backend.preview;
        let completed_job_id = job_id.clone();
        ctx.spawn(
            async move { blocking::unblock(move || preview(&job_id)).await },
            move |me, result, ctx| {
                me.previewing.remove(&completed_job_id);
                me.previews.insert(
                    completed_job_id.clone(),
                    result.map_err(|error| format!("Could not load transcript: {error:#}")),
                );
                ctx.emit(FleetModelEvent::PreviewUpdated);
            },
        );
    }

    pub fn refresh(&mut self, ctx: &mut ModelContext<Self>) {
        if self.refreshing {
            self.refresh_again = true;
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
                let job_ids = self
                    .runs
                    .iter()
                    .map(|run| run.job_id.as_str())
                    .collect::<HashSet<_>>();
                self.previews
                    .retain(|job_id, _| job_ids.contains(job_id.as_str()));
                self.last_error = None;
            }
            Err(error) => {
                log::warn!("Refreshing the AgentCtrl fleet failed: {error:#}");
                self.last_error = Some(format!("Could not refresh the fleet: {error:#}"));
            }
        }
        ctx.emit(FleetModelEvent::Updated);
        if std::mem::take(&mut self.refresh_again) {
            self.refresh(ctx);
        }
    }

    /// Starts `agent` on `repo_url`, then refreshes.
    pub fn start(&mut self, repo_url: String, agent: CLIAgent, ctx: &mut ModelContext<Self>) {
        let Some(profile) = agent_ctrl_profile(agent) else {
            log::error!("{agent:?} has no AgentCtrl profile to start");
            return;
        };
        self.action_error = None;
        let start = self.backend.start;
        ctx.spawn(
            async move { blocking::unblock(move || start(profile, &repo_url)).await },
            |me, result, ctx| {
                if let Err(error) = result {
                    me.action_error = Some(start_failure_message(&error));
                    ctx.emit(FleetModelEvent::Updated);
                }
                me.refresh(ctx);
            },
        );
    }

    /// Asks AgentCtrl to stop `job_id` gracefully, then refreshes.
    pub fn stop(&mut self, job_id: String, ctx: &mut ModelContext<Self>) {
        let stop = self.backend.stop;
        self.run_command(
            job_id,
            stop,
            managed_agent_sessions::stop_failure_message,
            ctx,
        );
    }

    /// Asks AgentCtrl to pause `job_id`, then refreshes.
    pub fn pause(&mut self, job_id: String, ctx: &mut ModelContext<Self>) {
        let pause = self.backend.pause;
        self.run_command(
            job_id,
            pause,
            managed_agent_sessions::pause_failure_message,
            ctx,
        );
    }

    /// Asks AgentCtrl to resume `job_id`, then refreshes.
    pub fn resume(&mut self, job_id: String, ctx: &mut ModelContext<Self>) {
        let resume = self.backend.resume;
        self.run_command(
            job_id,
            resume,
            managed_agent_sessions::resume_failure_message,
            ctx,
        );
    }

    /// Resumes `job_id`, waits until its session is live, then tells only the requesting view.
    pub fn resume_for_attach(
        &mut self,
        job_id: String,
        requester: EntityId,
        ctx: &mut ModelContext<Self>,
    ) {
        self.action_error = None;
        let resume = self.backend.resume_for_attach;
        let completed_job_id = job_id.clone();
        ctx.spawn(
            async move { blocking::unblock(move || resume(&job_id)).await },
            move |me, result, ctx| {
                match result {
                    Ok(()) => ctx.emit(FleetModelEvent::ResumeForAttachSucceeded {
                        job_id: completed_job_id,
                        requester,
                    }),
                    Err(error) => {
                        me.action_error =
                            Some(managed_agent_sessions::resume_failure_message(&error));
                        ctx.emit(FleetModelEvent::Updated);
                    }
                }
                me.refresh(ctx);
            },
        );
    }

    fn run_command(
        &mut self,
        job_id: String,
        command: fn(&str) -> anyhow::Result<()>,
        failure_message: fn(&anyhow::Error) -> String,
        ctx: &mut ModelContext<Self>,
    ) {
        self.action_error = None;
        ctx.spawn(
            async move { blocking::unblock(move || command(&job_id)).await },
            move |me, result, ctx| {
                if let Err(error) = result {
                    me.action_error = Some(failure_message(&error));
                    ctx.emit(FleetModelEvent::Updated);
                }
                me.refresh(ctx);
            },
        );
    }

    fn schedule_refresh(&mut self, ctx: &mut ModelContext<Self>) {
        if self.refresh_scheduled || !should_poll(self.attached_view_count()) {
            return;
        }
        self.refresh_scheduled = true;
        ctx.spawn(
            async { warpui::r#async::Timer::after(REFRESH_INTERVAL).await },
            |me, _, ctx| {
                me.refresh_scheduled = false;
                if should_poll(me.attached_view_count()) {
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
/// live runs of each group first, then the paused ones, then the finished ones.
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
        group.rows.sort_by_key(FleetRow::display_rank);
    }
    groups
}

fn fleet_row(run: &RunSummary, repo_url: Option<&str>) -> FleetRow {
    FleetRow {
        job_id: run.job_id.clone(),
        state: run.state.clone(),
        agent: run.agent_name.clone(),
        node_name: run.node_name.clone(),
        pause_allowed: run.pause_allowed.unwrap_or(false),
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
