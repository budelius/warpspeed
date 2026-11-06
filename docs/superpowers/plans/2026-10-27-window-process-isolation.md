# WarpSpeed per-window process isolation implementation plan

> **Status:** Feature-disabled foundation in progress. Shared-write proxies, complete cross-window routing, coordinator recovery, platform lifecycle acceptance, and process tests still block rollout. Do not weaken profile ownership or manipulate the user's running WarpSpeed processes while implementing it.

**Goal:** Run every normal WarpSpeed top-level window in its own GUI worker process so a crash, fatal render error, or hot main-thread loop in one window cannot terminate the other windows.

**Architecture:** Keep exactly one lightweight coordinator per effective AppId/profile. The coordinator owns profile identity, startup routing, writable shared state, recovery manifests, update orchestration, and the worker registry. Every visible window runs in one authenticated worker process. Workers own their GPU/window event loop and terminal views, but do not claim profile ownership or open shared profile state for writing. Live PTY streams continue to flow directly between a worker and the existing terminal supervisor. Only explicitly bounded records already destined for local persistence, such as a completed serialized block, may cross the mutation channel to the coordinator's writer.

**Tech stack:** Rust, WarpUI, the existing `ipc` crate, Windows named mutex/named pipes, Linux/FreeBSD advisory lock and D-Bus, macOS application lifecycle, existing terminal-supervisor protocol, SQLite, and JSON recovery checkpoints.

---

## Why this is not a launch-script change

Today `LaunchMode::App` is both the profile owner and the host for every window. A repeated launch is forwarded to that owner. The existing `Restart window in new process` handoff starts another full GUI writer, so WarpSpeed deliberately rejects it in `app/src/lib.rs` and `app/src/app_services/mod.rs`.

Removing that rejection or giving every window a synthetic `WARPSPEED_PROFILE` would be unsafe:

- several processes would write one profile's SQLite database, settings, recovery catalog, and session files;
- synthetic profiles would split login, settings, history, and recovery state;
- version-skewed workers could migrate shared data out from under each other;
- a worker crash could leave stale ownership or duplicate restored shells;
- on Windows, a child that remains in the coordinator's Job Object would still die with it.

The implementation therefore needs an explicit coordinator/worker process model rather than an operational convention.

---

## Non-negotiable invariants

1. **One profile owner:** Exactly one coordinator owns an effective AppId/profile and user session. The identity remains independent of executable path and version.
2. **One visible window per worker:** A worker may host exactly one non-transient top-level workspace window. Preview/drag surfaces do not count as independent windows.
3. **One shared-state writer:** Workers must not open SQLite, settings, secure-storage, app-state, session-recovery, or launch-session stores for writing. Mutations go through the coordinator.
4. **No ownership bypass:** A legacy `WARP_WINDOW_PROCESS_HANDOFF` payload is not sufficient authority to become a worker. A worker must authenticate to the live coordinator and match its profile, build identity, generation, and protocol version before profile initialization.
5. **No shared process fate:** Closing or crashing a worker affects only its window. A coordinator crash must not kill healthy workers.
6. **No duplicate sessions:** Recovery prefers attaching a known supervisor session. It must never start a replacement shell or agent while ownership of the old session is uncertain.
7. **No live terminal relay:** Raw live PTY input/output and ephemeral terminal credentials remain outside coordinator routing, logs, telemetry, and registry metadata. The persistence mutation channel may carry bounded records that the current local persistence layer already stores, but those payloads are never logged or retained in coordinator control metadata.
8. **Bounded protocols:** Every message, queue, retry loop, bootstrap payload, and recovery file has an explicit size/cardinality/time bound.
9. **Same-build pool:** A coordinator and its workers run the same exact build identity. A different installed version is forwarded as a version-mismatch request and cannot join the pool.
10. **Fail closed:** If worker isolation, peer authentication, independent process spawning, or the write proxy cannot be established, do not fall back to concurrent full GUI writers.
11. **Current non-GUI roles remain non-owners:** CLI commands, TUI, tests, crash helpers, remote roles, and terminal supervisors neither become coordinators nor initialize coordinator app services.
12. **No rollout side effects:** Building and installing do not launch WarpSpeed. Runtime tests use disposable profiles unless the user explicitly authorizes a real-profile restart.

