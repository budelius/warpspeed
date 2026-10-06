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
                model.sessions = sessions;
                model.loaded = true;
                ctx.emit(AgentSessionHistoryEvent::Updated);
            }
            if std::mem::take(&mut model.refresh_again) {
                model.refresh_with(load, ctx);
            }
        });
    }
}

impl Entity for AgentSessionHistoryModel {
    type Event = AgentSessionHistoryEvent;
}

impl SingletonEntity for AgentSessionHistoryModel {}

#[cfg(test)]
#[path = "agent_session_history_model_tests.rs"]
mod tests;
