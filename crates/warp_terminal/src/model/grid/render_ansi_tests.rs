use crate::event_listener::ChannelEventListener;
use crate::local_tty::screen::HeadlessScreen;

use super::*;

/// The repaint must put the terminal into a known state before drawing.
/// Without that, a snapshot fed into a screen still carrying state from earlier
/// output reconstructs the wrong thing.
#[test]
fn repaint_erases_before_drawing() {
    let screen = HeadlessScreen::new(10, 3, ChannelEventListener::new_for_test());
    let bytes = render_screen(screen.grid_for_test());

    let prefix: Vec<u8> = bytes.iter().copied().take(16).collect();
    assert!(
        bytes.windows(4).any(|window| window == b"\x1b[2J"),
        "repaint must erase the screen, got {:?}",
        String::from_utf8_lossy(&prefix)
    );
}
