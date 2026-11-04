use std::io::{IsTerminal as _, Write as _};
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;
use std::{fs, io, thread};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use command::blocking::Command;
use crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent,
    MouseEventKind,
};
use futures::{SinkExt as _, StreamExt as _};
use instant::Instant;
use reqwest::StatusCode;
use reqwest::blocking::Client;
use reqwest::redirect::Policy;
use serde::{Deserialize, Serialize};
use serde_json::json;
use warp_cli::managed_sessions::ManagedEnvironment;
use websocket::{Message, WebSocket, WebsocketMessage as _};

use super::{
    EnvironmentProfile, SessionDescriptor, dashboard_token, fetch_descriptor, profile, send,
};

const SSH_PORT: u16 = 2222;
const SSH_USER: &str = "agentctrl";
const ATTACH_WAIT: Duration = Duration::from_secs(30);
const ATTACH_POLL: Duration = Duration::from_millis(500);
const CHILD_POLL: Duration = Duration::from_millis(200);
const TERMINAL_GATEWAY_BASE_URL: &str = "https://dash-backend-dev.stozn.ai";
const TERMINAL_GATEWAY_ORIGIN: &str = "https://dash-dev.stozn.ai";
const TERMINAL_GATEWAY_PROTOCOL: &str = "stozn-agentctrl-terminal-v1";
const TERMINAL_GATEWAY_SOCKET_URL: &str =
    "wss://dash-backend-dev.stozn.ai/api/agent-control/terminal/socket";
const TERMINAL_GATEWAY_KEEPALIVE: Duration = Duration::from_secs(30);
const TERMINAL_GATEWAY_RECONNECT_DELAY: Duration = Duration::from_secs(1);
const TERMINAL_GATEWAY_WRITER_RETRIES: usize = 5;
const INPUT_POLL: Duration = Duration::from_millis(10);
const MAX_TERMINAL_DIMENSION: u16 = 500;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum NativeAttachOutcome {
    Attached,
    Fallback(String),
}

#[derive(Debug, Deserialize)]
struct AuthorizationResponse {
    authorization_id: String,
    job_id: String,
    execution_generation: String,
    client_id: String,
    expires_at_ms: u64,
    lease_renew_after_ms: u64,
    takeover: bool,
    endpoint: SshEndpoint,
}

#[derive(Debug, Deserialize)]
struct RenewalResponse {
    authorization_id: String,
    expires_at_ms: u64,
}

#[derive(Debug, Deserialize)]
struct RevokeResponse {
    authorization_id: String,
    revoked: bool,
}

#[derive(Debug, Deserialize)]
struct TerminalGatewayTicket {
    protocol: String,
    ticket: String,
    expires_at: String,
    websocket_url: String,
}

#[derive(Debug, Deserialize)]
struct TerminalGatewayControl {
    #[serde(rename = "type")]
    kind: String,
    code: Option<serde_json::Value>,
    message: Option<String>,
    job_id: Option<String>,
    execution_generation: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct SshEndpoint {
    host: String,
    port: u16,
    user: String,
    host_public_key: String,
}

#[derive(Serialize)]
struct AuthorizationRequest<'a> {
    execution_generation: &'a str,
    client_id: &'a str,
    public_key: &'a str,
    takeover: bool,
}

#[derive(Serialize)]
struct LeaseRequest<'a> {
    execution_generation: &'a str,
    client_id: &'a str,
}

#[derive(Serialize)]
struct RevokeRequest<'a> {
    execution_generation: &'a str,
    client_id: &'a str,
    reason: &'a str,
}

pub(super) fn attach_if_available(
    environment: ManagedEnvironment,
    job_id: &str,
    takeover: bool,
) -> Result<NativeAttachOutcome> {
    super::validate_job_id(job_id)?;
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return Ok(NativeAttachOutcome::Fallback(
            "Native terminal attachment requires an interactive local terminal".to_string(),
        ));
    }
    let profile = profile(environment);
    let deadline = Instant::now() + ATTACH_WAIT;
    let Some(mut descriptor) = wait_for_ready_descriptor(profile, job_id, deadline)? else {
        return Ok(NativeAttachOutcome::Fallback(
            "Native terminal descriptor is temporarily unavailable".to_string(),
        ));
    };
    for attempt in 0..2 {
        if let Some(reason) = descriptor_unavailable_reason(&descriptor) {
            return Ok(NativeAttachOutcome::Fallback(reason));
        }
        match attach_descriptor(profile, &descriptor, takeover) {
            Ok(()) => return Ok(NativeAttachOutcome::Attached),
            Err(error) if attempt == 0 && is_stale_generation(&error) => {
                let Some(refreshed) = wait_for_ready_descriptor(profile, job_id, deadline)? else {
                    return Ok(NativeAttachOutcome::Fallback(
                        "Native terminal descriptor is temporarily unavailable".to_string(),
                    ));
                };
                descriptor = refreshed;
            }
            Err(error) => {
                if let Some(reason) = unavailable_reason(&error) {
                    return Ok(NativeAttachOutcome::Fallback(reason));
                }
                return Err(error);
            }
        }
    }
    unreachable!("native attachment retry is bounded")
}

