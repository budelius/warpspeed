# Screen Snapshot Round-Trip Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the screen snapshot mechanism — render a terminal grid to a self-contained ANSI repaint sequence, and reconstruct a grid from one — proven by the detach-boundary equivalence property.

**Architecture:** A `HeadlessScreen` drives the existing `GridHandler` with no UI attached, seeded from a `ScreenSnapshot` (which is just ANSI bytes plus dimensions). A new `render_ansi` module walks a `GridHandler` and emits the repaint sequence. Nothing in this plan touches processes, sockets, or platform code — it is pure, cross-platform, and fully unit-testable.

**Tech Stack:** Rust 2024, `crates/warp_terminal`, existing `model::ansi::Processor` (a `vte` wrapper) and `model::grid::grid_handler::GridHandler`.

## Plan sequence

This is plan 1 of 4 for Spec A (`docs/superpowers/specs/2026-08-28-persistent-terminal-sessions-design.md`):

1. **This plan** — snapshot render/seed and the equivalence property.
2. Protocol v2, the `Transport` trait, loopback transport, and Unix supervisor detach/attach.
3. Windows supervisor: ConPTY ownership, named pipe, `DuplicateHandle` lending.
4. Idle classification, quit/launch orchestration, detached-sessions drawer.

Each produces working, testable software. Plans 2 through 4 are written after this one lands, so they can be informed by what it teaches.

## Global Constraints

- Rust edition 2024 (`crates/warp_terminal/Cargo.toml`).
- No new third-party dependencies. `vte` is already a workspace dependency and is reached through `crate::model::ansi`.
- **Cross-platform.** Nothing in this plan may be `#[cfg(unix)]`. This is why the new module goes at `local_tty/screen.rs` and **not** `local_tty/server/screen.rs` as the spec's component table says — `local_tty/server` is `#[cfg(unix)]` (`local_tty/mod.rs:10`) and the Windows supervisor needs this code too. Update the spec's table when this lands.
- Tests live in a sibling file wired with `#[cfg(test)] #[path = "..._tests.rs"] mod tests;`, matching `persistent.rs` / `persistent_tests.rs`.
- Doc comments on every public item, per `AGENTS.md`.
- Build gate: `cargo build -p warp_terminal`. Test gate: `cargo test -p warp_terminal --lib -- <module>`.
- **The baseline is not green, so "all tests pass" is not a usable gate.** Measured on an
  untouched checkout of this branch (552 tests):
  - `local_tty::windows::child::tests::test_event_is_emitted_when_child_exits` fails
    deterministically with `Os { code: 5, PermissionDenied }` — the sandbox denies process
    creation. Environmental, unrelated to this work.
  - `model::grid::grid_handler::secrets::tests::*` fail **nondeterministically**: one run failed
    `test_secret_redaction_secret_remains_after_resize`, the next failed
    `test_secret_redacted_after_byte_processing` and
    `test_secret_redacted_after_multiple_byte_processing` instead. Each passes in isolation, so it
    is shared state across tests running in parallel. Pre-existing.
  The gate is therefore **no new failures outside that set**, checked by running the touched
  modules directly. Note that piping cargo to `tail` masks its exit code; use `${PIPESTATUS[0]}`.
  The flaky secrets tests are worth fixing, but that is not this plan's job.

---

### Task 1: Detach-boundary equivalence, red

Write the property test before any implementation, so the API is designed by its use.

**Files:**
- Create: `crates/warp_terminal/src/local_tty/screen.rs`
- Create: `crates/warp_terminal/src/local_tty/screen_tests.rs`
- Modify: `crates/warp_terminal/src/local_tty/mod.rs` (add `pub mod screen;` after `pub mod recorder;`)

