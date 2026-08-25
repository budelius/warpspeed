use markdown_parser::{
    FormattedText, FormattedTextFragment, FormattedTextInline, FormattedTextLine,
};
use pathfinder_geometry::vector::vec2f;
use serde::{Deserialize, Serialize};
use warp_core::paths::home_relative_path;
use warp_core::safe_warn;
use warp_core::ui::theme::Fill;
use warpui::accessibility::{AccessibilityContent, WarpA11yRole};
use warpui::elements::{
    Align, Border, ChildAnchor, ChildView, Clipped, ConstrainedBox, Container, CornerRadius,
    Element, Empty, Flex, FormattedTextElement, MouseStateHandle, OffsetPositioning, ParentAnchor,
    ParentElement, ParentOffsetBounds, Radius, SavePosition, Shrinkable, Stack, Text,
};
use warpui::keymap::FixedBinding;
use warpui::ui_components::button::{Button, ButtonVariant};
use warpui::ui_components::components::{Coords, UiComponent, UiComponentStyles};
use warpui::{
    AppContext, Entity, FocusContext, SingletonEntity, TypedActionView, View, ViewContext,
    ViewHandle,
};

use crate::app_state::WindowSnapshot;
use crate::appearance::Appearance;
use crate::editor::{
    EditorView, Event as EditorEvent, PropagateAndNoOpNavigationKeys, SingleLineEditorOptions,
};
use crate::features::FeatureFlag;
use crate::launch_configs::launch_config::{
    LaunchConfig, LaunchConfigSource, LaunchConfigWindowSource, WindowTemplate,
};
use crate::send_telemetry_from_ctx;
use crate::server::telemetry::TelemetryEvent;
use crate::user_config::launch_configs_dir;
#[cfg(feature = "local_fs")]
use crate::user_config::{WarpConfig, util::file_name_to_human_readable_name};
use crate::util::bindings::keybinding_name_to_display_string;
#[cfg(feature = "local_fs")]
use crate::util::openable_file_type::FileTarget;

const MODAL_WIDTH: f32 = 660.;
const SIDE_PADDING: f32 = 16.;
const BUTTON_SIZE: f32 = 24.;
const DOC_LINK_WIDTH: f32 = 120.;
const SAVE_CONFIG_BUTTON_LABEL: &str = "Save Configuration";
const UPDATE_CONFIG_BUTTON_LABEL: &str = "Update Configuration";
const OPEN_FILE_BUTTON_LABEL: &str = "Open YAML File";

pub fn init(app: &mut AppContext) {
    use warpui::keymap::macros::*;

    app.register_fixed_bindings([FixedBinding::new(
        "escape",
        ActionRequest::Action(LaunchConfigSaveAction::Close),
        id!(LaunchConfigSaveModal::ui_name()),
    )]);

    app.register_fixed_bindings([FixedBinding::new(
        "enter",
        ActionRequest::Enter,
        id!(LaunchConfigSaveModal::ui_name()),
    )]);
}

#[derive(Default)]
struct SaveModalMouseStates {
    close_button_state: MouseStateHandle,
    documentation_link_state: MouseStateHandle,
    save_button_state: MouseStateHandle,
    open_file_button_state: MouseStateHandle,
}

/// Saves the current window as a new or existing launch configuration.
pub struct LaunchConfigSaveModal {
    editor: ViewHandle<EditorView>,
    mouse_states: SaveModalMouseStates,
    current_window_snapshot: Option<WindowSnapshot>,
    launch_config_source: Option<LaunchConfigWindowSource>,
    save_state: SaveState,
    file_path: Option<std::path::PathBuf>,
    open_modal_keybinding_str: String,
}

/// Keeps track of the current lifecycle state of the modal.
#[derive(PartialEq, Eq, Clone, Serialize, Deserialize)]
pub enum SaveState {
    /// Succeeds to save.
    Success,
    /// Fails to save
    Failure(FailureType),
    /// Not yet saved
    NotSaved,
}