fn wait_for_ready_descriptor(
    profile: &'static EnvironmentProfile,
    job_id: &str,
    deadline: Instant,
) -> Result<Option<SessionDescriptor>> {
    loop {
        let now = Instant::now();
        if now >= deadline {
            return Ok(None);
        }
        let token = dashboard_token()?;
        let remaining = deadline.saturating_duration_since(Instant::now());
        let Some(timeout) = descriptor_request_timeout(remaining) else {
            return Ok(None);
        };
        let client = Client::builder().timeout(timeout).build()?;
        let descriptor = match fetch_descriptor(&client, profile, job_id, &token) {
            Ok(descriptor) => descriptor,
            Err(error) if request_timed_out(&error) && Instant::now() < deadline => continue,
            Err(error) if request_timed_out(&error) || unavailable_reason(&error).is_some() => {
                return Ok(None);
            }
            Err(error) => return Err(error),
        };
        match descriptor
            .terminal
            .as_ref()
            .map(|terminal| terminal.availability.as_str())
        {
            Some("ready" | "attached") => return Ok(Some(descriptor)),
            Some("starting") if Instant::now() < deadline => {
                thread::sleep(ATTACH_POLL.min(deadline.saturating_duration_since(Instant::now())));
            }
            Some("starting") => return Ok(None),
            Some("paused") => {
                return Ok(Some(fallback_descriptor(
                    descriptor,
                    "Run is paused; resume it to create a new native terminal",
                )));
            }
            Some("completed" | "stopped" | "failed" | "lost" | "unsupported") | None => {
                return Ok(Some(descriptor));
            }
            Some(_) => return Ok(Some(descriptor)),
        }
    }
}

fn descriptor_request_timeout(remaining: Duration) -> Option<Duration> {
    (!remaining.is_zero()).then(|| remaining.min(Duration::from_secs(5)))
}

fn request_timed_out(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<reqwest::Error>()
            .is_some_and(reqwest::Error::is_timeout)
    })
}

fn fallback_descriptor(mut descriptor: SessionDescriptor, reason: &str) -> SessionDescriptor {
    if let Some(terminal) = descriptor.terminal.as_mut() {
        terminal.availability = "unsupported".to_string();
        terminal.reason = Some(reason.to_string());
    }
    descriptor
}

fn descriptor_unavailable_reason(descriptor: &SessionDescriptor) -> Option<String> {
    match descriptor
        .terminal
        .as_ref()
        .map(|terminal| terminal.availability.as_str())
    {
        Some("ready" | "attached") => None,
        Some(other) => Some(
            descriptor
                .terminal
                .as_ref()
                .and_then(|terminal| terminal.reason.clone())
                .unwrap_or_else(|| format!("Native terminal is unavailable ({other})")),
        ),
        None => Some("Native terminal is unavailable".to_string()),
    }
}

fn is_stale_generation(error: &anyhow::Error) -> bool {
    super::has_error_code(error, StatusCode::CONFLICT, "stale_generation")
}

fn unavailable_reason(error: &anyhow::Error) -> Option<String> {
    let unavailable = [
        (StatusCode::CONFLICT, "terminal_unavailable"),
        (StatusCode::CONFLICT, "terminal_unsupported"),
        (StatusCode::SERVICE_UNAVAILABLE, "control_unavailable"),
        (StatusCode::SERVICE_UNAVAILABLE, "controller_unavailable"),
    ]
    .into_iter()
    .any(|(status, code)| super::has_error_code(error, status, code));
    unavailable.then(|| "Native terminal authorization is temporarily unavailable".to_string())
}

fn attach_descriptor(
    profile: &'static EnvironmentProfile,
    descriptor: &SessionDescriptor,
    takeover: bool,
) -> Result<()> {
    let generation = descriptor
        .execution_generation
        .as_deref()
        .filter(|generation| {
            descriptor
                .terminal
                .as_ref()
                .and_then(|terminal| terminal.execution_generation.as_deref())
                == Some(*generation)
        })
        .context("native terminal descriptor has no current execution generation")?;
    validate_opaque_id("execution generation", generation)?;
    match attach_through_gateway(&descriptor.job_id, generation, takeover) {
        Ok(()) => return Ok(()),
        Err(error) if gateway_is_unavailable(&error) => {
            eprintln!("Live terminal gateway is unavailable; trying direct private SSH.");
        }
        Err(error) => return Err(error),
    }

    let files = EphemeralSshFiles::create()?;
    let client_id = uuid::Uuid::new_v4().to_string();
    let authorization = create_authorization(
        profile,
        &descriptor.job_id,
        generation,
        &client_id,
        &files.public_key,
        takeover,
    )?;
    validate_authorization(
        &authorization,
        &descriptor.job_id,
        generation,
        &client_id,
        takeover,
    )?;
    if let Err(error) = files.write_known_host(&authorization.endpoint) {
        let _ = revoke_authorization(
            profile,
            &descriptor.job_id,
            generation,
            &client_id,
            &authorization.authorization_id,
            "client_exit",
        );
        return Err(error);
    }

    eprintln!(
        "Attaching to the native Pi terminal. Its unredacted screen travels directly over private SSH."
    );
    eprintln!("Press Ctrl-] to detach without stopping the managed run.");

    let renewal_failed = Arc::new(AtomicBool::new(false));
    let (stop_tx, stop_rx) = mpsc::sync_channel(1);
    let renew = spawn_renewal(
        RenewalTask {
            profile,
            job_id: descriptor.job_id.clone(),
            generation: generation.to_string(),
            client_id: client_id.clone(),
            authorization_id: authorization.authorization_id.clone(),
            interval: renewal_interval(&authorization),
        },
        stop_rx,
        renewal_failed.clone(),
    );
    let status = run_ssh(&files, &authorization.endpoint, &renewal_failed);
    let _ = stop_tx.try_send(());
    let _ = renew.join();
    let reason = if renewal_failed.load(Ordering::SeqCst) {
        "access_revoked"
    } else {
        "detached"
    };
    let _ = revoke_authorization(
        profile,
        &descriptor.job_id,
        generation,
        &client_id,
        &authorization.authorization_id,
        reason,
    );
    let status = status?;
    if renewal_failed.load(Ordering::SeqCst) {
        bail!("Native terminal access expired or could not be renewed");
    }
    if status.success() {
        println!("Detached. The managed run was not stopped.");
        Ok(())
    } else {
        bail!(
            "Native terminal connection exited with status {}",
            status.code().unwrap_or(-1)
        )
    }
}

