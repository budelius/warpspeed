# Tasks: Multi-Window CPU Containment

**Input**: Design documents from `/specs/002-multi-window-cpu-containment/`
**Prerequisites**: plan.md, spec.md, research.md, data-model.md, contracts/

**Tests**: Required for ownership, restore sequencing, render admission, stale-layout repair, and recovery because the specification defines explicit continuity and fail-closed gates.

## Phase 1: Setup

- [x] T001 Create clean `002-multi-window-cpu-containment` worktree from `gitea/dev@20465a914`
- [x] T002 Copy validated SpecKit artifacts into `specs/002-multi-window-cpu-containment/`
- [x] T003 Configure `.specify/feature.json` for this feature
- [x] T004 Record baseline build identity and disposable-profile benchmark commands in `specs/002-multi-window-cpu-containment/quickstart.md`

---

## Phase 2: Foundational Safety

- [x] T005 Audit coordinator/worker ownership and fail-closed invariants in `app/src/window_process/manager.rs`, `coordinator.rs`, and `worker.rs`
- [ ] T006 [P] Add protocol/role regression coverage in `app/src/window_process/protocol_tests.rs` and `role_tests.rs`
- [ ] T007 [P] Add worker bootstrap and sequential-ready regression coverage in `app/src/window_process/manager_tests.rs`
- [x] T008 Enable `FeatureFlag::WindowProcessIsolation` for WarpSpeed dogfood builds in `crates/warp_features/src/lib.rs`
- [x] T009 Add a release-build assertion covering the isolation feature selection in `crates/warp_features/src/features_tests.rs`

**Checkpoint**: WarpSpeed starts one profile coordinator and restores windows sequentially into isolated workers without a shared-process fallback.

---

## Phase 3: User Story 1 - Interactive Work Survives Background Load (P1)

**Goal**: A background/minimized window cannot continuously drive native redraw work for an interactive window.

**Independent Test**: In a disposable profile, overlap/minimize one worker window while another receives input; verify hidden worker redraw requests stop and the foreground worker remains responsive.

### Tests

- [x] T010 [P] [US1] Add occlusion-state transition tests in `crates/warpui_core/src/core/mod_tests.rs`
- [ ] T011 [P] [US1] Add winit occlusion callback wiring test coverage in `crates/warpui/src/windowing/winit/event_loop/mod_tests.rs`

### Implementation

- [x] T012 [US1] Extend platform window callbacks with presentation-state updates in `crates/warpui_core/src/windowing/mod.rs`
- [x] T013 [US1] Track per-window occlusion and suppress redraw/timer admission while occluded in `crates/warpui_core/src/core/app.rs`
- [x] T014 [US1] Route `WindowEvent::Occluded` into the core callback in `crates/warpui/src/windowing/winit/event_loop/mod.rs`
- [x] T015 [US1] Ensure an exposed window receives a catch-up redraw without replaying hidden intermediate frames in `crates/warpui_core/src/core/app.rs`

**Checkpoint**: Occluded workers keep PTY/model processing alive but stop native redraw and visual timer churn until exposed.

---

## Phase 4: User Story 2 - Restored Windows Are Isolated by Default (P1)

**Goal**: Every restored window is hosted by its own authenticated GUI worker behind exactly one writable profile coordinator.

**Independent Test**: Restore an eight-window disposable profile and verify one coordinator plus eight distinct worker PIDs, sequential first-frame readiness, and exactly-once terminal attachment.

### Tests

- [ ] T016 [P] [US2] Add eight-window sequential restore and first-frame gate coverage in `app/src/window_process/manager_tests.rs`
- [ ] T017 [P] [US2] Add duplicate worker/session attachment rejection coverage in `app/src/window_process/coordinator_tests.rs`
- [ ] T018 [P] [US2] Add one-writer persistence routing coverage in `app/src/window_process/persistence_bridge_tests.rs`

### Implementation

- [ ] T019 [US2] Harden coordinator startup diagnostics and profile/build identity checks in `app/src/window_process/coordinator.rs`
- [x] T020 [US2] Preserve exact window ordering and fail closed on worker readiness failures in `app/src/window_process/manager.rs`
- [x] T021 [US2] Prevent isolated workers from becoming shared persistence owners in `app/src/window_process/worker.rs`
- [x] T022 [US2] Add bounded coordinator/worker lifecycle diagnostics in `app/src/window_process/manager.rs`

