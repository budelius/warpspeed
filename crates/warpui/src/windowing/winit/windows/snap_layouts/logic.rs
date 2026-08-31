//! The decisions the Snap Layouts window subclass makes, expressed without any Win32 calls so
//! they can be tested.

use windows::Win32::System::SystemServices::MK_LBUTTON;
use windows::Win32::UI::WindowsAndMessaging::{
    HTCLIENT, HTMAXBUTTON, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_NCLBUTTONDOWN,
    WM_NCLBUTTONUP, WM_NCMOUSELEAVE, WM_NCMOUSEMOVE,
};

/// The bounds of the app-drawn maximize button, in physical pixels relative to the top-left of the
/// window's client area.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ButtonRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl ButtonRect {
    /// Whether the point falls inside the button. The right and bottom edges are exclusive, which
    /// matches how Win32 treats rectangles.
    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.left && x < self.right && y >= self.top && y < self.bottom
    }
}

/// The answer to give Windows for a `WM_NCHITTEST`.
///
/// `default_answer` is what the original window procedure answered. Only `HTCLIENT` is overridden,
/// which is what keeps the resize borders working: they answer `HTTOP` or `HTTOPRIGHT` over the top
/// few pixels of the button and pass through untouched.
pub fn hit_test(
    default_answer: u32,
    client_x: i32,
    client_y: i32,
    button: Option<ButtonRect>,
) -> u32 {
    if default_answer != HTCLIENT {
        return default_answer;
    }

    match button {
        Some(button) if button.contains(client_x, client_y) => HTMAXBUTTON,
        _ => default_answer,
    }
}

/// A client-area mouse message to post in place of a non-client one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SyntheticMouseMessage {
    pub message: u32,
    pub wparam: usize,
}

/// The client-area message a non-client message is replayed as, if any.
///
/// Once the hit test answers `HTMAXBUTTON`, Windows delivers non-client mouse messages for that
/// rectangle and no client-area ones, which would leave the app-drawn button unable to see its own
/// hover and clicks. `hit` is the hit code Windows passes in `wParam`; `WM_NCMOUSELEAVE` carries no
/// hit code and is always translated.
pub fn synthetic_message(message: u32, hit: u32) -> Option<SyntheticMouseMessage> {
    match message {
        WM_NCMOUSEMOVE if hit == HTMAXBUTTON => Some(SyntheticMouseMessage {
            message: WM_MOUSEMOVE,
            wparam: 0,
        }),
        WM_NCLBUTTONDOWN if hit == HTMAXBUTTON => Some(SyntheticMouseMessage {
            message: WM_LBUTTONDOWN,
            wparam: MK_LBUTTON.0 as usize,
        }),
        WM_NCLBUTTONUP if hit == HTMAXBUTTON => Some(SyntheticMouseMessage {
            message: WM_LBUTTONUP,
            wparam: 0,
        }),
        WM_NCMOUSELEAVE => Some(SyntheticMouseMessage {
            message: WM_MOUSEMOVE,
            wparam: 0,
        }),
        _ => None,
    }
}

/// Where to aim the synthetic move that ends a hover.
///
/// `WM_NCMOUSELEAVE` carries no position, so the caller supplies the cursor's current client
/// position. When the cursor is still inside the button — which happens when it left through the
/// Snap Layouts flyout — the move is aimed just below the button instead, so the button sees the
/// pointer leave.
pub fn leave_position(cursor: (i32, i32), button: Option<ButtonRect>) -> (i32, i32) {
    match button {
        Some(button) if button.contains(cursor.0, cursor.1) => (button.left, button.bottom + 1),
        _ => cursor,
    }
}

/// Packs client coordinates into the `LPARAM` layout mouse messages use: x in the low word, y in
/// the high word, both signed 16-bit.
pub fn pack_coords(x: i32, y: i32) -> isize {
    let packed = ((y as u32) << 16) | ((x as u32) & 0xFFFF);
    packed as i32 as isize
}

#[cfg(test)]
mod tests {
    use windows::Win32::UI::WindowsAndMessaging::{
        HTCAPTION, HTCLIENT, HTMAXBUTTON, HTTOP, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE,
        WM_NCLBUTTONDOWN, WM_NCLBUTTONUP, WM_NCMOUSELEAVE, WM_NCMOUSEMOVE, WM_NCRBUTTONDOWN,
    };

