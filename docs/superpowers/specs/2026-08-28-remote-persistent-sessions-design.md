# Remote persistent sessions: the same supervisor, over two more transports

**Status:** design
**Date:** 2026-08-28
**Depends on:** `2026-08-28-persistent-terminal-sessions-design.md` (Spec A)
**Scope:** Spec B — reaching a persistent session that lives on another machine.

## Problem

Spec A keeps sessions alive on the machine Warp runs on. The same want applies across machines: a
build on the Linux box, a Claude Code run on the Pi, an agent working in a cloud VM — started from
the laptop, closed, and picked up later from wherever.

Warp has no path for this today. `crates/remote_server/` exists and is gated by
`FeatureFlag::SshRemoteServer`, but its RPCs are `RunCommand`, `GetDiffState`, `OpenBuffer`,
codebase index and repo metadata. Those are code and filesystem services. **It does not host
PTYs.** Remote persistent shells are unbuilt.

## The design in one line

Ship the *same* supervisor binary and the *same* control protocol everywhere, and change only the
`Transport` implementation.

Spec A defines `Transport` as a framed message stream with an **optional** `lend_endpoint()`
capability. Locally that capability is present and the app does direct I/O on a borrowed fd or
handle. Off-machine it is inherently absent — you cannot pass a file descriptor over a network — so
the transport reports no lending capability and the supervisor streams instead. Everything above
that line, including detach, screen snapshots, the ring buffer, and reattach, is byte-identical to
the local path.

Spec A ships a loopback transport with `lend_endpoint()` disabled specifically to prove this path
works before any remote code exists.

## Two transports, for two different situations

### SSH, via `remote_server`

For any host you can already SSH into. Reuses what `crates/remote_server/` has: `ssh.rs`,
`transport.rs`, `auth.rs`, and `install_remote_server.sh` for deploying the binary. One auth story,
no new daemon exposed to the network, and it works for hosts that are not part of any fleet.

The supervisor runs on the remote host and speaks its normal protocol over the SSH channel.

### agent-ctrl tunnel

For machines you *cannot* reach — behind NAT, on a home network, a Pi on someone else's LAN.

`stoz3n-agent-ctrl` is a fleet orchestrator whose nodes **dial out** to a controller and hold one
bidirectional `OpenStream`, registering with an `auth_token` and optional mTLS
(`src/remote/tls.rs:35,69`). That outbound-dial shape is the thing of value here: it traverses NAT
without inbound firewall rules, and it already carries a fleet inventory and an auth model.

Its payloads today are `AgentOutputChunk` and `SendInputCommand` — pipe-shaped, with no PTY, no
resize, and no terminal semantics. So this transport adds terminal messages rather than reusing the
agent ones.

## Changes to agent-ctrl

Additive only, inside the existing `oneof` payloads in `proto/agentctrl/v1.proto`. No field is
renumbered and no existing message changes, so older nodes keep working.

To `ControllerMessage`:

- `OpenTerminalCommand` — shell, cwd, env, size, session id
- `AttachTerminalCommand` — session id, client identity
- `TerminalInput` — session id, bytes, sequence number
- `ResizeTerminal` — session id, columns, rows
- `DetachTerminalCommand` — session id, screen snapshot
- `ListTerminalsCommand`

To `NodeMessage`:

- `TerminalOutput` — session id, bytes, sequence number
- `TerminalAttached` — session id, screen snapshot, resume sequence number
- `TerminalClosed` — session id, exit status
- `TerminalList` — live sessions with shell, cwd, title, age

The node hosts the Warp supervisor binary and proxies frames to it; the controller relays; Warp
connects as a `FleetControl` client. The supervisor is not modified — it sees a `Transport`.

## Reconnect and flow control

The network makes two problems real that the local path does not have.

**Reconnect.** Output frames carry monotonically increasing sequence numbers, and the supervisor's
ring buffer is indexed by them. A dropped connection leaves the session running on the remote host;
the pane shows a disconnected state and retries with backoff. On reattach the client sends the last
sequence number it saw and receives the current screen snapshot plus everything after that point.
If the ring buffer has already discarded it, the snapshot alone is sent and the gap is reported in
the pane rather than silently skipped.

**Backpressure.** A remote `yes` at full tilt must not queue unboundedly in the controller. The
supervisor stops reading the PTY when its ring buffer is full for a session with no draining
consumer, letting the kernel's PTY buffer apply the backpressure — the same thing that happens when
a local terminal stops reading.

## Security

Remote sessions inherit the transport's authentication and add no new surface: SSH credentials for
the SSH transport, the existing node `auth_token` plus optional mTLS for agent-ctrl. The supervisor
still binds only a local socket or pipe on the remote host, restricted to its own user; it is never
exposed to the network directly. The tunnel terminates in the node process, which is already
authenticated to the controller.

A remote session grants shell access to the remote machine, so the drawer labels remote sessions
with their host and never mixes them silently with local ones.

## Non-goals

- Teaching agent-ctrl's *agent* runs to be terminals. Agent runs stay pipe-shaped; terminal
  sessions are a separate concern that happens to share the transport.
- Replacing warpified SSH. An ordinary `ssh` pane keeps working as it does now. This is for
  sessions that must outlive the connection.
- Reboot survival, on either end.

## Risks worth stating

**Two repos, two release cycles.** The agent-ctrl work lands in a separate project with its own CI.
It will move slower than Spec A no matter how it is sequenced, and a Warp release cannot assume a
matching node version.

**Version skew.** The node hosts a Warp supervisor binary that may be older or newer than the Warp
client. The protocol version already lives in the handshake for Spec A; here it must be reported in
`TerminalList` and surfaced in the drawer, so an unattachable remote session says why instead of
failing obscurely.

**The SSH transport is the cheaper half and covers more hosts.** If only one gets built, it should
be that one. agent-ctrl earns its place specifically for machines SSH cannot reach.

## Testing

- The loopback transport test from Spec A is the contract; both remote transports must satisfy it
  unchanged.
- Sequence-number resume: drop the connection mid-output, reconnect, assert no gap and no
  duplication.
- Ring-buffer overrun: force a discard, assert the gap is reported rather than hidden.
- Backpressure: a fast writer with no consumer must not grow memory without bound in node or
  controller.
- agent-ctrl proto compatibility: an older node that does not know the terminal messages must still
  register and run agents.
