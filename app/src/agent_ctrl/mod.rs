//! The AgentCtrl fleet pane: the managed runs on staging, grouped by repository.
pub(crate) mod fleet_model;
pub(crate) mod fleet_view;

use std::collections::HashMap;

use warpui::{Entity, SingletonEntity, WindowId};

use crate::pane_group::PaneId;
use crate::workspace::PaneViewLocator;

/// The open fleet pane of each window, so a window shows at most one.
#[derive(Default)]
pub struct FleetPaneManager {
    panes: HashMap<WindowId, PaneViewLocator>,
}

impl FleetPaneManager {
    pub fn find_pane(&self, window_id: WindowId) -> Option<PaneViewLocator> {
        self.panes.get(&window_id).copied()
    }

    pub fn register_pane(&mut self, window_id: WindowId, locator: PaneViewLocator) {
        self.panes.insert(window_id, locator);
    }

    /// Forgets `window_id`'s fleet pane if it is `pane_id`, so a closed pane cannot deregister
    /// the one that replaced it.
    pub fn deregister_pane(&mut self, window_id: &WindowId, pane_id: PaneId) {
        if self
            .panes
            .get(window_id)
            .is_some_and(|locator| locator.pane_id == pane_id)
        {
            self.panes.remove(window_id);
        }
    }
}

impl Entity for FleetPaneManager {
    type Event = ();
}

impl SingletonEntity for FleetPaneManager {}
