# Session Holder Guard Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A restored pane never resumes an agent whose session another build's supervisor still holds; it shows who holds it and offers a one-click take-over that ends the agent and the shell there before resuming here. Pi markers carry a session id, and Pi writes a per-session event log.

**Architecture:** The saved pane snapshot records which supervisor held its session (`SessionHolder`). On restore, after the attach attempt fails in `on_shell_determined`, the terminal manager asks that holder over its pipe whether the session is still held, and if so withholds the resume command and tells the view to show a banner. "Take over" terminates the agent process (found by session id in the agent's own registry), kills the session through the holder's pipe, waits for it to end, then runs the withheld command. The Pi extension records the shell uuid Warp exports, so the registry resolves Pi without a process-tree walk, and the same extension appends an event log.

**Tech Stack:** Rust (WarpUI app crate, `warp_terminal` supervisor protocol, Diesel/SQLite), TypeScript Pi extension run by Node 24, `node --test`.

## Global Constraints

- Spec: `docs/superpowers/specs/2026-10-07-session-holder-guard-design.md`.
- The supervisor protocol (`crates/warp_terminal/src/local_tty/supervisor/message.rs`) and its transports are unchanged. `PROTOCOL_VERSION` stays 6.
- The pipe name stays per build. A holder that answers a different protocol version is treated as unreachable.
- Never start a supervisor from the guard: connect only. `WindowsTerminalSupervisorClient::request` spawns one when none answers, so the guard must not go through it.
- Snapshots without the new field restore exactly as today. Nothing here changes behaviour for same-build reopen.
- Every exhaustive `match` gets the new variants added; no `_` arms (AGENTS.md).
- Comments only for non-obvious "why". Context parameters are named `ctx` and go last.
- Run `./script/format` and `cargo clippy --workspace --all-targets --all-features --tests -- -D warnings` before each commit. On this machine the integration crate does not build; run clippy with `-p warp -p warp_terminal` if the workspace run fails on `crates/integration`.
- Commit messages follow the repo style: `feat(terminal): ...`, `fix(sessions): ...`, `docs: ...`, ending with `Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>`.
- Work in `T:\projects\dev-ops\warp-session-holder-guard` on branch `feat/session-holder-guard`. Push to `gitea` and `gitlab` after each task.

## File Structure

| File | Responsibility |
|---|---|
| `app/src/app_state.rs` | `SessionHolder` type and the new `TerminalPaneSnapshot::session_holder` field. |
| `app/src/terminal/local_tty/session_holder.rs` (new) | Talks to a holder over a transport: `verdict`, `end_session`, `current_channel`. Pure protocol, no UI. |
| `app/src/terminal/local_tty/session_holder_tests.rs` (new) | Loopback-transport tests for the above. |
| `app/src/terminal/cli_agent_sessions/process_registry.rs` | Lookup by session id and by shell uuid; terminate an agent process. |
| `app/src/terminal/cli_agent_sessions/process_registry/warp-session-registry.ts` | Pi extension: records `warpSessionUuid`, writes the event log. |
| `app/src/terminal/cli_agent_sessions/process_registry/warp-session-registry.test.mjs` (new) | Node tests for the extension. |
| `app/src/terminal/local_tty/terminal_manager.rs` | Guard in `on_shell_determined`; `take_over_held_session`; `run_withheld_restore`. |
| `app/src/terminal/terminal_manager.rs` | Trait methods with no-op defaults. |
| `app/src/terminal/writeable_pty/terminal_surface.rs` | `withhold_restore_command` surface method. |
| `app/src/terminal/view/inline_banner/session_holder.rs` (new) | The banner. |
| `app/src/terminal/view.rs`, `app/src/terminal/view/action.rs` | Banner state, render, actions. |
| `app/src/workspace/action.rs`, `app/src/workspace/view.rs` | Route banner actions to the pane's manager. |
| `app/src/pane_group/mod.rs`, `app/src/pane_group/pane/terminal_pane.rs` | Save the holder; pass it and the agent ref on restore; hand the uuid to the view. |
| `crates/persistence/migrations/2026-10-07-000001_add_session_holder_to_terminal_panes/` (new), `crates/persistence/src/schema.rs`, `crates/persistence/src/model.rs`, `app/src/persistence/sqlite.rs` | Persist the holder in SQLite. |

---

### Task 1: `SessionHolder` in the snapshot and in SQLite

**Files:**
- Modify: `app/src/app_state.rs` (after `CliAgentResumeDescriptor`, and `TerminalPaneSnapshot`)
- Modify: `app/src/app_state_tests.rs`
- Create: `crates/persistence/migrations/2026-10-07-000001_add_session_holder_to_terminal_panes/up.sql`, `down.sql`
- Modify: `crates/persistence/src/schema.rs:421-424`, `crates/persistence/src/model.rs:440` and `:621`
- Modify: `app/src/persistence/sqlite.rs:1373-1388` (write) and `:2360-2385` (read)
- Modify: `app/src/persistence/sqlite_tests.rs` (after `test_sqlite_round_trips_cli_agent_resume_descriptor`)

**Interfaces:**
- Produces: `pub struct SessionHolder { pub channel: String, pub version: String, pub pid: u32 }` (Clone, Debug, PartialEq, Serialize, Deserialize) in `app_state.rs`; `TerminalPaneSnapshot::session_holder: Option<SessionHolder>` with `#[serde(default)]`.

- [ ] **Step 1: Write the failing serde test** in `app/src/app_state_tests.rs`:

```rust
#[test]
fn a_snapshot_without_a_session_holder_still_deserializes() {
    let json = r#"{"uuid":[1,2],"persistent_session_id":7,"cwd":null,"shell_launch_data":null,
        "is_active":true,"is_read_only":false,"input_config":null,"llm_model_override":null,
        "active_profile_id":null,"conversation_ids_to_restore":[],"active_conversation_id":null}"#;
    let snapshot: TerminalPaneSnapshot = serde_json::from_str(json).unwrap();
    assert_eq!(snapshot.session_holder, None);
}

#[test]
fn a_session_holder_round_trips_through_json() {
    let holder = SessionHolder {
        channel: r"\\.\pipe\warp-terminal-supervisor-v6-abcdef0123456789".to_owned(),
        version: "20261004-173607861-268f56926594".to_owned(),
        pid: 27784,
    };
    let json = serde_json::to_string(&holder).unwrap();
    assert_eq!(serde_json::from_str::<SessionHolder>(&json).unwrap(), holder);
}
```

If `TerminalPaneSnapshot` has more required fields than the JSON above, add them with `null`/defaults until the first test compiles; the assertion is the point.

- [ ] **Step 2: Run** `cargo test -p warp app_state_tests::a_session_holder` and confirm it fails to compile (`SessionHolder` not found).

- [ ] **Step 3: Add the type and field** in `app/src/app_state.rs`, right after `CliAgentResumeDescriptor`'s `impl`:

```rust
/// The supervisor that was holding a pane's shell when the pane was saved.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionHolder {
    /// The supervisor's pipe or socket name.
    pub channel: String,
    /// The build that started that supervisor, as its instance marker records it.
    pub version: String,
    /// The instance that saved the pane.
    pub pid: u32,
}
```

and in `TerminalPaneSnapshot`, after `persistent_session_id`:

```rust
    #[serde(default)]
    pub session_holder: Option<SessionHolder>,
```

Fix every struct literal of `TerminalPaneSnapshot` the compiler reports (`pane_group/mod.rs:2312`, `pane/terminal_pane.rs:585,617,702`, `persistence/sqlite.rs:2360`, test helpers) with `session_holder: None` for now.

- [ ] **Step 4: Run** the two tests; they pass.

- [ ] **Step 5: Migration.** Create `crates/persistence/migrations/2026-10-07-000001_add_session_holder_to_terminal_panes/up.sql`:

```sql
ALTER TABLE terminal_panes ADD COLUMN session_holder TEXT;
```

and `down.sql`:

```sql
ALTER TABLE terminal_panes DROP COLUMN session_holder;
```

In `crates/persistence/src/schema.rs` add `session_holder -> Nullable<Text>,` after `cli_agent_resume_descriptor` in the `terminal_panes` table. In `crates/persistence/src/model.rs` add to both structs (the insertable one near line 440 and the queryable one near line 621), after `cli_agent_resume_descriptor`:

```rust
    /// This is serialized JSON data for a SessionHolder struct.
    pub session_holder: Option<String>,
```

- [ ] **Step 6: Write the failing SQLite test** after `test_sqlite_round_trips_cli_agent_resume_descriptor` in `app/src/persistence/sqlite_tests.rs`, copying that test's body and replacing the descriptor lines with:

```rust
    let holder = SessionHolder {
        channel: r"\\.\pipe\warp-terminal-supervisor-v6-abcdef0123456789".to_owned(),
        version: "20261004-173607861-268f56926594".to_owned(),
        pid: 27784,
    };
    terminal.session_holder = Some(holder.clone());
    terminal.persistent_session_id = Some(SessionId::from(42u64));
```

and the final assertion with:

```rust
    assert_eq!(terminal.session_holder, Some(holder));
    assert_eq!(terminal.persistent_session_id, Some(SessionId::from(42u64)));
```

Name it `test_sqlite_round_trips_session_holder`. If `SessionId::from(u64)` does not exist, use the constructor the file already uses for `persistent_session_id` (`.map(Into::into)` on a `u64` at `sqlite.rs:2362` shows `u64: Into<SessionId>` holds).

- [ ] **Step 7: Run** `cargo test -p warp sqlite_tests::test_sqlite_round_trips_session_holder`; it fails (the holder comes back `None`).

- [ ] **Step 8: Persist it.** In `app/src/persistence/sqlite.rs` at the write site (after the `cli_agent_resume_descriptor:` field, line ~1388):

```rust
                session_holder: terminal_snapshot
                    .session_holder
                    .as_ref()
                    .and_then(|holder| match serde_json::to_string(holder) {
                        Ok(serialized) => Some(serialized),
                        Err(error) => {
                            log::warn!(
                                "Dropping the session holder from a persisted terminal pane: {error}"
                            );
                            None
                        }
                    }),
```

and at the read site (after `cli_agent_resume_descriptor:` in the `TerminalPaneSnapshot` literal, line ~2385):

```rust
                        session_holder: terminal_pane
                            .session_holder
                            .and_then(|serialized| match serde_json::from_str(&serialized) {
                                Ok(holder) => Some(holder),
                                Err(error) => {
                                    log::warn!(
                                        "Ignoring an unreadable session holder for a restored terminal pane: {error}"
                                    );
                                    None
                                }
                            }),
```

- [ ] **Step 9: Run** `cargo test -p warp sqlite_tests` and `cargo test -p warp app_state_tests`; all pass.

- [ ] **Step 10: Commit**

```bash
git add app/src/app_state.rs app/src/app_state_tests.rs crates/persistence app/src/persistence app/src/pane_group
git commit -m "feat(terminal): record which supervisor holds a saved pane's session"
```

---

### Task 2: Talk to a holder: `session_holder.rs`

**Files:**
- Create: `app/src/terminal/local_tty/session_holder.rs`
- Create: `app/src/terminal/local_tty/session_holder_tests.rs`
- Modify: `app/src/terminal/local_tty/mod.rs` (add `pub(crate) mod session_holder;`)
- Modify: `app/src/remote_control/mod.rs:83` (`fn build_identity` becomes `pub(crate) fn build_identity`)
- Modify: `app/src/sessions_cli.rs:61-75` (`supervisor_channel` delegates to `session_holder::current_channel`)

**Interfaces:**
- Produces:

```rust
pub(crate) enum HolderVerdict { Unknown, HeldDetached, HeldAttached }
pub(crate) fn current_channel() -> String;              // this instance's supervisor channel
pub(crate) fn current_holder() -> SessionHolder;        // channel + build_identity() + process::id()
pub(crate) fn verdict(channel: &str, session_id: SessionId) -> HolderVerdict;        // connects, bounded
pub(crate) fn end_session(channel: &str, session_id: SessionId) -> anyhow::Result<()>; // Kill + wait
pub(crate) fn verdict_over(transport: &mut impl Transport, session_id: SessionId) -> HolderVerdict;
pub(crate) fn end_session_over(transport: &mut impl Transport, session_id: SessionId) -> anyhow::Result<()>;
```

- [ ] **Step 1: Write the failing tests** in `session_holder_tests.rs`. They drive the protocol through `LoopbackTransport::pair()` with a fake holder on a thread:

```rust
use std::thread;

use warp_core::SessionId;
use warp_terminal::local_tty::supervisor::loopback::LoopbackTransport;
use warp_terminal::local_tty::supervisor::message::{
    Message, PROTOCOL_VERSION, SessionMetadata, SessionSummary,
};
use warp_terminal::local_tty::supervisor::transport::Transport;

use super::*;

fn summary(id: u64, attached: bool) -> SessionSummary {
    SessionSummary {
        session_id: SessionId::from(id),
        metadata: SessionMetadata {
            shell: "bash".to_owned(),
            working_directory: r"T:\projects".to_owned(),
            title: String::new(),
        },
        detached_for_seconds: 3,
        attached,
    }
}

/// Answers like a supervisor: `Ping` with the given version, `ListSessions` with `sessions`,
/// `Kill` with `kill_error`, and `ChildStatus` with `running` decreasing to false after
/// `running_polls` answers.
fn fake_holder(
    mut side: LoopbackTransport,
    version: u32,
    sessions: Vec<SessionSummary>,
    kill_error: Option<String>,
    mut running_polls: usize,
) -> thread::JoinHandle<Vec<Message>> {
    thread::spawn(move || {
        let mut seen = Vec::new();
        loop {
            match side.receive() {
                Ok(Some(message)) => {
                    let reply = match &message {
                        Message::Ping => Some(Message::Pong { protocol_version: version }),
                        Message::ListSessions => Some(Message::ListSessionsResponse {
                            sessions: sessions.clone(),
                        }),
                        Message::Kill { .. } => Some(Message::KillResponse {
                            error: kill_error.clone(),
                        }),
                        Message::ChildStatus { .. } => {
                            let running = running_polls > 0;
                            running_polls = running_polls.saturating_sub(1);
                            Some(Message::ChildStatusResponse { running })
                        }
                        Message::ResponseReceived => None,
                        _ => Some(Message::KillResponse { error: Some("unexpected".into()) }),
                    };
                    seen.push(message);
                    if let Some(reply) = reply {
                        side.send(&reply).unwrap();
                    }
                }
                Ok(None) => thread::sleep(std::time::Duration::from_millis(5)),
                Err(_) => return seen,
            }
        }
    })
}

#[test]
fn a_detached_session_the_holder_lists_is_held_detached() {
    let (mut client, server) = LoopbackTransport::pair();
    let holder = fake_holder(server, PROTOCOL_VERSION, vec![summary(42, false)], None, 0);
    assert_eq!(verdict_over(&mut client, SessionId::from(42u64)), HolderVerdict::HeldDetached);
    drop(client);
    holder.join().unwrap();
}

#[test]
fn an_attached_session_is_held_attached() {
    let (mut client, server) = LoopbackTransport::pair();
    let holder = fake_holder(server, PROTOCOL_VERSION, vec![summary(42, true)], None, 0);
    assert_eq!(verdict_over(&mut client, SessionId::from(42u64)), HolderVerdict::HeldAttached);
    drop(client);
    holder.join().unwrap();
}

#[test]
fn a_session_the_holder_does_not_list_is_unknown() {
    let (mut client, server) = LoopbackTransport::pair();
    let holder = fake_holder(server, PROTOCOL_VERSION, vec![summary(1, false)], None, 0);
    assert_eq!(verdict_over(&mut client, SessionId::from(42u64)), HolderVerdict::Unknown);
    drop(client);
    holder.join().unwrap();
}

#[test]
fn a_holder_on_another_protocol_version_is_unknown_and_never_asked_for_sessions() {
    let (mut client, server) = LoopbackTransport::pair();
    let holder = fake_holder(server, PROTOCOL_VERSION + 1, vec![summary(42, false)], None, 0);
    assert_eq!(verdict_over(&mut client, SessionId::from(42u64)), HolderVerdict::Unknown);
    drop(client);
    let seen = holder.join().unwrap();
    assert!(!seen.iter().any(|m| matches!(m, Message::ListSessions)));
}

#[test]
fn an_unreachable_channel_is_unknown() {
    let channel = format!(r"\\.\pipe\warp-terminal-supervisor-test-missing-{}", std::process::id());
    assert_eq!(verdict(&channel, SessionId::from(42u64)), HolderVerdict::Unknown);
}

#[test]
fn ending_a_session_kills_it_and_waits_until_it_stops_running() {
    let (mut client, server) = LoopbackTransport::pair();
    let holder = fake_holder(server, PROTOCOL_VERSION, vec![summary(42, false)], None, 2);
    end_session_over(&mut client, SessionId::from(42u64)).unwrap();
    drop(client);
    let seen = holder.join().unwrap();
    assert!(seen.iter().any(|m| matches!(m, Message::Kill { session_id } if session_id.as_u64() == 42)));
    assert_eq!(seen.iter().filter(|m| matches!(m, Message::ChildStatus { .. })).count(), 3);
}

#[test]
fn a_kill_the_holder_refuses_is_an_error() {
    let (mut client, server) = LoopbackTransport::pair();
    let holder = fake_holder(server, PROTOCOL_VERSION, vec![], Some("no such session".into()), 0);
    let error = end_session_over(&mut client, SessionId::from(42u64)).unwrap_err();
    assert!(error.to_string().contains("no such session"));
    drop(client);
    holder.join().unwrap();
}
```

The unreachable-channel test is `#[cfg(windows)]`; on unix use a path in a `tempfile::tempdir()` that has no socket.

- [ ] **Step 2: Run** `cargo test -p warp session_holder_tests`; fails to compile.

- [ ] **Step 3: Implement** `app/src/terminal/local_tty/session_holder.rs`:

```rust
//! Asks the supervisor that last held a pane's session whether it still does, and ends the
//! session there when the user takes it over.

use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use instant::Instant;
use warp_core::SessionId;
use warp_terminal::local_tty::supervisor::message::{Message, PROTOCOL_VERSION};
use warp_terminal::local_tty::supervisor::transport::Transport;

use crate::app_state::SessionHolder;

/// How long one holder answer may take. A holder that is there answers in milliseconds; a
/// missing pipe fails at connect; this only bounds a supervisor that accepted and then stalled.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(2);
/// How long to wait for a killed session's shell to go.
const END_TIMEOUT: Duration = Duration::from_secs(5);
const POLL: Duration = Duration::from_millis(50);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HolderVerdict {
    /// Unreachable, another protocol, or the session is not there: restore as today.
    Unknown,
    /// Held with no window on it.
    HeldDetached,
    /// Held and a window is attached to it.
    HeldAttached,
}

pub(crate) fn current_channel() -> String {
    #[cfg(all(feature = "local_tty", windows))]
    {
        warp_terminal::local_tty::windows::pipe_transport::supervisor_pipe_name()
    }
    #[cfg(all(feature = "local_tty", unix))]
    {
        warp_terminal::local_tty::unix_supervisor::socket::socket_path()
            .to_string_lossy()
            .into_owned()
    }
    #[cfg(not(feature = "local_tty"))]
    {
        String::new()
    }
}

pub(crate) fn current_holder() -> SessionHolder {
    SessionHolder {
        channel: current_channel(),
        version: crate::remote_control::build_identity(),
        pid: std::process::id(),
    }
}

pub(crate) fn verdict(channel: &str, session_id: SessionId) -> HolderVerdict {
    match connect(channel) {
        Some(mut transport) => verdict_over(&mut transport, session_id),
        None => HolderVerdict::Unknown,
    }
}

pub(crate) fn end_session(channel: &str, session_id: SessionId) -> Result<()> {
    let mut transport = connect(channel)
        .with_context(|| format!("The supervisor on {channel} is not answering"))?;
    end_session_over(&mut transport, session_id)
}

pub(crate) fn verdict_over(transport: &mut impl Transport, session_id: SessionId) -> HolderVerdict {
    let Some(Message::Pong { protocol_version }) = ask(transport, Message::Ping) else {
        return HolderVerdict::Unknown;
    };
    if protocol_version != PROTOCOL_VERSION {
        return HolderVerdict::Unknown;
    }
    let Some(Message::ListSessionsResponse { sessions }) = ask(transport, Message::ListSessions)
    else {
        return HolderVerdict::Unknown;
    };
    match sessions.iter().find(|session| session.session_id == session_id) {
        Some(session) if session.attached => HolderVerdict::HeldAttached,
        Some(_) => HolderVerdict::HeldDetached,
        None => HolderVerdict::Unknown,
    }
}

pub(crate) fn end_session_over(transport: &mut impl Transport, session_id: SessionId) -> Result<()> {
    match ask(transport, Message::Kill { session_id }) {
        Some(Message::KillResponse { error: None }) => {}
        Some(Message::KillResponse { error: Some(error) }) => bail!("{error}"),
        _ => bail!("The supervisor did not answer the request to end the session"),
    }
    let deadline = Instant::now() + END_TIMEOUT;
    loop {
        match ask(transport, Message::ChildStatus { session_id }) {
            Some(Message::ChildStatusResponse { running: false }) => return Ok(()),
            Some(Message::ChildStatusResponse { running: true }) => {}
            _ => bail!("The supervisor stopped answering while the session was ending"),
        }
        if Instant::now() >= deadline {
            bail!("The session's shell did not end in time");
        }
        std::thread::sleep(POLL);
    }
}

/// One request, one answer, and the acknowledgement the named-pipe server waits for.
fn ask(transport: &mut impl Transport, message: Message) -> Option<Message> {
    transport.send(&message).ok()?;
    let deadline = Instant::now() + ANSWER_TIMEOUT;
    let answer = loop {
        match transport.receive() {
            Ok(Some(answer)) => break Some(answer),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            Ok(None) | Err(_) => break None,
        }
    };
    let _ = transport.send(&Message::ResponseReceived);
    answer
}

#[cfg(all(feature = "local_tty", windows))]
fn connect(channel: &str) -> Option<impl Transport> {
    warp_terminal::local_tty::windows::pipe_transport::PipeTransport::connect_waiting(
        channel,
        ANSWER_TIMEOUT,
    )
    .ok()
    .flatten()
}

#[cfg(all(feature = "local_tty", unix))]
fn connect(channel: &str) -> Option<impl Transport> {
    warp_terminal::local_tty::unix_supervisor::transport::UnixSocketTransport::connect(
        std::path::Path::new(channel),
    )
    .ok()
}

#[cfg(not(feature = "local_tty"))]
fn connect(_channel: &str) -> Option<warp_terminal::local_tty::supervisor::loopback::LoopbackTransport> {
    None
}

#[cfg(test)]
#[path = "session_holder_tests.rs"]
mod tests;
```

Check how `LoopbackTransport` behaves on `receive` after the peer drops (the fake holder loop returns on `Err`); if it returns `Ok(None)` forever instead, have the fake stop after it has answered a `ResponseReceived` following a `ChildStatusResponse { running: false }` or after 200 idle polls.

Make `build_identity` `pub(crate)` in `app/src/remote_control/mod.rs`, and replace the body of `sessions_cli::supervisor_channel` with `crate::terminal::local_tty::session_holder::current_channel()`.

- [ ] **Step 4: Run** `cargo test -p warp session_holder_tests`; all pass.

- [ ] **Step 5: Commit**

```bash
git add app/src/terminal/local_tty app/src/remote_control/mod.rs app/src/sessions_cli.rs
git commit -m "feat(terminal): ask a session's last supervisor whether it still holds it"
```

---

### Task 3: Find and terminate an agent by its session id

**Files:**
- Modify: `app/src/terminal/cli_agent_sessions/process_registry.rs`
- Modify: `app/src/terminal/cli_agent_sessions/process_registry_tests.rs`

**Interfaces:**
- Produces:

```rust
pub(crate) fn agent_pid_for_session(agent: CLIAgent, session_id: &str) -> Option<u32>;
pub(crate) fn agent_pid_for_session_in(agent: CLIAgent, session_id: &str, processes: &[ProcessInfo], registry_directory: &Path) -> Option<u32>;
/// Ends the agent process holding `session_id`. `Ok(false)` when none is registered.
pub(crate) fn terminate_agent_for_session(agent: CLIAgent, session_id: &str) -> Result<bool>;
```

- [ ] **Step 1: Write the failing tests**:

```rust
#[test]
fn an_agent_is_found_by_the_session_its_registry_entry_names() {
    let registry = tempfile::tempdir().unwrap();
    write_entry(registry.path(), 101, "aaaa-1111", r"T:\projects\warp");
    write_entry(registry.path(), 201, "bbbb-2222", r"T:\projects\warp");
    let processes = [
        process(100, 1, "bash.exe"),
        process(101, 100, "claude.exe"),
        process(201, 1, "claude.exe"),
    ];

    assert_eq!(
        agent_pid_for_session_in(CLIAgent::Claude, "bbbb-2222", &processes, registry.path()),
        Some(201)
    );
}

#[test]
fn a_registry_entry_whose_process_is_gone_does_not_name_an_agent() {
    let registry = tempfile::tempdir().unwrap();
    write_entry(registry.path(), 101, "aaaa-1111", r"T:\projects\warp");
    let processes = [process(100, 1, "bash.exe")];

    assert_eq!(
        agent_pid_for_session_in(CLIAgent::Claude, "aaaa-1111", &processes, registry.path()),
        None
    );
}

#[test]
fn a_registry_entry_for_another_kind_of_process_does_not_name_an_agent() {
    let registry = tempfile::tempdir().unwrap();
    write_entry(registry.path(), 101, "aaaa-1111", r"T:\projects\warp");
    let processes = [process(101, 1, "notepad.exe")];

    assert_eq!(
        agent_pid_for_session_in(CLIAgent::Claude, "aaaa-1111", &processes, registry.path()),
        None
    );
}
```

- [ ] **Step 2: Run** `cargo test -p warp process_registry_tests::an_agent_is_found`; fails to compile.

- [ ] **Step 3: Implement** in `process_registry.rs`, after `running_session_ids`:

```rust
pub(crate) fn agent_pid_for_session(agent: CLIAgent, session_id: &str) -> Option<u32> {
    let directory = registry_directory(agent)?;
    agent_pid_for_session_in(agent, session_id, &process_table(), &directory)
}

pub(crate) fn agent_pid_for_session_in(
    agent: CLIAgent,
    session_id: &str,
    processes: &[ProcessInfo],
    registry_directory: &Path,
) -> Option<u32> {
    processes
        .iter()
        .filter(|process| is_agent_process(agent, &process.name))
        .find(|process| {
            read_entry(&registry_directory.join(format!("{}.json", process.pid)))
                .is_some_and(|entry| entry.session_id == session_id)
        })
        .map(|process| process.pid)
}

pub(crate) fn terminate_agent_for_session(agent: CLIAgent, session_id: &str) -> Result<bool> {
    let Some(pid) = agent_pid_for_session(agent, session_id) else {
        return Ok(false);
    };
    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[sysinfo::Pid::from_u32(pid)]),
        true,
        ProcessRefreshKind::nothing(),
    );
    let process = system
        .process(sysinfo::Pid::from_u32(pid))
        .with_context(|| format!("The {} process {pid} is already gone", agent.display_name()))?;
    if !process.kill() {
        bail!("Could not end the {} process {pid}", agent.display_name());
    }
    Ok(true)
}
```

Add `bail` to the `anyhow` import. `sysinfo::Process::kill` sends `TerminateProcess` on Windows and `SIGKILL` on unix, which is what the spec asks for.

- [ ] **Step 4: Run** `cargo test -p warp process_registry_tests`; all pass.

- [ ] **Step 5: Commit**

```bash
git add app/src/terminal/cli_agent_sessions
git commit -m "feat(terminal): find and end the agent process registered for a session"
```

---

### Task 4: The guard in the terminal manager

**Files:**
- Modify: `app/src/terminal/terminal_manager.rs:84-86` (trait defaults)
- Modify: `app/src/terminal/writeable_pty/terminal_surface.rs:82` (surface method)
- Modify: `app/src/terminal/local_tty/terminal_manager.rs` (fields near `:170`, trait impl near `:369`, `on_shell_determined` at `:1420`)
- Modify: `app/src/terminal/local_tty/terminal_view_adaptor.rs:2047`
- Modify: `app/src/pane_group/mod.rs:1837-1845` (restore) and `app/src/pane_group/pane/terminal_pane.rs:672-704` (save)
- Modify: `app/src/pane_group/mod_tests.rs`

**Interfaces:**
- Produces, in `app/src/terminal/terminal_manager.rs` next to the trait:

```rust
/// What a restored pane was going to run before the guard found its session held elsewhere.
#[derive(Clone, Debug, PartialEq)]
pub struct WithheldRestore {
    pub command: String,
    pub holder: SessionHolder,
    pub session_id: SessionId,
    pub agent: Option<HeldAgent>,
    pub verdict: HeldVerdict,
}
#[derive(Clone, Debug, PartialEq)]
pub struct HeldAgent { pub agent: CLIAgent, pub session_id: String }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeldVerdict { Detached, Attached }
```

  Trait methods (defaults: no-op / `false`):

```rust
fn restore_session_holder(&mut self, _holder: SessionHolder, _agent: Option<HeldAgent>) {}
/// Ends the held session where it is and then runs the withheld command here.
#[must_use] fn take_over_held_session(&mut self, _ctx: &mut ModelContext<Box<dyn TerminalManager>>) -> bool { false }
/// Runs the withheld command without touching the holder.
#[must_use] fn run_withheld_restore(&mut self, _ctx: &mut ModelContext<Box<dyn TerminalManager>>) -> bool { false }
```

  Surface methods in `terminal_surface.rs` (defaults no-op):

```rust
fn withhold_restore_command(&mut self, _restore: WithheldRestore, _ctx: &mut ViewContext<Self>) {}
fn on_take_over_failed(&mut self, _error: String, _ctx: &mut ViewContext<Self>) {}
fn on_withheld_restore_resolved(&mut self, _ctx: &mut ViewContext<Self>) {}
```

- [ ] **Step 1: Write the failing manager test** in `app/src/pane_group/mod_tests.rs`, after `test_restart_clears_the_replaced_shells_post_mortem_state`:

```rust
/// The guard runs when the attach fails and a holder is recorded. With no holder answering, the
/// verdict is `Unknown` and the pane restores as it always has: the resume command is queued.
#[test]
fn test_an_unreachable_holder_does_not_withhold_the_resume_command() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let (_pane_group, manager, view) = restartable_pane(&mut app);
        app.update(|ctx| {
            manager.update(ctx, |manager, ctx| {
                manager.restore_persistent_session(SessionId::from(4242u64), ctx);
                manager.restore_cli_agent_resume_command("claude --resume abc".to_owned());
                manager.restore_session_holder(
                    SessionHolder {
                        channel: format!(
                            r"\\.\pipe\warp-terminal-supervisor-test-missing-{}",
                            std::process::id()
                        ),
                        version: "test".to_owned(),
                        pid: 1,
                    },
                    None,
                );
                manager.ensure_shell_started(ctx);
            });
        });
        app.read(|ctx| {
            let view = view.as_ref(ctx);
            assert!(!view.has_session_holder_banner_for_test());
            assert_eq!(view.pending_command_for_test().as_deref(), Some("claude --resume abc"));
        });
    });
}

#[test]
fn test_run_withheld_restore_queues_the_command_and_clears_the_banner() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let (_pane_group, manager, view) = restartable_pane(&mut app);
        let restore = WithheldRestore {
            command: "claude --resume abc".to_owned(),
            holder: SessionHolder { channel: "x".into(), version: "test".into(), pid: 1 },
            session_id: SessionId::from(4242u64),
            agent: None,
            verdict: HeldVerdict::Detached,
        };
        app.update(|ctx| {
            view.update(ctx, |view, ctx| {
                view.set_login_shell_bootstrapped_for_test();
                view.withhold_restore_command(restore.clone(), ctx);
            });
            manager.update(ctx, |manager, _| manager.set_withheld_restore_for_test(restore));
        });
        app.read(|ctx| assert!(view.as_ref(ctx).has_session_holder_banner_for_test()));
        app.update(|ctx| {
            manager.update(ctx, |manager, ctx| assert!(manager.run_withheld_restore(ctx)));
        });
        app.read(|ctx| {
            let view = view.as_ref(ctx);
            assert!(!view.has_session_holder_banner_for_test());
            assert_eq!(view.pending_command_for_test().as_deref(), Some("claude --resume abc"));
        });
    });
}
```

`pending_command_for_test` reads `self.input.read(ctx, |input, _| input.pending_command())`; if the input model exposes the pending text under another name, use that. `has_session_holder_banner_for_test` and `set_withheld_restore_for_test` are added in this task and Task 5 (`#[cfg(test)]`). On unix the pipe-name string is still a fine "unreachable" channel because `UnixSocketTransport::connect` fails on a path that does not exist.

- [ ] **Step 2: Run** `cargo test -p warp mod_tests::test_an_unreachable_holder`; fails to compile.

- [ ] **Step 3: Types and trait defaults.** Add the three types and three trait methods from **Interfaces** to `app/src/terminal/terminal_manager.rs` (import `SessionHolder` from `crate::app_state`, `CLIAgent` from `crate::terminal`). Add the three surface methods to `TerminalSurface` with empty default bodies, next to `set_pending_restore_command`.

- [ ] **Step 4: Manager fields and plumbing.** In `app/src/terminal/local_tty/terminal_manager.rs` add after `cli_agent_resume_command`:

```rust
    pub(super) session_holder: Option<SessionHolder>,
    pub(super) held_agent: Option<HeldAgent>,
    /// A resume command the guard withheld because the holder still has the session.
    pub(super) withheld_restore: Option<WithheldRestore>,
```

Initialise all three to `None` where `cli_agent_resume_command: None` is (line ~722). In the trait `impl` next to `restore_cli_agent_resume_command`:

```rust
    fn restore_session_holder(&mut self, holder: SessionHolder, agent: Option<HeldAgent>) {
        self.0.session_holder = Some(holder);
        self.0.held_agent = agent;
    }

    fn run_withheld_restore(&mut self, ctx: &mut ModelContext<Box<dyn TerminalManagerTrait>>) -> bool {
        let Some(restore) = self.0.withheld_restore.take() else {
            return false;
        };
        self.0.view.update(ctx, |view, ctx| {
            view.on_withheld_restore_resolved(ctx);
            view.set_pending_restore_command(restore.command, ctx);
        });
        true
    }

    fn take_over_held_session(&mut self, ctx: &mut ModelContext<Box<dyn TerminalManagerTrait>>) -> bool {
        let Some(restore) = self.0.withheld_restore.clone() else {
            return false;
        };
        let generation = self.0.session_generation;
        let work = {
            let restore = restore.clone();
            async move {
                if let Some(agent) = &restore.agent {
                    process_registry::terminate_agent_for_session(agent.agent, &agent.session_id)
                        .map(|_| ())?;
                }
                session_holder::end_session(&restore.holder.channel, restore.session_id)
            }
        };
        ctx.spawn(
            ctx.background_executor().spawn(work),
            move |manager: &mut Box<dyn TerminalManagerTrait>, result: anyhow::Result<()>, ctx| {
                let Some(manager) = TerminalManagerTrait::as_any_mut(manager.as_mut())
                    .downcast_mut::<Self>()
                else {
                    return;
                };
                if manager.0.session_generation != generation {
                    return;
                }
                match result {
                    Ok(()) => {
                        let _ = manager.run_withheld_restore(ctx);
                    }
                    Err(error) => manager.0.view.update(ctx, |view, ctx| {
                        view.on_take_over_failed(format!("{error:#}"), ctx);
                    }),
                }
            },
        )
        .detach();
        true
    }
```

Mirror the exact `ctx.spawn` / `background_executor` shapes used at `terminal_manager.rs:805` and `view.rs:26800` (the background executor returns a task; if `ctx.spawn` takes a future directly, wrap the blocking work in `ctx.background_executor().spawn(async move { ... })` and await it). Add `#[cfg(test)] pub(crate) fn set_withheld_restore_for_test(&mut self, restore: WithheldRestore)` on the trait with a default no-op and the real impl setting the field. Mirror the trait methods in `terminal_view_adaptor.rs` next to `restore_cli_agent_resume_command` (`restore_session_holder` stores into the same two fields on the adaptor; `run_withheld_restore` and `take_over_held_session` delegate to `TerminalManager::...` like the neighbouring methods do).

- [ ] **Step 5: The guard.** In `on_shell_determined`, replace

```rust
    if reattached {
        manager.cli_agent_resume_command = None;
    } else if let Some(command) = manager.cli_agent_resume_command.take() {
        manager.view.update(ctx, |view, ctx| {
            view.set_pending_restore_command(command, ctx);
        });
    }
```

with

```rust
    if reattached {
        manager.cli_agent_resume_command = None;
    } else if let Some(command) = manager.cli_agent_resume_command.take() {
        let held = manager
            .persistent_session_id
            .zip(manager.session_holder.as_ref())
            .filter(|(_, holder)| holder.channel != session_holder::current_channel())
            .and_then(|(session_id, holder)| {
                let verdict = match session_holder::verdict(&holder.channel, session_id) {
                    HolderVerdict::Unknown => return None,
                    HolderVerdict::HeldDetached => HeldVerdict::Detached,
                    HolderVerdict::HeldAttached => HeldVerdict::Attached,
                };
                Some(WithheldRestore {
                    command: command.clone(),
                    holder: holder.clone(),
                    session_id,
                    agent: manager.held_agent.clone(),
                    verdict,
                })
            });
        match held {
            Some(restore) => {
                log::info!(
                    "Withholding the resume command: session {} is still held by build {} (pid {})",
                    restore.session_id.as_u64(),
                    restore.holder.version,
                    restore.holder.pid
                );
                manager.withheld_restore = Some(restore.clone());
                manager.view.update(ctx, |view, ctx| view.withhold_restore_command(restore, ctx));
            }
            None => manager.view.update(ctx, |view, ctx| {
                view.set_pending_restore_command(command, ctx);
            }),
        }
    }
```

`manager.persistent_session_id` here is the id the pane was saved with; it is only overwritten a few lines later by `pty.persistent_session_id()`, so read it before that assignment (the block above already sits before it).

- [ ] **Step 6: Save and restore the holder.** In `terminal_pane.rs` after `let persistent_session_id = ...` (line ~674):

```rust
            let session_holder = persistent_session_id
                .map(|_| crate::terminal::local_tty::session_holder::current_holder());
```

and put `session_holder,` into the `TerminalPaneSnapshot` literal. In `pane_group/mod.rs` restore block (line ~1837), after `restore_cli_agent_resume_command`:

```rust
                    if let Some(holder) = terminal_snapshot.session_holder.clone() {
                        let agent = terminal_snapshot
                            .cli_agent_resume_descriptor
                            .as_ref()
                            .and_then(|descriptor| {
                                Some(HeldAgent {
                                    agent: descriptor.agent,
                                    session_id: descriptor.session_id.clone()?,
                                })
                            });
                        manager.restore_session_holder(holder, agent);
                    }
```

- [ ] **Step 7:** Add the view side the tests need so this task compiles on its own: in `view.rs` a `withheld_restore: Option<WithheldRestore>` field, `withhold_restore_command` storing it and calling `self.insert_session_holder_banner(ctx)` when `self.is_login_shell_bootstrapped` (and from the bootstrap hook at `view.rs:13885` when the field is set), `on_withheld_restore_resolved` removing the banner, `on_take_over_failed` storing the error on the banner state, plus `#[cfg(test)] pub(crate) fn has_session_holder_banner_for_test(&self) -> bool`. The banner itself is Task 5; for this step `insert_session_holder_banner` may only set the state field without a block-list entry, and `has_session_holder_banner_for_test` returns `self.inline_banners_state.session_holder_banner.is_some()`.

- [ ] **Step 8: Run** `cargo test -p warp mod_tests::test_an_unreachable_holder mod_tests::test_run_withheld_restore`; both pass. Run `cargo test -p warp` for the pane_group and terminal modules to catch broken struct literals.

- [ ] **Step 9: Commit**

```bash
git add app/src/terminal app/src/pane_group
git commit -m "feat(terminal): withhold a resume command while another supervisor holds the session"
```

---

### Task 5: The banner and its actions

**Files:**
- Create: `app/src/terminal/view/inline_banner/session_holder.rs`
- Modify: `app/src/terminal/view/inline_banner/mod.rs` (`mod session_holder; pub use session_holder::*;`)
- Modify: `app/src/terminal/view.rs` (`InlineBannerType`, `InlineBannersState`, render at `:24571`, removal at `:26962`, action dispatch at `:27378` and `:27715`, bootstrap hook `:13885`)
- Modify: `app/src/terminal/view/action.rs:206` and `:576`
- Modify: `app/src/workspace/action.rs:284` and `app/src/workspace/view.rs:27450`, `:13503`
- Modify: `app/src/pane_group/mod_tests.rs`

**Interfaces:**
- Produces: `TerminalAction::SessionHolderBanner(SessionHolderBannerAction)` with `enum SessionHolderBannerAction { TakeOver, StartFresh }`; `WorkspaceAction::TakeOverHeldSessionForTerminalView { terminal_view_id }` and `WorkspaceAction::StartFreshForTerminalView { terminal_view_id }`.

- [ ] **Step 1: Write the failing test** in `mod_tests.rs`:

```rust
#[test]
fn test_a_withheld_restore_shows_the_holder_banner_with_both_actions() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let (_pane_group, _manager, view) = restartable_pane(&mut app);
        let restore = WithheldRestore {
            command: "claude --resume abc".to_owned(),
            holder: SessionHolder {
                channel: "x".into(),
                version: "20261004-173607861-268f56926594".into(),
                pid: 27784,
            },
            session_id: SessionId::from(4242u64),
            agent: None,
            verdict: HeldVerdict::Detached,
        };
        app.update(|ctx| {
            view.update(ctx, |view, ctx| {
                view.set_login_shell_bootstrapped_for_test();
                view.withhold_restore_command(restore, ctx);
            });
        });
        app.read(|ctx| {
            let view = view.as_ref(ctx);
            let banner = view.session_holder_banner_for_test().expect("banner is shown");
            assert_eq!(banner.title(), "Still running under build 20261004-173607861-268f56926594 (pid 27784)");
            assert!(banner.offers_take_over());
            assert_eq!(view.model.lock().block_list().inline_banner_count(), 1);
        });
    });
}

#[test]
fn test_an_attached_session_offers_only_start_fresh() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let (_pane_group, _manager, view) = restartable_pane(&mut app);
        let restore = WithheldRestore {
            command: "pi --session abc".to_owned(),
            holder: SessionHolder { channel: "x".into(), version: "b".into(), pid: 7 },
            session_id: SessionId::from(1u64),
            agent: None,
            verdict: HeldVerdict::Attached,
        };
        app.update(|ctx| {
            view.update(ctx, |view, ctx| {
                view.set_login_shell_bootstrapped_for_test();
                view.withhold_restore_command(restore, ctx);
            });
        });
        app.read(|ctx| {
            let banner = view.as_ref(ctx).session_holder_banner_for_test().unwrap();
            assert_eq!(banner.title(), "Open in another window (build b, pid 7)");
            assert!(!banner.offers_take_over());
        });
    });
}
```

- [ ] **Step 2: Run** them; fail to compile.

- [ ] **Step 3: The banner file** `app/src/terminal/view/inline_banner/session_holder.rs`:

```rust
use warpui::Element;
use warpui::elements::{MouseStateHandle, Text};
use warpui::fonts::Weight;

use super::{
    InlineBannerButtonState, InlineBannerContent, InlineBannerIcon, InlineBannerStyle,
    InlineBannerTextButton, InlineBannerTextButtonFont, InlineBannerTextButtonVariant,
    render_inline_block_list_banner,
};
use crate::appearance::Appearance;
use crate::terminal::terminal_manager::{HeldVerdict, WithheldRestore};
use crate::terminal::view::{InlineBannerId, TerminalAction};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionHolderBannerAction {
    TakeOver,
    StartFresh,
}

pub struct SessionHolderBannerState {
    pub id: InlineBannerId,
    restore: WithheldRestore,
    error: Option<String>,
    take_over_mouse_state: MouseStateHandle,
    start_fresh_mouse_state: MouseStateHandle,
}

impl SessionHolderBannerState {
    pub fn new(id: InlineBannerId, restore: WithheldRestore) -> Self {
        Self {
            id,
            restore,
            error: None,
            take_over_mouse_state: Default::default(),
            start_fresh_mouse_state: Default::default(),
        }
    }

    pub fn set_error(&mut self, error: String) {
        self.error = Some(error);
    }

    pub fn title(&self) -> String {
        let holder = &self.restore.holder;
        match self.restore.verdict {
            HeldVerdict::Detached => format!(
                "Still running under build {} (pid {})",
                holder.version, holder.pid
            ),
            HeldVerdict::Attached => format!(
                "Open in another window (build {}, pid {})",
                holder.version, holder.pid
            ),
        }
    }

    pub fn offers_take_over(&self) -> bool {
        self.restore.verdict == HeldVerdict::Detached
    }
}

pub fn render_session_holder_banner(
    state: &SessionHolderBannerState,
    appearance: &Appearance,
) -> Box<dyn Element> {
    let text_color = appearance.theme().active_ui_text_color().into_solid();
    let button = |text: &str, action, handle: &MouseStateHandle, variant| InlineBannerTextButton {
        text: text.to_owned(),
        text_color,
        button_state: InlineBannerButtonState {
            on_click_event: TerminalAction::SessionHolderBanner(action),
            mouse_state_handle: handle.clone(),
        },
        font: InlineBannerTextButtonFont {
            weight: Some(Weight::Bold),
            ..Default::default()
        },
        position_id: None,
        variant,
    };
    let mut buttons = Vec::new();
    if state.offers_take_over() {
        buttons.push(button(
            "Take over",
            SessionHolderBannerAction::TakeOver,
            &state.take_over_mouse_state,
            InlineBannerTextButtonVariant::Primary,
        ));
    }
    buttons.push(button(
        "Start fresh",
        SessionHolderBannerAction::StartFresh,
        &state.start_fresh_mouse_state,
        InlineBannerTextButtonVariant::Secondary,
    ));

    let body = match (&state.error, state.restore.verdict) {
        (Some(error), _) => format!("Taking over failed: {error}"),
        (None, HeldVerdict::Detached) => format!(
            "The agent and its shell are still alive in that build's session supervisor. Take over \
             ends them there and runs `{}` here.",
            state.restore.command
        ),
        (None, HeldVerdict::Attached) => format!(
            "A window in that build is attached to this session. Start fresh runs `{}` here \
             without touching it.",
            state.restore.command
        ),
    };

    render_inline_block_list_banner(
        InlineBannerStyle::CallToAction,
        appearance,
        InlineBannerContent {
            title: state.title(),
            header_icon: Some(InlineBannerIcon {
                asset_path: "bundled/svg/warning.svg",
                aspect_ratio: 1.,
                color_override: Some(appearance.theme().foreground().into_solid()),
            }),
            content: Some(vec![Text::new(
                body,
                appearance.ui_font_family(),
                appearance.ui_font_size(),
            )]),
            buttons,
            ..Default::default()
        },
    )
}
```

- [ ] **Step 4: Wire the view.** In `view.rs`:
  - `InlineBannerType::SessionHolder` (hidden in agent view, like `ShellDetached`).
  - `InlineBannersState::session_holder_banner: Option<SessionHolderBannerState>`.
  - `insert_session_holder_banner(&mut self, restore: WithheldRestore, ctx)` copied from `insert_shell_detached_banner` (`view.rs:11069`) with the new state and type.
  - `withhold_restore_command` (from Task 4) now stores the restore and inserts the banner when bootstrapped; the bootstrap hook at `:13885` inserts it when `self.withheld_restore.take()` is `Some`.
  - `on_withheld_restore_resolved` removes the banner the way `:26962` removes the detached one.
  - `on_take_over_failed` calls `set_error` on the state and `ctx.notify()`.
  - Render block at `:24571`: insert `render_session_holder_banner(state, appearance)` keyed by `state.id`.
  - `#[cfg(test)] pub(crate) fn session_holder_banner_for_test(&self) -> Option<&SessionHolderBannerState>`; make `has_session_holder_banner_for_test` use it.
  - `TerminalAction::SessionHolderBanner(SessionHolderBannerAction)` in `action.rs` with its `Display` arm; in the dispatch `match` at `:27715`:

```rust
            SessionHolderBanner(SessionHolderBannerAction::TakeOver) => {
                ctx.dispatch_typed_action_deferred(WorkspaceAction::TakeOverHeldSessionForTerminalView {
                    terminal_view_id: self.view_id,
                });
            }
            SessionHolderBanner(SessionHolderBannerAction::StartFresh) => {
                ctx.dispatch_typed_action_deferred(WorkspaceAction::StartFreshForTerminalView {
                    terminal_view_id: self.view_id,
                });
            }
```

  and add `| SessionHolderBanner(_)` to the classification list at `:27378` beside `ReattachShell`.

- [ ] **Step 5: Wire the workspace.** In `workspace/action.rs` after `ReattachShellForTerminalView`:

```rust
    /// Ends a restored pane's session in the supervisor that still holds it, then resumes here.
    TakeOverHeldSessionForTerminalView {
        terminal_view_id: EntityId,
    },
    /// Resumes a restored pane here without touching the supervisor that still holds its session.
    StartFreshForTerminalView {
        terminal_view_id: EntityId,
    },
```

Add both to the exhaustive list at `action.rs:1177`. In `workspace/view.rs` next to `ReattachShellForTerminalView` at `:27450`:

```rust
            TakeOverHeldSessionForTerminalView { terminal_view_id } => {
                if self.focus_terminal_view_locally(*terminal_view_id, ctx) {
                    self.resolve_withheld_restore(true, ctx);
                }
            }
            StartFreshForTerminalView { terminal_view_id } => {
                if self.focus_terminal_view_locally(*terminal_view_id, ctx) {
                    self.resolve_withheld_restore(false, ctx);
                }
            }
```

and next to `reattach_focused_shell` at `:13503`:

```rust
    fn resolve_withheld_restore(&mut self, take_over: bool, ctx: &mut ViewContext<Self>) {
        let Some(manager) = self.focused_terminal_manager(ctx) else {
            return;
        };
        let handled = manager.update(ctx, |manager, ctx| {
            if take_over {
                manager.take_over_held_session(ctx)
            } else {
                manager.run_withheld_restore(ctx)
            }
        });
        if !handled {
            self.toast_stack.update(ctx, |toast_stack, ctx| {
                toast_stack.add_ephemeral_toast(
                    DismissibleToast::error("This pane has no withheld session to resume.".to_owned()),
                    ctx,
                );
            });
        }
    }
```

- [ ] **Step 6: Run** the two new tests and the Task 4 tests; all pass. Run `./script/format` and clippy.

- [ ] **Step 7: Commit**

```bash
git add app/src/terminal app/src/workspace app/src/pane_group
git commit -m "feat(terminal): offer to take over a session another build still holds"
```

---

### Task 6: Pi session ids by shell uuid

**Files:**
- Modify: `app/src/terminal/cli_agent_sessions/process_registry/warp-session-registry.ts`
- Modify: `app/src/terminal/cli_agent_sessions/process_registry.rs` (`RegistryEntry`, `resolve`, `resolve_in`, `discover`)
- Modify: `app/src/terminal/cli_agent_sessions/process_registry_tests.rs`
- Modify: `app/src/terminal/view.rs:26800` and `:26828` (pass the uuid), plus a `terminal_session_uuid_hex: Option<String>` field and `set_terminal_session_uuid`
- Modify: `app/src/pane_group/mod.rs:6342` (`create_session` hands the uuid to the view)

**Interfaces:**
- Produces: `resolve(agent, shell_pid, session_uuid_hex: Option<&str>)`, `discover(shell_pid, session_uuid_hex: Option<&str>)`, `resolve_in(agent, shell_pid, session_uuid_hex, processes, registry_directory)`. Registry entry field `warpSessionUuid` (camelCase, optional).

- [ ] **Step 1: Write the failing tests**:

```rust
fn write_entry_with_uuid(dir: &Path, pid: u32, session_id: &str, uuid: &str) {
    std::fs::write(
        dir.join(format!("{pid}.json")),
        format!(r#"{{"pid":{pid},"sessionId":"{session_id}","cwd":"T:\\p","warpSessionUuid":"{uuid}"}}"#),
    )
    .unwrap();
}

/// Pi under Git Bash: the `sh` shim's parent is already gone, so the tree from the shell never
/// reaches Node. The uuid Warp exported into the shell still identifies the pane.
#[test]
fn pi_is_found_by_the_shell_uuid_when_the_process_tree_is_broken() {
    let registry = tempfile::tempdir().unwrap();
    write_entry_with_uuid(registry.path(), 300, "pi-1", "0a0b0c");
    let processes = [
        process(100, 1, "bash.exe"),
        process(200, 999, "sh.exe"),
        process(300, 200, "node.exe"),
    ];

    let found = resolve_in(CLIAgent::Pi, 100, Some("0a0b0c"), &processes, registry.path()).unwrap();
    assert_eq!(found.session_id, "pi-1");
}

#[test]
fn an_entry_for_another_pane_uuid_is_not_this_panes_pi() {
    let registry = tempfile::tempdir().unwrap();
    write_entry_with_uuid(registry.path(), 300, "pi-1", "ffffff");
    let processes = [process(100, 1, "bash.exe"), process(300, 999, "node.exe")];

    assert!(resolve_in(CLIAgent::Pi, 100, Some("0a0b0c"), &processes, registry.path()).is_none());
}

#[test]
fn the_tree_walk_still_works_for_entries_without_a_uuid() {
    let registry = tempfile::tempdir().unwrap();
    write_entry(registry.path(), 300, "pi-1", r"T:\p");
    let processes = [
        process(100, 1, "bash.exe"),
        process(200, 100, "sh.exe"),
        process(300, 200, "node.exe"),
    ];

    let found = resolve_in(CLIAgent::Pi, 100, Some("0a0b0c"), &processes, registry.path()).unwrap();
    assert_eq!(found.session_id, "pi-1");
}
```

Update every existing `resolve_in(...)` call in the tests to pass `None` as the new third argument.

- [ ] **Step 2: Run** `cargo test -p warp process_registry_tests`; fails to compile.

- [ ] **Step 3: Registry.** In `process_registry.rs`:
  - `RegistryEntry` gets `#[serde(default)] warp_session_uuid: Option<String>`; `AgentProcessSession` is unchanged; add a private `fn read_entry_with_uuid(path) -> Option<(AgentProcessSession, Option<String>)>` and have `read_entry` call it and drop the uuid.
  - New signature for `resolve_in`:

```rust
pub(crate) fn resolve_in(
    agent: CLIAgent,
    shell_pid: u32,
    session_uuid_hex: Option<&str>,
    processes: &[ProcessInfo],
    registry_directory: &Path,
) -> Option<AgentProcessSession> {
    if let Some(uuid) = session_uuid_hex {
        let by_uuid = processes
            .iter()
            .filter(|process| is_agent_process(agent, &process.name))
            .find_map(|process| {
                let (session, entry_uuid) =
                    read_entry_with_uuid(&registry_directory.join(format!("{}.json", process.pid)))?;
                (entry_uuid.as_deref() == Some(uuid)).then_some(session)
            });
        if by_uuid.is_some() {
            return by_uuid;
        }
    }
    let names: HashMap<u32, &str> = /* unchanged */;
    descendants(shell_pid, processes)
        /* unchanged */
}
```

  - `resolve` and `discover` take `session_uuid_hex: Option<&str>` and pass it through.
  - In `view.rs` add `terminal_session_uuid_hex: Option<String>` (set by `pub(crate) fn set_terminal_session_uuid(&mut self, uuid: &[u8]) { self.terminal_session_uuid_hex = Some(hex::encode(uuid)); }`) and pass `uuid.as_deref()` into the two calls at `:26800` and `:26828` (clone the `Option<String>` into the spawned future).
  - In `create_session` (`pane_group/mod.rs`, local_tty branch, after `let terminal_view = terminal_init.surface;`):

```rust
                terminal_view.update(ctx, |view, _| view.set_terminal_session_uuid(terminal_session_uuid));
```

- [ ] **Step 4: Extension.** In `warp-session-registry.ts` `record`, add to `entry`:

```ts
      warpSessionUuid: process.env.WARP_TERMINAL_SESSION_UUID || undefined,
```

- [ ] **Step 5: Run** `cargo test -p warp process_registry_tests`; all pass. The extension's `install_pi_extension_in` test (if one compares content) still passes because it compares the embedded source to the installed file.

- [ ] **Step 6: Commit**

```bash
git add app/src/terminal app/src/pane_group
git commit -m "feat(terminal): resolve Pi's session by the shell uuid Warp exported"
```

---

### Task 7: The Pi event log

**Files:**
- Modify: `app/src/terminal/cli_agent_sessions/process_registry/warp-session-registry.ts`
- Create: `app/src/terminal/cli_agent_sessions/process_registry/warp-session-registry.test.mjs`
- Modify: `app/src/terminal/cli_agent_sessions/process_registry_tests.rs` (a test that runs `node --test`)

**Interfaces:**
- Produces: `<agent dir>/warp-sessions/logs/<session-id>.log`, one JSON object per line: `{"at": <ms>, "event": "<name>", ...fields}`.

- [ ] **Step 1: Write the failing Node test** `warp-session-registry.test.mjs`:

```js
import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, readFileSync, existsSync, mkdirSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const extension = fileURLToPath(new URL("./warp-session-registry.ts", import.meta.url));

function fakePi() {
  const handlers = new Map();
  return {
    api: { on: (event, handler) => handlers.set(event, handler) },
    fire: (event, payload, ctx) => handlers.get(event)?.(payload, ctx),
  };
}

function ctxFor(dir, id) {
  return { sessionManager: { getSessionFile: () => join(dir, `2026-10-07T00-00-00-000Z_${id}.jsonl`) } };
}

test("a turn's outcome is logged with its stop reason and error", async () => {
  const agentDir = mkdtempSync(join(tmpdir(), "pi-agent-"));
  process.env.PI_CODING_AGENT_DIR = agentDir;
  const { default: register } = await import(`${extension}?${Math.random()}`);
  const pi = fakePi();
  register(pi.api);
  await pi.fire("session_start", {}, ctxFor(agentDir, "sess-1"));
  await pi.fire(
    "message_end",
    { message: { role: "assistant", stopReason: "error", errorMessage: "WebSocket idle timeout after 300000ms",
      diagnostics: [{ type: "provider_transport_failure" }] } },
    ctxFor(agentDir, "sess-1"),
  );
  const log = readFileSync(join(agentDir, "warp-sessions", "logs", "sess-1.log"), "utf8").trim().split("\n").map(JSON.parse);
  assert.equal(log[0].event, "session_start");
  assert.equal(log[1].event, "message_end");
  assert.equal(log[1].stopReason, "error");
  assert.equal(log[1].errorMessage, "WebSocket idle timeout after 300000ms");
  assert.deepEqual(log[1].diagnostics, [{ type: "provider_transport_failure" }]);
  assert.equal(log[1].role, "assistant");
});

test("an uncaught exception is logged before the process dies", () => {
  const agentDir = mkdtempSync(join(tmpdir(), "pi-agent-"));
  const script = `
    const { default: register } = await import(${JSON.stringify(extension)});
    const handlers = new Map();
    register({ on: (e, h) => handlers.set(e, h) });
    await handlers.get("session_start")({}, { sessionManager: { getSessionFile: () => "x_sess-2.jsonl" } });
    setTimeout(() => { throw new Error("WebSocket idle timeout after 300000ms"); }, 0);
  `;
  const result = spawnSync(process.execPath, ["--input-type=module", "-e", script], {
    env: { ...process.env, PI_CODING_AGENT_DIR: agentDir },
    encoding: "utf8",
  });
  assert.notEqual(result.status, 0, "the exception must still end the process");
  const log = readFileSync(join(agentDir, "warp-sessions", "logs", "sess-2.log"), "utf8").trim().split("\n").map(JSON.parse);
  const crash = log.find((line) => line.event === "uncaught_exception");
  assert.ok(crash, "crash line present");
  assert.match(crash.message, /idle timeout/);
  assert.ok(log.some((line) => line.event === "exit" && line.code !== 0));
});

test("a log past the cap keeps only its tail", async () => {
  const agentDir = mkdtempSync(join(tmpdir(), "pi-agent-"));
  process.env.PI_CODING_AGENT_DIR = agentDir;
  const logs = join(agentDir, "warp-sessions", "logs");
  mkdirSync(logs, { recursive: true });
  writeFileSync(join(logs, "sess-3.log"), "x".repeat(5 * 1024 * 1024 + 1) + "\n");
  const { default: register } = await import(`${extension}?${Math.random()}`);
  const pi = fakePi();
  register(pi.api);
  await pi.fire("session_start", {}, ctxFor(agentDir, "sess-3"));
  const size = readFileSync(join(logs, "sess-3.log")).length;
  assert.ok(size <= 1024 * 1024 + 512, `truncated to about 1 MB, got ${size}`);
});

test("nothing is written when the session id is unknown", async () => {
  const agentDir = mkdtempSync(join(tmpdir(), "pi-agent-"));
  process.env.PI_CODING_AGENT_DIR = agentDir;
  const { default: register } = await import(`${extension}?${Math.random()}`);
  const pi = fakePi();
  register(pi.api);
  await pi.fire("session_start", {}, { sessionManager: { getSessionFile: () => "" } });
  assert.ok(!existsSync(join(agentDir, "warp-sessions", "logs")));
});
```

Node 24 strips the extension's `import type` and annotations natively. If `import` of a `.ts` path is refused, run the tests with `--experimental-strip-types`.

- [ ] **Step 2: Run** `node --test app/src/terminal/cli_agent_sessions/process_registry/warp-session-registry.test.mjs`; the log tests fail (no log directory).

- [ ] **Step 3: Implement the log** in `warp-session-registry.ts`. Keep `record`, `forget`, `sessionId` as they are and add:

```ts
const logDir = path.join(registryDir, "logs");
const LOG_CAP = 5 * 1024 * 1024;
const LOG_KEEP = 1024 * 1024;
let currentSessionId = "";

function logPath(): string {
  return path.join(logDir, `${currentSessionId}.log`);
}

function truncateIfOversized(file: string) {
  const size = fs.statSync(file).size;
  if (size <= LOG_CAP) return;
  const fd = fs.openSync(file, "r");
  const tail = Buffer.alloc(LOG_KEEP);
  fs.readSync(fd, tail, 0, LOG_KEEP, size - LOG_KEEP);
  fs.closeSync(fd);
  fs.writeFileSync(file, tail);
}

function log(event: string, fields: Record<string, unknown> = {}) {
  try {
    if (!currentSessionId) return;
    fs.mkdirSync(logDir, { recursive: true });
    const file = logPath();
    if (fs.existsSync(file)) truncateIfOversized(file);
    fs.appendFileSync(file, JSON.stringify({ at: Date.now(), event, ...fields }) + "\n");
  } catch {
    // The log is a courtesy to the next investigation; Pi must never pay for it.
  }
}

function errorFields(error: unknown): Record<string, unknown> {
  const e = error as { message?: string; stack?: string } | undefined;
  return { message: e?.message ?? String(error), stack: e?.stack };
}

function installProcessHooks() {
  for (const signal of ["SIGHUP", "SIGINT", "SIGTERM"] as const) {
    process.on(signal, () => log("signal", { signal }));
  }
  process.stdin?.on?.("end", () => log("stdin_end"));
  process.stdin?.on?.("close", () => log("stdin_close"));
  process.on("unhandledRejection", (reason) => log("unhandled_rejection", errorFields(reason)));
  process.on("uncaughtExceptionMonitor", (error) => log("uncaught_exception", errorFields(error)));
  process.on("exit", (code) => log("exit", { code }));
}

let hooksInstalled = false;
```

and in the exported function:

```ts
export default function (pi: ExtensionAPI) {
  pi.on("session_start", async (_event: any, ctx: any) => {
    currentSessionId = sessionId(ctx);
    record(ctx);
    process.once("exit", forget);
    if (!hooksInstalled) {
      hooksInstalled = true;
      installProcessHooks();
    }
    log("session_start", { file: ctx?.sessionManager?.getSessionFile?.(), cwd: process.cwd(), pid: process.pid });
  });
  pi.on("agent_start", async (_event: any, ctx: any) => {
    currentSessionId = sessionId(ctx);
    record(ctx);
    log("agent_start");
  });
  pi.on("agent_end", async () => log("agent_end"));
  pi.on("message_end", async (event: any) => {
    const message = event?.message ?? {};
    log("message_end", {
      role: message.role,
      stopReason: message.stopReason,
      errorMessage: message.errorMessage,
      diagnostics: message.diagnostics,
    });
  });
  pi.on("session_shutdown", async () => {
    log("session_shutdown");
    forget();
  });
}
```

Signal handlers on `process` change Node's default for `SIGINT`/`SIGTERM` (the process would no longer exit on them). Pi installs its own handlers for those; to be safe, register with `process.prependListener` only when `process.listenerCount(signal) > 0` at the time `session_start` fires, and otherwise skip that signal. Encode exactly that in `installProcessHooks` and note it in a one-line comment.

- [ ] **Step 4: Run** the Node tests; all pass.

- [ ] **Step 5: Run them from cargo.** In `process_registry_tests.rs`:

```rust
/// The extension's own tests run under Node; skipped where Node is not installed.
#[test]
fn the_pi_extension_passes_its_node_tests() {
    let test_file = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src/terminal/cli_agent_sessions/process_registry/warp-session-registry.test.mjs");
    let output = match std::process::Command::new("node").arg("--test").arg(&test_file).output() {
        Ok(output) => output,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        Err(error) => panic!("could not run node: {error}"),
    };
    assert!(
        output.status.success(),
        "node --test failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
```

- [ ] **Step 6: Run** `cargo test -p warp process_registry_tests::the_pi_extension_passes_its_node_tests`; passes.

- [ ] **Step 7: Commit**

```bash
git add app/src/terminal/cli_agent_sessions
git commit -m "feat(terminal): have the Pi extension log every turn outcome and every way Pi can end"
```

---

### Task 8: Spec amendment, presubmit, push

**Files:**
- Modify: `docs/superpowers/specs/2026-10-07-session-holder-guard-design.md` (Part 2)

- [ ] **Step 1:** In Part 2 replace "No shell, no resume." in both banner rows with "The shell starts as today; the resume command is withheld." and replace the sentence "The check runs off the UI thread and the pane stays empty until it answers, bounded by the same timeout the `sessions` CLI uses for a ping." with "The check runs where the attach attempt already runs, bounded by a two-second answer timeout; a missing pipe fails at connect." Add under Part 3: "`Start fresh` and `Take over` both end by queueing the withheld command into the pane's already-running shell."

- [ ] **Step 2:** `./script/format` then `cargo clippy --workspace --all-targets --all-features --tests -- -D warnings` (fall back to `-p warp -p warp_terminal -p persistence` if the integration crate blocks it). Fix everything reported.

- [ ] **Step 3:** `cargo nextest run -p warp -E 'test(session_holder) | test(process_registry) | test(sqlite_tests) | test(app_state_tests) | test(mod_tests::test_an_unreachable) | test(mod_tests::test_run_withheld) | test(mod_tests::test_a_withheld) | test(mod_tests::test_an_attached)'`; all pass.

- [ ] **Step 4: Commit and push**

```bash
git add docs
git commit -m "docs: the guard withholds the resume command inside a started shell"
git push gitea feat/session-holder-guard
git push gitlab feat/session-holder-guard
```

---

## Self-review notes

- Spec Part 1 → Task 1. Part 2 → Tasks 2, 4, 5. Part 3 → Tasks 3, 4 (order: agent first, then `Kill`, then `ChildStatus`). Part 4 → Task 6. Part 5 → Task 7. Testing section → each task's tests; the real-supervisor integration test for take-over is replaced by the loopback protocol test plus the registry termination unit, because the integration crate cannot build on this machine (see memory `integration-crate-needs-docker`); note this in the PR.
- Names used across tasks: `SessionHolder`, `HolderVerdict`, `HeldVerdict`, `HeldAgent`, `WithheldRestore`, `session_holder::{current_channel, current_holder, verdict, end_session}`, `process_registry::{agent_pid_for_session, terminate_agent_for_session, resolve_in(agent, shell_pid, session_uuid_hex, processes, dir)}`, `TerminalSurface::{withhold_restore_command, on_take_over_failed, on_withheld_restore_resolved}`, `TerminalManager::{restore_session_holder, take_over_held_session, run_withheld_restore}`, `TerminalAction::SessionHolderBanner`, `WorkspaceAction::{TakeOverHeldSessionForTerminalView, StartFreshForTerminalView}`.
