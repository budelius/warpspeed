use warpui::{Entity, ModelContext, SingletonEntity};

use super::agent_session_history::{PastAgentSession, find_all_resumable};

pub(crate) enum AgentSessionHistoryEvent {
    Updated,
}

/// Past agent conversations on disk, refreshed in the background so pickers never block on I/O.
#[derive(Default)]
pub(crate) struct AgentSessionHistoryModel {
    sessions: Vec<PastAgentSession>,
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

    pub(crate) fn is_loaded(&self) -> bool {
        self.loaded
    }

    pub(crate) fn refresh(&mut self, ctx: &mut ModelContext<Self>) {
        self.refresh_with(find_all_resumable, ctx);
    }

    fn refresh_with(&mut self, load: fn() -> Vec<PastAgentSession>, ctx: &mut ModelContext<Self>) {
        if self.refreshing {
            self.refresh_again = true;
            return;
        }
        self.refreshing = true;
        let (sender, receiver) = futures::channel::oneshot::channel();
        ctx.background_executor()
            .spawn(async move {
                let _ = sender.send(load());
            })
            .detach();
        ctx.spawn(receiver, move |model, sessions, ctx| {
            model.refreshing = false;
            if let Ok(sessions) = sessions {
                model.apply(sessions, ctx);
            }
            if std::mem::take(&mut model.refresh_again) {
                model.refresh_with(load, ctx);
            }
        });
    }

    /// Announces the scan only when it found something new: an update reruns the open pickers'
    /// queries, which would otherwise disturb them for nothing.
    fn apply(&mut self, sessions: Vec<PastAgentSession>, ctx: &mut ModelContext<Self>) {
        let changed = !self.loaded || self.sessions != sessions;
        self.sessions = sessions;
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
        self.apply(sessions, ctx);
    }
}

impl Entity for AgentSessionHistoryModel {
    type Event = AgentSessionHistoryEvent;
}

impl SingletonEntity for AgentSessionHistoryModel {}

#[cfg(test)]
#[path = "agent_session_history_model_tests.rs"]
mod tests;