    use super::*;

    /// The maximize button as it sits in a 1000-pixel-wide window at 100% scaling: 46 pixels wide,
    /// the middle of the three caption buttons, against the top edge.
    fn button() -> ButtonRect {
        ButtonRect {
            left: 908,
            top: 0,
            right: 954,
            bottom: 35,
        }
    }

    #[test]
    fn hit_test_claims_points_inside_the_button() {
        assert_eq!(
            hit_test(HTCLIENT, 930, 20, Some(button())),
            HTMAXBUTTON,
            "a point inside the button must be reported as the maximize button"
        );
    }

    #[test]
    fn hit_test_leaves_the_resize_border_alone() {
        // The top resize border overlaps the button. Windows answers HTTOP there, and that answer
        // has to survive or the window can no longer be resized from its top edge.
        assert_eq!(hit_test(HTTOP, 930, 1, Some(button())), HTTOP);
    }

    #[test]
    fn hit_test_leaves_other_answers_alone() {
        assert_eq!(hit_test(HTCAPTION, 930, 20, Some(button())), HTCAPTION);
    }

    #[test]
    fn hit_test_ignores_points_outside_the_button() {
        // Just left of the button, and just below it.
        assert_eq!(hit_test(HTCLIENT, 907, 20, Some(button())), HTCLIENT);
        assert_eq!(hit_test(HTCLIENT, 930, 35, Some(button())), HTCLIENT);
    }

    #[test]
    fn hit_test_ignores_everything_when_no_button_is_on_screen() {
        assert_eq!(hit_test(HTCLIENT, 930, 20, None), HTCLIENT);
    }

    #[test]
    fn non_client_mouse_messages_become_client_ones() {
        assert_eq!(
            synthetic_message(WM_NCMOUSEMOVE, HTMAXBUTTON),
            Some(SyntheticMouseMessage {
                message: WM_MOUSEMOVE,
                wparam: 0,
            })
        );
        assert_eq!(
            synthetic_message(WM_NCLBUTTONDOWN, HTMAXBUTTON),
            Some(SyntheticMouseMessage {
                message: WM_LBUTTONDOWN,
                wparam: MK_LBUTTON.0 as usize,
            })
        );
        assert_eq!(
            synthetic_message(WM_NCLBUTTONUP, HTMAXBUTTON),
            Some(SyntheticMouseMessage {
                message: WM_LBUTTONUP,
                wparam: 0,
            })
        );
    }

    #[test]
    fn leaving_the_non_client_area_ends_the_hover() {
        // WM_NCMOUSELEAVE carries no hit code, so it is translated regardless of wParam.
        assert_eq!(
            synthetic_message(WM_NCMOUSELEAVE, 0),
            Some(SyntheticMouseMessage {
                message: WM_MOUSEMOVE,
                wparam: 0,
            })
        );
    }

    #[test]
    fn messages_outside_the_button_are_not_translated() {
        assert_eq!(synthetic_message(WM_NCMOUSEMOVE, HTCAPTION), None);
        assert_eq!(synthetic_message(WM_NCLBUTTONDOWN, HTCAPTION), None);
        assert_eq!(synthetic_message(WM_NCRBUTTONDOWN, HTMAXBUTTON), None);
    }

    #[test]
    fn leave_position_moves_out_of_the_button_when_the_cursor_is_still_inside_it() {
        assert_eq!(leave_position((930, 20), Some(button())), (908, 36));
    }

    #[test]
    fn leave_position_keeps_a_cursor_that_is_already_outside() {
        assert_eq!(leave_position((400, 300), Some(button())), (400, 300));
        assert_eq!(leave_position((400, 300), None), (400, 300));
    }

    #[test]
    fn coordinates_pack_into_an_lparam() {
        assert_eq!(pack_coords(0, 0), 0);
        assert_eq!(pack_coords(930, 20), (20 << 16) | 930);
        // Negative coordinates occur while the mouse is captured outside the window.
        assert_eq!(pack_coords(-1, -1), 0xFFFF_FFFFu32 as i32 as isize);
    }
}
