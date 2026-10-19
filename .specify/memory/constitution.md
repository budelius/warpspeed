<!--
Sync Impact Report
- Version change: template -> 1.0.0
- Added principles: Preserve User State; Bound Resource Use; Lifecycle and Event Discipline;
  Testable Compatibility; Safe Observability
- Added sections: Engineering Constraints; Delivery and Verification
- Removed sections: none
- Deferred items: none
-->
# WarpSpeed Reliability Constitution

## Core Principles

### I. Preserve User State
Changes to session persistence, terminal restoration, process ownership, or upgrades MUST preserve
running work and recoverable history. Destructive cleanup MUST NOT be the default. Format changes
MUST read the previous format or retain an atomic rollback copy, and interruption at any point MUST
leave at least one valid representation. Terminal-session descriptors and live-process ownership MUST
remain usable across a GUI restart.

### II. Bound Resource Use
Persisted output, diagnostic output, queues, restoration work, and background retries MUST have
explicit byte, count, or time bounds. Bounds MUST be enforced before allocating or replaying the full
input. When a bound is reached, the application MUST preserve structural metadata, prefer the most
recently useful content, record that truncation occurred, and continue with degraded but usable
behavior.

### III. Lifecycle and Event Discipline
Restored state MUST NOT be treated as a newly completed user action. Bulk restoration MUST suppress
per-item side effects and emit at most one completion transition for consumers that need refreshed
state. Expected not-ready states MUST wait for the lifecycle event that can make them ready rather
than polling, warning, or spawning repeated commands. Only one primary GUI writer may own a given
application profile at a time; helper and supervisor roles MUST be explicit and non-competing.

### IV. Testable Compatibility
Every regression fix MUST include deterministic coverage at the lowest level that reproduces the
failure. Persistence changes MUST cover old-format reading, new-format round trips, limits,
corruption, and interrupted writes. Process and filesystem behavior MUST be tested on affected
operating systems. Tests MUST live in separate `*_tests.rs` or `mod_test.rs` files and MUST validate
observable state rather than implementation call sequences.

### V. Safe Observability
Logs MUST be bounded, low-volume at default levels, and safe for collection. Hot paths and expected
fallbacks MUST NOT emit warning-or-higher messages per item. Command output, terminal content,
credentials, prompts, and other user-generated content MUST NOT enter ordinary breadcrumbs or error
reports. Diagnostic logs that intentionally contain local command output MUST be debug-only,
size-limited, rotation-limited, and safe under concurrent processes.

## Engineering Constraints

- Terminal-model locks MUST remain short-lived and MUST NOT be reacquired in the same call stack.
- Session migrations and checkpoint writes MUST be atomic and MUST validate output before replacing
  the active file.
- Compression MUST NOT substitute for decoded-size limits.
- A damaged block payload MUST NOT prevent restoring unaffected window and pane structure.
- Profile isolation MUST include database, logs, process ownership, and session checkpoints.
- Rust code MUST use exhaustive matches where practical, concise imports, inline format arguments,
  and minimal why-only comments as defined by `AGENTS.md`.

## Delivery and Verification

Work MUST proceed from a reviewed specification and dependency-ordered task list. Bug-reproducing
tests MUST be written before or with the corresponding fix. Focused tests and checks MUST pass before
workspace-wide formatting and Clippy. Filesystem, process, and locking changes MUST receive the
smallest relevant Windows, macOS, and Linux verification matrix after local checks pass. Rollout plans
MUST include backup, migration, downgrade, and recovery validation before cleanup of legacy data.

## Governance

This constitution governs Spec Kit artifacts and implementation for WarpSpeed reliability work.
`AGENTS.md` remains the repository-wide coding and review authority; where both apply, the stricter
requirement wins. Amendments require an updated Sync Impact Report, semantic version change, and an
explicit migration note for any relaxed guarantee. Every implementation review MUST verify the five
principles and record any justified exception in the feature plan's Complexity Tracking section.

**Version**: 1.0.0 | **Ratified**: 2026-10-18 | **Last Amended**: 2026-10-18
