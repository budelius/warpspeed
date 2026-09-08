use chrono::{DateTime, Utc};
use warpui::elements::{
    Align, Border, ConstrainedBox, Container, CornerRadius, Flex, Highlight, ParentElement, Radius,
    Shrinkable, Text,
};
use warpui::fonts::{Properties, Weight};
use warpui::ui_components::components::{Coords, UiComponent, UiComponentStyles};
use warpui::ui_components::text::Span;
use warpui::{Element, SingletonEntity};

use crate::appearance::Appearance;
use crate::launch_configs::launch_config::LaunchConfig;
use crate::launch_configs::session_store::{StoredPaneSummary, StoredSessionSummary};
use crate::search::result_renderer::ItemHighlightState;
use crate::themes::theme::Fill;
use crate::util::time_format::format_approx_duration_from_now_utc;

impl LaunchConfig {
    /// Renders a [`LaunchConfig`] using a [`StylesProvider`]. Any character indices of the launch
    /// config title contained within `highlighted_indices` are highlighted in bold.
    pub(super) fn render(
        &self,
        appearance: &Appearance,
        item_highlight_state: ItemHighlightState,
        highlight_indices: Vec<usize>,
    ) -> Box<dyn Element> {
        let bg_color = background_fill(item_highlight_state, appearance);

        let text_color = appearance.theme().main_text_color(bg_color).into_solid();

        let highlight = Highlight::new()
            .with_properties(Properties::default().weight(Weight::Bold))
            .with_foreground_color(text_color);

        let label = self
            .render_launch_config_name(appearance, item_highlight_state)
            .with_single_highlight(highlight, highlight_indices)
            .finish();

        let mut configuration = Flex::row();
        configuration.add_child(Shrinkable::new(1., Align::new(label).left().finish()).finish());

        configuration.add_child(
            Container::new(self.render_config_description(appearance))
                .with_margin_right(14.)
                .finish(),
        );

        ConstrainedBox::new(configuration.finish())
            .with_height(40.)
            .finish()
    }

    fn default_pill_styles(appearance: &Appearance) -> UiComponentStyles {
        UiComponentStyles {
            font_family_id: Some(appearance.ui_font_family()),
            font_size: Some(appearance.monospace_font_size()),
            font_color: Some(
                appearance
                    .theme()
                    .hint_text_color(appearance.theme().background())
                    .into_solid(),
            ),
            border_radius: Some(CornerRadius::with_all(Radius::Pixels(4.))),
            background: Some(appearance.theme().background().into()),
            height: Some(24.),
            padding: Some(Coords::default().left(6.).right(6.)),
            margin: Some(Coords::default().left(3.)),
            ..Default::default()
        }
    }

    fn render_string_with_pill_styling(
        str: impl Into<String>,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        let style = Self::default_pill_styles(appearance);
        let mut container =
            Container::new(Align::new(Span::new(str.into(), style).build().finish()).finish());
        let mut border = Border::all(style.border_width.unwrap_or_default());
        if let Some(border_color) = style.border_color {
            border = border.with_border_fill(border_color);
        }
        container = container.with_border(border);
        if let Some(padding) = style.padding {
            container = container
                .with_padding_top(padding.top)
                .with_padding_right(padding.right)
                .with_padding_bottom(padding.bottom)
                .with_padding_left(padding.left);
        }
        if let Some(radius) = style.border_radius {
            container = container.with_corner_radius(radius);
        }
        if let Some(background_color) = style.background {
            container = container.with_background(background_color);
        }
        let mut sized_container = ConstrainedBox::new(container.finish());
        if let Some(width) = style.width {
            sized_container = sized_container.with_width(width);
        }
        if let Some(height) = style.height {
            sized_container = sized_container.with_height(height);
        }
        let mut container = Container::new(Align::new(sized_container.finish()).finish());
        if let Some(margin) = style.margin {
            container = container
                .with_margin_top(margin.top)
                .with_margin_right(margin.right)
                .with_margin_bottom(margin.bottom)
                .with_margin_left(margin.left);
        }
        container.finish()
    }

