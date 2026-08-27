//! A terminal screen maintained with no UI attached.
//!
//! The session supervisor uses this to keep a detached session's screen current
//! while the app is not attached, and to hand that screen back on reattach. The
//! wire form is [`ScreenSnapshot`] -- a repaint byte sequence rather than a
//! serialized grid, so the protocol never has to version a cell format.

use crate::SizeInfo;
use crate::event_listener::ChannelEventListener;
use crate::model::ansi;
use crate::model::grid::grid_handler::{GridHandler, PerformResetGridChecks};
use crate::model::grid::render_ansi;
use crate::model::grid::{Dimensions, RespectDisplayedOutput};
use crate::model::Point;
use crate::model::secrets::{ObfuscateSecrets, RespectObfuscatedSecrets};

/// A screen in a form that can be sent over the supervisor protocol.
///
/// `repaint` is a self-contained ANSI sequence that reconstructs the screen when
/// fed to any terminal: the screen mode, an erase, the visible text with its
/// attributes, then the cursor placed where it belongs.
#[derive(Debug, Clone, PartialEq, Eq)]
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
    grid: GridHandler,
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
        let size = SizeInfo::new_without_font_metrics(rows as usize, columns as usize);
        let grid = GridHandler::new(
            size,
            0,
            event_listener,
            false,
            ObfuscateSecrets::No,
            PerformResetGridChecks::No,
        );
        Self {
            grid,
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
        self.processor.parse_bytes(&mut self.grid, bytes, &mut sink);
    }

    /// Renders the current screen as a snapshot.
    pub fn snapshot(&self) -> ScreenSnapshot {
        ScreenSnapshot {
            columns: self.columns,
            rows: self.rows,
            repaint: render_ansi::render_screen(&self.grid),
        }
    }

    /// The visible screen text, for assertions and for logging a session's
    /// current state.
    pub fn visible_text(&self) -> String {
        let end = Point::new(
            self.grid.total_rows().saturating_sub(1),
            self.grid.columns().saturating_sub(1),
        );
        self.grid.bounds_to_string(
            Point::new(0, 0),
            end,
            false,
            RespectObfuscatedSecrets::No,
            false,
            RespectDisplayedOutput::No,
        )
    }

    /// The underlying grid, for renderer tests.
    #[cfg(test)]
    pub(crate) fn grid_for_test(&self) -> &GridHandler {
        &self.grid
    }
}

#[cfg(test)]
#[path = "screen_tests.rs"]
mod tests;