#[derive(PartialEq, Eq, Clone, Serialize, Deserialize, Copy)]
pub enum FailureType {
    FileAlreadyExists,
    Other,
}

#[derive(Debug, PartialEq, Clone)]
pub enum ActionRequest {
    Action(LaunchConfigSaveAction),
    Enter,
}

#[derive(Debug, PartialEq, Clone)]
pub enum LaunchConfigSaveAction {
    Close,
    Save,
    OpenFile,
}

impl LaunchConfigSaveAction {
    pub fn from_state(save_state: &SaveState) -> Self {
        match save_state {
            SaveState::Success => Self::OpenFile,
            SaveState::NotSaved => Self::Save,
            SaveState::Failure(_) => Self::Close,
        }
    }
}

#[derive(PartialEq, Eq)]
pub enum LaunchConfigModalEvent {
    /// Users who have just created a launch config want to verify that the launch config is saved
    /// and test opening the config. When they don't see it, they assume there is a bug.
    /// SuccessfullySavedConfig event addresses it.
    /// It's called when the new config was just saved. Note that when we save the configuration,
    /// it take a moment for the file system to register the change, and us to receive it (as there
    /// is a delay in our watcher). But because we actually have the LaunchConfig in our hands
    /// already, we may as well save it "manually" to the WarpConfig, while waiting for the update
    /// from the file system. This event passes a saved config to the handler to let us do that.
    SuccessfullySavedConfig(LaunchConfig),
    #[cfg(feature = "local_fs")]
    OpenFileWithTarget {
        path: std::path::PathBuf,
        target: FileTarget,
        line_col: Option<warp_util::path::LineAndColumnArg>,
    },
    Close,
}

impl Entity for LaunchConfigSaveModal {
    type Event = LaunchConfigModalEvent;
}

impl LaunchConfigSaveModal {
    pub fn new(ctx: &mut ViewContext<Self>) -> Self {
        let editor = {
            let options = SingleLineEditorOptions {
                propagate_and_no_op_vertical_navigation_keys:
                    PropagateAndNoOpNavigationKeys::Always,
                ..Default::default()
            };
            ctx.add_typed_action_view(|ctx| EditorView::single_line(options, ctx))
        };
        ctx.subscribe_to_view(&editor, |me, _, event, ctx| {
            me.handle_editor_event(event, ctx);
        });

        LaunchConfigSaveModal {
            editor,
            mouse_states: Default::default(),
            current_window_snapshot: None,
            launch_config_source: None,
            save_state: SaveState::NotSaved,
            file_path: None,
            open_modal_keybinding_str: keybinding_name_to_display_string(
                "workspace:toggle_launch_config_palette",
                ctx,
            )
            .unwrap_or_default(),
        }
    }

    pub fn set_window_snapshot(
        &mut self,
        window_snapshot: WindowSnapshot,
        launch_config_source: Option<LaunchConfigWindowSource>,
    ) {
        self.current_window_snapshot = Some(window_snapshot);
        self.launch_config_source = launch_config_source;
    }

    fn handle_editor_event(&mut self, event: &EditorEvent, ctx: &mut ViewContext<Self>) {
        match event {
            EditorEvent::Enter => {
                self.handle_action(&ActionRequest::Enter, ctx);
            }
            EditorEvent::Escape => {
                self.handle_action(&ActionRequest::Action(LaunchConfigSaveAction::Close), ctx)
            }
            _ => ctx.notify(),
        }
    }

    /// Open the saved file if the modal is in the correct state
    #[cfg(feature = "local_fs")]
    fn open_file(&self, ctx: &mut ViewContext<Self>) {
        use crate::util::file::external_editor::EditorSettings;
        use crate::util::openable_file_type::resolve_file_target;

        if let SaveState::Success = &self.save_state
            && let Some(file_path) = &self.file_path
        {
            let file_path = file_path.clone();
            // Resolve target and emit event - workspace will handle all cases
            let settings = EditorSettings::as_ref(ctx);
            let target = resolve_file_target(&file_path, settings, None);
            ctx.emit(LaunchConfigModalEvent::OpenFileWithTarget {
                path: file_path,
                target,
                line_col: None,
            });
            send_telemetry_from_ctx!(TelemetryEvent::OpenLaunchConfigFile, ctx);
        }
    }

