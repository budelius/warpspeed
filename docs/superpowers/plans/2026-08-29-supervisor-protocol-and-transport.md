# Supervisor Protocol v2 and the Transport Seam Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the supervisor's wire protocol transport-agnostic and add the v2 messages that carry a screen snapshot across a detach, proven by a loopback transport that lends no endpoint.

**Architecture:** Today's protocol is inseparable from its transport: `protocol.rs` speaks `RawFd` and `SCM_RIGHTS` throughout, and the whole `server` module is `#[cfg(unix)]`. This plan splits framing from transport, defines a `Transport` trait whose endpoint-lending capability is optional, and adds `Detach` / `Attach` / `ListSessions`. Nothing here spawns a process or touches a PTY.

**Tech Stack:** Rust 2024, `crates/warp_terminal`, `serde` + `bincode`-style length-prefixed framing as already used by `api::Message`.

## Sequencing change from the original four-plan split

The original order was protocol → Unix detach/attach → Windows → orchestration. That order cannot be executed or verified on a Windows development machine: `local_tty/server` is `#[cfg(unix)]` (`local_tty/mod.rs:12`), so every Unix supervisor change compiles to nothing here and no test can run against it.

Revised order:

1. **This plan** — framing, `Transport`, v2 messages, loopback. Fully cross-platform, fully testable here.
2. **Windows supervisor backend** — ConPTY ownership, named pipe, `DuplicateHandle`. Testable here.
3. **Unix transport impl** — adapt the existing `SCM_RIGHTS` path to the trait. **Must be verified on Linux or macOS, or in CI.** Do not mark it done from a Windows machine.
4. **Orchestration and drawer** — idle classification, quit/launch wiring, detached-sessions UI.

## Global Constraints

- Rust edition 2024. No new third-party dependencies.
- **Everything in this plan must compile on Windows and Unix alike.** Nothing added here may be `#[cfg(unix)]`. This is the whole point of the plan; a `#[cfg(unix)]` in a new file is a defect.
- Tests live in a sibling `_tests.rs` wired with `#[cfg(test)] #[path = "..."] mod tests;`.
- Doc comments on every public item, per `AGENTS.md`.
- Test gate: `cargo test -p warp_terminal --lib -- <module>`. The baseline carries one known failure,
  `local_tty::windows::child::tests::test_event_is_emitted_when_child_exits`, caused by
  `CREATE_BREAKAWAY_FROM_JOB` in `crates/command/src/blocking.rs:84` returning `ERROR_ACCESS_DENIED`
  inside a job object. The gate is **no new failures beyond that one**. Piping cargo to `tail` masks
  its exit code; check `${PIPESTATUS[0]}`.
- Assume any test comparing one derived output to another is vacuous until proven otherwise. Plan 1
  produced three such tests, and all three passed while the code was broken. Every task below pairs
  its equivalence check with an assertion on state.

## File Structure

New module `crates/warp_terminal/src/local_tty/supervisor/`, cross-platform, holding what `server/` cannot because it is Unix-only:

| File | Responsibility |
|---|---|
| `supervisor/mod.rs` | Module wiring and the `SessionId`-keyed types shared by protocol and transport |
| `supervisor/message.rs` | The v2 `Message` enum. Data only, no I/O |
| `supervisor/framing.rs` | Length-prefixed encode/decode over any `Read`/`Write`. No sockets, no fds |
| `supervisor/transport.rs` | The `Transport` trait and `Endpoint` capability |
| `supervisor/loopback.rs` | An in-memory `Transport` that lends no endpoint — the Spec B contract |
| `supervisor/ring.rs` | Bounded output buffer with sequence numbers, for detached output |

`server/api.rs` and `server/protocol.rs` stay where they are for now and are migrated in plan 3, so this plan changes no Unix code it cannot test.

---

### Task 1: Framing that does not know about sockets

