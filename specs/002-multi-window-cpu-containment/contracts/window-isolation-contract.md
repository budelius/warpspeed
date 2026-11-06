# Window Isolation Contract

## Purpose

Define the ownership, startup, failure, and recovery guarantees for running each WarpSpeed top-level window in an independent GUI failure boundary while retaining one coherent profile.

## Roles

### Profile Coordinator

Exactly one per effective AppId/profile and user session. It owns:

- profile mutex/advisory lock and external launch endpoint;
- writable SQLite and profile files;
- app-state aggregation and recovery checkpoints;
- settings, secure-storage, auth-refresh, telemetry-upload, and update ownership;
- logical-window ordering, worker registry, crash budget, and routing;
- sequential restoration and profile-wide quit.

It does not own a visible workspace window or live PTY stream.

### Window Worker

Exactly one per restored top-level workspace window. It owns:

- one native top-level window;
- one GUI event/render loop and presentation scheduler;
- window-local views and terminal models;
- direct connections to the existing terminal supervisor.

It does not claim profile ownership or open protected shared resources for writing.

### Terminal Supervisor

Retains its existing authority over eligible shell processes, PTY state, session generations, and attachment leases. It does not become profile coordinator.

## Identity and Authentication

A worker registration MUST prove:

- effective profile identity;
- exact build identity;
- protocol version;
- current coordinator/profile generation;
- logical worker ID and worker generation;
- possession of a one-shot bootstrap capability;
- same-user local IPC authority.

Capabilities and session identities MUST NOT be logged. Bootstrap material is bounded, consumed once, and deleted or invalidated immediately.

## Existing Protocol Bounds to Preserve

- Maximum framed message: 320 KiB.
- Maximum transfer chunk: 256 KiB.
- Maximum chunks per transfer: 256.
- Maximum encoded window snapshot: 16 MiB.
- Maximum queued windows/commands: 128.
- Worker readiness timeout: 60 seconds.
- Heartbeat interval: 5 seconds; unresponsive threshold: 30 seconds.
- Crash budget: 3 restarts per 5-minute window.

Changing a bound requires updated tests, rationale in the plan, and decoded-side enforcement before allocation.

## Startup Contract

1. The coordinator claims profile ownership before shared state opens.
2. It reads and normalizes the complete aggregate once.
3. For each real window in stable order:
   - create or recover a stable logical worker ID;
   - create a bounded one-shot bootstrap;
   - spawn independently from coordinator process fate;
   - authenticate the worker;
   - transfer one bounded window snapshot;
   - require exactly one real window and first-frame readiness;
   - only then start the next worker.
4. If there are no saved windows, start one empty worker.
5. Failure stops subsequent restoration and retains all unstarted snapshots. It never falls back mid-startup to hosting remaining windows in the coordinator or one existing worker.

## Mutation and Snapshot Contract

- Workers send revisioned, bounded owned messages; no trait objects or arbitrary debug strings cross IPC.
- The coordinator acknowledges durable user-visible writes before success is shown.
- State-changing requests carry idempotency keys.
- A stale worker generation or snapshot revision cannot overwrite current state.
- Closing a worker commits either a final snapshot plus deletion intent or an explicit retained-recovery state; disappearance alone never deletes a window.
- The coordinator always writes a complete aggregate readable by the legacy path.

## Session Contract

- Live PTY bytes flow directly between a worker and terminal supervisor, never through coordinator control IPC.
- Worker recovery attaches to the existing session first.
- A replacement shell or agent is forbidden while prior ownership is uncertain.
- Session transfer uses prepare/commit/abort lease semantics.
- Stale worker generations cannot consume agent events or release current leases.

## Failure Contract

| Failure | Required outcome |
|---|---|
| Worker crash | Other workers remain alive and interactive; coordinator retains last acknowledged snapshot and applies bounded restart policy. |
| Worker hot loop/hang | Other workers retain independent scheduling; no automatic force-kill solely for missed heartbeat; user can request restart. |
| Coordinator crash | Workers remain alive in bounded degraded/read-only mode and re-register with one replacement coordinator. |
| Worker fails before ready | Restoration stops safely; snapshot remains recoverable; later windows are not started. |
| Wrong build/profile/token | Peer rejected before profile initialization or snapshot transfer. |
| Versioned external launch | Routed to current coordinator and surfaced as mismatch; no second writer starts. |
| Quit | Coordinator closes workers sequentially after durable final state, then releases ownership. |

## Platform Contract

- **Windows**: Workers must escape kill-on-coordinator-close Job Object fate; inability to use the required independent spawn mode is a startup failure.
- **macOS**: Coordinator remains background-only; workers own foreground windows without duplicating URL/service/login ownership.
- **Linux/FreeBSD**: Advisory lock and D-Bus ownership remain coordinator-only; worker process groups do not share coordinator termination fate.

## Acceptance Contract

For `N` restored windows:

- exactly one coordinator owns the profile;
- exactly `N` ready GUI workers own one real workspace window each;
- protected resources have one writer PID;
- killing/hanging one worker does not exit or block the other `N - 1` workers;
- recovery creates at most one instance of each logical window and session attachment.
