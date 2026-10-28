use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use command::blocking::Command;
use serde::Deserialize;
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

use crate::remote_control::address::{self, InstanceMarker};
use crate::terminal::cli_agent_sessions::process_registry::{ProcessInfo, process_table};

const SUPERVISOR_ARGUMENT: &str = "--terminal-session-supervisor";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct SystemOverview {
    pub builds: Vec<BuildOverview>,
}

impl SystemOverview {
    pub(crate) fn process_count(&self) -> usize {
        self.builds
            .iter()
            .map(|build| {
                build.instances.len() + build.supervisor_pids.len() + build.other_process_pids.len()
            })
            .sum()
    }

    pub(crate) fn window_count(&self) -> usize {
        self.builds
            .iter()
            .flat_map(|build| &build.instances)
            .map(|instance| instance.windows.len())
            .sum()
    }

    pub(crate) fn pane_count(&self) -> usize {
        self.builds
            .iter()
            .flat_map(|build| &build.instances)
            .flat_map(|instance| &instance.windows)
            .map(|window| window.panes.len())
            .sum()
    }

    pub(crate) fn supervisor_count(&self) -> usize {
        self.builds
            .iter()
            .map(|build| {
                build
                    .supervisor_pids
                    .len()
                    .max(usize::from(build.supervisor.is_some()))
            })
            .sum()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct BuildOverview {
    pub version: String,
    pub profile: Option<String>,
    pub executable: PathBuf,
    pub instances: Vec<InstanceOverview>,
    pub supervisor_pids: Vec<u32>,
    pub supervisor: Option<SupervisorOverview>,
    pub supervisor_error: Option<String>,
    pub other_process_pids: Vec<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct InstanceOverview {
    pub pid: u32,
    pub version: String,
    pub windows: Vec<WindowOverview>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WindowOverview {
    pub id: String,
    pub index: Option<u32>,
    pub is_active: bool,
    pub has_workspace: bool,
    pub panes: Vec<PaneOverview>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PaneOverview {
    pub terminal_view_id: Option<u64>,
    pub session_id: Option<u64>,
    pub tab_index: usize,
    pub title: String,
    pub shell: String,
    pub working_directory: String,
    pub has_running_agent: bool,
    pub is_busy: bool,
    pub survives_quit: bool,
    pub agent: Option<String>,
    pub agent_session_id: Option<String>,
    pub agent_not_resumed: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SupervisorOverview {
    pub channel: String,
    pub protocol_version: Option<u32>,
    pub state_directory: String,
    pub sessions: Vec<SupervisorSessionOverview>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub(crate) struct SupervisorSessionOverview {
    pub session_id: u64,
    #[serde(default)]
    pub shell: String,
    #[serde(default)]
    pub working_directory: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub detached_for_seconds: u64,
    #[serde(default)]
    pub attached: bool,
    #[serde(default)]
    pub shell_pid: u32,
    #[serde(default)]
    pub session_uuid_hex: String,
}

#[derive(Deserialize)]
struct InstanceReport {
    pid: u32,
    version: String,
}

#[derive(Deserialize)]
struct SupervisorStatus {
    channel: String,
    reachable: bool,
    state_directory: String,
}

#[derive(Deserialize)]
struct PaneReport {
    #[serde(default)]
    terminal_view_id: Option<u64>,
    #[serde(default)]
    session_id: Option<u64>,
    window: String,
    tab_index: usize,
    #[serde(default)]
    title: String,
    #[serde(default)]
    shell: String,
    #[serde(default)]
    working_directory: String,
    #[serde(default)]
    has_running_agent: bool,
    #[serde(default)]
    is_busy: bool,
    #[serde(default)]
    survives_quit: bool,
    #[serde(default)]
    agent: Option<String>,
    #[serde(default)]
    agent_session_id: Option<String>,
    #[serde(default)]
    agent_not_resumed: bool,
}

#[derive(Deserialize)]
struct WindowListReport {
    windows: Vec<WindowReport>,
}

#[derive(Deserialize)]
struct WindowReport {
    window_id: String,
    index: u32,
    is_active: bool,
    has_workspace: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct BuildSource {
    executable: PathBuf,
    version: String,
    profile: Option<String>,
    marker_instances: Vec<(u32, String)>,
    supervisor_pids: Vec<u32>,
    app_process_pids: Vec<u32>,
}

fn process_profiles(processes: &[ProcessInfo]) -> HashMap<u32, Option<String>> {
    let pids = processes
        .iter()
        .map(|process| sysinfo::Pid::from_u32(process.pid))
        .collect::<Vec<_>>();
    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::Some(&pids),
        false,
        ProcessRefreshKind::nothing().with_environ(UpdateKind::Always),
    );
    processes
        .iter()
        .map(|process| {
            let profile = system
                .process(sysinfo::Pid::from_u32(process.pid))
                .and_then(|process| {
                    process.environ().iter().find_map(|entry| {
                        let entry = entry.to_string_lossy();
                        let (name, value) = entry.split_once('=')?;
                        (name.eq_ignore_ascii_case("WARPSPEED_PROFILE") && !value.is_empty())
                            .then(|| value.to_owned())
                    })
                });
            (process.pid, profile)
        })
        .collect()
}

pub(crate) fn collect() -> SystemOverview {
    let processes = process_table();
    let markers = address::read_markers();
    let current_executable = std::env::current_exe().ok();
    let profiles = process_profiles(&processes);
    let sources = build_sources(
        &processes,
        &markers,
        current_executable.as_deref(),
        &profiles,
    );
    let handles = sources
        .into_iter()
        .map(|source| std::thread::spawn(move || collect_build(source)))
        .collect::<Vec<_>>();
    let mut builds = handles
        .into_iter()
        .filter_map(|handle| handle.join().ok())
        .collect::<Vec<_>>();
    builds.sort_by(|left, right| {
        right
            .version
            .cmp(&left.version)
            .then_with(|| left.profile.cmp(&right.profile))
            .then_with(|| left.executable.cmp(&right.executable))
    });
    SystemOverview { builds }
}

fn build_sources(
    processes: &[ProcessInfo],
    markers: &[InstanceMarker],
    current_executable: Option<&Path>,
    profiles: &HashMap<u32, Option<String>>,
) -> Vec<BuildSource> {
    let by_pid = processes
        .iter()
        .map(|process| (process.pid, process))
        .collect::<BTreeMap<_, _>>();
    let mut grouped = BTreeMap::<(String, Option<String>), BuildSource>::new();
    for process in processes
        .iter()
        .filter(|process| is_warp_process(process, current_executable))
    {
        let Some(executable) = &process.executable else {
            continue;
        };
        let profile = profiles.get(&process.pid).cloned().flatten();
        let key = (path_key(executable), profile.clone());
        let source = grouped.entry(key).or_insert_with(|| BuildSource {
            version: build_name(executable),
            executable: executable.clone(),
            profile,
            marker_instances: Vec::new(),
            supervisor_pids: Vec::new(),
            app_process_pids: Vec::new(),
        });
        if is_supervisor(process) {
            source.supervisor_pids.push(process.pid);
        } else {
            source.app_process_pids.push(process.pid);
        }
    }
    if let Some(executable) = current_executable {
        let profile = std::env::var("WARPSPEED_PROFILE")
            .ok()
            .filter(|profile| !profile.is_empty());
        grouped
            .entry((path_key(executable), profile.clone()))
            .or_insert_with(|| BuildSource {
                version: build_name(executable),
                executable: executable.to_path_buf(),
                profile,
                marker_instances: Vec::new(),
                supervisor_pids: Vec::new(),
                app_process_pids: Vec::new(),
            });
    }
    for marker in markers {
        let Some(process) = by_pid
            .get(&marker.pid)
            .filter(|process| is_warp_process(process, current_executable))
        else {
            continue;
        };
        let Some(executable) = &process.executable else {
            continue;
        };
        let profile = profiles.get(&process.pid).cloned().flatten();
        let Some(source) = grouped.get_mut(&(path_key(executable), profile)) else {
            continue;
        };
        source.version = marker.version.clone();
        source
            .marker_instances
            .push((marker.pid, marker.version.clone()));
    }
    for source in grouped.values_mut() {
        source.marker_instances.sort_by_key(|(pid, _)| *pid);
        source.marker_instances.dedup_by_key(|(pid, _)| *pid);
        source.supervisor_pids.sort_unstable();
        source.supervisor_pids.dedup();
        source.app_process_pids.sort_unstable();
        source.app_process_pids.dedup();
    }
    grouped.into_values().collect()
}

fn collect_build(source: BuildSource) -> BuildOverview {
    let discovered_instances = run_json::<Vec<InstanceReport>>(
        &source.executable,
        source.profile.as_deref(),
        &["sessions", "instances", "--json"],
    )
    .unwrap_or_default();
    let mut instance_versions = source
        .marker_instances
        .iter()
        .cloned()
        .collect::<BTreeMap<_, _>>();
    for instance in discovered_instances {
        if source.app_process_pids.contains(&instance.pid) {
            instance_versions.insert(instance.pid, instance.version);
        }
    }
    let instances = instance_versions
        .iter()
        .map(|(pid, version)| {
            collect_instance(
                &source.executable,
                source.profile.as_deref(),
                *pid,
                version.clone(),
            )
        })
        .collect::<Vec<_>>();
    let other_process_pids = source
        .app_process_pids
        .iter()
        .copied()
        .filter(|pid| !instance_versions.contains_key(pid))
        .collect();
    let (supervisor, supervisor_error) =
        collect_supervisor(&source.executable, source.profile.as_deref());
    let version = instance_versions
        .values()
        .next()
        .cloned()
        .unwrap_or(source.version);
    BuildOverview {
        version,
        profile: source.profile,
        executable: source.executable,
        instances,
        supervisor_pids: source.supervisor_pids,
        supervisor,
        supervisor_error,
        other_process_pids,
    }
}

fn collect_instance(
    executable: &Path,
    profile: Option<&str>,
    pid: u32,
    version: String,
) -> InstanceOverview {
    let pid_text = pid.to_string();
    let window_report = run_json::<Vec<WindowReport>>(
        executable,
        profile,
        &["sessions", "windows", "--instance", &pid_text, "--json"],
    )
    .map(|windows| WindowListReport { windows })
    .or_else(|_| {
        run_json::<WindowListReport>(
            executable,
            profile,
            &[
                "--warpctrl",
                "--output-format",
                "json",
                "window",
                "list",
                "--pid",
                &pid_text,
            ],
        )
    })
    .ok();
    match run_json::<Vec<PaneReport>>(
        executable,
        profile,
        &["sessions", "panes", "--instance", &pid_text, "--json"],
    ) {
        Ok(panes) => InstanceOverview {
            pid,
            version,
            windows: group_panes(panes, window_report),
            error: None,
        },
        Err(error) => InstanceOverview {
            pid,
            version,
            windows: window_report
                .map_or_else(Vec::new, |report| group_panes(Vec::new(), Some(report))),
            error: Some(error),
        },
    }
}

fn collect_supervisor(
    executable: &Path,
    profile: Option<&str>,
) -> (Option<SupervisorOverview>, Option<String>) {
    let status = match run_json::<SupervisorStatus>(
        executable,
        profile,
        &["sessions", "supervisor", "--json"],
    ) {
        Ok(status) => status,
        Err(error) => return (None, Some(error)),
    };
    if !status.reachable {
        return (None, None);
    }
    match run_json::<Vec<SupervisorSessionOverview>>(
        executable,
        profile,
        &["sessions", "list", "--json"],
    ) {
        Ok(mut sessions) => {
            sessions.sort_by_key(|session| (session.attached, session.session_id));
            let protocol_version = protocol_version(&status.channel);
            (
                Some(SupervisorOverview {
                    channel: status.channel,
                    protocol_version,
                    state_directory: status.state_directory,
                    sessions,
                }),
                None,
            )
        }
        Err(error) => (None, Some(error)),
    }
}

fn group_panes(
    panes: Vec<PaneReport>,
    window_report: Option<WindowListReport>,
) -> Vec<WindowOverview> {
    let mut windows = BTreeMap::<String, WindowOverview>::new();
    if let Some(report) = window_report {
        for window in report.windows {
            windows.insert(
                window.window_id.clone(),
                WindowOverview {
                    id: window.window_id,
                    index: Some(window.index),
                    is_active: window.is_active,
                    has_workspace: window.has_workspace,
                    panes: Vec::new(),
                },
            );
        }
    }
    for pane in panes {
        windows
            .entry(pane.window.clone())
            .or_insert_with(|| WindowOverview {
                id: pane.window,
                index: None,
                is_active: false,
                has_workspace: true,
                panes: Vec::new(),
            })
            .panes
            .push(PaneOverview {
                terminal_view_id: pane.terminal_view_id,
                session_id: pane.session_id,
                tab_index: pane.tab_index,
                title: pane.title,
                shell: pane.shell,
                working_directory: pane.working_directory,
                has_running_agent: pane.has_running_agent,
                is_busy: pane.is_busy,
                survives_quit: pane.survives_quit,
                agent: pane.agent,
                agent_session_id: pane.agent_session_id,
                agent_not_resumed: pane.agent_not_resumed,
            });
    }
    let mut windows = windows.into_values().collect::<Vec<_>>();
    for window in &mut windows {
        window.panes.sort_by_key(|pane| pane.tab_index);
    }
    windows.sort_by(|left, right| {
        left.index
            .cmp(&right.index)
            .then_with(|| left.id.cmp(&right.id))
    });
    windows
}

fn run_json<T: serde::de::DeserializeOwned>(
    executable: &Path,
    profile: Option<&str>,
    args: &[&str],
) -> Result<T, String> {
    let mut command = Command::new(executable);
    command.args(args).env_remove("WARPSPEED_PROFILE");
    if let Some(profile) = profile {
        command.env("WARPSPEED_PROFILE", profile);
    }
    let output = command
        .output()
        .map_err(|error| format!("Could not start {}: {error}", executable.display()))?;
    if !output.status.success() {
        let error = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        return Err(if error.is_empty() {
            format!("{} exited with {}", executable.display(), output.status)
        } else {
            error
        });
    }
    parse_json_output(&output.stdout).map_err(|error| {
        format!(
            "{} returned invalid system information: {error}",
            executable.display()
        )
    })
}

fn parse_json_output<T: serde::de::DeserializeOwned>(
    output: &[u8],
) -> Result<T, serde_json::Error> {
    let mut last_error = serde_json::from_slice::<T>(output).err();
    for index in output
        .iter()
        .enumerate()
        .filter_map(|(index, byte)| matches!(byte, b'{' | b'[').then_some(index))
    {
        let mut deserializer = serde_json::Deserializer::from_slice(&output[index..]);
        match T::deserialize(&mut deserializer) {
            Ok(value) => return Ok(value),
            Err(error) => last_error = Some(error),
        }
    }
    Err(
        last_error.unwrap_or_else(|| match serde_json::from_slice::<serde_json::Value>(b"") {
            Ok(_) => unreachable!("empty JSON cannot contain a value"),
            Err(error) => error,
        }),
    )
}

fn is_warp_process(process: &ProcessInfo, current_executable: Option<&Path>) -> bool {
    if is_supervisor(process) {
        return true;
    }
    let Some(executable) = process.executable.as_deref() else {
        return false;
    };
    if current_executable.is_some_and(|current| path_key(current) == path_key(executable)) {
        return true;
    }
    let name = executable
        .file_stem()
        .map(|name| name.to_string_lossy().to_ascii_lowercase());
    let current_name = current_executable.and_then(|current| {
        current
            .file_stem()
            .map(|name| name.to_string_lossy().to_ascii_lowercase())
    });
    name == current_name
        || matches!(
            name.as_deref(),
            Some("warp" | "warpspeed" | "warposs" | "warp-preview")
        )
}

fn is_supervisor(process: &ProcessInfo) -> bool {
    process
        .command
        .iter()
        .any(|argument| argument == SUPERVISOR_ARGUMENT)
}

fn path_key(path: &Path) -> String {
    let key = path.to_string_lossy();
    #[cfg(windows)]
    {
        key.to_ascii_lowercase()
    }
    #[cfg(not(windows))]
    {
        key.into_owned()
    }
}

fn build_name(executable: &Path) -> String {
    let normalized = executable.to_string_lossy().replace('\\', "/");
    let parts = normalized.split('/').collect::<Vec<_>>();
    if let Some(index) = parts.iter().position(|part| *part == "versions")
        && let Some(build) = parts.get(index + 1).filter(|build| !build.is_empty())
    {
        return (*build).to_owned();
    }
    executable
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "unknown".to_owned())
}

fn protocol_version(channel: &str) -> Option<u32> {
    let marker = if let Some((_, suffix)) = channel.rsplit_once("supervisor-v") {
        suffix
    } else {
        channel.rsplit(['/', '\\']).next()?.strip_prefix('v')?
    };
    marker
        .chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .ok()
}

#[cfg(test)]
#[path = "system_overview_tests.rs"]
mod tests;