---

## Process and data ownership

| Concern | Coordinator | Window worker | Terminal supervisor |
| --- | --- | --- | --- |
| Profile mutex/advisory lock | Owns | Never opens | Never opens |
| External launch URI/D-Bus endpoint | Owns | Never opens | Never opens |
| Worker registry and restart policy | Owns | Registers/heartbeats | None |
| SQLite migrations and writer | Owns | Read-only bootstrap or coordinator snapshot only | None |
| Aggregated `AppState` | Owns | Sends one-window snapshots | None |
| Session-recovery checkpoints | Writes | Sends one-window snapshots | None |
| Settings and secure-storage writes | Owns | Uses authenticated proxy | None |
| Auth refresh/cloud sync/autoupdate | Owns once | Uses coordinator state/proxies | None |
| Telemetry upload | Owns once | Sends bounded event envelopes | Headless policy remains unchanged |
| GPU, OS window, input, rendering | None | Owns one window | None |
| Terminal view/model | None | Owns | None |
| Live shell process and PTY byte stream | Never relays it | Attaches directly | Owns session |
| Bounded persisted block records | Writes through existing local persistence | Sends through mutation channel | None |
| Prompt diagnostics | None | Existing PID-qualified bounded file | None |
| `sessions` remote control | Aggregates/routes | Answers window-local operations | Existing supervisor channel |

The coordinator may use a headless WarpUI `AppContext` for models that require it, but it must not initialize a GPU device or create a visible window. On macOS it must be background-only. Workers initialize only the models required to render and operate their one window.

---

## Coordinator protocol

Create a versioned local protocol under `app/src/window_process/` rather than extending the untrusted one-shot handoff file as the authority boundary.

### Identity

- `ProfileGeneration`: random 256-bit value created in memory by each coordinator generation.
- `WorkerId`: stable UUID for one logical window, retained across worker restarts.
- `WorkerGeneration`: monotonic counter incremented each time that logical window is respawned.
- `BuildIdentity`: existing exact WarpSpeed version identity.
- `ProtocolVersion`: part of the endpoint name so incompatible peers fail by not connecting.

Use user-scoped OS IPC permissions. Give each spawned worker a one-shot bootstrap capability through an inherited handle or a mode-`0600` file that is deleted immediately after consumption. Never log capability values. A marker may contain PID, endpoint, build, and non-secret generation metadata only.

### Existing seams to reuse

- `app/src/window_process_handoff.rs` already serializes a `WindowSnapshot`, defers shells, transfers session leases, and has ready/release/abort semantics.
- `app/src/session_recovery.rs` already uses independently claimed checkpoints and supersedes metadata, so worker-level recovery does not need a new unsafe replacement format.
- `app/src/remote_control/` already advertises PID-qualified window endpoints and treats stale markers as ordinary crash residue.
- `crates/warp_terminal/src/local_tty/spawner.rs` and the session-holder metadata already keep terminal processes outside the GUI process.
- `app/src/app_services/windows/` and `app/src/app_services/linux/` already implement profile-scoped forwarding and fail-closed ownership.

These are mechanisms, not permission to re-enable the old full-GUI handoff. The new worker path adds coordinator authentication and removes shared writes before any WarpSpeed handoff is permitted.

### Worker-to-coordinator messages

- `RegisterWorker { worker_id, worker_generation, pid, build, capability_proof }`
- `WindowReady { first_frame, remote_control_pid }`
- `MainThreadHeartbeat { monotonic_sequence }`
- `WindowSnapshot { revision, snapshot }`
- `ProfileMutation { request_id, operation }`
- `OpenIntent { intent }`
- `RequestNewWindow { source_worker }`
- `RequestCloseWindow { final_snapshot }`
- `RequestQuitProfile`
- `FocusChanged`
- `PrepareTabTransfer` / `CommitTabTransfer` / `AbortTabTransfer`

### Coordinator-to-worker messages

