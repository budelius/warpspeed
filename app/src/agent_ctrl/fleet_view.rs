//! The fleet pane's view: AgentCtrl staging's runs, grouped by repository, with Open and Stop on
//! each run and a Start… menu listing the repositories a run can start on.
use std::collections::HashMap;
use std::time::Duration;

use instant::Instant;
use warpui::elements::{
    ClippedScrollStateHandle, ClippedScrollable, Container, CornerRadius, CrossAxisAlignment,
    Expanded, Flex, MainAxisSize, MouseStateHandle, ParentElement, Radius, ScrollbarWidth, Text,
};
use warpui::fonts::{Properties, Weight};
use warpui::text_layout::ClipConfig;
use warpui::ui_components::button::ButtonVariant;
use warpui::ui_components::components::UiComponent;
use warpui::{
    AppContext, Element, Entity, ModelHandle, SingletonEntity, TypedActionView, View, ViewContext,
};

use super::fleet_model::{FleetGroup, FleetModel, FleetModelEvent, FleetRow};
use crate::appearance::Appearance;
use crate::pane_group::focus_state::PaneFocusHandle;
use crate::pane_group::pane::view::{self, HeaderContent, StandardHeader, StandardHeaderOptions};
use crate::pane_group::{BackingView, PaneConfiguration, PaneEvent};
use crate::terminal::CLIAgent;
use crate::ui_components::blended_colors;
use crate::workspace::WorkspaceAction;

pub const FLEET_HEADER_TEXT: &str = "AgentCtrl fleet · staging";

/// How long a first click on Stop waits for the confirming second click.
const STOP_CONFIRMATION_WINDOW: Duration = Duration::from_secs(5);

const STARTABLE_AGENTS: [(CLIAgent, &str); 3] = [
    (CLIAgent::Claude, "Claude"),
    (CLIAgent::Pi, "Pi"),
    (CLIAgent::Codex, "Codex"),
];

