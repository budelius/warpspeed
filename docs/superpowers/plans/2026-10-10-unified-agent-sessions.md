# Unified Agent Sessions Implementation Plan

**Goal:** Make the pane-level Sessions window a single place to inspect a local or AgentCtrl conversation and then focus, resume, attach, or resume-and-attach it safely.

**Architecture:** Extend the existing `AgentSessionsView` into a master/detail browser. The list keeps local sessions grouped by tab and pane and appends live AgentCtrl runs from the shared fleet model. Selecting a conversation renders a bounded, sanitized preview; its explicit action button performs the state-appropriate operation. Local previews are extracted during the existing background history scan. AgentCtrl previews come from a bounded public managed-session event API. Workspace code remains responsible for choosing the target pane and never overwrites a busy pane.

## Product behavior

- Clicking a row selects it and shows recent user/agent content without starting anything.
- The row's action button remains one click:
  - local running: **Focus**
  - local pending: **Start**
  - local detached: **Attach**
  - local past: **Resume here**
  - AgentCtrl running: **Attach here**
  - AgentCtrl paused: **Resume & attach**
  - AgentCtrl finished: **View transcript**
- The pane that opened the browser is the target. An idle empty shell is reused; a busy pane gets a new split beside it.
- Detaching from AgentCtrl never stops the managed run.
- Loading and action failures stay visible instead of closing silently.

## Task 1: Local transcript previews

- Add bounded preview messages to `PastAgentSession` and `AgentSessionRow`.
- Parse recent user/assistant text from Claude, Codex, and Pi JSONL files during the background scan.
- Read only a bounded tail, sanitize control characters, omit tool payloads, and cap message and total sizes.
- Add parser, truncation, and row-merging unit tests.

## Task 2: AgentCtrl preview API

- Add a public, read-only preview API to `managed_agent_sessions` over normalized run events.
- Bound pages and returned events, deduplicate replayed events, report history gaps/truncation, and refresh desktop credentials between requests.
- Add focused tests for event selection, sanitization boundaries, and pagination stopping conditions.

## Task 3: Unified master/detail view

- Add AgentCtrl rows from `FleetModel` to `AgentSessionsView`, with one-shot refresh/polling only while the modal is open.
- Split the modal into a searchable list and transcript preview panel.
- Separate row selection from action-button activation; preserve keyboard Up/Down/Enter behavior.
- Load AgentCtrl previews asynchronously and cache them for the open modal.
- Add pure row/action tests and `warpui::App::test` coverage for selection and preview state.

## Task 4: Safe AgentCtrl attach and resume

- Add workspace handling for attach and resume-and-attach events targeted at the pane that opened the browser.
- Reuse an idle empty shell; otherwise split beside the target. Fall back to a new tab only when the target disappeared.
- Execute the exact current WarpSpeed binary with `agent-ctrl open <job-id> --environment staging`.
- Resume paused runs before opening and show any failure in the modal/toast.
- Add workspace tests for idle reuse, busy-pane splitting, paused resume, and missing-target fallback.

## Task 5: Verification and integration

- Run focused unit/workspace tests, `./script/format`, strict targeted Clippy, and `git diff --check`.
- Verify the GUI manually without replacing or terminating an existing WarpSpeed process.
- Rebase onto current `gitea/dev`, push the feature branch to Gitea and GitLab, merge with `--no-ff` into `dev`, and push `dev` to both remotes.
- Do not install or restart WarpSpeed unless separately requested.