- `BootstrapWindow { worker_id, worker_generation, snapshot, read_epoch }`
- `MutationResult { request_id, result }`
- `SettingsInvalidated { revision }`
- `AuthStateChanged { revision }`
- `OpenIntent { intent }`
- `PrepareShutdown { reason }`
- `RestartWindow`
- `FocusWindow`
- `TabTransferOffer`

Apply fixed message-size limits, bounded pending request counts, per-request timeouts, and exhaustive matching. State-changing requests carry idempotency keys so reconnect/retry cannot duplicate a save, close, or transfer.

---

## Implementation tasks

### Task 0 — Complete the write and process-global behavior audit

**Files:**
- Create: `docs/superpowers/specs/2026-10-27-window-process-isolation-audit.md`
- Inspect: `app/src/lib.rs`, `app/src/workspace/`, `app/src/persistence/`, settings, auth, secure storage, session stores, autoupdate, telemetry, remote control, and platform app services

- [ ] Inventory every path opened for writing under the profile and identify its current writer model/call sites.
- [ ] Inventory every use of `ctx.window_ids()`, `WorkspaceRegistry::all_workspaces`, active-window state, and process-global quit/new-window behavior that assumes all windows share an `AppContext`.
- [ ] Classify singleton models as coordinator-only, worker-only, replicated read-only, or proxied mutation clients.
- [ ] Identify every `ModelEvent` variant that can originate from a window and define whether it becomes a bounded IPC DTO or remains coordinator-local.
- [ ] Record platform lifecycle constraints, including Windows Job Objects, macOS application/menu/URL ownership, Linux/FreeBSD D-Bus and process groups, and updater behavior.
- [ ] Confirm that no required user journey lacks an ownership/routing design. Unresolved shared-write or cross-window behavior blocks implementation rather than becoming an implicit exception.

**Gate:** The audit accounts for every shared writer and process-global window assumption. Review and approve that artifact before adding a worker launch path.

### Task 1 — Add the role model, feature gate, and contract tests

**Files:**
- Modify: `crates/warp_core/src/features.rs`
- Modify: `app/src/lib.rs`
- Modify: `app/src/app_services/mod.rs`
- Create: `app/src/window_process/mod.rs`
- Create: `app/src/window_process/role.rs`
- Create: `app/src/window_process/role_tests.rs`
- Modify: `app/src/app_services/mod_tests.rs`

- [ ] Add a runtime `FeatureFlag::WindowProcessIsolation`, off by default and not in dogfood initially.
- [ ] Introduce an exhaustive `GuiProcessRole::{LegacyOwner, Coordinator, WindowWorker}`. Keep CLI, TUI, test, helper, remote, and supervisor roles outside this enum through existing launch modes.
- [ ] Parse the worker bootstrap before any settings, SQLite, secure-storage, app service, session-recovery, or GPU initialization.
- [ ] Make `participates_in_gui_profile_ownership()` true only for `LegacyOwner` when the flag is off and `Coordinator` when it is on.
- [ ] Keep the current legacy handoff rejection. A coordinator-authenticated worker path is separate and cannot be entered by setting only `WARP_WINDOW_PROCESS_HANDOFF`.
- [ ] Unit-test role selection, feature-off compatibility, malformed bootstrap rejection, helper exclusion, and exhaustive ownership policy.

**Gate:** With the flag off, process count, startup forwarding, restoration, and all current ownership tests remain unchanged.

### Task 2 — Build the coordinator IPC and profile ownership service

**Files:**
- Create: `app/src/window_process/protocol.rs`
- Create: `app/src/window_process/coordinator.rs`
- Create: `app/src/window_process/client.rs`
- Create: `app/src/window_process/address.rs`
- Create: corresponding `*_tests.rs`
- Modify: `app/src/app_services/windows/{mod.rs,single_instance_manager.rs}`
- Modify: `app/src/app_services/linux/mod.rs`
- Modify: `app/src/app_services/mac.rs`
- Modify: `app/src/remote_control/address.rs`

