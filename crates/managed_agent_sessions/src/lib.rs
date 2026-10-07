use std::collections::HashSet;
use std::io::{self, Write as _};
use std::path::PathBuf;
use std::time::Duration;
use std::{fs, thread};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use instant::Instant;
use reqwest::StatusCode;
use reqwest::blocking::multipart::{Form, Part};
use reqwest::blocking::{Client, Response};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use warp_cli::managed_sessions::{ManagedEnvironment, ManagedSessionsArgs, ManagedSessionsCommand};

const STAGING_BASE_URL: &str = "https://dash-backend-dev.stozn.ai";
const STAGING_WEB_URL: &str = "https://dash-dev.stozn.ai";
const STAGING_FLEET_ID: &str = "stoz3n-agent-ctrl-staging";
const SUPPORTED_PROTOCOL: u32 = 1;
const BOOKMARK_FORMAT: u32 = 1;
const STOZN_AUTH_FORMAT: u32 = 1;
const STOZN_LOGIN_MAX_WAIT: Duration = Duration::from_secs(330);
pub const MANAGED_SESSION_DEEP_LINK_SCHEME: &str = "warpspeed";
pub const MANAGED_SESSION_DEEP_LINK_HOST: &str = "agent-ctrl";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedSessionDeepLink {
    pub environment: ManagedEnvironment,
    pub job_id: String,
}

impl ManagedSessionDeepLink {
    pub fn cli_arguments(&self) -> String {
        let environment = match self.environment {
            ManagedEnvironment::Staging => "staging",
        };
        format!(
            "agent-ctrl open {} --environment {environment}",
            self.job_id
        )
    }
}

pub fn parse_managed_session_deep_link(url: &reqwest::Url) -> Result<ManagedSessionDeepLink> {
    if url.scheme() != MANAGED_SESSION_DEEP_LINK_SCHEME
        || url.host_str() != Some(MANAGED_SESSION_DEEP_LINK_HOST)
        || url.path() != "/attach"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.fragment().is_some()
    {
        bail!("invalid WarpSpeed managed-session link");
    }

    let mut environment = None;
    let mut fleet_id = None;
    let mut job_id = None;
    for (key, value) in url.query_pairs() {
        let target = match key.as_ref() {
            "environment" => &mut environment,
            "fleet_id" => &mut fleet_id,
            "job_id" => &mut job_id,
            _ => bail!("invalid WarpSpeed managed-session link parameter"),
        };
        if target.replace(value.into_owned()).is_some() {
            bail!("duplicate WarpSpeed managed-session link parameter");
        }
    }

    if environment.as_deref() != Some("staging") || fleet_id.as_deref() != Some(STAGING_FLEET_ID) {
        bail!("WarpSpeed managed-session link targets an unsupported environment");
    }
    let job_id = job_id.context("WarpSpeed managed-session link omitted its job id")?;
    validate_job_id(&job_id)?;
    Ok(ManagedSessionDeepLink {
        environment: ManagedEnvironment::Staging,
        job_id,
    })
}

#[derive(Debug, Clone)]
pub struct StoznLoginFlow {
    request_id: String,
    verifier: String,
    verification_url: String,
    user_code: String,
    expires_at: DateTime<Utc>,
    poll_interval: Duration,
}

impl StoznLoginFlow {
    pub fn verification_url(&self) -> &str {
        &self.verification_url
    }

