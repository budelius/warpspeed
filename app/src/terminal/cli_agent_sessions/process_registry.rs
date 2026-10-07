//! Finds the session a CLI agent running in a pane is using, by asking the agent's own process
//! registry rather than guessing from transcripts.
//!
//! Claude Code writes `<config dir>/sessions/<pid>.json` for every running instance, holding the
//! exact session ID it is writing to. Pi has no such file, so Warp installs a Pi extension that
//! writes the equivalent under `<agent dir>/warp-sessions/<pid>.json`. Both are keyed by the
//! agent's process ID, so the pane's shell is walked down to the agent process and its entry
//! read. A transcript-based guess -- the newest conversation in the pane's directory -- swaps
//! sessions as soon as two agents share a directory, which is the common case.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use instant::Instant;
use serde::Deserialize;
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System};

use crate::terminal::CLIAgent;

/// How long one process table read is reused. Every pane with an agent asks on its own
/// schedule, and walking every process on the machine once per pane would be wasted work.
const PROCESS_TABLE_TTL: Duration = Duration::from_secs(5);

const PI_REGISTRY_DIRECTORY: &str = "warp-sessions";
const PI_EXTENSION_FILE_NAME: &str = "warp-session-registry.ts";
const PI_EXTENSION_SOURCE: &str = include_str!("process_registry/warp-session-registry.ts");

/// The session an agent process reported, and the directory it is running in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AgentProcessSession {
    pub session_id: String,
    pub cwd: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ProcessInfo {
    pub pid: u32,
    pub parent: Option<u32>,
    pub name: String,
    /// When the process started, in seconds since the Unix epoch.
    pub start_time: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RegistryEntry {
    session_id: String,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    warp_session_uuid: Option<String>,
    /// Claude's process start, in milliseconds since the Unix epoch.
    #[serde(default)]
    started_at: Option<u64>,
    /// When the entry was last written, in milliseconds since the Unix epoch.
    #[serde(default)]
    updated_at: Option<u64>,
}

/// A registry entry as read from disk.
struct StoredEntry {
    session: AgentProcessSession,
    warp_session_uuid: Option<String>,
    /// The earliest moment the entry is known to have been written, in seconds since the Unix
    /// epoch.
    written_secs: u64,
}

/// Agents whose session can be read from a process registry.
pub(crate) fn supports(agent: CLIAgent) -> bool {
    registry_directory(agent).is_some()
}

/// The session of the `agent` process running under `shell_pid`, if it can be found.
///
/// Blocks on process enumeration and file reads, so callers run it off the main thread.
pub(crate) fn resolve(
    agent: CLIAgent,
    shell_pid: u32,
    session_uuid_hex: Option<&str>,
) -> Option<AgentProcessSession> {
    let directory = registry_directory(agent)?;
    resolve_in(
        agent,
        shell_pid,
        session_uuid_hex,
        &process_table(),
        &directory,
    )
}

/// The agent running under `shell_pid`, for a pane that is already running one it did not see
/// start -- a session reattached from the supervisor, say.
pub(crate) fn discover(
    shell_pid: u32,
    session_uuid_hex: Option<&str>,
) -> Option<(CLIAgent, AgentProcessSession)> {
    let processes = process_table();
    DISCOVERABLE_AGENTS.into_iter().find_map(|agent| {
        let directory = registry_directory(agent)?;
        resolve_in(agent, shell_pid, session_uuid_hex, &processes, &directory)
            .map(|session| (agent, session))
    })
}

const DISCOVERABLE_AGENTS: [CLIAgent; 2] = [CLIAgent::Claude, CLIAgent::Pi];

pub(crate) fn resolve_in(
    agent: CLIAgent,
    shell_pid: u32,
    session_uuid_hex: Option<&str>,
    processes: &[ProcessInfo],
    registry_directory: &Path,
) -> Option<AgentProcessSession> {
    let by_pid: HashMap<u32, &ProcessInfo> = processes
        .iter()
        .map(|process| (process.pid, process))
        .collect();
    if let Some(uuid) = session_uuid_hex
        && let Some(shell) = by_pid.get(&shell_pid)
    {
        let by_uuid = processes
            .iter()
            .filter(|process| process.start_time >= shell.start_time)
            .filter_map(|process| live_entry(agent, process, registry_directory))
            .find(|entry| entry.warp_session_uuid.as_deref() == Some(uuid))
            .map(|entry| entry.session);
        if by_uuid.is_some() {
            return by_uuid;
        }
    }
    descendants(shell_pid, processes)
        .into_iter()
        .filter_map(|pid| by_pid.get(&pid))
        .find_map(|process| live_entry(agent, process, registry_directory))
        .map(|entry| entry.session)
}

/// The sessions agents on this machine are running right now, whichever pane or terminal they
/// are in.
pub(crate) fn running_session_ids() -> std::collections::HashSet<String> {
    let processes = process_table();
    DISCOVERABLE_AGENTS
        .into_iter()
        .filter_map(|agent| Some((agent, registry_directory(agent)?)))
        .flat_map(|(agent, directory)| {
            processes
                .iter()
                .filter_map(|process| live_entry(agent, process, &directory))
                .map(|entry| entry.session.session_id)
                .collect::<Vec<_>>()
        })
        .collect()
}

/// The process of the `agent` that is running `session_id`, by its own registry entry.
pub(crate) fn agent_pid_for_session(agent: CLIAgent, session_id: &str) -> Option<u32> {
    let directory = registry_directory(agent)?;
    agent_pid_for_session_in(agent, session_id, &process_table(), &directory)
}

pub(crate) fn agent_pid_for_session_in(
    agent: CLIAgent,
    session_id: &str,
    processes: &[ProcessInfo],
    registry_directory: &Path,
) -> Option<u32> {
    processes
        .iter()
        .find(|process| {
            live_entry(agent, process, registry_directory)
                .is_some_and(|entry| entry.session.session_id == session_id)
        })
        .map(|process| process.pid)
}

/// Ends the agent process holding `session_id`. `Ok(false)` when none is registered.
///
/// The pid comes from a cached table and an entry a crashed agent can leave behind, and Windows
/// reuses pids, so the process is checked again, fresh, before it is ended.
pub(crate) fn terminate_agent_for_session(agent: CLIAgent, session_id: &str) -> Result<bool> {
    let Some(directory) = registry_directory(agent) else {
        return Ok(false);
    };
    let Some(pid) = agent_pid_for_session_in(agent, session_id, &process_table(), &directory)
    else {
        return Ok(false);
    };
    let mut system = System::new();
    refresh_process(&mut system, pid);
    let process = system
        .process(sysinfo::Pid::from_u32(pid))
        .with_context(|| format!("The {} process {pid} is already gone", agent.display_name()))?;
    let still_the_agent = is_agent_process(agent, &process.name().to_string_lossy())
        && read_stored_entry(&entry_path(&directory, pid)).is_some_and(|entry| {
            entry.session.session_id == session_id
                && entry_can_belong_to(process.start_time(), entry.written_secs)
        });
    if !still_the_agent {
        return Ok(false);
    }
    if !process.kill() {
        bail!("Could not end the {} process {pid}", agent.display_name());
    }
    Ok(true)
}

/// Whether a process that started at `process_start_secs` can have written a registry entry
/// written at `entry_written_secs`, both in seconds since the Unix epoch. A process that started
/// later received the pid of the agent that wrote the entry.
pub(crate) fn entry_can_belong_to(process_start_secs: u64, entry_written_secs: u64) -> bool {
    process_start_secs <= entry_written_secs
}

fn refresh_process(system: &mut System, pid: u32) {
    system.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[sysinfo::Pid::from_u32(pid)]),
        true,
        ProcessRefreshKind::nothing(),
    );
}