**Interfaces:**
- Consumes: `crate::model::ansi::Processor`, `crate::model::grid::grid_handler::GridHandler`, `crate::SizeInfo`, `crate::event_listener::ChannelEventListener`.
- Produces:
  - `pub struct ScreenSnapshot { pub columns: u16, pub rows: u16, pub repaint: Vec<u8> }`
  - `pub struct HeadlessScreen`
  - `pub fn HeadlessScreen::new(columns: u16, rows: u16) -> Self`
  - `pub fn HeadlessScreen::feed(&mut self, bytes: &[u8])`
  - `pub fn HeadlessScreen::snapshot(&self) -> ScreenSnapshot`
  - `pub fn HeadlessScreen::from_snapshot(snapshot: &ScreenSnapshot) -> Self`
  - `pub fn HeadlessScreen::visible_text(&self) -> String`

- [ ] **Step 1: Write the failing test**

In `crates/warp_terminal/src/local_tty/screen_tests.rs`:

```rust
use super::*;

/// Feeding `a` then `b` with a snapshot round-trip in between must produce the
/// same screen as feeding `a ++ b` straight through. This is the property the
/// whole detach/reattach design rests on: detaching mid-stream and reattaching
/// has to be indistinguishable from never having detached.
fn assert_detach_boundary_equivalence(a: &[u8], b: &[u8]) {
    let mut continuous = HeadlessScreen::new(20, 5);
    continuous.feed(a);
    continuous.feed(b);

    let mut before_detach = HeadlessScreen::new(20, 5);
    before_detach.feed(a);
    let mut after_reattach = HeadlessScreen::from_snapshot(&before_detach.snapshot());
    after_reattach.feed(b);

    assert_eq!(
        continuous.visible_text(),
        after_reattach.visible_text(),
        "screen diverged across the snapshot boundary\n  a: {a:?}\n  b: {b:?}"
    );
}

#[test]
fn plain_text_survives_the_detach_boundary() {
    assert_detach_boundary_equivalence(b"hello", b" world");
}

/// With no bytes after the boundary the property degenerates to the renderer
/// round-trip: render the screen, feed it back, get the same screen.
#[test]
fn rendering_and_reseeding_reproduces_the_screen() {
    assert_detach_boundary_equivalence(b"first line\r\nsecond line", b"");
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p warp_terminal --lib local_tty::screen`
Expected: FAIL to compile — `screen.rs` does not exist yet, so the module is unresolved.

- [ ] **Step 3: Write the minimal implementation**

In `crates/warp_terminal/src/local_tty/screen.rs`:

```rust
//! A terminal screen maintained with no UI attached.
//!
//! The session supervisor uses this to keep a detached session's screen current
//! while the app is not attached, and to hand that screen back on reattach. The
//! wire form is [`ScreenSnapshot`] — a repaint byte sequence rather than a
//! serialized grid, so the protocol never has to version a cell format.

use crate::SizeInfo;
use crate::event_listener::ChannelEventListener;
use crate::model::ansi;
use crate::model::grid::grid_handler::GridHandler;
use crate::model::grid::{PerformResetGridChecks, RespectDisplayedOutput};
use crate::model::secrets::{ObfuscateSecrets, RespectObfuscatedSecrets};

/// A screen in a form that can be sent over the supervisor protocol.
///
/// `repaint` is a self-contained ANSI sequence that reconstructs the screen when
/// fed to any terminal: cursor home, erase, then the visible text with its
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
    pub fn new(columns: u16, rows: u16) -> Self {
        let size = SizeInfo::new_without_font_metrics(rows as usize, columns as usize);
        let grid = GridHandler::new(
            size,
            0,
            ChannelEventListener::new_for_test(),
            false,
            ObfuscateSecrets::No,
            PerformResetGridChecks::default(),
        );
        Self {
            grid,
            processor: ansi::Processor::new(),
            columns,
            rows,
        }
    }

    /// Rebuilds a screen from a snapshot.
    pub fn from_snapshot(snapshot: &ScreenSnapshot) -> Self {
        let mut screen = Self::new(snapshot.columns, snapshot.rows);
        screen.feed(&snapshot.repaint);
        screen
    }

    /// Advances the screen by a run of output bytes.
    pub fn feed(&mut self, bytes: &[u8]) {
        // The terminal's replies to queries (DA, DSR) have nowhere to go while
        // detached: there is no attached client to send them to, and the shell
        // does not block on them. Discard them.
        let mut sink = std::io::sink();
        self.processor.parse_bytes(&mut self.grid, bytes, &mut sink);
    }

    /// Renders the current screen as a snapshot.
    pub fn snapshot(&self) -> ScreenSnapshot {
        ScreenSnapshot {
            columns: self.columns,
            rows: self.rows,
            repaint: crate::model::grid::render_ansi::render_screen(&self.grid),
        }
    }

    /// The visible screen text, for assertions and for logging a session's
    /// current state.
    pub fn visible_text(&self) -> String {
        self.grid.bounds_to_string(
            crate::model::grid::Point::new(0, 0),
            self.grid.end_point(),
            false,
            RespectObfuscatedSecrets::No,
            false,
            RespectDisplayedOutput::No,
        )
    }
}

#[cfg(test)]
#[path = "screen_tests.rs"]
mod tests;
```

