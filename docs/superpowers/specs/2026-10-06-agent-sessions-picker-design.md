# Agent sessions: see them per pane, and attach, start or resume with a click

**Status:** design
**Date:** 2026-10-06
**Builds on:** `2026-09-08-agent-session-attach-design.md` (its `agent_session_history` scanner and
the supervisor attach path both exist; the picker that joins them was never built).

## Problem

Recovering an agent conversation today means knowing its session ID and typing
`claude --resume <id>` or `pi --session <id>` in the right directory by hand. Warp already holds
every fact needed to do this for the user, but in four places nothing joins:

| Source | What it knows | Where |
|---|---|---|
| Live agents | the agent running in each pane, and its session ID | `CLIAgentSessionsModel` |
| Pending restores | the resume command a restored pane will run once its shell starts | each local `TerminalManager` |
| Detached shells | shells the supervisor holds with no pane showing them | `detached_sessions::list` |
| Past conversations | Claude, Codex and Pi conversations on disk, with folder and title | `agent_session_history` |

On 2026-10-06 a restored STOZN window showed the cost: twelve agents were restored as pending,
none of them started when their tabs were opened, and the only way back was a list of hand-typed
commands.

## Goals

- From any pane, see which agent conversations belong to it and act on each with one click.
- Search every agent conversation on the machine by title, folder, agent or ID.
- Never start a second process on a conversation that is already running.

## Non-goals

- Agents whose CLI cannot resume by ID (`CliAgentResumeDescriptor::resume_command` returns `None`).
- Remote and shared sessions.
- Changing how the supervisor holds or hands over shells.

## Part 0 — Pending restores must start

This comes first because the **Start** action depends on it.

### What happens today

Restoring a pane whose snapshot names an agent parks the pane's shell and records the agent's
resume command on its terminal manager (`pane_group/mod.rs`, `restore_cli_agent_resume_command`).
When the shell starts, `on_shell_determined` hands the command to the view
(`set_pending_restore_command` → `execute_command_or_set_pending`), which writes it into the input
editor as text and marks it pending (`input.rs`, `set_pending_command`). It is submitted on the
next `execute_pending_command`, triggered by `BootstrapPrecmdDone` or a completed block.

### What was observed

The STOZN restore logged `Restoring a pane that ran …: pi --session …` for every pane at
14:58:41, and the user opened the tabs later, starting their shells (`Shell is bootstrapped` at
15:59, 16:01 and 18:15). No agent ran: none of the twelve conversations' files changed after
the restore.

### Hypothesis to test

The pending command lives only as editor text plus a flag. Anything that replaces the input
buffer between the shell starting and bootstrap finishing — input-state restore, an input-mode
switch, a zero-state reset — drops the text while the flag survives, so the command that is
finally submitted is empty or absent.

### Required outcome

A failing test reproduces the loss through the real restore path (snapshot with a descriptor →
deferred shell → start → bootstrap), the root cause is fixed where the command is lost, and the
test then passes. If the hypothesis is wrong, the test and the log lines added while finding the
real cause determine the fix; the rest of this design does not depend on which.

## Part 1 — One index of agent sessions

A new module `app/src/terminal/agent_sessions_index.rs` builds rows from the four sources:

```rust
pub struct AgentSessionRow {
    pub agent: CLIAgent,
    pub session_id: Option<String>,
    pub title: Option<String>,
    pub cwd: PathBuf,
    pub last_used: Option<SystemTime>,
    pub state: AgentSessionState,
}

pub enum AgentSessionState {
    /// Running in a pane of this app.
    Running { locator: PaneViewLocator },
    /// Restored into a pane whose shell or resume command has not run yet.
    Pending { locator: PaneViewLocator },
    /// Held by the supervisor with no pane showing it.
    Detached { session_id: SessionId },
    /// On disk only.
    Past,
}
```

**One row per conversation.** Rows are keyed by `(agent, session_id)` and a conversation takes
its strongest state: `Running`, then `Detached`, then `Pending`, then `Past`. Rows without a
session ID (an agent detected from its command line alone) are kept, keyed by their pane.

**Where the facts come from:**

- `Running`: every pane's terminal view looked up in `CLIAgentSessionsModel`.
- `Pending`: panes whose manager still holds a restore command or a parked shell. This needs one
  new accessor on `TerminalManager`, `pending_cli_agent_resume`, returning the restored
  descriptor; `TerminalPane` already keeps it in `RestoredShellSnapshotFields`.