/// Whether any process is running under `pid`.
pub(crate) fn has_child_process(pid: u32) -> bool {
    has_child_process_in(pid, &process_table())
}

pub(crate) fn has_child_process_in(pid: u32, processes: &[ProcessInfo]) -> bool {
    processes
        .iter()
        .any(|process| process.parent == Some(pid) && process.pid != pid)
}

/// Every process below `root`, nearest first.
pub(crate) fn descendants(root: u32, processes: &[ProcessInfo]) -> Vec<u32> {
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for process in processes {
        if let Some(parent) = process.parent
            && parent != process.pid
        {
            children.entry(parent).or_default().push(process.pid);
        }
    }
    let mut found = Vec::new();
    let mut queue = VecDeque::from([root]);
    while let Some(pid) = queue.pop_front() {
        for &child in children.get(&pid).into_iter().flatten() {
            if !found.contains(&child) && child != root {
                found.push(child);
                queue.push_back(child);
            }
        }
    }
    found
}

/// Whether a process with this executable name can be the agent itself. A registry entry left
/// behind by an agent that crashed must not be credited to an unrelated process that later
/// received the same ID.
fn is_agent_process(agent: CLIAgent, name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    match agent {
        // Native installs run as `claude`; npm installs run on Node.
        CLIAgent::Claude => name.starts_with("claude") || is_script_runtime(&name),
        CLIAgent::Pi => name.starts_with("pi") || is_script_runtime(&name),
        CLIAgent::Gemini
        | CLIAgent::Codex
        | CLIAgent::Amp
        | CLIAgent::Droid
        | CLIAgent::OpenCode
        | CLIAgent::Copilot
        | CLIAgent::OhMyPi
        | CLIAgent::Auggie
        | CLIAgent::CursorCli
        | CLIAgent::Goose
        | CLIAgent::Hermes
        | CLIAgent::Vibe
        | CLIAgent::Antigravity
        | CLIAgent::WarpTui
        | CLIAgent::Unknown => false,
    }
}