    pub fn user_code(&self) -> &str {
        &self.user_code
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoznIdentity {
    #[serde(rename = "sub")]
    pub subject: String,
    pub role: String,
    pub display_name: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct StoredStoznAuth {
    format_version: u32,
    environment_alias: String,
    token: String,
    expires_at: String,
    user: StoznIdentity,
}

#[derive(Debug, Deserialize)]
struct StoznLoginStart {
    request_id: String,
    verification_url: String,
    user_code: String,
    expires_at: String,
    poll_interval_seconds: u64,
}

#[derive(Debug, Deserialize)]
struct StoznLoginPoll {
    status: String,
    token: Option<String>,
    expires_at: Option<String>,
    user: Option<StoznIdentity>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ManagedSessionBookmark {
    format_version: u32,
    environment_alias: String,
    fleet_id: String,
    job_id: String,
    last_cursor: Option<String>,
    display_name: Option<String>,
    agent_name: Option<String>,
    last_known_state: Option<String>,
    created_at: String,
    last_opened_at: String,
}

#[derive(Debug, Deserialize)]
struct SessionDescriptor {
    protocol_version: u32,
    environment: String,
    fleet_id: String,
    job_id: String,
    display_name: String,
    agent_name: String,
    state: String,
    capabilities: Vec<String>,
    next_cursor: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct RunEvent {
    event_id: String,
    job_id: String,
    sequence: u64,
    timestamp: String,
    #[serde(rename = "type")]
    event_type: String,
    content: String,
    redacted: bool,
    truncated: bool,
}

#[derive(Debug, Deserialize)]
struct EventPage {
    items: Vec<RunEvent>,
    next_cursor: Option<String>,
    terminal: bool,
    gap: bool,
    earliest_cursor: Option<String>,
}

pub fn begin_stozn_login() -> Result<StoznLoginFlow> {
    let profile = profile(ManagedEnvironment::Staging);
    let verifier = format!("{}{}", uuid::Uuid::new_v4(), uuid::Uuid::new_v4());
    let request = Client::new()
        .post(format!("{}/api/auth/warpspeed/start", profile.base_url))
        .header("X-Stozn-Client", "warpspeed")
        .json(&json!({ "verifier": verifier }));
    let started: StoznLoginStart = send(profile, request)?
        .json()
        .context("invalid STOZN login start response")?;
    if started.request_id.len() != 43
        || !started
            .request_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
    {
        bail!("dashboard returned an invalid STOZN login request ID");
    }
    let expires_at = DateTime::parse_from_rfc3339(&started.expires_at)
        .context("dashboard returned an invalid STOZN login expiry")?
        .with_timezone(&Utc);
    if expires_at <= Utc::now() || started.poll_interval_seconds == 0 {
        bail!("dashboard returned an expired STOZN login request");
    }
    validate_verification_url(profile, &started.verification_url, &started.request_id)?;
    if !valid_user_code(&started.user_code) {
        bail!("dashboard returned an invalid STOZN confirmation code");
    }
    Ok(StoznLoginFlow {
        request_id: started.request_id,
        verifier,
        verification_url: started.verification_url,
        user_code: started.user_code,
        expires_at,
        poll_interval: Duration::from_secs(started.poll_interval_seconds.clamp(1, 10)),
    })
}

pub fn complete_stozn_login(flow: StoznLoginFlow) -> Result<StoznIdentity> {
    let profile = profile(ManagedEnvironment::Staging);
    let started = Instant::now();
    loop {
        if Utc::now() >= flow.expires_at || started.elapsed() >= STOZN_LOGIN_MAX_WAIT {
            bail!("STOZN login request expired");
        }
        let request = Client::new()
            .post(format!("{}/api/auth/warpspeed/poll", profile.base_url))
            .header("X-Stozn-Client", "warpspeed")
            .json(&json!({
                "request_id": flow.request_id,
                "verifier": flow.verifier,
            }));
        validate_request_destination(profile, &request)?;
        let response = request.send().context("STOZN login polling failed")?;
        if response.status() == StatusCode::ACCEPTED {
            thread::sleep(flow.poll_interval);
            continue;
        }
        if !response.status().is_success() {
            return Err(http_error(response));
        }
        let approved: StoznLoginPoll = response
            .json()
            .context("invalid STOZN login approval response")?;
        if approved.status != "approved" {
            bail!("dashboard returned an invalid STOZN login status");
        }
        let token = approved
            .token
            .context("STOZN login response omitted its token")?;
        let expires_at = approved
            .expires_at
            .context("STOZN login response omitted its expiry")?;
        let identity = approved
            .user
            .context("STOZN login response omitted its user")?;
        if !valid_dashboard_token(&token) || identity.role != "admin" {
            bail!("dashboard returned invalid STOZN login credentials");
        }
        let parsed_expiry = DateTime::parse_from_rfc3339(&expires_at)
            .context("dashboard returned an invalid STOZN token expiry")?
            .with_timezone(&Utc);
        if parsed_expiry <= Utc::now() {
            bail!("dashboard returned expired STOZN login credentials");
        }
        save_stozn_auth(&StoredStoznAuth {
            format_version: STOZN_AUTH_FORMAT,
            environment_alias: profile.alias.to_owned(),
            token,
            expires_at,
            user: identity.clone(),
        })?;
        return Ok(identity);
    }
}

pub fn stored_stozn_identity() -> Option<StoznIdentity> {
    load_stozn_auth().ok().map(|auth| auth.user)
}

pub fn logout_stozn() -> Result<()> {
    let path = stozn_auth_path();
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("failed to remove {}", path.display())),
    }
}

pub fn run(args: &ManagedSessionsArgs) -> Result<()> {
    match &args.command {
        ManagedSessionsCommand::List => list(args.json),
        ManagedSessionsCommand::Start {
            agent,
            task,
            repo,
            branch,
            prompt,
            interactive,
            environment,
        } => {
            let agent = resolve_agent_name(*environment, agent)?;
            match (repo, task) {
                (Some(repo), _) => start_on_repo(
                    *environment,
                    &agent,
                    repo,
                    branch.as_deref(),
                    prompt.as_deref(),
                    args.json,
                ),
                (None, Some(task)) => start(
                    *environment,
                    &agent,
                    task,
                    prompt.as_deref(),
                    *interactive,
                    args.json,
                ),
                (None, None) => bail!("start requires --task or --repo"),
            }
        }
        ManagedSessionsCommand::Repos { environment } => repos(*environment, args.json),
        ManagedSessionsCommand::MoveAll { .. } => bail!("move-all is handled by the application"),
        ManagedSessionsCommand::Open {
            job_id,
            environment,
        } => ui_session(*environment, job_id),
        ManagedSessionsCommand::Attach {
            job_id,
            environment,
            once,
        } => attach(*environment, job_id, *once, args.json),
        ManagedSessionsCommand::Input {
            job_id,
            text,
            environment,
        } => input(*environment, job_id, Some(text), args.json),
        ManagedSessionsCommand::Close {
            job_id,
            environment,
        } => input(*environment, job_id, None, args.json),
        ManagedSessionsCommand::Stop {
            job_id,
            force,
            environment,
        } => stop(*environment, job_id, *force, args.json),
        ManagedSessionsCommand::Pause {
            job_id,
            environment,
        } => pause(*environment, job_id, args.json),
        ManagedSessionsCommand::Resume {
            job_id,
            environment,
        } => resume(*environment, job_id, args.json),
    }
}

fn list(json_output: bool) -> Result<()> {
    let profile = profile(ManagedEnvironment::Staging);
    let token = dashboard_token()?;
    let page = fetch_runs_page(profile, &token)?;
    if json_output {
        println!("{}", serde_json::to_string_pretty(&page)?);
        return Ok(());
    }
    let runs = parse_items::<RunSummary>(page, "managed-run list")?;
    if runs.is_empty() {
        println!("No AgentCtrl managed runs in staging.");
    }
    for run in runs {
        let repository = run.repo_url.as_deref().unwrap_or("-");
        println!(
            "{}  {:<12} {:<24} {} {repository}",
            run.job_id, run.state, run.agent_name, profile.alias
        );
    }
    Ok(())
}

fn repos(environment: ManagedEnvironment, json_output: bool) -> Result<()> {
    let repositories = list_repositories(environment)?;
    if json_output {
        println!("{}", serde_json::to_string_pretty(&repositories)?);
        return Ok(());
    }
    if repositories.is_empty() {
        println!("No repositories are configured on the AgentCtrl node.");
    }
    for repository in repositories {
        let reachable = if repository.reachable {
            "reachable"
        } else {
            "unreachable"
        };
        let error = repository
            .error
            .map(|error| format!("  ({error})"))
            .unwrap_or_default();
        println!(
            "{:<24} {:<20} {reachable:<12} {}{error}",
            repository.name, repository.branch, repository.url
        );
    }
    Ok(())
}

/// A repository the AgentCtrl node has configured for fresh interactive runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepositoryInfo {
    pub name: String,
    pub url: String,
    pub branch: String,
    pub reachable: bool,
    #[serde(default)]
    pub error: Option<String>,
}

/// One entry of the dashboard's managed-run list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunSummary {
    pub job_id: String,
    pub state: String,
    pub agent_name: String,
    #[serde(default)]
    pub repo_url: Option<String>,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub interactive: bool,
    #[serde(default)]
    pub last_activity_at: Option<String>,
    /// The AgentCtrl node the run is on, or last ran on while paused.
    #[serde(default)]
    pub node_name: Option<String>,
    #[serde(default)]
    pub paused_at: Option<String>,
    /// Whether the dashboard would accept a pause now; absent from dashboards that predate it.
    #[serde(default)]
    pub pause_allowed: Option<bool>,
}

pub fn list_repositories(environment: ManagedEnvironment) -> Result<Vec<RepositoryInfo>> {
    let token = dashboard_token()?;
    list_repositories_with(profile(environment), &token)
}

fn list_repositories_with(
    profile: &EnvironmentProfile,
    token: &str,
) -> Result<Vec<RepositoryInfo>> {
    let page: Value = send(
        profile,
        Client::new()
            .get(format!("{}/api/agent-control/repos", profile.base_url))
            .bearer_auth(token)
            .header("X-Stozn-Client", "warpspeed"),
    )?
    .json()
    .context("invalid repository list response")?;
    parse_items(page, "repository list")
}

pub fn list_runs(environment: ManagedEnvironment) -> Result<Vec<RunSummary>> {
    let token = dashboard_token()?;
    list_runs_with(profile(environment), &token)
}

fn list_runs_with(profile: &EnvironmentProfile, token: &str) -> Result<Vec<RunSummary>> {
    parse_items(fetch_runs_page(profile, token)?, "managed-run list")
}

fn fetch_runs_page(profile: &EnvironmentProfile, token: &str) -> Result<Value> {
    send(
        profile,
        Client::new()
            .get(format!(
                "{}/api/agent-control/runs?limit=100",
                profile.base_url
            ))
            .bearer_auth(token)
            .header("X-Stozn-Client", "warpspeed"),
    )?
    .json()
    .context("invalid managed-run list response")
}

fn parse_items<T: serde::de::DeserializeOwned>(mut page: Value, what: &str) -> Result<Vec<T>> {
    let items = page
        .get_mut("items")
        .map(Value::take)
        .with_context(|| format!("{what} did not contain items"))?;
    serde_json::from_value(items).with_context(|| format!("invalid {what} items"))
}

/// The AgentCtrl profile that runs moved and fleet sessions of a CLI agent (`claude`, `pi`,
/// `codex`).
pub fn agent_ctrl_profile(agent: &str) -> Option<&'static str> {
    match agent {
        "claude" => Some("staging-claude-interactive-rw"),
        "pi" => Some("staging-pi-interactive-rw"),
        "codex" => Some("staging-codex-interactive-rw"),
        _ => None,
    }
}