- [ ] Define the versioned coordinator protocol, identities, limits, and idempotency keys.
- [ ] Have the coordinator claim the existing profile-scoped Windows mutex or a profile-local Unix advisory lock before shared state opens; preserve macOS application-instance behavior on top of that lock.
- [ ] Keep the existing Windows URI pipe and Linux/FreeBSD D-Bus service on the coordinator. On macOS, make any URL/open event delivered to a worker forward immediately to the coordinator. External launches never execute intents directly in an arbitrary worker.
- [ ] Add a common profile-scoped internal IPC endpoint for coordinator/worker traffic on Windows, macOS, Linux, and FreeBSD.
- [ ] Validate same user, profile scope, build identity, coordinator generation, worker ID, and one-shot capability before accepting registration.
- [ ] Remove stale non-secret markers only after proving their process/endpoint is unreachable.
- [ ] Add simultaneous-start, occupied-endpoint, stale-marker, invalid-token, wrong-build, wrong-profile, oversized-message, and coordinator-death tests.

**Gate:** Ten simultaneous launches for one disposable profile produce one coordinator and no workers until explicitly requested. A second profile remains independent.

### Task 3 — Spawn workers independently and restore one window per process

**Files:**
- Create: `app/src/window_process/spawn.rs`
- Create: `app/src/window_process/registry.rs`
- Create: `app/src/window_process/worker.rs`
- Modify: `app/src/window_process_handoff.rs`
- Modify: `app/src/lib.rs`
- Modify: `app/src/root_view.rs`
- Modify: `app/src/app_state.rs`
- Test: new `spawn_tests.rs`, `registry_tests.rs`, and focused `app_state_tests.rs`

- [ ] Add `WorkerId` to `WindowSnapshot` with a backward-compatible default generated only by the coordinator.
- [ ] Let the coordinator load persisted/recovery state once, normalize worker IDs, and spawn exactly one worker for each real window.
- [ ] Spawn sequentially: do not launch worker `n + 1` until worker `n` authenticates, creates its only window, and reports a first frame or a bounded failure.
- [ ] Reuse the existing snapshot/deferred-shell handoff mechanics after authentication, including session-config lease transfer and agent resume descriptors.
- [ ] Ensure a worker rejects a second real workspace window. `new_window` requests go to the coordinator, which starts another worker.
- [ ] Do not create an extra blank window when saved windows exist. If no window exists, spawn one empty worker.
- [ ] Preserve bounds, fullscreen, custom title, active tab, panels, launch-config source, and focus order.
- [ ] On Windows, add a spawn primitive that requires `CREATE_BREAKAWAY_FROM_JOB`; never silently retry an isolation worker inside a kill-on-close Job Object.
- [ ] On Unix, put workers in an independent process group/session where needed so coordinator signals cannot terminate them as a group.
- [ ] On macOS, keep the coordinator background-only and workers foreground-capable without multiplying login-item or services-provider registrations.

**Gate:** A disposable profile with three restored windows shows three distinct worker PIDs and one non-window coordinator PID. Terminating the coordinator test fixture does not terminate those workers.

### Task 4 — Centralize shared writes and split initialization by role

**Files:**
- Modify: `app/src/lib.rs` (`initialize_app` split)
- Modify: `app/src/persistence/{mod.rs,sqlite.rs}`
- Modify: `app/src/workspace/global_actions.rs`
- Modify: `app/src/settings/init.rs` and preference adapters
- Modify: auth/secure-storage/telemetry/autoupdate initialization call sites found by the audit
- Create: `app/src/window_process/profile_services.rs`
- Create: `app/src/window_process/mutations.rs`
- Create: `app/src/window_process/mutations_tests.rs`

