# Data Model: WarpSpeed Multi-Window CPU Containment

This feature adds bounded runtime coordination state. Existing terminal content and persisted window snapshots remain in their current formats except for stable logical-window identity and revision metadata required for aggregation.

## 1. LogicalWindowIdentity

Identifies one durable top-level window across worker restarts.

| Field | Type | Rules |
|---|---|---|
| `worker_id` | UUID | Created once by the profile owner; stable across worker generations and app restarts. |
| `worker_generation` | Monotonic integer | Incremented before each respawn; stale generations cannot mutate state. |
| `profile_generation` | Random generation ID | Matches the live coordinator generation; never persisted as authority across coordinator restarts. |
| `build_identity` | Bounded string | Exact coordinator build; workers with a mismatch are rejected. |
| `window_order` | Non-negative integer | Unique within the current aggregate; normalized by the coordinator. |

### Validation

- A live profile has at most one registered worker for `(worker_id, worker_generation)`.
- A message with an older worker or profile generation is stale and has no side effects.
- Identity strings retain the existing protocol limit of 256 bytes.

## 2. WorkerLifecycle

Represents coordinator knowledge of one logical window's execution boundary.

```text
Queued
  -> Starting(deadline)
  -> Authenticating
  -> Restoring
  -> Ready(first_frame_at)
  -> Quiescent | Active
  -> Closing(final_revision)
  -> Closed

Starting | Authenticating | Restoring | Ready | Active | Quiescent
  -> Failed(category, recoverable_snapshot)
  -> RestartBackoff(attempt, deadline)
  -> Queued(next_generation)

Any live state
  -> CoordinatorUnavailable
  -> ReRegistering
  -> Ready | Failed
```

### Fields

| Field | Purpose |
|---|---|
| `identity` | Logical and generation identity. |
| `pid` | Content-free local diagnostic and liveness field. |
| `state` | Exhaustive lifecycle variant. |
| `ready_deadline` | Bounded startup timeout; initially 60 seconds. |
| `last_heartbeat` | Detects unresponsive workers; worker cadence 5 seconds, timeout 30 seconds. |
| `restart_attempts` | Bounded crash budget; initially 3 attempts per 5-minute window. |
| `last_acked_snapshot_revision` | Durable aggregate boundary. |
| `pending_commands` | Bounded queue; at most 128 coordinator commands. |

### Invariants

- The coordinator starts at most one worker at a time during bulk restoration.
- `Ready` requires successful authentication, exactly one real window, and first-frame acknowledgement.
- Exhausting the crash budget retains the recoverable snapshot and requires explicit recovery.

## 3. WindowPresentationState

Describes whether visual work is useful for one platform window.

| Field | Type | Meaning |
|---|---|---|
| `focused` | Boolean | Window currently receives keyboard interaction. |
| `explicitly_visible` | Boolean/Unknown | Native window visibility where available. |
| `minimized` | Boolean/Unknown | Native minimized state or conservative platform inference. |
| `occlusion` | `Unknown`, `Visible`, `Occluded` | Native occlusion result when supported. |
| `surface_available` | Boolean | A configured presentation surface can accept a frame. |
| `transition_generation` | Monotonic integer | Invalidates timers created under an older state. |
| `changed_at` | Monotonic instant | Supports latency measurement without wall-clock assumptions. |

### Derived `PresentationClass`

| Class | Condition | Render behavior |
|---|---|---|
| `Interactive` | Focused, visible, not minimized, surface available | Immediate demand. |
| `BackgroundVisible` | Unfocused, visible, not minimized, not known occluded, surface available | Static on demand; changing content capped at 5 Hz. |
| `Suppressed` | Hidden, minimized, known occluded, or surface unavailable | No continuous animation; retain one dirty demand. |
| `UnknownBackground` | Unfocused with unreliable visibility/occlusion | Preserve correctness; changing content capped at 5 Hz. |

### Transitions

- Entering `Interactive` cancels obsolete background timers and admits one catch-up demand.
- Entering `Suppressed` cancels animation repaint timers but does not clear model invalidations.
- A surface recreation increments the generation and forces one correctness frame when presentable.
- Focus, visibility, minimization, occlusion, resize, display-scale, and surface events are idempotent when they do not change the derived class.

## 4. RenderDemand

A single coalesced record of visual work owed by one window.

| Field | Type | Rules |
|---|---|---|
| `dirty` | Boolean | True after any admitted semantic invalidation until a current scene is built. |
| `reason_bits` | Fixed bit set | Contains only predefined reason categories. |
| `urgency` | `Interactive`, `Correctness`, `Content`, `Animation` | Highest urgency wins when demands merge. |
| `first_requested_at` | Monotonic instant | Preserved across merges for latency. |
| `last_requested_at` | Monotonic instant | Tracks newest state. |
| `eligible_at` | Monotonic instant | Earliest allowed presentation under the current policy. |
| `scheduled_generation` | Integer | Must equal the current presentation transition generation. |
| `request_count` | Saturating integer | Diagnostic count, not an unbounded event list. |

### Fixed reason categories

- terminal content
- cursor/input
- focus/exposure
- resize/display change
- model notification
- animation timer
- asset ready
- accessibility/frame capture
- layout reconciliation
- recovery/first frame
- unknown-other

### Merge rules