const SPACING: f32 = 8.;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FleetViewEvent {
    Pane(PaneEvent),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FleetViewAction {
    Refresh,
    DismissActionError,
    ToggleStartMenu,
    Start { repo_url: String, agent: CLIAgent },
    Stop { job_id: String },
}

/// Pane header overflow menu actions (none).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FleetViewHeaderAction {}

#[derive(Clone, Default)]
struct RowMouseStates {
    open: MouseStateHandle,
    stop: MouseStateHandle,
    branch: MouseStateHandle,
}

pub struct FleetView {
    model: ModelHandle<FleetModel>,
    pane_configuration: ModelHandle<PaneConfiguration>,
    focus_handle: Option<PaneFocusHandle>,
    scroll_state: ClippedScrollStateHandle,
    refresh_mouse_state: MouseStateHandle,
    start_mouse_state: MouseStateHandle,
    dismiss_error_mouse_state: MouseStateHandle,
    row_mouse_states: HashMap<String, RowMouseStates>,
    /// One per startable agent, by repository URL.
    repo_mouse_states: HashMap<String, [MouseStateHandle; 3]>,
    start_menu_open: bool,
    /// The run whose Stop was clicked once, and when.
    stop_armed: Option<(String, Instant)>,
}

impl FleetView {
    pub fn new(ctx: &mut ViewContext<Self>) -> Self {
        let pane_configuration = ctx.add_model(|_| PaneConfiguration::new(FLEET_HEADER_TEXT));
        let model = FleetModel::handle(ctx);
        ctx.subscribe_to_model(&model, |me, _, event, ctx| match event {
            FleetModelEvent::Updated => {
                me.sync_mouse_states(ctx);
                ctx.notify();
            }
        });
        let mut view = Self {
            model,
            pane_configuration,
            focus_handle: None,
            scroll_state: Default::default(),
            refresh_mouse_state: Default::default(),
            dismiss_error_mouse_state: Default::default(),
            start_mouse_state: Default::default(),
            row_mouse_states: HashMap::new(),
            repo_mouse_states: HashMap::new(),
            start_menu_open: false,
            stop_armed: None,
        };
        view.sync_mouse_states(ctx);
        view
    }

    pub fn pane_configuration(&self) -> ModelHandle<PaneConfiguration> {
        self.pane_configuration.clone()
    }

    pub fn focus(&mut self, ctx: &mut ViewContext<Self>) {
        ctx.focus_self();
    }

    pub fn refresh(&mut self, ctx: &mut ViewContext<Self>) {
        self.model.update(ctx, |model, ctx| model.refresh(ctx));
    }

    /// Keeps one set of mouse states per run and repository shown, so they outlive renders.
    fn sync_mouse_states(&mut self, ctx: &mut ViewContext<Self>) {
        let model = self.model.as_ref(ctx);
        let job_ids: Vec<String> = model
            .groups()
            .into_iter()
            .flat_map(|group| group.rows)
            .map(|row| row.job_id)
            .collect();
        let repo_urls: Vec<String> = model.repos().iter().map(|repo| repo.url.clone()).collect();

        self.row_mouse_states
            .retain(|job_id, _| job_ids.contains(job_id));
        for job_id in job_ids {
            self.row_mouse_states.entry(job_id).or_default();
        }
        self.repo_mouse_states
            .retain(|repo_url, _| repo_urls.contains(repo_url));
        for repo_url in repo_urls {
            self.repo_mouse_states.entry(repo_url).or_default();
        }
    }

    fn stop(&mut self, job_id: String, ctx: &mut ViewContext<Self>) {
        let confirmed = self
            .stop_armed
            .as_ref()
            .is_some_and(|(armed, at)| *armed == job_id && at.elapsed() < STOP_CONFIRMATION_WINDOW);
        if confirmed {
            self.stop_armed = None;
            self.model.update(ctx, |model, ctx| model.stop(job_id, ctx));
        } else {
            let armed_at = Instant::now();
            self.stop_armed = Some((job_id, armed_at));
            ctx.spawn(
                async { warpui::r#async::Timer::after(STOP_CONFIRMATION_WINDOW).await },
                move |me, _, ctx| {
                    if me
                        .stop_armed
                        .as_ref()
                        .is_some_and(|(_, at)| *at == armed_at)
                    {
                        me.stop_armed = None;
                        ctx.notify();
                    }
                },
            );
        }
        ctx.notify();
    }

    fn is_stop_armed(&self, job_id: &str) -> bool {
        self.stop_armed
            .as_ref()
            .is_some_and(|(armed, at)| armed == job_id && at.elapsed() < STOP_CONFIRMATION_WINDOW)
    }

    fn text(text: impl Into<String>, appearance: &Appearance) -> Text {
        Text::new_inline(
            text.into(),
            appearance.ui_font_family(),
            appearance.ui_font_size(),
        )
        .with_color(appearance.theme().active_ui_text_color().into())
    }

    fn secondary_text(text: impl Into<String>, appearance: &Appearance) -> Text {
        let theme = appearance.theme();
        Self::text(text, appearance).with_color(blended_colors::text_sub(theme, theme.surface_1()))
    }

    fn button(
        label: &str,
        mouse_state: &MouseStateHandle,
        action: Option<FleetViewAction>,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        let button = appearance
            .ui_builder()
            .button(ButtonVariant::Secondary, mouse_state.clone())
            .with_centered_text_label(label.to_owned())
            .build();
        match action {
            Some(action) => button
                .on_click(move |ctx, _, _| ctx.dispatch_typed_action(action.clone()))
                .finish(),
            None => button.disable().finish(),
        }
    }

    fn render_toolbar(&self, appearance: &Appearance, app: &AppContext) -> Box<dyn Element> {
        let model = self.model.as_ref(app);
        let status = if model.is_refreshing() {
            "Refreshing…"
        } else {
            "Refreshed every 15 s while open"
        };
        let start_label = if self.start_menu_open {
            "Close start menu"
        } else {
            "Start…"
        };
        Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(SPACING)
            .with_child(
                Expanded::new(1., Self::secondary_text(status, appearance).finish()).finish(),
            )
            .with_child(Self::button(
                "Refresh",
                &self.refresh_mouse_state,
                Some(FleetViewAction::Refresh),
                appearance,
            ))
            .with_child(Self::button(
                start_label,
                &self.start_mouse_state,
                Some(FleetViewAction::ToggleStartMenu),
                appearance,
            ))
            .finish()
    }

    fn render_start_menu(&self, appearance: &Appearance, app: &AppContext) -> Box<dyn Element> {
        let theme = appearance.theme();
        let ui_builder = appearance.ui_builder().clone();
        let mut menu = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(SPACING)
            .with_child(
                Self::text("Start a run on", appearance)
                    .with_style(Properties::default().weight(Weight::Bold))
                    .finish(),
            );
        let repos = self.model.as_ref(app).repos();
        if repos.is_empty() {
            menu.add_child(
                Self::secondary_text("AgentCtrl lists no repositories.", appearance).finish(),
            );
        }
        for repo in repos {
            let Some(mouse_states) = self.repo_mouse_states.get(&repo.url) else {
                continue;
            };
            let mut description = Flex::column()
                .with_child(Self::text(repo.name.clone(), appearance).finish())
                .with_child(
                    Self::secondary_text(format!("{} · {}", repo.url, repo.branch), appearance)
                        .with_clip(ClipConfig::start())
                        .finish(),
                );
            let unreachable_reason = (!repo.reachable).then(|| {
                repo.error
                    .clone()
                    .unwrap_or_else(|| "AgentCtrl cannot reach this repository".to_owned())
            });
            if let Some(reason) = &unreachable_reason {
                description.add_child(
                    Self::text(reason.clone(), appearance)
                        .with_color(theme.ui_error_color())
                        .finish(),
                );
            }
            let mut row = Flex::row()
                .with_main_axis_size(MainAxisSize::Max)
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(SPACING)
                .with_child(Expanded::new(1., description.finish()).finish());
            for ((agent, label), mouse_state) in STARTABLE_AGENTS.iter().zip(mouse_states) {
                let button = ui_builder
                    .button(ButtonVariant::Secondary, mouse_state.clone())
                    .with_centered_text_label((*label).to_owned());
                let element = match &unreachable_reason {
                    Some(reason) => {
                        let reason = reason.clone();
                        let tooltip_builder = ui_builder.clone();
                        button
                            .with_tooltip(move || tooltip_builder.tool_tip(reason).build().finish())
                            .build()
                            .disable()
                            .finish()
                    }
                    None => {
                        let action = FleetViewAction::Start {
                            repo_url: repo.url.clone(),
                            agent: *agent,
                        };
                        button
                            .build()
                            .on_click(move |ctx, _, _| ctx.dispatch_typed_action(action.clone()))
                            .finish()
                    }
                };
                row.add_child(element);
            }
            menu.add_child(row.finish());
        }
        Container::new(menu.finish())
            .with_uniform_padding(12.)
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.)))
            .with_background(theme.surface_2())
            .finish()
    }

    fn render_group(&self, group: &FleetGroup, appearance: &Appearance) -> Box<dyn Element> {
        let mut heading = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(SPACING)
            .with_child(
                Self::text(group.repo_name.clone(), appearance)
                    .with_style(Properties::default().weight(Weight::Bold))
                    .finish(),
            );
        if let Some(repo_url) = &group.repo_url {
            heading.add_child(
                Self::secondary_text(repo_url.clone(), appearance)
                    .with_clip(ClipConfig::start())
                    .finish(),
            );
        }
        let mut section = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(SPACING)
            .with_child(heading.finish());
        for row in &group.rows {
            if let Some(mouse_states) = self.row_mouse_states.get(&row.job_id) {
                section.add_child(self.render_row(row, mouse_states, appearance));
            }
        }
        section.finish()
    }

    fn render_row(
        &self,
        row: &FleetRow,
        mouse_states: &RowMouseStates,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        let theme = appearance.theme();
        let badge_background = if row.is_finished() {
            theme.surface_2()
        } else {
            theme.accent()
        };
        let badge = Container::new(Self::text(row.state.clone(), appearance).finish())
            .with_horizontal_padding(6.)
            .with_vertical_padding(2.)
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.)))
            .with_background(badge_background)
            .finish();
        let branch = match (&row.branch, &row.branch_url) {
            (Some(branch), Some(url)) => appearance
                .ui_builder()
                .link(
                    branch.clone(),
                    Some(url.clone()),
                    None,
                    mouse_states.branch.clone(),
                )
                .soft_wrap(false)
                .build()
                .finish(),
            (Some(branch), None) => Self::text(branch.clone(), appearance).finish(),
            (None, Some(_)) | (None, None) => {
                Self::secondary_text("no branch", appearance).finish()
            }
        };
        let details = Flex::column()
            .with_child(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(SPACING)
                    .with_child(badge)
                    .with_child(Self::text(row.job_id.clone(), appearance).finish())
                    .with_child(Self::secondary_text(row.agent.clone(), appearance).finish())
                    .finish(),
            )
            .with_child(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(SPACING)
                    .with_child(branch)
                    .with_child(
                        Self::secondary_text(
                            row.last_activity
                                .as_deref()
                                .map(|at| format!("last activity {at}"))
                                .unwrap_or_else(|| "no activity yet".to_owned()),
                            appearance,
                        )
                        .finish(),
                    )
                    .finish(),
            )
            .finish();

        let job_id = row.job_id.clone();
        let open = appearance
            .ui_builder()
            .button(ButtonVariant::Secondary, mouse_states.open.clone())
            .with_centered_text_label("Open".to_owned())
            .build()
            .on_click(move |ctx, _, _| {
                ctx.dispatch_typed_action(WorkspaceAction::OpenManagedSession {
                    job_id: job_id.clone(),
                })
            })
            .finish();
        let stop_label = if self.is_stop_armed(&row.job_id) {
            "Confirm stop"
        } else {
            "Stop"
        };
        let stop = Self::button(
            stop_label,
            &mouse_states.stop,
            (!row.is_finished()).then(|| FleetViewAction::Stop {
                job_id: row.job_id.clone(),
            }),
            appearance,
        );

        Container::new(
            Flex::row()
                .with_main_axis_size(MainAxisSize::Max)
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(SPACING)
                .with_child(Expanded::new(1., details).finish())
                .with_child(open)
                .with_child(stop)
                .finish(),
        )
        .with_uniform_padding(8.)
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.)))
        .with_background(theme.surface_1())
        .finish()
    }
}

