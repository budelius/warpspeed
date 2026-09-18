use super::*;
use crate::event_listener::ChannelEventListener;

fn screen(columns: u16, rows: u16) -> HeadlessScreen {
    HeadlessScreen::new(columns, rows, ChannelEventListener::new_for_test())
}

/// Feeding `a` then `b` with a snapshot round-trip in between must produce the
/// same screen as feeding `a ++ b` straight through. This is the property the
/// whole detach/reattach design rests on: detaching mid-stream and reattaching
/// has to be indistinguishable from never having detached.
fn assert_detach_boundary_equivalence(a: &[u8], b: &[u8]) {
    let mut continuous = screen(20, 5);
    continuous.feed(a);
    continuous.feed(b);

    let mut before_detach = screen(20, 5);
    before_detach.feed(a);
    let mut after_reattach = HeadlessScreen::from_snapshot(
        &before_detach.snapshot(),
        ChannelEventListener::new_for_test(),
    );
    after_reattach.feed(b);

    assert_eq!(
        continuous.visible_text(),
        after_reattach.visible_text(),
        "screen diverged across the snapshot boundary\n  a: {a:?}\n  b: {b:?}"
    );
}

/// Guards the equivalence tests against passing vacuously. They compare
/// `visible_text` to `visible_text`, so an implementation that always returned
/// the empty string would satisfy every one of them.
#[test]
fn visible_text_reflects_the_bytes_that_were_fed() {
    let mut screen = screen(20, 5);
    screen.feed(b"hello world");

    assert!(
        screen.visible_text().contains("hello world"),
        "visible_text lost the fed bytes, got {:?}",
        screen.visible_text()
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

#[test]
fn resizing_reflows_both_the_screen_and_its_snapshot() {
    let mut screen = screen(20, 5);
    screen.feed(b"a line long enough to wrap across the old width");

    screen.resize(40, 8);
    let snapshot = screen.snapshot();

    assert_eq!((snapshot.columns, snapshot.rows), (40, 8));
    assert_eq!(screen.grid_for_test().columns(), 40);
    assert_eq!(screen.grid_for_test().visible_rows(), 8);
    assert!(screen.visible_text().contains("a line long enough"));
}

/// Every cell's appearance, in a form that is cheap to compare and readable
/// when it differs.
///
/// Deliberately not "compare the two rendered repaints": a renderer that drops
/// attributes uniformly produces equal bytes on both sides and satisfies that
/// comparison while losing every color. Reading the cells is what actually
/// detects the loss.
fn cell_appearance(screen: &HeadlessScreen) -> Vec<String> {
    let grid = screen.grid_for_test();
    let mut cells = Vec::new();
    for row in 0..grid.total_rows() {
        for column in 0..grid.columns() {
            let cell = &grid.grid_storage()[Point::new(row, column)];
            if cell.c == ' ' {
                continue;
            }
            cells.push(format!(
                "{:?} fg={:?} bg={:?} flags={:?}",
                cell.c, cell.fg, cell.bg, cell.flags
            ));
        }
    }
    cells
}

/// A reattached screen that kept its text but lost its colors is visibly wrong,
/// and `visible_text` cannot see that.
#[test]
fn text_attributes_survive_the_detach_boundary() {
    let styled = b"\x1b[31mred\x1b[0m plain \x1b[1mbold\x1b[0m";

    let mut continuous = screen(40, 3);
    continuous.feed(styled);

    let reattached =
        HeadlessScreen::from_snapshot(&continuous.snapshot(), ChannelEventListener::new_for_test());

    assert_eq!(
        cell_appearance(&continuous),
        cell_appearance(&reattached),
        "attributes were lost across the snapshot boundary"
    );
}

/// The case the whole design exists for. A full-screen application detached
/// mid-draw is what the raw byte spool got wrong: replaying its output paints
/// garbage.
#[test]
fn alternate_screen_survives_the_detach_boundary() {
    let draw = b"\x1b[?1049h\x1b[H\x1b[2Jtop row\r\n\r\nbottom row\x1b[2;3H";

    assert_detach_boundary_equivalence(draw, b" more");
}

/// Guards the test above against passing vacuously. If the snapshot dropped the
/// mode and both sides simply drew on the primary screen, their text would
/// still match while a real reattach landed on the wrong screen entirely.
#[test]
fn alternate_screen_mode_survives_the_detach_boundary() {
    let mut continuous = screen(20, 5);
    continuous.feed(b"\x1b[?1049hinside the tui");
    assert!(
        continuous.is_alternate_screen_active(),
        "test setup failed to enter the alternate screen"
    );

    let reattached =
        HeadlessScreen::from_snapshot(&continuous.snapshot(), ChannelEventListener::new_for_test());

    assert!(
        reattached.is_alternate_screen_active(),
        "reattached screen fell back to the primary screen"
    );
}

/// A scroll region set by a TUI decides where later output lands. If the
/// snapshot does not restore it, everything after reattach scrolls wrongly.
#[test]
fn scroll_region_survives_the_detach_boundary() {
    assert_detach_boundary_equivalence(b"\x1b[2;4r\x1b[3;1Hinside", b"\r\nnext");
}

/// Same guard as for the alternate screen: the equivalence check above compares
/// text, and a small fixture's text can match even when the region was lost.
/// This reads the region itself.
#[test]
fn scroll_region_is_restored_by_a_snapshot() {
    let mut continuous = screen(20, 5);
    continuous.feed(b"\x1b[2;4r");
    let expected = continuous.grid_for_test().scroll_region().clone();

    let reattached =
        HeadlessScreen::from_snapshot(&continuous.snapshot(), ChannelEventListener::new_for_test());

    assert_eq!(
        expected,
        reattached.grid_for_test().scroll_region().clone(),
        "scroll region was lost across the snapshot boundary"
    );
}

/// Wide characters occupy two cells. If the renderer emits the spacer cell as
/// its own character, every column after it shifts on reattach.
#[test]
fn wide_characters_survive_the_detach_boundary() {
    assert_detach_boundary_equivalence("日本語 text".as_bytes(), b" more");
}

/// Warp stamps OSC 8 hyperlinks into cells, but a snapshot does not carry them,
/// so a reattached session shows the text without the link. Recorded
/// deliberately: when the renderer learns to emit OSC 8, this test fails and
/// should be replaced by one asserting the link survives.
#[test]
fn hyperlinks_are_not_yet_carried_by_a_snapshot() {
    let linked = b"\x1b]8;;https://example.com\x1b\\link text\x1b]8;;\x1b\\";

    let mut screen = screen(40, 3);
    screen.feed(linked);

    assert!(
        !screen
            .snapshot()
            .repaint
            .windows(4)
            .any(|window| window == b"\x1b]8;"),
        "renderer now emits OSC 8 -- carry hyperlinks and update this test"
    );
}