/// The CLI agent kinds that [`agent_ctrl_profile`] maps to an AgentCtrl profile.
pub const AGENT_KINDS: [&str; 3] = ["claude", "pi", "codex"];

/// The AgentCtrl profile that `agent` names: a CLI agent kind maps through
/// [`agent_ctrl_profile`], and a name with the environment's profile prefix is taken as a
/// configured profile. Anything else fails before any request, listing what is accepted.
pub fn resolve_agent_name(environment: ManagedEnvironment, agent: &str) -> Result<String> {
    if let Some(profile_name) = agent_ctrl_profile(agent) {
        return Ok(profile_name.to_owned());
    }
    let alias = profile(environment).alias;
    let is_profile_name = agent
        .strip_prefix(alias)
        .and_then(|rest| rest.strip_prefix('-'))
        .is_some_and(|rest| {
            !rest.is_empty()
                && rest
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
        });
    if is_profile_name {
        return Ok(agent.to_owned());
    }
    bail!(
        "unknown agent {agent:?}; use {} or a profile name starting with {alias}- (for example {alias}-claude-interactive-rw)",
        AGENT_KINDS.join(", ")
    )
}

/// Canonical form of a repository URL, identical to the AgentCtrl node's rule: scheme and host
/// lowercased, userinfo, trailing `/` and `.git` dropped; scp-style `user@host:path` becomes
/// `https://host/path`. Local paths are returned unchanged.
pub fn normalize_repo_url(url: &str) -> String {
    let url = url.trim().trim_end_matches('/');
    let url = url.strip_suffix(".git").unwrap_or(url);
    let (scheme, host, path) = match url.split_once("://") {
        Some((scheme, rest)) => {
            let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
            let host = authority.rsplit('@').next().unwrap_or(authority);
            (scheme, host, path)
        }
        None => match split_scp_url(url) {
            Some((host, path)) => ("https", host, path.trim_start_matches('/')),
            None => return url.to_owned(),
        },
    };
    format!(
        "{}://{}/{}",
        scheme.to_ascii_lowercase(),
        host.to_ascii_lowercase(),
        path
    )
    .trim_end_matches('/')
    .to_owned()
}

/// `(host, path)` of an scp-style `user@host:path`. The userinfo is required so that a Windows
/// drive path (`C:\...`) never matches, and neither it nor the host may contain a path separator,
/// so a local path with `@` and `:` in it stays a path.
fn split_scp_url(url: &str) -> Option<(&str, &str)> {
    url.match_indices('@').find_map(|(at, _)| {
        let userinfo = &url[..at];
        let (host, path) = url[at + 1..].split_once(':')?;
        let separator = |character| matches!(character, '/' | '\\');
        (!userinfo.is_empty()
            && !host.is_empty()
            && !userinfo.contains(separator)
            && !host.contains(separator))
        .then_some((host, path))
    })
}

fn start(
    environment: ManagedEnvironment,
    agent: &str,
    task_id: &str,
    prompt: Option<&str>,
    interactive: bool,
    json_output: bool,
) -> Result<()> {
    let token = dashboard_token()?;
    let result = launch_request_with(
        profile(environment),
        &token,
        agent,
        LaunchTarget::Task(task_id),
        prompt,
        interactive,
    )?;
    if json_output {
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else if let Some(job_id) = result.get("job_id").and_then(Value::as_str) {
        println!("Started {job_id}. Attach with: warpspeed agent-ctrl attach {job_id}");
    } else {
        println!("{}", serde_json::to_string_pretty(&result)?);
    }
    Ok(())
}

fn start_on_repo(
    environment: ManagedEnvironment,
    agent: &str,
    repo_url: &str,
    base_branch: Option<&str>,
    prompt: Option<&str>,
    json_output: bool,
) -> Result<()> {
    let job_id = start_run_on_repo(environment, agent, repo_url, base_branch, prompt)?;
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({ "status": "accepted", "job_id": job_id }))?
        );
    } else {
        println!("Started {job_id}. Open with: warpspeed agent-ctrl open {job_id}");
    }
    Ok(())
}

/// Starts an interactive run in a fresh worktree of a configured repository and returns its job
/// id. A node at its interactive-run cap fails with an [`HttpStatusError`] that
/// [`is_node_at_capacity`] recognizes.
pub fn start_run_on_repo(
    environment: ManagedEnvironment,
    agent_name: &str,
    repo_url: &str,
    base_branch: Option<&str>,
    prompt: Option<&str>,
) -> Result<String> {
    let token = dashboard_token()?;
    start_run_on_repo_with(
        profile(environment),
        &token,
        agent_name,
        repo_url,
        base_branch,
        prompt,
    )
}

fn start_run_on_repo_with(
    profile: &EnvironmentProfile,
    token: &str,
    agent_name: &str,
    repo_url: &str,
    base_branch: Option<&str>,
    prompt: Option<&str>,
) -> Result<String> {
    let result = launch_request_with(
        profile,
        token,
        agent_name,
        LaunchTarget::Repository {
            url: repo_url,
            base_branch,
        },
        prompt,
        true,
    )?;
    result
        .get("job_id")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .context("launch response did not include a job id")
}

/// What a launched run is bound to.
enum LaunchTarget<'a> {
    Task(&'a str),
    Repository {
        url: &'a str,
        base_branch: Option<&'a str>,
    },
}

fn launch_request_with(
    profile: &EnvironmentProfile,
    token: &str,
    agent: &str,
    target: LaunchTarget<'_>,
    prompt: Option<&str>,
    interactive: bool,
) -> Result<Value> {
    if agent.is_empty() || agent.len() > 128 {
        bail!("agent name must contain 1 to 128 characters");
    }
    if prompt.is_some_and(|value| value.is_empty() || value.chars().count() > 32_000) {
        bail!("prompt must contain 1 to 32000 characters");
    }
    let mut body = json!({
        "agent_name": agent,
        "interactive": interactive
    });
    match target {
        LaunchTarget::Task(task_id) => {
            validate_task_id(task_id)?;
            body["task_id"] = Value::String(task_id.to_owned());
        }
        LaunchTarget::Repository { url, base_branch } => {
            if url.trim().is_empty() || url.chars().count() > 512 {
                bail!("repository URL must contain 1 to 512 characters");
            }
            body["repo_url"] = Value::String(url.to_owned());
            if let Some(base_branch) = base_branch {
                if !valid_base_branch(base_branch) {
                    bail!("invalid base branch {base_branch:?}");
                }
                body["base_branch"] = Value::String(base_branch.to_owned());
            }
        }
    }
    if let Some(prompt) = prompt {
        body["prompt"] = Value::String(prompt.to_owned());
    }
    let response = send(
        profile,
        Client::new()
            .post(format!("{}/api/agent-control/runs", profile.base_url))
            .bearer_auth(token)
            .header("X-Stozn-Client", "warpspeed")
            .header(
                "Idempotency-Key",
                format!("warpspeed-{}", uuid::Uuid::new_v4()),
            )
            .json(&body),
    )?;
    let result: Value = response.json().context("invalid launch response")?;
    ensure_command_accepted(&result)?;
    Ok(result)
}