fn attach_through_gateway(job_id: &str, generation: &str, takeover: bool) -> Result<()> {
    eprintln!(
        "Attaching to the native Pi terminal through STOZN's authenticated terminal gateway."
    );
    eprintln!("Press Ctrl-] to detach without stopping the managed run.");

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("failed to start the Live terminal runtime")?;
    let mut reconnecting = false;
    let mut writer_retries = 0;
    loop {
        if reconnecting {
            thread::sleep(TERMINAL_GATEWAY_RECONNECT_DELAY);
        }
        let ticket = match create_gateway_ticket(job_id, takeover) {
            Ok(ticket) => ticket,
            Err(error) if gateway_ticket_error_is_retryable(&error) => {
                if !reconnecting {
                    eprintln!("\r\nLive terminal ticket request was interrupted; reconnecting.");
                }
                reconnecting = true;
                continue;
            }
            Err(error) => return Err(error),
        };
        validate_gateway_ticket(&ticket, Utc::now())?;
        match runtime.block_on(run_gateway_terminal(ticket, job_id, generation))? {
            GatewayTerminalOutcome::Detached => {
                println!("Detached. The managed run was not stopped.");
                return Ok(());
            }
            GatewayTerminalOutcome::Reconnect => {
                reconnecting = true;
                writer_retries = 0;
                eprintln!("\r\nLive terminal connection was interrupted; reconnecting.");
            }
            GatewayTerminalOutcome::WriterExists
                if reconnecting && writer_retries < TERMINAL_GATEWAY_WRITER_RETRIES =>
            {
                writer_retries += 1;
            }
            GatewayTerminalOutcome::WriterExists => {
                bail!(
                    "Live terminal is open elsewhere; disconnect it or retry with `agent-ctrl attach {job_id} --takeover`"
                );
            }
        }
    }
}

fn create_gateway_ticket(job_id: &str, takeover: bool) -> Result<TerminalGatewayTicket> {
    let token = dashboard_token()?;
    let client = Client::builder()
        .timeout(Duration::from_secs(20))
        .redirect(Policy::none())
        .build()
        .context("failed to create the Live terminal gateway client")?;
    let response = client
        .post(format!(
            "{TERMINAL_GATEWAY_BASE_URL}/api/agent-control/runs/{job_id}/terminal/tickets"
        ))
        .bearer_auth(token)
        .header("X-Stozn-Client", "warpspeed")
        .json(&json!({ "takeover": takeover }))
        .send()
        .context("Live terminal ticket request failed")?;
    if !response.status().is_success() {
        return Err(super::http_error(response));
    }
    response
        .json()
        .context("invalid Live terminal ticket response")
}

fn validate_gateway_ticket(ticket: &TerminalGatewayTicket, now: DateTime<Utc>) -> Result<()> {
    if ticket.protocol != TERMINAL_GATEWAY_PROTOCOL
        || ticket.websocket_url != TERMINAL_GATEWAY_SOCKET_URL
        || ticket.ticket.is_empty()
        || ticket.ticket.len() > 4_096
        || ticket.ticket.split('.').count() != 2
        || !ticket
            .ticket
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        bail!("Live terminal ticket response is invalid");
    }
    let expires_at = DateTime::parse_from_rfc3339(&ticket.expires_at)
        .context("Live terminal ticket expiry is invalid")?
        .with_timezone(&Utc);
    if expires_at <= now || expires_at > now + chrono::Duration::seconds(120) {
        bail!("Live terminal ticket expiry is invalid");
    }
    Ok(())
}

fn gateway_ticket_error_is_retryable(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<super::HttpStatusError>()
            .is_some_and(|error| {
                error.status == StatusCode::TOO_MANY_REQUESTS || error.status.is_server_error()
            })
            || cause
                .downcast_ref::<reqwest::Error>()
                .is_some_and(|error| error.is_connect() || error.is_timeout() || error.is_request())
    })
}

fn gateway_is_unavailable(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<super::HttpStatusError>()
        .is_some_and(|error| {
            matches!(
                error.status,
                StatusCode::NOT_FOUND | StatusCode::METHOD_NOT_ALLOWED
            )
        })
        || super::has_error_code(error, StatusCode::CONFLICT, "terminal_unsupported")
}