In `crates/warp_terminal/src/local_tty/mod.rs`, add after `pub mod recorder;`:

```rust
pub mod screen;
```

This will not compile yet: `render_ansi` does not exist. Task 2 adds it. Confirm the failure is exactly that unresolved path and nothing else — if other imports are wrong, fix them here before moving on, adjusting the `use` paths and the `bounds_to_string` / `end_point` calls to match the real signatures in `grid_handler.rs`.

- [ ] **Step 4: Run the test to verify the failure has moved**

Run: `cargo test -p warp_terminal --lib local_tty::screen`
Expected: FAIL with `unresolved import` / `could not find render_ansi in grid`. Any other error means an import above is wrong — fix it now.

- [ ] **Step 5: Commit**

```bash
git add crates/warp_terminal/src/local_tty/screen.rs \
        crates/warp_terminal/src/local_tty/screen_tests.rs \
        crates/warp_terminal/src/local_tty/mod.rs
git commit -m "test(terminal): assert detach-boundary screen equivalence"
```

---

### Task 2: Render text and cursor position

The smallest renderer that can satisfy Task 1's two tests: visible text and final cursor position. Attributes come in Task 3.

**Files:**
- Create: `crates/warp_terminal/src/model/grid/render_ansi.rs`
- Create: `crates/warp_terminal/src/model/grid/render_ansi_tests.rs`
- Modify: `crates/warp_terminal/src/model/grid/mod.rs` (add `pub mod render_ansi;` next to `pub mod grid_handler;`)

**Interfaces:**
- Consumes: `GridHandler` read accessors — `columns()`, `screen_lines()` or `rows()`, `grid_storage()`, and the cursor position accessor. Confirm the exact names in `grid_handler.rs` before writing; use what is there rather than inventing.
- Produces: `pub fn render_screen(grid: &GridHandler) -> Vec<u8>`

- [ ] **Step 1: Write the failing test**

In `crates/warp_terminal/src/model/grid/render_ansi_tests.rs`:

```rust
use super::*;

/// The repaint must begin by putting the terminal into a known state. Without
/// this, a snapshot fed into a screen carrying leftover state from earlier
/// output reproduces the wrong thing.
#[test]
fn repaint_starts_from_a_known_state() {
    let grid = crate::local_tty::screen::HeadlessScreen::new(10, 3);
    let bytes = render_screen(grid.grid_for_test());

    assert!(
        bytes.starts_with(b"\x1b[H\x1b[2J"),
        "repaint must home the cursor and erase the screen first, got {:?}",
        String::from_utf8_lossy(&bytes[..bytes.len().min(16)])
    );
}
```

Add to `crates/warp_terminal/src/local_tty/screen.rs`, inside `impl HeadlessScreen`:

```rust
    /// The underlying grid, for renderer tests.
    #[cfg(test)]
    pub(crate) fn grid_for_test(&self) -> &GridHandler {
        &self.grid
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p warp_terminal --lib model::grid::render_ansi`
Expected: FAIL to compile — `render_screen` is not defined.

- [ ] **Step 3: Write the minimal implementation**

In `crates/warp_terminal/src/model/grid/render_ansi.rs`:

