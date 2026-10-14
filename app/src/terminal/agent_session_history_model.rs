use warpui::{Entity, ModelContext, SingletonEntity};

use super::agent_session_history::{PastAgentSession, find_all_resumable};
use super::cli_agent_sessions::agent_process_scan::{ScannedAgent, scan};

pub(crate) enum AgentSessionHistoryEvent {
    Updated,
}

fn scan_agents() -> Vec<ScannedAgent> {
    scan()
}

/// Local agent history and process placement, refreshed off the UI thread.
#[derive(Default)]
pub(crate) struct AgentSessionHistoryModel {
    sessions: Vec<PastAgentSession>,
    scanned_agents: Vec<ScannedAgent>,
    loaded: bool,
    refreshing: bool,
    refresh_again: bool,
}

impl AgentSessionHistoryModel {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn sessions(&self) -> &[PastAgentSession] {
        &self.sessions
    }

    pub(crate) fn scanned_agents(&self) -> &[ScannedAgent] {
        &self.scanned_agents
    }

    pub(crate) fn is_loaded(&self) -> bool {
        self.loaded
    }

    pub(crate) fn refresh(&mut self, ctx: &mut ModelContext<Self>) {
        self.refresh_snapshot_with(find_all_resumable, scan_agents, ctx);
    }

    #[cfg(test)]
    fn refresh_with(&mut self, load: fn() -> Vec<PastAgentSession>, ctx: &mut ModelContext<Self>) {
        self.refresh_snapshot_with(load, Vec::<ScannedAgent>::new, ctx);
    }

    fn refresh_snapshot_with(
        &mut self,
        load: fn() -> Vec<PastAgentSession>,
        scan_agents: fn() -> Vec<ScannedAgent>,
        ctx: &mut ModelContext<Self>,
    ) {
        if self.refreshing {
            self.refresh_again = true;
            return;
        }
        self.refreshing = true;
        let (sender, receiver) = futures::channel::oneshot::channel();
        ctx.background_executor()
            .spawn(async move {
                let _ = sender.send((load(), scan_agents()));
            })
            .detach();
        ctx.spawn(receiver, move |model, snapshot, ctx| {
            model.refreshing = false;
            if let Ok((sessions, scanned_agents)) = snapshot {
                model.apply(sessions, scanned_agents, ctx);
            }
            if std::mem::take(&mut model.refresh_again) {
                model.refresh_snapshot_with(load, scan_agents, ctx);
            }
        });
    }

    /// Announces the scan only when it found something new: an update reruns the open pickers'
    /// queries, which would otherwise disturb them for nothing.
    fn apply(
        &mut self,
        sessions: Vec<PastAgentSession>,
        scanned_agents: Vec<ScannedAgent>,
        ctx: &mut ModelContext<Self>,
    ) {
        let changed =
            !self.loaded || self.sessions != sessions || self.scanned_agents != scanned_agents;
        self.sessions = sessions;
        self.scanned_agents = scanned_agents;
        self.loaded = true;
        if changed {
            ctx.emit(AgentSessionHistoryEvent::Updated);
        }
    }

    /// Replaces the scanned sessions, and holds back the scans pickers start on opening so they
    /// cannot replace them with what is on this machine's disk.
    #[cfg(test)]
    pub(crate) fn set_sessions_for_test(
        &mut self,
        sessions: Vec<PastAgentSession>,
        ctx: &mut ModelContext<Self>,
    ) {
        self.refreshing = true;
        self.apply(sessions, Vec::new(), ctx);
    }
}

impl Entity for AgentSessionHistoryModel {
    type Event = AgentSessionHistoryEvent;
}

impl SingletonEntity for AgentSessionHistoryModel {}

#[cfg(test)]
#[path = "agent_session_history_model_tests.rs"]
mod tests;