fn is_script_runtime(name: &str) -> bool {
    name.starts_with("node") || name.starts_with("bun")
}

fn entry_path(registry_directory: &Path, pid: u32) -> PathBuf {
    registry_directory.join(format!("{pid}.json"))
}

/// The entry `process` wrote, if it is the `agent` and can have written it.
fn live_entry(
    agent: CLIAgent,
    process: &ProcessInfo,
    registry_directory: &Path,
) -> Option<StoredEntry> {
    if !is_agent_process(agent, &process.name) {
        return None;
    }
    read_stored_entry(&entry_path(registry_directory, process.pid))
        .filter(|entry| entry_can_belong_to(process.start_time, entry.written_secs))
}

fn read_stored_entry(path: &Path) -> Option<StoredEntry> {
    let contents = std::fs::read(path).ok()?;
    let entry: RegistryEntry = serde_json::from_slice(&contents).ok()?;
    if !is_safe_session_id(&entry.session_id) {
        return None;
    }
    let written_secs = match entry.started_at.or(entry.updated_at) {
        Some(millis) => millis / 1000,
        None => std::fs::metadata(path)
            .and_then(|metadata| metadata.modified())
            .ok()?
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_secs(),
    };
    Some(StoredEntry {
        session: AgentProcessSession {
            session_id: entry.session_id,
            cwd: entry.cwd.filter(|cwd| !cwd.is_empty()),
        },
        warp_session_uuid: entry.warp_session_uuid,
        written_secs,
    })
}

/// The ID becomes an argument of a command typed into a shell, so it is held to the characters
/// real IDs use.
fn is_safe_session_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
}

fn registry_directory(agent: CLIAgent) -> Option<PathBuf> {
    match agent {
        CLIAgent::Claude => {
            crate::ai::agent_sdk::driver::harness::claude_transcript::claude_config_dir()
                .ok()
                .map(|dir| dir.join("sessions"))
        }
        CLIAgent::Pi => pi_agent_dir().map(|dir| dir.join(PI_REGISTRY_DIRECTORY)),
        CLIAgent::Gemini
        | CLIAgent::Codex
        | CLIAgent::Amp
        | CLIAgent::Droid
        | CLIAgent::OpenCode
        | CLIAgent::Copilot
        | CLIAgent::OhMyPi
        | CLIAgent::Auggie
        | CLIAgent::CursorCli
        | CLIAgent::Goose
        | CLIAgent::Hermes
        | CLIAgent::Vibe
        | CLIAgent::Antigravity
        | CLIAgent::WarpTui
        | CLIAgent::Unknown => None,
    }
}

fn pi_agent_dir() -> Option<PathBuf> {
    match std::env::var_os("PI_CODING_AGENT_DIR") {
        Some(dir) if !dir.is_empty() => Some(PathBuf::from(dir)),
        _ => dirs::home_dir().map(|home| home.join(".pi").join("agent")),
    }
}

fn process_table() -> Arc<Vec<ProcessInfo>> {
    static CACHE: Mutex<Option<(Instant, Arc<Vec<ProcessInfo>>)>> = Mutex::new(None);
    let mut cache = CACHE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some((read_at, table)) = cache.as_ref()
        && read_at.elapsed() < PROCESS_TABLE_TTL
    {
        return table.clone();
    }
    let mut system = System::new();
    system.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing());
    let table: Arc<Vec<ProcessInfo>> = Arc::new(
        system
            .processes()
            .values()
            .map(|process| ProcessInfo {
                pid: process.pid().as_u32(),
                parent: process.parent().map(|parent| parent.as_u32()),
                name: process.name().to_string_lossy().into_owned(),
                start_time: process.start_time(),
            })
            .collect(),
    );
    *cache = Some((Instant::now(), table.clone()));
    table
}

/// Installs or updates the Pi extension that writes Pi's process registry, when Pi is set up on
/// this machine. Returns whether the file was written.
pub(crate) fn install_pi_extension() -> Result<bool> {
    match pi_agent_dir() {
        Some(dir) if dir.is_dir() => install_pi_extension_in(&dir),
        _ => Ok(false),
    }
}

pub(crate) fn install_pi_extension_in(pi_agent_dir: &Path) -> Result<bool> {
    let extensions = pi_agent_dir.join("extensions");
    let path = extensions.join(PI_EXTENSION_FILE_NAME);
    if std::fs::read_to_string(&path).is_ok_and(|existing| existing == PI_EXTENSION_SOURCE) {
        return Ok(false);
    }
    std::fs::create_dir_all(&extensions)
        .with_context(|| format!("Failed to create Pi extensions directory {extensions:?}"))?;
    std::fs::write(&path, PI_EXTENSION_SOURCE)
        .with_context(|| format!("Failed to write Pi extension {path:?}"))?;
    Ok(true)
}

#[cfg(test)]
#[path = "process_registry_tests.rs"]
mod tests;