```rust
//! Renders a grid back into an ANSI repaint sequence.
//!
//! This is the inverse of parsing: given a screen, produce bytes that
//! reconstruct it. It exists so a detached session's screen can travel over the
//! supervisor protocol as bytes rather than as a serialized cell format.

use super::grid_handler::GridHandler;

/// Renders `grid`'s visible screen as a self-contained repaint sequence.
///
/// The output is safe to feed to a terminal in any state: it resets the cursor
/// and erases before drawing.
pub fn render_screen(grid: &GridHandler) -> Vec<u8> {
    let mut out = Vec::new();

    // Home the cursor and erase, so the result does not depend on what the
    // receiving screen already contained.
    out.extend_from_slice(b"\x1b[H\x1b[2J");

    let columns = grid.columns();
    let rows = grid.screen_lines();

    for row in 0..rows {
        if row > 0 {
            out.extend_from_slice(b"\r\n");
        }
        let mut line = String::new();
        for column in 0..columns {
            let point = super::Point::new(row, column);
            match grid.cell_char(point) {
                Some(character) => line.push(character),
                None => line.push(' '),
            }
        }
        // Trailing blanks are re-created by the erase above, so drop them and
        // keep snapshots small.
        out.extend_from_slice(line.trim_end().as_bytes());
    }

    out
}

#[cfg(test)]
#[path = "render_ansi_tests.rs"]
mod tests;
```

`cell_char`, `columns`, and `screen_lines` are the accessor names to confirm against `grid_handler.rs`. If a cell-character accessor does not exist, read the cell through `grid.grid_storage()[point]` and take its character — `cell.rs:341` shows the cell API. Do not add a new accessor to `GridHandler` unless nothing suitable exists.

In `crates/warp_terminal/src/model/grid/mod.rs`, add next to `pub mod grid_handler;`:

```rust
pub mod render_ansi;
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p warp_terminal --lib model::grid::render_ansi local_tty::screen`
Expected: `repaint_starts_from_a_known_state` PASSES, and both Task 1 tests PASS — plain text and line breaks now survive the boundary.

If `plain_text_survives_the_detach_boundary` still fails, the cursor is not being restored. Add cursor placement to the end of `render_screen`:

```rust
    let cursor = grid.cursor_point();
    out.extend_from_slice(
        format!("\x1b[{};{}H", cursor.row + 1, cursor.col + 1).as_bytes(),
    );
```

- [ ] **Step 5: Commit**

```bash
git add crates/warp_terminal/src/model/grid/render_ansi.rs \
        crates/warp_terminal/src/model/grid/render_ansi_tests.rs \
        crates/warp_terminal/src/model/grid/mod.rs \
        crates/warp_terminal/src/local_tty/screen.rs
git commit -m "feat(terminal): render a grid to an ANSI repaint sequence"
```

---

### Task 3: Preserve text attributes across the boundary

Colors and styles are lost by Task 2's renderer. This task makes them survive.

**Files:**
- Modify: `crates/warp_terminal/src/model/grid/render_ansi.rs`
- Modify: `crates/warp_terminal/src/local_tty/screen_tests.rs`

**Interfaces:**
- Consumes: `Cell` accessors for foreground, background, and `flags()` (`model/grid/cell.rs:341`); the `Color` type used by cells.
- Produces: no new public API — `render_screen` gains attribute output.

- [ ] **Step 1: Write the failing test**

Append to `crates/warp_terminal/src/local_tty/screen_tests.rs`:

```rust
/// A reattached screen that has lost its colors is visibly wrong even though
/// the text matches, so `visible_text` alone cannot catch this. Compare the
/// rendered repaint instead: it carries the attributes.
#[test]
fn text_attributes_survive_the_detach_boundary() {
    let styled = b"\x1b[31mred\x1b[0m plain \x1b[1mbold\x1b[0m";

    let mut continuous = HeadlessScreen::new(40, 3);
    continuous.feed(styled);

    let reattached = HeadlessScreen::from_snapshot(&continuous.snapshot());

    assert_eq!(
        continuous.snapshot().repaint,
        reattached.snapshot().repaint,
        "attributes were lost across the snapshot boundary"
    );
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p warp_terminal --lib local_tty::screen::tests::text_attributes_survive_the_detach_boundary`
Expected: FAIL — the two repaints differ, because Task 2's renderer emits no SGR and the reattached screen is uniformly default-styled.

