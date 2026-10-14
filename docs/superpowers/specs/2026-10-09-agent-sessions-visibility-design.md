# Agent sessions visibility: name every running agent, wherever its terminal went

**Status:** implemented
**Date:** 2026-10-09
**Branch:** `feat/agent-sessions-visibility`, off `dev`
**Scope:** The Agent Sessions modal (`workspace:show_agent_sessions`), its command-palette data
source, the row model in `agent_sessions_index`, the process registry, the terminal supervisor's
session summary (protocol v7), and the `warpspeed sessions` CLI. Pane restore and attach behavior
are unchanged.

## Problem

On 2026-10-09 a Pi agent looped closing and relaunching every WarpSpeed window. Each relaunch
resumed every agent again, and by 01:30 the machine ran 34 agent processes: five Claude sessions
and five Pi sessions twice each, several more in shells whose window had been closed, and two
copies of the looping Pi itself. Working out who was where took an hour of process-table walks,
registry reads and supervisor-log grepping from a shell. The Agent Sessions modal, which exists
for exactly this, would have shown none of it:

- A detached supervisor session is matched to a conversation only by pane title and directory
  (`agent_sessions_index::is_detached_conversation`). Titles are empty for every session the
  supervisor held that night, so every detached row said "shell", with no agent, no session id
  and no title.
- The modal lists this window's panes and this build's supervisor. Agents held by another
  build's supervisor, which the pipe name makes unreachable
  (`pipe_transport::supervisor_pipe_name_for` hashes the executable path), and agents whose
  shell is gone do not appear anywhere.
- Nothing flags a session that two processes are running at once, which was the actual damage.
- A Pi process cannot be found by walking its pane's process tree: the `pi` shim execs Node and
  Cygwin leaves Node with a dead parent pid. Treating the broken chain as "no terminal" is wrong;
  it cost four live Pi processes that night.

## Goals

- Every detached row names its agent, its session, what it was last asked to do, and how long
  it has been detached and since when it was last active.
- Every agent process on this machine appears in the modal once, in a section that says where
  it is: this window, this build's supervisor, another build's supervisor, or no known pane.
- A session with more than one live process is marked.
- Agents held by another build or with no known pane can be ended by exact PID from the modal.
- The same facts are available from the CLI for scripts and future TUI use.

## Non-goals

- Attaching to a session held by another build's supervisor. The pipe is per build; that stays.
- Agents without a process registry (Codex, Gemini, Amp, Droid, OpenCode, Copilot, Kimi, DSH).
  They keep today's pane and past rows. Adding a registry for one is its own change.
- A standalone window or tab. The modal and the palette remain the surface.
- Parsing the supervisor log. The supervisor reports what it knows on the wire instead.

## Design

### 1. The supervisor reports the shell pid and the pane uuid

`SessionSummary` (`crates/warp_terminal/src/local_tty/supervisor/message.rs`) gains two fields:

```rust
/// The pid of the shell the supervisor spawned for this session.
pub shell_pid: u32,
/// `WARP_TERMINAL_SESSION_UUID` as hex, from the spawn environment; empty when it was absent.
pub session_uuid_hex: String,
```

The spawn handler in `serve.rs` already has `endpoint.pid`; `PtyOptions.env_vars` carries
`WARP_TERMINAL_SESSION_UUID` (`focus_env::add_session_focus_env_vars`). Both are stored on
`SupervisedSession` at spawn and copied into every summary. A session handed over by a client
that spawned it elsewhere keeps `shell_pid = 0` and an empty uuid, and is treated as unidentified.

`PROTOCOL_VERSION` becomes 7. `framing_tests` gets `wire_v7.hex` in place of `wire_v6.hex`, with
the golden bytes regenerated and the doc comment on the constant extended with a v7 line. The
pipe name already separates builds, so no mixed-version decoding exists.

`DetachedSession` carries both fields for the CLI. `DetachedInput` carries the pane UUID into the
row builder.