    fn render_config_description(&self, appearance: &Appearance) -> Box<dyn Element> {
        let num_windows = self.windows.len();
        let num_tabs: usize = self.windows.iter().map(|window| window.tabs.len()).sum();
        let mut windows_str = num_windows.to_string();
        match num_windows {
            1 => windows_str.push_str(" window "),
            _ => windows_str.push_str(" windows"),
        }
        let mut tabs_str = num_tabs.to_string();
        match num_tabs {
            1 => tabs_str.push_str(" tab "),
            _ => tabs_str.push_str(" tabs"),
        }
        Flex::row()
            .with_children(vec![
                Self::render_string_with_pill_styling(self.source_label(), appearance),
                Self::render_string_with_pill_styling(windows_str, appearance),
                Self::render_string_with_pill_styling(tabs_str, appearance),
            ])
            .finish()
    }

    /// A YAML template and the session config derived from it commonly share a
    /// name, so the row has to say which one it is.
    fn source_label(&self) -> &'static str {
        match self
            .source
            .as_ref()
            .and_then(|source| source.path.extension())
        {
            Some(extension) if extension == "toml" => "Session",
            Some(_) => "YAML",
            None => "Configuration",
        }
    }

    fn render_launch_config_name(
        &self,
        appearance: &Appearance,
        item_highlight_state: ItemHighlightState,
    ) -> Text {
        let text = Text::new_inline(
            self.name.clone(),
            appearance.ui_font_family(),
            appearance.monospace_font_size(),
        );

        let bg_color = background_fill(item_highlight_state, appearance);
        text.with_color(appearance.theme().sub_text_color(bg_color).into_solid())
    }
}

impl StoredSessionSummary {
    pub(super) fn render(
        &self,
        appearance: &Appearance,
        item_highlight_state: ItemHighlightState,
        highlight_indices: Vec<usize>,
    ) -> Box<dyn Element> {
        let bg_color = background_fill(item_highlight_state, appearance);
        let text_color = appearance.theme().main_text_color(bg_color).into_solid();
        let highlight = Highlight::new()
            .with_properties(Properties::default().weight(Weight::Bold))
            .with_foreground_color(text_color);
        let label = Text::new_inline(
            self.name().to_owned(),
            appearance.ui_font_family(),
            appearance.monospace_font_size(),
        )
        .with_color(appearance.theme().sub_text_color(bg_color).into_solid())
        .with_single_highlight(highlight, highlight_indices)
        .finish();

        let mut row = Flex::row();
        row.add_child(Shrinkable::new(1., Align::new(label).left().finish()).finish());
        let saved = DateTime::<Utc>::from_timestamp(self.saved_at_unix_seconds(), 0)
            .map(format_approx_duration_from_now_utc)
            .unwrap_or_else(|| "unknown time".to_owned());
        row.add_child(LaunchConfig::render_string_with_pill_styling(
            format!(
                "{} {saved}",
                if self.is_current() {
                    "Autosaved"
                } else {
                    "Stored"
                }
            ),
            appearance,
        ));
        row.add_child(LaunchConfig::render_string_with_pill_styling(
            count_label(self.tabs().count(), "tab"),
            appearance,
        ));
        if self.agent_count() > 0 {
            row.add_child(LaunchConfig::render_string_with_pill_styling(
                count_label(self.agent_count(), "agent"),
                appearance,
            ));
        }

        ConstrainedBox::new(row.finish()).with_height(40.).finish()
    }

