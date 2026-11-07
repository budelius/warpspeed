# Implementation Plan: WarpSpeed Multi-Window CPU Containment

**Branch**: `002-multi-window-cpu-containment` | **Date**: 2026-11-06 | **Spec**: [spec.md](spec.md)

**Input**: Feature specification from `/specs/002-multi-window-cpu-containment/spec.md`

## Summary

Contain excessive multi-window CPU use through two complementary controls: complete the existing disabled coordinator/worker foundation so each top-level WarpSpeed window has an independent GUI event loop, and add presentation-aware render admission so unfocused, minimized, occluded, and unchanged windows do not rebuild or present scenes continuously. Preserve one profile writer and direct terminal-supervisor attachments, reconcile orphaned transient layout items outside render-time model locks, and verify the result with a disposable 8-window/95-tab/208-pane benchmark plus bounded content-free diagnostics.

## Technical Context

**Language/Version**: Rust 1.92.0, edition 2024

**Primary Dependencies**: WarpUI and `warpui_core`, `winit`, `wgpu`, existing `ipc` local transport, `serde`, `serde_json`, SQLite/Diesel persistence, existing terminal-supervisor and session-recovery protocols

**Storage**: Existing SQLite app state, JSON session-recovery and launch-session checkpoints, plus bounded in-memory per-window render and diagnostic state; no new user-content store

**Testing**: Rust unit/model tests with `cargo nextest`, fake-clock WarpUI scheduling tests, disposable-profile multi-process integration tests, controlled crash/hang fixtures, and platform runtime measurements

**Target Platform**: WarpSpeed GUI on Windows, macOS, Linux, and FreeBSD; Windows is the reference performance environment

**Project Type**: Cross-platform desktop terminal application in a large Cargo workspace

**Performance Goals**: Reference idle workload at or below 25% of one logical core median and 50% p95; typed-character display latency below 50 ms p95 and 100 ms p99; background output at no more than 5 visual updates per second; activation catch-up below 250 ms

**Constraints**: Preserve live shells and agents; exactly one writable profile owner; one independent GUI failure boundary per top-level window; sequential worker readiness; bounded IPC, queues, retries, snapshots, and diagnostics; no terminal content in logs; no terminal-model lock reacquisition from layout/render code; old aggregated app state remains rollback-readable

**Scale/Scope**: Initial reference profile of 8 windows, 95 tabs, 208 terminal panes, and 500 visible/persisted blocks; design remains bounded up to the existing 128-window coordinator queue and 16 MiB per-window snapshot limits in the isolation foundation

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-checked after Phase 1 design.*

| Principle | Gate | Design response |
|---|---|---|
| Preserve User State | PASS | Workers continue attaching directly to supervisors; the coordinator persists a complete legacy-readable aggregate; rollback, crash, and interruption tests preserve windows and sessions. |
| Bound Resource Use | PASS | Render demand coalesces to one bounded record per window, background cadence is capped, worker queues and snapshots retain explicit limits, and diagnostics use fixed rolling buckets. |
| Lifecycle and Event Discipline | PASS | Presentation transitions drive repaint eligibility; hidden windows do not poll; one coordinator owns the profile; restoration waits for each worker readiness transition. |
| Testable Compatibility | PASS | Fake-clock scheduler tests, stale-layout regression fixtures, disposable multi-process tests, and cross-platform process/lifecycle checks verify observable behavior. |
| Safe Observability | PASS | Measurements contain only role, logical-window identity, counts, reason categories, durations, and outcomes; hot paths aggregate instead of warning per frame. |

**Post-design re-check**: PASS. The data model and contracts preserve all five principles. No exception requires Complexity Tracking.

## Project Structure

### Documentation (this feature)

```text
specs/002-multi-window-cpu-containment/
├── spec.md
├── plan.md
├── research.md
├── data-model.md
├── quickstart.md
├── contracts/
│   ├── diagnostics-contract.md
│   ├── render-quiescence-contract.md
│   └── window-isolation-contract.md
└── checklists/
    └── requirements.md
```

### Source Code (repository root)

```text
app/src/
├── app_services/                       # sole profile ownership and external launch routing
├── app_state.rs                        # aggregate and one-window snapshots
├── lib.rs                              # role-specific startup and restoration
├── persistence/                        # coordinator-only writer boundary
├── root_view.rs                        # routed new/close/focus window operations
├── session_recovery.rs                 # compatible aggregate recovery
├── terminal/
│   ├── block_list_element.rs           # missing transient-item detection
│   ├── model/blocks.rs                 # bounded reconciliation mutation
│   └── view.rs                         # terminal redraw and banner lifecycle
├── window_process/                     # coordinator/worker foundation and completion
└── workspace/                          # window-local state and cross-window routing

crates/warpui/src/windowing/winit/
├── event_loop/mod.rs                   # focus, resize, minimize, and occlusion transitions
└── window.rs                           # native visibility/presentation state

crates/warpui_core/src/
├── core/app.rs                         # render admission and deferred repaint ownership
├── platform/mod.rs                     # platform-neutral presentation-state contract
└── presenter.rs                        # animation and next-repaint demand

crates/integration/                     # disposable-profile process and responsiveness driver
```

