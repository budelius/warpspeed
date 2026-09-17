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
/// hit code and is always translated.
pub fn synthetic_message(message: u32, hit: u32) -> Option<SyntheticMouseMessage> {
    match message {
        WM_NCMOUSEMOVE if hit == HTMAXBUTTON => Some(SyntheticMouseMessage {
            message: WM_MOUSEMOVE,
            wparam: 0,
        }),
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
#[path = "logic_tests.rs"]
mod tests;
