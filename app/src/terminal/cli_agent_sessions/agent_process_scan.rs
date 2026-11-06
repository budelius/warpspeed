//! Finds every registered agent process on the machine and describes where its terminal went.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::terminal::CLIAgent;

const SUPERVISOR_ARGUMENT: &str = "--terminal-session-supervisor";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SavedPane {
    pub session_name: String,
    pub tab_title: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Placement {
    Known,
    HeldByOtherBuild {
        build: String,
        supervisor_pid: u32,
        saved_in: Option<SavedPane>,
    },
    NoKnownPane {
        saved_in: Option<SavedPane>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ScannedAgent {
    pub agent: CLIAgent,
    pub session_id: String,
    pub cwd: Option<PathBuf>,
    pub pid: u32,
    pub started_at_unix_seconds: u64,
    pub warp_session_uuid: Option<String>,
    pub placement: Placement,
    pub process_count: usize,
}

#[cfg(not(target_family = "wasm"))]
pub(crate) fn scan() -> Vec<ScannedAgent> {
    use super::process_registry::{process_table, registered_agent_processes_in};

    let processes = process_table();
    let registered = registered_agent_processes_in(&processes);
    let current_executable = std::env::current_exe().unwrap_or_default();
    let scanned = scan_in(&registered, &processes, &current_executable, &saved_panes());
    log::debug!("Agent process scan found {} live processes", scanned.len());
    for process in &scanned {
        let placement = match &process.placement {
            Placement::Known => "known",
            Placement::HeldByOtherBuild { .. } => "held_by_other_build",
            Placement::NoKnownPane { .. } => "no_known_pane",
        };
        log::debug!(
            "Agent process provenance: agent={} pid={} session={} placement={placement}",
            process.agent.display_name(),
            process.pid,
            process.session_id,
        );
    }
    scanned
}

#[cfg(target_family = "wasm")]
pub(crate) fn scan() -> Vec<ScannedAgent> {
    Vec::new()
}

#[cfg(not(target_family = "wasm"))]
fn saved_panes() -> HashMap<String, SavedPane> {
    let mut panes = HashMap::new();
    for session in crate::launch_configs::session_store::list_summaries() {
        for window in session.windows() {
            for tab in window.tabs() {
                for pane in &tab.panes {
                    panes
                        .entry(pane.pane_uuid.to_ascii_lowercase())
                        .or_insert_with(|| SavedPane {
                            session_name: session.name().to_owned(),
                            tab_title: tab
                                .title
                                .clone()
                                .unwrap_or_else(|| format!("Tab {}", tab.tab_index + 1)),
                        });
                }
            }
        }
    }
    panes
}

#[cfg(not(target_family = "wasm"))]
fn scan_in(
    registered: &[super::process_registry::RegisteredAgentProcess],
    processes: &[super::process_registry::ProcessInfo],
    current_executable: &Path,
    saved_panes: &HashMap<String, SavedPane>,
) -> Vec<ScannedAgent> {
    let by_pid = processes
        .iter()
        .map(|process| (process.pid, process))
        .collect::<HashMap<_, _>>();
    let supervisors = processes
        .iter()
        .filter(|process| is_supervisor(process))
        .collect::<Vec<_>>();
    let other_supervisors = supervisors
        .iter()
        .copied()
        .filter(|process| !same_executable(process, current_executable))
        .collect::<Vec<_>>();
    let counts = registered
        .iter()
        .fold(HashMap::new(), |mut counts, process| {
            *counts
                .entry((process.agent, process.session_id.as_str()))
                .or_insert(0) += 1;
            counts
        });

    let mut scanned = registered
        .iter()
        .map(|process| {
            let saved_in = process
                .warp_session_uuid
                .as_deref()
                .and_then(|uuid| saved_panes.get(&uuid.to_ascii_lowercase()))
                .cloned();
            let supervisor = supervisor_ancestor(process.pid, &by_pid);
            let placement = match supervisor {
                Some(supervisor) if same_executable(supervisor, current_executable) => {
                    Placement::Known
                }
                Some(supervisor) => Placement::HeldByOtherBuild {
                    build: build_name(supervisor),
                    supervisor_pid: supervisor.pid,
                    saved_in,
                },
                None if saved_in.is_some() && other_supervisors.len() == 1 => {
                    let supervisor = other_supervisors[0];
                    Placement::HeldByOtherBuild {
                        build: build_name(supervisor),
                        supervisor_pid: supervisor.pid,
                        saved_in,
                    }
                }
                None => Placement::NoKnownPane { saved_in },
            };
            ScannedAgent {
                agent: process.agent,
                session_id: process.session_id.clone(),
                cwd: process.cwd.clone(),
                pid: process.pid,
                started_at_unix_seconds: process.started_at_unix_seconds,
                warp_session_uuid: process.warp_session_uuid.clone(),
                placement,
                process_count: counts[&(process.agent, process.session_id.as_str())],
            }
        })
        .collect::<Vec<_>>();
    scanned.sort_by(|left, right| {
        left.agent
            .display_name()
            .cmp(right.agent.display_name())
            .then_with(|| left.session_id.cmp(&right.session_id))
            .then_with(|| left.pid.cmp(&right.pid))
    });
    scanned
}

#[cfg(not(target_family = "wasm"))]
fn is_supervisor(process: &super::process_registry::ProcessInfo) -> bool {
    process.command.iter().any(|arg| arg == SUPERVISOR_ARGUMENT)
}

#[cfg(not(target_family = "wasm"))]
fn supervisor_ancestor<'a>(
    pid: u32,
    processes: &HashMap<u32, &'a super::process_registry::ProcessInfo>,
) -> Option<&'a super::process_registry::ProcessInfo> {
    let mut parent = processes.get(&pid).and_then(|process| process.parent);
    let mut seen = HashSet::new();
    while let Some(pid) = parent.filter(|pid| seen.insert(*pid)) {
        let process = processes.get(&pid)?;
        if is_supervisor(process) {
            return Some(process);
        }
        parent = process.parent;
    }
    None
}

#[cfg(not(target_family = "wasm"))]
fn same_executable(
    process: &super::process_registry::ProcessInfo,
    current_executable: &Path,
) -> bool {
    process.executable.as_deref().is_some_and(|executable| {
        #[cfg(windows)]
        {
            executable
                .to_string_lossy()
                .eq_ignore_ascii_case(&current_executable.to_string_lossy())
        }
        #[cfg(not(windows))]
        {
            executable == current_executable
        }
    })
}

#[cfg(not(target_family = "wasm"))]
fn build_name(process: &super::process_registry::ProcessInfo) -> String {
    let executable = process
        .executable
        .as_deref()
        .unwrap_or_else(|| Path::new(&process.name));
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
        .unwrap_or_else(|| process.name.clone())
}

#[cfg(all(test, not(target_family = "wasm")))]
#[path = "agent_process_scan_tests.rs"]
mod tests;