**Structure Decision**: Extend existing WarpUI invalidation and platform-window abstractions rather than adding a terminal-specific frame loop. Base implementation on the fetched `gitea/dev` revision `20465a914`, which already contains merge `690b7d1f2` and the disabled window-process foundation (`240ec5d29`, `251ca225d`, `433827255`). The current worktree is intentionally not merged because it contains unrelated uncommitted session work and is 1 commit ahead/18 commits behind fetched `dev`; create or update the implementation branch from a clean integration point. Keep PTY transport in the terminal supervisor and profile writes in the coordinator.

## Design and Implementation Strategy

### Phase A - Establish a reproducible baseline

1. Add a generated disposable profile with the observed shape: 8 windows, 95 tabs, 208 panes, and representative inactive animations and terminal models.
2. Add test-only counters for invalidations, redraw requests, scene builds, presentations, delayed repaint timers, wake reasons, and input-to-first-frame latency.
3. Capture a baseline on the reference Windows machine and record the exact build, CPU normalization, sample interval, active window, graphics backend, and output activity.
4. Add a deterministic stale-inline-banner fixture that reproduces the blank reservation and repeated `Missing banner element` path.

**Gate**: The fixture reproduces unnecessary background scene work and the stale-layout warning loop without using the developer's real profile.

### Phase B - Integrate the disabled isolation foundation safely

1. Start implementation from a clean branch based on fetched `gitea/dev@20465a914`; preserve the current branch's unique commit and unrelated uncommitted session work separately rather than merging in-place.
2. Confirm merge `690b7d1f2` includes the three isolation-foundation commits, resolve any feature-branch changes onto this baseline deliberately, and keep `WindowProcessIsolation` disabled by default.
3. Re-run role, protocol, bootstrap-file, coordinator, manager, worker, persistence, startup, and feature-off compatibility tests before extending the foundation.
4. Audit the foundation against [window-isolation-contract.md](contracts/window-isolation-contract.md), especially write ownership, platform process lifetime, coordinator loss, window routing, and terminal-session attachment.

**Gate**: With isolation disabled, startup, restoration, ownership, and session behavior are unchanged. With the test override enabled, a disposable N-window profile creates one coordinator and N ready workers, not one N-window GUI process.

### Phase C - Complete coordinator and worker ownership

1. Keep the coordinator as the sole profile mutex/lock holder, external-launch endpoint, persistence writer, recovery aggregator, settings/auth/autoupdate owner, and worker registry.
2. Give each worker exactly one logical top-level workspace window and one GUI/render event loop. Reject a second real workspace window in a worker and route new-window requests to the coordinator.
3. Restore workers sequentially. A worker must authenticate, restore one bounded snapshot, create its window, and present a first frame before the next worker starts.
4. Route new, close, focus, quit, deep-link, and state-save operations through coordinator contracts. Aggregate monotonically revised one-window snapshots so one worker cannot erase another.
5. Keep live PTY streams directly between workers and terminal supervisors. Recovery attaches first and creates a replacement only after absence is proven.
6. Ensure workers have independent process lifetime on each platform. Failure to escape shared-fate process containment fails the feature closed.

**Gate**: A controlled hot loop or crash in one worker leaves all other workers responsive and produces exactly one recoverable logical window with no duplicate sessions.

### Phase D - Add presentation-aware render admission

1. Introduce a platform-neutral window presentation state containing focus, explicit visibility, minimization, known occlusion, presentability, and transition generation.
2. Handle native occlusion events where available. Combine them with focus, visibility, minimize/restore, resize, and surface availability. Treat unknown occlusion conservatively rather than pretending a window is hidden.
3. Centralize admission in WarpUI's invalidation-to-redraw path. Do not spread independent throttles across terminal views.
4. Preserve a coalesced dirty demand while a window is suppressed. Do not build a scene merely to clear invalidations. On activation or exposure, cancel obsolete timers and build the latest state once.
5. Prevent `manage_delayed_repaint_timers` from re-arming animation loops for minimized or fully occluded windows. Retain only the latest semantic state, not each missed animation frame.
6. Cap changing unfocused windows to the background cadence. Input, active resize/drag, accessibility, frame capture, and newly focused windows remain immediate.
7. Keep model updates, PTY consumption, persistence, notifications, and supervisor heartbeats independent from render admission.

**Gate**: Fake-clock tests prove static hidden windows schedule no continuous frames, background output coalesces to the contract rate, and activation produces one current frame without replaying skipped frames.

### Phase E - Remove terminal and layout amplification

1. Audit terminal-model notifications to ensure a worker invalidates only views that can affect its one window and inactive tabs do not start animation-only redraw chains.
2. Replace the render-time missing-banner warning with bounded detection. Record missing IDs in a fixed-size reconciliation request without taking or reacquiring the terminal-model lock.
3. Apply reconciliation after layout completes: remove orphaned block-list reservations, update indices once, and request one redraw. Overflow becomes one bounded full transient-item reconciliation request, not an unbounded ID list.
4. Make every banner teardown path clear both view state and block-list state. Retain regression coverage for restart, detach/reattach, restoration, and interrupted teardown.
5. Demote expected fallback diagnostics and aggregate repeated reason categories so logs cannot become a new hot path.

