# Persistent terminal sessions: survive app exit, reattach later

**Status:** design
**Date:** 2026-08-28
**Scope:** Spec A — the local, cross-platform half. Remote transports are Spec B
(`2026-08-28-remote-persistent-sessions-design.md`) and are out of scope here.

## Problem

Quitting Warp kills every shell it owns. A running build, a test suite, a Claude Code turn
mid-flight — all die with the app. There is no way to close the window and come back to the work.

A partial answer already ships. `FeatureFlag::PersistentLaunchConfigSessions` gates a session
supervisor (`crates/warp_terminal/src/local_tty/server/persistent.rs`) that keeps PTYs alive and
reattaches them. It has three limits that make it not the feature:

1. **Unix only.** `local_tty/server` is `#[cfg(unix)]`. The mechanism is `SCM_RIGHTS` fd passing,
   which has no Windows equivalent. On Windows the supervisor does not exist at all.
2. **Launch-config windows only.** An ad-hoc window — the common case — is not covered.
3. **Raw byte spool.** Output while detached is appended to a temp file and replayed. Correct for
   line-oriented output, visibly wrong for anything on the alternate screen: a reattached `htop`,
   `vim`, or Claude Code TUI paints garbage.

One test covers the whole mechanism
(`persistent_tests.rs::detached_output_is_replayed_when_session_reattaches`).

## Goals

- A session survives Warp exiting on **both** Unix and Windows.
- Reattach restores a **correct screen**, including alternate-screen applications.
- Sessions are kept **unless provably idle**; nothing the user cares about is silently killed.
- Sessions with no window to return to are **visible and recoverable**, not invisibly orphaned.
- The control protocol admits a remote transport **without a protocol break**.

## Non-goals

- Surviving a reboot. The supervisor is a process; when the machine goes down, so do the shells.
- Remote or multi-machine sessions. That is Spec B.
- Putting the supervisor in the I/O hot path. See "Why not proxy everything".

## Architecture

One supervisor daemon per user per state directory. It **always** owns the shell process and its
PTY; Warp is only ever a client that borrows the I/O endpoint.

```
Warp app process                     Supervisor process (outlives Warp)
+------------------+                 +----------------------------------------+
| TerminalManager  |                 | Supervisor                             |
|  PtyHandle ------+-- control ------+-> sessions: HashMap<SessionId, Session> |
|  event_loop  <---+-- direct I/O ---+---- owns child + PTY, always            |
+------------------+   (fd / HANDLE) |     HeadlessScreen (while detached)     |
                                     +----------------------------------------+
```

### Why not proxy everything

tmux sits between the client and the PTY for every byte. Warp deliberately does not, and the
rationale is already recorded at `local_tty/server/mod.rs:124`: handing the endpoint to the app
beats relaying through the server. We keep that. While a pane is attached, the supervisor performs
no I/O for it at all.

The cost of that choice is that the supervisor is **blind while attached**, so its emulator cannot
simply observe the stream. Screen state is therefore *handed over* at the detach boundary rather
than continuously tracked. That handoff is the load-bearing idea in this design.

### Platform backends

Only two things differ per platform. Everything above the `Transport` trait is shared.

| | Unix | Windows |
|---|---|---|
| Transport | `AF_UNIX`, `/tmp/terminal-session-supervisor-v2.sock-<uid>-<hash>`, mode `0600` | Named pipe `warp-terminal-supervisor-v2-<sid-hash>`, DACL limited to the current user SID |
| Endpoint lending | `SCM_RIGHTS` fd passing (exists today) | `DuplicateHandle` of the ConPTY pipe handles into the client PID |
| Detach | Warp closes its fd; supervisor resumes reading the leader | Warp closes its duplicates; supervisor resumes reading its originals |
| Daemonize | `setsid()` (exists today) | `DETACHED_PROCESS`, and excluded from Warp's job object so it is not killed with the tree |

Windows already loads a bundled `conpty.dll` exposing `ConptyReleasePseudoConsole`
(`windows/conpty_api.rs:102`). The supervisor creates the ConPTY, so `Release` is not needed on the
normal path; it is the escape hatch for adopting a pseudoconsole created elsewhere.

## The screen snapshot is ANSI, not a serialized grid

`GridHandler` cells carry a hyperlink registry, an image map, and secret handles, and
`GridHandler::new` takes a pixel-based `SizeInfo`. Serializing that structure across a version
boundary would be fragile, and would put a cell format in the protocol forever.

Instead a snapshot is **a self-contained repaint byte sequence**:

```rust
struct ScreenSnapshot {
    columns: u16,
    rows: u16,
    cell_width_px: u16,   // HeadlessScreen needs these to build a SizeInfo
    cell_height_px: u16,
    repaint: Vec<u8>,     // reset, modes, alt-screen, SGR + text per row,
                          // scroll region, cursor position and visibility
}
```

Both directions become "here are some bytes". The supervisor feeds a snapshot to `HeadlessScreen`;
the app feeds one to its ordinary grid. The protocol never learns what a cell is.

This needs one new piece of code — a **grid-to-ANSI renderer** — and it buys the round-trip
property test described under Testing.

## Components

All paths under `crates/warp_terminal/src/local_tty/` unless noted.

