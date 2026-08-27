//! Renders a grid back into an ANSI repaint sequence.
//!
//! This is the inverse of parsing: given a screen, produce bytes that
//! reconstruct it. It exists so a detached session's screen can travel over the
//! supervisor protocol as bytes rather than as a serialized cell format, which
//! would otherwise have to be versioned forever.

use super::Dimensions;
use super::grid_handler::GridHandler;
use crate::model::Point;
use crate::model::char_or_str::CharOrStr;

/// Renders `grid`'s screen as a self-contained repaint sequence.
///
/// The output is safe to feed to a terminal in any state: it homes the cursor
/// and erases before drawing, so the result does not depend on what the
/// receiving screen already contained.
pub fn render_screen(grid: &GridHandler) -> Vec<u8> {
    let mut out = Vec::new();

    out.extend_from_slice(b"\x1b[H\x1b[2J");

    let columns = grid.columns();
    let rows = grid.total_rows();

    for row in 0..rows {
        if row > 0 {
            out.extend_from_slice(b"\r\n");
        }

        let mut line = String::new();
        for column in 0..columns {
            let cell = &grid.grid_storage()[Point::new(row, column)];
            match cell.content_for_display() {
                CharOrStr::Char(character) => line.push(character),
                CharOrStr::Str(content) => line.push_str(content),
            }
        }

        // The erase above already produced blanks, so trailing spaces would
        // only make snapshots bigger without changing what is displayed.
        out.extend_from_slice(line.trim_end().as_bytes());
    }

    // Put the cursor back where the session left it. Without this, output that
    // arrives after the snapshot is drawn in the wrong place.
    let cursor = grid.cursor_point();
    out.extend_from_slice(format!("\x1b[{};{}H", cursor.row + 1, cursor.col + 1).as_bytes());

    out
}

#[cfg(test)]
#[path = "render_ansi_tests.rs"]
mod tests;
