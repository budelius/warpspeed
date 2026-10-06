use chrono::{DateTime, Local};
use ordered_float::OrderedFloat;
use warp_core::ui::icons::Icon;
use warpui::elements::{
    Container, CrossAxisAlignment, Expanded, Flex, Highlight, MainAxisSize, ParentElement, Text,
};
use warpui::fonts::{Properties, Weight};
use warpui::{AppContext, Element, SingletonEntity};

use crate::appearance::Appearance;
use crate::search::SearchItem;
use crate::search::command_palette::agent_sessions::search::{RowMatch, row_folder, row_title};
use crate::search::command_palette::mixer::CommandPaletteItemAction;
use crate::search::command_palette::render_util::render_search_item_icon;
use crate::search::item::IconLocation;
use crate::search::result_renderer::ItemHighlightState;
use crate::terminal::agent_sessions_index::AgentSessionRow;
use crate::util::time_format::format_approx_duration_from_now;
use crate::workspace::agent_session_actions::AgentSessionAction;

/// One agent session in the command palette. A row with no action renders disabled.
#[derive(Debug)]
pub struct AgentSessionSearchItem {
    row: AgentSessionRow,
    action: Option<AgentSessionAction>,
    matched: RowMatch,
}

impl AgentSessionSearchItem {
    pub(super) fn new(
        row: AgentSessionRow,
        action: Option<AgentSessionAction>,
        matched: RowMatch,
    ) -> Self {
        Self {
            row,
            action,
            matched,
        }
    }

    fn action_hint(&self) -> Option<&'static str> {
        self.action.as_ref().map(|action| match action {
            AgentSessionAction::Focus(_) => "Focus",
            AgentSessionAction::Start(_) => "Start",
            AgentSessionAction::Attach(_) => "Attach",
            AgentSessionAction::Resume { .. } => "Resume",
        })
    }

    fn trailing_text(&self) -> String {
        let last_used = self
            .row
            .last_used
            .map(|last_used| format_approx_duration_from_now(DateTime::<Local>::from(last_used)));
        [last_used.as_deref(), self.action_hint()]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" · ")
    }
}

impl SearchItem for AgentSessionSearchItem {
    type Action = CommandPaletteItemAction;

    fn is_multiline(&self) -> bool {
        true
    }

    fn render_icon(
        &self,
        highlight_state: ItemHighlightState,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        let icon = self
            .row
            .agent
            .and_then(|agent| agent.icon())
            .unwrap_or(Icon::Terminal);
        render_search_item_icon(
            appearance,
            icon,
            appearance.theme().foreground().into_solid(),
            highlight_state,
        )
    }

    fn icon_location(&self, appearance: &Appearance) -> IconLocation {
        // Aligns the icon, sized to the font, with the first line of text, sized to the line height.
        let margin_top = (appearance.line_height_ratio() * appearance.monospace_font_size())
            - appearance.monospace_font_size();
        IconLocation::Top { margin_top }
    }

    fn render_item(
        &self,
        highlight_state: ItemHighlightState,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();
        let text_color = if self.is_disabled() {
            theme.disabled_text_color(theme.surface_2()).into_solid()
        } else {
            highlight_state.sub_text_fill(appearance).into_solid()
        };
        let sub_text_font_size = appearance.monospace_font_size() - 2.;
        let highlight = Highlight::new()
            .with_properties(Properties::default().weight(Weight::Bold))
            .with_foreground_color(highlight_state.main_text_fill(appearance).into_solid());

        let mut title = Text::new_inline(
            row_title(&self.row),
            appearance.ui_font_family(),
            appearance.monospace_font_size(),
        )
        .with_color(text_color)
        .with_style(Properties::default().weight(Weight::Bold));
        if !self.matched.title_indices.is_empty() {
            title = title.with_single_highlight(highlight, self.matched.title_indices.clone());
        }

        let mut folder = Text::new_inline(
            row_folder(&self.row),
            appearance.ui_font_family(),
            sub_text_font_size,
        )
        .with_color(text_color);
        if !self.matched.folder_indices.is_empty() {
            folder = folder.with_single_highlight(highlight, self.matched.folder_indices.clone());
        }

        let trailing = Container::new(
            Text::new_inline(
                self.trailing_text(),
                appearance.ui_font_family(),
                sub_text_font_size,
            )
            .with_color(text_color)
            .finish(),
        )
        .with_padding_left(8.)
        .finish();

        Flex::row()
            .with_child(
                Expanded::new(
                    1.0,
                    Flex::column()
                        .with_spacing(4.)
                        .with_child(title.finish())
                        .with_child(folder.finish())
                        .finish(),
                )
                .finish(),
            )
            .with_child(trailing)
            .with_cross_axis_alignment(CrossAxisAlignment::Start)
            .with_main_axis_size(MainAxisSize::Max)
            .finish()
    }

    fn score(&self) -> OrderedFloat<f64> {
        OrderedFloat(self.matched.score as f64)
    }

    fn accept_result(&self) -> Self::Action {
        match &self.action {
            Some(action) => CommandPaletteItemAction::AgentSession {
                action: action.clone(),
            },
            None => CommandPaletteItemAction::NoOp,
        }
    }

    fn execute_result(&self) -> Self::Action {
        self.accept_result()
    }

    fn accessibility_label(&self) -> String {
        format!("Agent session: {}", row_title(&self.row))
    }

    fn accessibility_help_message(&self) -> Option<String> {
        self.action_hint().map(|hint| {
            format!(
                "Press enter to {} \"{}\".",
                hint.to_lowercase(),
                row_title(&self.row)
            )
        })
    }

    fn is_disabled(&self) -> bool {
        self.action.is_none()
    }
}