- [ ] Use Task 0's checked-in ownership audit as the exhaustive checklist for this split; update it when implementation discovers another writer.
- [ ] Split initialization into coordinator-owned profile services, worker UI services, and shared read-only services.
- [ ] Run SQLite migrations and the writer thread only in the coordinator. Workers may use a schema-epoch-validated read-only connection only where sending a bounded bootstrap snapshot is impractical.
- [ ] Replace worker `SyncSender<ModelEvent>` use with a coordinator-backed mutation client. Convert events crossing the process boundary into bounded owned DTOs; do not serialize trait objects or arbitrary debug output. A completed block may carry only the already-budgeted serialized output accepted by current local persistence.
- [ ] Make the coordinator the only writer for public/private preferences, secure storage, global app state, launch-session catalogs, recovery files, auth refresh state, cloud-sync state, telemetry upload, and autoupdate state.
- [ ] Broadcast revision-only invalidations. Workers reload read-only state or receive a bounded value snapshot; they do not race writes.
- [ ] Require an acknowledgement before reporting success for user-visible durable operations.
- [ ] Give each mutation an idempotency key and bounded reconnect queue. When the coordinator is unavailable, disable durable actions rather than pretending they succeeded.
- [ ] Add test-only write guards that panic/fail if a worker opens a protected profile path with write access.

**Gate:** Under the feature, filesystem/SQLite instrumentation observes exactly one writer PID for every protected profile resource while all workers remain interactive.

### Task 5 — Aggregate app state without one worker erasing another

**Files:**
- Modify: `app/src/app_state.rs`
- Modify: `app/src/session_recovery.rs`
- Modify: `app/src/workspace/global_actions.rs`
- Create: `app/src/window_process/snapshot_registry.rs`
- Create: `app/src/window_process/snapshot_registry_tests.rs`

- [ ] Have each worker send a monotonic one-window snapshot after the same lifecycle events that currently call `workspace:save_app`.
- [ ] Store the latest acknowledged snapshot per `WorkerId` in the coordinator.
- [ ] Assemble persisted `AppState` in stable window order and preserve the active worker/window index.
- [ ] Never implement aggregation as read-modify-write by workers. A stale worker revision cannot delete or replace another worker's window.
- [ ] Make normal worker close transactional: final snapshot or explicit deletion is committed before the worker exits.
- [ ] Move recovery checkpoint writes to the coordinator while retaining one logical checkpoint per worker/generation and the current claim/supersedes safety.
- [ ] Keep old single-process checkpoints readable. An old build must see a complete aggregated `AppState`, not coordinator-only metadata.
- [ ] Bound snapshot frequency, payload size, retained failed generations, and recovery retries.

**Gate:** Concurrent snapshot updates, worker close, worker crash, coordinator crash during replacement, and stale-generation updates never lose another window or duplicate a recovered window.

### Task 6 — Preserve terminal and agent sessions across worker failure

**Files:**
- Modify: `crates/warp_terminal/src/local_tty/spawner.rs`
- Modify: `app/src/terminal/detached_sessions.rs`
- Modify: `app/src/terminal/local_tty/session_holder.rs`
- Modify: `app/src/remote_control/{handler.rs,protocol.rs}`
- Modify: `app/src/session_configs/` lease paths
- Add focused tests beside each modified module

- [ ] Keep every PTY stream directly between its worker and the existing supervisor.
- [ ] On worker disconnect, have the supervisor retain eligible sessions and their screen state using the existing generation/lease authority.
- [ ] Include only supervisor channel, session ID, generation, pane UUID, and bounded structural metadata in coordinator snapshots.
- [ ] On restart, attach first. Spawn or resume an agent only after the supervisor proves the old session is absent and the resume descriptor remains valid.
- [ ] Preserve agent-event token routing to the correct worker. Stale worker generations cannot consume events.
- [ ] Transfer launch-config/session leases through coordinator-mediated prepare/commit/abort steps.
- [ ] Test long-running shell commands and Claude/Pi/Codex sessions in multiple workers; crashing one worker must preserve its sessions and leave all other workers attached.

**Gate:** Forced failure of one disposable worker produces no duplicate shell/agent, and every other worker's PTY continues accepting input throughout.

### Task 7 — Replace process-global window operations with coordinator routing

**Files:**
- Modify: `app/src/workspace/view.rs`
- Modify: `app/src/workspace/mod.rs` and `WorkspaceRegistry`
- Modify: `app/src/workspace/cross_window_tab_drag.rs`
- Modify: `app/src/app_menus.rs`
- Modify: `app/src/uri/`
- Modify: `app/src/sessions_cli.rs`
- Modify: `app/src/remote_control/{protocol.rs,handler.rs,client.rs}`
- Add focused unit tests and process-level integration fixtures

