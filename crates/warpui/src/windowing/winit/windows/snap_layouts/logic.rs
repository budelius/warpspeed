//! The decisions the Snap Layouts window subclass makes, expressed without any Win32 calls so
//! they can be tested.

use pathfinder_geometry::rect::RectF;
use windows::Win32::Foundation::LPARAM;
use windows::Win32::System::SystemServices::MK_LBUTTON;
use windows::Win32::UI::WindowsAndMessaging::{
    HTCLIENT, HTMAXBUTTON, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_NCLBUTTONDBLCLK,
    WM_NCLBUTTONDOWN, WM_NCLBUTTONUP, WM_NCMOUSELEAVE, WM_NCMOUSEMOVE,
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

/// Converts the maximize button's logical bounds, as the app painted them, into the physical client
/// coordinates the hit test compares against.
///
/// The window's current scale factor is applied here, as the bounds arrive, so the window procedure
/// itself needs no coordinate conversion. A scale-factor change corrects itself on the next paint.
pub fn to_physical(bounds: RectF, scale_factor: f32) -> ButtonRect {
    ButtonRect {
        left: (bounds.min_x() * scale_factor).round() as i32,
        top: (bounds.min_y() * scale_factor).round() as i32,
        right: (bounds.max_x() * scale_factor).round() as i32,
        bottom: (bounds.max_y() * scale_factor).round() as i32,
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
/// hit code.
///
/// Moves are withheld while the synthetic press is active. winit classifies every move before its
/// matching release as a drag, including a fabricated hover-leave position outside the button. That
/// both leaks a drag into the UI behind the button and makes the release miss the button.
pub fn synthetic_message(
    message: u32,
    hit: u32,
    maximize_button_pressed: bool,
) -> Option<SyntheticMouseMessage> {
    match message {
        WM_NCMOUSEMOVE if hit == HTMAXBUTTON && !maximize_button_pressed => {
            Some(SyntheticMouseMessage {
                message: WM_MOUSEMOVE,
                wparam: 0,
            })
        }
        // A double-click is replayed as another press, which is exactly what the app saw before
        // Windows started routing these through the non-client area: a fast double-click used to be
        // two plain `WM_LBUTTONDOWN`s and two clean toggles. Left untranslated it would reach
        // `DefWindowProc`, which enters its own caption-button tracking loop and maximizes the
        // window behind the app-drawn button's back.
        WM_NCLBUTTONDOWN | WM_NCLBUTTONDBLCLK if hit == HTMAXBUTTON => {
            Some(SyntheticMouseMessage {
                message: WM_LBUTTONDOWN,
                wparam: MK_LBUTTON.0 as usize,
            })
        }
        WM_NCLBUTTONUP if hit == HTMAXBUTTON => Some(SyntheticMouseMessage {
            message: WM_LBUTTONUP,
            wparam: 0,
        }),
        WM_NCMOUSELEAVE if !maximize_button_pressed => Some(SyntheticMouseMessage {
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

/// Splits a mouse message's `LPARAM` into its signed 16-bit coordinates. The inverse of
/// `pack_coords`.
pub fn unpack_coords(lparam: LPARAM) -> (i32, i32) {
    let packed = lparam.0 as u32;
    (
        (packed & 0xFFFF) as u16 as i16 as i32,
        (packed >> 16) as u16 as i16 as i32,
    )
}

#[cfg(test)]
mod tests {
    use pathfinder_geometry::vector::Vector2F;
    use windows::Win32::UI::WindowsAndMessaging::{
        HTCAPTION, HTCLIENT, HTMAXBUTTON, HTTOP, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE,
        WM_NCLBUTTONDBLCLK, WM_NCLBUTTONDOWN, WM_NCLBUTTONUP, WM_NCMOUSELEAVE, WM_NCMOUSEMOVE,
        WM_NCRBUTTONDOWN,
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
    fn hit_test_claims_the_inclusive_left_and_top_edges() {
        // The left and top edges belong to the button; the right and bottom ones do not. Pinning
        // both halves keeps the rectangle's edge convention from drifting.
        assert_eq!(hit_test(HTCLIENT, 908, 20, Some(button())), HTMAXBUTTON);
        assert_eq!(hit_test(HTCLIENT, 930, 0, Some(button())), HTMAXBUTTON);
        assert_eq!(hit_test(HTCLIENT, 908, 0, Some(button())), HTMAXBUTTON);
        assert_eq!(hit_test(HTCLIENT, 954, 20, Some(button())), HTCLIENT);
        assert_eq!(hit_test(HTCLIENT, 930, 35, Some(button())), HTCLIENT);
    }

    #[test]
    fn hit_test_ignores_everything_when_no_button_is_on_screen() {
        assert_eq!(hit_test(HTCLIENT, 930, 20, None), HTCLIENT);
    }

    #[test]
    fn non_client_mouse_messages_become_client_ones() {
        assert_eq!(
            synthetic_message(WM_NCMOUSEMOVE, HTMAXBUTTON, false),
            Some(SyntheticMouseMessage {
                message: WM_MOUSEMOVE,
                wparam: 0,
            })
        );
        assert_eq!(
            synthetic_message(WM_NCLBUTTONDOWN, HTMAXBUTTON, false),
            Some(SyntheticMouseMessage {
                message: WM_LBUTTONDOWN,
                wparam: MK_LBUTTON.0 as usize,
            })
        );
        assert_eq!(
            synthetic_message(WM_NCLBUTTONUP, HTMAXBUTTON, true),
            Some(SyntheticMouseMessage {
                message: WM_LBUTTONUP,
                wparam: 0,
            })
        );
    }

    #[test]
    fn a_double_click_is_replayed_as_another_press() {
        // Without this the message reaches `DefWindowProc`, which runs its own caption-button
        // tracking loop and maximizes the window while the app-drawn button shows no feedback.
        assert_eq!(
            synthetic_message(WM_NCLBUTTONDBLCLK, HTMAXBUTTON, false),
            Some(SyntheticMouseMessage {
                message: WM_LBUTTONDOWN,
                wparam: MK_LBUTTON.0 as usize,
            })
        );
        assert_eq!(
            synthetic_message(WM_NCLBUTTONDBLCLK, HTCAPTION, false),
            None
        );
    }

    #[test]
    fn leaving_the_non_client_area_ends_the_hover() {
        // WM_NCMOUSELEAVE carries no hit code, so it is translated regardless of wParam.
        assert_eq!(
            synthetic_message(WM_NCMOUSELEAVE, 0, false),
            Some(SyntheticMouseMessage {
                message: WM_MOUSEMOVE,
                wparam: 0,
            })
        );
    }

    #[test]
    fn messages_outside_the_button_are_not_translated() {
        assert_eq!(synthetic_message(WM_NCMOUSEMOVE, HTCAPTION, false), None);
        assert_eq!(synthetic_message(WM_NCLBUTTONDOWN, HTCAPTION, false), None);
        assert_eq!(
            synthetic_message(WM_NCRBUTTONDOWN, HTMAXBUTTON, false),
            None
        );
    }

    #[test]
    fn non_client_moves_are_withheld_during_a_synthetic_press() {
        assert_eq!(synthetic_message(WM_NCMOUSEMOVE, HTMAXBUTTON, true), None);
        assert_eq!(synthetic_message(WM_NCMOUSELEAVE, 0, true), None);
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

    /// The same button as [`button`], in logical pixels: 46 wide, 35 tall, right-aligned in a
    /// 1000-pixel-wide window.
    fn logical_button() -> RectF {
        RectF::new(Vector2F::new(908., 0.), Vector2F::new(46., 35.))
    }

    #[test]
    fn logical_bounds_scale_to_physical_client_coordinates() {
        assert_eq!(
            to_physical(logical_button(), 1.0),
            ButtonRect {
                left: 908,
                top: 0,
                right: 954,
                bottom: 35,
            }
        );
        // 125%: the right and bottom edges land on a half pixel and round away from zero.
        assert_eq!(
            to_physical(logical_button(), 1.25),
            ButtonRect {
                left: 1135,
                top: 0,
                right: 1193,
                bottom: 44,
            }
        );
        assert_eq!(
            to_physical(logical_button(), 1.5),
            ButtonRect {
                left: 1362,
                top: 0,
                right: 1431,
                bottom: 53,
            }
        );
        assert_eq!(
            to_physical(logical_button(), 2.0),
            ButtonRect {
                left: 1816,
                top: 0,
                right: 1908,
                bottom: 70,
            }
        );
    }

    #[test]
    fn scaling_preserves_the_button_away_from_the_window_origin() {
        // A button that is not flush against the top of the window, so a scale factor applied to
        // the origin rather than to both edges would be visible.
        let bounds = RectF::new(Vector2F::new(100., 8.), Vector2F::new(46., 35.));
        assert_eq!(
            to_physical(bounds, 2.0),
            ButtonRect {
                left: 200,
                top: 16,
                right: 292,
                bottom: 86,
            }
        );
    }

    #[test]
    fn coordinates_round_trip_through_pack_and_unpack() {
        assert_eq!(unpack_coords(LPARAM(pack_coords(930, 20))), (930, 20));
        // Negative coordinates occur while the mouse is captured outside the window; this pins the
        // sign-extension in `unpack_coords`.
        assert_eq!(unpack_coords(LPARAM(pack_coords(-5, -10))), (-5, -10));
    }
}
