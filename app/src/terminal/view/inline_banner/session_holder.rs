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
    Attach,
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
    in_flight: bool,
    attaching: bool,
    attach_mouse_state: MouseStateHandle,
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
            in_flight: false,
            attaching: false,
            attach_mouse_state: Default::default(),
            take_over_mouse_state: Default::default(),
            start_fresh_mouse_state: Default::default(),
        }
    }

    pub fn set_in_flight(&mut self) {
        self.in_flight = true;
        self.attaching = false;
        self.error = None;
    }

    #[cfg(windows)]
    pub fn set_attaching(&mut self) {
        self.in_flight = true;
        self.attaching = true;
        self.error = None;
    }

    pub fn set_attach_error(&mut self, error: String) {
        self.attaching = true;
        self.set_error(error);
    }

    pub fn offers_attach(&self) -> bool {
        cfg!(windows)
            && self.restore.verdict == HeldVerdict::Detached
            && self.restore.holder.is_some()
            && self.restore.session_id.is_some()
    }

    pub fn set_error(&mut self, error: String) {
        self.in_flight = false;
        self.error = Some(error);
    }

    #[cfg(test)]
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    #[cfg(test)]
    pub fn in_flight(&self) -> bool {
        self.in_flight
    }

    pub fn title(&self) -> String {
        let (version, pid) = match &self.restore.holder {
            Some(holder) => (holder.version.as_str(), holder.pid.to_string()),
            None => ("unknown", "unknown".to_owned()),
        };
        match self.restore.verdict {
            HeldVerdict::Detached => format!("Still running under build {version} (pid {pid})"),
            HeldVerdict::Attached => format!("Open in another window (build {version}, pid {pid})"),
            HeldVerdict::HeldByUnknown => {
                "Still running in another Warp (holder unknown)".to_owned()
            }
        }
    }

    /// An attached holder has a window in use, so ending its session from here is not offered.
    pub fn offers_take_over(&self) -> bool {
        match self.restore.verdict {
            HeldVerdict::Detached | HeldVerdict::HeldByUnknown => true,
            HeldVerdict::Attached => false,
        }
    }

    fn body(&self) -> String {
        if self.in_flight {
            return if self.attaching {
                "Connecting to the existing session…"
            } else {
                "Ending the session there…"
            }
            .to_owned();
        }
        if let Some(error) = &self.error {
            return if self.attaching {
                format!("Attaching failed: {error}")
            } else {
                format!("Taking over failed: {error}")
            };
        }
        let command = &self.restore.command;
        match self.restore.verdict {
            HeldVerdict::Detached => {
                let attach = if self.offers_attach() {
                    "Attach reconnects without restarting them. "
                } else {
                    ""
                };
                format!(
                    "The agent and its shell are still alive in that build's session supervisor. \
                     {attach}Take over ends them there and runs `{command}` here. Start fresh leaves \
                     this shell as it is."
                )
            }
            HeldVerdict::Attached => "A window in that build is attached to this session. Start \
                                      fresh leaves this shell as it is; use that window to \
                                      continue."
                .to_owned(),
            HeldVerdict::HeldByUnknown => format!(
                "The agent is still running in another Warp. Take over ends the agent there and \
                 runs `{command}` here. Start fresh leaves this shell as it is."
            ),
        }
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
    if !state.in_flight && state.offers_attach() {
        buttons.push(button(
            "Attach",
            SessionHolderBannerAction::Attach,
            &state.attach_mouse_state,
            InlineBannerTextButtonVariant::Primary,
        ));
    }
    if !state.in_flight && state.offers_take_over() {
        buttons.push(button(
            "Take over",
            SessionHolderBannerAction::TakeOver,
            &state.take_over_mouse_state,
            if state.offers_attach() {
                InlineBannerTextButtonVariant::Secondary
            } else {
                InlineBannerTextButtonVariant::Primary
            },
        ));
    }
    if !state.in_flight {
        buttons.push(button(
            "Start fresh",
            SessionHolderBannerAction::StartFresh,
            &state.start_fresh_mouse_state,
            InlineBannerTextButtonVariant::Secondary,
        ));
    }

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
                state.body(),
                appearance.ui_font_family(),
                appearance.ui_font_size(),
            )]),
            buttons,
            ..Default::default()
        },
    )
}

#[cfg(all(test, feature = "local_tty"))]
#[path = "session_holder_tests.rs"]
mod tests;