- [ ] Route New Window, deep links, file opens, login-item launches, and forwarded URLs through the coordinator.
- [ ] Route Close Window to only that worker. Route Quit WarpSpeed through the coordinator, which asks workers to close sequentially and waits for each durable final snapshot.
- [ ] Change `Restart window in new process` into an idempotent restart of the current logical worker; it no longer creates a second profile writer.
- [ ] Provide coordinator-backed window enumeration/focus so Window menus and active-window routing work across processes.
- [ ] Make `sessions instances` identify workers and window titles. Make read-only listing commands aggregate across workers when no PID is given; commands that mutate one pane still require an unambiguous pane/worker target.
- [ ] Make profile-wide `sessions quit` address the coordinator and retain the platform-correct close semantics.
- [ ] Implement cross-worker tab drag as a coordinator transaction: prepare source snapshot and supervisor lease, restore in destination, commit source removal only after destination acknowledgement, otherwise roll back.
- [ ] Preserve undo-close, custom titles, quake-window uniqueness, synced-input scope, and active-window behavior.

**Gate:** Existing multi-window user journeys remain available; no feature silently degrades merely because windows no longer share an `AppContext`.

### Task 8 — Worker and coordinator crash lifecycle

**Files:**
- Modify: `app/src/crash_recovery.rs`
- Create: `app/src/window_process/recovery.rs`
- Create: `app/src/window_process/recovery_tests.rs`
- Modify: `crates/command/src/{blocking.rs,windows.rs}` only if the independent spawn primitive belongs there

- [ ] Make the coordinator the sole authority for automatic worker respawn. Prevent the existing graphics crash-recovery helper and coordinator from both restoring the same window.
- [ ] Distinguish clean close, requested restart, process crash, fatal render exit, unresponsive main thread, coordinator loss, and version shutdown.
- [ ] Restart only the failed `WorkerId`, using its last acknowledged snapshot and incremented generation.
- [ ] Use bounded exponential backoff and a crash budget. After repeated failure, retain the checkpoint and surface a recover action in a healthy worker; do not create a respawn storm.
- [ ] A missed main-thread heartbeat marks a window unresponsive but does not force-kill it automatically. Other windows remain usable and can request a deliberate restart.
- [ ] Healthy workers continue in bounded degraded/read-only mode when the coordinator disappears.
- [ ] Elect/restart one coordinator under the existing profile lock, then re-register surviving workers. Never spawn replacement windows for workers that successfully re-register.
- [ ] Ensure Windows workers really escaped kill-on-coordinator-close job semantics. Treat inability to establish that guarantee as startup failure for the feature.

**Gate:** Crash one worker, crash the coordinator, and crash both in controlled disposable tests. In every case surviving windows remain open, one coordinator eventually owns the profile, and each logical window appears at most once.

### Task 9 — Observability and security hardening

**Files:**
- Modify/create tests in `app/src/window_process/`
- Modify: `app/src/build_info.rs`
- Modify: crash-reporting metadata allowlists only as needed

- [ ] Log only role, PID, worker ID, generation, protocol/build identity, lifecycle state, counts, durations, and bounded error categories.
- [ ] Never log IPC capabilities, auth tokens, persisted terminal payloads, prompts, commands, CWDs, session IDs, or snapshot payloads.
- [ ] Add content-free counters for coordinator restarts, worker crashes, recovery attempts, crash-loop suppression, stale messages, and rejected peers.
- [ ] Include process role and worker generation in local build diagnostics and sanitized crash metadata.
- [ ] Threat-test same-user unauthorized clients, stale capabilities, replayed mutations, PID reuse, symlink/reparse-point marker attacks, oversized payloads, and partial bootstrap files.
- [ ] Keep coordinator and worker queues bounded under a wedged peer.

### Task 10 — Automated and runtime verification

**Unit/model tests:**

