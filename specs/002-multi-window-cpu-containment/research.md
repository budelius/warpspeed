# Research: WarpSpeed Multi-Window CPU Containment

## Observed Baseline

The live reference profile showed the following steady-state shape after restoration:

- 8 visible top-level windows
- 95 tabs
- 208 terminal panes
- 500 blocks
- approximately 175% process CPU, equivalent to 1.75 logical cores
- approximately 4.1 GiB private memory
- two dominant GUI/render threads near 78% and 69% of one logical core
- a separate `SYS` profile contributing CPU independently
- repeated missing-inline-banner and resize/layout diagnostics in recent logs

Terminal supervisors consumed a small minority of total CPU. The primary issue is therefore GUI scheduling/render work, not shell execution.

## Decision 1: Use Both Per-Window Failure Isolation and Render Quiescence

**Decision**: Complete one independent GUI worker per top-level window and also suppress/coalesce visual work according to presentation state.

**Rationale**: Render throttling can reduce aggregate CPU but cannot keep healthy windows interactive if one shared main thread enters a hot loop. Process isolation contains that failure but does not reduce aggregate CPU if every worker continues rendering unnecessarily. Both controls are required.

**Alternatives considered**:

- **Throttle the existing shared GUI process only**: Rejected because one hot layout/render path can still starve input for all windows.
- **Use separate `WARPSPEED_PROFILE` values per window**: Rejected because it splits settings, login, history, recovery, and persistence ownership rather than isolating windows within one profile.
- **Rely on the operating-system compositor to skip covered windows**: Rejected because Warp still builds scenes, runs timers, and requests redraws before compositor presentation decisions.

## Decision 2: Reuse and Audit the Existing Isolation Foundation

**Decision**: Start implementation from fetched `gitea/dev@20465a914`, which contains merge `690b7d1f2` and the disabled foundation commits `240ec5d29`, `251ca225d`, and `433827255`, then close its production gaps.

**Rationale**: The foundation already defines bounded authenticated bootstrap files, coordinator/worker roles, chunked restore snapshots, sequential worker startup, readiness/heartbeat messages, crash budgets, and aggregate state plumbing. Reimplementing these primitives would add risk. The current working branch is 1 commit ahead and 18 commits behind fetched `dev` and contains unrelated uncommitted session changes, so the fetch is intentionally not merged into this worktree; implementation needs a clean integration branch.

**Alternatives considered**:

- **Implement a second coordinator design from scratch**: Rejected because it would duplicate reviewed primitives and increase protocol migration risk.
- **Enable the foundation immediately**: Rejected because it remains explicitly disabled and does not yet prove complete shared-writer ownership, cross-window routing, platform lifetime independence, or performance acceptance.

## Decision 3: Centralize Render Admission in WarpUI

**Decision**: Add one presentation-aware admission policy at WarpUI's invalidation-to-redraw boundary and delayed repaint scheduler.

**Rationale**: `App::update_windows` currently requests redraw for each invalidated window, while `manage_delayed_repaint_timers` can continuously re-arm animation work. Terminal-only throttles would miss menus, overlays, AI progress views, images, and framework animations. A central policy sees all redraw sources and can preserve a single coalesced dirty state.

**Alternatives considered**:

- **Throttle only terminal output notifications**: Rejected because the two hot threads can be driven by any view or animation.
- **Rate-limit every component separately**: Rejected because independent timers can still sum to a hot loop and are difficult to test exhaustively.
- **Drop background invalidations**: Rejected because activation must show the latest state and persistence/model work must remain correct.

## Decision 4: Model Presentation as Explicit State

**Decision**: Track focus, explicit visibility, minimization, known occlusion, surface availability, and a transition generation for each window.

**Rationale**: Focus alone is insufficient: an unfocused window may remain visible on another display, while a minimized or fully occluded window cannot benefit from animation frames. `winit` exposes occlusion events, but the current event conversion does not handle them. Platform differences require an explicit `Unknown` occlusion state and conservative fallback.

**Admission policy**:

- **Focused and presentable**: Immediate interaction and active rendering.
- **Visible, unfocused, and not known occluded**: Static windows render only on change; sustained terminal/model changes coalesce to at most 5 visual updates per second.
- **Minimized, hidden, fully occluded, or surface unavailable**: No continuous animations; retain one dirty demand and present once after exposure.
- **Unknown occlusion**: Preserve correctness and apply background coalescing when unfocused; never claim the window is hidden.
- **Urgent attention**: Use bounded notification state rather than restarting a frame loop.

**Alternatives considered**:

- **Treat every unfocused window as hidden**: Rejected because users can view several windows across monitors.
- **Use only native occlusion**: Rejected because support and reliability vary across Windows, macOS, X11, Wayland, and FreeBSD.

## Decision 5: Coalesce Visual Demand, Not Terminal Data

**Decision**: Continue consuming PTY bytes, updating terminal models, saving state, and processing agent events normally. Coalesce only scene rebuild/presentation demand.

**Rationale**: Pausing PTY consumption risks pipe backpressure, lost process responsiveness, and incorrect screen state. The user needs the latest terminal model on activation, not every intermediate rendered frame.

**Alternatives considered**:

- **Pause background PTY reads**: Rejected for correctness and process-liveness reasons.
- **Discard background output**: Rejected because session history and agent state must remain complete within existing persistence bounds.
- **Replay every skipped frame on activation**: Rejected because only the latest model state is useful and replay would recreate the CPU spike.

## Decision 6: Keep One Shared-State Writer

