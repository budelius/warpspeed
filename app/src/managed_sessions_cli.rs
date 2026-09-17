use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use chrono::Utc;
use reqwest::StatusCode;
use reqwest::blocking::{Client, Response};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use warp_cli::managed_sessions::{ManagedEnvironment, ManagedSessionsArgs, ManagedSessionsCommand};

const STAGING_BASE_URL: &str = "https://dash-backend-dev.stozn.ai";
const STAGING_FLEET_ID: &str = "stoz3n-agent-ctrl-staging";
const SUPPORTED_PROTOCOL: u32 = 1;
const BOOKMARK_FORMAT: u32 = 1;

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

pub(crate) fn run(args: &ManagedSessionsArgs) -> Result<()> {
    match &args.command {
        ManagedSessionsCommand::List => list(args.json),
        ManagedSessionsCommand::Start {
            agent,
            prompt,
            interactive,
            environment,
        } => start(
            *environment,
            agent,
            prompt.as_deref(),
            *interactive,
            args.json,
        ),
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
    prompt: Option<&str>,
    interactive: bool,
    json_output: bool,
) -> Result<()> {
    if agent.is_empty() || agent.len() > 128 {
        bail!("agent name must contain 1 to 128 characters");
    }
    if prompt.is_some_and(|value| value.is_empty() || value.chars().count() > 32_000) {
        bail!("prompt must contain 1 to 32000 characters");
    }
    let profile = profile(environment);
    let token = dashboard_token()?;
    let mut body = json!({ "agent_name": agent, "interactive": interactive });
    if let Some(prompt) = prompt {
        body["prompt"] = Value::String(prompt.to_owned());
    }
    let response = send(
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
    if json_output {
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else if let Some(job_id) = result.get("job_id").and_then(Value::as_str) {
        println!("Started {job_id}. Attach with: warpspeed agent-ctrl attach {job_id}");
    } else {
        println!("{}", serde_json::to_string_pretty(&result)?);
    }
    Ok(())
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
    let path = dirs::home_dir()
        .context("home directory is unavailable")?
        .join(".stozn.json");
    let value: Value = serde_json::from_slice(&fs::read(&path).with_context(|| {
        format!(
            "no dashboard token; set STOZN_TOKEN or authenticate with stozn ({})",
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

fn send(request: reqwest::blocking::RequestBuilder) -> Result<Response> {
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
#[path = "managed_sessions_cli_tests.rs"]
mod tests;