fn ui_session(environment: ManagedEnvironment, job_id: &str) -> Result<()> {
    loop {
        attach(environment, job_id, true, false)?;
        let profile = profile(environment);
        let token = dashboard_token()?;
        let descriptor = fetch_descriptor(&Client::new(), profile, job_id, &token)?;
        if session_is_finished(&descriptor.state) {
            println!("Session finished with state {}.", descriptor.state);
            return Ok(());
        }
        println!("\n{}", viewer_commands(&descriptor.state));
        let command = read_line("> ")?;
        if command.is_empty() || command == "refresh" {
            continue;
        }
        if command == "detach" || command == "quit" {
            println!("Detached. The managed run was not stopped.");
            return Ok(());
        }
        if command == "close" {
            keep_viewing(input(environment, job_id, None, false));
            continue;
        }
        if command == "stop" {
            keep_viewing(stop(environment, job_id, false, false));
            continue;
        }
        if command == "resume" {
            match resume_run(environment, job_id) {
                Ok(outcome) => println!("{}", run_control_summary("Resumed", job_id, &outcome)),
                Err(error) => println!("{}", resume_failure_message(&error)),
            }
            continue;
        }
        if let Some(text) = command.strip_prefix("send ") {
            keep_viewing(input(environment, job_id, Some(text.trim()), false));
            continue;
        }
        println!("Unknown command. Use send <text>, close, resume, stop, refresh, or detach.");
    }
}

/// Reports a failed viewer command and lets the viewer carry on, as a paused run refuses `send`,
/// `close` and `stop`.
fn keep_viewing(result: Result<()>) {
    if let Err(error) = result {
        println!("{}", failure_detail(&error));
    }
}

fn session_is_finished(state: &str) -> bool {
    matches!(state, "completed" | "failed" | "stopped" | "interrupted")
}

/// A paused run is live (not finished) but runs no agent until it is resumed.
pub fn is_paused_state(state: &str) -> bool {
    state == "paused"
}

fn paused_notice(job_id: &str) -> String {
    format!("Paused — resume with: warpspeed agent-ctrl resume {job_id}")
}

fn attach_banner(descriptor: &SessionDescriptor) -> String {
    let capabilities = descriptor.capabilities.join(",");
    let mut banner = format!(
        "Attached to {} ({}, capabilities: {capabilities}). Closing this view does not stop the run.",
        descriptor.display_name, descriptor.state
    );
    if is_paused_state(&descriptor.state) {
        banner.push('\n');
        banner.push_str(&paused_notice(&descriptor.job_id));
    }
    banner
}

fn viewer_commands(state: &str) -> &'static str {
    if is_paused_state(state) {
        "Commands: Enter=refresh, resume, stop, detach"
    } else {
        "Commands: Enter=refresh, send <text>, close, stop, detach"
    }
}

fn read_line(prompt: &str) -> Result<String> {
    print!("{prompt}");
    io::stdout().flush().context("failed to flush prompt")?;
    let mut line = String::new();
    if io::stdin().read_line(&mut line)? == 0 {
        bail!("managed-session viewer input closed");
    }
    Ok(line.trim().to_owned())
}

fn input(
    environment: ManagedEnvironment,
    job_id: &str,
    text: Option<&str>,
    json_output: bool,
) -> Result<()> {
    let token = dashboard_token()?;
    let result = post_input_with(profile(environment), &token, job_id, text)?;
    if json_output {
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else {
        let action = if text.is_some() { "input" } else { "close" };
        println!("Managed-session {action} accepted for {job_id}.");
    }
    Ok(())
}

/// Sends one follow-up turn to a live interactive run.
pub fn send_input(environment: ManagedEnvironment, job_id: &str, text: &str) -> Result<()> {
    let token = dashboard_token()?;
    post_input_with(profile(environment), &token, job_id, Some(text)).map(drop)
}

/// Posts `text` as input, or closes the run's input when `text` is `None`.
fn post_input_with(
    profile: &EnvironmentProfile,
    token: &str,
    job_id: &str,
    text: Option<&str>,
) -> Result<Value> {
    validate_job_id(job_id)?;
    if text.is_some_and(|value| value.is_empty() || value.chars().count() > 8_000) {
        bail!("input must contain 1 to 8000 characters");
    }
    let action = if text.is_some() { "input" } else { "close" };
    let body = text.map_or_else(|| json!({}), |text| json!({ "text": text }));
    let response = send(
        profile,
        Client::new()
            .post(format!(
                "{}/api/agent-control/runs/{job_id}/{action}",
                profile.base_url
            ))
            .bearer_auth(token)
            .header("X-Stozn-Client", "warpspeed")
            .header(
                "Idempotency-Key",
                format!("warpspeed-{}", uuid::Uuid::new_v4()),
            )
            .json(&body),
    )?;
    let result: Value = response.json().context("invalid input response")?;
    ensure_command_accepted(&result)?;
    Ok(result)
}

fn stop(
    environment: ManagedEnvironment,
    job_id: &str,
    force: bool,
    json_output: bool,
) -> Result<()> {
    validate_job_id(job_id)?;
    let token = dashboard_token()?;
    let result = stop_run_with(profile(environment), &token, job_id, force)?;
    if json_output {
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else {
        println!("Stop request accepted for {job_id}.");
    }
    Ok(())
}

fn pause(environment: ManagedEnvironment, job_id: &str, json_output: bool) -> Result<()> {
    let outcome = pause_run(environment, job_id)
        .map_err(|error| anyhow::Error::msg(pause_failure_message(&error)))?;
    print_run_control_outcome("Paused", job_id, &outcome, json_output)
}

fn resume(environment: ManagedEnvironment, job_id: &str, json_output: bool) -> Result<()> {
    let outcome = resume_run(environment, job_id)
        .map_err(|error| anyhow::Error::msg(resume_failure_message(&error)))?;
    print_run_control_outcome("Resumed", job_id, &outcome, json_output)
}

fn print_run_control_outcome(
    verb: &str,
    job_id: &str,
    outcome: &RunControlOutcome,
    json_output: bool,
) -> Result<()> {
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&run_control_json(job_id, outcome))?
        );
    } else {
        println!("{}", run_control_summary(verb, job_id, outcome));
    }
    Ok(())
}

fn run_control_json(job_id: &str, outcome: &RunControlOutcome) -> Value {
    json!({
        "job_id": job_id,
        "state": outcome.state,
        "node_name": outcome.node_name,
    })
}

fn run_control_summary(verb: &str, job_id: &str, outcome: &RunControlOutcome) -> String {
    let state = if outcome.state.is_empty() {
        "unknown"
    } else {
        &outcome.state
    };
    let expected = match verb {
        "Paused" => "paused",
        _ => "running",
    };
    let location = outcome
        .node_name
        .as_ref()
        .map(|node| format!(" on {node}"))
        .unwrap_or_default();
    if state == expected {
        format!("{verb} {job_id} ({state}){location}")
    } else {
        let request = match verb {
            "Paused" => "Pause",
            _ => "Resume",
        };
        format!("{request} requested for {job_id}; still {state}{location}")
    }
}