- [ ] **Step 3: Write the implementation**

Replace the per-row loop body in `render_screen` so it emits SGR only when the style changes from the previous cell, which keeps snapshots compact:

```rust
    let mut active_style: Option<CellStyle> = None;

    for row in 0..rows {
        if row > 0 {
            out.extend_from_slice(b"\r\n");
        }
        let mut trailing_blanks = 0usize;
        for column in 0..columns {
            let point = super::Point::new(row, column);
            let cell = &grid.grid_storage()[point];
            let style = CellStyle::of(cell);
            let character = cell.c();

            if character == ' ' && style.is_default() {
                // Defer: the erase already produced blanks. Only emit if more
                // content follows on this row.
                trailing_blanks += 1;
                continue;
            }
            for _ in 0..std::mem::take(&mut trailing_blanks) {
                out.push(b' ');
            }
            if active_style != Some(style) {
                out.extend_from_slice(style.sgr().as_bytes());
                active_style = Some(style);
            }
            let mut buffer = [0u8; 4];
            out.extend_from_slice(character.encode_utf8(&mut buffer).as_bytes());
        }
    }

    // Leave the receiving screen in the default style regardless of the last
    // cell, so subsequent live output is not tinted by the snapshot.
    if active_style.is_some_and(|style| !style.is_default()) {
        out.extend_from_slice(b"\x1b[0m");
        active_style = Some(CellStyle::default());
    }
    let _ = active_style;
```

And add the style helper in the same file:

```rust
/// The subset of a cell's appearance the repaint reproduces.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
struct CellStyle {
    foreground: Option<Color>,
    background: Option<Color>,
    flags: Flags,
}

impl CellStyle {
    fn of(cell: &Cell) -> Self {
        Self {
            foreground: Some(cell.fg()),
            background: Some(cell.bg()),
            flags: *cell.flags(),
        }
    }

    fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// The SGR sequence that selects this style, starting from a reset so the
    /// result never depends on what was active before.
    fn sgr(&self) -> String {
        let mut parameters = vec!["0".to_owned()];
        if self.flags.contains(Flags::BOLD) {
            parameters.push("1".to_owned());
        }
        if self.flags.contains(Flags::ITALIC) {
            parameters.push("3".to_owned());
        }
        if self.flags.contains(Flags::UNDERLINE) {
            parameters.push("4".to_owned());
        }
        if let Some(color) = self.foreground {
            parameters.push(sgr_color_parameters(color, true));
        }
        if let Some(color) = self.background {
            parameters.push(sgr_color_parameters(color, false));
        }
        format!("\x1b[{}m", parameters.join(";"))
    }
}
```

Write `sgr_color_parameters` against the actual `Color` enum in the grid's cell module — it will have named, indexed, and true-color variants, mapping to `3x`/`4x`, `38;5;n`/`48;5;n`, and `38;2;r;g;b`/`48;2;r;g;b` respectively. Read the enum first and handle every variant; do not use a catch-all arm, so a future variant fails the build instead of silently rendering wrong.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p warp_terminal --lib local_tty::screen model::grid::render_ansi`
Expected: all PASS, including the two Task 1 tests, which must not regress.

- [ ] **Step 5: Commit**

```bash
git add crates/warp_terminal/src/model/grid/render_ansi.rs \
        crates/warp_terminal/src/local_tty/screen_tests.rs
git commit -m "feat(terminal): carry text attributes through a screen snapshot"
```

---

### Task 4: Alternate screen and scroll region

The case the whole design exists for: a TUI on the alternate screen must reattach correctly.

**Files:**
- Modify: `crates/warp_terminal/src/model/grid/render_ansi.rs`
- Modify: `crates/warp_terminal/src/local_tty/screen.rs`
- Modify: `crates/warp_terminal/src/local_tty/screen_tests.rs`

