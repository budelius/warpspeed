# Tasks: WarpSpeed Session Performance and Reliability

**Input**: Design documents from `/specs/001-warpspeed-session-performance/`

**Prerequisites**: plan.md, spec.md, research.md, data-model.md, contracts/, quickstart.md

**Tests**: Regression tests are required by the specification and constitution. Write each listed test before its implementation and confirm it fails for the intended reason.

**Organization**: Tasks are grouped by user story so each story remains independently testable.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel with other marked tasks that touch different files
- **[Story]**: Maps to the corresponding user story in `spec.md`
- Every task includes exact file paths

## Phase 1: Setup and Baseline

**Purpose**: Capture reproducible regression inputs and establish shared limits.

- [x] T001 Add generated dense-ANSI and oversized legacy-block fixture helpers in `app/src/launch_configs/session_store_tests.rs`
- [x] T002 Record checkpoint, diagnostic, and process-ownership constants with concise API documentation in `app/src/terminal/model/block.rs`, `app/src/context_chips/logging.rs`, and `app/src/app_services/mod.rs`
- [x] T003 Verify existing ignore files cover Spec Kit and generated test artifacts without changing unrelated repository policy in `.gitignore`

---

## Phase 2: Foundational Bounded Data Primitives

**Purpose**: Provide shared decoded-byte limiting and content-free measurement used by session writing, migration, and diagnostics.

- [x] T004 Add failing unit tests for byte-safe truncation, original-length accounting, and terminal reset boundaries in `app/src/terminal/model/block/serialized_block_tests.rs`
- [x] T005 Implement reusable bounded serialized-output helpers and truncation metadata in `app/src/terminal/model/block/serialized_block.rs`
- [x] T006 Add content-free checkpoint timing, size, truncation, and migration-result diagnostics using safe logging in `app/src/launch_configs/session_store.rs`

**Checkpoint**: Decoded payloads can be bounded and measured before story-specific persistence and logging work.

---

## Phase 3: User Story 1 - Restore Sessions Without Freezing (Priority: P1) 🎯 MVP

**Goal**: Bound checkpoint allocation and replay, compact new checkpoints, and safely migrate legacy files without losing layout or live-session descriptors.

**Independent Test**: Restore a generated V1 session equivalent to the observed dense ANSI structure and verify budgets, V2 output, rollback, layout recovery, and interruption safety.

### Tests for User Story 1

- [x] T007 [P] [US1] Add failing V1 integer-array and V2 base64 compatibility tests in `app/src/launch_configs/session_store_tests.rs`
- [x] T008 [P] [US1] Add failing block, pane, and window budget-priority tests in `app/src/launch_configs/session_store_tests.rs`
- [x] T009 [P] [US1] Add failing atomic migration, rollback-copy, interrupted-write, and corrupt-block recovery tests in `app/src/launch_configs/session_store_tests.rs`
- [x] T010 [P] [US1] Add failing SQLite-to-checkpoint budget regression coverage in `app/src/persistence/sqlite_tests.rs`

### Implementation for User Story 1

- [x] T011 [US1] Enforce the per-block output budget while serializing live and SQLite-restored blocks in `app/src/terminal/model/block/serialized_block.rs`
- [x] T012 [US1] Enforce pane and window decoded budgets while preserving metadata and prioritizing newer output in `app/src/launch_configs/session_store.rs`
- [x] T013 [US1] Add compatible bounded V1-array/V2-base64 checkpoint readers and compact V2 writers in `app/src/launch_configs/session_store.rs`
- [x] T014 [US1] Implement validated `.v1.bak` creation and atomic first-write migration in `app/src/launch_configs/session_store.rs`
- [x] T015 [US1] Recover window and pane structure when individual block output is invalid or over budget in `app/src/launch_configs/session_store.rs`
- [x] T016 [US1] Bound replay input before terminal parsing and preserve a visible safe truncation boundary in `app/src/terminal/model/blocks.rs`

**Checkpoint**: User Story 1 passes independently and representative checkpoints remain within the V2 contract.

---

## Phase 4: User Story 2 - Use a Responsive Prompt After Restoration (Priority: P1)

**Goal**: Eliminate per-restored-block side effects and avoid Git/process retries until meaningful context changes.

**Independent Test**: Replay 1,000 blocks without a live execution context, attach a local shell in a non-repository directory, and verify zero replay commands, zero warning storm, and one final refresh.

### Tests for User Story 2

