# Feature Specification: WarpSpeed Multi-Window CPU Containment

**Feature Branch**: `fix/resume-cross-supervisor-agents`

**Created**: 2026-11-06

**Status**: Draft

**Input**: User description: "Reduce WarpSpeed's excessive CPU use in large restored multi-window profiles while preserving all terminal and agent sessions. The observed profile restores 8 windows, 95 tabs, and 208 panes into one GUI process, uses about 175% CPU and 4.1 GB private memory at idle, and exhibits stale-banner relayout warnings. Complete safe per-window isolation, quiesce background rendering, repair orphaned layout state, and provide bounded diagnostics and rollout validation."

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Work Responsively With Many Restored Windows (Priority: P1)

A user restores a large WarpSpeed profile and can type, switch tabs, and interact with the active terminal without idle background windows consuming enough CPU to cause visible input lag.

**Why this priority**: The current profile consumes roughly two logical cores while apparently idle. Typing responsiveness is the immediate user-visible failure.

**Independent Test**: Restore the reference profile shape of 8 windows, 95 tabs, and 208 panes, leave terminal output idle, and measure aggregate WarpSpeed CPU and input-to-display latency after startup settles.

**Acceptance Scenarios**:

1. **Given** the reference multi-window profile is restored and no terminal is producing output, **When** the application has settled for 30 seconds, **Then** background windows stop generating continuous visual work and aggregate CPU remains within the success threshold.
2. **Given** all reference windows are open, **When** the user types continuously in the active terminal, **Then** characters appear within the interaction-latency threshold without stalls caused by other windows.
3. **Given** the user rapidly changes the active window, **When** each newly focused window becomes active, **Then** its latest terminal state is displayed before it accepts the next user interaction.

---

### User Story 2 - Keep Background Sessions Alive Without Repainting Them Continuously (Priority: P1)

A user can minimize, cover, or leave a WarpSpeed window in the background while its shells and agents continue running, without paying the cost of continuously rendering content that cannot be seen.

**Why this priority**: Background work must remain correct, but model and session activity must not imply full-rate visual work for every inactive window.

**Independent Test**: Run bounded continuous output in one background window, keep another window active, and verify session continuity, background refresh limits, foreground responsiveness, and catch-up behavior when the background window is activated.

**Acceptance Scenarios**:

1. **Given** a window is minimized or not presentable, **When** an animation or timer requests another frame, **Then** visual updates are deferred while shell, agent, persistence, and notification processing continue.
2. **Given** a background terminal produces sustained output, **When** its window remains inactive, **Then** visual updates are coalesced to the background budget and terminal data is not dropped or duplicated.
3. **Given** a background window has accumulated changes, **When** the user activates it, **Then** it presents the current state within the catch-up threshold without replaying every skipped frame.
4. **Given** an inactive window requires immediate user attention, **When** that state occurs, **Then** the application may surface a bounded notification without resuming continuous background rendering.

---

### User Story 3 - Contain a Hot or Failed Window (Priority: P1)

A user with several WarpSpeed windows can continue using healthy windows when one window enters a render loop, becomes unresponsive, or crashes.

**Why this priority**: All windows currently share a failure and scheduling boundary, so one hot window can degrade every terminal in the profile.

**Independent Test**: Trigger a controlled render loop and a controlled crash in one reference window, then verify that all other windows remain interactive and that the affected window can recover its existing supervised sessions exactly once.

**Acceptance Scenarios**:

1. **Given** eight restored windows, **When** one window enters a controlled hot loop, **Then** the other seven remain within the interaction-latency threshold.
2. **Given** one window terminates unexpectedly, **When** recovery is requested, **Then** only that logical window is restored and its existing eligible terminal and agent sessions are reattached without duplicates.
3. **Given** safe window isolation cannot be established during restoration, **When** startup reaches that window, **Then** restoration stops safely and reports the limitation instead of placing all remaining windows into one shared failure boundary.
4. **Given** the profile coordinator becomes unavailable, **When** healthy windows are still open, **Then** those windows remain usable in a bounded degraded mode until ownership is safely restored.

---

### User Story 4 - Recover From Stale Layout State Without Warning Storms (Priority: P2)

A user restoring a pane with stale transient layout state sees a correct terminal layout instead of a blank band, repeated relayout, or a warning emitted on every frame.

**Why this priority**: The observed `Missing banner element` loop amplifies layout and logging work and makes diagnosis harder, even though it is not the only CPU source.

**Independent Test**: Restore a generated pane whose saved layout references a missing transient banner and verify one bounded repair, correct layout, and no repeated warning or repaint loop.

**Acceptance Scenarios**:

