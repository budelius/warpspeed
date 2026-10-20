//! One row per known agent conversation or otherwise unplaced live process.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use warp_core::SessionId;

use crate::app_state::{CliAgentResumeDescriptor, SessionHolder};
use crate::terminal::agent_session_history::{
    AgentSessionPreviewMessage, PastAgentSession, ResolvedPath, is_within_known,
};
use crate::terminal::cli_agent::CLIAgent;
use crate::terminal::cli_agent_sessions::agent_process_scan::{Placement, SavedPane, ScannedAgent};
use crate::terminal::terminal_manager::HeldAgent;
use crate::workspace::PaneViewLocator;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum AgentSessionState {
    Running {
        locator: PaneViewLocator,
    },
    RunningInOtherInstance {
        instance_pid: u32,
    },
    Pending {
        locator: PaneViewLocator,
    },
    Detached {
        session_id: SessionId,
        instance_pid: Option<u32>,
        holder: Option<SessionHolder>,
    },
    KnownProcess {
        pid: u32,
    },
    HeldByOtherBuild {
        pid: u32,
        build: String,
        saved_in: Option<SavedPane>,
    },
    NoKnownPane {
        pid: u32,
        saved_in: Option<SavedPane>,
    },
    Past,
}

impl AgentSessionState {
    /// How a row names its state.
    pub(crate) fn label(&self) -> &'static str {
        match self {
            AgentSessionState::Running { .. } => "● running",
            AgentSessionState::RunningInOtherInstance { .. } => "● another Warp instance",
            AgentSessionState::Pending { .. } => "○ pending",
            AgentSessionState::Detached {
                instance_pid: Some(_),
                ..
            } => "detached from another Warp instance",
            AgentSessionState::Detached {
                instance_pid: None, ..
            } => "detached",
            AgentSessionState::KnownProcess { .. } => "this build",
            AgentSessionState::HeldByOtherBuild { .. } => "held by another build",
            AgentSessionState::NoKnownPane { .. } => "no pane",
            AgentSessionState::Past => "past",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AgentSessionRow {
    /// `None` for a detached shell whose agent is unknown.
    pub agent: Option<CLIAgent>,
    pub session_id: Option<String>,
    pub title: Option<String>,
    pub last_prompt: Option<String>,
    pub cwd: PathBuf,
    /// `cwd` as the background scan resolved it, for a past conversation; `None` otherwise.
    pub canonical_cwd: Option<PathBuf>,
    pub last_used: Option<SystemTime>,
    pub preview: Vec<AgentSessionPreviewMessage>,
    pub process_count: usize,
    pub supervisor: Option<SessionHolder>,
    pub state: AgentSessionState,
}

/// Whether `row`'s conversation lies in `folder` or beneath it, by the paths the background scan
/// resolved, so it touches no filesystem however many rows are checked.
pub(crate) fn is_row_within(row: &AgentSessionRow, folder: &ResolvedPath) -> bool {
    is_within_known(&row.cwd, row.canonical_cwd.as_deref(), folder)
}

/// One terminal pane of the window, in tab order, as the window view groups it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PaneAgentInput {
    pub locator: Option<PaneViewLocator>,
    pub remote_instance_pid: Option<u32>,
    pub tab_index: usize,
    pub tab_title: String,
    /// 1-based position among the tab's visible panes, as laid out.
    pub pane_number: usize,
    pub cwd: Option<PathBuf>,
    pub session_uuid_hex: Option<String>,
    pub supervisor_session_id: Option<SessionId>,
    pub session_holder: Option<SessionHolder>,
    pub held_agent: Option<HeldAgent>,
    pub latest_user_prompt: Option<String>,
    /// From `CLIAgentSessionsModel`.
    pub running: Option<CliAgentResumeDescriptor>,
    /// Restored, not yet running.
    pub pending: Option<CliAgentResumeDescriptor>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DetachedInput {
    pub session_id: SessionId,
    pub remote_instance_pid: Option<u32>,
    pub title: String,
    pub cwd: PathBuf,
    pub detached_for_seconds: u64,
    pub session_uuid_hex: String,
    pub holder: Option<SessionHolder>,
}

type ConversationKey = (CLIAgent, String);

fn key_of(descriptor: &CliAgentResumeDescriptor) -> Option<ConversationKey> {
    Some((descriptor.agent, descriptor.session_id.clone()?))
}

fn row_key(row: &AgentSessionRow) -> Option<ConversationKey> {
    Some((row.agent?, row.session_id.clone()?))
}

fn pane_holds_detached(pane: &PaneAgentInput, detached: &DetachedInput) -> bool {
    pane.supervisor_session_id == Some(detached.session_id)
        && detached.holder.as_ref().is_none_or(|holder| {
            pane.session_holder
                .as_ref()
                .is_some_and(|pane_holder| pane_holder.channel == holder.channel)
        })
}

fn held_descriptor(pane: &PaneAgentInput) -> Option<CliAgentResumeDescriptor> {
    let held = pane.held_agent.as_ref()?;
    Some(CliAgentResumeDescriptor {
        agent: held.agent,
        session_id: Some(held.session_id.clone()),
        cwd: pane.cwd.clone().unwrap_or_default(),
        title: None,
        captured_unix_seconds: None,
    })
}

fn live_row(
    pane: &PaneAgentInput,
    descriptor: &CliAgentResumeDescriptor,
    history: Option<&PastAgentSession>,
    state: AgentSessionState,
) -> AgentSessionRow {
    let cwd = if descriptor.cwd.as_os_str().is_empty() {
        pane.cwd.clone().unwrap_or_default()
    } else {
        descriptor.cwd.clone()
    };
    AgentSessionRow {
        agent: Some(descriptor.agent),
        session_id: descriptor.session_id.clone(),
        title: descriptor
            .title
            .clone()
            .or_else(|| history.and_then(|session| session.title.clone())),
        last_prompt: pane
            .latest_user_prompt
            .clone()
            .or_else(|| history.and_then(|session| session.last_prompt.clone())),
        cwd,
        canonical_cwd: None,
        last_used: descriptor
            .captured_unix_seconds
            .and_then(|seconds| u64::try_from(seconds).ok())
            .map(|seconds| SystemTime::UNIX_EPOCH + Duration::from_secs(seconds))
            .or_else(|| history.map(|session| session.modified)),
        preview: history
            .map(|session| session.preview.clone())
            .unwrap_or_default(),
        process_count: 0,
        supervisor: pane.session_holder.clone(),
        state,
    }
}

fn past_row(past: &PastAgentSession, state: AgentSessionState) -> AgentSessionRow {
    AgentSessionRow {
        agent: Some(past.agent),
        session_id: Some(past.session_id.clone()),
        title: past.title.clone(),
        last_prompt: past.last_prompt.clone(),
        cwd: past.cwd.clone(),
        canonical_cwd: past.canonical_cwd.clone(),
        last_used: Some(past.modified),
        preview: past.preview.clone(),
        process_count: 0,
        supervisor: None,
        state,
    }
}

fn scanned_row(scanned: &ScannedAgent, history: Option<&PastAgentSession>) -> AgentSessionRow {
    let state = match &scanned.placement {
        Placement::Known => AgentSessionState::KnownProcess { pid: scanned.pid },
        Placement::HeldByOtherBuild {
            build, saved_in, ..
        } => AgentSessionState::HeldByOtherBuild {
            pid: scanned.pid,
            build: build.clone(),
            saved_in: saved_in.clone(),
        },
        Placement::NoKnownPane { saved_in } => AgentSessionState::NoKnownPane {
            pid: scanned.pid,
            saved_in: saved_in.clone(),
        },
    };
    let mut row = history.map_or_else(
        || AgentSessionRow {
            agent: Some(scanned.agent),
            session_id: Some(scanned.session_id.clone()),
            title: None,
            last_prompt: None,
            cwd: scanned.cwd.clone().unwrap_or_default(),
            canonical_cwd: None,
            last_used: None,
            preview: Vec::new(),
            process_count: scanned.process_count,
            supervisor: None,
            state: state.clone(),
        },
        |history| past_row(history, state.clone()),
    );
    if let Some(cwd) = &scanned.cwd {
        row.cwd = cwd.clone();
        row.canonical_cwd = None;
    }
    if row.last_used.is_none() && scanned.started_at_unix_seconds > 0 {
        row.last_used =
            Some(SystemTime::UNIX_EPOCH + Duration::from_secs(scanned.started_at_unix_seconds));
    }
    row.process_count = scanned.process_count;
    row
}

/// Merges pane, supervisor, and history sources without a machine-wide process scan.
#[cfg(test)]
pub(crate) fn build_rows(
    panes: &[PaneAgentInput],
    detached: &[DetachedInput],
    past: &[PastAgentSession],
) -> Vec<AgentSessionRow> {
    build_rows_with_scanned(panes, detached, past, &[])
}

/// Merges pane, supervisor, process, and history sources. Panes are expected in tab order.
pub(crate) fn build_rows_with_scanned(
    panes: &[PaneAgentInput],
    detached: &[DetachedInput],
    past: &[PastAgentSession],
    scanned: &[ScannedAgent],
) -> Vec<AgentSessionRow> {
    let running_keys: HashSet<ConversationKey> = panes
        .iter()
        .filter_map(|pane| pane.running.as_ref().and_then(key_of))
        .collect();
    let local_running_keys: HashSet<ConversationKey> = panes
        .iter()
        .filter(|pane| pane.remote_instance_pid.is_none())
        .filter_map(|pane| pane.running.as_ref().and_then(key_of))
        .collect();

    let mut detached_order: Vec<&DetachedInput> = detached.iter().collect();
    detached_order.sort_by_key(|input| input.detached_for_seconds);

    let mut pasts: Vec<&PastAgentSession> = past.iter().collect();
    pasts.sort_by_key(|session| std::cmp::Reverse(session.modified));
    let history_for_key = |key: &ConversationKey| {
        pasts
            .iter()
            .copied()
            .find(|session| session.agent == key.0 && session.session_id == key.1)
    };
    let history_for = |descriptor: &CliAgentResumeDescriptor| history_for_key(&key_of(descriptor)?);

    let mut rows = Vec::new();
    let mut live_keys = running_keys.clone();
    let mut represented_remote_keys = HashSet::new();
    for pane in panes {
        if let Some(descriptor) = &pane.running {
            if pane.remote_instance_pid.is_some()
                && let Some(key) = key_of(descriptor)
                && (local_running_keys.contains(&key) || !represented_remote_keys.insert(key))
            {
                continue;
            }
            let state = match (pane.remote_instance_pid, pane.locator) {
                (Some(instance_pid), _) => {
                    AgentSessionState::RunningInOtherInstance { instance_pid }
                }
                (None, Some(locator)) => AgentSessionState::Running { locator },
                (None, None) => continue,
            };
            rows.push(live_row(pane, descriptor, history_for(descriptor), state));
        } else if let Some(descriptor) = &pane.pending {
            let key = key_of(descriptor);
            if detached
                .iter()
                .any(|input| pane_holds_detached(pane, input))
                || key.as_ref().is_some_and(|key| live_keys.contains(key))
            {
                continue;
            }
            live_keys.extend(key);
            let Some(locator) = pane.locator else {
                continue;
            };
            rows.push(live_row(
                pane,
                descriptor,
                history_for(descriptor),
                AgentSessionState::Pending { locator },
            ));
        }
    }

    let mut detached_keys = HashSet::new();
    for input in detached_order {
        let state = AgentSessionState::Detached {
            session_id: input.session_id,
            instance_pid: input.remote_instance_pid,
            holder: input.holder.clone(),
        };
        let pane = panes.iter().find(|pane| pane_holds_detached(pane, input));
        let pane_descriptor = pane.and_then(|pane| {
            pane.pending
                .clone()
                .or_else(|| held_descriptor(pane))
                .map(|descriptor| (pane, descriptor))
        });
        if pane_descriptor
            .as_ref()
            .and_then(|(_, descriptor)| key_of(descriptor))
            .is_some_and(|key| running_keys.contains(&key))
        {
            continue;
        }
        let identified = (!input.session_uuid_hex.is_empty())
            .then(|| {
                scanned.iter().find(|process| {
                    process.warp_session_uuid.as_deref() == Some(input.session_uuid_hex.as_str())
                })
            })
            .flatten();
        let row = match (pane_descriptor, identified) {
            (Some((pane, descriptor)), _) => {
                if let Some(key) = key_of(&descriptor) {
                    detached_keys.insert(key);
                }
                let mut row = live_row(pane, &descriptor, history_for(&descriptor), state.clone());
                row.title = row
                    .title
                    .or_else(|| Some(input.title.clone()).filter(|title| !title.is_empty()));
                if row.cwd.as_os_str().is_empty() {
                    row.cwd = input.cwd.clone();
                }
                row
            }
            (None, Some(process)) => {
                let key = (process.agent, process.session_id.clone());
                detached_keys.insert(key.clone());
                let mut row = history_for_key(&key).map_or_else(
                    || AgentSessionRow {
                        agent: Some(process.agent),
                        session_id: Some(process.session_id.clone()),
                        title: None,
                        last_prompt: None,
                        cwd: PathBuf::new(),
                        canonical_cwd: None,
                        last_used: None,
                        preview: Vec::new(),
                        process_count: process.process_count,
                        supervisor: input.holder.clone(),
                        state: state.clone(),
                    },
                    |history| past_row(history, state.clone()),
                );
                row.title = row
                    .title
                    .or_else(|| Some(input.title.clone()).filter(|title| !title.is_empty()));
                row.cwd = process
                    .cwd
                    .clone()
                    .filter(|cwd| !cwd.as_os_str().is_empty())
                    .unwrap_or_else(|| input.cwd.clone());
                row.canonical_cwd = None;
                row.process_count = process.process_count;
                row.supervisor = input.holder.clone();
                row
            }
            (None, None) => AgentSessionRow {
                agent: None,
                session_id: None,
                title: Some(input.title.clone()).filter(|title| !title.is_empty()),
                last_prompt: None,
                cwd: input.cwd.clone(),
                canonical_cwd: None,
                last_used: None,
                preview: Vec::new(),
                process_count: 0,
                supervisor: input.holder.clone(),
                state,
            },
        };
        rows.push(row);
    }

    let process_counts = scanned.iter().fold(HashMap::new(), |mut counts, process| {
        counts.insert(
            (process.agent, process.session_id.clone()),
            process.process_count,
        );
        counts
    });
    for row in &mut rows {
        if let Some(key) = row_key(row) {
            row.process_count = process_counts.get(&key).copied().unwrap_or_default();
        }
    }

    let scanned_keys = scanned
        .iter()
        .map(|process| (process.agent, process.session_id.clone()))
        .collect::<HashSet<_>>();
    let represented_pids = scanned
        .iter()
        .filter(|process| {
            let Some(uuid) = process.warp_session_uuid.as_deref() else {
                return false;
            };
            let key = (process.agent, process.session_id.clone());
            panes.iter().any(|pane| {
                pane.session_uuid_hex.as_deref() == Some(uuid)
                    && pane.running.as_ref().and_then(key_of) == Some(key.clone())
            }) || detached
                .iter()
                .any(|session| session.session_uuid_hex == uuid)
        })
        .map(|process| process.pid)
        .collect::<HashSet<_>>();
    for process in scanned {
        let key = (process.agent, process.session_id.clone());
        let represented_by_remote_pane = panes.iter().any(|pane| {
            pane.remote_instance_pid.is_some()
                && pane.running.as_ref().and_then(key_of) == Some(key.clone())
        });
        let uniquely_represented = process.process_count == 1
            && (represented_by_remote_pane
                || (process.warp_session_uuid.is_none()
                    && (running_keys.contains(&key) || detached_keys.contains(&key))));
        if represented_pids.contains(&process.pid) || uniquely_represented {
            continue;
        }
        rows.push(scanned_row(process, history_for_key(&key)));
    }

    for session in pasts {
        let key = (session.agent, session.session_id.clone());
        if live_keys.contains(&key) || detached_keys.contains(&key) || scanned_keys.contains(&key) {
            continue;
        }
        rows.push(past_row(session, AgentSessionState::Past));
    }
    rows
}

#[cfg(test)]
#[path = "agent_sessions_index_tests.rs"]
mod tests;
