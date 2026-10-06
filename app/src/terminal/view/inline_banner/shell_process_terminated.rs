use warpui::elements::{MouseStateHandle, Text};
use warpui::fonts::Weight;
use warpui::{Element, EntityId};

use super::{
    InlineBannerButtonState, InlineBannerContent, InlineBannerIcon, InlineBannerStyle,
    InlineBannerTextButton, InlineBannerTextButtonFont, InlineBannerTextButtonVariant,
    render_inline_block_list_banner,
};
use crate::appearance::Appearance;
use crate::terminal::view::{InlineBannerId, TerminalAction};

/// A pane whose connection to its shell was lost while the shell kept running in the supervisor.
pub struct ShellDetachedBannerState {
    pub id: InlineBannerId,
    reattach_button_mouse_state: MouseStateHandle,
}

impl ShellDetachedBannerState {
    pub fn new(id: InlineBannerId) -> Self {
        Self {
            id,
            reattach_button_mouse_state: Default::default(),
        }
    }
}

pub fn render_shell_detached_banner(
    state: &ShellDetachedBannerState,
    view_id: EntityId,
    appearance: &Appearance,
) -> Box<dyn Element> {
    let reattach_button = InlineBannerTextButton {
        text: "Reattach".to_string(),
        text_color: appearance.theme().active_ui_text_color().into_solid(),
        button_state: InlineBannerButtonState {
            on_click_event: TerminalAction::ReattachShell,
            mouse_state_handle: state.reattach_button_mouse_state.clone(),
        },
        font: InlineBannerTextButtonFont {
            weight: Some(Weight::Bold),
            ..Default::default()
        },
        position_id: Some(format!("shell_detached_banner_reattach_button_{view_id}")),
        variant: InlineBannerTextButtonVariant::Primary,
    };

    render_inline_block_list_banner(
        InlineBannerStyle::CallToAction,
        appearance,
        InlineBannerContent {
            title: "Lost the connection to this shell".to_string(),
            header_icon: Some(InlineBannerIcon {
                asset_path: "bundled/svg/warning.svg",
                aspect_ratio: 1.,
                color_override: Some(appearance.theme().foreground().into_solid()),
            }),
            content: Some(vec![Text::new(
                "The shell is still running in the session supervisor, with everything started in it.",
                appearance.ui_font_family(),
                appearance.ui_font_size(),
            )]),
            buttons: vec![reattach_button],
            ..Default::default()
        },
    )
}

pub fn render_shell_process_terminated_banner(
    appearance: &Appearance,
    was_premature_termination: bool,
) -> Box<dyn Element> {
    if was_premature_termination {
        render_inline_block_list_banner(
            InlineBannerStyle::CallToAction,
            appearance,
            InlineBannerContent {
                title: "Shell process exited prematurely!".to_string(),
                header_icon: Some(InlineBannerIcon {
                    asset_path: "bundled/svg/warning.svg",
                    aspect_ratio: 1.,
                    color_override: Some(appearance.theme().foreground().into_solid()),
                }),
                content: Some(vec![Text::new(
                    "The output from Warp's initialization script is visible above to assist with debugging.",
                    appearance.ui_font_family(),
                    appearance.ui_font_size(),
                )]),
                ..Default::default()
            },
        )
    } else {
        render_inline_block_list_banner(
            InlineBannerStyle::LowPriority,
            appearance,
            InlineBannerContent {
                title: "Shell process exited".to_string(),
                header_icon: Some(InlineBannerIcon {
                    asset_path: "bundled/svg/info.svg",
                    aspect_ratio: 1.,
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
    }
}