async fn run_gateway_terminal(
    ticket: TerminalGatewayTicket,
    job_id: &str,
    generation: &str,
) -> Result<GatewayTerminalOutcome> {
    let socket = match WebSocket::connect_with_headers(
        &ticket.websocket_url,
        [ticket.protocol.as_str(), ticket.ticket.as_str()],
        vec![("Origin", TERMINAL_GATEWAY_ORIGIN.to_string())],
    )
    .await
    {
        Ok(socket) => socket,
        Err(_) => return Ok(GatewayTerminalOutcome::Reconnect),
    };
    let (mut sink, mut stream) = socket.split().await;
    let (cols, rows) = terminal_size()?;
    if sink
        .send(Message::new_text(
            json!({ "type": "hello", "cols": cols, "rows": rows }).to_string(),
        ))
        .await
        .is_err()
    {
        return Ok(GatewayTerminalOutcome::Reconnect);
    }

    let mut output_filter = TerminalOutputFilter::default();
    loop {
        let Some(Ok(message)) = stream.next().await else {
            return Ok(GatewayTerminalOutcome::Reconnect);
        };
        let message = gateway_message(message, job_id, generation, false, &mut output_filter)?;
        if let Some(outcome) = gateway_terminal_outcome(&message) {
            return Ok(outcome);
        }
        if message == GatewayMessage::Ready {
            break;
        }
    }

    let _raw_mode = RawModeGuard::enter()?;
    let mut poll = tokio::time::interval(INPUT_POLL);
    poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut keepalive = tokio::time::interval_at(
        tokio::time::Instant::now() + TERMINAL_GATEWAY_KEEPALIVE,
        TERMINAL_GATEWAY_KEEPALIVE,
    );
    keepalive.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            message = stream.next() => {
                let Some(message) = message else {
                    return Ok(GatewayTerminalOutcome::Reconnect);
                };
                let Ok(message) = message else {
                    return Ok(GatewayTerminalOutcome::Reconnect);
                };
                match gateway_message(message, job_id, generation, true, &mut output_filter)? {
                    GatewayMessage::Ready | GatewayMessage::Continue => {}
                    GatewayMessage::Reconnect => return Ok(GatewayTerminalOutcome::Reconnect),
                    GatewayMessage::WriterExists => {
                        return Ok(GatewayTerminalOutcome::WriterExists);
                    }
                }
            }
            _ = keepalive.tick() => {
                if sink.send(gateway_keepalive()).await.is_err() {
                    return Ok(GatewayTerminalOutcome::Reconnect);
                }
            }
            _ = poll.tick() => {
                while event::poll(Duration::ZERO).context("failed to read Live terminal input")? {
                    match local_terminal_input(event::read().context("failed to read Live terminal input")?) {
                        LocalTerminalInput::Bytes(bytes) => {
                            for chunk in bytes.chunks(64 * 1024) {
                                if sink.send(Message::new_binary(chunk.to_vec())).await.is_err() {
                                    return Ok(GatewayTerminalOutcome::Reconnect);
                                }
                            }
                        }
                        LocalTerminalInput::Resize(cols, rows) => {
                            if sink.send(Message::new_text(
                                json!({ "type": "resize", "cols": cols, "rows": rows }).to_string(),
                            )).await.is_err() {
                                return Ok(GatewayTerminalOutcome::Reconnect);
                            }
                        }
                        LocalTerminalInput::Detach => {
                            let _ = sink.close().await;
                            return Ok(GatewayTerminalOutcome::Detached);
                        }
                        LocalTerminalInput::Ignore => {}
                    }
                }
            }
        }
    }
}

fn gateway_keepalive() -> Message {
    Message::new_text(json!({ "type": "ping" }).to_string())
}

#[derive(Debug, PartialEq, Eq)]
enum GatewayTerminalOutcome {
    Detached,
    Reconnect,
    WriterExists,
}

fn gateway_terminal_outcome(message: &GatewayMessage) -> Option<GatewayTerminalOutcome> {
    match message {
        GatewayMessage::Ready | GatewayMessage::Continue => None,
        GatewayMessage::Reconnect => Some(GatewayTerminalOutcome::Reconnect),
        GatewayMessage::WriterExists => Some(GatewayTerminalOutcome::WriterExists),
    }
}

#[derive(Default)]
struct TerminalOutputFilter {
    pending: Vec<u8>,
}

impl TerminalOutputFilter {
    fn push(&mut self, bytes: &[u8]) -> Vec<u8> {
        const QUERIES: [&[u8]; 4] = [b"\x1b[>c", b"\x1b[>0c", b"\x1b[>q", b"\x1b[>0q"];

        let mut output = Vec::with_capacity(bytes.len());
        for byte in bytes {
            if self.pending.is_empty() && *byte != b'\x1b' {
                output.push(*byte);
                continue;
            }
            self.pending.push(*byte);
            loop {
                if QUERIES.iter().any(|query| *query == self.pending) {
                    self.pending.clear();
                    break;
                }
                if QUERIES.iter().any(|query| query.starts_with(&self.pending)) {
                    break;
                }
                output.push(self.pending.remove(0));
                if self.pending.is_empty() {
                    break;
                }
            }
        }
        output
    }
}

#[derive(Debug, PartialEq, Eq)]
enum GatewayMessage {
    Ready,
    Continue,
    Reconnect,
    WriterExists,
}

