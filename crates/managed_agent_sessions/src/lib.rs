use std::collections::HashSet;
use std::fs;
use std::io::{self, Write as _};
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use instant::Instant;
use reqwest::StatusCode;
use reqwest::blocking::{Client, Response};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use warp_cli::managed_sessions::{ManagedEnvironment, ManagedSessionsArgs, ManagedSessionsCommand};

const STAGING_BASE_URL: &str = "https://dash-backend-dev.stozn.ai";
const STAGING_FLEET_ID: &str = "stoz3n-agent-ctrl-staging";
const SUPPORTED_PROTOCOL: u32 = 1;
const BOOKMARK_FORMAT: u32 = 1;
const STOZN_AUTH_FORMAT: u32 = 1;
const STOZN_LOGIN_MAX_WAIT: Duration = Duration::from_secs(330);

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
            prompt,
            interactive,
            environment,
        } => start(
            *environment,
            agent,
            task,
            prompt.as_deref(),
            *interactive,
            args.json,
        ),
        ManagedSessionsCommand::Ui { environment } => ui(*environment),
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
    }
}

fn list(json_output: bool) -> Result<()> {
    let profile = profile(ManagedEnvironment::Staging);
    let token = dashboard_token()?;
    let page: Value = send(
        profile,
        Client::new()
            .get(format!(
                "{}/api/agent-control/runs?limit=100",
                profile.base_url
            ))
            .bearer_auth(&token)
            .header("X-Stozn-Client", "warpspeed"),
    )?
    .json()
    .context("invalid managed-run list response")?;
    let items = page
        .get("items")
        .and_then(Value::as_array)
        .context("managed-run list did not contain items")?;
    if json_output {
        println!("{}", serde_json::to_string_pretty(&page)?);
    } else if items.is_empty() {
        println!("No AgentCtrl managed runs in staging.");
    } else {
        for run in items {
            let job_id = run
                .get("job_id")
                .and_then(Value::as_str)
                .unwrap_or("invalid-job");
            let state = run
                .get("state")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let agent = run
                .get("agent_name")
                .and_then(Value::as_str)
                .unwrap_or("agent");
            println!("{job_id}  {state:<12} {agent:<24} {}", profile.alias);
        }
    }
    Ok(())
}

