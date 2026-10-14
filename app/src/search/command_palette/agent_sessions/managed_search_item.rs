use managed_agent_sessions::RunSummary;
use ordered_float::OrderedFloat;
use warp_core::ui::icons::Icon;
use warpui::elements::{Flex, ParentElement, Text};
use warpui::{AppContext, Element, SingletonEntity};

use crate::agent_ctrl::session_picker::{
    managed_action_label, managed_run_detail, managed_run_title,
};
use crate::appearance::Appearance;
use crate::search::SearchItem;
use crate::search::command_palette::mixer::CommandPaletteItemAction;
use crate::search::command_palette::render_util::render_search_item_icon;
use crate::search::result_renderer::ItemHighlightState;
use crate::workspace::PaneViewLocator;
use crate::workspace::agent_session_actions::AgentSessionAction;

#[derive(Debug)]
pub(super) struct ManagedSessionSearchItem {
    run: RunSummary,
    target: Option<PaneViewLocator>,
    score: i64,
}

impl ManagedSessionSearchItem {
    pub(super) fn new(run: RunSummary, target: Option<PaneViewLocator>, score: i64) -> Self {
        Self { run, target, score }
    }
}

impl SearchItem for ManagedSessionSearchItem {
    type Action = CommandPaletteItemAction;

    fn is_multiline(&self) -> bool {
        true
    }

    fn render_icon(
        &self,
        highlight_state: ItemHighlightState,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        render_search_item_icon(
            appearance,
            Icon::Terminal,
            appearance.theme().foreground().into_solid(),
            highlight_state,
        )
    }

    fn render_item(
        &self,
        highlight_state: ItemHighlightState,
        ctx: &AppContext,
    ) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(ctx);
        Flex::column()
            .with_spacing(4.)
            .with_child(
                Text::new_inline(
                    format!(
                        "{} · {}",
                        managed_run_title(&self.run),
                        managed_action_label(&self.run.state)
                    ),
                    appearance.ui_font_family(),
                    appearance.monospace_font_size(),
                )
                .with_color(highlight_state.main_text_fill(appearance).into_solid())
                .finish(),
            )
            .with_child(
                Text::new_inline(
                    managed_run_detail(&self.run),
                    appearance.ui_font_family(),
                    appearance.monospace_font_size() - 2.,
                )
                .with_color(highlight_state.sub_text_fill(appearance).into_solid())
                .finish(),
            )
            .finish()
    }

    fn score(&self) -> OrderedFloat<f64> {
        OrderedFloat(self.score as f64)
    }

    fn accept_result(&self) -> Self::Action {
        CommandPaletteItemAction::AgentSession {
            action: AgentSessionAction::Managed {
                job_id: self.run.job_id.clone(),
                target: self.target,
            },
        }
    }

    fn execute_result(&self) -> Self::Action {
        self.accept_result()
    }

    fn accessibility_label(&self) -> String {
        format!(
            "AgentCtrl session: {} · {}",
            managed_run_title(&self.run),
            managed_run_detail(&self.run)
        )
    }

    fn selection_key(&self) -> Option<String> {
        Some(format!("agentctrl:{}", self.run.job_id))
    }

    fn accessibility_help_message(&self) -> Option<String> {
        Some(format!(
            "Press enter to {}.",
            managed_action_label(&self.run.state).to_lowercase()
        ))
    }
}