1. **Given** a restored layout references a transient element that no longer exists, **When** the mismatch is first detected, **Then** the stale reservation is removed and the layout becomes consistent.
2. **Given** the mismatch has been repaired, **When** later frames are rendered, **Then** no warning or relayout is generated for that removed item.
3. **Given** several distinct stale items exist, **When** they are repaired, **Then** diagnostics report bounded content-free counts rather than one message per frame.

---

### User Story 5 - Verify and Roll Out the Fix Safely (Priority: P2)

An engineer can prove that CPU reductions come from fewer unnecessary visual updates, identify a regressing window without recording terminal content, and disable the new behavior on the next clean launch if a rollout problem appears.

**Why this priority**: The fix changes window lifecycle and recovery behavior, so it needs objective performance evidence and a safe rollback path.

**Independent Test**: Run the disposable-profile performance and failure matrix, inspect content-free per-window measurements, and perform enable, disable, upgrade, and downgrade cycles without losing saved or live session state.

**Acceptance Scenarios**:

1. **Given** a disposable reference profile, **When** the benchmark runs, **Then** it reports CPU, frame cadence, wake reasons, focus latency, and recovery outcomes per logical window without terminal content.
2. **Given** the new behavior is disabled after a clean shutdown, **When** WarpSpeed starts again, **Then** all windows remain recoverable through the compatible saved state.
3. **Given** measurements exceed their retention or cardinality limits, **When** new measurements arrive, **Then** older data is aggregated or discarded without increasing application load materially.

### Edge Cases

- A background pane emits output faster than it can be displayed for several minutes.
- A window is minimized, covered, moved between displays, or rapidly focused and unfocused while a repaint timer is pending.
- The operating system cannot reliably report occlusion; the application must still avoid unsafe assumptions and preserve correctness.
- A window contains an active animation, progress indicator, notification, or drag operation when it becomes inactive.
- The machine sleeps or changes graphics devices while windows are quiescent.
- A worker fails before its first frame, after session attachment, or while sending its final state.
- A coordinator restarts while surviving windows and terminal supervisors remain alive.
- A legacy checkpoint contains unknown or partially corrupt transient layout items.
- A version mismatch occurs while another version owns the profile.
- The user closes or opens a window while sequential multi-window restoration is still in progress.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: WarpSpeed MUST track whether each top-level window is focused, visible, minimized, occluded when available, and able to present visual updates.
- **FR-002**: A window that is not presentable MUST NOT run continuous animation or timer-driven visual updates, while non-visual session and persistence work continues.
- **FR-003**: Inactive-window terminal and model changes MUST be coalesced within explicit time, count, and memory bounds rather than generating one visual update per event.
- **FR-004**: Activating a quiescent window MUST cancel obsolete deferred work and present its latest complete state before normal interaction resumes.
- **FR-005**: Active-window input, resize, drag, accessibility, and frame-capture work MUST bypass background throttling when required for correctness.
- **FR-006**: Quiescing visual work MUST NOT pause, terminate, duplicate, or detach live shell and agent sessions.
- **FR-007**: Each restored top-level window MUST have an independent render and failure boundary while exactly one owner controls shared profile state.
- **FR-008**: Shared databases, settings, recovery catalogs, and profile files MUST have exactly one writable owner for a profile.
- **FR-009**: Multi-window restoration MUST wait for each isolated window to become ready or fail within a bounded timeout before starting the next window.
- **FR-010**: If safe isolation, authentication, or independent process lifetime cannot be established, WarpSpeed MUST fail closed and preserve recoverable state rather than restoring all windows into one GUI process or creating multiple profile writers.
- **FR-011**: Closing, crashing, hanging, or restarting one logical window MUST NOT close or block healthy windows in the same profile.
- **FR-012**: Recovery MUST prefer attachment to supervisor-owned terminal sessions and MUST NOT start a replacement shell or agent while prior ownership is uncertain.
- **FR-013**: Cross-window actions, external launch requests, profile quit, and saved-state aggregation MUST continue to behave as one application from the user's perspective.
- **FR-014**: A stale transient layout reservation with no corresponding renderable element MUST be removed through a bounded repair path that cannot mutate terminal state while it is already locked for rendering.
- **FR-015**: The same stale layout item MUST NOT produce repeated warning-or-higher messages or repeated repairs after reconciliation.
- **FR-016**: Performance diagnostics MUST distinguish logical windows and categorize visual wake reasons, frame counts, deferred updates, focus latency, recovery events, and dropped measurements.
- **FR-017**: Diagnostics MUST be bounded and content-free; terminal output, commands, prompts, credentials, paths, and session payloads MUST NOT be recorded in ordinary logs or telemetry.
- **FR-018**: The application MUST preserve window order, bounds, active tab, pane layout, custom title, launch source, and session attachment metadata through enable, disable, restart, upgrade, and downgrade paths.
- **FR-019**: The feature MUST support a clean-start rollback path that retains a complete state readable by the prior single-process behavior.
- **FR-020**: Automated verification MUST cover idle, active input, background output, rapid focus changes, stale layout repair, one-window crash/hang, coordinator loss, version mismatch, and interrupted restoration using disposable profiles.
- **FR-021**: After the implementation build, installation, and disposable-profile gates pass, the authorized deployment procedure MUST gracefully restart the user's normal WarpSpeed GUI, preserve supervisor-managed sessions, restore every saved window into its own isolated worker, and stop with a recoverable error if that topology cannot be established.

