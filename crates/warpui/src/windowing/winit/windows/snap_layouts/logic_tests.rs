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
