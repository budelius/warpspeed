# Session library

Open **Sessions: show library** from the Command Palette to browse local and AgentCtrl sessions in a full-window view.

- Filter by **All sessions**, **Current directory**, **Running & detached**, **AgentCtrl**, or **Past sessions**, and search within the selected filter.
- Select a row to inspect its preview. Use its action button to focus, attach, open, or resume the session where supported. Selecting a row does not start or take over a session.
- Drag the vertical divider between the session list and preview to widen the list. Its width is retained when changing filters or reopening the library in the same workspace, but is not saved across application restarts. Small windows clamp the width to leave room for the preview.
- Close the library with its close button or Escape to return to the workspace.

The scrollbar and resize handle have separate hit areas. Opening the library does not end processes; ending an externally held agent requires the existing explicit confirmation action.

## Verification

Unit tests cover filtering, search, ordering, session actions, synthetic mouse dragging across renders, and width clamping as the window shrinks. Workspace tests cover opening the library and routing session actions.

Real-display acceptance remains separate: the integration runner does not currently support hermetic Windows execution. Do not bypass that platform guard or use production sessions as test fixtures. On a supported test host, verify that dragging the list/preview divider reveals more of long session titles, scrolling remains usable, reopening retains the width, and the preview remains visible after shrinking the window.
