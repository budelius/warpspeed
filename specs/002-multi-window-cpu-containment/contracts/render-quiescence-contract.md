# Render Quiescence Contract

## Purpose

Define when a WarpSpeed window may build and present a scene, what work continues while visual presentation is deferred, and how activation catches up without replaying skipped frames.

## Inputs

The admission decision receives:

- current `WindowPresentationState` and transition generation;
- one coalesced `RenderDemand`;
- current monotonic time;
- whether a correctness-critical operation is active (input, resize/drag, accessibility, frame capture, first frame, or surface recovery);
- whether an equivalent redraw is already pending.

No terminal content or arbitrary component label participates in the policy.

## Admission Matrix

| Presentation class | Interactive/correctness demand | Content/model demand | Animation timer | Asset completion |
|---|---|---|---|---|
| `Interactive` | Admit immediately | Admit immediately | Admit at requested cadence | Admit if it changes the visible scene |
| `BackgroundVisible` | Admit immediately when OS interaction requires it | Coalesce; maximum 5 presentations/second | Coalesce to background cadence | Merge into the current demand |
| `UnknownBackground` | Admit immediate correctness work | Coalesce; maximum 5 presentations/second | Coalesce to background cadence | Merge into the current demand |
| `Suppressed` | Retain until presentable unless needed to complete a native operation | Retain one dirty demand | Do not arm/re-arm | Mark dirty; do not present |

## Required Behavior

1. There is at most one coalesced dirty demand and one active repaint timer per window.
2. A repeated invalidation while an admitted redraw is pending merges into that demand and does not issue another native redraw request.
3. A suppressed window does not build scenes merely to clear invalidations.
4. Terminal-model updates, PTY reads, agent state, persistence, notifications, and supervisor heartbeats continue regardless of render admission.
5. Entering `Interactive` or becoming newly presentable:
   - increments the presentation generation;
   - cancels stale timers;
   - promotes any dirty demand to immediate correctness work;
   - builds the latest model state once;
   - does not replay missed animation frames.
6. A presentation failure retains the dirty demand and follows the existing bounded surface/device recovery path.
7. Rapid focus/occlusion changes are idempotent. A timer created under an older generation no-ops.
8. Static windows initiate no periodic frames.

## Performance Guarantees

- Background changing content: no more than 5 presentations per second per window.
- Minimized or known fully occluded unchanged window: zero continuous animation frames.
- Focus/exposure catch-up: latest state presented within 250 ms under the reference workload.
- Focused input: 50 ms p95 and 100 ms p99 input-to-present latency under the reference workload.

## Correctness Exceptions

The following may bypass background cadence for one bounded operation:

- user input directed to that window;
- native resize, drag, display-scale, or surface reconfiguration;
- accessibility query requiring current geometry;
- explicit frame capture;
- first-frame readiness handshake;
- re-exposure after suppression;
- layout reconciliation completion.

An exception admits one operation; it does not permanently resume animation cadence.

## Test Contract

Fake-time tests MUST verify:

- repeated invalidations coalesce;
- stale timer generations no-op;
- suppressed animations do not re-arm;
- background cadence never exceeds the cap;
- activation produces one current frame;
- model updates continue while presentation is suppressed;
- surface failure keeps the demand dirty;
- multiple windows maintain independent demand and timer state.
