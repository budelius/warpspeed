use super::*;
use crate::event_listener::ChannelEventListener;
use crate::local_tty::screen::HeadlessScreen;
use crate::model::ansi;

/// The repaint must put the terminal into a known state before drawing.
/// Without that, a snapshot fed into a screen still carrying state from earlier
/// output reconstructs the wrong thing.
///
/// Asserted on the resulting screen rather than on the erase sequence itself.
/// The erase is per-row (`CSI K`), not a single leading `CSI 2J`, because on
/// Warp's primary-screen grid `CSI 2J` preserves the visible rows as block
/// scrollback instead of blanking them -- so a repaint that erased that way
/// appended a second copy of the frame. See `render_screen`.
#[test]
fn repaint_erases_before_drawing() {
    let mut source = HeadlessScreen::new(10, 3, ChannelEventListener::new_for_test());
    source.feed(b"new");
    let bytes = render_screen(source.grid_for_test());

    let mut receiver = HeadlessScreen::new(10, 3, ChannelEventListener::new_for_test());
    receiver.feed(b"stale1\r\nstale2\r\nstale3");
    receiver.feed(&bytes);

    assert_eq!(
        receiver.visible_text().trim_end(),
        source.visible_text().trim_end(),
        "repaint did not erase what the receiving screen already held"
    );
}

#[test]
fn repaint_ignores_scrollback_when_rendering_visible_screen() {
    let mut source = GridHandler::new_for_test_with_scroll_limit(2, 10, 10);
    ansi::Processor::new().parse_bytes(
        &mut source,
        b"old\r\nvisible1\r\nvisible2",
        &mut std::io::sink(),
    );
    assert_eq!(source.history_size(), 1);

    let bytes = render_screen(&source);
    let mut receiver = HeadlessScreen::new(10, 2, ChannelEventListener::new_for_test());
    receiver.feed(&bytes);

    assert_eq!(receiver.visible_text().trim_end(), "visible1\nvisible2");
}

#[test]
fn repaint_cursor_position_is_relative_to_visible_screen() {
    let mut source = GridHandler::new_for_test_with_scroll_limit(2, 10, 10);
    ansi::Processor::new().parse_bytes(
        &mut source,
        b"old\r\nvisible1\r\nvisible2\x1b[1;3H",
        &mut std::io::sink(),
    );
    assert_eq!(source.history_size(), 1);

    let bytes = render_screen(&source);

    assert!(
        bytes.ends_with(b"\x1b[1;3H"),
        "repaint restored the cursor outside the visible screen: {:?}",
        String::from_utf8_lossy(&bytes)
    );
}
