use std::io::{IsTerminal as _, Write as _};
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;
use std::{fs, thread};

use anyhow::{Context, Result, bail};
use command::blocking::Command;
use instant::Instant;
use reqwest::StatusCode;
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use warp_cli::managed_sessions::ManagedEnvironment;

use super::{
    EnvironmentProfile, SessionDescriptor, dashboard_token, fetch_descriptor, profile, send,
};

const SSH_PORT: u16 = 2222;
const SSH_USER: &str = "agentctrl";
const ATTACH_WAIT: Duration = Duration::from_secs(30);
const ATTACH_POLL: Duration = Duration::from_millis(500);
const CHILD_POLL: Duration = Duration::from_millis(200);

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
    let client = Client::builder().timeout(Duration::from_secs(25)).build()?;
    let deadline = Instant::now() + ATTACH_WAIT;
    let mut descriptor = wait_for_ready_descriptor(&client, profile, job_id, deadline)?;
    for attempt in 0..2 {
        if let Some(reason) = descriptor_unavailable_reason(&descriptor) {
            return Ok(NativeAttachOutcome::Fallback(reason));
        }
        match attach_descriptor(profile, &descriptor, takeover) {
            Ok(()) => return Ok(NativeAttachOutcome::Attached),
            Err(error) if attempt == 0 && is_stale_generation(&error) => {
                descriptor = wait_for_ready_descriptor(&client, profile, job_id, deadline)?;
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
    client: &Client,
    profile: &'static EnvironmentProfile,
    job_id: &str,
    deadline: Instant,
) -> Result<SessionDescriptor> {
    loop {
        let token = dashboard_token()?;
        let descriptor = fetch_descriptor(client, profile, job_id, &token)?;
        match descriptor
            .terminal
            .as_ref()
            .map(|terminal| terminal.availability.as_str())
        {
            Some("ready" | "attached") => return Ok(descriptor),
            Some("starting") if Instant::now() < deadline => thread::sleep(ATTACH_POLL),
            Some("starting") => {
                return Ok(fallback_descriptor(
                    descriptor,
                    "Native Pi terminal is still starting",
                ));
            }
            Some("paused") => {
                return Ok(fallback_descriptor(
                    descriptor,
                    "Run is paused; resume it to create a new native terminal",
                ));
            }
            Some("completed" | "stopped" | "failed" | "lost" | "unsupported") | None => {
                return Ok(descriptor);
            }
            Some(_) => return Ok(descriptor),
        }
    }
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
