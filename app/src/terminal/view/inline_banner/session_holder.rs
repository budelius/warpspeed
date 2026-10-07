#[cfg(feature = "local_tty")]
use warpui::Element;
#[cfg(feature = "local_tty")]
use warpui::elements::{MouseStateHandle, Text};
#[cfg(feature = "local_tty")]
use warpui::fonts::Weight;

#[cfg(feature = "local_tty")]
use super::{
    InlineBannerButtonState, InlineBannerContent, InlineBannerIcon, InlineBannerStyle,
    InlineBannerTextButton, InlineBannerTextButtonFont, InlineBannerTextButtonVariant,
    render_inline_block_list_banner,
};
#[cfg(feature = "local_tty")]
use crate::appearance::Appearance;
#[cfg(feature = "local_tty")]
use crate::terminal::terminal_manager::{HeldVerdict, WithheldRestore};
#[cfg(feature = "local_tty")]
use crate::terminal::view::{InlineBannerId, TerminalAction};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionHolderBannerAction {
    TakeOver,
    StartFresh,
}

/// The prompt shown while a restore command is withheld for the build that still holds the
/// session.
#[cfg(feature = "local_tty")]
pub struct SessionHolderBannerState {
    pub id: InlineBannerId,
    restore: WithheldRestore,
    error: Option<String>,
    take_over_mouse_state: MouseStateHandle,
    start_fresh_mouse_state: MouseStateHandle,
}

#[cfg(feature = "local_tty")]
impl SessionHolderBannerState {
    pub fn new(id: InlineBannerId, restore: WithheldRestore) -> Self {
        Self {
            id,
            restore,
            error: None,
            take_over_mouse_state: Default::default(),
            start_fresh_mouse_state: Default::default(),
        }
    }

    pub fn set_error(&mut self, error: String) {
        self.error = Some(error);
    }

    pub fn title(&self) -> String {
        let holder = &self.restore.holder;
        match self.restore.verdict {
            HeldVerdict::Detached => format!(
                "Still running under build {} (pid {})",
                holder.version, holder.pid
            ),
            HeldVerdict::Attached => format!(
                "Open in another window (build {}, pid {})",
                holder.version, holder.pid
            ),
        }
    }

    /// An attached holder has a window in use, so ending its session from here is not offered.
    pub fn offers_take_over(&self) -> bool {
        self.restore.verdict == HeldVerdict::Detached
    }
}

#[cfg(feature = "local_tty")]
pub fn render_session_holder_banner(
    state: &SessionHolderBannerState,
    appearance: &Appearance,
) -> Box<dyn Element> {
    let text_color = appearance.theme().active_ui_text_color().into_solid();
    let button = |text: &str, action, handle: &MouseStateHandle, variant| InlineBannerTextButton {
        text: text.to_owned(),
        text_color,
        button_state: InlineBannerButtonState {
            on_click_event: TerminalAction::SessionHolderBanner(action),
            mouse_state_handle: handle.clone(),
        },
        font: InlineBannerTextButtonFont {
            weight: Some(Weight::Bold),
            ..Default::default()
        },
        position_id: None,
        variant,
    };
    let mut buttons = Vec::new();
    if state.offers_take_over() {
        buttons.push(button(
            "Take over",
            SessionHolderBannerAction::TakeOver,
            &state.take_over_mouse_state,
            InlineBannerTextButtonVariant::Primary,
        ));
    }
    buttons.push(button(
        "Start fresh",
        SessionHolderBannerAction::StartFresh,
        &state.start_fresh_mouse_state,
        InlineBannerTextButtonVariant::Secondary,
    ));

    let body = match (&state.error, state.restore.verdict) {
        (Some(error), _) => format!("Taking over failed: {error}"),
        (None, HeldVerdict::Detached) => format!(
            "The agent and its shell are still alive in that build's session supervisor. Take over \
             ends them there and runs `{}` here.",
            state.restore.command
        ),
        (None, HeldVerdict::Attached) => format!(
            "A window in that build is attached to this session. Start fresh runs `{}` here \
             without touching it.",
            state.restore.command
        ),
    };

    render_inline_block_list_banner(
        InlineBannerStyle::CallToAction,
        appearance,
        InlineBannerContent {
            title: state.title(),
            header_icon: Some(InlineBannerIcon {
                asset_path: "bundled/svg/warning.svg",
                aspect_ratio: 1.,
                color_override: Some(appearance.theme().foreground().into_solid()),
            }),
            content: Some(vec![Text::new(
                body,
                appearance.ui_font_family(),
                appearance.ui_font_size(),
            )]),
            buttons,
            ..Default::default()
        },
    )
}