### 2. Identifying the agent in a detached shell

The machine-wide process scan in the next section reads the agent, agent session id, cwd, PID and
`warpSessionUuid` from each valid registry entry. `agent_sessions_index::build_rows_with_scanned`
joins a detached supervisor summary to a process only when their pane UUIDs are equal. The matched
process supplies the agent conversation key and cwd.

An identified detached row joins the conversation key space `(agent, session_id)`, so it merges
with matching history and dedupes against a pane row the way running rows already do. The
old title-and-directory guess is removed; a detached shell with no exact UUID match remains an
unidentified shell.

### 3. Machine-wide agent scan

A new module `app/src/terminal/cli_agent_sessions/agent_process_scan.rs` lists every live agent
process from the registries of `DISCOVERABLE_AGENTS`, reusing `live_entry` and the process table
from `process_registry`. For each process it records:

```rust
pub(crate) struct ScannedAgent {
    pub agent: CLIAgent,
    pub session_id: String,
    pub cwd: Option<PathBuf>,
    pub pid: u32,
    pub warp_session_uuid: Option<String>,
    pub started_at_unix_seconds: u64,
    pub placement: Placement,
    pub process_count: usize,
}

pub(crate) enum Placement {
    /// The process descends from this build's terminal supervisor.
    Known,
    /// Its shell belongs to a supervisor started from another build.
    HeldByOtherBuild { build: String, supervisor_pid: u32, saved_in: Option<SavedPane> },
    /// No pane of this build, no supervisor of any build, and no saved layout knows its uuid.
    NoKnownPane { saved_in: Option<SavedPane> },
}

pub(crate) struct SavedPane {
    pub session_name: String,
    pub tab_title: String,
}
```

Placement is decided in this order, and never from a broken parent chain alone:

1. **Known** when walking the pid's parents reaches this executable's terminal supervisor. Exact
   pane and detached UUID matches account for the process while rows are merged. A known process
   not represented in this window remains visible as `KnownProcess` ("this build") rather than
   disappearing.
2. **HeldByOtherBuild** when walking the pid's parents reaches a process whose command line
   contains `--terminal-session-supervisor` and whose executable is not this one. The build is
   the `versions\<build>\` path segment of that executable, or the executable's file name when
   there is none.
3. Otherwise, the uuid is looked up in the saved layouts (`session_store::read_catalog` and
   `terminal_pane_uuids`). A hit gives `saved_in`, which the row shows as "saved in M3RP › WA3P".
   With a hit the placement is still **HeldByOtherBuild** when a supervisor of another build is
   running at all (the uuid walk failed only because of the Pi exec gap), else **NoKnownPane**.
4. **NoKnownPane** when nothing above applies.

The scan also counts live processes per `(agent, session_id)`; the count is attached to every row
of that conversation as `process_count`.

The scan runs once per modal opening, in the same background task as the history scan, and never
on render. It touches the process table once (`process_table()` already caches for five seconds)
and reads one small JSON file per agent process.

### 4. Titles and ages

`agent_session_history` gains:

```rust
/// The first line of the last user prompt in the conversation, truncated like `title`.
pub(crate) fn last_prompt(agent: CLIAgent, session_id: &str) -> Option<String>
```

It reads the last 64 KiB of the transcript, splits into lines, and scans backwards for the last
entry whose role is `user` and whose content is text, not a tool result: Claude lines are
`{"type":"user","message":{"role":"user","content":"..."|[{"type":"text","text":"..."}]}}` and
Pi lines are `{"type":"message","message":{"role":"user","content":[{"type":"text",...}]}}`. A
`<task-notification>`, `<system-reminder>` or tool-result block is skipped. When nothing is found
in the tail, the existing head-based `title` is used, then the cwd's last component.

`AgentSessionRow` gains `last_prompt: Option<String>`, uses its existing activity timestamp for
the transcript mtime or process start fallback, and gains `process_count: usize`.
`AgentSessionState` gains:

```rust
KnownProcess { pid: u32 },
HeldByOtherBuild { pid: u32, build: String, saved_in: Option<SavedPane> },
NoKnownPane { pid: u32, saved_in: Option<SavedPane> },
```

with labels "this build", "held by another build", and "no pane".

### 5. The modal

`view_rows` groups detached, this-build, other-build, and unplaced rows under an empty-aware
"Other local sessions" section after the window's panes. Every non-pane row renders as:

```
● running   Claude  Fix the CORS preflight for Idempotency-Key            [Attach]
            T:\projects\SOTZ3N\stoz3n-agent-dash · detached 2h · active 5m ago · runs 2×