- [ ] Put deterministic protocol/state-machine tests in separate `*_tests.rs` files.
- [ ] Use `warpui::App::test` for role-specific model initialization and one-window assertions.
- [ ] Test serialization limits, identity mismatch, idempotency, stale generations, snapshot aggregation, crash budgets, and transfer rollback without real sleeps.

**Process integration harness:**

- [ ] Extend the GUI integration harness with a disposable-profile multi-process driver rather than using the developer's profile.
- [ ] Add process-level cases for: three windows/three workers, sequential readiness, one-worker crash, one-worker hang/restart, coordinator crash/election, simultaneous external launches, graceful profile quit, version mismatch, and cross-worker tab transfer.
- [ ] Assert user-visible terminal continuity, not only process counts.
- [ ] Use a test-only controlled crash action; never force-kill production processes as part of automated acceptance.

**Local gates:**

```bash
cargo nextest run -p warp --lib -E 'test(window_process) or test(profile_ownership) or test(session_recovery)'
cargo check --workspace
./script/format
cargo clippy --workspace --all-targets --all-features --tests -- -D warnings
cargo nextest run --no-fail-fast --workspace --exclude command-signatures-v2
cargo test --doc
git diff --check
```

**Cross-platform runtime matrix:** This work is OS-sensitive but not architecture-sensitive. After local gates pass and the exact commit is remotely reachable, discover runners at execution time and use one representative per available desktop OS:

- Windows x64: mutex/pipe identity, independent Job Object behavior, worker crash, coordinator crash, atomic state writes.
- Linux x64: advisory lock/D-Bus ownership, worker process-group independence, crash/election, tab transfer.
- macOS arm64: background coordinator, foreground workers, Dock/menu/focus behavior, worker crash, coordinator recovery.
- FreeBSD: compile and focused Unix ownership tests unless a GUI runner is available.

Use the `cross-platform-cloud-verification` workflow only after local checks pass. Do not substitute cloud runners for local deterministic failures.

---

## Rollout

1. **Flag off:** Land role/protocol and test infrastructure with legacy behavior unchanged.
2. **Disposable-profile opt-in:** Enable only through an explicit local feature override. Require write-guard and multi-process acceptance evidence.
3. **Dogfood opt-in:** Add a discoverable setting and matching Command Palette enable/disable actions only after cross-window operations, coordinator recovery, and supervisor continuity pass.
4. **Dogfood default:** Promote after crash/recovery telemetry shows no duplicate windows, writer violations, or crash loops. Keep a launch-time escape hatch that returns to legacy single-process restoration on the next clean start.
5. **Preview/Stable:** Promote through the normal feature process. Do not remove legacy checkpoint compatibility until at least one full rollback window has elapsed.

A rollback disables worker spawning on the next clean launch. The coordinator must always persist a complete legacy-readable aggregated `AppState`, so the old single-process path can restore every window without interpreting worker registry files.

---

## Acceptance criteria

- With `N` restored normal windows, the profile has exactly one coordinator and `N` GUI worker PIDs; each worker owns exactly one visible workspace window.
- Crashing or hanging one worker leaves the other `N - 1` windows responsive and their terminal sessions untouched.
- Restarting the failed worker restores only its logical window and attaches existing supervisor sessions without duplicate shells or agents.
- Crashing the coordinator does not terminate workers; one replacement coordinator takes ownership and adopts them without duplicating windows.
- Repeated launches and different-version launches cannot create another coordinator or writer for the profile.
- Instrumentation observes one writer PID for every protected profile resource.
- Quit closes workers sequentially, persists one complete aggregated app state, and leaves eligible supervised terminal sessions resumable.
- No live PTY stream, auth token, or ephemeral capability enters coordinator routing, logs, telemetry, or crash reports; bounded terminal records cross only the local persistence mutation path and are never logged.
- The feature can be disabled and the current single-process implementation restores the complete profile state.

---

## Deliberate non-goals for the first release

- Running different WarpSpeed builds in one coordinator pool.
- Isolating every tab or pane into a process.
- Moving the terminal supervisor into the coordinator.
- Automatically killing a merely unresponsive worker.
- Changing the headless TUI process model.
- Using separate `WARPSPEED_PROFILE` values as fake window isolation.