fn start(
    environment: ManagedEnvironment,
    agent: &str,
    task_id: &str,
    prompt: Option<&str>,
    interactive: bool,
    json_output: bool,
) -> Result<()> {
    let result = launch_request(environment, agent, task_id, prompt, interactive)?;
    if json_output {
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else if let Some(job_id) = result.get("job_id").and_then(Value::as_str) {
        println!("Started {job_id}. Attach with: warpspeed agent-ctrl attach {job_id}");
    } else {
        println!("{}", serde_json::to_string_pretty(&result)?);
    }
    Ok(())
}

fn launch_request(
    environment: ManagedEnvironment,
    agent: &str,
    task_id: &str,
    prompt: Option<&str>,
    interactive: bool,
) -> Result<Value> {
    if agent.is_empty() || agent.len() > 128 {
        bail!("agent name must contain 1 to 128 characters");
    }
    validate_task_id(task_id)?;
    if prompt.is_some_and(|value| value.is_empty() || value.chars().count() > 32_000) {
        bail!("prompt must contain 1 to 32000 characters");
    }
    let profile = profile(environment);
    let token = dashboard_token()?;
    let mut body = json!({
        "agent_name": agent,
        "task_id": task_id,
        "interactive": interactive
    });
    if let Some(prompt) = prompt {
        body["prompt"] = Value::String(prompt.to_owned());
    }
    let response = send(
        profile,
        Client::new()
            .post(format!("{}/api/agent-control/runs", profile.base_url))
            .bearer_auth(&token)
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

fn ui(environment: ManagedEnvironment) -> Result<()> {
    let profile = profile(environment);
    let token = dashboard_token()?;
    let client = Client::new();
    println!("STOZN managed session · {}", profile.alias);
    println!("Every session is attached to a STOZN task and uses normalized AgentCtrl events.");
    let task_search = read_line("Search open tasks (Enter for newest 100): ")?;
    let tasks_page: Value = send(
        profile,
        client
            .get(task_list_url(profile, &task_search)?)
            .bearer_auth(&token)
            .header("X-Stozn-Client", "warpspeed"),
    )?
    .json()
    .context("invalid STOZN task response")?;
    let overview: Value = send(
        profile,
        client
            .get(format!("{}/api/agent-control/overview", profile.base_url))
            .bearer_auth(&token)
            .header("X-Stozn-Client", "warpspeed"),
    )?
    .json()
    .context("invalid managed-agent overview")?;

    let tasks = tasks_page
        .get("items")
        .and_then(Value::as_array)
        .context("STOZN task response did not contain items")?
        .iter()
        .filter_map(|task| {
            let id = task.get("id")?.as_str()?;
            validate_task_id(id).ok()?;
            let reference = terminal_label(task.get("ref").and_then(Value::as_str).unwrap_or(id));
            let title = terminal_label(
                task.get("title")
                    .and_then(Value::as_str)
                    .unwrap_or("Untitled STOZN task"),
            );
            Some((id.to_owned(), format!("{reference} · {title}")))
        })
        .collect::<Vec<_>>();
    let agents = overview
        .get("agents")
        .and_then(Value::as_array)
        .context("managed-agent overview did not contain agents")?
        .iter()
        .filter(|agent| agent.get("launchable").and_then(Value::as_bool) == Some(true))
        .filter_map(|agent| {
            let name = agent.get("name")?.as_str()?;
            let backend = terminal_label(
                agent
                    .get("backend")
                    .and_then(Value::as_str)
                    .unwrap_or("agent"),
            );
            Some((
                name.to_owned(),
                format!("{} ({backend})", terminal_label(name)),
                agent
                    .get("allow_interactive")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            ))
        })
        .collect::<Vec<_>>();
    if tasks.is_empty() {
        if task_search.is_empty() {
            bail!(
                "no open STOZN tasks are available; create or reopen a task in the dashboard first"
            );
        }
        bail!("no open STOZN tasks matched that search");
    }
    if agents.is_empty() {
        bail!("no operator-enabled managed agents are available");
    }

    println!();
    let task_index = choose("Choose task", tasks.iter().map(|(_, label)| label.as_str()))?;
    let agent_index = choose(
        "Choose agent",
        agents.iter().map(|(_, label, _)| label.as_str()),
    )?;
    let prompt = read_line("Optional initial instruction (Enter for profile default): ")?;
    let interactive = if agents[agent_index].2 {
        let answer = read_line("Keep open for audited follow-up turns? [Y/n]: ")?;
        !matches!(answer.to_ascii_lowercase().as_str(), "n" | "no")
    } else {
        false
    };
    let result = launch_request(
        environment,
        &agents[agent_index].0,
        &tasks[task_index].0,
        (!prompt.is_empty()).then_some(prompt.as_str()),
        interactive,
    )?;
    let job_id = result
        .get("job_id")
        .and_then(Value::as_str)
        .context("accepted launch did not contain a job id")?
        .to_owned();
    println!("\nStarted {job_id} · {}", tasks[task_index].1);
    ui_session(environment, &job_id)
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
        println!("\nCommands: Enter=refresh, send <text>, close, stop, detach");
        let command = read_line("> ")?;
        if command.is_empty() || command == "refresh" {
            continue;
        }
        if command == "detach" || command == "quit" {
            println!("Detached. The managed run was not stopped.");
            return Ok(());
        }
        if command == "close" {
            input(environment, job_id, None, false)?;
            continue;
        }
        if command == "stop" {
            stop(environment, job_id, false, false)?;
            continue;
        }
        if let Some(text) = command.strip_prefix("send ") {
            input(environment, job_id, Some(text.trim()), false)?;
            continue;
        }
        println!("Unknown command. Use send <text>, close, stop, refresh, or detach.");
    }
}

fn session_is_finished(state: &str) -> bool {
    matches!(state, "completed" | "failed" | "stopped" | "interrupted")
}

fn task_list_url(profile: &EnvironmentProfile, search: &str) -> Result<reqwest::Url> {
    let mut url = reqwest::Url::parse(&format!("{}/api/work-items/", profile.base_url))
        .context("invalid compiled STOZN task endpoint")?;
    url.query_pairs_mut()
        .append_pair("open_only", "true")
        .append_pair("limit", "100");
    if !search.is_empty() {
        url.query_pairs_mut().append_pair("q", search);
    }
    Ok(url)
}

fn choose<'a>(label: &str, choices: impl Iterator<Item = &'a str>) -> Result<usize> {
    let choices = choices.collect::<Vec<_>>();
    println!("{label}:");
    for (index, choice) in choices.iter().enumerate() {
        println!("  {}) {choice}", index + 1);
    }
    loop {
        let answer = read_line("Selection: ")?;
        if let Ok(selected) = answer.parse::<usize>()
            && (1..=choices.len()).contains(&selected)
        {
            return Ok(selected - 1);
        }
        println!("Enter a number from 1 to {}.", choices.len());
    }
}

fn read_line(prompt: &str) -> Result<String> {
    print!("{prompt}");
    io::stdout().flush().context("failed to flush prompt")?;
    let mut line = String::new();
    if io::stdin().read_line(&mut line)? == 0 {
        bail!("managed-session UI closed before a selection was made");
    }
    Ok(line.trim().to_owned())
}

fn terminal_label(value: &str) -> String {
    value
        .chars()
        .filter(|character| !character.is_control())
        .take(160)
        .collect()
}

fn input(
    environment: ManagedEnvironment,
    job_id: &str,
    text: Option<&str>,
    json_output: bool,
) -> Result<()> {
    validate_job_id(job_id)?;
    if text.is_some_and(|value| value.is_empty() || value.chars().count() > 8_000) {
        bail!("input must contain 1 to 8000 characters");
    }
    let profile = profile(environment);
    let token = dashboard_token()?;
    let action = if text.is_some() { "input" } else { "close" };
    let body = text.map_or_else(|| json!({}), |text| json!({ "text": text }));
    let response = send(
        profile,
        Client::new()
            .post(format!(
                "{}/api/agent-control/runs/{job_id}/{action}",
                profile.base_url
            ))
            .bearer_auth(&token)
            .header("X-Stozn-Client", "warpspeed")
            .header(
                "Idempotency-Key",
                format!("warpspeed-{}", uuid::Uuid::new_v4()),
            )
            .json(&body),
    )?;
    let result: Value = response.json().context("invalid input response")?;
    ensure_command_accepted(&result)?;
    if json_output {
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else {
        println!("Managed-session {action} accepted for {job_id}.");
    }
    Ok(())
}

fn stop(
    environment: ManagedEnvironment,
    job_id: &str,
    force: bool,
    json_output: bool,
) -> Result<()> {
    validate_job_id(job_id)?;
    let profile = profile(environment);
    let token = dashboard_token()?;
    let response = send(
        profile,
        Client::new()
            .post(format!(
                "{}/api/agent-control/runs/{job_id}/stop",
                profile.base_url
            ))
            .bearer_auth(&token)
            .header("X-Stozn-Client", "warpspeed")
            .header(
                "Idempotency-Key",
                format!("warpspeed-{}", uuid::Uuid::new_v4()),
            )
            .json(&json!({ "force": force })),
    )?;
    let result: Value = response.json().context("invalid stop response")?;
    ensure_command_accepted(&result)?;
    if json_output {
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else {
        println!("Stop request accepted for {job_id}.");
    }
    Ok(())
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
        let capabilities = descriptor.capabilities.join(",");
        println!(
            "Attached to {} ({}, capabilities: {}). Closing this view does not stop the run.",
            descriptor.display_name, descriptor.state, capabilities
        );
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
    fleet_id: &'static str,
}

fn profile(environment: ManagedEnvironment) -> &'static EnvironmentProfile {
    static STAGING: EnvironmentProfile = EnvironmentProfile {
        alias: "staging",
        base_url: STAGING_BASE_URL,
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
    let allowed = reqwest::Url::parse(profile.base_url)?;
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
    if result.get("status").and_then(Value::as_str) == Some("accepted") {
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
    let allowed = reqwest::Url::parse(profile.base_url)
        .context("invalid compiled managed-session dashboard URL")?;
    let requested = request.url();
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

fn http_error(response: Response) -> anyhow::Error {
    let status = response.status();
    let body = response.text().unwrap_or_default();
    anyhow::anyhow!("dashboard returned {status}: {body}")
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
