use std::collections::HashSet;
use std::path::{Path, PathBuf};

use warpui::{Entity, ModelContext, SingletonEntity};

use super::agent_session_history::{PastAgentSession, find_all_resumable, find_all_resumable_in};
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
    recent_sessions: Vec<PastAgentSession>,
    folder_sessions: Vec<(PathBuf, Vec<PastAgentSession>)>,
    refreshing_folders: HashSet<PathBuf>,
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

    /// Adds every recorded session in `folder` to the recent machine-wide snapshot.
    pub(crate) fn refresh_folder(&mut self, folder: &Path, ctx: &mut ModelContext<Self>) {
        if folder.as_os_str().is_empty() {
            return;
        }
        let folder = folder.to_path_buf();
        if !self.refreshing_folders.insert(folder.clone()) {
            return;
        }
        let scan_folder = folder.clone();
        let (sender, receiver) = futures::channel::oneshot::channel();
        ctx.background_executor()
            .spawn(async move {
                let _ = sender.send(find_all_resumable_in(&scan_folder));
            })
            .detach();
        ctx.spawn(receiver, move |model, sessions, ctx| {
            model.refreshing_folders.remove(&folder);
            if let Ok(sessions) = sessions {
                match model
                    .folder_sessions
                    .iter_mut()
                    .find(|(scanned, _)| *scanned == folder)
                {
                    Some((_, current)) => *current = sessions,
                    None => model.folder_sessions.push((folder, sessions)),
                }
                model.rebuild_sessions(ctx);
            }
        });
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
        let was_loaded = self.loaded;
        let agents_changed = self.scanned_agents != scanned_agents;
        self.recent_sessions = sessions;
        self.scanned_agents = scanned_agents;
        self.loaded = true;
        self.rebuild_sessions_if(!was_loaded || agents_changed, ctx);
    }

    fn rebuild_sessions(&mut self, ctx: &mut ModelContext<Self>) {
        self.rebuild_sessions_if(false, ctx);
    }

    fn rebuild_sessions_if(&mut self, force_update: bool, ctx: &mut ModelContext<Self>) {
        let mut sessions = self.recent_sessions.clone();
        for (_, folder_sessions) in &self.folder_sessions {
            for session in folder_sessions {
                if !sessions.iter().any(|current| {
                    current.agent == session.agent && current.session_id == session.session_id
                }) {
                    sessions.push(session.clone());
                }
            }
        }
        sessions.sort_by(|left, right| right.modified.cmp(&left.modified));
        let changed = force_update || self.sessions != sessions;
        self.sessions = sessions;
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