fn gateway_message(
    message: Message,
    job_id: &str,
    generation: &str,
    ready: bool,
    output_filter: &mut TerminalOutputFilter,
) -> Result<GatewayMessage> {
    if let Some(bytes) = message.binary() {
        if !ready {
            bail!("Live terminal sent output before it was ready");
        }
        let bytes = output_filter.push(bytes);
        let mut stdout = io::stdout().lock();
        stdout
            .write_all(&bytes)
            .and_then(|()| stdout.flush())
            .context("failed to display Live terminal output")?;
        return Ok(GatewayMessage::Continue);
    }
    let Some(text) = message.text() else {
        return Ok(GatewayMessage::Continue);
    };
    if text.len() > 4_096 {
        bail!("Live terminal returned an oversized control message");
    }
    let control: TerminalGatewayControl =
        serde_json::from_str(text).context("Live terminal returned an invalid control message")?;
    match control.kind.as_str() {
        "ready" => {
            if ready
                || control.job_id.as_deref() != Some(job_id)
                || control.execution_generation.as_deref() != Some(generation)
            {
                bail!("Live terminal ready message did not match the requested run");
            }
            Ok(GatewayMessage::Ready)
        }
        "pong" => Ok(GatewayMessage::Continue),
        "exit" => Ok(GatewayMessage::Reconnect),
        "error" => {
            let code = control.code.as_ref().and_then(serde_json::Value::as_str);
            if matches!(code, Some("idle_timeout" | "duration_limit")) {
                return Ok(GatewayMessage::Reconnect);
            }
            if code == Some("writer_exists") {
                return Ok(GatewayMessage::WriterExists);
            }
            let message = control
                .message
                .filter(|message| {
                    !message.is_empty()
                        && message.len() <= 500
                        && !message.chars().any(char::is_control)
                })
                .unwrap_or_else(|| "Live terminal connection failed".to_string());
            bail!("{message}")
        }
        _ => bail!("Live terminal returned an unsupported control message"),
    }
}

struct RawModeGuard;

impl RawModeGuard {
    fn enter() -> Result<Self> {
        crossterm::terminal::enable_raw_mode().context("failed to enter Live terminal raw mode")?;
        Ok(Self)
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        let _ = crossterm::terminal::disable_raw_mode();
    }
}

#[derive(Debug, PartialEq, Eq)]
enum LocalTerminalInput {
    Bytes(Vec<u8>),
    Resize(u16, u16),
    Detach,
    Ignore,
}

fn terminal_size() -> Result<(u16, u16)> {
    let (cols, rows) = crossterm::terminal::size().context("failed to read terminal size")?;
    Ok(clamp_terminal_size(cols, rows))
}

fn clamp_terminal_size(cols: u16, rows: u16) -> (u16, u16) {
    (
        cols.clamp(2, MAX_TERMINAL_DIMENSION),
        rows.clamp(2, MAX_TERMINAL_DIMENSION),
    )
}

fn local_terminal_input(event: Event) -> LocalTerminalInput {
    match event {
        Event::Key(key)
            if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat)
                && key.code == KeyCode::Char(']')
                && key.modifiers.contains(KeyModifiers::CONTROL) =>
        {
            LocalTerminalInput::Detach
        }
        Event::Key(key) if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) => {
            encode_key(key).map_or(LocalTerminalInput::Ignore, LocalTerminalInput::Bytes)
        }
        Event::Key(_) => LocalTerminalInput::Ignore,
        Event::Mouse(mouse) => LocalTerminalInput::Bytes(encode_mouse(mouse)),
        Event::Paste(text) => LocalTerminalInput::Bytes(
            [
                b"\x1b[200~".as_slice(),
                text.as_bytes(),
                b"\x1b[201~".as_slice(),
            ]
            .concat(),
        ),
        Event::Resize(cols, rows) => {
            let (cols, rows) = clamp_terminal_size(cols, rows);
            LocalTerminalInput::Resize(cols, rows)
        }
        Event::FocusGained => LocalTerminalInput::Bytes(b"\x1b[I".to_vec()),
        Event::FocusLost => LocalTerminalInput::Bytes(b"\x1b[O".to_vec()),
    }
}

fn encode_key(key: KeyEvent) -> Option<Vec<u8>> {
    let modifier = xterm_modifier(key.modifiers);
    let sequence = match key.code {
        KeyCode::Char(character) => {
            let mut bytes = Vec::new();
            if key.modifiers.contains(KeyModifiers::ALT) {
                bytes.push(0x1b);
            }
            if key.modifiers.contains(KeyModifiers::CONTROL)
                && let Some(control) = control_character(character)
            {
                bytes.push(control);
                return Some(bytes);
            }
            let mut encoded = [0; 4];
            bytes.extend_from_slice(character.encode_utf8(&mut encoded).as_bytes());
            return Some(bytes);
        }
        KeyCode::Backspace => "\x7f".to_string(),
        KeyCode::Enter => "\r".to_string(),
        KeyCode::Left => modified_csi("D", modifier),
        KeyCode::Right => modified_csi("C", modifier),
        KeyCode::Up => modified_csi("A", modifier),
        KeyCode::Down => modified_csi("B", modifier),
        KeyCode::Home => modified_csi("H", modifier),
        KeyCode::End => modified_csi("F", modifier),
        KeyCode::PageUp => modified_tilde_csi(5, modifier),
        KeyCode::PageDown => modified_tilde_csi(6, modifier),
        KeyCode::Tab => "\t".to_string(),
        KeyCode::BackTab => "\x1b[Z".to_string(),
        KeyCode::Delete => modified_tilde_csi(3, modifier),
        KeyCode::Insert => modified_tilde_csi(2, modifier),
        KeyCode::F(number) => function_key(number, modifier)?,
        KeyCode::Null => "\0".to_string(),
        KeyCode::Esc => "\x1b".to_string(),
        KeyCode::CapsLock
        | KeyCode::ScrollLock
        | KeyCode::NumLock
        | KeyCode::PrintScreen
        | KeyCode::Pause
        | KeyCode::Menu
        | KeyCode::KeypadBegin
        | KeyCode::Media(_)
        | KeyCode::Modifier(_) => return None,
    };
    let mut bytes = Vec::new();
    if key.modifiers.contains(KeyModifiers::ALT)
        && matches!(
            key.code,
            KeyCode::Backspace | KeyCode::Enter | KeyCode::Tab | KeyCode::BackTab | KeyCode::Null
        )
    {
        bytes.push(0x1b);
    }
    bytes.extend_from_slice(sequence.as_bytes());
    Some(bytes)
}

