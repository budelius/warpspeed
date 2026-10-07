# Session holder guard: never resume an agent that is still running elsewhere

**Status:** design
**Date:** 2026-10-07
**Branch:** `feat/session-holder-guard`, off `dev`
**Scope:** The restore path for terminal panes that carry a persistent session id, the take-over
action it offers, Pi session-id capture, and a Pi event log. The supervisor protocol and its
transports are unchanged.

## Problem

Supervised shells survive their window closing. Each installed build runs its own supervisor,
reached through a pipe whose name hashes the state directory and the executable path
(`crates/warp_terminal/src/local_tty/windows/pipe_transport.rs:53`). A window reopened in an
instance of a different build therefore cannot reattach: `Pty::attach` fails
(`app/src/terminal/local_tty/terminal_manager.rs:1305`), the pane starts a fresh shell, and the
restore path queues the pane's resume command anyway (`app/src/pane_group/mod.rs:1706`).

On 2026-10-07 the STOZN window was closed in the 2026-10-04 build and reopened in a 3885dddc
instance. Its four warp Claude sessions were still alive, idle and detached under the old
supervisor (pids 179100, 171520, 181368, 2744). The reopen ran `claude --resume` on the same ids
again, so three sessions had two processes on one transcript, and two of the new processes never
produced a turn. The user saw stuck panes. Nothing told them the originals were still running.

Two related faults showed up in the same incident:

- **Pi markers never carry a session id.** Warp finds an agent's session by walking the pane's
  shell down to the agent process and reading the agent's own registry entry
  (`app/src/terminal/cli_agent_sessions/process_registry.rs`). Under Git Bash, `pi` is an
  `sh` shim that `exec`s Node, and Cygwin's exec leaves the Node process with a parent pid that no
  longer exists. The walk from the shell never reaches Pi, the marker is saved as
  `{"agent":"Pi","session_id":null}`, and a restore starts a bare `pi`.
- **Pi dies without a trace.** The b2bscouts Pi ended at 00:27:41 in the same second its
  transcript recorded a provider transport failure thrown from a timer callback. Pi has no log of
  its own, so the only evidence is the transcript's last line.

## Goals

- A restored pane never runs a resume command for a session another supervisor still holds.
- The user sees who holds it and can end it there and resume here with one action.
- Taking over ends the agent process, not only the shell.
- Pi markers carry the session id whenever Pi is running in the pane.
- Pi leaves a per-session log that records every turn's outcome and every way the process can
  end.

## Non-goals

- Attaching a pane to another build's supervisor. The pipe stays per build.
- Changing the supervisor protocol or any of its messages.
- Explaining why bash exited together with Pi on the 2026-10-04 build. That instance's log is
  not recoverable.

## Part 1: the holder record

`TerminalPaneSnapshot` (`app/src/app_state.rs`) gains one optional field next to
`persistent_session_id`:

```rust
#[serde(default)]
pub session_holder: Option<SessionHolder>,

pub struct SessionHolder {
    /// The supervisor pipe or socket name that holds the session.
    pub channel: String,
    /// The build that started that supervisor, as the instance marker records it.
    pub version: String,
    /// The instance that saved the pane.
    pub pid: u32,
}
```

It is written wherever `persistent_session_id` is written, from this instance's own
`supervisor_pipe_name()` and the values already in its `InstanceMarker`
(`app/src/remote_control/address.rs:30`). Snapshots without the field deserialize as `None` and
restore exactly as today.

## Part 2: the guard at restore

In the restore path, a pane whose snapshot has both `persistent_session_id` and
`session_holder` consults the holder before anything starts. The check runs where the attach
attempt already runs, bounded by a one-second budget shared by its two requests; a missing pipe
fails at connect.

The pane connects to `session_holder.channel`, pings, and lists sessions. The verdict is one of:

| Holder says | Pane does |
|---|---|
| Unreachable, or the session is not listed | Today's path: fresh shell, queued resume command. |
| Listed, detached, and `channel` is not ours | The shell starts as today; the resume command is withheld. Banner: "Still running under build `<version>` (pid `<pid>`)" with **Take over** and **Start fresh**. |
| Listed and attached | The shell starts as today; the resume command is withheld. Banner: "Open in another window" with **Start fresh** only. |
| Listed, detached, and `channel` is ours | Not reached: the pane attaches, as it does today. |

**Start fresh** runs today's path. **Take over** runs Part 3, then today's path.