**Files:**
- Create: `crates/warp_terminal/src/local_tty/supervisor/mod.rs`
- Create: `crates/warp_terminal/src/local_tty/supervisor/message.rs`
- Create: `crates/warp_terminal/src/local_tty/supervisor/framing.rs`
- Create: `crates/warp_terminal/src/local_tty/supervisor/framing_tests.rs`
- Modify: `crates/warp_terminal/src/local_tty/mod.rs` (add `pub mod supervisor;`)

**Interfaces:**
- Consumes: `serde`, `crate::local_tty::screen::ScreenSnapshot`, `warp_core::SessionId`.
- Produces:
  - `pub enum Message` with variants `Detach`, `DetachResponse`, `Attach`, `AttachResponse`, `ListSessions`, `ListSessionsResponse`, `Ping`, `Pong`
  - `pub fn write_message<W: Write>(writer: &mut W, message: &Message) -> Result<()>`
  - `pub fn read_message<R: Read>(reader: &mut R) -> Result<Option<Message>>`
  - `pub const PROTOCOL_VERSION: u32 = 2`

- [ ] **Step 1: Write the failing test**

In `framing_tests.rs`:

```rust
use super::*;
use crate::local_tty::screen::ScreenSnapshot;

fn snapshot() -> ScreenSnapshot {
    ScreenSnapshot {
        columns: 80,
        rows: 24,
        repaint: b"\x1b[H\x1b[2Jhello".to_vec(),
    }
}

/// A detach carries the screen with it, and the screen is the largest thing on
/// the wire. If framing truncates or mis-lengths it, a reattached pane paints
/// garbage -- so round-tripping the whole message is the property to hold.
#[test]
fn a_detach_message_round_trips_through_framing() {
    let message = Message::Detach {
        session_id: SessionId::new_for_test(),
        snapshot: snapshot(),
    };

    let mut buffer = Vec::new();
    write_message(&mut buffer, &message).expect("write should succeed");

    let decoded = read_message(&mut buffer.as_slice())
        .expect("read should succeed")
        .expect("a whole message was written");

    assert_eq!(message, decoded);
}

/// The reader must not treat a partial write as a whole message. A transport
/// delivers bytes when it feels like it, and a half-read snapshot that decodes
/// to "success" would repaint a pane with a truncated screen.
#[test]
fn a_partial_message_reads_as_none_rather_than_a_short_one() {
    let message = Message::Detach {
        session_id: SessionId::new_for_test(),
        snapshot: snapshot(),
    };

    let mut buffer = Vec::new();
    write_message(&mut buffer, &message).expect("write should succeed");
    buffer.truncate(buffer.len() - 1);

    let decoded = read_message(&mut buffer.as_slice()).expect("a short read is not an error");

    assert!(
        decoded.is_none(),
        "a truncated message must not decode as a whole one"
    );
}
```

Check `SessionId`'s real constructor before using `new_for_test`; `warp_core::SessionId` is the type the existing `api.rs` uses, so follow whatever that does.

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p warp_terminal --lib -- local_tty::supervisor::framing`
Expected: FAIL to compile — the module does not exist.

- [ ] **Step 3: Write the implementation**

`message.rs` defines the enum; keep it data-only so it can be shared by every transport:

```rust
//! The supervisor wire protocol, version 2.
//!
//! Data only: nothing here performs I/O, so the same messages travel over a
//! Unix socket, a Windows named pipe, or a network tunnel.

use serde::{Deserialize, Serialize};
use warp_core::SessionId;

use crate::local_tty::screen::ScreenSnapshot;

/// The protocol version, carried in the socket and pipe name so that a client
/// never speaks v2 to a v1 supervisor.
pub const PROTOCOL_VERSION: u32 = 2;

/// A message in either direction between a client and the supervisor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Message {
    /// Client is releasing a session. The screen travels with it so the
    /// supervisor can keep the session's display current while detached.
    Detach {
        session_id: SessionId,
        snapshot: ScreenSnapshot,
    },
    DetachResponse {
        error: Option<String>,
    },
    /// Client wants a session back, resuming output after `after_sequence`.
    Attach {
        session_id: SessionId,
        after_sequence: Option<u64>,
    },
    AttachResponse {
        result: Result<AttachedSession, String>,
    },
    ListSessions,
    ListSessionsResponse {
        sessions: Vec<SessionSummary>,
    },
    Ping,
    Pong {
        protocol_version: u32,
    },
}