/// Asks the dashboard to stop a managed run.
pub fn stop_run(environment: ManagedEnvironment, job_id: &str, force: bool) -> Result<()> {
    validate_job_id(job_id)?;
    let token = dashboard_token()?;
    stop_run_with(profile(environment), &token, job_id, force).map(drop)
}

fn stop_run_with(
    profile: &EnvironmentProfile,
    token: &str,
    job_id: &str,
    force: bool,
) -> Result<Value> {
    post_run_command_with(profile, token, job_id, "stop", &json!({ "force": force }))
}

/// Asks the dashboard to pause a live interactive run: its agent exits, its work is pushed to the
/// run's branch, and it stops holding a node slot. A run whose agent session AgentCtrl does not
/// know fails with an error [`is_session_unknown`] recognizes.
pub fn pause_run(environment: ManagedEnvironment, job_id: &str) -> Result<RunControlOutcome> {
    validate_job_id(job_id)?;
    let token = dashboard_token()?;
    pause_run_with(profile(environment), &token, job_id).map(|result| run_control_outcome(&result))
}

/// Asks the dashboard to resume a paused run on a node with a free slot. With every node full it
/// fails with an error [`is_node_at_capacity`] recognizes.
pub fn resume_run(environment: ManagedEnvironment, job_id: &str) -> Result<RunControlOutcome> {
    validate_job_id(job_id)?;
    let token = dashboard_token()?;
    resume_run_with(profile(environment), &token, job_id).map(|result| run_control_outcome(&result))
}

/// What the dashboard reports after accepting a pause or resume.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunControlOutcome {
    pub state: String,
    pub node_name: Option<String>,
}

fn run_control_outcome(result: &Value) -> RunControlOutcome {
    let text = |key| result.get(key).and_then(Value::as_str).map(str::to_owned);
    RunControlOutcome {
        state: text("state").unwrap_or_default(),
        node_name: text("node_name"),
    }
}

fn pause_run_with(profile: &EnvironmentProfile, token: &str, job_id: &str) -> Result<Value> {
    post_run_command_with(profile, token, job_id, "pause", &json!({}))
}

fn resume_run_with(profile: &EnvironmentProfile, token: &str, job_id: &str) -> Result<Value> {
    post_run_command_with(profile, token, job_id, "resume", &json!({}))
}

/// Posts `body` to the run's `command` route and requires the dashboard to accept it.
fn post_run_command_with(
    profile: &EnvironmentProfile,
    token: &str,
    job_id: &str,
    command: &str,
    body: &Value,
) -> Result<Value> {
    let response = send(
        profile,
        Client::new()
            .post(format!(
                "{}/api/agent-control/runs/{job_id}/{command}",
                profile.base_url
            ))
            .bearer_auth(token)
            .header("X-Stozn-Client", "warpspeed")
            .header(
                "Idempotency-Key",
                format!("warpspeed-{}", uuid::Uuid::new_v4()),
            )
            .json(body),
    )?;
    let result: Value = response
        .json()
        .with_context(|| format!("invalid {command} response"))?;
    ensure_command_accepted(&result)?;
    Ok(result)
}

fn attach(
    environment: ManagedEnvironment,
    job_id: &str,
    once: bool,
    json_output: bool,
) -> Result<()> {
    validate_job_id(job_id)?;
    let profile = profile(environment);
    let token = dashboard_token()?;
    let client = Client::builder().timeout(Duration::from_secs(25)).build()?;
    let descriptor = fetch_descriptor(&client, profile, job_id, &token)?;

    let mut bookmarks = load_bookmarks()?;
    let existing_cursor = bookmarks
        .iter()
        .find(|bookmark| {
            bookmark.environment_alias == profile.alias
                && bookmark.fleet_id == descriptor.fleet_id
                && bookmark.job_id == job_id
        })
        .and_then(|bookmark| bookmark.last_cursor.clone());
    let now = Utc::now().to_rfc3339();
    upsert_bookmark(
        &mut bookmarks,
        ManagedSessionBookmark {
            format_version: BOOKMARK_FORMAT,
            environment_alias: profile.alias.to_owned(),
            fleet_id: descriptor.fleet_id.clone(),
            job_id: descriptor.job_id.clone(),
            last_cursor: existing_cursor.clone(),
            display_name: Some(descriptor.display_name.clone()),
            agent_name: Some(descriptor.agent_name.clone()),
            last_known_state: Some(descriptor.state.clone()),
            created_at: now.clone(),
            last_opened_at: now,
        },
    );
    save_bookmarks(&bookmarks)?;

    if !json_output {
        println!("{}", attach_banner(&descriptor));
    }

    let mut cursor = existing_cursor;
    let mut known_events = HashSet::new();
    loop {
        let wait_ms = if once { 0 } else { 15_000 };
        let (page, history_gap) =
            fetch_event_page(&client, profile, job_id, &token, cursor.as_deref(), wait_ms)?;
        if history_gap || page.gap {
            if json_output {
                println!(
                    "{}",
                    json!({ "type": "history_gap", "earliest_cursor": page.earliest_cursor })
                );
            } else {
                println!("--- retained event history has a gap ---");
            }
        }
        for event in retain_new_events(page.items, &mut known_events) {
            render_event(&event, json_output)?;
        }
        cursor = page.next_cursor;
        update_bookmark_cursor(
            &mut bookmarks,
            profile,
            job_id,
            cursor.clone(),
            page.terminal,
        );
        save_bookmarks(&bookmarks)?;
        if once || page.terminal {
            break;
        }
        if cursor.is_none() && descriptor.next_cursor.is_none() {
            thread::sleep(Duration::from_millis(500));
        }
    }
    Ok(())
}

pub const MAX_SESSION_FILE_BYTES: usize = 33_554_432;

pub struct MovedSessionImport<'a> {
    pub agent_name: &'a str,
    pub session_id: &'a str,
    pub session_file: Vec<u8>,
    pub repo_url: &'a str,
    pub handoff_branch: &'a str,
    pub source_cwd: &'a str,
    pub idempotency_key: String,
}

/// Uploads a local session file to the dashboard and returns the new run's job id.
pub fn import_moved_session(
    environment: ManagedEnvironment,
    import: MovedSessionImport<'_>,
) -> Result<String> {
    let token = dashboard_token()?;
    import_moved_session_with(profile(environment), &token, import)
}

