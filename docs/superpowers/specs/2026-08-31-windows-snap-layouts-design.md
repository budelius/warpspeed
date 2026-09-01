# Windows 11 Snap Layouts for the custom maximize button

**Status:** design
**Date:** 2026-08-31
**Scope:** make the Snap Layouts flyout appear when the pointer rests on Warp's own maximize button on Windows 11, without changing how that button looks or behaves.

## Problem

Every Warp window is undecorated. `AppContext` sets `hide_title_bar: true` for all windows
(`crates/warpui_core/src/core/app.rs:2507`), and `create_window` turns that into
`with_decorations(false)` (`crates/warpui/src/windowing/winit/window.rs:1316`). Warp therefore draws its own
Windows caption buttons — minimize, maximize/restore, close, 46 logical pixels each — in the tab row
(`app/src/util/traffic_lights/windows/renderer.rs`).

Windows 11 shows the Snap Layouts flyout only for a window that answers `WM_NCHITTEST` with `HTMAXBUTTON`.
Nothing in the tree handles `WM_NCHITTEST`, so every point inside the client area resolves to `HTCLIENT` and
the flyout never appears. Applications with native title bars, and applications with custom title bars that do
answer `HTMAXBUTTON`, both get it. Warp does not.

The window styles already allow it: winit strips `WS_CAPTION` and `WS_BORDER` from an undecorated window but
keeps `WS_MAXIMIZEBOX` (`window_state.rs:258-287` in the pinned `warpdotdev/winit` fork), which is what
Windows requires before it will offer the flyout.

## Goals

- Hovering Warp's maximize button on Windows 11 opens the Snap Layouts flyout.
- The button keeps its current hover highlight, press feedback, Maximize/Restore icon swap, and click
  behavior, including the fullscreen case that dispatches `root_view:toggle_fullscreen` instead of
  `root_view:toggle_maximize_window`.
- Dragging the top and top-right window edges still resizes the window.
- Windows 10 behavior is unchanged.

## Non-goals

- `HTMINBUTTON` and `HTCLOSE` for the minimize and close buttons. Snap Layouts needs neither, and the two
  buttons work today.
- Snap Layouts on Linux or macOS, which have no such feature.
- Changing the appearance, size, or position of the caption buttons.
- Adopting the system caption button bounds that `get_system_caption_button_bounds`
  (`crates/warpui/src/windowing/winit/windows/system_caption_buttons.rs`) reads and its caller currently
  discards.

## Design

### 1. A window subclass answers the hit test

A Windows-only subclass installed on each window's `HWND` through `SetWindowSubclass` handles the messages
and forwards everything else. It lives beside the other Win32 helpers, in
`crates/warpui/src/windowing/winit/windows/`, and is installed from the same
`#[cfg(windows)]` block in `create_window` that sets the rounded-corner attribute
(`crates/warpui/src/windowing/winit/window.rs:1494-1526`). Placing it there rather than in the winit fork
keeps the change in this repository and avoids a pinned-revision bump.

On `WM_NCHITTEST` the subclass calls `DefSubclassProc` first and overrides the answer only when two
conditions hold: the original answer is `HTCLIENT`, and the point lies inside the registered maximize-button
rectangle. Then it answers `HTMAXBUTTON`.

Consulting the original answer first is what preserves the resize borders. The maximize button sits against
the top edge of the window, where the first few pixels resolve to `HTTOP` or `HTTOPRIGHT`; because those
answers are not `HTCLIENT`, they pass through untouched and the edge still resizes.

The subclass is installed only on Windows 11, guarded by the `windows-version` build check already used to
decide whether rounded corners are available (build 22000 and later). On Windows 10 no subclass exists and
no message path changes.

### 2. The button's rectangle reaches the platform layer

A new platform-window method carries the rectangle down:

```rust
fn set_maximize_button_bounds(&self, bounds: Option<RectF>);
```

It matches `set_titlebar_height` in shape and is a no-op on macOS, Linux, WASM, headless, and the test
delegate.

The rectangle must come from paint, not from a view method. `update_titlebar_height`
(`app/src/workspace/view.rs:15240-15249`) is called only at workspace construction and on settings changes,
whereas the button is right-aligned and therefore moves on every window resize — and a resize re-lays-out the
existing element tree without re-running `Workspace::render`. Anything pushed from a render-time call site
would go stale the moment the window is resized.

