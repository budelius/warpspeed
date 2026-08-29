//! Renders a grid back into an ANSI repaint sequence.
//!
//! This is the inverse of parsing: given a screen, produce bytes that
//! reconstruct it. It exists so a detached session's screen can travel over the
//! supervisor protocol as bytes rather than as a serialized cell format, which
//! would otherwise have to be versioned forever.

use super::Dimensions;
use super::cell::{Cell, Flags};
use super::grid_handler::GridHandler;
use crate::model::Point;
use crate::model::ansi::{Color, NamedColor};
use crate::model::char_or_str::CharOrStr;

/// The flags a repaint can reproduce with SGR.
///
/// The rest of [`Flags`] is structural rather than stylistic -- wrapping, wide
/// character spacers, cursor tracking -- and is re-established by the receiving
/// grid as it lays the text out, not by anything this renderer emits.
const STYLE_FLAGS: Flags = Flags::INVERSE
    .union(Flags::BOLD)
    .union(Flags::ITALIC)
    .union(Flags::UNDERLINE)
    .union(Flags::DIM)
    .union(Flags::HIDDEN)
    .union(Flags::STRIKEOUT)
    .union(Flags::DOUBLE_UNDERLINE);

/// The subset of a cell's appearance a repaint reproduces.
#[derive(Clone, Copy, PartialEq, Eq)]
struct CellStyle {
    foreground: Color,
    background: Color,
    flags: Flags,
}

impl CellStyle {
    fn of(cell: &Cell) -> Self {
        Self {
            foreground: cell.fg,
            background: cell.bg,
            flags: cell.flags.intersection(STYLE_FLAGS),
        }
    }

    /// The SGR sequence selecting this style.
    ///
    /// Always starts from a reset, so the result never depends on what was
    /// active before it. That costs a few bytes per style change and removes a
    /// whole class of ordering bug.
    fn sgr(&self) -> String {
        let mut parameters = vec!["0".to_owned()];

        for (flag, parameter) in [
            (Flags::BOLD, "1"),
            (Flags::DIM, "2"),
            (Flags::ITALIC, "3"),
            (Flags::UNDERLINE, "4"),
            (Flags::INVERSE, "7"),
            (Flags::HIDDEN, "8"),
            (Flags::STRIKEOUT, "9"),
            (Flags::DOUBLE_UNDERLINE, "21"),
        ] {
            if self.flags.contains(flag) {
                parameters.push(parameter.to_owned());
            }
        }

        if let Some(parameter) = color_parameter(self.foreground, true) {
            parameters.push(parameter);
        }
        if let Some(parameter) = color_parameter(self.background, false) {
            parameters.push(parameter);
        }

        format!("\x1b[{}m", parameters.join(";"))
    }
}

impl Default for CellStyle {
    fn default() -> Self {
        Self {
            foreground: Color::Named(NamedColor::Foreground),
            background: Color::Named(NamedColor::Background),
            flags: Flags::empty(),
        }
    }
}

/// The SGR parameter selecting `color`, or `None` when the leading reset
/// already produces it.
///
/// Every [`NamedColor`] is listed rather than caught by a wildcard, so adding a
/// variant fails the build here instead of silently rendering the wrong color.
fn color_parameter(color: Color, is_foreground: bool) -> Option<String> {
    let base = if is_foreground { 30 } else { 40 };
    let bright_base = if is_foreground { 90 } else { 100 };

    let parameter = match color {
        Color::Named(named) => match named {
            NamedColor::Black => base,
            NamedColor::Red => base + 1,
            NamedColor::Green => base + 2,
            NamedColor::Yellow => base + 3,
            NamedColor::Blue => base + 4,
            NamedColor::Magenta => base + 5,
            NamedColor::Cyan => base + 6,
            NamedColor::White => base + 7,
            NamedColor::BrightBlack => bright_base,
            NamedColor::BrightRed => bright_base + 1,
            NamedColor::BrightGreen => bright_base + 2,
            NamedColor::BrightYellow => bright_base + 3,
            NamedColor::BrightBlue => bright_base + 4,
            NamedColor::BrightMagenta => bright_base + 5,
            NamedColor::BrightCyan => bright_base + 6,
            NamedColor::BrightWhite => bright_base + 7,
            // The dim variants are produced by `NamedColor::to_dim`, which the
            // ANSI handler never calls, so a parsed grid does not contain them.
            // Render the base color and let the DIM flag carry the dimming, so
            // that a grid built some other way still round-trips its hue.
            NamedColor::DimBlack => base,
            NamedColor::DimRed => base + 1,
            NamedColor::DimGreen => base + 2,
            NamedColor::DimYellow => base + 3,
            NamedColor::DimBlue => base + 4,
            NamedColor::DimMagenta => base + 5,
            NamedColor::DimCyan => base + 6,
            NamedColor::DimWhite => base + 7,
            // Defaults: the leading reset already selects these.
            NamedColor::Foreground
            | NamedColor::Background
            | NamedColor::BrightForeground
            | NamedColor::DimForeground => return None,
            // Never stamped into a cell; it colors the cursor, not text.
            NamedColor::Cursor => return None,
        },
        Color::Indexed(index) => {
            let selector = if is_foreground { 38 } else { 48 };
            return Some(format!("{selector};5;{index}"));
        }
        Color::Spec(rgb) => {
            let selector = if is_foreground { 38 } else { 48 };
            return Some(format!("{};2;{};{};{}", selector, rgb.r, rgb.g, rgb.b));
        }
    };

    Some(parameter.to_string())
}

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

    // A scroll region set by a full-screen application decides where later
    // output lands, so it has to be restored before any of that output arrives.
    // DECSTBM is 1-based and inclusive, and it homes the cursor -- which is why
    // it goes here rather than after the content, whose final act is to place
    // the cursor.
    let region = grid.scroll_region();
    if region.start.0 != 0 || region.end.0 != rows {
        out.extend_from_slice(format!("\x1b[{};{}r", region.start.0 + 1, region.end.0).as_bytes());
    }

    let mut active_style = CellStyle::default();

    for row in 0..rows {
        // Each row is positioned absolutely rather than reached with a newline.
        // A newline at the bottom of a scroll region scrolls it, and DECSTBM
        // above has already moved the cursor, so walking the screen with "\r\n"
        // puts content on the wrong rows.
        out.extend_from_slice(format!("\x1b[{};1H", row + 1).as_bytes());

        // Blank cells at the end of a row are already blank from the erase
        // above, so they are held back and only written if more content
        // follows on the same row.
        let mut pending_blanks = 0usize;

        for column in 0..columns {
            let cell = &grid.grid_storage()[Point::new(row, column)];
            let content = cell.content_for_display();
            let is_blank = matches!(content, CharOrStr::Char(' '));
            let style = CellStyle::of(cell);

            if is_blank && style == CellStyle::default() {
                pending_blanks += 1;
                continue;
            }

            for _ in 0..std::mem::take(&mut pending_blanks) {
                out.push(b' ');
            }

            if style != active_style {
                out.extend_from_slice(style.sgr().as_bytes());
                active_style = style;
            }

            match content {
                CharOrStr::Char(character) => {
                    let mut buffer = [0u8; 4];
                    out.extend_from_slice(character.encode_utf8(&mut buffer).as_bytes());
                }
                CharOrStr::Str(text) => out.extend_from_slice(text.as_bytes()),
            }
        }
    }

    // Leave the receiving screen in the default style, so live output arriving
    // after the snapshot is not tinted by whatever the last cell happened to be.
    if active_style != CellStyle::default() {
        out.extend_from_slice(b"\x1b[0m");
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