impl Entity for FleetView {
    type Event = FleetViewEvent;
}

impl View for FleetView {
    fn ui_name() -> &'static str {
        "AgentCtrlFleetView"
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let model = self.model.as_ref(app);
        let mut content = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(12.)
            .with_child(self.render_toolbar(appearance, app));
        if let Some(error) = model.last_error() {
            content.add_child(
                Text::new(
                    error.to_owned(),
                    appearance.ui_font_family(),
                    appearance.ui_font_size(),
                )
                .with_color(appearance.theme().ui_error_color())
                .finish(),
            );
        }
        if let Some(error) = model.action_error() {
            content.add_child(
                Flex::row()
                    .with_main_axis_size(MainAxisSize::Max)
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(SPACING)
                    .with_child(
                        Expanded::new(
                            1.,
                            Text::new(
                                error.to_owned(),
                                appearance.ui_font_family(),
                                appearance.ui_font_size(),
                            )
                            .with_color(appearance.theme().ui_error_color())
                            .finish(),
                        )
                        .finish(),
                    )
                    .with_child(Self::button(
                        "Dismiss",
                        &self.dismiss_error_mouse_state,
                        Some(FleetViewAction::DismissActionError),
                        appearance,
                    ))
                    .finish(),
            );
        }
        if self.start_menu_open {
            content.add_child(self.render_start_menu(appearance, app));
        }
        let groups = model.groups();
        if groups.is_empty() && !model.is_refreshing() {
            content.add_child(
                Self::secondary_text("No runs on AgentCtrl staging.", appearance).finish(),
            );
        }
        for group in &groups {
            content.add_child(self.render_group(group, appearance));
        }