```

The first line is the state label, the agent's display name and the title; the second line is the
detail, built from the parts that exist, joined by " · ". "runs 2×" appears only when
`process_count > 1`, in the warning color.

Buttons by state:

| State | Button | Effect |
|---|---|---|
| Detached, identified | Attach | `AgentSessionAction::Attach`, unchanged |
| Detached, unidentified | Attach | unchanged; title is "shell" |
| KnownProcess | None | remains visible for diagnosis without offering an unsafe action |
| HeldByOtherBuild | End process | ends only the selected process; the row becomes a past row |
| NoKnownPane | End process | ends only the selected process; the row becomes a past row |

Ending is a two-click action: the first click relabels the button "Confirm end"; the second calls
`process_registry::terminate_pid(agent, pid, session_id)`. Selecting another row or refreshing
resets confirmation. Failures stay visible in the selected row's preview and return the button to
its unarmed state. The view refreshes history and process placement after an attempt.

The command-palette data source places these processes under "Running elsewhere", so searching
by last prompt finds every registered agent.

### 6. CLI

`warpspeed sessions list --json` includes `shell_pid` and `session_uuid_hex`.

A new `warpspeed sessions agents [--json]` runs the scan out of process and prints one row per
agent process: agent, session id, pid, state, build, saved-in, cwd, last prompt, process start,
last active, and duplicate count. The pretty format mirrors the modal's two lines.

## Error handling

- A registry file that fails to parse, or whose pid belongs to a process that started after the
  entry was written (`entry_can_belong_to`), is skipped.
- A transcript that cannot be opened yields no `last_prompt`; the row falls back as in section 4.
- End-process failures surface in the selected row's preview and the button returns to
  "End process".
- The supervisor being unreachable keeps today's `SupervisorUnreachable` row; the scan still
  runs, so "Held by another build" and "No known pane" fill even then.

## Testing

All tests live in `*_tests.rs` files beside their modules, per repo convention.

- `agent_sessions_index_tests`: an exact detached UUID merges with history and accounts for its
  process; a known process outside the current window stays visible; `process_count` reaches every
  row of a conversation.
- `agent_process_scan_tests`: placement on a synthetic process table and registry directory:
  under this build's supervisor, under another build's supervisor (build parsed from the path),
  Pi with a dead parent but a uuid in a saved layout, Claude with no parent and no uuid anywhere.
- `agent_session_history_tests`: `last_prompt` on Claude and Pi fixtures, including a transcript
  whose last entries are a tool result and a task notification, a transcript shorter than the
  tail window, and an empty file.
- `framing_tests`: golden bytes for v7; the v6 fixture file is deleted.
- `session_table_tests`: `shell_pid` and `session_uuid_hex` survive spawn, detach and list.
- `sessions_cli` snapshot: `sessions agents --json` on a fixture scan.
- `agent_sessions_view_tests`: the two-click end-process state machine and provenance details.

## Rollout

The protocol bump means windows of the new build see only the new build's supervisor, which is
already the case for any build change. The machine-wide scan is what makes the old supervisor's
agents visible across that boundary, so after installing, the modal in a new window lists the
sessions still held by the 997f1e1c supervisor under "Held by another WarpSpeed build", with Kill
and the saved-in pane name, and nothing is duplicated by opening the modal.