/// What a client receives when it takes a session back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttachedSession {
    /// The session's current screen.
    pub snapshot: ScreenSnapshot,

    /// Output produced after that screen was rendered.
    pub buffered_output: Vec<u8>,

    /// The sequence number `buffered_output` ends at.
    pub sequence: u64,

    /// `true` when output was discarded because the ring buffer wrapped, so the
    /// pane can say so rather than silently showing a gap.
    pub output_was_dropped: bool,
}

/// A session as shown in the detached-sessions drawer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSummary {
    pub session_id: SessionId,
    pub shell: String,
    pub working_directory: String,
    pub title: String,
    /// Seconds since the session was detached.
    pub detached_for_seconds: u64,
}
```

`ScreenSnapshot` must gain `Serialize, Deserialize` — add them to its derive in `screen.rs`.

`framing.rs` carries the length prefix:

```rust
//! Length-prefixed message framing over any byte stream.
//!
//! Deliberately ignorant of sockets and file descriptors: the Unix transport
//! passes descriptors alongside these bytes, the Windows one duplicates
//! handles, and a network transport does neither. Framing is the part they
//! share.

use std::io::{Read, Write};

use anyhow::{Context, Result, bail};

use super::message::Message;

/// Refuses a length prefix larger than this, so a corrupt or hostile header
/// cannot make the reader allocate without bound.
const MAX_MESSAGE_BYTES: u32 = 64 * 1024 * 1024;

/// Writes `message` with a four-byte big-endian length prefix.
pub fn write_message<W: Write>(writer: &mut W, message: &Message) -> Result<()> {
    let payload = serde_json::to_vec(message).context("Failed to serialize supervisor message")?;
    let length = u32::try_from(payload.len()).context("Supervisor message is too large to frame")?;
    if length > MAX_MESSAGE_BYTES {
        bail!("Supervisor message of {length} bytes exceeds the framing limit");
    }
    writer.write_all(&length.to_be_bytes())?;
    writer.write_all(&payload)?;
    writer.flush()?;
    Ok(())
}

/// Reads one message, or `None` when the stream does not yet hold a whole one.
pub fn read_message<R: Read>(reader: &mut R) -> Result<Option<Message>> {
    let mut header = [0u8; 4];
    match read_exact_or_none(reader, &mut header)? {
        false => return Ok(None),
        true => {}
    }

    let length = u32::from_be_bytes(header);
    if length > MAX_MESSAGE_BYTES {
        bail!("Supervisor message header claims {length} bytes, beyond the framing limit");
    }

    let mut payload = vec![0u8; length as usize];
    if !read_exact_or_none(reader, &mut payload)? {
        return Ok(None);
    }

    let message =
        serde_json::from_slice(&payload).context("Failed to deserialize supervisor message")?;
    Ok(Some(message))
}

/// Fills `buffer`, returning `false` if the stream ended first. A short read is
/// not an error: it means the rest has not arrived yet.
fn read_exact_or_none<R: Read>(reader: &mut R, buffer: &mut [u8]) -> Result<bool> {
    let mut filled = 0;
    while filled < buffer.len() {
        match reader.read(&mut buffer[filled..]) {
            Ok(0) => return Ok(false),
            Ok(read) => filled += read,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return Ok(false),
            Err(error) => return Err(error).context("Failed to read a supervisor message"),
        }
    }
    Ok(true)
}

#[cfg(test)]
#[path = "framing_tests.rs"]
mod tests;
```

Use whatever serialization the existing `api.rs` messages already use rather than introducing a second one — check what `server/protocol.rs` calls before settling on `serde_json`, and match it.

`mod.rs`:

```rust
//! The cross-platform half of the terminal session supervisor.
//!
//! `local_tty::server` is Unix-only because it passes file descriptors. Message
//! shapes, framing, and the transport seam are not Unix-specific and live here
//! so the Windows backend can share them.

