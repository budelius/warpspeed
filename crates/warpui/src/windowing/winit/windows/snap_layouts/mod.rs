//! Windows 11 Snap Layouts support for Warp's app-drawn maximize button.
//!
//! Warp's windows are undecorated and the caption buttons are drawn by the app, so Windows never
//! sees the `HTMAXBUTTON` hit-test answer that makes it offer the Snap Layouts flyout. The window
//! subclass in this module supplies that answer, and replays the non-client mouse messages that
//! follow as ordinary client-area ones so the app-drawn button keeps behaving normally.

use std::sync::{Arc, Mutex, Once};

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    TME_LEAVE, TME_NONCLIENT, TRACKMOUSEEVENT, TrackMouseEvent,
};
use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, HTMAXBUTTON, PostMessageW, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE,
    WM_NCDESTROY, WM_NCHITTEST, WM_NCLBUTTONDBLCLK, WM_NCLBUTTONDOWN, WM_NCLBUTTONUP,
    WM_NCMOUSELEAVE, WM_NCMOUSEMOVE,
};
use winit::window::Window as WinitWindow;

use super::is_windows_11;
use super::window_attribute::to_hwnd;

mod logic;

pub use logic::*;

/// The state the window subclass reads while answering the hit test.
///
/// Shared between the warpui window, which publishes the button's bounds as the app paints it, and
/// the subclass, which runs inside the window procedure.
#[derive(Debug, Default)]
pub struct SnapLayoutsState {
    button: Mutex<Option<ButtonRect>>,
    /// Whether the window procedure believes the pointer is currently over the maximize button.
    ///
    /// Only ever touched from the message thread; the `Mutex` is here to match the field above and
    /// to keep the state shareable, not because there is contention to resolve.
    hovered: Mutex<bool>,
    /// Whether the synthetic client-area press is waiting for its release.
    pressed: Mutex<bool>,
}

impl SnapLayoutsState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Publishes the maximize button's bounds, or `None` when no maximize button is on screen.
    pub fn set_button_rect(&self, rect: Option<ButtonRect>) {
        if let Ok(mut button) = self.button.lock() {
            *button = rect;
        }
    }

    /// The maximize button's bounds, if one is on screen.
    pub fn button_rect(&self) -> Option<ButtonRect> {
        self.button.lock().ok().and_then(|button| *button)
    }

    /// Records that the pointer is over the maximize button.
    fn note_hovered(&self) {
        if let Ok(mut hovered) = self.hovered.lock() {
            *hovered = true;
        }
    }

    /// Clears the recorded hover, reporting whether one was in effect.
    fn take_hovered(&self) -> bool {
        match self.hovered.lock() {
            Ok(mut hovered) => std::mem::replace(&mut hovered, false),
            Err(_) => false,
        }
    }

    fn note_pressed(&self) {
        if let Ok(mut pressed) = self.pressed.lock() {
            *pressed = true;
        }
    }

    fn clear_pressed(&self) {
        if let Ok(mut pressed) = self.pressed.lock() {
            *pressed = false;
        }
    }

    fn is_pressed(&self) -> bool {
        self.pressed.lock().is_ok_and(|pressed| *pressed)
    }
}

/// Distinguishes our subclass from any other on the same window.
const SUBCLASS_ID: usize = 0x5741_5250;

