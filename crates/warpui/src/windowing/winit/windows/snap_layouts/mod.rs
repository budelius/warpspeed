//! Windows 11 Snap Layouts support for Warp's app-drawn maximize button.
//!
//! Warp's windows are undecorated and the caption buttons are drawn by the app, so Windows never
//! sees the `HTMAXBUTTON` hit-test answer that makes it offer the Snap Layouts flyout. The window
//! subclass in this module supplies that answer, and replays the non-client mouse messages that
//! follow as ordinary client-area ones so the app-drawn button keeps behaving normally.

mod logic;

pub use logic::*;