- `Detached`: `detached_sessions::list`. Its label carries no agent today. Where the handover
  metadata the supervisor stores records the agent and its session ID, they are added to
  `DetachedSession`; where it does not, the row shows the shell and folder only and its action is
  still Attach.
- `Past`: `agent_session_history::find_all` for Claude, Codex and Pi.

**Cost.** The disk scan is the only slow source. It runs on a background executor into a cached
`AgentSessionHistoryModel`, refreshed when either picker opens; the pickers render what the cache
holds and update when the refresh lands. The other three sources are in-memory or one supervisor
round trip and are read on open.

## Part 2 — Actions

`app/src/workspace/agent_session_actions.rs` turns a row into what happens on click. Both pickers
call it; nothing else decides.

| State | Action | Does |
|---|---|---|
| `Running` | **Focus** | activates the tab and focuses the pane |
| `Pending` | **Start** | starts the pane's shell (`ensure_shell_started`), which runs its resume command |
| `Detached` | **Attach** | `Workspace::attach_session_by_id`, the path `sessions attach` already uses |
| `Past` | **Resume** | runs `cd <folder> && <resume command>` |

**Where Resume runs.** In the target pane if its shell is at an idle prompt; otherwise in a new
split of that pane, created with the conversation's folder as its starting directory. The target
pane is the pane the picker was opened for, or the focused pane when opened from the palette.

**No duplicates.** Before Resume or Start, the conversation's session ID is checked against
running panes and the process registry's running sessions. If it is running anywhere the action
becomes Focus, or reports where it runs when that is outside this app.

**Shell syntax.** The `cd … &&` prefix is built for the pane's shell (Bash, PowerShell, Zsh,
Fish) through the same shell-type quoting the app uses for other injected commands.

## Part 3 — Palette: "Agent sessions"

A new `QueryFilter::AgentSessions` with a data source modelled on the conversations source
(`search/command_palette/conversations/`): sections **This pane**, **Running elsewhere**,
**Detached**, **Past**. Fuzzy-matched on title, folder, agent name and session ID. Each row shows
the agent icon, title (or ID), folder, last-used time and the state's action as its hint; Enter
performs that action.

Entry points: the filter chip in the palette, and a palette command "Agent sessions: search".

## Part 4 — Window view: agent sessions by tab and pane

A modal over the workspace, listing the window's tabs in order and, under each, its terminal
panes:

```
Agent sessions — STOZN                              [search…        ]
▾ WARP
   pane 1  t:\..\warp   Pi     01a1…            ● running   [Focus]
   pane 2  t:\..\warp   Claude c4b2…            ● running   [Focus]
   pane 5  t:\..\warp   Pi     01a0ec5a         ○ pending   [Start]
     past in this folder (7)                                ▸
▾ Agent Chat
   pane 1  ..\stoz3n-chat-agent  Claude 689b…   ○ pending   [Start]
   pane 2  ..\stoz3n-chat-agent  (shell)           —        [Resume…]
▾ Detached in supervisor (2)
   1258…  bash  t:\..\obachan   Claude  3d ago               [Attach]
```

- A pane row shows its folder, its agent and session if any, the state and the action.
- "past in this folder (N)" expands inline into Past rows for that folder and its subfolders
  (Claude worktrees live under `.claude\worktrees`).
- **Resume…** on a plain shell pane opens the palette in Agent sessions mode, filtered to that
  pane's folder, with the pane as the target.
- The search box filters rows by the same fields as the palette.
- The view lives in `app/src/workspace/agent_sessions_view.rs`, built from Warp's own elements
  and using the actions in Part 2.

Entry points: the pane menu item "Agent sessions…", which opens the view scrolled to that pane,
and a palette command "Agent sessions: show window".

## Errors

- A failed action shows an error toast naming the session and the reason (supervisor
  unreachable, conversation file gone, pane closed meanwhile); the picker stays open.
- A supervisor that cannot be reached leaves the Detached section with one row saying so; the
  other sections still render.
- A scan error is logged and the Past section shows what the cache already held.

## Testing

- **Part 0:** a regression test through the real restore path, failing before the fix.
- **Index:** merging and state precedence — the same conversation from several sources yields one
  row in its strongest state; rows without IDs are kept per pane; folder matching includes
  subfolders.
- **Actions:** Resume chooses this pane when idle and a split when busy; a running session turns
  Resume and Start into Focus; the `cd` prefix per shell type.
- **Palette source:** sections and fuzzy matching on each field.
- **Window view:** unit tests of the row model the view draws — which panes appear under which
  tab, and which state and action each gets. The GUI has no render-to-text harness.
- Verified in the running app by the user; the GUI cannot be observed from this environment.
