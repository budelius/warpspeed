//! Where a running instance listens, and how a `sessions` process finds it.
//!
//! ## Why a directory of marker files rather than asking the OS
//!
//! Both platforms can be interrogated for live channels -- Windows can
//! enumerate `\\.\pipe\`, Unix can list a socket directory -- but they answer
//! differently enough that discovery would be two implementations with two sets
//! of failure modes, and the Windows one goes through a directory handle that
//! is not a directory in any sense `std::fs` promises to keep working.
//!
//! So each instance writes one small file naming the channel it listens on, and
//! discovery reads that directory on every platform. A file outliving its
//! process is the expected case, not an error: nothing is trusted until a
//! `Ping` comes back, and a file whose owner never answers is removed by
//! whoever noticed.
//!
//! ## Protocol versions
//!
//! Builds of different protocol versions share the directory, and each marker's file name says
//! which version it speaks. A request goes only to [`current_markers`]: the version in the channel
//! name does not keep builds apart on its own, because the marker hands a reader the channel. Code
//! that only asks whether other instances exist reads every version with [`read_markers`].

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::protocol::PROTOCOL_VERSION;

/// The directory holding one marker file per running instance.
const MARKER_DIRECTORY_NAME: &str = "remote-control";

/// A running instance, as its marker file records it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstanceMarker {
    /// The process id, which is also the marker's file name.
    pub pid: u32,

    /// What to hand [`ipc::Client::connect`] to reach it.
    pub address: String,

    /// The build this instance is running, for telling an installed Warp from
    /// one started out of a build directory.
    pub version: String,
}

/// Six bytes of the state directory's hash.
///
/// Instances are addressed by pid, which is already unique, so this is not
/// about collisions. It scopes *discovery*: `WARPSPEED_PROFILE` gives an
/// instance its own state directory and therefore its own supervisor, and a
/// listing that mixed profiles would offer sessions from a supervisor the other
/// subcommands never talk to.
fn state_directory_hash() -> String {
    let hash = Sha256::digest(warp_core::paths::state_dir().to_string_lossy().as_bytes());
    hex::encode(&hash[..6])
}

/// The directory this profile's marker files live in.
pub fn marker_directory() -> PathBuf {
    warp_core::paths::state_dir().join(MARKER_DIRECTORY_NAME)
}

/// The marker file for one process.
pub fn marker_path(pid: u32) -> PathBuf {
    marker_directory().join(format!("v{PROTOCOL_VERSION}-{pid}.json"))
}

/// The channel this process listens on.
///
/// On Windows this is a named-pipe name, which `interprocess` places under
/// `\\.\pipe\` with the default security descriptor -- the same exposure the
/// app's own URI channel has, created by the same crate.
///
/// On Unix it has to be a filesystem path, and it goes in the supervisor's own
/// socket directory: per-user, `0700`, never `/tmp`, and short enough that a
/// `sockaddr_un` can hold it. Those properties were worked out once for the
/// supervisor, and a second directory chosen separately would be a second
/// chance to get them wrong.
///
/// What reaching this channel grants is worth stating. A caller can list panes and detached
/// session metadata, move a session between the supervisor and a window, and quit the app. It can
/// also read every pane's agent session id, agent directory, and transcript
/// path, and move a pane's agent session to AgentCtrl unattended: that pushes a
/// handoff commit of the working tree, untracked files that are not ignored
/// included, uploads the session transcript to AgentCtrl staging, and types the
/// local agent's exit command into its pane. The caller may name the directory
/// for a pane whose shell has none, but only one the agent itself recorded is
/// honoured, so it cannot pick which repository is pushed. Beyond that it
/// cannot read a terminal's output or write to one: that needs the supervisor's
/// own channel, which lends PTY handles and is guarded accordingly.
#[cfg(windows)]
pub fn listen_address(pid: u32) -> Result<String> {
    Ok(format!(
        "warp-remote-control-v{}-{}-{}",
        PROTOCOL_VERSION,
        state_directory_hash(),
        pid
    ))
}

#[cfg(unix)]
pub fn listen_address(pid: u32) -> Result<String> {
    use warp_terminal::local_tty::unix_supervisor::socket::{
        prepare_socket_directory, supervisor_socket_directory,
    };

    let directory = supervisor_socket_directory();
    prepare_socket_directory(&directory)?;
    let path = directory.join(format!(
        "ctl-v{}-{}-{}.sock",
        PROTOCOL_VERSION,
        state_directory_hash(),
        pid
    ));
    Ok(path.to_string_lossy().into_owned())
}

#[cfg(not(any(unix, windows)))]
pub fn listen_address(_pid: u32) -> Result<String> {
    anyhow::bail!("Remote control is not supported on this platform")
}

/// Records that this process is listening, so the `sessions` command can find it.
pub fn write_marker(marker: &InstanceMarker) -> Result<()> {
    let directory = marker_directory();
    std::fs::create_dir_all(&directory)
        .with_context(|| format!("Failed to create {directory:?}"))?;
    let path = marker_path(marker.pid);
    let contents =
        serde_json::to_vec_pretty(marker).context("Failed to encode an instance marker")?;
    std::fs::write(&path, contents).with_context(|| format!("Failed to write {path:?}"))?;
    Ok(())
}