### Key Entities

- **Logical Window**: A durable top-level window identity with saved layout, focus order, launch source, recovery generation, and current failure boundary.
- **Window Presentation State**: Whether a logical window is focused, visible, minimized, occluded when known, presentable, dirty, or awaiting a deferred visual update.
- **Visual Update Demand**: A bounded request to rebuild or present a window, including its reason, urgency, earliest eligible time, and whether newer work supersedes it.
- **Profile Owner**: The sole authority for shared profile identity, durable writes, external launch routing, and logical-window coordination.
- **Session Attachment**: The identity and generation needed to reconnect a pane to a supervisor-owned shell or agent without creating a duplicate.
- **Layout Reconciliation**: A bounded record that identifies a stale transient reservation, its repair state, and a content-free outcome category.
- **Performance Window**: Aggregated measurements for CPU, visual updates, wake reasons, interaction latency, deferrals, and recovery over a bounded interval.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: On the reference Windows machine, the idle 8-window, 95-tab, 208-pane disposable profile uses no more than 25% of one logical CPU core at the median and 50% at the 95th percentile during a 60-second sample after settling.
- **SC-002**: With the reference profile open, 95% of typed characters appear within 50 milliseconds and 99% within 100 milliseconds when no foreground command is saturating the machine.
- **SC-003**: A minimized or fully occluded unchanged window initiates zero continuous animation frames during a 60-second interval and performs at most one catch-up presentation after becoming presentable.
- **SC-004**: A background window receiving sustained terminal output performs no more than 5 visual updates per second, retains correct terminal state, and displays the latest state within 250 milliseconds after activation.
- **SC-005**: During a controlled hot loop, hang, or crash in one of eight windows, each healthy window continues accepting and displaying input within 100 milliseconds and no healthy window exits.
- **SC-006**: In 100 consecutive controlled worker-recovery runs, each eligible shell and agent session is attached exactly once and no replacement is started while the original session remains owned.
- **SC-007**: Restoring a fixture with orphaned transient layout items produces no blank reserved band, completes repair before the next stable frame, and emits at most one bounded diagnostic event per distinct item.
- **SC-008**: Process and filesystem instrumentation observes exactly one writer for every protected profile resource throughout startup, steady state, window recovery, and shutdown.
- **SC-009**: Enable, disable, upgrade, downgrade, and interruption tests preserve 100% of logical window structure and leave all eligible supervised sessions attachable.
- **SC-010**: The focused unit, process-integration, formatting, lint, and Windows/macOS/Linux verification matrix passes without introducing warning storms or unbounded diagnostic files.
- **SC-011**: Final installed-build validation restores all expected normal-profile windows with exactly one coordinator and one GUI worker PID per window, while all preflight-counted eligible shell and agent sessions remain alive and attach exactly once.

## Assumptions

- Terminal supervisors remain the authority for keeping eligible local shell and agent processes alive across GUI-window and coordinator restarts.
- The observed 8-window, 95-tab, 208-pane profile shape is the initial reference workload; automated tests may generate a smaller structurally equivalent fixture for fast checks.
- The existing disabled per-window coordinator/worker foundation and its design document are prior work to audit and extend, not proof that isolation is production-ready.
- Operating-system occlusion reporting differs by platform; minimization, focus, and explicit visibility provide conservative fallbacks when exact occlusion is unavailable.
- Aggregate memory may not decrease when windows move to independent failure boundaries; this feature's primary targets are CPU containment, interaction latency, and failure isolation.
- Live terminal output must continue to be consumed even when visual presentation is deferred, so backpressure applies to visual work rather than PTY correctness.
- The separate `SYS` profile is outside the reference profile's CPU target and remains independently owned.
- The user has explicitly authorized a final normal-profile GUI restart after a successful build and all disposable-profile safety gates; this authorization does not permit force-killing terminal supervisors, shells, agents, `OpenConsole`, or the separate `SYS` profile.