**Interfaces:**
- Consumes: `GridHandler`'s alt-screen flag (`ansi_handler::State::is_alt_screen`, reachable via the handler's state) and its `scroll_region`.
- Produces: `ScreenSnapshot` unchanged in shape; `repaint` gains the mode and scroll-region prologue.

- [ ] **Step 1: Write the failing test**

Append to `crates/warp_terminal/src/local_tty/screen_tests.rs`:

```rust
/// A full-screen application detached mid-draw is exactly the case the raw
/// byte spool got wrong: replaying its output produces garbage. The snapshot
/// must reproduce the alternate screen and leave the receiver on it.
#[test]
fn alternate_screen_survives_the_detach_boundary() {
    // Enter the alt screen, draw a box-ish layout, leave the cursor mid-screen.
    let draw = b"\x1b[?1049h\x1b[H\x1b[2Jtop row\r\n\r\nbottom row\x1b[2;3H";

    assert_detach_boundary_equivalence(draw, b" more");
}

/// A scroll region set by a TUI changes where subsequent output lands. If the
/// snapshot does not restore it, output after reattach goes to the wrong rows.
#[test]
fn scroll_region_survives_the_detach_boundary() {
    assert_detach_boundary_equivalence(b"\x1b[2;4r\x1b[3;1Hinside", b"\r\nnext");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p warp_terminal --lib local_tty::screen`
Expected: both new tests FAIL — the reattached screen is on the primary screen with a full-height scroll region, so text lands on different rows.

- [ ] **Step 3: Write the implementation**

`HeadlessScreen::new` currently hardcodes `is_alt_screen: false`. Give the renderer the mode and have `from_snapshot` apply it from the repaint itself, which it already will once the prologue is emitted.

In `render_screen`, immediately after the reset and before drawing:

```rust
    if grid.is_alt_screen() {
        // Switch the receiver to the alternate screen before drawing, so the
        // snapshot lands where the application expects to be.
        out.extend_from_slice(b"\x1b[?1049h");
    }

    let scroll_region = grid.scroll_region();
    let full_height = 0..rows;
    if scroll_region != full_height {
        out.extend_from_slice(
            format!("\x1b[{};{}r", scroll_region.start + 1, scroll_region.end).as_bytes(),
        );
    }
```

The mode prologue must come before `\x1b[2J`, because erasing applies to whichever screen is active — reorder the reset so it reads: alt-screen mode, then `\x1b[H\x1b[2J`, then scroll region, then content, then cursor. Update Task 2's `repaint_starts_from_a_known_state` assertion accordingly: it should assert the erase appears, not that the sequence starts with it.

Expose the two accessors on `GridHandler` if they are not already public, keeping them read-only:

```rust
    /// Whether this grid is the alternate screen.
    pub fn is_alt_screen(&self) -> bool {
        self.ansi_handler_state.is_alt_screen
    }

    /// The active scroll region, as visible row indices.
    pub fn scroll_region(&self) -> std::ops::Range<usize> {
        let region = &self.ansi_handler_state.scroll_region;
        region.start.0..region.end.0
    }
```

- [ ] **Step 4: Run the full crate test suite**

Run: `cargo test -p warp_terminal`
Expected: all PASS. The whole crate runs here, not just the new module, because Task 3 and Task 4 touch `GridHandler` and `render_ansi` sits inside the grid module — a regression in existing grid tests is the signal that matters.

- [ ] **Step 5: Commit**

```bash
git add crates/warp_terminal/src/model/grid/render_ansi.rs \
        crates/warp_terminal/src/model/grid/grid_handler.rs \
        crates/warp_terminal/src/local_tty/screen.rs \
        crates/warp_terminal/src/local_tty/screen_tests.rs
git commit -m "feat(terminal): reproduce alt screen and scroll region in a snapshot"
```

---

### Task 5: Record what the renderer does not reproduce

Every snapshot renderer has a coverage boundary. Make this one explicit and tested rather than discovered in the field.

