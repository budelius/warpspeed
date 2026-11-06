# Feature Specification: WarpSpeed Session Performance and Reliability

**Feature Branch**: `001-warpspeed-session-performance`

**Created**: 2026-10-18

**Status**: Draft

**Input**: User description: "Fix repeated restoration warnings, non-repository Git prompt retries, NUL-heavy and unbounded prompt logs, oversized restored-session files, unsafe legacy cleanup, and concurrent WarpSpeed versions using Spec Kit."

## User Scenarios & Testing

### User Story 1 - Restore Sessions Without Freezing (Priority: P1)

A user restarts WarpSpeed with large terminal histories and gets their windows, panes, commands, recent output, and resumable processes back promptly without the application becoming unresponsive.

**Why this priority**: Session restoration currently processes checkpoint files large enough to dominate startup time and memory, directly blocking the user's work.

**Independent Test**: Restore a representative legacy session containing heavily styled terminal output and verify that its layout and useful recent history return within the stated resource limits.

**Acceptance Scenarios**:

1. **Given** a valid legacy checkpoint with oversized command output, **When** WarpSpeed starts, **Then** the checkpoint is migrated safely and the window layout, pane metadata, commands, recent output, and process-resume metadata are restored.
2. **Given** an output payload larger than the persistence budget, **When** it is checkpointed, **Then** the saved session remains within the configured bounds and clearly records that older output was omitted.
3. **Given** a checkpoint whose block-output payload is damaged, **When** it is restored, **Then** unaffected window and pane structure remains recoverable.
4. **Given** migration is interrupted before replacement completes, **When** WarpSpeed starts again, **Then** at least one complete checkpoint remains available.

---

### User Story 2 - Use a Responsive Prompt After Restoration (Priority: P1)

A user opening restored panes sees prompt context update once the live shell is ready, without warning storms, repeated helper commands, or Git usage output in non-repository directories.

**Why this priority**: Restoration currently turns every restored block into prompt work, amplifying startup cost and producing thousands of warnings.

**Independent Test**: Restore hundreds of blocks into a pane with no live execution context, then attach a shell in a non-repository directory and observe command execution and diagnostics.

**Acceptance Scenarios**:

1. **Given** blocks are being replayed, **When** each restored block completes, **Then** no prompt command is launched for that individual block.
2. **Given** replay has finished but the shell context is not ready, **When** prompt state is evaluated, **Then** the state waits without being treated as an error or repeatedly retried.
3. **Given** a live local shell in a known non-repository directory, **When** Git chips are evaluated, **Then** no Git fallback process is launched.
4. **Given** a remote shell whose repository state cannot be determined natively, **When** its quiet fallback fails, **Then** it is attempted at most once until the session, directory, or relevant command state changes.

---

### User Story 3 - Keep Diagnostics Bounded and Useful (Priority: P2)

A user or engineer can inspect prompt diagnostics without logs consuming unbounded space, containing giant command output, or becoming mostly NUL bytes when more than one process exists.

**Why this priority**: Diagnostics should help investigate failures and must not become another performance or disk-space failure mode.

**Independent Test**: Run concurrent diagnostic writers and commands producing oversized standard output and error output, then inspect all resulting log files.

**Acceptance Scenarios**:

1. **Given** two processes initialize prompt diagnostics concurrently, **When** both write records, **Then** records remain readable and no zero-filled holes are created.
2. **Given** a command emits output above the excerpt limit, **When** it is logged, **Then** the record includes a bounded excerpt, the original byte count, and a truncation indicator.
3. **Given** diagnostics continue for an extended period, **When** retention limits are reached, **Then** old diagnostics are rotated or removed without blocking prompt updates.
4. **Given** diagnostic writes cannot keep up, **When** the bounded queue fills, **Then** the application remains responsive and reports dropped diagnostics without exposing terminal content in normal logs.

---

### User Story 4 - Run One Primary WarpSpeed Per Profile (Priority: P2)

A user launching WarpSpeed repeatedly or after installing a new version gets a new window in the existing profile owner or a safe version handoff, rather than multiple GUI generations writing the same data.

**Why this priority**: Concurrent GUI generations currently share databases, logs, and checkpoints and can consume several gigabytes while contending on profile state.

**Independent Test**: Launch the application twice for one profile, repeat with a version mismatch, and separately launch an explicitly isolated profile.

**Acceptance Scenarios**:

1. **Given** a primary GUI owns a profile, **When** the same version is launched again, **Then** the request is forwarded to the primary and the second GUI exits.
2. **Given** an older primary owns a profile, **When** a newer version starts, **Then** no concurrent GUI writer is created and the user can complete a graceful restart/handoff without losing running terminal sessions.
3. **Given** two distinct explicit profiles, **When** both are launched, **Then** each may have its own primary and isolated data.
4. **Given** helper, crash-recovery, supervisor, or CLI processes, **When** primary ownership is evaluated, **Then** those roles do not incorrectly become competing GUI owners.

### Edge Cases

- A single terminal row contains more styled bytes than the per-block budget.
- Output contains invalid UTF-8, incomplete ANSI control sequences, embedded images, or no newline boundaries.
- A session has many small blocks that individually fit but exceed pane or window budgets together.
- A legacy checkpoint is larger than available memory or has a valid layout followed by malformed block data.
- The target filesystem does not support sparse files or has platform-specific atomic rename semantics.
- A process crashes while holding primary ownership or while a diagnostic record is queued.
- A terminal supervisor from an older version still owns resumable sessions during a GUI upgrade.
- Repository state changes from unknown to repository or non-repository after shell bootstrap.