| Component | State | Purpose |
|---|---|---|
| `server/persistent.rs` `Supervisor` | extend | Session table, owns child + PTY, TTL and GC |
| `server/screen.rs` `HeadlessScreen` | new | Drives the existing `model/grid` `GridHandler` with no UI; seeds from a snapshot, parses detached bytes, renders a snapshot |
| `model/grid/render_ansi.rs` | new | Grid to repaint byte sequence |
| `server/transport.rs` `Transport` | new | Framed message stream, plus an **optional** `lend_endpoint()` capability |
| `server/protocol.rs`, `server/api.rs` | extend | Version 2 message set; version encoded in the socket/pipe name |
| `windows/supervised.rs` | new | Supervisor-owned ConPTY; handle duplication into the client |
| `DetachedSessionsModel` + drawer | new (app) | Orphan list: attach into a pane, or discard |

### Protocol changes from v1

`server/api.rs` today has `PrepareForTerminationRequest { client_pid, persistent_session_ids }` —
a single message that names which sessions to keep at quit. It is replaced by a per-session
`Detach { session_id, snapshot, metadata }`, because the snapshot is per-session and must travel
with it. `AttachSessionResponse` gains the snapshot and the buffered tail, which makes
`PtySpawnResult::replay_fd` obsolete: the temp-file replay it points at is exactly the raw-spool
mechanism this design replaces. `ListSessions` is new. All of this is why the version in the socket
and pipe name moves to `v2`.

## Data flow

### Quit

1. `on_will_terminate` classifies each pane: `Idle | Busy | Unknown`.
2. **Provably idle** — warpified, no executing block (`Block::is_executing`), no tracked CLI agent
   session, no live descendants — is killed, as today.
3. Everything else survives. The app renders the pane's current screen to a `ScreenSnapshot` and
   sends `Detach { session_id, snapshot, metadata }`, then closes its endpoint.
4. The supervisor seeds `HeadlessScreen` from the snapshot and resumes reading into a bounded ring
   buffer, updating the screen as bytes arrive.
5. A session-index row is written to sqlite: session id, pane and window identity, shell, cwd,
   title, detach time.

`Unknown` survives by design. The classifier is trusted only when it says *idle*.

### Launch

1. Connect to the supervisor, spawning it if absent. `ListSessions` returns the live set.
2. Session restore matches restored panes by session id and sends `Attach`. The response carries
   the endpoint, the current `ScreenSnapshot`, and buffered output since that snapshot.
3. The pane paints the snapshot, applies the buffered tail, and resumes direct I/O.
4. Any live session no restored pane claimed goes to `DetachedSessionsModel` and appears in the
   detached-sessions drawer with shell, cwd, age, and what is running. From there it can be
   attached into any pane or discarded. Ignored sessions are reaped at TTL (default 24h,
   configurable).

## Error handling

- **Supervisor unreachable.** Fall back to the current ephemeral spawn path. The feature degrades
  to today's behavior; nothing else breaks.
- **Attach rejected** (session gone, or already attached by another client). The pane opens as a
  fresh shell with a banner explaining which, plus the last known screen from sqlite if present.
- **Protocol mismatch after an upgrade.** Do not adopt. The version is in the socket and pipe name,
  so the old daemon keeps running with its sessions; the drawer lists them as *from an earlier
  version, not attachable* and the TTL reaps them.
- **Ungraceful app death** (crash, `kill -9`). No snapshot was handed over. The supervisor seeds a
  blank screen at takeover, and reattach additionally issues a resize-nudge (resize to `cols - 1`,
  then back) so well-behaved TUIs repaint. Output is correct from that point forward; the pre-crash
  screen is lost. This is an accepted limitation, not something to fix with heartbeat snapshots —
  those cost continuous work to serve a rare case.

## Testing

**The discriminating test.** Feed identical byte streams to the app's grid and to `HeadlessScreen`,
then assert the resulting screens are equal. If seeding and parsing are not equivalent, nothing
downstream is trustworthy. Written first.

**The renderer round-trip.** bytes to grid A, render to bytes, feed to grid B, assert A equals B.
This is the property that makes the ANSI-snapshot design sound, and it fails loudly for any escape
sequence the renderer cannot reproduce.

Then:

- **TUI fidelity.** A fixture that writes an alternate-screen layout; detach mid-screen; reattach;
  assert screen equality with the pre-detach screen plus subsequent output.
- **Idle classification truth table.** Warpified idle, warpified executing, tracked agent session,
  non-warpified with and without descendants; assert kill only in the first case.
- **Real-shell integration, both platforms.** Spawn through the supervisor, start a long-running
  command (`sleep`, `ping -t`), detach, reconnect from a new client; assert the pid is identical
  and output is continuous across the boundary.
- **Lifecycle.** TTL reaping, double-attach rejection, supervisor restart, stale-socket takeover,
  termination-preserve selection.
- **Loopback transport with `lend_endpoint()` disabled.** Proves the streaming path works before any
  remote code exists, and keeps Spec B honest.

## Rollout

Behind `FeatureFlag::PersistentTerminalSessions`, dogfood first. The existing
`PersistentLaunchConfigSessions` flag is subsumed: launch-config windows become one caller of a
general mechanism rather than the only one.
