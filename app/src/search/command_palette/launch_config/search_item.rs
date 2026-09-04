use std::sync::Arc;

use fuzzy_match::FuzzyMatchResult;
use ordered_float::OrderedFloat;
use warpui::{AppContext, Element, SingletonEntity};

use crate::appearance::Appearance;
use crate::launch_configs::launch_config::LaunchConfig;
use crate::launch_configs::session_store::StoredSessionSummary;
use crate::search::command_palette::mixer::CommandPaletteItemAction;
use crate::search::command_palette::render_util::render_search_item_icon;
use crate::search::result_renderer::ItemHighlightState;
use crate::ui_components::icons::Icon;

#[derive(Debug)]
enum Item {
    Configuration(Arc<LaunchConfig>),
    StoredSession(Arc<StoredSessionSummary>),
}

/// Search item for a launch configuration or one of its autosaved sessions.
#[derive(Debug)]
pub struct SearchItem {
    match_result: FuzzyMatchResult,
    item: Item,
}

impl SearchItem {
    pub fn new(launch_config: Arc<LaunchConfig>, match_result: FuzzyMatchResult) -> Self {
        Self {
            match_result,
            item: Item::Configuration(launch_config),
        }
    }

    pub fn stored_session(
        summary: Arc<StoredSessionSummary>,
        match_result: FuzzyMatchResult,
    ) -> Self {
        Self {
            match_result,
            item: Item::StoredSession(summary),
        }
    }

    fn name(&self) -> &str {
        match &self.item {
            Item::Configuration(config) => &config.name,
            Item::StoredSession(summary) => summary.name(),
        }
    }
}

impl crate::search::item::SearchItem for SearchItem {
    type Action = CommandPaletteItemAction;

    fn render_icon(
        &self,
        highlight_state: ItemHighlightState,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        let color = appearance.theme().foreground().into_solid();
        let icon = match &self.item {
            Item::Configuration(_) => Icon::Navigation,
            Item::StoredSession(_) => Icon::ClockRefresh,
        };
        render_search_item_icon(appearance, icon, color, highlight_state)
    }

    fn render_item(
        &self,
        highlight_state: ItemHighlightState,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        match &self.item {
            Item::Configuration(config) => config.render(
                appearance,
                highlight_state,
                self.match_result.matched_indices.clone(),
            ),
            Item::StoredSession(summary) => summary.render(
                appearance,
                highlight_state,
                self.match_result.matched_indices.clone(),
            ),
        }
    }

    fn render_details(&self, app: &AppContext) -> Option<Box<dyn Element>> {
        match &self.item {
            Item::Configuration(_) => None,
            Item::StoredSession(summary) => Some(summary.render_details(app)),
        }
    }

    fn score(&self) -> OrderedFloat<f64> {
        OrderedFloat::from(self.match_result.score as f64)
    }

    fn accept_result(&self) -> Self::Action {
        match &self.item {
            Item::Configuration(config) => CommandPaletteItemAction::OpenLaunchConfiguration {
                config: config.clone(),
                open_in_active_window: false,
            },
            Item::StoredSession(summary) => CommandPaletteItemAction::OpenStoredSession {
                summary: summary.clone(),
            },
        }
    }

    fn execute_result(&self) -> Self::Action {
        match &self.item {
            Item::Configuration(config) => CommandPaletteItemAction::OpenLaunchConfiguration {
                config: config.clone(),
                open_in_active_window: true,
            },
            Item::StoredSession(summary) => CommandPaletteItemAction::OpenStoredSession {
                summary: summary.clone(),
            },
        }
    }

    fn accessibility_label(&self) -> String {
        match &self.item {
            Item::Configuration(_) => format!("Selected configuration {}.", self.name()),
            Item::StoredSession(_) => format!("Selected stored session {}.", self.name()),
        }
    }

    fn accessibility_help_message(&self) -> Option<String> {
        Some(
            match &self.item {
                Item::Configuration(_) => "Press enter to use this launch configuration.",
                Item::StoredSession(_) => "Press enter to restore the complete autosaved session.",
            }
            .into(),
        )
    }
}
