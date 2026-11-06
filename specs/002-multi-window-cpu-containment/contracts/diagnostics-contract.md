# CPU Containment Diagnostics Contract

## Purpose

Provide enough bounded, content-free evidence to identify hot windows and visual wake sources without making diagnostics another CPU, disk, privacy, or multi-process contention problem.

## Runtime Aggregation

Each logical window owns a fixed ring of 60 one-second `PerformanceWindow` buckets. Each bucket contains only fixed enums, saturating integer counters, and fixed latency histogram buckets.

### Required counters

- render demands requested;
- demands admitted, deferred, and coalesced;
- native redraw requests;
- scenes built;
- frames presented and presentation failures;
- delayed timers armed, cancelled, and ignored as stale;
- focus/exposure catch-up latency histogram;
- worker readiness and heartbeat status categories;
- layout reconciliation detected, repaired, superseded, failed, and overflow-to-full-scan counts;
- dropped or saturated diagnostic measurements.

### Fixed wake-reason categories

- terminal content;
- cursor/input;
- focus/exposure;
- resize/display change;
- model notification;
- animation timer;
- asset ready;
- accessibility/frame capture;
- layout reconciliation;
- recovery/first frame;
- unknown-other.

No dynamic reason labels are allowed.

## Allowed Identity Fields

- process role;
- PID in local diagnostics;
- logical worker UUID and generation;
- coordinator generation category (not capability);
- build/protocol identity;
- presentation-state category;
- graphics backend category;
- operating-system category.

## Prohibited Content

Diagnostics MUST NOT contain:

- terminal output or input;
- commands, prompts, current directories, repository paths, or filenames;
- usernames beyond existing sanitized profile identity;
- environment values, credentials, auth tokens, bootstrap capabilities, or IPC secrets;
- session IDs, agent payloads, snapshots, serialized blocks, or arbitrary debug representations;
- window/tab titles derived from user content in telemetry or crash reports.

## Emission Policy

- Default logs record lifecycle transitions, bounded failure categories, and threshold summaries only.
- No per-frame, per-terminal-byte, or repeated per-layout warning is permitted.
- The same orphaned transient item produces at most one repair result for one detection generation.
- Detailed per-window bucket snapshots are available only through test/debug tooling and are size-limited.
- Multi-process logs use existing PID-qualified or coordinator-owned destinations; a short-lived forwarding process must not rotate or truncate the active GUI worker's log.
- Queue overflow increments one counter and drops measurements; it never blocks input or rendering.

## Retention

- In memory: 60 seconds per live logical window.
- On worker close: one final bounded aggregate may be sent to the coordinator, then worker buckets are released.
- On disk: no new raw frame-event log. Optional benchmark output is explicitly requested, stored under a disposable test artifact directory, and bounded by the test harness.

## Threshold Summaries

A content-free summary may be emitted when:

- idle CPU/frame acceptance fails in a controlled benchmark;
- a worker exceeds its background presentation cadence;
- first-frame or focus catch-up exceeds its deadline;
- a crash budget is exhausted;
- one-writer instrumentation detects a violation;
- layout reconciliation cannot restore consistency within its bounded retry.

Each transition is rate-limited by logical window and outcome category.

## Verification

Tests MUST prove:

- fixed memory/cardinality under a simulated million wake events;
- saturation and dropped counters without blocking;
- no dynamic labels from terminal or path data;
- one summary rather than repeated warnings for a persistent stale item;
- concurrent workers cannot rotate/truncate each other's active diagnostic stream;
- benchmark artifacts stay below their declared retention limit.