**Checkpoint**: Isolation is the only restoration path when enabled; partial restore remains recoverable and never collapses into one GUI process.

---

## Phase 5: User Story 3 - Background Visual Work Is Coalesced (P2)

**Goal**: Background visual invalidations are coalesced while terminal data and model state remain current.

**Independent Test**: Generate rapid PTY output in an occluded worker, expose it, and verify one catch-up render presents current state without a frame burst.

### Tests

- [x] T023 [P] [US3] Add delayed repaint cancellation/coalescing tests in `crates/warpui_core/src/core/mod_tests.rs`
- [x] T024 [P] [US3] Add PTY-continuity disposable-profile procedure in `specs/002-multi-window-cpu-containment/quickstart.md`

### Implementation

- [x] T025 [US3] Cancel visual repaint timers when a window becomes occluded in `crates/warpui_core/src/core/app.rs`
- [x] T026 [US3] Coalesce invalidations into one exposure redraw and cap visible-unfocused redraw admission at 5 FPS in `crates/warpui_core/src/core/app.rs`
- [x] T027 [US3] Add bounded presentation-state transition logs in `crates/warpui/src/windowing/winit/event_loop/mod.rs`

**Checkpoint**: Hidden visual work is bounded and catch-up rendering is latest-state-only.

---

## Phase 6: User Story 4 - Stale Layout State Self-Heals (P2)

**Goal**: A missing banner/layout element is removed from the terminal block-list model once and cannot create a per-frame warning/relayout loop.

**Independent Test**: Inject an inline-banner model entry with no render element, render twice, and verify the first render reconciles it and the second emits no warning or layout work for it.

### Tests

- [x] T028 [P] [US4] Add stale inline-banner reconciliation coverage in `app/src/terminal/model/blocks_tests.rs`
- [ ] T029 [P] [US4] Add missing banner layout regression coverage in `app/src/terminal/block_list_element_tests.rs`

### Implementation

- [x] T030 [US4] Add explicit stale inline-banner discovery/removal helpers in `app/src/terminal/model/blocks.rs`
- [x] T031 [US4] Reconcile missing inline-banner metadata before viewport layout in `app/src/terminal/block_list_element.rs`
- [x] T032 [US4] Replace repeated missing-banner warnings with one bounded repair diagnostic in `app/src/terminal/block_list_element.rs`

**Checkpoint**: Banner state converges after one repair and no frame-rate log loop remains.

---

## Phase 7: Recovery, Validation, and Delivery

- [ ] T033 [P] Add worker crash/restart-budget and recoverable-checkpoint tests in `app/src/window_process/manager_tests.rs`
- [x] T034 Run focused Rust tests for `warp_features`, `warpui_core`, `warpui`, and `app`
- [x] T035 Run formatting, `git diff --check`, and changed-crate compilation gates
- [ ] T036 Run disposable-profile one-writer, eight-window restore, CPU, crash, and continuity gates from `quickstart.md`
- [ ] T037 Build and install WarpSpeed with `script/windows/install-warpspeed.ps1` without auto-launch
- [ ] T038 Capture the normal-profile preflight manifest and recovery checkpoints
- [ ] T039 Gracefully stop only the normal-profile GUI and relaunch the installed non-elevated WarpSpeed launcher
- [ ] T040 Verify one coordinator plus one worker per restored window, exactly-once session attachment, and post-restart CPU/latency targets

---

## Dependencies

- Phase 1 precedes all implementation.
- Phase 2 is the safety gate for all user stories.
- US1 and US2 may proceed in parallel after Phase 2.
- US3 depends on US1 presentation-state plumbing.
- US4 is independent of US1/US3 but must pass before the real-profile restart.
- Phase 7 depends on all selected story checkpoints.

## Parallel Opportunities

- T006/T007 and T008/T009 can proceed in parallel.
- T010/T011 can proceed in parallel before T012-T015.
- T016-T018 can proceed in parallel.
- T028/T029 can proceed in parallel before T030-T032.
- Focused crate tests can run in parallel once their source changes stabilize.
