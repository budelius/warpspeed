//! The AgentCtrl fleet pane: the managed runs on staging, grouped by repository.
pub(crate) mod fleet_model;
pub(crate) mod fleet_view;

use std::collections::HashMap;

use warpui::{Entity, SingletonEntity, WindowId};

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

    pub fn deregister_pane(&mut self, window_id: &WindowId) {
        self.panes.remove(window_id);
    }
}

impl Entity for FleetPaneManager {
    type Event = ();
}

impl SingletonEntity for FleetPaneManager {}