pub mod framing;
pub mod loopback;
pub mod message;
pub mod ring;
pub mod transport;

pub use message::{AttachedSession, Message, PROTOCOL_VERSION, SessionSummary};
```

Create `loopback.rs`, `ring.rs`, and `transport.rs` as empty placeholder modules in this task only if `mod.rs` needs them to compile; otherwise add each `pub mod` line in the task that creates the file.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p warp_terminal --lib -- local_tty::supervisor::framing`
Expected: both PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/warp_terminal/src/local_tty/supervisor crates/warp_terminal/src/local_tty/mod.rs crates/warp_terminal/src/local_tty/screen.rs
git commit -m "feat(terminal): transport-agnostic supervisor framing and v2 messages"
```

---

### Task 2: The bounded output ring

While a session is detached the supervisor accumulates its output. Unbounded, a runaway `yes` eats memory; unbounded-with-truncation, a pane silently misses output. The ring reports what it dropped.

**Files:**
- Create: `crates/warp_terminal/src/local_tty/supervisor/ring.rs`
- Create: `crates/warp_terminal/src/local_tty/supervisor/ring_tests.rs`

**Interfaces:**
- Produces:
  - `pub struct OutputRing`
  - `pub fn OutputRing::new(capacity_bytes: usize) -> Self`
  - `pub fn OutputRing::push(&mut self, bytes: &[u8])`
  - `pub fn OutputRing::sequence(&self) -> u64`
  - `pub fn OutputRing::read_after(&self, sequence: Option<u64>) -> (Vec<u8>, bool)` returning the bytes and whether anything was dropped

- [ ] **Step 1: Write the failing test**

```rust
use super::*;

/// The ordinary case: everything written since the client left comes back.
#[test]
fn output_after_a_sequence_is_returned_whole() {
    let mut ring = OutputRing::new(1024);
    ring.push(b"before");
    let checkpoint = ring.sequence();
    ring.push(b"after");

    let (bytes, dropped) = ring.read_after(Some(checkpoint));

    assert_eq!(bytes, b"after");
    assert!(!dropped, "nothing should have been dropped");
}

/// Overrun must be reported, not hidden. A pane that silently misses output is
/// worse than one that says it missed some: the user cannot tell the difference
/// between "the build printed nothing" and "we lost it".
#[test]
fn overrunning_the_ring_reports_dropped_output() {
    let mut ring = OutputRing::new(8);
    let checkpoint = ring.sequence();
    ring.push(b"0123456789abcdef");

    let (bytes, dropped) = ring.read_after(Some(checkpoint));

    assert!(dropped, "overrun must be reported");
    assert_eq!(
        bytes, b"89abcdef",
        "the most recent bytes are the ones worth keeping"
    );
}