- [x] T017 [P] [US2] Add failing tests that restored blocks do not refresh prompt state and a completed replay refreshes once in `app/src/terminal/view_tests.rs`
- [x] T018 [P] [US2] Add failing waiting-for-context and lifecycle-retry tests in `app/src/context_chips/current_prompt_tests.rs`
- [x] T019 [P] [US2] Add failing known-non-repository and remote-failure-fingerprint tests in `app/src/context_chips/current_prompt_tests.rs`
- [x] T020 [P] [US2] Add failing quiet Git fallback command assertions in `app/src/context_chips/builtins_tests.rs`

### Implementation for User Story 2

- [x] T021 [US2] Exclude `BlockType::Restored` from per-command prompt and block-filter completion effects and schedule one final refresh in `app/src/terminal/view.rs` and `app/src/terminal/model/blocks.rs`
- [x] T022 [US2] Add `WaitingForContext` handling that preserves cache and retries only on lifecycle changes in `app/src/context_chips/current_prompt.rs` and `app/src/context_chips/context_chip.rs`
- [x] T023 [US2] Track unknown, non-repository, and attached repository context from terminal view subscriptions in `app/src/terminal/view.rs` and `app/src/context_chips/current_prompt.rs`
- [x] T024 [US2] Skip Git generators for known non-repositories and suppress deterministic fallback failures by fingerprint in `app/src/context_chips/current_prompt.rs` and `app/src/context_chips/mod.rs`
- [x] T025 [US2] Make Git fallback commands silent for expected non-repository and missing-HEAD conditions in `app/src/context_chips/builtins.rs`

**Checkpoint**: User Story 2 passes independently with no shell-generator or warning amplification during restoration.

---

## Phase 5: User Story 3 - Keep Diagnostics Bounded and Useful (Priority: P2)

**Goal**: Prevent prompt diagnostics from producing NUL holes, unbounded records, unbounded queues, or unbounded files.

**Independent Test**: Exercise concurrent writer initialization, oversized streams, queue pressure, rotation, and retention while verifying readable bounded records.

### Tests for User Story 3

- [x] T026 [P] [US3] Add failing excerpt-length, original-length, and truncation-marker tests in `app/src/context_chips/logging_tests.rs`
- [x] T027 [P] [US3] Add failing file naming, rotation, retention, and concurrent-writer tests in `app/src/context_chips/logging_tests.rs`
- [x] T028 [P] [US3] Add failing bounded-queue non-blocking behavior tests in `app/src/context_chips/logging_tests.rs`

### Implementation for User Story 3

- [x] T029 [US3] Bound command, stdout, and stderr diagnostic fields and include content-free original lengths in `app/src/context_chips/logging.rs`
- [x] T030 [US3] Replace the unbounded channel with non-blocking bounded delivery and content-free dropped-record accounting in `app/src/context_chips/logging.rs`
- [x] T031 [US3] Write buffered PID-qualified diagnostic files with 10 MiB rotation and three-file retention in `app/src/context_chips/logging.rs`

**Checkpoint**: User Story 3 passes independently and prompt diagnostics remain readable and bounded under concurrency.

---

## Phase 6: User Story 4 - Run One Primary WarpSpeed Per Profile (Priority: P2)

**Goal**: Reuse platform startup forwarding so one GUI writer owns each effective profile across repeated launches and executable versions.

**Independent Test**: Launch one profile repeatedly and from two versions, then launch an explicit second profile and verify primary, helper, CLI, and supervisor roles.

### Tests for User Story 4

- [x] T032 [P] [US4] Add profile-scoped mutex and pipe identity tests in `app/src/app_services/windows/single_instance_manager_tests.rs`
- [x] T033 [P] [US4] Add profile-scoped D-Bus identity tests in `app/src/app_services/linux/mod_tests.rs`
- [x] T034 [P] [US4] Add GUI-versus-CLI/helper forwarding eligibility tests in `app/src/app_services/mod_tests.rs`

### Implementation for User Story 4

- [x] T035 [US4] Introduce shared effective-profile ownership identity and WarpSpeed GUI eligibility in `app/src/app_services/mod.rs` and `app/src/bin/warpspeed.rs`
- [x] T036 [US4] Enable startup forwarding for eligible standalone WarpSpeed launches in `app/src/lib.rs`, `app/src/app_services/windows/mod.rs`, and `app/src/app_services/linux/mod.rs`
- [x] T037 [US4] Scope Windows mutex and named-pipe names to effective profile while preserving release-channel compatibility in `app/src/app_services/windows/single_instance_manager.rs` and `app/src/app_services/windows/service_impl.rs`
- [x] T038 [US4] Scope Linux D-Bus service name and object path to effective profile while preserving release-channel compatibility in `app/src/app_services/linux/mod.rs`
- [x] T039 [US4] Include primary-version metadata in startup forwarding diagnostics and fail closed rather than creating a concurrent writer in `app/src/app_services/windows/service_impl.rs` and `app/src/app_services/linux/mod.rs`