## Requirements

### Functional Requirements

- **FR-001**: WarpSpeed MUST enforce explicit serialized-output, checkpoint-file, pane-history, and retained-block limits before allocating or replaying the complete input.
- **FR-002**: When limits are reached, WarpSpeed MUST preserve layout, pane identity, command metadata, timestamps, process-resume metadata, and a useful bounded portion of recent output.
- **FR-003**: Persisted data MUST record whether output was truncated and MUST restore truncated output without malformed text or terminal-style state leaking into subsequent content.
- **FR-004**: WarpSpeed MUST read existing session checkpoints and migrate oversized checkpoints without requiring users to delete their history.
- **FR-005**: Migration MUST use validated atomic replacement and retain a rollback source until the migrated checkpoint is confirmed usable.
- **FR-006**: Corrupt or oversized block output MUST NOT prevent restoration of unaffected window and pane structure.
- **FR-007**: Restored blocks MUST NOT trigger the same prompt, filter, telemetry, queue, or helper-command side effects as newly completed user commands.
- **FR-008**: A restored pane MUST perform at most one prompt refresh after replay and live shell context are ready.
- **FR-009**: Missing shell execution context MUST be represented as an expected not-ready state and retried only after a relevant lifecycle change.
- **FR-010**: Known local non-repository directories MUST disable repository-dependent prompt work without spawning Git.
- **FR-011**: Shell fallbacks for indeterminate repository state MUST suppress expected error output and cache deterministic failures until their input state changes.
- **FR-012**: Prompt diagnostic records MUST impose fixed limits on captured standard output, standard error, queued records, file size, and retained files.
- **FR-013**: Prompt diagnostic storage MUST remain valid when multiple processes write diagnostics and MUST NOT create zero-filled gaps through shared truncate-on-open behavior.
- **FR-014**: Normal application logs and error reports MUST contain only metadata about diagnostic truncation or loss, never terminal or command output.
- **FR-015**: Exactly one primary GUI writer MUST own a given WarpSpeed profile at a time, including during controlled window restart or handoff paths.
- **FR-016**: Repeated launches for an owned profile MUST forward the request to the primary or complete a graceful version handoff without concurrent profile writers.
- **FR-017**: Every non-empty explicitly named profile MUST retain a distinct runnable identity after sanitization and remain isolated.
- **FR-018**: Primary ownership MUST distinguish GUI, CLI, crash helper, and terminal-supervisor roles and recover automatically after process failure.
- **FR-019**: Upgrade and cleanup flows MUST preserve live terminal sessions and MUST NOT force-kill processes or delete checkpoints as their normal path.
- **FR-020**: The application MUST expose bounded, content-free measurements for checkpoint size, migration result, restoration duration, truncation count, and dropped diagnostic records.

### Key Entities

- **Session Checkpoint**: Versioned representation of a saved window, its pane layout, bounded block history, validation data, and save time.
- **Persisted Block Output**: A command's styled output with retained bytes, original size, truncation state, and safe terminal-style boundaries.
- **Migration Result**: The source and destination versions, validation outcome, retained backup, and content-free size/count measurements.
- **Prompt Refresh State**: Whether prompt data is unavailable, waiting for context, loading, cached, ready, disabled, or failed for the current input fingerprint.
- **Repository Context**: Whether repository state is unknown, known absent, or attached to a repository status source.
- **Diagnostic Record**: Bounded command metadata and output excerpts stored only in local debug diagnostics.
- **Profile Ownership**: Profile identity, primary GUI process identity, application version, role, and handoff state.

## Success Criteria

### Measurable Outcomes

- **SC-001**: A representative 143 MB legacy checkpoint restores its usable layout and recent history in under 5 seconds on the reference Windows development machine after migration.
- **SC-002**: Newly written checkpoints never exceed their declared decoded window budget, and a representative migrated checkpoint occupies no more than 25 MB on disk.
- **SC-003**: Restoring 1,000 blocks launches zero prompt helper commands during replay and produces zero per-block prompt or block-filter warnings.
- **SC-004**: A known non-repository directory launches zero Git prompt processes; an indeterminate remote failure launches at most once per unchanged input state.
- **SC-005**: Prompt diagnostics remain below 30 MB total by default, no individual command contributes more than 12 KiB of captured output and metadata, and concurrent-writer tests produce zero NUL-filled gaps.
- **SC-006**: Ten repeated launches of one profile result in one primary GUI writer, while an explicitly separate profile can own one additional primary.
- **SC-007**: Forced interruption at every migration replacement boundary leaves either the original or replacement checkpoint valid in 100% of tested cases.
- **SC-008**: Users can restart into the new version with all previously live supervisor-managed terminal sessions still attachable.
- **SC-009**: Local focused tests, formatting, Clippy, and the relevant Windows, macOS, and Linux verification matrix complete without regressions.

## Assumptions

- Existing terminal supervisors are the authority for keeping live local terminal processes alive across GUI restarts.
- Recent output and structural metadata are more valuable for recovery than unlimited historical styled output.
- Expected repository absence is not an application error and should not create warnings or error reports.
- Diagnostic command output is local debug data and is not suitable for ordinary logs, telemetry, or crash reports.
- Different `WARPSPEED_PROFILE` values represent intentional isolation; elevation-derived profiles remain separate.
- The 143 MB STOZN checkpoint is the reference worst-case fixture, while automated tests may use a smaller structurally equivalent generated fixture.