The guard is one new function, `session_holder::verdict(holder, session_id) -> Verdict`, called
from the restore code where `cli_agent_resume_command` is computed. It owns the connection; the
restore code only switches on the verdict.

## Part 3: take over ends the whole session

The supervisor's `Kill` on Windows is `TerminateProcess` on the shell
(`crates/warp_terminal/src/local_tty/windows/mod.rs:159`). The agent child survives it. Pid 34440
has been an orphaned `claude` since 2026-10-06 for exactly this reason.

Take over therefore runs in this order, and stops at the first failure with the banner showing the
error:

1. Find the agent process by the agent session id in the pane's resume descriptor: Claude Code's
   `~/.claude/sessions/<pid>.json` and Pi's `warp-sessions/<pid>.json` both carry `sessionId`,
   so no shell pid or process tree is needed. Terminate it. If no live entry names that session,
   skip this step.
2. Send `Kill { session_id }` to the holder.
3. Wait until `ChildStatus` reports not running, bounded by a short timeout.
4. Run today's path: fresh shell, queued resume command.

Step 1 happens before step 2 so a Claude process cannot outlive its shell and keep writing to the
transcript the new process is about to resume.

`Start fresh` and `Take over` both end by queueing the withheld command into the pane's
already-running shell.

## Part 4: Pi session ids by shell uuid, not by process tree

Warp exports `WARP_TERMINAL_SESSION_UUID` into every shell it starts
(`crates/warp_terminal/src/focus_env.rs:7`). The Pi extension Warp installs,
`app/src/terminal/cli_agent_sessions/process_registry/warp-session-registry.ts`, adds that value
to the `<pid>.json` entry it already writes:

```json
{"pid": 230728, "sessionId": "...", "cwd": "...", "warpSessionUuid": "...", "updatedAt": ...}
```

`process_registry` looks Pi entries up by the uuid Warp exported into the pane's shell first and
falls back to the tree walk only for entries written by an older extension that lack the field.
The implementation plan verifies that this value is the pane's `TerminalPaneSnapshot::uuid`; if
it is a different id, the pane records the exported value alongside it. Claude Code's own
`~/.claude/sessions/<pid>.json` has no such field, so Claude keeps the tree walk, which works for
it because `claude.exe` is spawned directly.

## Part 5: the Pi event log

The same extension appends one JSON line per event to
`<agent dir>/warp-sessions/logs/<session-id>.log`:

| Hook | What is written |
|---|---|
| `session_start`, `session_shutdown` | event, session file, cwd, pid |
| `agent_start`, `agent_end` | event |
| `message_end` | role, `stopReason`, `errorMessage`, `diagnostics` when present |
| `process.on("SIGHUP" / "SIGINT" / "SIGTERM")` | the signal, then the handler re-raises nothing and lets Pi's own handling proceed |
| `process.stdin.on("end" / "close")` | event |
| `process.on("unhandledRejection")` | the reason's message and stack |
| `process.on("uncaughtExceptionMonitor")` | the error's message and stack; this hook observes without changing whether Node exits |
| `process.on("exit")` | the exit code |

Rules the extension keeps: every handler is wrapped in `try`/`catch` and never throws into Pi;
writes are synchronous appends so an exit handler can finish; a file past 5 MB is truncated to its
last 1 MB before the next append; nothing is written when the session id is unknown.

Warp reinstalls the extension from the embedded source on startup, so shipping this file ships the
change.

## Error handling

- A holder that answers with a different protocol version is treated as unreachable: the pane
  takes today's path. Mixing builds is what the per-build pipe exists to prevent.
- A take-over whose kill fails leaves the banner up with the error text and both actions still
  available.
- A registry entry whose pid is no longer alive is ignored, as it is today.

## Testing

- `session_holder::verdict` gets unit tests against a fake holder for each row of the table in
  Part 2, including the protocol-version mismatch.
- Take over gets an integration test under the real supervisor: spawn a shell with a long-lived
  child, take the session over, and assert that both the shell and the child are gone before the
  resume command runs.
- `process_registry` gets a test where the Pi entry carries `warpSessionUuid` and the process
  tree does not connect the shell to Pi, and the lookup still resolves.
- The extension gets a Node test that drives each hook with a fake `ExtensionAPI`, triggers each
  process event in a child process, and reads the log back, including the truncation rule.
- A render-to-lines or GUI test covers the banner's two action sets.

## Rollout

The snapshot field and the extension field are additive. Panes saved by this build and reopened on
an older build ignore both. Panes saved by an older build and reopened here take today's path
until they are saved again.