fn control_character(character: char) -> Option<u8> {
    match character {
        '@' | ' ' | '2' => Some(0x00),
        'a'..='z' => Some(character as u8 - b'a' + 1),
        'A'..='Z' => Some(character as u8 - b'A' + 1),
        '[' | '3' => Some(0x1b),
        '\\' | '4' => Some(0x1c),
        ']' | '5' => Some(0x1d),
        '^' | '6' => Some(0x1e),
        '_' | '7' | '/' => Some(0x1f),
        '?' | '8' => Some(0x7f),
        _ => None,
    }
}

fn xterm_modifier(modifiers: KeyModifiers) -> u8 {
    1 + u8::from(modifiers.contains(KeyModifiers::SHIFT))
        + 2 * u8::from(modifiers.contains(KeyModifiers::ALT))
        + 4 * u8::from(modifiers.contains(KeyModifiers::CONTROL))
}

fn modified_csi(suffix: &str, modifier: u8) -> String {
    if modifier == 1 {
        format!("\x1b[{suffix}")
    } else {
        format!("\x1b[1;{modifier}{suffix}")
    }
}

fn modified_tilde_csi(number: u8, modifier: u8) -> String {
    if modifier == 1 {
        format!("\x1b[{number}~")
    } else {
        format!("\x1b[{number};{modifier}~")
    }
}

fn function_key(number: u8, modifier: u8) -> Option<String> {
    let suffix = match number {
        1 => "P",
        2 => "Q",
        3 => "R",
        4 => "S",
        _ => "",
    };
    if !suffix.is_empty() {
        return Some(if modifier == 1 {
            format!("\x1bO{suffix}")
        } else {
            format!("\x1b[1;{modifier}{suffix}")
        });
    }
    let code = match number {
        5 => 15,
        6 => 17,
        7 => 18,
        8 => 19,
        9 => 20,
        10 => 21,
        11 => 23,
        12 => 24,
        13 => 25,
        14 => 26,
        15 => 28,
        16 => 29,
        17 => 31,
        18 => 32,
        19 => 33,
        20 => 34,
        _ => return None,
    };
    Some(modified_tilde_csi(code, modifier))
}

fn encode_mouse(mouse: MouseEvent) -> Vec<u8> {
    let (mut code, released) = match mouse.kind {
        MouseEventKind::Down(button) => (mouse_button_code(button), false),
        MouseEventKind::Up(button) => (mouse_button_code(button), true),
        MouseEventKind::Drag(button) => (32 + mouse_button_code(button), false),
        MouseEventKind::Moved => (35, false),
        MouseEventKind::ScrollUp => (64, false),
        MouseEventKind::ScrollDown => (65, false),
        MouseEventKind::ScrollLeft => (66, false),
        MouseEventKind::ScrollRight => (67, false),
    };
    if mouse.modifiers.contains(KeyModifiers::SHIFT) {
        code += 4;
    }
    if mouse.modifiers.contains(KeyModifiers::ALT) {
        code += 8;
    }
    if mouse.modifiers.contains(KeyModifiers::CONTROL) {
        code += 16;
    }
    format!(
        "\x1b[<{};{};{}{}",
        code,
        mouse.column.saturating_add(1),
        mouse.row.saturating_add(1),
        if released { 'm' } else { 'M' }
    )
    .into_bytes()
}

fn mouse_button_code(button: MouseButton) -> u8 {
    match button {
        MouseButton::Left => 0,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
    }
}

fn create_authorization(
    profile: &'static EnvironmentProfile,
    job_id: &str,
    generation: &str,
    client_id: &str,
    public_key: &str,
    takeover: bool,
) -> Result<AuthorizationResponse> {
    let token = dashboard_token()?;
    send(
        profile,
        terminal_http_client(Duration::from_secs(20))?
            .post(format!(
                "{}/api/agent-control/runs/{job_id}/terminal/authorizations",
                profile.base_url
            ))
            .bearer_auth(token)
            .header("X-Stozn-Client", "warpspeed")
            .header(
                "Idempotency-Key",
                format!("warpspeed-terminal-{}", uuid::Uuid::new_v4()),
            )
            .json(&AuthorizationRequest {
                execution_generation: generation,
                client_id,
                public_key,
                takeover,
            }),
    )?
    .json()
    .context("invalid native terminal authorization response")
}

fn renew_authorization(
    profile: &'static EnvironmentProfile,
    job_id: &str,
    generation: &str,
    client_id: &str,
    authorization_id: &str,
) -> Result<()> {
    validate_opaque_id("authorization id", authorization_id)?;
    let token = dashboard_token()?;
    let response: RenewalResponse = send(
        profile,
        terminal_http_client(Duration::from_secs(5))?
            .post(format!(
                "{}/api/agent-control/runs/{job_id}/terminal/authorizations/{authorization_id}/renew",
                profile.base_url
            ))
            .bearer_auth(token)
            .header("X-Stozn-Client", "warpspeed")
            .header(
                "Idempotency-Key",
                format!("warpspeed-terminal-{}", uuid::Uuid::new_v4()),
            )
            .json(&LeaseRequest {
                execution_generation: generation,
                client_id,
            }),
    )?
    .json()
    .context("invalid native terminal renewal response")?;
    if response.authorization_id != authorization_id || response.expires_at_ms == 0 {
        bail!("native terminal renewal response did not match the authorization");
    }
    Ok(())
}

