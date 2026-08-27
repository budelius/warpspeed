use crate::event_listener::ChannelEventListener;

use super::*;

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