**Files:**
- Modify: `crates/warp_terminal/src/model/grid/render_ansi.rs` (module doc)
- Modify: `crates/warp_terminal/src/local_tty/screen_tests.rs`

- [ ] **Step 1: Write the characterization tests**

Append to `crates/warp_terminal/src/local_tty/screen_tests.rs`:

```rust
/// Wide characters occupy two cells. If the renderer emits the spacer cell as
/// a space, the reattached screen shifts by one column for the rest of the row.
#[test]
fn wide_characters_survive_the_detach_boundary() {
    assert_detach_boundary_equivalence("日本語 text".as_bytes(), b" more");
}

/// Warp stamps OSC 8 hyperlinks into cells. The snapshot does not currently
/// carry them, so a reattached session shows the text without the link. This
/// test records that gap deliberately: when the renderer learns to emit OSC 8,
/// flip the assertion.
#[test]
fn hyperlinks_are_not_yet_carried_by_a_snapshot() {
    let linked = b"\x1b]8;;https://example.com\x1b\\link text\x1b]8;;\x1b\\";

    let mut screen = HeadlessScreen::new(40, 3);
    screen.feed(linked);
    let repaint = screen.snapshot().repaint;

    assert!(
        !repaint.windows(4).any(|window| window == b"\x1b]8;"),
        "renderer now emits OSC 8 — carry hyperlinks and update this test"
    );
}
```

- [ ] **Step 2: Run them**

Run: `cargo test -p warp_terminal --lib local_tty::screen`
Expected: `hyperlinks_are_not_yet_carried_by_a_snapshot` PASSES immediately — it documents current behavior. `wide_characters_survive_the_detach_boundary` may FAIL; if it does, that is a real bug in the renderer, not a test to weaken. Fix it by skipping the spacer cell (`CellType::WideCharSpacer`, per `cell_type.rs`) rather than emitting a space for it.

- [ ] **Step 3: Write the module doc**

At the top of `render_ansi.rs`, under the existing module comment:

```rust
//! ## What a snapshot does not carry
//!
//! - **OSC 8 hyperlinks.** Cells keep a `HyperlinkId` into the grid's registry;
//!   the repaint emits text without the link. A reattached session shows
//!   unclickable text. See `hyperlinks_are_not_yet_carried_by_a_snapshot`.
//! - **Inline images.** iTerm2 and Kitty image protocol payloads are held in
//!   the grid's `ImageMap` and are not re-emitted.
//! - **Scrollback.** Only the visible screen is rendered. Scrollback comes from
//!   the app's own sqlite block restore, not from the supervisor.
```

- [ ] **Step 4: Run the full crate suite**

Run: `cargo test -p warp_terminal`
Expected: all PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/warp_terminal/src/model/grid/render_ansi.rs \
        crates/warp_terminal/src/local_tty/screen_tests.rs
git commit -m "test(terminal): record the snapshot renderer's coverage boundary"
```

---

## Self-Review

**Spec coverage.** This plan implements the spec's "screen snapshot is ANSI, not a serialized grid" section and its discriminating test. It deliberately does not touch: the `Transport` trait, protocol v2, the Windows backend, idle classification, or the drawer — those are plans 2 through 4, listed under Plan sequence.

**Two deviations from the spec, both intentional.** The module goes at `local_tty/screen.rs` rather than `local_tty/server/screen.rs`, because `server` is `#[cfg(unix)]` and Windows needs this code. And the spec's `ScreenSnapshot` carries `cell_width_px` / `cell_height_px`; this plan drops them, because `SizeInfo::new_without_font_metrics` builds a headless size from rows and columns alone and no pixel metric affects the parsed screen. Update the spec's struct and component table when this lands.

**Known risk carried forward.** Accessor names — `cell_char`, `screen_lines`, `cursor_point`, `fg`, `bg`, `Color` variants — are named from the shapes seen in `grid_handler.rs` and `cell.rs` but not all verified. Task 2 Step 3 and Task 3 Step 3 both instruct the implementer to confirm against the real source and use what is there. Any mismatch surfaces as a compile error in the very next step, not as wrong behavior.