/// A client with no sequence number -- a fresh attach -- gets whatever the ring
/// still holds, and is told if that is not everything.
#[test]
fn attaching_without_a_sequence_returns_the_whole_ring() {
    let mut ring = OutputRing::new(1024);
    ring.push(b"everything");

    let (bytes, dropped) = ring.read_after(None);

    assert_eq!(bytes, b"everything");
    assert!(!dropped);
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p warp_terminal --lib -- local_tty::supervisor::ring`
Expected: FAIL to compile — `OutputRing` is undefined.

- [ ] **Step 3: Implement**

```rust
//! A bounded buffer of a detached session's output.
//!
//! Sequence numbers let a reattaching client ask for "everything after what I
//! already saw", which is also what a dropped network connection needs.

use std::collections::VecDeque;

/// Output held for a detached session, oldest bytes discarded first.
pub struct OutputRing {
    buffer: VecDeque<u8>,
    capacity: usize,
    /// Total bytes ever pushed. The sequence number of the newest byte.
    sequence: u64,
    /// Sequence of the oldest byte still held.
    oldest_sequence: u64,
}

impl OutputRing {
    /// Creates a ring holding at most `capacity_bytes`.
    pub fn new(capacity_bytes: usize) -> Self {
        Self {
            buffer: VecDeque::with_capacity(capacity_bytes.min(64 * 1024)),
            capacity: capacity_bytes,
            sequence: 0,
            oldest_sequence: 0,
        }
    }

    /// Appends output, discarding the oldest bytes if that overruns capacity.
    pub fn push(&mut self, bytes: &[u8]) {
        self.buffer.extend(bytes.iter().copied());
        self.sequence += bytes.len() as u64;

        while self.buffer.len() > self.capacity {
            self.buffer.pop_front();
            self.oldest_sequence += 1;
        }
    }

    /// The sequence number of the newest byte held.
    pub fn sequence(&self) -> u64 {
        self.sequence
    }

    /// Everything after `sequence`, and whether anything in that span was
    /// already discarded.
    pub fn read_after(&self, sequence: Option<u64>) -> (Vec<u8>, bool) {
        let requested_from = sequence.unwrap_or(self.oldest_sequence);
        let dropped = requested_from < self.oldest_sequence;
        let start = requested_from.max(self.oldest_sequence) - self.oldest_sequence;
        let bytes = self
            .buffer
            .iter()
            .skip(start as usize)
            .copied()
            .collect();
        (bytes, dropped)
    }
}

#[cfg(test)]
#[path = "ring_tests.rs"]
mod tests;
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p warp_terminal --lib -- local_tty::supervisor::ring`
Expected: all three PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/warp_terminal/src/local_tty/supervisor/ring.rs crates/warp_terminal/src/local_tty/supervisor/ring_tests.rs crates/warp_terminal/src/local_tty/supervisor/mod.rs
git commit -m "feat(terminal): bounded output ring that reports what it dropped"
```

---

### Task 3: The Transport trait and a loopback that lends nothing

This is the task Spec B depends on. If a transport that cannot lend an endpoint still works end to end, a remote transport is possible; if it does not, Spec B is fiction.

**Files:**
- Create: `crates/warp_terminal/src/local_tty/supervisor/transport.rs`
- Create: `crates/warp_terminal/src/local_tty/supervisor/loopback.rs`
- Create: `crates/warp_terminal/src/local_tty/supervisor/loopback_tests.rs`

**Interfaces:**
- Produces:
  - `pub trait Transport { fn send(&mut self, message: &Message) -> Result<()>; fn receive(&mut self) -> Result<Option<Message>>; fn lend_endpoint(&mut self, session_id: SessionId) -> Option<Result<Endpoint>>; }`
  - `pub struct Endpoint` — an opaque per-platform handle; on a transport that cannot lend, `lend_endpoint` returns `None` and the caller streams instead
  - `pub struct LoopbackTransport`
  - `pub fn LoopbackTransport::pair() -> (LoopbackTransport, LoopbackTransport)`

- [ ] **Step 1: Write the failing test**

```rust
use super::*;
use crate::local_tty::screen::ScreenSnapshot;
use crate::local_tty::supervisor::message::Message;

/// The contract Spec B rests on: a transport that lends no endpoint still
/// carries every message. If this holds, an SSH or agent-ctrl transport is a
/// matter of moving bytes; if it does not, remote sessions are not reachable
/// from this design.
#[test]
fn a_transport_that_lends_no_endpoint_still_carries_messages() {
    let (mut client, mut supervisor) = LoopbackTransport::pair();

    client.send(&Message::ListSessions).expect("send should succeed");

    let received = supervisor
        .receive()
        .expect("receive should succeed")
        .expect("a message was sent");

    assert_eq!(received, Message::ListSessions);
}

/// Endpoint lending is a capability, not a requirement. A loopback transport
/// reports that it has none, and callers must handle that rather than assume a
/// descriptor is always available.
#[test]
fn loopback_reports_that_it_cannot_lend_an_endpoint() {
    let (mut client, _supervisor) = LoopbackTransport::pair();

    assert!(
        client.lend_endpoint(SessionId::new_for_test()).is_none(),
        "loopback must report no lending capability"
    );
}

/// A detach carries the screen, which is the biggest message and the one whose
/// loss is most visible. Round-tripping it over a real transport, not just
/// through framing, is what proves the two fit together.
#[test]
fn a_screen_snapshot_survives_a_loopback_round_trip() {
    let (mut client, mut supervisor) = LoopbackTransport::pair();
    let snapshot = ScreenSnapshot {
        columns: 80,
        rows: 24,
        repaint: b"\x1b[H\x1b[2Jrestored".to_vec(),
    };

    client
        .send(&Message::Detach {
            session_id: SessionId::new_for_test(),
            snapshot: snapshot.clone(),
        })
        .expect("send should succeed");

    let Some(Message::Detach { snapshot: received, .. }) =
        supervisor.receive().expect("receive should succeed")
    else {
        panic!("expected a Detach message");
    };

    assert_eq!(snapshot, received);
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p warp_terminal --lib -- local_tty::supervisor::loopback`
Expected: FAIL to compile.

- [ ] **Step 3: Implement**

`transport.rs`:

```rust
//! The seam between the supervisor protocol and how its bytes travel.
//!
//! Local transports can hand the client the PTY endpoint itself -- a file
//! descriptor on Unix, a duplicated handle on Windows -- so the app does its
//! own I/O and the supervisor stays out of the hot path. No transport that
//! leaves the machine can do that, so lending is optional and callers fall
//! back to streaming.

use anyhow::Result;
use warp_core::SessionId;

use super::message::Message;

/// A borrowed PTY endpoint. Its contents are platform-specific; callers pass it
/// back to the platform layer rather than inspecting it.
#[derive(Debug)]
pub struct Endpoint {
    /// The child process id, which the app needs regardless of platform.
    pub pid: u32,

    /// The raw descriptor or handle value, interpreted by the platform layer.
    pub raw: i64,
}

/// A bidirectional channel carrying [`Message`]s to and from the supervisor.
pub trait Transport {
    /// Sends one message.
    fn send(&mut self, message: &Message) -> Result<()>;

    /// Receives one message, or `None` if a whole one has not arrived.
    fn receive(&mut self) -> Result<Option<Message>>;

    /// Borrows the session's PTY endpoint, when this transport can.
    ///
    /// `None` means the capability does not exist here -- not that it failed.
    /// Callers must then stream output through [`Message`]s instead.
    fn lend_endpoint(&mut self, session_id: SessionId) -> Option<Result<Endpoint>>;
}
```

`loopback.rs` wires two in-memory buffers together; use `std::sync::mpsc` or a shared `Arc<Mutex<VecDeque<u8>>>` per direction and run the framing over it, so the test exercises real encode/decode rather than passing structs around.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p warp_terminal --lib -- local_tty::supervisor`
Expected: all PASS, framing and ring included.

- [ ] **Step 5: Commit**

```bash
git add crates/warp_terminal/src/local_tty/supervisor
git commit -m "feat(terminal): transport seam with a loopback that lends no endpoint"
```

---

## Self-Review

**Spec coverage.** Implements the spec's "Protocol changes from v1" section and the loopback test named under Testing. Does not implement: the supervisor's session table, the Unix or Windows transports, idle classification, or the drawer.

**Deviation from the spec.** The spec puts these under `server/`; this plan puts them under a new cross-platform `supervisor/` module, because `server` is `#[cfg(unix)]` and the Windows backend needs all of it. Same reason `screen.rs` moved in plan 1. Update the spec's component table when this lands.

**Carried risk.** `SessionId::new_for_test` and the serialization format used by `server/protocol.rs` are named from context and not verified. Task 1 Step 3 says to check both against the real source before settling. A mismatch surfaces as a compile error in the next step.

**Explicitly not verifiable here.** Nothing in this plan is Unix-only, so all of it runs on this machine. Plan 3's Unix transport is the part that cannot be, and it is sequenced last for that reason.
