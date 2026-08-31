//! A terminal screen maintained with no UI attached.
//!
//! The session supervisor uses this to keep a detached session's screen current
//! while the app is not attached, and to hand that screen back on reattach. The
//! wire form is [`ScreenSnapshot`] -- a repaint byte sequence rather than a
//! serialized grid, so the protocol never has to version a cell format.
//!
//! Two grids are held, not one. `GridHandler` panics on
//! [`ansi::Mode::SwapScreen`] with "Handled in model layer": in the app the
//! primary/alternate swap belongs to `TerminalModel`, which owns both screens.
//! The supervisor has no `TerminalModel`, so [`HeadlessScreen`] owns the swap
//! itself -- see the [`ansi::Handler`] impl in `screen_handler.rs`.

use crate::SizeInfo;
use crate::event_listener::ChannelEventListener;
use crate::model::grid::grid_handler::{GridHandler, PerformResetGridChecks};
use crate::model::grid::{Dimensions, RespectDisplayedOutput, render_ansi};
use crate::model::secrets::{ObfuscateSecrets, RespectObfuscatedSecrets};
use crate::model::{Point, ansi};

/// A screen in a form that can be sent over the supervisor protocol.
///
/// `repaint` is a self-contained ANSI sequence that reconstructs the screen when
/// fed to any terminal: the screen mode, an erase, the visible text with its
/// attributes, then the cursor placed where it belongs.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ScreenSnapshot {
    /// Screen width in columns.
    pub columns: u16,

    /// Screen height in rows.
    pub rows: u16,

    /// The repaint byte sequence.
    pub repaint: Vec<u8>,
}

/// A terminal screen driven by bytes, with no view attached.
pub struct HeadlessScreen {
    primary: GridHandler,
    alternate: GridHandler,
    alternate_active: bool,
    processor: ansi::Processor,
    columns: u16,
    rows: u16,
}

impl HeadlessScreen {
    /// Creates an empty screen of the given size.
    ///
    /// The listener is supplied by the caller rather than constructed here
    /// because the grid emits terminal events as it parses, and who should
    /// receive them differs between the supervisor and a test.
    pub fn new(columns: u16, rows: u16, event_listener: ChannelEventListener) -> Self {
        let build = |is_alternate: bool| {
            GridHandler::new(
                SizeInfo::new_without_font_metrics(rows as usize, columns as usize),
                0,
                event_listener.clone(),
                is_alternate,
                ObfuscateSecrets::No,
                PerformResetGridChecks::No,
            )
        };

        Self {
            primary: build(false),
            alternate: build(true),
            alternate_active: false,
            processor: ansi::Processor::new(),
            columns,
            rows,
        }
    }

    /// Rebuilds a screen from a snapshot.
    pub fn from_snapshot(snapshot: &ScreenSnapshot, event_listener: ChannelEventListener) -> Self {
        let mut screen = Self::new(snapshot.columns, snapshot.rows, event_listener);
        screen.feed(&snapshot.repaint);
        screen
    }

    /// Advances the screen by a run of output bytes.
    pub fn feed(&mut self, bytes: &[u8]) {
        // The terminal's replies to queries (DA, DSR) have nowhere to go while
        // detached: there is no attached client to send them to, and the shell
        // does not block waiting for them. Discard them.
        let mut sink = std::io::sink();

        // The processor has to be moved out for the call, because parsing needs
        // `&mut self` as the handler and the processor lives inside `self`.
        let mut processor = std::mem::take(&mut self.processor);
        processor.parse_bytes(self, bytes, &mut sink);
        self.processor = processor;
    }

    /// Renders the current screen as a snapshot.
    pub fn snapshot(&self) -> ScreenSnapshot {
        let mut repaint = Vec::new();

        // Switch the receiver to the alternate screen before the repaint draws
        // anything, so the erase and the content land on the right screen.
        if self.alternate_active {
            repaint.extend_from_slice(b"\x1b[?1049h");
        }
        repaint.extend_from_slice(&render_ansi::render_screen(self.active_grid()));

        ScreenSnapshot {
            columns: self.columns,
            rows: self.rows,
            repaint,
        }
    }

    /// The visible screen text, for assertions and for logging a session's
    /// current state.
    pub fn visible_text(&self) -> String {
        let grid = self.active_grid();
        let end = Point::new(
            grid.total_rows().saturating_sub(1),
            grid.columns().saturating_sub(1),
        );
        grid.bounds_to_string(
            Point::new(0, 0),
            end,
            false,
            RespectObfuscatedSecrets::No,
            false,
            RespectDisplayedOutput::No,
        )
    }

    /// Whether the alternate screen is the one receiving output.
    pub fn is_alternate_screen_active(&self) -> bool {
        self.alternate_active
    }

    /// The grid currently receiving output.
    pub(crate) fn active_grid(&self) -> &GridHandler {
        if self.alternate_active {
            &self.alternate
        } else {
            &self.primary
        }
    }

    /// The grid currently receiving output.
    pub(crate) fn active_grid_mut(&mut self) -> &mut GridHandler {
        if self.alternate_active {
            &mut self.alternate
        } else {
            &mut self.primary
        }
    }

    /// Switches output to the alternate screen, leaving the primary screen
    /// untouched so it can be restored on exit.
    pub(crate) fn enter_alternate_screen(&mut self, clear_screen: bool) {
        if self.alternate_active {
            return;
        }
        self.alternate_active = true;

        if clear_screen {
            // Mode 1049 hands the application a blank alternate screen. Without
            // this, a second entry would show remnants of the first.
            use crate::model::ansi::Handler as _;
            self.alternate.clear_screen(ansi::ClearMode::All);
            self.alternate.goto(crate::model::index::VisibleRow(0), 0);
        }
    }

    /// Switches output back to the primary screen.
    pub(crate) fn exit_alternate_screen(&mut self, _restore_cursor: bool) {
        if !self.alternate_active {
            return;
        }
        self.alternate_active = false;
    }

    /// The active grid, for renderer tests.
    #[cfg(test)]
    pub(crate) fn grid_for_test(&self) -> &GridHandler {
        self.active_grid()
    }
}

#[cfg(test)]
#[path = "screen_tests.rs"]
mod tests;
