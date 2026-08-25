use warpui::elements::{
    Align, Container, CrossAxisAlignment, Flex, MouseStateHandle, ParentElement, Text,
};
use warpui::ui_components::button::ButtonVariant;
use warpui::ui_components::components::{Coords, UiComponent, UiComponentStyles};
use warpui::{AppContext, Element, Entity, SingletonEntity, TypedActionView, View, ViewContext};

use crate::appearance::Appearance;

/// Confirmation shown before restarting or changing a pane's shell while a
/// long-running foreground command (including a running CLI agent) is still
/// active in it. Names the running command so the choice is informed rather
/// than a generic "are you sure".
pub struct RestartShellConfirmationModal {
    cancel_mouse_state: MouseStateHandle,
    confirm_mouse_state: MouseStateHandle,
    running_command: Option<String>,
}

impl RestartShellConfirmationModal {
    pub fn new() -> Self {
        Self {
            cancel_mouse_state: Default::default(),
            confirm_mouse_state: Default::default(),
            running_command: None,
        }
    }

    /// Sets the display text for the command currently running in the pane
    /// being restarted.
    pub fn set_running_command(&mut self, running_command: String) {
        self.running_command = Some(running_command);
    }
}

impl Entity for RestartShellConfirmationModal {
    type Event = RestartShellConfirmationEvent;
}

impl View for RestartShellConfirmationModal {
    fn ui_name() -> &'static str {
        "RestartShellConfirmationModal"
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();

        let description = match self.running_command.as_deref() {
            Some(command) if !command.trim().is_empty() => format!(
                "\"{}\" is still running in this pane. Restarting the shell will end it. Continue?",
                command.trim()
            ),
            _ => "A command is still running in this pane. Restarting the shell will end it. \
                  Continue?"
                .to_string(),
        };

        let description_text = Text::new(description, appearance.ui_font_family(), 14.)
            .with_color(theme.sub_text_color(theme.surface_2()).into())
            .finish();

        let button_style = UiComponentStyles {
            font_size: Some(14.),
            padding: Some(Coords::uniform(8.).left(12.).right(12.)),
            ..Default::default()
        };

        let buttons_row = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(
                appearance
                    .ui_builder()
                    .button(ButtonVariant::Secondary, self.cancel_mouse_state.clone())
                    .with_text_label("Cancel".to_string())
                    .with_style(button_style)
                    .build()
                    .on_click(|ctx, _, _| {
                        ctx.dispatch_typed_action(RestartShellConfirmationAction::Cancel);
                    })
                    .finish(),
            )
            .with_child(
                Container::new(
                    appearance
                        .ui_builder()
                        .button(ButtonVariant::Accent, self.confirm_mouse_state.clone())
                        .with_text_label("Restart".to_string())
                        .with_style(button_style)
                        .build()
                        .on_click(|ctx, _, _| {
                            ctx.dispatch_typed_action(RestartShellConfirmationAction::Confirm);
                        })
                        .finish(),
                )
                .with_margin_left(12.)
                .finish(),
            )
            .finish();

        Flex::column()
            .with_child(
                Container::new(description_text)
                    .with_margin_bottom(24.)
                    .finish(),
            )
            .with_child(Align::new(buttons_row).right().finish())
            .finish()
    }
}

pub enum RestartShellConfirmationEvent {
    Confirm,
    Cancel,
}

#[derive(Debug)]
pub enum RestartShellConfirmationAction {
    Confirm,
    Cancel,
}

impl TypedActionView for RestartShellConfirmationModal {
    type Action = RestartShellConfirmationAction;

    fn handle_action(
        &mut self,
        action: &RestartShellConfirmationAction,
        ctx: &mut ViewContext<Self>,
    ) {
        match action {
            RestartShellConfirmationAction::Confirm => {
                ctx.emit(RestartShellConfirmationEvent::Confirm);
            }
            RestartShellConfirmationAction::Cancel => {
                ctx.emit(RestartShellConfirmationEvent::Cancel);
            }
        }
    }
}