**Checkpoint**: User Story 4 passes independently with one primary GUI per profile and isolated explicit profiles.

---

## Phase 7: Polish and Cross-Cutting Validation

**Purpose**: Validate the complete feature, migration path, operating-system behavior, and Spec Kit traceability.

- [x] T040 Run focused unit tests for serialized blocks, session store, prompt chips, diagnostics, SQLite, and app services using `cargo nextest` commands from `specs/001-warpspeed-session-performance/quickstart.md`
- [x] T041 Benchmark the representative legacy fixture and record size, duration, peak memory, warning count, and subprocess count in `specs/001-warpspeed-session-performance/quickstart.md`
- [x] T042 Run `./script/format`, workspace Clippy with `-D warnings`, workspace nextest, doc tests, and `git diff --check`
- [x] T043 Perform the minimal Windows, Linux, and macOS verification matrix defined in `specs/001-warpspeed-session-performance/quickstart.md` after local gates pass
- [x] T044 Re-run Spec Kit convergence against `spec.md`, `plan.md`, and `tasks.md`, append any remaining gaps, and complete them before rollout
- [ ] T045 Back up the real WarpSpeed profile, gracefully retire old GUI generations without stopping supervisors, migrate copied checkpoints, install the merged build, and validate restart/reconnect behavior per `specs/001-warpspeed-session-performance/quickstart.md`

---

## Phase 8: Convergence

**Purpose**: Close implementation gaps found by the post-implementation Spec Kit review.

- [x] T046 Bound checkpoint input size and retained pane/block-list cardinality before restoration allocation per Constitution II and FR-001 (partial)
- [x] T047 Prevent controlled window-process handoffs from bypassing WarpSpeed profile ownership in `app/src/lib.rs`, `app/src/window_process_handoff.rs`, and `app/src/workspace/view.rs` per FR-015 (contradicts)
- [x] T048 Preserve isolation for non-empty `WARPSPEED_PROFILE` values made entirely of unsupported characters in `app/src/bin/warpspeed.rs` per FR-017 (partial)
- [x] T049 Degrade malformed legacy output arrays without losing block metadata and cancel incomplete terminal control sequences at truncation boundaries in `app/src/terminal/model/block/serialized_block.rs` per FR-003 and FR-006 (partial)

---

## Dependencies & Execution Order

### Phase Dependencies

- **Phase 1** starts immediately.
- **Phase 2** depends on Phase 1 and blocks all user stories.
- **US1** and **US2** are both P1; US2 can proceed after Phase 2 while US1 persistence work proceeds in different files, except terminal replay integration must be coordinated.
- **US3** can proceed after Phase 2 independently of US1 and US2.
- **US4** can proceed after Phase 1 independently, but rollout waits for migration safeguards from US1.
- **Phase 7** depends on all selected stories.

### User Story Dependencies

- **US1**: Depends only on bounded primitives; delivers the storage/restoration MVP.
- **US2**: Depends only on prompt lifecycle semantics; integrates with US1 at the final restoration boundary.
- **US3**: Independent diagnostic hardening.
- **US4**: Independent ownership implementation; operational rollout depends on US1 backups and migration.

### Parallel Opportunities

- T007-T010 may be authored in parallel because they use separate test areas except coordinated session-store edits.
- T017-T020 may be authored in parallel across terminal, prompt, and builtins test files.
- T026-T028 are logically parallel but touch one test file and should be serialized by one implementer.
- T032-T034 may run in parallel by platform module.
- US3 and US4 can proceed in parallel with US1/US2 after Phase 2.

## Parallel Example: Prompt and Diagnostics

```text
Task: T017 restored-block terminal-view regression tests
Task: T018 waiting-for-context prompt tests
Task: T020 quiet Git command tests
Task: T026 bounded diagnostic formatting tests
Task: T032 Windows ownership identity tests
Task: T033 Linux ownership identity tests
```

## Implementation Strategy

### MVP First

1. Complete Setup and Foundational phases.
2. Complete US1 bounded V2 persistence and safe migration.
3. Validate copied legacy sessions without touching the real profile.
4. Complete US2 before using the migrated build interactively.

### Incremental Delivery

1. Bounded persistence prevents new oversized checkpoints.
2. Restoration lifecycle removes startup command and warning amplification.
3. Diagnostic hardening prevents future log corruption and growth.
4. Profile ownership prevents concurrent generations from recreating contention.
5. Cross-platform and operational validation completes rollout.

## Notes

- Do not force-kill WarpSpeed or delete user checkpoints as part of implementation.
- Do not log terminal payloads, command output, or migration contents to ordinary logs.
- Keep terminal-model lock scopes short and avoid nested locks.
- A task is complete only after its focused regression test passes.