/// Removes a marker, and on Unix the socket it named.
///
/// Errors are swallowed: this runs while an instance is going away, and there
/// is nothing useful a dying process can do about a file it cannot delete.
/// Discovery prunes whatever is left behind.
pub fn remove_marker(marker: &InstanceMarker) {
    let _ = std::fs::remove_file(marker_path(marker.pid));
    #[cfg(unix)]
    let _ = std::fs::remove_file(Path::new(&marker.address));
    #[cfg(not(unix))]
    let _ = Path::new(&marker.address);
}

/// A marker as discovery found it on disk.
#[derive(Debug, Clone)]
pub struct FoundMarker {
    /// The protocol version the marker's file name declares, or `None` for a name this build does
    /// not recognise.
    pub protocol_version: Option<u32>,
    pub marker: InstanceMarker,
}

impl FoundMarker {
    /// Whether this build can talk to the instance. One on any other version would decode this
    /// build's messages as something else.
    pub fn speaks_this_protocol(&self) -> bool {
        self.protocol_version == Some(PROTOCOL_VERSION)
    }
}

/// The protocol version a marker file name declares: `v7-1234.json` is version 7.
fn marker_protocol_version(file_name: &str) -> Option<u32> {
    let (version, _) = file_name.strip_prefix('v')?.split_once('-')?;
    version.parse().ok()
}

/// Every marker in `directory`, of every protocol version, newest pid last.
fn read_markers_in(directory: &Path) -> Vec<FoundMarker> {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut markers: Vec<FoundMarker> = entries
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))
        .filter_map(|entry| {
            let contents = std::fs::read(entry.path()).ok()?;
            let marker = serde_json::from_slice::<InstanceMarker>(&contents).ok()?;
            Some(FoundMarker {
                protocol_version: marker_protocol_version(&entry.file_name().to_string_lossy()),
                marker,
            })
        })
        .collect();
    markers.sort_by_key(|found| found.marker.pid);
    markers
}

/// Every marker currently on disk, of every protocol version, newest pid last.
///
/// Says nothing about whether the instances are alive. Callers probe.
pub fn found_markers() -> Vec<FoundMarker> {
    read_markers_in(&marker_directory())
}

/// Every instance marker on disk, whatever protocol it speaks.
///
/// For deciding whether other instances exist. Only [`current_markers`] are safe to send a
/// request to.
pub fn read_markers() -> Vec<InstanceMarker> {
    found_markers()
        .into_iter()
        .map(|found| found.marker)
        .collect()
}

/// The markers of instances speaking this build's protocol, the only ones a request can be sent
/// to.
///
/// The version is in the channel name too, but that does not keep builds apart: a marker records
/// its channel, so a reader that took every marker would connect to an older build's channel and
/// have its bytes read as different messages.
pub fn current_markers() -> Vec<InstanceMarker> {
    found_markers()
        .into_iter()
        .filter(FoundMarker::speaks_this_protocol)
        .map(|found| found.marker)
        .collect()
}

/// The markers of other protocol versions whose processes are still running.
///
/// This build cannot talk to them, but a command that finds no instance should say they exist
/// rather than report that nothing is running.
pub fn running_markers_of_other_protocols() -> Vec<FoundMarker> {
    let others: Vec<FoundMarker> = found_markers()
        .into_iter()
        .filter(|found| !found.speaks_this_protocol())
        .collect();
    let running = running_pids(others.iter().map(|found| found.marker.pid));
    others
        .into_iter()
        .filter(|found| running.contains(&found.marker.pid))
        .collect()
}

/// Which of `pids` belong to running processes.
///
/// Checks the process table rather than pinging. A reused pid can make a dead instance look
/// alive, which only errs towards caution.
fn running_pids(pids: impl IntoIterator<Item = u32>) -> Vec<u32> {
    let pids: Vec<sysinfo::Pid> = pids.into_iter().map(sysinfo::Pid::from_u32).collect();
    if pids.is_empty() {
        return Vec::new();
    }
    let mut system = sysinfo::System::new();
    system.refresh_processes_specifics(
        sysinfo::ProcessesToUpdate::Some(&pids),
        false, /* refresh_dead_processes */
        sysinfo::ProcessRefreshKind::nothing(),
    );
    pids.into_iter()
        .filter(|pid| system.process(*pid).is_some())
        .map(|pid| pid.as_u32())
        .collect()
}

/// Pids of other instances whose processes are still running, on any protocol version.
///
/// Cheap enough to call on every save. Every build shares one saved session, so an instance of
/// another version counts as much as one of this version.
pub fn other_running_instance_pids() -> Vec<u32> {
    let own_pid = std::process::id();
    running_pids(
        read_markers()
            .into_iter()
            .map(|marker| marker.pid)
            .filter(|pid| *pid != own_pid),
    )
}

/// Deletes the marker for a pid that did not answer.
pub fn forget(pid: u32) {
    let _ = std::fs::remove_file(marker_path(pid));
}

#[cfg(test)]
#[path = "address_tests.rs"]
mod tests;