1. Keep one record per window.
2. Union fixed reason bits and saturate counts.
3. Keep the earliest first request and latest semantic state.
4. Promote urgency but never demote it until presentation.
5. Replace an older scheduled generation rather than running both timers.
6. Clear only after a scene representing the latest model revision is built; presentation failure leaves it dirty.

## 5. DeferredRepaint

Owns at most one pending timer per window.

| Field | Purpose |
|---|---|
| `window_id` | Platform window receiving the eventual request. |
| `deadline` | Next admissible render instant. |
| `presentation_generation` | Causes stale timer callbacks to no-op. |
| `kind` | Background coalescing, animation, or asset completion. |

### Bounds

- At most one coalescing/animation timer and one bounded asset-completion wait per window.
- Suppressed windows retain no periodic animation timer.
- A new earlier correctness or interactive demand replaces a later timer.
- Timer callbacks never create another timer if presentation remains suppressed.

## 6. WindowSnapshotRevision

Revisioned state sent from a worker to the coordinator.

| Field | Rules |
|---|---|
| `worker_id` | Must match the registered worker. |
| `worker_generation` | Must be current. |
| `revision` | Strictly increases for that worker generation. |
| `window_snapshot` | Existing bounded one-window snapshot, maximum 16 MiB encoded. |
| `save_reason` | Fixed category: periodic coalesced save, user close, requested restart, crash recovery, or shutdown. |
| `created_at` | Monotonic/local timestamp used only for ordering and diagnostics. |

### Aggregation

- The coordinator stores only the latest acknowledged snapshot per logical window.
- Stale revisions are acknowledged as stale or rejected without replacing current data.
- Aggregated `AppState` follows stable `window_order` and records one active logical window.
- A close commits explicit logical-window deletion; absence of a new snapshot is never interpreted as deletion.

## 7. SessionAttachment

Minimal metadata needed to reconnect a pane to a supervisor-owned session.

| Field | Rules |
|---|---|
| `supervisor_endpoint` | Existing bounded local endpoint identity; never logged. |
| `session_id` | Existing opaque session identity; excluded from ordinary diagnostics. |
| `session_generation` | Must match supervisor authority. |
| `pane_id` | Durable pane identity. |
| `lease_generation` | Prevents stale workers from attaching or releasing a newer lease. |
| `resume_descriptor` | Existing bounded agent-resume metadata. |

### State transitions

```text
Described -> AttachRequested -> Attached
Described -> OwnershipUncertain -> AwaitAuthority
AwaitAuthority -> Attached | ProvenAbsent
ProvenAbsent -> ReplacementAllowed
Attached -> TransferPrepared -> TransferCommitted | TransferAborted
```

A replacement shell or agent is forbidden before `ProvenAbsent`.

## 8. LayoutReconciliationRequest

Carries stale transient-item findings from layout to a safe mutation point.

| Field | Type | Rules |
|---|---|---|
| `terminal_view_id` | Local entity ID | Used only inside the worker. |
| `missing_inline_banner_ids` | Bounded deduplicated set | Maximum 256 IDs per request. |
| `full_transient_reconciliation` | Boolean | Set instead of growing beyond the ID limit. |
| `detection_generation` | Monotonic integer | Prevents replay after a newer reconciliation. |
| `reason` | Fixed enum | Missing inline banner, stale rich content, or index mismatch. |

### State transitions

```text
Detected -> Queued -> Applying -> Applied -> Reported
                      |-> Superseded
                      |-> RetryNeeded (bounded)
```

### Invariants

- Detection does not mutate the terminal model.
- Applying occurs after render/layout lock scopes end.
- One apply removes all matching view and block-list representations atomically from the user's perspective.
- Applied IDs are not reported or queued again unless a new transient item with that identity is created.

## 9. PerformanceWindow

Bounded content-free measurements for one logical window.

| Field | Rules |
|---|---|
| `worker_id`, `worker_generation` | Diagnostic identity only. |
| `bucket_start` | One-second monotonic bucket. |
| `presentation_class` | Final/majority class for the bucket. |
| `wake_reason_counts` | Fixed array indexed by reason enum. |
| `requested`, `admitted`, `deferred`, `coalesced` | Saturating counts. |
| `scenes_built`, `frames_presented`, `presentation_failures` | Saturating counts. |
| `focus_latency_histogram` | Fixed latency buckets; no raw event list. |
| `reconciliation_counts` | Fixed outcome categories. |
| `dropped_measurements` | Count of values omitted due to bounds. |

### Bounds and retention

- Exactly 60 one-second buckets per live logical window.
- Closed workers contribute one final aggregate and are then removed from memory.
- Ordinary logs emit only lifecycle or threshold summaries.
- No map key or label derives from terminal, command, path, or user content.

## Relationships

```text
ProfileOwner
  1 ─── * WorkerLifecycle
  1 ─── * WindowSnapshotRevision (latest per LogicalWindowIdentity)

LogicalWindowIdentity
  1 ─── 1 WindowPresentationState
  1 ─── 0..1 RenderDemand
  1 ─── 0..1 DeferredRepaint
  1 ─── * SessionAttachment
  1 ─── 1 PerformanceWindow

TerminalView
  1 ─── 0..1 LayoutReconciliationRequest
```

## Compatibility

- Legacy app-state windows without `worker_id` receive IDs only from the coordinator during a controlled migration and are then persisted in the complete aggregate.
- The aggregate remains readable by the legacy single-process path; worker registry and runtime presentation state are not required to restore user windows.
- Render admission and diagnostics are runtime-only and do not change terminal content serialization.
