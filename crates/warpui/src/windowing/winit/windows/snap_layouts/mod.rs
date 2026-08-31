//! Windows 11 Snap Layouts support for Warp's app-drawn maximize button.
//!
//! Warp's windows are undecorated and the caption buttons are drawn by the app, so Windows never
//! sees the `HTMAXBUTTON` hit-test answer that makes it offer the Snap Layouts flyout. The window
//! subclass in this module supplies that answer, and replays the non-client mouse messages that
//! follow as ordinary client-area ones so the app-drawn button keeps behaving normally.

use std::sync::Mutex;

mod logic;

pub use logic::*;

/// The state the window subclass reads while answering the hit test.
///
/// Shared between the warpui window, which publishes the button's bounds as the app paints it, and
/// the subclass, which runs inside the window procedure.
#[derive(Debug, Default)]
pub struct SnapLayoutsState {
    button: Mutex<Option<ButtonRect>>,
}

impl SnapLayoutsState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Publishes the maximize button's bounds, or `None` when no maximize button is on screen.
    pub fn set_button_rect(&self, rect: Option<ButtonRect>) {
        if let Ok(mut button) = self.button.lock() {
            *button = rect;
        }
    }

    /// The maximize button's bounds, if one is on screen.
    pub fn button_rect(&self) -> Option<ButtonRect> {
        self.button.lock().ok().and_then(|button| *button)
    }
}