fn import_moved_session_with(
    profile: &EnvironmentProfile,
    token: &str,
    import: MovedSessionImport<'_>,
) -> Result<String> {
    if import.session_file.len() > MAX_SESSION_FILE_BYTES {
        bail!("session file is larger than the 32 MiB import limit");
    }
    let form = Form::new()
        .text("agent_name", import.agent_name.to_owned())
        .text("session_id", import.session_id.to_owned())
        .text("repo_url", import.repo_url.to_owned())
        .text("handoff_branch", import.handoff_branch.to_owned())
        .text("source_cwd", import.source_cwd.to_owned())
        .part(
            "session_file",
            Part::bytes(import.session_file).file_name("session.jsonl"),
        );
    let client = Client::builder()
        .timeout(Duration::from_secs(300))
        .build()?;
    // Multipart bodies cannot be cloned, so validate the built request rather than using `send`.
    let request = client
        .post(format!(
            "{}/api/agent-control/runs/import",
            profile.base_url
        ))
        .bearer_auth(token)
        .header("X-Stozn-Client", "warpspeed")
        .header("Idempotency-Key", import.idempotency_key)
        .multipart(form)
        .build()
        .context("invalid managed-session import request")?;
    validate_destination_url(profile, request.url())?;
    let response = client
        .execute(request)
        .context("dashboard request failed")?;
    let response = if response.status().is_success() {
        response
    } else {
        return Err(http_error(response));
    };
    let result: Value = response.json().context("invalid import response")?;
    ensure_command_accepted(&result)?;
    result
        .get("job_id")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .context("import response did not include a job id")
}

/// Blocks until the imported run reports that its session has resumed.
pub fn wait_until_resumed(
    environment: ManagedEnvironment,
    job_id: &str,
    timeout: Duration,
) -> Result<()> {
    validate_job_id(job_id)?;
    let token = dashboard_token()?;
    let client = Client::builder().timeout(Duration::from_secs(25)).build()?;
    wait_until_resumed_with(
        &client,
        profile(environment),
        &token,
        job_id,
        Instant::now() + timeout,
    )
}

fn wait_until_resumed_with(
    client: &Client,
    profile: &EnvironmentProfile,
    token: &str,
    job_id: &str,
    deadline: Instant,
) -> Result<()> {
    let started = Instant::now();
    let mut cursor: Option<String> = None;
    loop {
        let (idle, last_error) =
            match fetch_event_page(client, profile, job_id, token, cursor.as_deref(), 15_000) {
                Ok((page, history_gap)) => {
                    if page
                        .items
                        .iter()
                        .any(|event| event.event_type == "status" && event.content == "resumed")
                    {
                        return Ok(());
                    }
                    if page.terminal {
                        bail!("managed run {job_id} ended before resuming");
                    }
                    // The cursor fell out of retained history; replaying from the start still
                    // includes the `resumed` event if it was emitted.
                    cursor = if history_gap {
                        None
                    } else {
                        page.next_cursor.or(cursor)
                    };
                    (page.items.is_empty() && cursor.is_none(), None)
                }
                Err(error) if is_rejection(&error) => {
                    return Err(error.context(format!("could not watch managed run {job_id}")));
                }
                // The run is already importing; a dashboard blip must not abandon it early.
                Err(error) => (true, Some(error)),
            };
        if Instant::now() >= deadline {
            let secs = started.elapsed().as_secs();
            match last_error {
                Some(error) => {
                    bail!("managed run {job_id} did not resume within {secs}s: {error:#}")
                }
                None => bail!("managed run {job_id} did not resume within {secs}s"),
            }
        }
        if idle {
            thread::sleep(Duration::from_millis(500));
        }
    }
}

fn fetch_descriptor(
    client: &Client,
    profile: &EnvironmentProfile,
    job_id: &str,
    token: &str,
) -> Result<SessionDescriptor> {
    let descriptor: SessionDescriptor = send(
        profile,
        client
            .get(format!(
                "{}/api/agent-control/runs/{job_id}/session",
                profile.base_url
            ))
            .bearer_auth(token)
            .header("X-Stozn-Client", "warpspeed"),
    )?
    .json()
    .context("invalid managed-session descriptor")?;
    validate_descriptor(profile, job_id, &descriptor)?;
    Ok(descriptor)
}

fn fetch_event_page(
    client: &Client,
    profile: &EnvironmentProfile,
    job_id: &str,
    token: &str,
    cursor: Option<&str>,
    wait_ms: u64,
) -> Result<(EventPage, bool)> {
    let wait = wait_ms.to_string();
    let mut request = client
        .get(format!(
            "{}/api/agent-control/runs/{job_id}/events",
            profile.base_url
        ))
        .bearer_auth(token)
        .header("X-Stozn-Client", "warpspeed")
        .query(&[("limit", "100"), ("wait_ms", wait.as_str())]);
    if let Some(after) = cursor {
        request = request.query(&[("after", after)]);
    }
    validate_request_destination(profile, &request)?;
    let response = request
        .send()
        .context("managed-session event request failed")?;
    let history_gap = response.status() == StatusCode::GONE;
    let response = if history_gap || response.status().is_success() {
        response
    } else {
        return Err(http_error(response));
    };
    let page = response
        .json()
        .context("invalid managed-session event page")?;
    Ok((page, history_gap))
}

fn retain_new_events(events: Vec<RunEvent>, known_events: &mut HashSet<String>) -> Vec<RunEvent> {
    events
        .into_iter()
        .filter(|event| known_events.insert(event.event_id.clone()))
        .collect()
}

fn render_event(event: &RunEvent, json_output: bool) -> Result<()> {
    const TYPES: &[&str] = &[
        "assistant",
        "tool_use",
        "stdout",
        "stderr",
        "user_input",
        "status",
        "system",
    ];
    if json_output {
        if TYPES.contains(&event.event_type.as_str()) {
            println!("{}", serde_json::to_string(event)?);
        } else {
            println!(
                "{}",
                json!({
                    "event_id": event.event_id,
                    "job_id": event.job_id,
                    "sequence": event.sequence,
                    "timestamp": event.timestamp,
                    "type": "unsupported",
                    "content": "[unsupported event type]"
                })
            );
        }
        return Ok(());
    }
    if TYPES.contains(&event.event_type.as_str()) {
        let mut markers = String::new();
        if event.redacted {
            markers.push_str(" [redacted]");
        }
        if event.truncated {
            markers.push_str(" [truncated]");
        }
        println!(
            "{:>6} {:<10} {}{markers}",
            event.sequence, event.event_type, event.content
        );
    } else {
        println!("{:>6} unsupported [unsupported event type]", event.sequence);
    }
    Ok(())
}

struct EnvironmentProfile {
    alias: &'static str,
    base_url: &'static str,
    web_url: &'static str,
    fleet_id: &'static str,
}

fn profile(environment: ManagedEnvironment) -> &'static EnvironmentProfile {
    static STAGING: EnvironmentProfile = EnvironmentProfile {
        alias: "staging",
        base_url: STAGING_BASE_URL,
        web_url: STAGING_WEB_URL,
        fleet_id: STAGING_FLEET_ID,
    };
    match environment {
        ManagedEnvironment::Staging => &STAGING,
    }
}

fn validate_descriptor(
    profile: &EnvironmentProfile,
    requested_job_id: &str,
    descriptor: &SessionDescriptor,
) -> Result<()> {
    if descriptor.protocol_version != SUPPORTED_PROTOCOL {
        bail!(
            "unsupported managed-session protocol {}",
            descriptor.protocol_version
        );
    }
    if descriptor.environment != profile.alias
        || descriptor.fleet_id != profile.fleet_id
        || descriptor.job_id != requested_job_id
    {
        bail!("managed-session identity does not match the selected environment");
    }
    Ok(())
}

fn valid_user_code(value: &str) -> bool {
    value.len() == 9
        && value.bytes().enumerate().all(|(index, byte)| {
            if index == 4 {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit() && !byte.is_ascii_lowercase()
            }
        })
}