    #[cfg(feature = "local_fs")]
    fn try_save_launch_config(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(window_snapshot) = self.current_window_snapshot.as_ref() else {
            self.failed_save(FailureType::Other, ctx);
            return;
        };

        let result = if let Some(source) = self.launch_config_source.as_ref() {
            let window_template = WindowTemplate::from(window_snapshot.clone());
            WarpConfig::overwrite_launch_config_window(source, window_template).and_then(
                |launch_config| {
                    if FeatureFlag::PersistentLaunchConfigSessions.is_enabled() {
                        crate::launch_configs::session_store::save(source, window_snapshot)?;
                    }
                    Ok((source.path.clone(), launch_config))
                },
            )
        } else {
            let file_name_candidate = self.editor.as_ref(ctx).buffer_text(ctx);
            let launch_config_name = file_name_to_human_readable_name(&file_name_candidate);
            let mut launch_config =
                LaunchConfig::from_window_snapshot(launch_config_name, window_snapshot);
            WarpConfig::save_new_launch_config(file_name_candidate, launch_config.clone()).and_then(
                |file_name| {
                    let path = launch_configs_dir().join(file_name);
                    let source = LaunchConfigSource {
                        path: path.clone(),
                        document_index: 0,
                    };
                    launch_config.source = Some(source);
                    if FeatureFlag::PersistentLaunchConfigSessions.is_enabled() {
                        let window_source = launch_config
                            .window_source(0)
                            .expect("new launch configuration has a source");
                        crate::launch_configs::session_store::save(
                            &window_source,
                            window_snapshot,
                        )?;
                    }
                    Ok((path, launch_config))
                },
            )
        };

        match result {
            Ok((path, launch_config)) => {
                self.saved_successfully(path, ctx);
                ctx.emit(LaunchConfigModalEvent::SuccessfullySavedConfig(
                    launch_config,
                ));
            }
            Err(error) => {
                safe_warn!(
                    safe: ("Failed to save current window as launch configuration"),
                    full: ("Failed to save current window as launch configuration: {error:#}")
                );
                if error.to_string().contains("File already exists") {
                    self.failed_save(FailureType::FileAlreadyExists, ctx);
                } else {
                    self.failed_save(FailureType::Other, ctx);
                }
            }
        }
        ctx.notify();
    }

    /// Clears the editor
    pub fn reset_editor(&mut self, ctx: &mut ViewContext<Self>) {
        self.editor.update(ctx, |editor, ctx| {
            editor.clear_buffer_and_reset_undo_stack(ctx);
            editor.set_placeholder_text("launch_config.yaml", ctx);
        });
    }

    /// Closes the modal
    pub fn close(&mut self, ctx: &mut ViewContext<Self>) {
        self.reset_editor(ctx); // clears editor and resets placeholder
        self.current_window_snapshot = None;
        self.launch_config_source = None;
        self.file_path = None;
        self.save_state = SaveState::NotSaved;
        ctx.notify();
        ctx.emit(LaunchConfigModalEvent::Close);
    }

    fn save_modal_button(
        &self,
        appearance: &Appearance,
        button_text: String,
        mouse_state: MouseStateHandle,
        disabled: bool,
    ) -> Button {
        let button = appearance
            .ui_builder()
            .button(ButtonVariant::Accent, mouse_state)
            .with_centered_text_label(button_text)
            .with_style(UiComponentStyles {
                padding: Some(Coords {
                    top: 10.,
                    bottom: 10.,
                    left: 20.,
                    right: 20.,
                }),
                ..Default::default()
            });

        if disabled { button.disabled() } else { button }
    }