        ClippedScrollable::vertical(
            self.scroll_state.clone(),
            Container::new(content.finish())
                .with_uniform_padding(16.)
                .finish(),
            ScrollbarWidth::Auto,
            appearance.theme().nonactive_ui_detail().into(),
            appearance.theme().active_ui_detail().into(),
            warpui::elements::Fill::None,
        )
        .finish()
    }
}

impl TypedActionView for FleetView {
    type Action = FleetViewAction;

    fn handle_action(&mut self, action: &Self::Action, ctx: &mut ViewContext<Self>) {
        match action {
            FleetViewAction::Refresh => self.refresh(ctx),
            FleetViewAction::DismissActionError => self
                .model
                .update(ctx, |model, ctx| model.dismiss_action_error(ctx)),
            FleetViewAction::ToggleStartMenu => {
                self.start_menu_open = !self.start_menu_open;
                ctx.notify();
            }
            FleetViewAction::Start { repo_url, agent } => {
                self.start_menu_open = false;
                let repo_url = repo_url.clone();
                let agent = *agent;
                self.model
                    .update(ctx, |model, ctx| model.start(repo_url, agent, ctx));
                ctx.notify();
            }
            FleetViewAction::Stop { job_id } => self.stop(job_id.clone(), ctx),
        }
    }
}

impl BackingView for FleetView {
    type PaneHeaderOverflowMenuAction = FleetViewHeaderAction;
    type CustomAction = ();
    type AssociatedData = ();

    fn handle_pane_header_overflow_menu_action(
        &mut self,
        action: &Self::PaneHeaderOverflowMenuAction,
        _ctx: &mut ViewContext<Self>,
    ) {
        match *action {}
    }

    fn close(&mut self, ctx: &mut ViewContext<Self>) {
        ctx.emit(FleetViewEvent::Pane(PaneEvent::Close));
    }

    fn focus_contents(&mut self, ctx: &mut ViewContext<Self>) {
        self.focus(ctx);
    }

    fn render_header_content(
        &self,
        _ctx: &view::HeaderRenderContext<'_>,
        _app: &AppContext,
    ) -> HeaderContent {
        HeaderContent::Standard(StandardHeader {
            title: FLEET_HEADER_TEXT.to_owned(),
            title_secondary: None,
            title_style: None,
            title_clip_config: ClipConfig::start(),
            title_max_width: None,
            left_of_title: None,
            right_of_title: None,
            left_of_overflow: None,
            options: StandardHeaderOptions::default(),
        })
    }

    fn set_focus_handle(&mut self, focus_handle: PaneFocusHandle, _ctx: &mut ViewContext<Self>) {
        self.focus_handle = Some(focus_handle);
    }
}