**Decision**: The coordinator remains the only writer for SQLite, app state, session recovery, settings, secure storage, auth refresh, telemetry upload, autoupdate state, and launch-session catalogs. Workers send bounded, revisioned mutations and one-window snapshots.

**Rationale**: Per-window GUI processes otherwise create concurrent writers and migration races. This is required by the WarpSpeed reliability constitution and existing process-isolation design.

**Alternatives considered**:

- **Let every worker use SQLite WAL writes**: Rejected because the profile contains more than SQLite and process-global app state could still be lost or overwritten.
- **Give each worker a private database and merge later**: Rejected because it creates conflict resolution, authentication, and downgrade problems without user value.

## Decision 7: Repair Orphaned Layout Items Outside Render-Time Locks

**Decision**: Detect missing transient layout elements during layout, record them in a bounded deduplicated reconciliation request, and mutate the block list only after layout/model lock scopes have ended.

**Rationale**: The current missing-banner path warns each time the stale block-list reservation is laid out. Directly removing it inside layout risks nested terminal-model locking and invalidating the tree while iterating. A two-stage repair provides one safe mutation and one redraw.

**Bounds**:

- Deduplicate missing IDs per pass.
- Carry at most 256 IDs in one reconciliation request.
- If the bound is exceeded, set one `full_transient_reconciliation` flag instead of growing the collection.
- Emit one content-free result per distinct repair batch, never one warning per frame.

**Alternatives considered**:

- **Demote the warning but leave stale state**: Rejected because the blank reserved band and relayout work remain.
- **Remove the item synchronously from layout**: Rejected due to iterator invalidation and terminal-model deadlock risk.
- **Ignore missing items on restore**: Rejected because stale state can arise after detach, restart, interrupted teardown, or future transient-element bugs.

## Decision 8: Use Fixed Aggregate Diagnostics

**Decision**: Keep 60 one-second buckets per logical window with fixed reason categories and integer counters. Expose detailed snapshots only to tests/debug tooling; ordinary logs contain lifecycle transitions and threshold summaries.

**Rationale**: Per-frame logs contributed to prior diagnostic amplification and can rotate another live process's logs. Fixed buckets bound memory and cardinality while retaining enough evidence to identify a hot window or wake source.

**Allowed fields**:

- logical worker ID and generation
- process role and PID in local diagnostics
- presentation-state category
- requested/admitted/deferred/coalesced/built/presented counts
- fixed wake-reason category
- latency histogram buckets
- recovery and reconciliation outcome categories

**Disallowed fields**:

- terminal output, commands, prompts, CWDs, paths, credentials, auth tokens
- session payloads, bootstrap capabilities, IPC payloads, or arbitrary debug strings

**Alternatives considered**:

- **Log every frame and warning**: Rejected as unbounded and itself capable of causing CPU and disk pressure.
- **Record full terminal context for diagnosis**: Rejected by Safe Observability and privacy constraints.

## Decision 9: Validate With Disposable Profiles and Fake Time

**Decision**: Use fake-clock unit tests for admission/coalescing and disposable-profile process tests for CPU, failure isolation, and session continuity. Never benchmark by modifying or killing the developer's real profile.

**Rationale**: Scheduler behavior must be deterministic at unit level, while process lifetime, GPU/event-loop, and supervisor continuity need real platform tests. The reference profile shape can be generated without storing user terminal content.

**Alternatives considered**:

- **Accept only ad hoc Task Manager observations**: Rejected because results are not repeatable or suitable for regression gates.
- **Use the live profile as the integration fixture**: Rejected because failures could destroy or duplicate active sessions.

## Decision 10: Roll Out Behind a Clean-Start Gate

**Decision**: Keep isolation off by default until deterministic and platform gates pass. Enable it only at clean startup and retain a complete legacy-readable aggregate for rollback. Once enabled for a startup, an isolation failure fails closed rather than falling back mid-run.

**Rationale**: A mid-startup fallback can create a coordinator plus a full GUI writer or duplicate session attachments. Clean-start enable/disable preserves one ownership model for the whole run.

**Alternatives considered**:

- **Automatic mid-run fallback to the legacy multi-window process**: Rejected because it violates one-writer and no-duplicate-session guarantees.
- **Irreversible migration**: Rejected because rollback and downgrade are constitutional requirements.

## Decision 11: Make the Installed-Build Restart an Explicit Acceptance Step

**Decision**: After build, installation, disposable-profile, and platform gates pass, gracefully restart the authorized normal profile and require one coordinator plus one GUI worker per restored top-level window.

**Rationale**: The reported problem occurs only at the real 8-window/95-tab/208-pane scale, so final validation must prove the installed topology and session continuity. `AGENTS.md` forbids making launch a build/install side effect, and terminal supervisors must survive GUI replacement. The user explicitly authorized this final restart on 2026-11-06.

**Safety conditions**:

- preflight records expected windows and eligible sessions;
- build/install does not launch WarpSpeed;
- shutdown is requested through the profile owner;
- terminal supervisors, shells, agents, `OpenConsole`, and the separate `SYS` profile are not terminated;
- launch uses the normal non-elevated launcher;
- failure to establish one worker per window stops restoration and retains recovery state;
- all-windows-in-one-GUI fallback is forbidden.

**Alternatives considered**:

- **Treat disposable tests as sufficient**: Rejected because they cannot prove compatibility with the existing supervised-session population and real profile scale.
- **Restart automatically from the installer**: Rejected because it violates repository policy and makes rollback/preflight impossible.
- **Force-kill the entire Warp process tree**: Rejected because it would destroy the sessions the feature is required to preserve.