    fn render_save_config_button(
        &self,
        appearance: &Appearance,
        disabled: bool,
    ) -> Box<dyn Element> {
        self.save_modal_button(
            appearance,
            if self.launch_config_source.is_some() {
                UPDATE_CONFIG_BUTTON_LABEL.to_owned()
            } else {
                SAVE_CONFIG_BUTTON_LABEL.to_owned()
            },
            self.mouse_states.save_button_state.clone(),
            disabled,
        )
        .build()
        .on_click(move |ctx, _, _| {
            ctx.dispatch_typed_action(ActionRequest::Action(LaunchConfigSaveAction::Save));
        })
        .finish()
    }

    fn render_open_file_button(&self, appearance: &Appearance) -> Box<dyn Element> {
        self.save_modal_button(
            appearance,
            OPEN_FILE_BUTTON_LABEL.to_owned(),
            self.mouse_states.open_file_button_state.clone(),
            false,
        )
        .build()
        .on_click(move |ctx, _, _| {
            ctx.dispatch_typed_action(ActionRequest::Action(LaunchConfigSaveAction::OpenFile));
        })
        .finish()
    }

    /// Helper function to add padding to a button
    fn add_button_padding(container: Container) -> Container {
        container
            .with_padding_top(20.)
            .with_padding_bottom(20.)
            .with_padding_left(SIDE_PADDING)
            .with_padding_right(SIDE_PADDING)
    }

    /// Renders the button based on the state the modal is in
    fn render_button_row(
        &self,
        appearance: &Appearance,
        save_config_disabled: bool,
    ) -> Box<dyn Element> {
        match LaunchConfigSaveAction::from_state(&self.save_state) {
            LaunchConfigSaveAction::OpenFile => {
                Self::add_button_padding(Container::new(self.render_open_file_button(appearance)))
                    .finish()
            }
            LaunchConfigSaveAction::Save => Self::add_button_padding(Container::new(
                self.render_save_config_button(appearance, save_config_disabled),
            ))
            .finish(),
            LaunchConfigSaveAction::Close => Empty::new().finish(),
        }
    }

    /// Renders a generic text block in a span
    fn render_text_block(&self, appearance: &Appearance, text: String) -> Container {
        Container::new(
            appearance
                .ui_builder()
                .span(text)
                .with_soft_wrap()
                .build()
                .finish(),
        )
        .with_padding_left(SIDE_PADDING)
        .with_padding_right(SIDE_PADDING)
    }

    fn render_formatted_text_line(
        &self,
        appearance: &Appearance,
        formatted_text_fragments: FormattedTextInline,
    ) -> Container {
        let formatted_text =
            FormattedText::new([FormattedTextLine::Line(formatted_text_fragments)]);
        Container::new(
            FormattedTextElement::new(
                formatted_text,
                appearance.ui_font_size(),
                appearance.ui_font_family(),
                appearance.monospace_font_family(),
                appearance.theme().active_ui_text_color().into(),
                Default::default(),
            )
            .finish(),
        )
        .with_padding_left(SIDE_PADDING)
        .with_padding_right(SIDE_PADDING)
    }

    /// Renders the title of the modal
    fn render_header(&self, appearance: &Appearance) -> Box<dyn Element> {
        let header = Flex::row()
            .with_child(
                Shrinkable::new(
                    1.0,
                    Align::new(
                        Text::new_inline(
                            if let Some(source) = &self.launch_config_source {
                                format!("Update {}", source.config_name)
                            } else {
                                "Save Current Window".to_string()
                            },
                            appearance.header_font_family(),
                            appearance.header_font_size(),
                        )
                        .with_color(appearance.theme().active_ui_text_color().into())
                        .finish(),
                    )
                    .left()
                    .finish(),
                )
                .finish(),
            )
            .with_child(
                appearance
                    .ui_builder()
                    .close_button(BUTTON_SIZE, self.mouse_states.close_button_state.clone())
                    .build()
                    .on_click(|ctx, _, _| {
                        ctx.dispatch_typed_action(ActionRequest::Action(
                            LaunchConfigSaveAction::Close,
                        ))
                    })
                    .finish(),
            )
            .finish();

        Container::new(header)
            .with_corner_radius(CornerRadius::with_top(Radius::Pixels(8.)))
            .with_padding_left(SIDE_PADDING)
            .with_padding_right(SIDE_PADDING)
            .with_padding_top(SIDE_PADDING)
            .with_padding_bottom(8.)
            .finish()
    }