fn revoke_authorization(
    profile: &'static EnvironmentProfile,
    job_id: &str,
    generation: &str,
    client_id: &str,
    authorization_id: &str,
    reason: &str,
) -> Result<()> {
    validate_opaque_id("authorization id", authorization_id)?;
    let token = dashboard_token()?;
    let response: RevokeResponse = send(
        profile,
        terminal_http_client(Duration::from_secs(5))?
            .delete(format!(
                "{}/api/agent-control/runs/{job_id}/terminal/authorizations/{authorization_id}",
                profile.base_url
            ))
            .bearer_auth(token)
            .header("X-Stozn-Client", "warpspeed")
            .header(
                "Idempotency-Key",
                format!("warpspeed-terminal-{}", uuid::Uuid::new_v4()),
            )
            .json(&RevokeRequest {
                execution_generation: generation,
                client_id,
                reason,
            }),
    )?
    .json()
    .context("invalid native terminal revoke response")?;
    if response.authorization_id != authorization_id || !response.revoked {
        bail!("native terminal revoke response did not match the authorization");
    }
    Ok(())
}

struct RenewalTask {
    profile: &'static EnvironmentProfile,
    job_id: String,
    generation: String,
    client_id: String,
    authorization_id: String,
    interval: Duration,
}

fn terminal_http_client(timeout: Duration) -> Result<Client> {
    Client::builder()
        .timeout(timeout)
        .build()
        .context("failed to create native terminal HTTP client")
}

fn spawn_renewal(
    task: RenewalTask,
    stop: mpsc::Receiver<()>,
    failed: Arc<AtomicBool>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut next_renewal = Instant::now() + task.interval;
        loop {
            let wait = next_renewal.saturating_duration_since(Instant::now());
            match stop.recv_timeout(wait) {
                Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => return,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
            if renew_authorization(
                task.profile,
                &task.job_id,
                &task.generation,
                &task.client_id,
                &task.authorization_id,
            )
            .is_err()
            {
                failed.store(true, Ordering::SeqCst);
                return;
            }
            next_renewal += task.interval;
            if next_renewal <= Instant::now() {
                next_renewal = Instant::now() + Duration::from_secs(1);
            }
        }
    })
}

fn run_ssh(
    files: &EphemeralSshFiles,
    endpoint: &SshEndpoint,
    renewal_failed: &AtomicBool,
) -> Result<ExitStatus> {
    validate_endpoint(endpoint)?;
    let mut command = ssh_command(files, endpoint);
    let mut child = command
        .spawn()
        .context("OpenSSH is required for native terminal attachment")?;
    loop {
        if let Some(status) = child.try_wait().context("failed to wait for OpenSSH")? {
            return Ok(status);
        }
        if renewal_failed.load(Ordering::SeqCst) {
            child
                .kill()
                .context("failed to close the expired native terminal connection")?;
            return child.wait().context("failed to wait for OpenSSH shutdown");
        }
        thread::sleep(CHILD_POLL);
    }
}

fn ssh_command(files: &EphemeralSshFiles, endpoint: &SshEndpoint) -> Command {
    let mut command = Command::new("ssh");
    command
        .args(["-F", "none"])
        .arg("-tt")
        .args(["-p", &endpoint.port.to_string()])
        .args(["-i"])
        .arg(&files.private_key)
        .args(["-o", "BatchMode=yes"])
        .args(["-o", "IdentitiesOnly=yes"])
        .args(["-o", "IdentityAgent=none"])
        .args(["-o", "PreferredAuthentications=publickey"])
        .args(["-o", "PasswordAuthentication=no"])
        .args(["-o", "KbdInteractiveAuthentication=no"])
        .args(["-o", "GSSAPIAuthentication=no"])
        .args(["-o", "HostbasedAuthentication=no"])
        .args(["-o", "StrictHostKeyChecking=yes"])
        .args(["-o", "UpdateHostKeys=no"])
        .arg("-o")
        .arg(format!(
            "UserKnownHostsFile={}",
            files.known_hosts.display()
        ))
        .args(["-o", null_known_hosts_option()])
        .args(["-o", "HostKeyAlgorithms=ssh-ed25519"])
        .args(["-o", "PubkeyAcceptedAlgorithms=ssh-ed25519"])
        .args(["-o", "PasswordAuthentication=no"])
        .args(["-o", "KbdInteractiveAuthentication=no"])
        .args(["-o", "ForwardAgent=no"])
        .args(["-o", "ClearAllForwardings=yes"])
        .args(["-o", "PermitLocalCommand=no"])
        .args(["-o", "ControlMaster=no"])
        .args(["-o", "ControlPath=none"])
        .args(["-o", "ControlPersist=no"])
        .args(["-o", "RequestTTY=force"])
        .args(["-o", "ConnectTimeout=10"])
        .args(["-o", "ConnectionAttempts=1"])
        .args(["-o", "ServerAliveInterval=15"])
        .args(["-o", "ServerAliveCountMax=2"])
        .args(["-o", "LogLevel=ERROR"])
        .arg(format!("{}@{}", endpoint.user, endpoint.host))
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    command
}