fn validate_task_id(task_id: &str) -> Result<()> {
    let valid = !task_id.is_empty()
        && task_id.len() <= 128
        && task_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._:-".contains(&byte));
    if !valid {
        bail!("invalid STOZN task id");
    }
    Ok(())
}

/// Same rule as the dashboard's `base_branch` check: allowed characters only, at most 100, no
/// leading `-` or `/`, no trailing `/`, no `..` or `//`, and no component starting with `.`.
fn valid_base_branch(branch: &str) -> bool {
    (1..=100).contains(&branch.len())
        && branch
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._/-".contains(&byte))
        && !branch.starts_with(['-', '/'])
        && !branch.ends_with('/')
        && !branch.contains("..")
        && !branch.contains("//")
        && !branch
            .split('/')
            .any(|component| component.starts_with('.'))
}

fn validate_job_id(job_id: &str) -> Result<()> {
    let valid = job_id.starts_with("job-")
        && job_id.len() <= 128
        && job_id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-');
    if !valid {
        bail!("invalid AgentCtrl job id");
    }
    Ok(())
}

fn valid_dashboard_token(token: &str) -> bool {
    !token.is_empty()
        && token.len() <= 8_192
        && token.bytes().all(|byte| (0x21..=0x7e).contains(&byte))
}

fn dashboard_token() -> Result<String> {
    if let Ok(token) = std::env::var("STOZN_TOKEN") {
        if valid_dashboard_token(&token) {
            return Ok(token);
        }
        bail!("STOZN_TOKEN must contain 1 to 8192 visible ASCII bytes");
    }
    if let Ok(auth) = load_stozn_auth() {
        return Ok(auth.token);
    }
    let path = dirs::home_dir()
        .context("home directory is unavailable")?
        .join(".stozn.json");
    let value: Value = serde_json::from_slice(&fs::read(&path).with_context(|| {
        format!(
            "not signed in to STOZN; use the WarpSpeed by STOZN login screen ({})",
            path.display()
        )
    })?)
    .context("invalid ~/.stozn.json")?;
    let token = value.get("token").and_then(Value::as_str).unwrap_or("");
    if !valid_dashboard_token(token) {
        bail!("~/.stozn.json does not contain a valid visible-ASCII token");
    }
    Ok(token.to_owned())
}

fn stozn_auth_path() -> PathBuf {
    warp_core::paths::state_dir().join("stozn-auth.bin")
}

fn load_stozn_auth() -> Result<StoredStoznAuth> {
    let path = stozn_auth_path();
    let protected =
        fs::read(&path).with_context(|| format!("failed to read {}", path.display()))?;
    let plaintext = unprotect_auth_bytes(protected)?;
    let auth: StoredStoznAuth = serde_json::from_slice(&plaintext)
        .context("invalid WarpSpeed STOZN authentication state")?;
    let expires_at = DateTime::parse_from_rfc3339(&auth.expires_at)
        .context("invalid WarpSpeed STOZN authentication expiry")?
        .with_timezone(&Utc);
    if auth.format_version != STOZN_AUTH_FORMAT
        || auth.environment_alias != profile(ManagedEnvironment::Staging).alias
        || auth.user.role != "admin"
        || expires_at <= Utc::now()
        || !valid_dashboard_token(&auth.token)
    {
        bail!("WarpSpeed STOZN authentication is invalid or expired");
    }
    Ok(auth)
}

fn save_stozn_auth(auth: &StoredStoznAuth) -> Result<()> {
    let path = stozn_auth_path();
    let parent = path.parent().context("STOZN auth path has no parent")?;
    fs::create_dir_all(parent)?;
    let temp = path.with_extension("bin.tmp");
    let plaintext = serde_json::to_vec(auth)?;
    fs::write(&temp, protect_auth_bytes(plaintext)?)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(&temp, fs::Permissions::from_mode(0o600))?;
    }
    if cfg!(windows) && path.exists() {
        fs::remove_file(&path)?;
    }
    fs::rename(&temp, &path)?;
    Ok(())
}

#[cfg(not(windows))]
fn protect_auth_bytes(plaintext: Vec<u8>) -> Result<Vec<u8>> {
    Ok(plaintext)
}

#[cfg(not(windows))]
fn unprotect_auth_bytes(protected: Vec<u8>) -> Result<Vec<u8>> {
    Ok(protected)
}

#[cfg(windows)]
fn protect_auth_bytes(mut plaintext: Vec<u8>) -> Result<Vec<u8>> {
    use windows::Win32::Foundation::{HLOCAL, LocalFree};
    use windows::Win32::Security::Cryptography::{CRYPT_INTEGER_BLOB, CryptProtectData};
    use windows::core::BSTR;

    let input = CRYPT_INTEGER_BLOB {
        cbData: plaintext.len() as u32,
        pbData: plaintext.as_mut_ptr(),
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    unsafe {
        CryptProtectData(
            &input,
            &BSTR::from("WarpSpeed by STOZN dashboard auth"),
            None,
            None,
            None,
            0,
            &mut output,
        )
        .context("failed to protect the STOZN desktop credential")?;
        let encrypted = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        LocalFree(Some(HLOCAL(output.pbData.cast())));
        Ok(encrypted)
    }
}

#[cfg(windows)]
fn unprotect_auth_bytes(mut protected: Vec<u8>) -> Result<Vec<u8>> {
    use windows::Win32::Foundation::{HLOCAL, LocalFree};
    use windows::Win32::Security::Cryptography::{CRYPT_INTEGER_BLOB, CryptUnprotectData};

    let input = CRYPT_INTEGER_BLOB {
        cbData: protected.len() as u32,
        pbData: protected.as_mut_ptr(),
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    unsafe {
        CryptUnprotectData(&input, None, None, None, None, 0, &mut output)
            .context("failed to unlock the STOZN desktop credential for this OS user")?;
        let plaintext = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        LocalFree(Some(HLOCAL(output.pbData.cast())));
        Ok(plaintext)
    }
}

fn validate_verification_url(
    profile: &EnvironmentProfile,
    verification_url: &str,
    request_id: &str,
) -> Result<()> {
    let allowed = reqwest::Url::parse(profile.web_url)?;
    let requested = reqwest::Url::parse(verification_url)
        .context("dashboard returned an invalid STOZN login URL")?;
    let same_origin = requested.scheme() == allowed.scheme()
        && requested.host_str() == allowed.host_str()
        && requested.port_or_known_default() == allowed.port_or_known_default();
    let matching_request = requested.path() == "/warpspeed-auth"
        && requested
            .query_pairs()
            .any(|(key, value)| key == "request_id" && value == request_id);
    if !same_origin
        || !matching_request
        || !requested.username().is_empty()
        || requested.password().is_some()
        || requested.fragment().is_some()
    {
        bail!("dashboard returned an unsafe STOZN login URL");
    }
    Ok(())
}

fn ensure_command_accepted(result: &Value) -> Result<()> {
    if result.get("status").and_then(Value::as_str) == Some("accepted")
        || result.get("accepted").and_then(Value::as_bool) == Some(true)
    {
        return Ok(());
    }
    let message = result
        .pointer("/error/error")
        .and_then(Value::as_str)
        .unwrap_or("managed-session command was rejected");
    bail!("{message}")
}

fn validate_request_destination(
    profile: &EnvironmentProfile,
    request: &reqwest::blocking::RequestBuilder,
) -> Result<()> {
    let request = request
        .try_clone()
        .context("managed-session request cannot be safely inspected")?
        .build()
        .context("invalid managed-session request")?;
    validate_destination_url(profile, request.url())
}

fn validate_destination_url(profile: &EnvironmentProfile, requested: &reqwest::Url) -> Result<()> {
    let allowed = reqwest::Url::parse(profile.base_url)
        .context("invalid compiled managed-session dashboard URL")?;
    let same_origin = requested.scheme() == allowed.scheme()
        && requested.host_str() == allowed.host_str()
        && requested.port_or_known_default() == allowed.port_or_known_default()
        && requested.username().is_empty()
        && requested.password().is_none();
    if !same_origin {
        bail!("managed-session network request refused: destination is not the selected dashboard");
    }
    Ok(())
}

fn send(
    profile: &EnvironmentProfile,
    request: reqwest::blocking::RequestBuilder,
) -> Result<Response> {
    validate_request_destination(profile, &request)?;
    let response = request.send().context("dashboard request failed")?;
    if response.status().is_success() {
        Ok(response)
    } else {
        Err(http_error(response))
    }
}

/// A non-success dashboard response, kept typed so callers can tell rejections from blips.
#[derive(Debug)]
pub struct HttpStatusError {
    pub status: StatusCode,
    pub body: String,
}

impl std::fmt::Display for HttpStatusError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "dashboard returned {}: {}", self.status, self.body)
    }
}

