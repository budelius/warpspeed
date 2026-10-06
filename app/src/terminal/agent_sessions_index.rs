//! One row per agent conversation, in the strongest state any source knows it in.

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use warp_core::SessionId;

use crate::app_state::CliAgentResumeDescriptor;
use crate::terminal::agent_session_history::{PastAgentSession, is_within};
use crate::terminal::cli_agent::CLIAgent;
use crate::workspace::PaneViewLocator;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum AgentSessionState {
    Running { locator: PaneViewLocator },
    Pending { locator: PaneViewLocator },
    Detached { session_id: SessionId },
    Past,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AgentSessionRow {
    /// `None` for a detached shell whose agent is unknown.
    pub agent: Option<CLIAgent>,
    pub session_id: Option<String>,
    pub title: Option<String>,
    pub cwd: PathBuf,
    pub last_used: Option<SystemTime>,
    pub state: AgentSessionState,
}

/// One terminal pane of the window, in tab order, as the window view groups it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PaneAgentInput {
    pub locator: PaneViewLocator,
    pub tab_index: usize,
    pub tab_title: String,
    /// 1-based position among the tab's visible panes, as laid out.
    pub pane_number: usize,
    pub cwd: Option<PathBuf>,
    /// From `CLIAgentSessionsModel`.
    pub running: Option<CliAgentResumeDescriptor>,
    /// Restored, not yet running.
    pub pending: Option<CliAgentResumeDescriptor>,
}

pub(crate) struct DetachedInput {
    pub session_id: SessionId,
    pub title: String,
    pub cwd: PathBuf,
    pub detached_for_seconds: u64,
}

type ConversationKey = (CLIAgent, String);

fn key_of(descriptor: &CliAgentResumeDescriptor) -> Option<ConversationKey> {
    Some((descriptor.agent, descriptor.session_id.clone()?))
}

fn live_row(
    pane: &PaneAgentInput,
    descriptor: &CliAgentResumeDescriptor,
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
        title: descriptor.title.clone(),
        cwd,
        last_used: descriptor
            .captured_unix_seconds
            .and_then(|seconds| u64::try_from(seconds).ok())
            .map(|seconds| SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)),
        state,
    }
}

fn past_row(past: &PastAgentSession, state: AgentSessionState) -> AgentSessionRow {
    AgentSessionRow {
        agent: Some(past.agent),
        session_id: Some(past.session_id.clone()),
        title: past.title.clone(),
        cwd: past.cwd.clone(),
        last_used: Some(past.modified),
        state,
    }
}

/// Whether a past session is plausibly the conversation a detached shell holds. A detached shell
/// records no agent, so the title and a shared directory are all there is to go on.
fn is_detached_conversation(past: &PastAgentSession, detached: &DetachedInput) -> bool {
    !detached.title.is_empty()
        && !detached.cwd.as_os_str().is_empty()
        && past.title.as_deref() == Some(detached.title.as_str())
        && is_within(&past.cwd, &detached.cwd)
}

/// Merges every source into one row per conversation, each in its strongest state
/// (`Running` > `Detached` > `Pending` > `Past`). Panes are expected in tab order.
pub(crate) fn build_rows(
    panes: &[PaneAgentInput],
    detached: &[DetachedInput],
    past: &[PastAgentSession],
) -> Vec<AgentSessionRow> {
    let running_keys: HashSet<ConversationKey> = panes
        .iter()
        .filter_map(|pane| pane.running.as_ref().and_then(key_of))
        .collect();

    let mut detached_order: Vec<&DetachedInput> = detached.iter().collect();
    detached_order.sort_by_key(|input| input.detached_for_seconds);

    let mut pasts: Vec<&PastAgentSession> = past.iter().collect();
    pasts.sort_by_key(|session| std::cmp::Reverse(session.modified));

    let mut merged_pasts = HashSet::new();
    let mut detached_rows = Vec::new();
    let mut detached_keys = HashSet::new();
    for input in detached_order {
        let conversation = pasts.iter().enumerate().find(|(index, session)| {
            !merged_pasts.contains(index)
                && !running_keys.contains(&(session.agent, session.session_id.clone()))
                && is_detached_conversation(session, input)
        });
        let state = AgentSessionState::Detached {
            session_id: input.session_id,
        };
        let row = match conversation {
            Some((index, session)) => {
                merged_pasts.insert(index);
                detached_keys.insert((session.agent, session.session_id.clone()));
                AgentSessionRow {
                    title: Some(input.title.clone()),
                    cwd: input.cwd.clone(),
                    ..past_row(session, state)
                }
            }
            None => AgentSessionRow {
                agent: None,
                session_id: None,
                title: Some(input.title.clone()).filter(|title| !title.is_empty()),
                cwd: input.cwd.clone(),
                last_used: None,
                state,
            },
        };
        detached_rows.push(row);
    }

    let mut rows = Vec::new();
    let mut live_keys = running_keys.clone();
    for pane in panes {
        if let Some(descriptor) = &pane.running {
            let state = AgentSessionState::Running {
                locator: pane.locator,
            };
            rows.push(live_row(pane, descriptor, state));
        } else if let Some(descriptor) = &pane.pending {
            let key = key_of(descriptor);
            if key
                .as_ref()
                .is_some_and(|key| running_keys.contains(key) || detached_keys.contains(key))
            {
                continue;
            }
            live_keys.extend(key);
            let state = AgentSessionState::Pending {
                locator: pane.locator,
            };
            rows.push(live_row(pane, descriptor, state));
        }
    }
    rows.extend(detached_rows);

    for (index, session) in pasts.into_iter().enumerate() {
        let key = (session.agent, session.session_id.clone());
        if merged_pasts.contains(&index) || live_keys.contains(&key) || detached_keys.contains(&key)
        {
            continue;
        }
        rows.push(past_row(session, AgentSessionState::Past));
    }
    rows
}

#[cfg(test)]
#[path = "agent_sessions_index_tests.rs"]
mod tests;