A small Windows-only wrapper element, `MaximizeButtonRegion`, therefore publishes the rectangle itself. It
wraps the maximize button in `traffic_lights/windows/renderer.rs`, and on each paint reports the bounds it
was just laid out at to the platform window for its window id. Reporting the laid-out rectangle rather than
recomputing it analytically also keeps the two rendering paths in `renderer.rs`, the Segoe Fluent glyph path
and the SVG fallback, from drifting away from the hit test.

The winit window converts to physical pixels with its current scale factor as the rectangle arrives, so the
hit test itself needs no coordinate conversion, and a DPI change corrects itself on the next paint.

The rectangle is `None`, and the flyout consequently absent, whenever the button is not a maximize target:

- the window is fullscreen, where the button dispatches `root_view:toggle_fullscreen`. The wrapper element
  reports `None` in that state;
- the traffic lights are not rendered at all, where `maybe_render_traffic_lights`
  (`app/src/workspace/view.rs:22303`) returns early. It reports `None` on that path;
- the window is decorated. The subclass is not installed on a window created with native decorations, and
  when `enable_decorations_if_needed` (`crates/warpui/src/windowing/winit/window.rs:667`) turns them on later
  to work around the GPU rendering-offset bug, `traffic_light_data` returns `None` and no rectangle is ever
  published.

### 3. Non-client messages become ordinary mouse messages

Once the hit test answers `HTMAXBUTTON`, Windows stops delivering client-area mouse messages for that
rectangle and sends non-client ones instead. Left alone, the hover highlight would simply stop working.

The subclass translates each message into its client-area equivalent, posts it to the same window, and
returns 0 to swallow the original:

| Received | Posted |
| --- | --- |
| `WM_NCMOUSEMOVE`, wParam `HTMAXBUTTON` | `WM_MOUSEMOVE` at the client coordinate |
| `WM_NCMOUSEMOVE`, any other wParam, while the button is hovered | `WM_MOUSEMOVE` at the client coordinate, or one pixel below the rectangle if that coordinate is still inside it |
| `WM_NCMOUSELEAVE` | `WM_MOUSEMOVE` at the current cursor position, or one pixel below the rectangle if that position is still inside it |
| `WM_NCLBUTTONDOWN`, wParam `HTMAXBUTTON` | `WM_LBUTTONDOWN` |
| `WM_NCLBUTTONDBLCLK`, wParam `HTMAXBUTTON` | `WM_LBUTTONDOWN` |
| `WM_NCLBUTTONUP`, wParam `HTMAXBUTTON` | `WM_LBUTTONUP` |

Every row but the second is swallowed. The second is not: the pointer is over the resize border or some
other part of the non-client area, and that area has to keep behaving exactly as it did before, so the
message goes on to `DefSubclassProc` and only the synthetic move is added.

`WM_NCLBUTTONDBLCLK` has to be translated for the same reason `WM_NCLBUTTONDOWN` does, and more urgently.
Left to `DefWindowProc` with an `HTMAXBUTTON` hit code it enters a caption-button tracking loop — a nested
modal message loop this app has never run — and posts `WM_SYSCOMMAND SC_MAXIMIZE` itself, while the
app-drawn button never sees the press. Replaying it as another `WM_LBUTTONDOWN` reproduces exactly what the
app saw before Windows started routing these through the non-client area: a fast double-click was two plain
presses and two clean toggles.

### Ending the hover

On every `WM_NCMOUSEMOVE` the subclass arms `TrackMouseEvent` with `TME_NONCLIENT | TME_LEAVE`, but that
request cannot be relied on, because winit tracks the pointer on the same `HWND`. winit's `WM_MOUSEMOVE`
handler classifies pointer position purely geometrically, by `GetClientRect` containment, and the maximize
button is geometrically inside the client rectangle. So it reads the synthetic move as an enter and calls
`TrackMouseEvent(TME_LEAVE)`, client-only, replacing our request — and because the pointer is in the
non-client area as far as the hit test is concerned, Windows satisfies that client request at once with
`WM_MOUSELEAVE`, leaving nothing armed.