**Gate**: The stale fixture repairs before the next stable frame, leaves no blank band, and emits at most one content-free outcome per distinct item.

### Phase F - Add bounded observability and performance acceptance

1. Maintain fixed one-second aggregate buckets per logical window for 60 seconds, with a fixed wake-reason enum and counters for requested, admitted, deferred, coalesced, built, presented, and dropped work.
2. Record focus-to-present latency, worker readiness, crash/restart outcome, and reconciliation counts. Never record terminal text, commands, prompts, CWDs, credentials, or snapshot payloads.
3. Expose test/debug snapshots and sanitized build diagnostics. Ordinary logs report only state transitions and threshold breaches, not frames or terminal events.
4. Run the reference benchmark and controlled failure scenarios from [quickstart.md](quickstart.md). Compare aggregate profile CPU separately from any explicitly isolated `SYS` profile.

**Gate**: All success criteria have reproducible evidence and diagnostics remain bounded under a deliberately hot worker.

### Phase G - Rollout and rollback

1. Land deterministic tests and feature-off compatibility first.
2. Enable only for disposable profiles through a test/local override until one-writer instrumentation, cross-window routing, and supervisor continuity pass on all target platforms.
3. Add a discoverable setting and matching Command Palette actions only when dogfood opt-in is safe; activation takes effect on the next clean launch.
4. Promote to dogfood default only after CPU, duplicate-session, writer-violation, worker-crash, and coordinator-recovery measurements are clean.
5. Preserve a clean-start rollback to the legacy reader using the complete aggregated app state. Once isolation is enabled for a startup, an isolation failure stops restoration and reports recovery options; it never falls back mid-startup to multiple windows in one GUI worker.
6. After all build, disposable-profile, and platform gates pass, perform the user-authorized installed-build restart described below. This is an acceptance gate, not an installer side effect.

## Verification Matrix

### Focused local checks

```bash
cargo nextest run -p warpui_core -E 'test(render_admission) or test(presentation_state)'
cargo nextest run -p warp --lib -E 'test(window_process) or test(layout_reconciliation) or test(profile_ownership) or test(session_recovery)'
cargo check --workspace
./script/format
cargo clippy --workspace --all-targets --all-features --tests -- -D warnings
git diff --check
```

### Process/runtime checks

- **Windows x64**: native minimize/occlusion transitions, independent Job Object lifetime, idle CPU benchmark, worker hot loop/crash, coordinator restart, one-writer proof.
- **macOS arm64**: background coordinator, foreground workers, App Nap/occlusion behavior, Dock/menu/focus routing, worker crash and recovery.
- **Linux x64**: X11 and Wayland visibility fallbacks, process-group independence, D-Bus ownership, worker crash, and render coalescing.
- **FreeBSD**: compile and focused Unix ownership/render-policy tests; GUI runtime when a capable runner exists.

Full workspace nextest and doc tests run after focused and platform-sensitive checks pass.

## Authorized Installed-Build Restart Gate

This gate runs only after every disposable-profile check passes and uses the user's explicit authorization from 2026-11-06.

1. Build and install through `script/windows/install-warpspeed.ps1`; the installer itself MUST NOT launch WarpSpeed.
2. Record the normal profile's expected logical windows, current GUI/coordinator/worker PIDs, supervisor endpoints, eligible shell/agent session identities, installed build identity, and recoverable aggregate/checkpoint. Back up only bounded state needed for rollback.
3. Confirm the new build has isolation enabled for the next clean start and can independently spawn workers. Abort before shutdown if this cannot be proven.
4. Request a graceful profile GUI shutdown through the owner/coordinator. Do not terminate terminal supervisors, shells, agents, `OpenConsole`, or the separate `SYS` profile. Escalation may target only a proven-stuck GUI owner/worker and requires preserving the preflight recovery record.
5. Launch the installed build through the normal non-elevated WarpSpeed launcher so it uses the normal profile rather than `WarpSpeedAdmin`.
6. Wait for sequential restoration and verify exactly one normal-profile coordinator plus one GUI worker PID for every expected top-level window. Each worker must own exactly one visible workspace window; no worker may host all restored windows.
7. Verify every preflight-counted eligible terminal and agent session stayed alive and attached exactly once. Compare logical window order, titles, tab/pane counts, launch sources, and active window with the preflight record.
8. Sample aggregate idle CPU and input latency against SC-001 and SC-002. Keep the separately owned `SYS` profile out of the normal-profile aggregate.
9. If topology, ownership, session continuity, or responsiveness fails, stop further restoration, retain supervisors and checkpoints, collect only bounded diagnostics, and use the clean-start rollback. Never accept a fallback that restores all windows into one GUI process.

## Complexity Tracking

No constitution violations require justification. Per-window processes are mandated by `AGENTS.md` and are necessary to contain a hot GUI event loop; presentation-aware render admission is still required because process isolation alone does not reduce aggregate CPU.