/// Installs the Snap Layouts subclass on `window`, returning the state to publish the maximize
/// button's bounds through.
///
/// Returns `None`, leaving the window untouched, on anything before Windows 11 and on a window that
/// has the native frame, which draws its own caption buttons.
pub fn install(window: &WinitWindow) -> Option<Arc<SnapLayoutsState>> {
    if !is_windows_11() {
        // Every window open on Windows 10 takes this path, so this is not worth surfacing above
        // debug level.
        log::debug!("Snap Layouts is not supported before Windows 11");
        return None;
    }

    if window.is_decorated() {
        return None;
    }

    let hwnd = to_hwnd(window)
        .inspect_err(|err| {
            log::warn!("Couldn't install the Snap Layouts subclass: {err:?}");
        })
        .ok()?;

    let state = Arc::new(SnapLayoutsState::new());
    // The subclass owns a reference for as long as the window lives; `WM_NCDESTROY` reclaims it.
    let reference_data = Arc::into_raw(Arc::clone(&state)) as usize;

    let installed =
        unsafe { SetWindowSubclass(hwnd, Some(subclass_proc), SUBCLASS_ID, reference_data) };
    if !installed.as_bool() {
        // SAFETY: the pointer came from `Arc::into_raw` immediately above and the subclass never
        // took ownership of it, so this is the only reclaim.
        drop(unsafe { Arc::from_raw(reference_data as *const SnapLayoutsState) });
        log::warn!("Couldn't install the Snap Layouts subclass");
        return None;
    }

    Some(state)
}

/// Sits in front of winit's window procedure, answering the maximize button's hit test and
/// replaying the non-client mouse messages that answer produces.
unsafe extern "system" fn subclass_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _subclass_id: usize,
    reference_data: usize,
) -> LRESULT {
    // SAFETY: `install` leaked an `Arc` into `reference_data`, and `WM_NCDESTROY` below reclaims it
    // only after the last message has been handled.
    let state = unsafe { &*(reference_data as *const SnapLayoutsState) };

    match message {
        WM_NCHITTEST => {
            let default_answer = unsafe { DefSubclassProc(hwnd, message, wparam, lparam) };
            let button = state.button_rect();
            if button.is_none() {
                return default_answer;
            }

            let (screen_x, screen_y) = unpack_coords(lparam);
            let Some((client_x, client_y)) = screen_to_client(hwnd, screen_x, screen_y) else {
                return default_answer;
            };

            // `hit_test` only ever overrides an `HTCLIENT` answer with `HTMAXBUTTON`; every other
            // answer, including negative ones like `HTTRANSPARENT`, must come back bit-identical to
            // what `DefSubclassProc` returned, so we return `default_answer` itself rather than
            // rebuild an `LRESULT` from the `u32` hit test result.
            if hit_test(default_answer.0 as u32, client_x, client_y, button) == HTMAXBUTTON {
                LRESULT(HTMAXBUTTON as isize)
            } else {
                default_answer
            }
        }
        WM_LBUTTONUP => {
            state.clear_pressed();
            unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
        }
        WM_NCMOUSEMOVE | WM_NCMOUSELEAVE | WM_NCLBUTTONDOWN | WM_NCLBUTTONUP
        | WM_NCLBUTTONDBLCLK => {
            let button_pressed = state.is_pressed();
            let Some(synthetic) = synthetic_message(message, wparam.0 as u32, button_pressed)
            else {
                // Non-client activity that should not be translated. If we still believe the
                // button is hovered, an unpressed move outside it is the pointer leaving it, and it
                // is the only notice we are going to get: winit answers our synthetic
                // `WM_MOUSEMOVE` by arming its own client-only `TrackMouseEvent`, which replaces
                // the `TME_NONCLIENT` request below, so `WM_NCMOUSELEAVE` may never arrive. End the
                // hover here, and let the message itself go on to `DefSubclassProc` so the resize
                // borders and the rest of the non-client area keep working exactly as before.
                //
                // While pressed, even a move posted only to end the hover becomes a drag in winit
                // and changes the position used for the eventual release. Wait for the real client
                // release instead.
                if message == WM_NCMOUSEMOVE && !button_pressed && state.take_hovered() {
                    let (screen_x, screen_y) = unpack_coords(lparam);
                    if let Some(cursor) = screen_to_client(hwnd, screen_x, screen_y) {
                        let (x, y) = leave_position(cursor, state.button_rect());
                        post_synthetic(hwnd, WM_MOUSEMOVE, 0, x, y);
                    }
                }

                return unsafe { DefSubclassProc(hwnd, message, wparam, lparam) };
            };

            if message == WM_NCLBUTTONUP {
                state.clear_pressed();
            }

            let position = if message == WM_NCMOUSELEAVE {
                // WM_NCMOUSELEAVE carries no position of its own.
                cursor_client_position(hwnd)
                    .map(|cursor| leave_position(cursor, state.button_rect()))
            } else {
                let (screen_x, screen_y) = unpack_coords(lparam);
                screen_to_client(hwnd, screen_x, screen_y)
            };

            let Some((x, y)) = position else {
                return unsafe { DefSubclassProc(hwnd, message, wparam, lparam) };
            };

            match message {
                WM_NCMOUSEMOVE => {
                    track_mouse_leave(hwnd);
                    state.note_hovered();
                }
                // Belt and braces: when the request above does survive, this is the tidier way for
                // the hover to end.
                WM_NCMOUSELEAVE => {
                    state.take_hovered();
                }
                WM_NCLBUTTONDOWN | WM_NCLBUTTONDBLCLK => state.note_pressed(),
                WM_NCLBUTTONUP => {}
                _ => {}
            }

            if synthetic.message == WM_LBUTTONDOWN {
                // winit's mouse-button event has no position and reuses its last `WM_MOUSEMOVE`.
                // A hover-leave may have deliberately moved that position below the button, so
                // restore the real press position before replaying the button down.
                post_synthetic(hwnd, WM_MOUSEMOVE, 0, x, y);
            }
            post_synthetic(hwnd, synthetic.message, synthetic.wparam, x, y);

            // Swallow the original. Left alone, `DefWindowProc` would open the system menu on a
            // press over a caption button, and run its own tracking loop on a double-click.
            LRESULT(0)
        }
        WM_NCDESTROY => unsafe {
            let _ = RemoveWindowSubclass(hwnd, Some(subclass_proc), SUBCLASS_ID);
            let result = DefSubclassProc(hwnd, message, wparam, lparam);
            // SAFETY: reclaims the reference `install` leaked. No further message reaches this
            // procedure once the subclass has been removed.
            drop(Arc::from_raw(reference_data as *const SnapLayoutsState));
            result
        },
        _ => unsafe { DefSubclassProc(hwnd, message, wparam, lparam) },
    }
}