impl std::error::Error for HttpStatusError {}

fn http_error(response: Response) -> anyhow::Error {
    let status = response.status();
    let body = response.text().unwrap_or_default();
    HttpStatusError { status, body }.into()
}

/// Whether the dashboard refused a launch or resume because no node has a free interactive slot.
pub fn is_node_at_capacity(error: &anyhow::Error) -> bool {
    has_error_code(error, StatusCode::CONFLICT, "node_at_capacity")
}

/// Whether the dashboard refused a pause because AgentCtrl does not know the run's agent session,
/// without which the run could never be resumed.
pub fn is_session_unknown(error: &anyhow::Error) -> bool {
    has_error_code(error, StatusCode::PRECONDITION_FAILED, "session_unknown")
}

fn has_error_code(error: &anyhow::Error, status: StatusCode, code: &str) -> bool {
    error
        .downcast_ref::<HttpStatusError>()
        .is_some_and(|error| {
            error.status == status
                && serde_json::from_str::<Value>(&error.body)
                    .is_ok_and(|body| body.get("code").and_then(Value::as_str) == Some(code))
        })
}

/// Removes `user[:password]@` from every URL in `text`, since git and HTTP errors can echo a
/// remote URL that embeds a token.
pub fn redact_url_credentials(text: &str) -> String {
    let mut redacted = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(scheme_end) = rest.find("://") {
        let authority_start = scheme_end + 3;
        redacted.push_str(&rest[..authority_start]);
        rest = &rest[authority_start..];
        let authority_end = rest
            .find(|c: char| matches!(c, '/' | '\'' | '"') || c.is_whitespace())
            .unwrap_or(rest.len());
        if let Some(at) = rest[..authority_end].rfind('@') {
            rest = &rest[at + 1..];
        }
    }
    redacted.push_str(rest);
    redacted
}

/// The dashboard's `error` text for a rejected request, or the whole error chain when the failure
/// carries no such text; URL credentials are removed either way.
fn failure_detail(error: &anyhow::Error) -> String {
    let dashboard_text = error
        .downcast_ref::<HttpStatusError>()
        .and_then(|error| serde_json::from_str::<Value>(&error.body).ok())
        .and_then(|body| body.get("error").and_then(Value::as_str).map(str::to_owned));
    redact_url_credentials(&dashboard_text.unwrap_or_else(|| format!("{error:#}")))
}

/// The message shown when asking the dashboard to stop a run failed with `error`.
pub fn stop_failure_message(error: &anyhow::Error) -> String {
    format!("Could not stop the run: {}", failure_detail(error))
}

/// The message shown when asking the dashboard to pause a run failed with `error`.
pub fn pause_failure_message(error: &anyhow::Error) -> String {
    if is_session_unknown(error) {
        "AgentCtrl does not know this run's agent session, so it cannot be paused".to_owned()
    } else {
        format!("Could not pause the run: {}", failure_detail(error))
    }
}

/// The message shown when asking the dashboard to resume a run failed with `error`.
pub fn resume_failure_message(error: &anyhow::Error) -> String {
    if is_node_at_capacity(error) {
        "Every AgentCtrl node is at capacity; stop or pause a run, then resume again".to_owned()
    } else if is_session_unknown(error) {
        "AgentCtrl does not know this run's agent session, so it cannot be resumed".to_owned()
    } else {
        format!("Could not resume the run: {}", failure_detail(error))
    }
}

/// Whether retrying cannot help: the dashboard rejected the request itself (auth, a missing run,
/// a bad request), as opposed to being briefly unavailable or rate limiting us.
fn is_rejection(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<HttpStatusError>()
        .is_some_and(|error| {
            error.status.is_client_error()
                && error.status != StatusCode::GONE
                && error.status != StatusCode::TOO_MANY_REQUESTS
        })
}

fn bookmarks_path() -> PathBuf {
    warp_core::paths::state_dir().join("agentctrl-managed-sessions.json")
}

fn load_bookmarks() -> Result<Vec<ManagedSessionBookmark>> {
    let path = bookmarks_path();
    match fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).context("invalid AgentCtrl bookmark file"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error).with_context(|| format!("failed to read {}", path.display())),
    }
}

fn save_bookmarks(bookmarks: &[ManagedSessionBookmark]) -> Result<()> {
    let path = bookmarks_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temp = path.with_extension("json.tmp");
    fs::write(&temp, serde_json::to_vec_pretty(bookmarks)?)?;
    if cfg!(windows) && path.exists() {
        fs::remove_file(&path)?;
    }
    fs::rename(&temp, &path)?;
    Ok(())
}

fn upsert_bookmark(
    bookmarks: &mut Vec<ManagedSessionBookmark>,
    mut replacement: ManagedSessionBookmark,
) {
    if let Some(existing) = bookmarks.iter_mut().find(|bookmark| {
        bookmark.environment_alias == replacement.environment_alias
            && bookmark.fleet_id == replacement.fleet_id
            && bookmark.job_id == replacement.job_id
    }) {
        replacement.created_at = existing.created_at.clone();
        *existing = replacement;
    } else {
        bookmarks.push(replacement);
    }
}

fn update_bookmark_cursor(
    bookmarks: &mut [ManagedSessionBookmark],
    profile: &EnvironmentProfile,
    job_id: &str,
    cursor: Option<String>,
    terminal: bool,
) {
    if let Some(bookmark) = bookmarks.iter_mut().find(|bookmark| {
        bookmark.environment_alias == profile.alias
            && bookmark.fleet_id == profile.fleet_id
            && bookmark.job_id == job_id
    }) {
        bookmark.last_cursor = cursor;
        if terminal {
            bookmark.last_known_state = Some("terminal".to_owned());
        }
        bookmark.last_opened_at = Utc::now().to_rfc3339();
    }
}

#[cfg(test)]
mod tests;
