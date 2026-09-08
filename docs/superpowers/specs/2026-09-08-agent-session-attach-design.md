# Attaching sessions: keep the window, and find what was closed

**Status:** design
**Date:** 2026-09-08
**Scope:** The local recovery path for terminal sessions and CLI agent conversations. The
supervisor's own protocol and transports are unchanged; this spec sits above them and adds a
second, supervisor-independent source of sessions.

## Problem

Detaching a session in the only pane of the only tab quits Warp, and the reopened app offers no
way back in. Three separate faults compose into that one experience.

**Detach closes the window.** `detach_session_for_terminal_view`
(`app/src/workspace/view.rs:13555`) hands the session over and then calls `close_pane`
unconditionally. When that pane is the last one, the tab goes, the window goes, and on a
single-window app so does the process. The session survives; the user watching the screen has no
way to know that.

**The way back is hidden behind the thing that broke.** The one visible entry point,
"Attach detached session", is a row in a tab's right-click submenu drawn only when
`detached_sessions::are_supported(ctx)` holds (`app/src/tab.rs:665`). That predicate needs a
reachable supervisor. So in exactly the state where recovery matters — supervisor unreachable
from a freshly started app — the row that would explain the situation is the row that vanishes.
The command-palette entries carry no default keystroke (CHANGELOG, "with no default
keybinding"), so there is nothing else to find.

**The list only knows live PTYs.** `detached_sessions::list` asks the supervisor and nothing
else. The persistent-sessions design already concedes the consequence: "a session outlives its
metadata only as long as the supervisor does." Meanwhile Claude, Codex and Pi each keep a full
on-disk history that survives reboots, and `CliAgentResumeDescriptor::resume_command()`
(`app/src/app_state.rs:296`) already knows how to reopen one. Nothing joins the two.

## Goals

- Detaching never closes the window.
- A session is reachable from a cold-started app with no supervisor, from a surface a user finds
  without being told it exists.
- Closed Claude, Codex and Pi conversations are listed and resumable, ordered so the ones for the
  directory in front of you come first.
- Listing costs bounded work regardless of how much history is on disk.

## Non-goals

- Reviving a dead PTY. A closed agent session resumes as a *new* process replaying the
  conversation, which is what `claude --resume` does; the original shell is gone.
- Typeahead search in the picker. `Menu` has no filter today, and adding one is its own change.
  Ordering plus a cap carries v1.
- Agents whose CLI cannot reopen a session by ID. `resume_command()` returns `None` for Gemini,
  OpenCode, Amp and the rest, and a guessed flag is worse than no row.
- Remote sessions. Local stores only.

## Part 1 — Detach keeps the window

Detaching one pane of a split should still close that pane; that is correct and stays. The only
case to guard is the one where closing would take the window with it.

Before closing, `detach_session_for_terminal_view` asks whether this pane is the last pane of the
last tab in the window. If it is, a replacement tab is added **first** — a single terminal with
`initial_directory` set to the detached session's working directory — and only then is the old
pane closed. The window never passes through zero tabs, so nothing downstream can interpret the
detach as a quit.

Ordering matters here for the same reason the existing code hands the session over before closing
the pane: the recoverable state must be established before the destructive step, never after.

## Part 2 — A second source of sessions

A new module, `app/src/terminal/agent_session_history/`, with `mod.rs` and one scanner per store
(`claude.rs`, `codex.rs`, `pi.rs`), each with a sibling `*_tests.rs` in the repo's convention.

```rust
pub struct PastAgentSession {
    pub agent: CLIAgent,
    pub session_id: String,
    pub cwd: PathBuf,
    pub title: Option<String>,
    pub modified: SystemTime,
}
```

### The three layouts

| Agent | Location | Session ID | `cwd` and title |
|---|---|---|---|
| Claude | `<config>/projects/<slug>/<uuid>.jsonl` | file stem | first entry carrying `cwd`; title from the first user message |
| Codex | `<sessions>/YYYY/MM/DD/rollout-<ts>-<uuid>.jsonl` | trailing UUID of the stem | `session_meta` line, parsed by the existing `parse_session_meta` |
| Pi | `~/.pi/agent/sessions/--<slug>--/<ts>_<uuid>.jsonl` | after the `_` in the stem | first line is a `session` record with `id`, `timestamp`, `cwd` |

Roots come from code that already exists rather than new constants:
`claude_transcript::claude_config_dir()` (`claude_transcript.rs:89`, honors `$CLAUDE_CONFIG_DIR`)
and `codex_transcript::codex_sessions_root()` (`codex_transcript.rs:95`, honors `$CODEX_HOME`).
Pi's root is new: `~/.pi/agent/sessions`. Pi exposes no documented home override that has been
verified against the shipped CLI, so none is invented; the resolver takes its home the same way
the Claude module does, through a helper that honours `HOME` under `#[cfg(test)]` so fixtures can
redirect it.

**The directory slug is never decoded.** Claude's and Pi's per-project directories are lossy
encodings of a path — `T:\projects\dev-ops\warp` becomes `T--projects-dev-ops-warp`, and the
separators, drive colon and any literal dashes are indistinguishable afterwards. Every scanner
therefore takes `cwd` from inside the file, where it is stored verbatim. A session whose head
yields no `cwd` is listed with the slug shown as-is and is still resumable, since resuming needs
only the ID.

### Scanning is two-stage

A single Pi transcript on the author's machine is 35 MB. Reading transcripts to build a menu is
not an option, so identity and content are separated:

1. **Enumerate.** Walk each root with `read_dir`, opening nothing. Yields path, session ID from
   the filename, and mtime from the directory entry. Codex's `YYYY/MM/DD` tree is walked newest
   year first and stops once enough candidates are in hand.
2. **Read heads.** Keep the newest 200 candidates across all three agents by mtime, and read at
   most the first 64 KB or 40 lines of each for `cwd` and title. Titles are trimmed to a single
   line and truncated to 80 characters.

Worst case is 200 bounded reads, independent of how much history exists.

### Where the scan runs

On a background executor, into an `AgentSessionHistoryModel` cache. The scan is started when a
workspace opens and refreshed when the picker opens; the picker renders whatever the cache holds
and updates in place when a refresh lands. A cold cache renders a "Looking for sessions…" row.

This deliberately diverges from the supervisor list, which blocks the UI thread on open and
justifies it as "a blocking round trip … it runs only when the user opens the picker." That
reasoning holds for one IPC call and does not extend to 200 file reads.

## Part 3 — One picker, two sections

`attach_session_menu_modal` is retitled "Sessions" and built from both sources, using the
`MenuItem::Header` and `MenuItem::Separator` variants that already exist (`app/src/menu.rs:1475`).

```
Running
  ⌨  claude — T:\projects\dev-ops\warp (3m ago)
  ─────────────────────────────────────────────
Recent
  π   Fix the snap-layout hit test — T:\projects\dev-ops\warp (2h ago)
  ✳   Forge token resolution — T:\projects\dev-ops\warp (yesterday)
  ⬡   rollout for stoz3n-chat-agent — T:\projects\SOTZ3N\… (3d ago)
```

**Ordering.** Sessions whose `cwd` equals the focused pane's working directory come first, each
group by recency, then everything else by recency. Capped at 20 rows.

**Icons** come from `CLIAgent::icon()`, so an agent is identifiable before its title is read.

**Filtering.** A past session whose ID is live in an open pane is omitted — offering to resume the
conversation already on screen promises something confusing. This needs a small accessor on
`CLIAgentSessionsModel`, whose `sessions` map is private today
(`app/src/terminal/cli_agent_sessions/mod.rs:355`).

**Empty state** is a disabled row inside the menu naming the three directories that were searched,
not a toast. A toast closes the surface the user just opened and leaves them where they started,
which is the failure this whole spec is about.

**Resuming** dispatches a new `WorkspaceAction::ResumeAgentSession { agent, session_id, cwd }`,
which opens a tab with `initial_directory: cwd` and queues the resume command with
`set_pending_command_queue` — the same mechanism the process-handoff path already uses
(`app/src/workspace/view.rs:13162`). The command text is `resume_command()`, unchanged.

## Part 4 — Ways in, none of them gated

The picker's availability no longer depends on the supervisor, because the disk-backed half never
did. Concretely:

- **`cmd-shift-L` / `ctrl-shift-L`** on the existing `workspace:attach_detached_session` binding.
  Verified free against the current table; `shift-R`, `shift-D`, `shift-O` and `shift-Y` are all
  taken.
- **Command palette** — already registered; its `with_enabled(PersistentTerminalSessions)` gate is
  dropped (`app/src/workspace/mod.rs:1023`).
- **New-tab (+) dropdown** — a "Resume session…" entry, next to where a user already goes to start
  something.
- **Get-started pane** — a "Recent sessions" group showing the newest three, each resuming
  directly. This is the surface a cold-started window actually presents.
- **Tab menu** — the existing row, ungated and relabelled "Attach or resume session…".

The supervisor's absence stops being invisible: the "Running" section is simply empty, and the
"Recent" section still has content.

## Errors

Every failure is local to one entry and never empties the list. An unreadable root, a directory
that does not exist, a zero-byte transcript, a malformed first line, a filename with no parseable
UUID — each is skipped with a `log::warn!` and the scan continues. A scan that finds nothing
anywhere renders the empty-state row; it does not raise an error, because no agent history is a
normal state for a new machine.

Detach failure keeps its current behaviour: the pane stays open and a toast says so. The new
replacement-tab step runs only after a successful handover, so a failed detach cannot leave a
stray tab.

## Testing

Per-scanner unit tests against fixture trees in a temp home — the Claude module already overrides
`HOME` under `#[cfg(test)]` (`claude_transcript.rs:100`); Codex redirects via `$CODEX_HOME`, and
Pi through the same `#[cfg(test)]` home helper:

- IDs extracted from each of the three filename shapes.
- `cwd` recovered from the head of each format.
- Title taken from the first user message, trimmed and truncated.
- A malformed first line, a zero-byte file, a filename with no UUID, and a missing root each yield
  no entry and no panic.
- The 200-candidate cap keeps the newest, not an arbitrary 200.

Module-level tests for ordering: current-directory sessions ahead of older ones from elsewhere,
recency within each group, live sessions filtered out, cap applied after ordering.

An integration test in `crates/integration/src/test/detached_sessions.rs`: detaching the last pane
of the last tab leaves the window open with a terminal in the session's directory, and the
detached session is then listed.

A bindings test asserting the new keystroke does not collide.

## Out of scope, but noticed

`claude_transcript::encode_cwd` (`claude_transcript.rs:80`) replaces `/` and `.` with `-` and
leaves `\` and `:` alone, so on Windows it maps `T:\projects\dev-ops\warp` to itself rather than
to the `T--projects-dev-ops-warp` directory Claude actually writes. Anything resolving a Claude
transcript path by encoding a Windows `cwd` will miss. This spec's scanners never call it — they
walk the directories and read `cwd` out of the files — so the work here is unaffected, and fixing
it belongs to whoever owns the transcript-upload path.
