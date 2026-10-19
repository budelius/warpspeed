# Implementation Plan: WarpSpeed Session Performance and Reliability

**Branch**: `001-warpspeed-session-performance` | **Date**: 2026-10-18 | **Spec**: [spec.md](spec.md)

**Input**: Feature specification from `/specs/001-warpspeed-session-performance/spec.md`

## Summary

Bound terminal-session output before persistence and replay, introduce a backward-readable compact V2 checkpoint with atomic V1 migration, batch restored-block lifecycle effects, make prompt and Git context updates event-driven, bound process-safe prompt diagnostics, and enable existing profile-scoped single-instance services for standalone WarpSpeed builds.

## Technical Context

**Language/Version**: Rust 1.92.0

**Primary Dependencies**: `serde`, `serde_json`, `serde_bytes`, `serde_bytes_repr`, `tempfile`, `fs4`, WarpUI entity/event infrastructure, existing `ipc` named-pipe service on Windows, and existing `zbus` application service on Linux/FreeBSD

**Storage**: SQLite block persistence plus versioned JSON launch-config session checkpoints under the WarpSpeed profile data directory

**Testing**: Rust unit tests with `cargo nextest`, existing WarpUI model test harness, filesystem tests using temporary directories, and platform startup/IPC integration checks

**Target Platform**: WarpSpeed desktop GUI on Windows, macOS, Linux, and FreeBSD; the reported failure and primary-instance remediation are Windows-first

**Project Type**: Cross-platform desktop terminal application in a large Cargo workspace

**Performance Goals**: Restore the representative 143 MB V1 session in under 5 seconds after migration; V2 checkpoint at most 25 MiB; no prompt subprocesses during replay; prompt diagnostics at most 30 MiB by default

**Constraints**: Preserve live supervisor-managed terminals; no destructive migration; one GUI writer per profile, including window restart paths; terminal payloads remain out of ordinary logs/telemetry; decoded, input-file, and retained-history limits enforced before unbounded allocation; old checkpoints remain rollback-readable

**Scale/Scope**: Session windows with at least 1,000 restored blocks, individual legacy outputs above 10 MiB, and multiple installed executable generations sharing one profile

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-checked after Phase 1 design.*

| Principle | Gate | Design response |
|---|---|---|
| Preserve User State | PASS | V1 remains readable, migration is atomic, `.v1.bak` is retained, and supervisor descriptors are unchanged. |
| Bound Resource Use | PASS | Block, pane, window, queue, record, and file budgets are explicit and decoded-side enforced. |
| Lifecycle and Event Discipline | PASS | Restored blocks skip per-command side effects; one post-restore refresh is event-driven; one primary owns each profile. |
| Testable Compatibility | PASS | Separate unit-test files cover V1/V2, limits, interruption, prompt state, logging, and ownership identity. |
| Safe Observability | PASS | Diagnostics are local debug-only, per-process, excerpt-limited, queue-limited, and rotation-limited. |

Post-design re-check: PASS. The contracts retain all required guarantees and introduce no justified constitution violations.

## Project Structure

### Documentation (this feature)

```text
specs/001-warpspeed-session-performance/
├── spec.md
├── plan.md
├── research.md
├── data-model.md
├── quickstart.md
├── contracts/
│   ├── profile-ownership.md
│   └── session-checkpoint-v2.md
├── checklists/
│   └── requirements.md
└── tasks.md
```

### Source Code (repository root)

```text
app/src/
├── app_services/
│   ├── linux/mod.rs
│   └── windows/
│       ├── mod.rs
│       ├── service_impl.rs
│       └── single_instance_manager.rs
├── bin/warpspeed.rs
├── context_chips/
│   ├── builtins.rs
│   ├── context_chip.rs
│   ├── current_prompt.rs
│   ├── current_prompt_tests.rs
│   ├── logging.rs
│   └── logging_tests.rs
├── launch_configs/
│   ├── session_store.rs
│   └── session_store_tests.rs
├── persistence/
│   ├── sqlite.rs
│   └── sqlite_tests.rs
├── terminal/
│   ├── model/block.rs
│   ├── model/block/serialized_block.rs
│   ├── model/block/serialized_block_tests.rs
│   ├── model/blocks.rs
│   └── view.rs
└── lib.rs
```

**Structure Decision**: Keep all changes in existing app subsystems. Persistence budgets belong with serialized blocks and the launch-config session store; prompt behavior remains in context chips and terminal event handling; process ownership reuses platform app services rather than adding a new service.

## Implementation Phases

### Phase A: Regression harness and bounded primitives

- Add generated oversized-output fixtures and pure truncation tests.
- Add bounded byte serialization/deserialization helpers accepting V1 arrays and V2 base64.
- Add pane/window byte and history-cardinality accounting that retains newer useful output.

### Phase B: Restoration and prompt lifecycle

- Prevent `BlockType::Restored` from triggering prompt and user-command side effects.
- Introduce a waiting-for-context prompt status and refresh only on relevant session events.
- Represent repository knowledge explicitly and avoid local non-repository Git fallback.
- Quiet and fingerprint-cache remote Git fallback failures.

### Phase C: Bounded diagnostics

- Add bounded stream excerpts with original lengths and truncation flags.
- Replace unbounded `mpsc` and per-record flush with a bounded non-blocking queue and buffered writer.
- Use PID-qualified files with size rotation and retention cleanup.

### Phase D: V2 checkpoint and safe migration

- Write compact base64 V2 checkpoints through the atomic writer.
- Read V1 and V2 through the compatible bounded deserializer.
- Create and retain rollback copies before first V2 replacement.
- Validate temporary output before atomic replacement and recover layout when block output is unusable.
- Add content-free timing/size/truncation diagnostics.

### Phase E: Profile ownership

- Make Windows and Linux startup forwarding available to standalone WarpSpeed app launches.
- Reserve Linux profile ownership before application state opens, then use D-Bus for launch forwarding.
- Derive IPC and ownership names from effective profile identity while preserving release-channel names.
- Ensure GUI app mode alone acquires/forwards ownership, reject full-GUI window handoff bypasses, and distinguish version metadata.
- Add same-profile, different-profile, helper-role, and stale-owner tests.

### Phase F: Validation and rollout

- Run focused tests after each subsystem phase.
- Run formatter, workspace Clippy, workspace nextest, doc tests, and `git diff --check`.
- Benchmark a copied/generated equivalent of the observed checkpoint.
- Execute the minimal OS-sensitive cloud matrix after local gates pass.
- Back up the real profile, gracefully stop old GUI generations, migrate, install, and validate the merged build without force-killing supervisors.

## Complexity Tracking

No constitution violations require justification. The V2 format remains JSON. Windows reuses its named mutex and pipe; Linux adds an early profile-local advisory lock to close the race before its existing D-Bus forwarding service starts, without adding a sidecar or daemon.