fn validate_authorization(
    response: &AuthorizationResponse,
    job_id: &str,
    generation: &str,
    client_id: &str,
    takeover: bool,
) -> Result<()> {
    validate_opaque_id("authorization id", &response.authorization_id)?;
    if response.job_id != job_id
        || response.execution_generation != generation
        || response.client_id != client_id
        || response.takeover != takeover
        || response.expires_at_ms == 0
        || response.lease_renew_after_ms == 0
        || response.lease_renew_after_ms > 20_000
    {
        bail!("native terminal authorization response did not match the request");
    }
    validate_endpoint(&response.endpoint)
}

fn validate_endpoint(endpoint: &SshEndpoint) -> Result<()> {
    if endpoint.port != SSH_PORT || endpoint.user != SSH_USER {
        bail!("native terminal endpoint has an unsupported port or user");
    }
    let address: IpAddr = endpoint
        .host
        .parse()
        .context("native terminal endpoint is not an IP address")?;
    if !is_private_address(address) {
        bail!("native terminal endpoint is not private");
    }
    validate_public_key(&endpoint.host_public_key)?;
    Ok(())
}

fn validate_public_key(public_key: &str) -> Result<()> {
    use base64::Engine as _;

    let mut parts = public_key.split_whitespace();
    if parts.next() != Some("ssh-ed25519") {
        bail!("native terminal host key is not Ed25519");
    }
    let encoded = parts
        .next()
        .context("native terminal host key body is missing")?;
    if parts.next().is_some() {
        bail!("native terminal host key contains unexpected fields");
    }
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .context("native terminal host key is invalid")?;
    if decoded.len() != 51
        || decoded[..4] != 11_u32.to_be_bytes()
        || &decoded[4..15] != b"ssh-ed25519"
        || decoded[15..19] != 32_u32.to_be_bytes()
    {
        bail!("native terminal host key is not canonical Ed25519");
    }
    Ok(())
}

fn validate_opaque_id(name: &str, value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        bail!("native terminal {name} is invalid");
    }
    Ok(())
}

fn renewal_interval(authorization: &AuthorizationResponse) -> Duration {
    Duration::from_millis(authorization.lease_renew_after_ms.clamp(1_000, 20_000))
}

fn is_private_address(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => {
            address.is_private() || address.is_loopback() || address.is_link_local()
        }
        IpAddr::V6(address) => {
            address.is_loopback() || address.is_unique_local() || address.is_unicast_link_local()
        }
    }
}

struct EphemeralSshFiles {
    directory: PathBuf,
    private_key: PathBuf,
    known_hosts: PathBuf,
    public_key: String,
}

impl EphemeralSshFiles {
    fn create() -> Result<Self> {
        let directory =
            std::env::temp_dir().join(format!("warpspeed-agentctrl-{}", uuid::Uuid::new_v4()));
        create_private_directory(&directory)?;
        let private_key = directory.join("identity");
        let known_hosts = directory.join("known_hosts");
        let mut files = Self {
            directory,
            private_key,
            known_hosts,
            public_key: String::new(),
        };
        let status = Command::new("ssh-keygen")
            .args(["-q", "-t", "ed25519", "-N", "", "-C", ""])
            .args(["-f"])
            .arg(&files.private_key)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .context("ssh-keygen is required for native terminal attachment")?;
        if !status.success() {
            bail!("ssh-keygen could not create an ephemeral terminal identity");
        }
        let public_key_path = files.private_key.with_extension("pub");
        let public_key = fs::read_to_string(&public_key_path)
            .context("failed to read ephemeral terminal public key")?;
        fs::remove_file(public_key_path).context("failed to remove temporary public key file")?;
        files.public_key = public_key.trim().to_string();
        validate_public_key(&files.public_key)?;
        Ok(files)
    }

    fn write_known_host(&self, endpoint: &SshEndpoint) -> Result<()> {
        validate_endpoint(endpoint)?;
        let (_, key) = endpoint
            .host_public_key
            .split_once(' ')
            .context("native terminal host key body is missing")?;
        let mut file = private_file(&self.known_hosts)?;
        writeln!(
            file,
            "[{}]:{} ssh-ed25519 {key}",
            endpoint.host, endpoint.port
        )
        .context("failed to pin native terminal host key")
    }
}

impl Drop for EphemeralSshFiles {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}

#[cfg(unix)]
fn create_private_directory(path: &Path) -> Result<()> {
    use std::os::unix::fs::DirBuilderExt as _;
    let mut builder = fs::DirBuilder::new();
    builder.mode(0o700);
    builder
        .create(path)
        .context("failed to create temporary SSH directory")
}

#[cfg(windows)]
fn create_private_directory(path: &Path) -> Result<()> {
    fs::create_dir(path).context("failed to create temporary SSH directory")
}

#[cfg(unix)]
fn private_file(path: &Path) -> Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt as _;
    fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .context("failed to create private SSH file")
}

#[cfg(windows)]
fn private_file(path: &Path) -> Result<fs::File> {
    fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .context("failed to create private SSH file")
}

#[cfg(windows)]
fn null_known_hosts_option() -> &'static str {
    "GlobalKnownHostsFile=NUL"
}

#[cfg(not(windows))]
fn null_known_hosts_option() -> &'static str {
    "GlobalKnownHostsFile=/dev/null"
}

#[cfg(test)]
#[path = "native_terminal_tests.rs"]
mod tests;
