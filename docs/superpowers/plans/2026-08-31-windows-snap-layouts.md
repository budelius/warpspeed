# Windows 11 Snap Layouts Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the Windows 11 Snap Layouts flyout open when the pointer rests on Warp's app-drawn maximize button, without changing how that button looks or behaves.

**Architecture:** A Windows-only window subclass sits in front of winit's window procedure. It answers `WM_NCHITTEST` with `HTMAXBUTTON` when the point falls inside the maximize button, which is the one thing Windows requires before it offers the flyout, and it replays the resulting non-client mouse messages as ordinary client-area ones so the existing `Hoverable` button keeps its hover, press, and click behavior. The button's rectangle reaches the subclass from paint, through a new `set_maximize_button_bounds` method on the platform window trait.

**Tech Stack:** Rust 2024, `windows` crate 0.62.2, winit (the pinned `warpdotdev/winit` fork), warpui / warpui_core (Warp's own UI framework).

**Design document:** `docs/superpowers/specs/2026-08-31-windows-snap-layouts-design.md`

## Global Constraints

- Every change outside the app crate's traffic-light module is compiled only on Windows, behind `#[cfg(windows)]` or `#[cfg(target_os = "windows")]`. macOS, Linux, WASM, headless and test builds must be unaffected.
- The subclass is installed only on Windows 11, meaning `windows_version::OsVersion::current().build >= 22000`. Windows 10 must take no new code path at all.
- No changes to the appearance, size, or position of the caption buttons.
- The maximize button's rectangle is published from paint, never from a view method. A window resize re-lays-out the existing element tree without re-running `Workspace::render`, so anything published from a render-time call site goes stale.
- Bounds cross the platform boundary in **logical** pixels and are converted to **physical** pixels on arrival, using the window's current scale factor.
- Follow the repo's existing conventions: `report_error!` for errors worth surfacing, `log::warn!` for degradations, doc comments on public items.
- Commit messages follow Conventional Commits, as in `git log`.

---

## File Structure

**Created:**

- `crates/warpui/src/windowing/winit/windows/snap_layouts/logic.rs` — pure decisions: the hit-test answer, the non-client to client message mapping, coordinate packing. No Win32 calls, so it is unit testable.
- `crates/warpui/src/windowing/winit/windows/snap_layouts/mod.rs` — the shared state, the subclass installation, and the window procedure. All Win32 glue lives here.
- `app/src/util/traffic_lights/windows/maximize_button_region.rs` — the wrapper element that reports where the maximize button was painted.

**Modified:**

- `crates/warpui/Cargo.toml` — one added `windows` crate feature.
- `crates/warpui/src/windowing/winit/windows/mod.rs` — declare and re-export the new module.
- `crates/warpui/src/windowing/winit/windows/window_attribute.rs` — widen `to_hwnd` visibility.
- `crates/warpui_core/src/platform/mod.rs` — the new trait method.
- `crates/warpui_core/src/platform/test/delegate.rs`, `crates/warpui/src/platform/headless/windowing.rs`, `crates/warpui/src/platform/mac/window.rs` — no-op implementations.
- `crates/warpui/src/windowing/winit/window.rs` — hold the state, install the subclass, implement the trait method.
- `app/src/util/traffic_lights.rs` — carry the window id on `TrafficLightData`.
- `app/src/util/traffic_lights/windows/mod.rs`, `app/src/util/traffic_lights/windows/renderer.rs` — wrap the maximize button.
- `app/src/workspace/view.rs` — clear the rectangle when the traffic lights are not rendered.
- `CHANGELOG.md` — record the change.

---

## Task 1: Pure hit-test and message-translation logic

**Files:**
- Create: `crates/warpui/src/windowing/winit/windows/snap_layouts/logic.rs`
- Create: `crates/warpui/src/windowing/winit/windows/snap_layouts/mod.rs`
- Modify: `crates/warpui/src/windowing/winit/windows/mod.rs`

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `pub struct ButtonRect { pub left: i32, pub top: i32, pub right: i32, pub bottom: i32 }` with `pub fn contains(&self, x: i32, y: i32) -> bool`
  - `pub fn hit_test(default_answer: u32, client_x: i32, client_y: i32, button: Option<ButtonRect>) -> u32`
  - `pub struct SyntheticMouseMessage { pub message: u32, pub wparam: usize }`
  - `pub fn synthetic_message(message: u32, hit: u32) -> Option<SyntheticMouseMessage>`
  - `pub fn leave_position(cursor: (i32, i32), button: Option<ButtonRect>) -> (i32, i32)`
  - `pub fn pack_coords(x: i32, y: i32) -> isize`

- [ ] **Step 1: Create the module skeleton so the crate still compiles**

Create `crates/warpui/src/windowing/winit/windows/snap_layouts/mod.rs`:

```rust
//! Windows 11 Snap Layouts support for Warp's app-drawn maximize button.
//!
//! Warp's windows are undecorated and the caption buttons are drawn by the app, so Windows never
//! sees the `HTMAXBUTTON` hit-test answer that makes it offer the Snap Layouts flyout. The window
//! subclass in this module supplies that answer, and replays the non-client mouse messages that
//! follow as ordinary client-area ones so the app-drawn button keeps behaving normally.

mod logic;

pub use logic::*;
```

Add to `crates/warpui/src/windowing/winit/windows/mod.rs`, keeping both lists alphabetical:

```rust
mod snap_layouts;
```

and in the re-export block:

```rust
pub use snap_layouts::*;
```

- [ ] **Step 2: Write the failing tests**

Create `crates/warpui/src/windowing/winit/windows/snap_layouts/logic.rs` containing only the tests for now:

```rust
#[cfg(test)]
mod tests {
    use windows::Win32::UI::WindowsAndMessaging::{
        HTCAPTION, HTCLIENT, HTMAXBUTTON, HTTOP, MK_LBUTTON, WM_LBUTTONDOWN, WM_LBUTTONUP,
        WM_MOUSEMOVE, WM_NCLBUTTONDOWN, WM_NCLBUTTONUP, WM_NCMOUSELEAVE, WM_NCMOUSEMOVE,
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
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p warpui --lib snap_layouts`
Expected: FAIL to compile, with `cannot find type ButtonRect in this scope` and similar errors for every item under test.

- [ ] **Step 4: Write the implementation**

Add above the `tests` module in `crates/warpui/src/windowing/winit/windows/snap_layouts/logic.rs`:

```rust
//! The decisions the Snap Layouts window subclass makes, expressed without any Win32 calls so
//! they can be tested.

use windows::Win32::UI::WindowsAndMessaging::{
    HTCLIENT, HTMAXBUTTON, MK_LBUTTON, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE,
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
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p warpui --lib snap_layouts`
Expected: PASS, 11 tests.

- [ ] **Step 6: Check formatting and lints**

Run: `cargo fmt -p warpui && cargo clippy -p warpui --all-targets -- -D warnings`
Expected: no diagnostics.

- [ ] **Step 7: Commit**

```bash
git add crates/warpui/src/windowing/winit/windows/snap_layouts crates/warpui/src/windowing/winit/windows/mod.rs
git commit -m "feat(warpui): add the Snap Layouts hit-test and message-translation logic"
```

---

## Task 2: Carry the button's bounds to the platform window

**Files:**
- Modify: `crates/warpui_core/src/platform/mod.rs:448-466`
- Modify: `crates/warpui_core/src/platform/test/delegate.rs:426`
- Modify: `crates/warpui/src/platform/headless/windowing.rs:210`
- Modify: `crates/warpui/src/platform/mac/window.rs:1160`
- Modify: `crates/warpui/src/windowing/winit/windows/snap_layouts/mod.rs`
- Modify: `crates/warpui/src/windowing/winit/window.rs:717-739`

**Interfaces:**
- Consumes: `ButtonRect` from Task 1.
- Produces:
  - `platform::Window::set_maximize_button_bounds(&self, bounds: Option<RectF>)` on the trait in `warpui_core`
  - `pub struct SnapLayoutsState` with `pub fn new() -> Self`, `pub fn set_button_rect(&self, rect: Option<ButtonRect>)`, `pub fn button_rect(&self) -> Option<ButtonRect>`

- [ ] **Step 1: Add the shared state**

In `crates/warpui/src/windowing/winit/windows/snap_layouts/mod.rs`, above the `pub use logic::*;`:

```rust
use std::sync::Mutex;

mod logic;

pub use logic::*;

/// The state the window subclass reads while answering the hit test.
///
/// Shared between the warpui window, which publishes the button's bounds as the app paints it, and
/// the subclass, which runs inside the window procedure.
#[derive(Debug, Default)]
pub struct SnapLayoutsState {
    button: Mutex<Option<ButtonRect>>,
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
}
```

- [ ] **Step 2: Add the trait method**

In `crates/warpui_core/src/platform/mod.rs`, in `pub trait Window`, directly below `fn set_titlebar_height(&self, height: f64);`:

```rust
    /// Reports where the app-drawn maximize button was painted, in logical pixels relative to the
    /// window's client area, or `None` when no maximize button is on screen.
    ///
    /// Windows answers its non-client hit test from this, which is what opens the Snap Layouts
    /// flyout over the button. Every other platform ignores it.
    fn set_maximize_button_bounds(&self, bounds: Option<RectF>);
```

- [ ] **Step 3: Compile to find every implementation that needs it**

Run: `cargo check -p warpui_core -p warpui`
Expected: FAIL, one `not all trait items implemented` error per `impl platform::Window` block. Those blocks are the ones listed in this task's **Files**.

- [ ] **Step 4: Add the no-op implementations**

In each of `crates/warpui_core/src/platform/test/delegate.rs`, `crates/warpui/src/platform/headless/windowing.rs`, and `crates/warpui/src/platform/mac/window.rs`, next to that file's `set_titlebar_height`:

```rust
    fn set_maximize_button_bounds(&self, _bounds: Option<RectF>) {
        // Snap Layouts is a Windows 11 feature; there is nothing to report here.
    }
```

Add `use pathfinder_geometry::rect::RectF;` to any of those files that does not already import it.

- [ ] **Step 5: Hold the state on the winit window**

In `crates/warpui/src/windowing/winit/window.rs`, add a field to `pub(super) struct Window`, below `titlebar_height`:

```rust
    /// Shared with the Snap Layouts window subclass on Windows 11. `None` when the subclass is not
    /// installed: on Windows 10, on a window created with native decorations, and on every other
    /// platform.
    #[cfg(windows)]
    snap_layouts: RefCell<Option<std::sync::Arc<super::windows::SnapLayoutsState>>>,
```

and initialize it in `Window::new`, below `titlebar_height: Cell::new(DEFAULT_TITLEBAR_HEIGHT),`:

```rust
            #[cfg(windows)]
            snap_layouts: RefCell::new(None),
```

- [ ] **Step 6: Implement the trait method on the winit window**

In the same file, in the `impl platform::Window for Window` block, below `fn set_titlebar_height`:

```rust
    fn set_maximize_button_bounds(&self, bounds: Option<RectF>) {
        #[cfg(windows)]
        {
            let state = self.snap_layouts.borrow().as_ref().cloned();
            let Some(state) = state else {
                return;
            };

            // The subclass compares against physical client coordinates, so convert here rather
            // than in the window procedure. A scale-factor change corrects itself on the next
            // paint, which is when this is called.
            let scale_factor = self.backing_scale_factor();
            state.set_button_rect(bounds.map(|bounds| super::windows::ButtonRect {
                left: (bounds.min_x() * scale_factor).round() as i32,
                top: (bounds.min_y() * scale_factor).round() as i32,
                right: (bounds.max_x() * scale_factor).round() as i32,
                bottom: (bounds.max_y() * scale_factor).round() as i32,
            }));
        }

        #[cfg(not(windows))]
        let _ = bounds;
    }
```

- [ ] **Step 7: Verify it compiles on this machine and for a non-Windows target**

Run: `cargo check -p warpui_core -p warpui`
Expected: PASS.

Run: `cargo clippy -p warpui --all-targets -- -D warnings`
Expected: no diagnostics.

- [ ] **Step 8: Commit**

```bash
git add crates/warpui_core/src/platform crates/warpui/src/platform crates/warpui/src/windowing/winit/window.rs crates/warpui/src/windowing/winit/windows/snap_layouts/mod.rs
git commit -m "feat(warpui): report the maximize button's bounds to the platform window"
```

---

## Task 3: Install the window subclass

**Files:**
- Modify: `crates/warpui/src/windowing/winit/windows/snap_layouts/mod.rs`
- Modify: `crates/warpui/src/windowing/winit/windows/window_attribute.rs:65`
- Modify: `crates/warpui/Cargo.toml:211-227`
- Modify: `crates/warpui/src/windowing/winit/window.rs` (in `Window::open_window`)

**Interfaces:**
- Consumes: `hit_test`, `synthetic_message`, `leave_position`, `pack_coords`, `ButtonRect` (Task 1); `SnapLayoutsState`, the `snap_layouts` field on the winit `Window` (Task 2).
- Produces: `pub fn install(window: &winit::window::Window) -> Option<std::sync::Arc<SnapLayoutsState>>`

- [ ] **Step 1: Add the `windows` crate feature that `TrackMouseEvent` needs**

In `crates/warpui/Cargo.toml`, in the `[target.'cfg(target_os = "windows")'.dependencies]` `windows` feature list, add in alphabetical position:

```toml
    "Win32_UI_Input_KeyboardAndMouse",
```

`SetWindowSubclass` comes from the already-enabled `Win32_UI_Shell`, the hit-test and message constants from the already-enabled `Win32_UI_WindowsAndMessaging`, and `ScreenToClient` from the already-enabled `Win32_Graphics_Gdi`.

- [ ] **Step 2: Let the new module reach `to_hwnd`**

In `crates/warpui/src/windowing/winit/windows/window_attribute.rs:65`, change:

```rust
fn to_hwnd(window: &WinitWindow) -> Result<HWND, rwh::HandleError> {
```

to:

```rust
pub(super) fn to_hwnd(window: &WinitWindow) -> Result<HWND, rwh::HandleError> {
```

- [ ] **Step 3: Write the subclass**

Append to `crates/warpui/src/windowing/winit/windows/snap_layouts/mod.rs`:

```rust
use std::sync::Arc;
use std::sync::LazyLock;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    TME_LEAVE, TME_NONCLIENT, TRACKMOUSEEVENT, TrackMouseEvent,
};
use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, PostMessageW, WM_NCDESTROY, WM_NCHITTEST, WM_NCLBUTTONDOWN, WM_NCLBUTTONUP,
    WM_NCMOUSELEAVE, WM_NCMOUSEMOVE,
};
use winit::window::Window as WinitWindow;

use super::window_attribute::to_hwnd;

/// Distinguishes our subclass from any other on the same window.
const SUBCLASS_ID: usize = 0x5741_5250;

/// The first Windows 11 build. Snap Layouts does not exist before it.
const WINDOWS_11_BUILD: u32 = 22000;

static WINDOWS_VERSION: LazyLock<windows_version::OsVersion> =
    LazyLock::new(windows_version::OsVersion::current);

/// Installs the Snap Layouts subclass on `window`, returning the state to publish the maximize
/// button's bounds through.
///
/// Returns `None`, leaving the window untouched, on anything before Windows 11 and on a window that
/// has the native frame, which draws its own caption buttons.
pub fn install(window: &WinitWindow) -> Option<Arc<SnapLayoutsState>> {
    if WINDOWS_VERSION.build < WINDOWS_11_BUILD {
        log::info!("Snap Layouts is not supported before Windows 11");
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

            LRESULT(hit_test(default_answer.0 as u32, client_x, client_y, button) as isize)
        }
        WM_NCMOUSEMOVE | WM_NCMOUSELEAVE | WM_NCLBUTTONDOWN | WM_NCLBUTTONUP => {
            let Some(synthetic) = synthetic_message(message, wparam.0 as u32) else {
                return unsafe { DefSubclassProc(hwnd, message, wparam, lparam) };
            };

            let position = if message == WM_NCMOUSELEAVE {
                // WM_NCMOUSELEAVE carries no position of its own.
                cursor_client_position(hwnd).map(|cursor| leave_position(cursor, state.button_rect()))
            } else {
                let (screen_x, screen_y) = unpack_coords(lparam);
                screen_to_client(hwnd, screen_x, screen_y)
            };

            let Some((x, y)) = position else {
                return unsafe { DefSubclassProc(hwnd, message, wparam, lparam) };
            };

            if message == WM_NCMOUSEMOVE {
                track_mouse_leave(hwnd);
            }

            unsafe {
                let _ = PostMessageW(
                    Some(hwnd),
                    synthetic.message,
                    WPARAM(synthetic.wparam),
                    LPARAM(pack_coords(x, y)),
                );
            }

            // Swallow the original. Left alone, `DefWindowProc` would open the system menu on a
            // press over a caption button.
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

/// Splits a mouse message's `LPARAM` into its signed 16-bit coordinates.
fn unpack_coords(lparam: LPARAM) -> (i32, i32) {
    let packed = lparam.0 as u32;
    (
        (packed & 0xFFFF) as u16 as i16 as i32,
        (packed >> 16) as u16 as i16 as i32,
    )
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

/// Asks for a `WM_NCMOUSELEAVE` when the pointer leaves the non-client area, which is the only
/// notice we get that the hover over the maximize button has ended.
fn track_mouse_leave(hwnd: HWND) {
    let mut event = TRACKMOUSEEVENT {
        cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
        dwFlags: TME_LEAVE | TME_NONCLIENT,
        hwndTrack: hwnd,
        dwHoverTime: 0,
    };

    if let Err(err) = unsafe { TrackMouseEvent(&mut event) } {
        log::warn!("Couldn't track the pointer leaving the maximize button: {err:?}");
    }
}
```

- [ ] **Step 4: Install it when the window opens**

In `crates/warpui/src/windowing/winit/window.rs`, in `Window::open_window`, immediately after `let window = Arc::new(window);`:

```rust
        #[cfg(windows)]
        {
            *self.snap_layouts.borrow_mut() = super::windows::install(&window);
        }
```

- [ ] **Step 5: Verify it compiles and lints**

Run: `cargo check -p warpui && cargo clippy -p warpui --all-targets -- -D warnings`
Expected: PASS with no diagnostics.

Run: `cargo test -p warpui --lib snap_layouts`
Expected: PASS, the 10 tests from Task 1 still green.

- [ ] **Step 6: Commit**

```bash
git add crates/warpui/Cargo.toml crates/warpui/src/windowing/winit
git commit -m "feat(warpui): answer the maximize button's non-client hit test on Windows 11"
```

---

## Task 4: Publish the button's bounds from the app

**Files:**
- Create: `app/src/util/traffic_lights/windows/maximize_button_region.rs`
- Modify: `app/src/util/traffic_lights/windows/mod.rs`
- Modify: `app/src/util/traffic_lights/windows/renderer.rs:109-155` and `:186-250`
- Modify: `app/src/util/traffic_lights.rs:63-95` and `:126-148`
- Modify: `app/src/workspace/view.rs:22303-22307`

**Interfaces:**
- Consumes: `platform::Window::set_maximize_button_bounds` (Task 2).
- Produces:
  - `pub struct MaximizeButtonRegion` with `pub fn new(child: Box<dyn Element>, window_id: WindowId, reports_bounds: bool) -> Self`
  - `pub fn report_maximize_button_bounds(window_id: WindowId, bounds: Option<RectF>, app: &AppContext)`
  - `TrafficLightData::window_id: WindowId` (private field, read inside `traffic_lights`)

- [ ] **Step 1: Write the wrapper element**

Create `app/src/util/traffic_lights/windows/maximize_button_region.rs`:

```rust
//! Reports where the app-drawn maximize button was painted.
//!
//! Windows answers its non-client hit test from those bounds, which is what opens the Snap Layouts
//! flyout over the button. The bounds have to come from paint: the button is right-aligned, so it
//! moves on every window resize, and a resize re-lays-out the existing element tree without
//! re-running the workspace's `render`.

use warpui::elements::Point;
use warpui::event::DispatchedEvent;
use warpui::geometry::rect::RectF;
use warpui::geometry::vector::Vector2F;
use warpui::{
    AfterLayoutContext, AppContext, Element, EventContext, LayoutContext, PaintContext,
    SizeConstraint, WindowId,
};

/// Wraps the maximize button and reports its painted bounds to the platform window.
pub struct MaximizeButtonRegion {
    child: Box<dyn Element>,
    window_id: WindowId,
    /// `false` in fullscreen, where the button leaves fullscreen instead of maximizing and must not
    /// open the flyout.
    reports_bounds: bool,
    size: Option<Vector2F>,
    origin: Option<Point>,
}

impl MaximizeButtonRegion {
    pub fn new(child: Box<dyn Element>, window_id: WindowId, reports_bounds: bool) -> Self {
        Self {
            child,
            window_id,
            reports_bounds,
            size: None,
            origin: None,
        }
    }
}

impl Element for MaximizeButtonRegion {
    fn layout(
        &mut self,
        constraint: SizeConstraint,
        ctx: &mut LayoutContext,
        app: &AppContext,
    ) -> Vector2F {
        let size = self.child.layout(constraint, ctx, app);
        self.size = Some(size);
        size
    }

    fn after_layout(&mut self, ctx: &mut AfterLayoutContext, app: &AppContext) {
        self.child.after_layout(ctx, app);
    }

    fn paint(&mut self, origin: Vector2F, ctx: &mut PaintContext, app: &AppContext) {
        self.origin = Some(Point::from_vec2f(origin, ctx.scene.z_index()));
        self.child.paint(origin, ctx, app);

        let bounds = if self.reports_bounds {
            self.bounds()
        } else {
            None
        };
        report_maximize_button_bounds(self.window_id, bounds, app);
    }

    fn dispatch_event(
        &mut self,
        event: &DispatchedEvent,
        ctx: &mut EventContext,
        app: &AppContext,
    ) -> bool {
        self.child.dispatch_event(event, ctx, app)
    }

    fn size(&self) -> Option<Vector2F> {
        self.size
    }

    fn origin(&self) -> Option<Point> {
        self.origin
    }
}

/// Publishes the maximize button's bounds, or `None` when the window has no maximize button on
/// screen.
pub fn report_maximize_button_bounds(
    window_id: WindowId,
    bounds: Option<RectF>,
    app: &AppContext,
) {
    if let Some(window) = app.windows().platform_window(window_id) {
        window.as_ref().set_maximize_button_bounds(bounds);
    }
}
```

Declare and re-export it in `app/src/util/traffic_lights/windows/mod.rs`, keeping both lists alphabetical:

```rust
mod maximize_button_region;

pub use maximize_button_region::{MaximizeButtonRegion, report_maximize_button_bounds};
```

- [ ] **Step 2: Carry the window id on `TrafficLightData`**

In `app/src/util/traffic_lights.rs`, add the field to the struct:

```rust
pub struct TrafficLightData {
    width: f32,
    pub side: TrafficLightSide,
    /// Whether the traffic light buttons can scale with the app's zoom level.
    ///
    /// If `false` that means the traffic light buttons are of fixed size as determined by the OS
    /// and we cannot scale them as the user configures the zoom level.
    scales_with_zoom: bool,
    /// The window these buttons belong to. Windows reports the maximize button's bounds to its
    /// platform window so it can open the Snap Layouts flyout over it.
    window_id: WindowId,
}
```

and add `window_id,` to each of the three `Some(TrafficLightData { .. })` literals in `traffic_light_data`.

- [ ] **Step 3: Wrap the maximize button in the glyph path**

In `app/src/util/traffic_lights/windows/renderer.rs`, add to the imports:

```rust
use crate::util::traffic_lights::windows::MaximizeButtonRegion;
```

Change `render_tab_row_with_glyph_icons` to take the window id, and wrap the middle button. The signature becomes:

```rust
fn render_tab_row_with_glyph_icons(
    fullscreen_state: FullscreenState,
    mouse_states: &TrafficLightMouseStates,
    theme: &WarpTheme,
    icon_font_family: FamilyId,
    window_id: WindowId,
) -> Box<dyn Element> {
```

and the middle child of the `Flex::row()` becomes:

```rust
            {
                let icon = if fullscreen_state == FullscreenState::Normal {
                    WindowsTrafficLightIcon::Maximize
                } else {
                    WindowsTrafficLightIcon::Restore
                };
                let action_name = if fullscreen_state == FullscreenState::Fullscreen {
                    "root_view:toggle_fullscreen"
                } else {
                    "root_view:toggle_maximize_window"
                };
                MaximizeButtonRegion::new(
                    icon.render(
                        mouse_states.maximize_window_button.clone(),
                        theme,
                        icon_font_family,
                        action_name,
                    ),
                    window_id,
                    fullscreen_state != FullscreenState::Fullscreen,
                )
                .finish()
            },
```

Add `use warpui::WindowId;` to the file's imports.

- [ ] **Step 4: Wrap the maximize button in the SVG fallback path**

In the same file, in `render_tab_row_with_svg_icons`, replace the middle child — the `Self::render_button(Arc::clone(&mouse_states.maximize_window_button), ..)` expression — with:

```rust
                        MaximizeButtonRegion::new(
                            Self::render_button(
                                Arc::clone(&mouse_states.maximize_window_button),
                                Self::render_windows_maximize_button_icon(
                                    fg_color,
                                    fullscreen_state,
                                ),
                                theme.surface_3().into(),
                            )
                            .on_click(move |evt, _, _| {
                                if fullscreen_state == FullscreenState::Fullscreen {
                                    evt.dispatch_action("root_view:toggle_fullscreen", ());
                                } else {
                                    evt.dispatch_action("root_view:toggle_maximize_window", ());
                                }
                            })
                            .finish(),
                            self.window_id,
                            fullscreen_state != FullscreenState::Fullscreen,
                        )
                        .finish(),
```

Pass `self.window_id` into `render_tab_row_with_glyph_icons` from `render_tab_row`.

- [ ] **Step 5: Clear the bounds when the traffic lights are not rendered**

In `app/src/workspace/view.rs`, change the early return at the top of `maybe_render_traffic_lights`:

```rust
        let Some(traffic_light_data) = traffic_light_data(app, self.window_id) else {
            // No traffic lights means no maximize button, so Windows must stop claiming that
            // region. This covers the native-frame case, where `traffic_light_data` returns `None`.
            #[cfg(windows)]
            crate::util::traffic_lights::windows::report_maximize_button_bounds(
                self.window_id,
                None,
                app,
            );
            return;
        };
```

- [ ] **Step 6: Verify it compiles and lints**

Run: `cargo check -p warp && cargo clippy -p warp --all-targets -- -D warnings`
Expected: PASS with no diagnostics.

Run: `cargo fmt --all && git diff --stat`
Expected: no reformatting beyond the files this plan touches.

- [ ] **Step 7: Commit**

```bash
git add app/src/util/traffic_lights.rs app/src/util/traffic_lights/windows app/src/workspace/view.rs
git commit -m "feat(app): publish the maximize button's bounds for Snap Layouts"
```

---

## Task 5: Verify on Windows and record the change

**Files:**
- Modify: `CHANGELOG.md`

**Interfaces:**
- Consumes: everything from Tasks 1 to 4.
- Produces: nothing.

- [ ] **Step 1: Build and run the app**

Run: `cargo run -p warp --bin warp-oss`
Expected: the app starts with its usual custom title bar.

- [ ] **Step 2: Walk the verification list**

Confirm each of these by hand, on Windows 11:

1. Resting the pointer on the maximize button opens the Snap Layouts flyout, and picking a zone snaps the window into it.
2. The maximize button still highlights while hovered, and reacts to a press.
3. Clicking it maximizes, and clicking again restores.
4. Dragging the top edge and the top-right corner still resizes the window.
5. Dragging the tab bar still moves the window, and double-clicking it still maximizes and restores.
6. In fullscreen the button restores as before, and no flyout appears.
7. The minimize and close buttons are unchanged.

If any of 2 to 7 regressed, stop and fix it before continuing — those are the behaviors this change is required to preserve.

- [ ] **Step 3: Record the change**

In `CHANGELOG.md`, under `## [Unreleased]` → `### Added`, add:

```markdown
- Windows 11 Snap Layouts on the maximize button. Warp draws its own caption buttons on an
  undecorated window, so Windows never saw the `HTMAXBUTTON` hit-test answer that offers the
  flyout. A window subclass now supplies it and replays the non-client mouse messages that follow,
  leaving the button's hover, press, and click behavior unchanged. Windows 10 is untouched.
```

- [ ] **Step 4: Run the full check one more time**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test -p warpui --lib snap_layouts`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add CHANGELOG.md
git commit -m "docs(changelog): record Windows 11 Snap Layouts support"
```