While the pointer keeps moving over the button our request is re-armed on each `WM_NCMOUSEMOVE`, so it
mostly holds. It fails precisely when the pointer comes to rest — the gesture Snap Layouts exists for. The
last arming is winit's, it is consumed immediately, and a later departure through the non-client area
produces no `WM_NCMOUSELEAVE` at all; the hover highlight would stay lit until the pointer re-entered the
client area.

The subclass therefore does not depend on `WM_NCMOUSELEAVE`. It records in its shared state whether it
currently believes the button is hovered — set on a `WM_NCMOUSEMOVE` with `HTMAXBUTTON`, cleared on the
first non-client move whose hit code is anything else, and cleared by `WM_NCMOUSELEAVE` when that does
arrive. The recorded hover is what distinguishes a departure from the button from ordinary non-client
activity that never involved it, so no synthetic move is posted for the latter.

winit converts the posted messages into `WindowEvent::CursorMoved` and `WindowEvent::MouseInput`, which the
event loop already turns into `MouseMoved`, `LeftMouseDown`, and `LeftMouseUp`
(`crates/warpui/src/windowing/winit/event_loop/mod.rs:1167-1210`). Nothing in
`traffic_lights/windows/renderer.rs` changes: hover, press, the icon swap, and the click all keep running on
the shared `Hoverable` path.

Two consequences are worth stating:

- Swallowing `WM_NCLBUTTONDOWN` keeps `DefWindowProc` from opening the system menu.
- The synthesized press is handled by the button, exactly as a real press is today, so the titlebar
  drag branch at `event_loop/mod.rs:1643` does not claim it and turn the click into a window drag.

The flyout itself is unaffected by the swallowing. The compositor opens it in response to the hit-test
answer, not to `DefWindowProc`.

`TrackMouseEvent` lives in `Win32_UI_Input_KeyboardAndMouse`, which `crates/warpui/Cargo.toml` will need to
add. `SetWindowSubclass` is in `Win32_UI_Shell` and `HTMAXBUTTON` in `Win32_UI_WindowsAndMessaging`, both
already enabled.

## Testing

Message handling cannot run in CI, so the decisions are factored into pure functions that can be tested
directly:

- the hit-test decision, `(original answer, client point, Option<rect>) -> answer`, covering the override,
  the `HTTOP` pass-through, a point outside the rectangle, and a `None` rectangle;
- the non-client to client message mapping, covering each row of the table above and the messages that pass
  through untouched;
- the logical-to-physical conversion of the button's bounds, `(RectF, scale factor) -> rect`, at each of the
  common Windows scale factors. This is the one piece of arithmetic outside the decision module, and an
  off-by-scale in it would put the flyout slightly off the button — the hardest failure to diagnose from a
  screenshot;
- the shared state's rectangle round trip, which is the seam between the two halves of the feature.

All are unit tested under `#[cfg(windows)]` in the new module.

Manual verification on Windows 11:

1. Hovering the maximize button opens the Snap Layouts flyout, and choosing a zone snaps the window.
2. The button still highlights on hover and reacts to a press.
3. Clicking it maximizes and restores.
4. Double-clicking it maximizes and restores, with the same press feedback a single click gives, and
   without the window jumping ahead of the button's own handling.
5. Letting the pointer rest on the button and then leaving through the window's top edge ends the hover
   highlight.
6. Dragging the top and top-right edges still resizes.
7. Dragging the tab bar still moves the window; double-clicking it still maximizes.
8. In fullscreen, and in a window forced to native decorations, no flyout appears.

Manual verification on Windows 10: the caption buttons behave exactly as before and no flyout appears.

A CHANGELOG.md entry accompanies the change.

## Risks

- **Reentrancy.** The subclass posts messages rather than dispatching events directly, so warpui's event
  loop processes them on its own terms.
- **Mouse capture.** winit captures the mouse on `WM_LBUTTONDOWN`. While captured, the release arrives as a
  client `WM_LBUTTONUP` and no `WM_NCLBUTTONUP` follows, so a press cannot produce two release events.
- **Subclass lifetime.** The subclass must be removed with `RemoveWindowSubclass` before the window is
  destroyed, and its reference data freed exactly once.