    pub fn saved_successfully(
        &mut self,
        file_path: std::path::PathBuf,
        ctx: &mut ViewContext<Self>,
    ) {
        self.set_save_state(SaveState::Success, Some(file_path));
        send_telemetry_from_ctx!(
            TelemetryEvent::SaveLaunchConfig {
                state: SaveState::Success,
            },
            ctx
        );
        ctx.notify();
    }

    pub fn failed_save(&mut self, failure_type: FailureType, ctx: &mut ViewContext<Self>) {
        self.set_save_state(SaveState::Failure(failure_type), None);
        send_telemetry_from_ctx!(
            TelemetryEvent::SaveLaunchConfig {
                state: SaveState::Failure(failure_type)
            },
            ctx
        );
        ctx.notify();
    }

    /// Renders the editor portion of the modal
    fn render_editor(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let editor = self.editor.as_ref(app);
        let height = editor.line_height(app.font_cache(), appearance);
        let editor =
            ConstrainedBox::new(Clipped::new(ChildView::new(&self.editor).finish()).finish())
                .with_height(height)
                .finish();
        Container::new(editor)
            .with_uniform_padding(SIDE_PADDING)
            // TODO theme should be agnostic of different UI elements / features
            .with_background(appearance.theme().background())
            .finish()
    }

    /// Renders the entire modal
    fn render_modal(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);

        // Title of modal
        let header = Flex::column().with_child(
            ConstrainedBox::new(self.render_header(appearance))
                .with_max_height(60.)
                .finish(),
        );

        let link_to_docs = Container::new(
            ConstrainedBox::new(
                appearance
                    .ui_builder()
                    .link(
                        "Link to Documentation".to_string(),
                        Some(
                            "https://docs.warp.dev/terminal/sessions/launch-configurations"
                                .to_string(),
                        ),
                        None,
                        self.mouse_states.documentation_link_state.clone(),
                    )
                    .soft_wrap(false)
                    .build()
                    .finish(),
            )
            .with_width(DOC_LINK_WIDTH)
            .finish(),
        )
        .with_uniform_padding(SIDE_PADDING)
        .finish();

        let info = match &self.save_state {
            SaveState::Success => header
                .with_child(
                    self.render_formatted_text_line(appearance, vec![
                        FormattedTextFragment::plain_text("Saved successfully to "),
                        FormattedTextFragment::inline_code(
                            self.file_path
                                .as_deref()
                                .map(home_relative_path)
                                .unwrap_or_default()
                        ),
                        FormattedTextFragment::plain_text(".")
                    ])
                    .with_padding_bottom(24.)
                    .finish(),
                )
                .with_child(link_to_docs),
            SaveState::Failure(failure_type) => header.with_child(
                self.render_text_block(
                    appearance,
                    match failure_type {
                        FailureType::FileAlreadyExists => {
                            "Failed to save. A launch configuration with the same name already exists.".to_string()
                        }
                        FailureType::Other => "An issue was encountered while saving.".to_string(),
                    },
                )
                .with_padding_bottom(24.)
                .finish(),
            ),
            SaveState::NotSaved => {
                if let Some(source) = &self.launch_config_source {
                    header
                        .with_child(
                            self.render_formatted_text_line(
                                appearance,
                                vec![FormattedTextFragment::plain_text(format!(
                                    "This replaces only the window opened from {}. Other Warp windows and launch configurations are not included. Shell and agent recovery state continues saving automatically.",
                                    source.config_name
                                ))],
                            )
                            .with_padding_bottom(24.)
                            .finish(),
                        )
                        .with_child(link_to_docs)
                } else {
                    let mut text = "This saves the current window's tabs and panes so you can easily open it again. Shell and agent recovery state will save automatically".to_string();
                    if self.open_modal_keybinding_str.is_empty() {
                        text.push('.');
                    } else {
                        text.push_str(&format!(" with {}.", self.open_modal_keybinding_str));
                    }
                    header
                        .with_child(
                            self.render_formatted_text_line(
                                appearance,
                                vec![FormattedTextFragment::plain_text(text)],
                            )
                            .finish(),
                        )
                        .with_child(
                            self.render_formatted_text_line(appearance, vec![
                                FormattedTextFragment::plain_text("\nThe YAML file is saved to "),
                                FormattedTextFragment::inline_code(home_relative_path(&launch_configs_dir())),
                                FormattedTextFragment::plain_text("."),
                            ])
                            .with_padding_bottom(24.)
                            .finish(),
                        )
                        .with_child(link_to_docs)
                        .with_child(self.render_editor(app))
                }
            }
        };