    pub(super) fn render_details(&self, app: &warpui::AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let background = appearance.theme().surface_2();
        let main_text = appearance.theme().main_text_color(background).into_solid();
        let sub_text = appearance.theme().sub_text_color(background).into_solid();
        let mut details = Flex::column();
        details.add_child(
            Container::new(
                Text::new_inline(
                    self.name().to_owned(),
                    appearance.ui_font_family(),
                    appearance.monospace_font_size(),
                )
                .with_color(main_text)
                .finish(),
            )
            .with_margin_bottom(4.)
            .finish(),
        );

        let saved = DateTime::<Utc>::from_timestamp(self.saved_at_unix_seconds(), 0)
            .map(format_approx_duration_from_now_utc)
            .unwrap_or_else(|| "unknown time".to_owned());
        details.add_child(
            Container::new(
                Text::new_inline(
                    format!(
                        "{} session · saved {saved} · {} · {}",
                        if self.is_current() {
                            "Autosaved"
                        } else {
                            "Stored"
                        },
                        count_label(self.windows().len(), "window"),
                        count_label(self.pane_count(), "pane")
                    ),
                    appearance.ui_font_family(),
                    appearance.monospace_font_size() - 1.,
                )
                .with_color(sub_text)
                .finish(),
            )
            .with_margin_bottom(4.)
            .finish(),
        );
        let mut provenance = Vec::new();
        if let Some(profile) = self.profile() {
            provenance.push(format!("Profile: {profile}"));
        }
        if let Some(source_path) = self.source_path() {
            provenance.push(format!("Source: {}", source_path.display()));
        }
        if !provenance.is_empty() {
            details.add_child(
                Container::new(
                    Text::new(
                        provenance.join(" · "),
                        appearance.ui_font_family(),
                        appearance.monospace_font_size() - 2.,
                    )
                    .with_color(sub_text)
                    .finish(),
                )
                .with_margin_bottom(8.)
                .finish(),
            );
        }

        for window in self.windows() {
            if self.windows().len() > 1 {
                details.add_child(detail_heading(
                    format!("Window {}", window.window_index() + 1),
                    appearance,
                    main_text,
                ));
            }
            for tab in window.tabs() {
                let tab_name = tab
                    .title
                    .clone()
                    .unwrap_or_else(|| format!("Tab {}", tab.tab_index + 1));
                details.add_child(detail_heading(tab_name, appearance, main_text));
                for pane in &tab.panes {
                    details.add_child(render_pane_detail(pane, appearance, sub_text));
                }
            }
        }
        details.finish()
    }
}

fn count_label(count: usize, singular: &str) -> String {
    format!("{count} {singular}{}", if count == 1 { "" } else { "s" })
}

fn detail_heading(
    text: String,
    appearance: &Appearance,
    color: pathfinder_color::ColorU,
) -> Box<dyn Element> {
    Container::new(
        Text::new_inline(
            text,
            appearance.ui_font_family(),
            appearance.monospace_font_size() - 1.,
        )
        .with_color(color)
        .finish(),
    )
    .with_margin_top(4.)
    .with_margin_bottom(2.)
    .finish()
}

fn render_pane_detail(
    pane: &StoredPaneSummary,
    appearance: &Appearance,
    color: pathfinder_color::ColorU,
) -> Box<dyn Element> {
    let pane_id = pane.pane_uuid.chars().take(8).collect::<String>();
    let shell = pane.shell.as_deref().unwrap_or("Default shell");
    let (agent, activity) = match &pane.agent {
        Some(agent) => {
            let mut metadata = Vec::new();
            if let Some(title) = &agent.title {
                metadata.push(title.clone());
            }
            if let Some(session_id) = &agent.session_id {
                metadata.push(format!("Session {session_id}"));
            } else {
                metadata.push("Session ID unavailable; install the Warp agent plugin".to_owned());
            }
            if let Some(resume_command) = &agent.resume_command {
                metadata.push(format!("Resume: {resume_command}"));
            }
            (agent.agent.display_name().to_owned(), metadata.join(" · "))
        }
        None => ("Shell".to_owned(), "No agent captured".to_owned()),
    };
    let cwd = pane.cwd.as_deref().unwrap_or("Unknown directory");
    let recovery = if pane.has_persistent_process {
        "reattach process if available"
    } else if pane
        .agent
        .as_ref()
        .is_some_and(|agent| agent.resume_command.is_some())
    {
        "agent resume"
    } else {
        "shell restore"
    };
    Container::new(
        Text::new(
            format!(
                "Pane {} ({pane_id}) · {agent} · {shell}\n{activity}\n{cwd} · {recovery}",
                pane.pane_number
            ),
            appearance.ui_font_family(),
            appearance.monospace_font_size() - 2.,
        )
        .with_color(color)
        .finish(),
    )
    .with_margin_left(6.)
    .with_margin_bottom(6.)
    .finish()
}

fn background_fill(item_highlight_state: ItemHighlightState, appearance: &Appearance) -> Fill {
    item_highlight_state
        .container_background_fill(appearance)
        .unwrap_or_else(|| appearance.theme().surface_2())
}