/// Posts a client-area mouse message at the given client coordinates.
fn post_synthetic(hwnd: HWND, message: u32, wparam: usize, x: i32, y: i32) {
    unsafe {
        let _ = PostMessageW(
            Some(hwnd),
            message,
            WPARAM(wparam),
            LPARAM(pack_coords(x, y)),
        );
    }
}

fn screen_to_client(hwnd: HWND, x: i32, y: i32) -> Option<(i32, i32)> {
    let mut point = POINT { x, y };
    let converted = unsafe { ScreenToClient(hwnd, &mut point) };
    converted.as_bool().then_some((point.x, point.y))
}

fn cursor_client_position(hwnd: HWND) -> Option<(i32, i32)> {
    let mut point = POINT::default();
    unsafe { GetCursorPos(&mut point) }.ok()?;
    screen_to_client(hwnd, point.x, point.y)
}

/// Asks for a `WM_NCMOUSELEAVE` when the pointer leaves the non-client area.
///
/// This request does not always survive: winit classifies our synthetic `WM_MOUSEMOVE` by client
/// rectangle containment, decides the pointer has entered the client area, and arms its own
/// client-only `TrackMouseEvent`, which replaces this one. The window procedure therefore also ends
/// the hover from the first non-client move away from the button, and treats `WM_NCMOUSELEAVE` as
/// the tidier of two paths rather than as the only one.
///
/// Called on every unpressed `WM_NCMOUSEMOVE` over the button, so a failure is only logged once per
/// process rather than on every mouse move.
fn track_mouse_leave(hwnd: HWND) {
    static WARNED: Once = Once::new();

    let mut event = TRACKMOUSEEVENT {
        cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
        dwFlags: TME_LEAVE | TME_NONCLIENT,
        hwndTrack: hwnd,
        dwHoverTime: 0,
    };

    if let Err(err) = unsafe { TrackMouseEvent(&mut event) } {
        WARNED.call_once(|| {
            log::warn!("Couldn't track the pointer leaving the maximize button: {err:?}");
        });
    }
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