        ConstrainedBox::new(
            Container::new(
                info.with_child(self.render_button_row(
                    appearance,
                    self.launch_config_source.is_none() && self.editor.as_ref(app).is_empty(app),
                ))
                .finish(),
            )
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.)))
            .with_border(Border::all(1.).with_border_fill(appearance.theme().outline()))
            .with_uniform_margin(35.)
            .with_background(appearance.theme().surface_2())
            .finish(),
        )
        .with_width(MODAL_WIDTH)
        .finish()
    }

    fn set_save_state(&mut self, save_state: SaveState, file_path: Option<std::path::PathBuf>) {
        self.save_state = save_state;
        if let Some(file_path) = file_path {
            self.file_path = Some(file_path);
        }
    }
}

impl View for LaunchConfigSaveModal {
    fn ui_name() -> &'static str {
        "LaunchConfigSaveModal"
    }

    fn on_focus(&mut self, focus_ctx: &FocusContext, ctx: &mut ViewContext<Self>) {
        if focus_ctx.is_self_focused() {
            if self.launch_config_source.is_none() {
                ctx.focus(&self.editor);
            }
            ctx.notify();
        }
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        // renders the modal in a stack
        let mut stack = Stack::new();
        stack.add_positioned_child(
            SavePosition::new(self.render_modal(app), "save_modal:modal").finish(),
            OffsetPositioning::offset_from_parent(
                vec2f(0., 0.),
                ParentOffsetBounds::WindowByPosition,
                ParentAnchor::Center,
                ChildAnchor::Center,
            ),
        );

        Container::new(Align::new(stack.finish()).finish())
            .with_background_color(Fill::blur().into())
            .with_corner_radius(app.windows().window_corner_radius())
            .finish()
    }

    fn accessibility_contents(&self, _ctx: &AppContext) -> Option<AccessibilityContent> {
        Some(AccessibilityContent::new(
            "Save Config Modal",
            "Save the current window as a new launch configuration or update the launch
            configuration that opened it. Use enter to save and escape to close the modal.",
            WarpA11yRole::PopoverRole,
        ))
    }
}

impl TypedActionView for LaunchConfigSaveModal {
    type Action = ActionRequest;

    fn handle_action(&mut self, action: &ActionRequest, ctx: &mut ViewContext<Self>) {
        // TODO(vorporeal): We should figure out a better way to handle the
        // interactions with the filesystem here, whether it's compiling out
        // the save modal more completely or doing something else.  Perhaps
        // this will become moot when we put launch configs in Warp Drive.
        let action = match action {
            ActionRequest::Action(action) => action.clone(),
            ActionRequest::Enter => LaunchConfigSaveAction::from_state(&self.save_state),
        };
        match action {
            LaunchConfigSaveAction::Close => self.close(ctx),
            LaunchConfigSaveAction::Save => {
                #[cfg(feature = "local_fs")]
                if self.launch_config_source.is_some() || !self.editor.as_ref(ctx).is_empty(ctx) {
                    self.try_save_launch_config(ctx);
                }
            }
            LaunchConfigSaveAction::OpenFile => {
                #[cfg(feature = "local_fs")]
                self.open_file(ctx);
                self.close(ctx);
            }
        }
    }
}
