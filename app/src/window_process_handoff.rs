use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::time::Duration;

use anyhow::{Context as _, Result, anyhow, bail};
use instant::Instant;
use serde::{Deserialize, Serialize};
use warpui::r#async::Timer;

use crate::app_state::WindowSnapshot;

const HANDOFF_ENV: &str = "WARP_WINDOW_PROCESS_HANDOFF";
const INSTALL_ROOT_ENV: &str = "WARPSPEED_INSTALL_ROOT";
const PROTOCOL_VERSION: u32 = 1;
const READY_TIMEOUT: Duration = Duration::from_secs(60);
const RELEASE_TIMEOUT: Duration = Duration::from_secs(30);
const POLL_INTERVAL: Duration = Duration::from_millis(50);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct AgentResumeCommand {
    pub pane_uuid: Vec<u8>,
    pub command: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct HandoffPayload {
    protocol_version: u32,
    window: WindowSnapshot,
    agent_resume_commands: Vec<AgentResumeCommand>,
    #[serde(default)]
    transfers_session_lease: bool,
}

pub(crate) struct IncomingWindowHandoff {
    payload: HandoffPayload,
    request_path: PathBuf,
    ready_path: PathBuf,
    release_path: PathBuf,
}

pub(crate) struct OutgoingWindowHandoff {
    child: Child,
    request_path: PathBuf,
    ready_path: PathBuf,
    release_path: PathBuf,
}

impl IncomingWindowHandoff {
    pub(crate) fn from_environment() -> Result<Option<Self>> {
        let Some(request_path) = std::env::var_os(HANDOFF_ENV).map(PathBuf::from) else {
            return Ok(None);
        };

        // This runs before application threads start. Removing the one-shot path prevents PTYs and
        // worker processes spawned by the replacement process from inheriting the handoff request.
        unsafe { std::env::remove_var(HANDOFF_ENV) };

        Self::from_path(request_path).map(Some)
    }

    fn from_path(request_path: PathBuf) -> Result<Self> {
        let bytes = fs::read(&request_path).context("Failed to read window process handoff")?;
        let payload: HandoffPayload =
            serde_json::from_slice(&bytes).context("Failed to decode window process handoff")?;
        if payload.protocol_version != PROTOCOL_VERSION {
            bail!(
                "Unsupported window process handoff protocol version {}",
                payload.protocol_version
            );
        }

        let (ready_path, release_path) = marker_paths(&request_path);
        let _ = fs::remove_file(&request_path);
        Ok(Self {
            payload,
            request_path,
            ready_path,
            release_path,
        })
    }

    pub(crate) fn window(&self) -> &WindowSnapshot {
        &self.payload.window
    }

    pub(crate) fn agent_resume_commands(&self) -> &[AgentResumeCommand] {
        &self.payload.agent_resume_commands
    }

    pub(crate) fn transfers_session_lease(&self) -> bool {
        self.payload.transfers_session_lease
    }

    pub(crate) fn mark_ready(&self) -> Result<()> {
        write_marker(&self.ready_path).context("Failed to acknowledge window process handoff")
    }

    pub(crate) async fn wait_for_release(self) -> bool {
        let released = wait_for_path(&self.release_path, RELEASE_TIMEOUT).await;
        self.cleanup();
        released
    }

    fn cleanup(&self) {
        for path in [&self.request_path, &self.ready_path, &self.release_path] {
            let _ = fs::remove_file(path);
        }
    }
}

impl OutgoingWindowHandoff {
    pub(crate) async fn wait_until_ready(self) -> (Self, bool) {
        let ready = wait_for_path(&self.ready_path, READY_TIMEOUT).await;
        (self, ready)
    }

    pub(crate) fn release(&self) -> Result<()> {
        write_marker(&self.release_path).context("Failed to release window process handoff")
    }

    pub(crate) fn abort(mut self) {
        let _ = self.child.kill();
        self.cleanup();
    }

    fn cleanup(&self) {
        for path in [&self.request_path, &self.ready_path, &self.release_path] {
            let _ = fs::remove_file(path);
        }
    }
}

pub(crate) fn launch(
    window: WindowSnapshot,
    agent_resume_commands: Vec<AgentResumeCommand>,
) -> Result<OutgoingWindowHandoff> {
    let handoff_dir = warp_core::paths::state_dir().join("window-process-handoffs");
    fs::create_dir_all(&handoff_dir)
        .context("Failed to create window process handoff directory")?;

    let request_path = handoff_dir.join(format!("{}.json", uuid::Uuid::new_v4()));
    let (ready_path, release_path) = marker_paths(&request_path);
    let transfers_session_lease = window
        .launch_config_source
        .as_ref()
        .is_some_and(|source| source.is_session_config());
    let payload = HandoffPayload {
        protocol_version: PROTOCOL_VERSION,
        window,
        agent_resume_commands,
        transfers_session_lease,
    };
    write_request(&request_path, &payload)?;

    let executable = replacement_executable()?;
    let child = command::blocking::Command::new(executable)
        .env(HANDOFF_ENV, &request_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("Failed to launch replacement window process");

    match child {
        Ok(child) => Ok(OutgoingWindowHandoff {
            child,
            request_path,
            ready_path,
            release_path,
        }),
        Err(error) => {
            let _ = fs::remove_file(request_path);
            Err(error)
        }
    }
}

fn write_request(path: &Path, payload: &HandoffPayload) -> Result<()> {
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }

    let mut file = options
        .open(path)
        .context("Failed to create window process handoff")?;
    serde_json::to_writer(&mut file, payload).context("Failed to encode window process handoff")?;
    file.flush()
        .context("Failed to flush window process handoff")
}

fn write_marker(path: &Path) -> Result<()> {
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    options.open(path).map(|_: File| ()).map_err(Into::into)
}

fn marker_paths(request_path: &Path) -> (PathBuf, PathBuf) {
    (
        request_path.with_extension("ready"),
        request_path.with_extension("release"),
    )
}

async fn wait_for_path(path: &Path, timeout: Duration) -> bool {
    let started = Instant::now();
    while started.elapsed() < timeout {
        if path.is_file() {
            return true;
        }
        Timer::after(POLL_INTERVAL).await;
    }
    false
}

fn replacement_executable() -> Result<PathBuf> {
    let current_executable = std::env::current_exe()
        .context("Failed to resolve the current executable for window restart")?;
    let Some(install_root) = install_root() else {
        return Ok(current_executable);
    };

    Ok(
        installed_replacement_executable(&current_executable, &install_root)?
            .unwrap_or(current_executable),
    )
}

fn installed_replacement_executable(
    current_executable: &Path,
    install_root: &Path,
) -> Result<Option<PathBuf>> {
    let Ok(version) = fs::read_to_string(install_root.join("current.txt")) else {
        return Ok(None);
    };
    let version = version.trim();
    if version.is_empty()
        || !version
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        return Err(anyhow!("The installed WarpSpeed version is invalid"));
    }

    let executable_name = match current_executable.file_name().and_then(OsStr::to_str) {
        Some("warp-oss.exe" | "warp-oss") => current_executable
            .file_name()
            .expect("the matched executable has a file name"),
        _ if cfg!(windows) => OsStr::new("warpspeed.exe"),
        _ => OsStr::new("warpspeed"),
    };
    let installed_executable = install_root
        .join("versions")
        .join(version)
        .join(executable_name);
    Ok(installed_executable
        .is_file()
        .then_some(installed_executable))
}

fn install_root() -> Option<PathBuf> {
    if let Some(root) = std::env::var_os(INSTALL_ROOT_ENV) {
        return Some(PathBuf::from(root));
    }

    #[cfg(windows)]
    return std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .map(|root| root.join("Programs").join("WarpSpeed"));

    #[cfg(not(windows))]
    None
}

#[cfg(test)]
#[path = "window_process_handoff_tests.rs"]
mod tests;
