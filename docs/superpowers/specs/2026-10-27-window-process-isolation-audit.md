# WarpSpeed window-process isolation ownership audit

**Date:** 2026-10-27

**Purpose:** Establish the shared-write and process-global behavior boundaries that must be split before a WarpSpeed window may run as a separate GUI worker. This is the Task 0 gate for `docs/superpowers/plans/2026-10-27-window-process-isolation.md`.

## Current process model

`LaunchMode::App` currently performs all of these jobs in one process:

1. claims the effective AppId/profile;
2. owns Windows URI forwarding or Linux/FreeBSD D-Bus forwarding;
3. initializes settings, secure storage, auth, cloud synchronization, telemetry, autoupdate, and SQLite;
4. creates every top-level window in one WarpUI `AppContext`;
5. writes one process-wide `AppState` containing all windows;
6. owns one remote-control endpoint for all windows;
7. connects every terminal view to a build-scoped terminal supervisor.

The existing `window_process_handoff` child is another complete `LaunchMode::App`. It is therefore a competing profile writer, not a renderer/helper, and remains correctly rejected for WarpSpeed.

## Shared writable resources

| Resource | Current initialization/writer | Mutation boundary | Required isolated ownership |
| --- | --- | --- | --- |
| SQLite `warp.sqlite`, WAL, SHM, schema | `persistence::initialize`, `sqlite::setup_database`, writer thread | `SyncSender<ModelEvent>` | Coordinator only; workers use a bounded mutation client and optional read-only connection after schema epoch is established. |
| Persisted application/window state | SQLite writer | `ModelEvent::Snapshot(AppState)` | Coordinator aggregates one snapshot per worker and writes the complete state. |
| Crash/session recovery | `session_recovery::{begin,save,clean_exit}` | `ModelEvent::RecoverySnapshot` plus direct atomic files | Coordinator claims and writes recovery files; worker snapshots are inputs. |
| Launch-config sessions/history | `launch_configs::session_store`, `session_configs` | `ModelEvent::SaveLaunchConfigWindowSessions`, lease files | Coordinator writes catalogs and arbitrates leases. |
| Public/private preferences | settings `UserPreferences`, `Setting::set_value` | Generic preferences backend | Coordinator-backed preferences adapter; worker reads cache and proxies writes. |
| Settings TOML/imports | `settings::init`, imported-config paths | Generic settings writer and direct import actions | Coordinator performs writes/imports and broadcasts revisions. |
| Secure storage | platform registration in `initialize_app` | secure-storage trait | Coordinator is authoritative writer/refresher; workers receive bounded in-memory auth state or proxy operations. |
| Auth refresh/current user | `AuthState`, `AuthManager`, `StoznAuthManager` | secure storage and `ModelEvent::UpsertCurrentUserInformation` | Coordinator owns refresh/persisted identity; worker mirrors state. |
| Cloud objects/workspaces/actions | cloud object models and update manager | many `ModelEvent` variants | Coordinator owns sync and writer; worker sends owned mutation DTOs. |
| Command/block history | terminal/block models | `SaveBlock`, `DeleteBlocks`, `InsertCommand`, `UpdateFinishedCommand` | Coordinator writer; bounded serialized block records may cross persistence IPC but never logs/control metadata. |
| AI query/conversation/document state | AI models | `UpsertAIQuery`, conversation/document variants | Coordinator writer; per-window models proxy mutations. |
| Projects, rules, LSP metadata, indexes | project/repo/LSP models | corresponding `ModelEvent` variants | Coordinator writer; worker-local watchers may propose mutations. |
| MCP installations/environment | MCP models | corresponding `ModelEvent` variants and process lifecycle | Coordinator owns persisted/global installation state; window-local UI is a client. |
| Autoupdate and executable cleanup | `autoupdate` in app lifecycle | direct filesystem/process operations | Coordinator only. |
| Telemetry upload and focus duration | `TelemetryCollector` and app callbacks | network queue/files | One coordinator uploader; workers send bounded content-free/event DTOs. |
| Prompt diagnostics | `PromptChipLogger` | PID-qualified bounded files | Per worker; already process-safe and not shared. |
| Application logs | `warp_logging` | per-AppId log files | Must become PID/role-qualified or use a process-safe sink before multiple workers. |
| Remote-control markers | `remote_control::address` | PID-qualified marker files | Per worker; coordinator provides aggregate discovery/routing. |
| Coordinator/ownership markers | app services | mutex/lock/D-Bus/pipe | Coordinator only. |
| Window-process handoff files | `window_process_handoff` | one-shot JSON and markers | Replace as authority with authenticated coordinator bootstrap; retain snapshot mechanics. |
| Agent session registry | `agent_session_registry` state files | direct atomic files | Audit indicates process-shared state; coordinator owns writes or records must remain independently keyed and atomic. |

## SQLite mutation inventory

All window-originated SQLite writes eventually use `ModelEvent`; no worker may retain a real writer-thread sender. The process protocol must cover these classes:

- terminal state: `SaveBlock`, `DeleteBlocks`, command start/finish, agent-view visibility;
- complete/launch app state: `Snapshot`, `RecoverySnapshot`, `SaveLaunchConfigWindowSessions`;
- Warp Drive objects: workflow/notebook/folder/generic string/workspace upserts, object metadata, retries, sync actions, deletions;
- user/settings cache: user profiles, current user, experiments, next refresh;
- AI: queries, multi-agent conversations, summaries, AI documents;
- code intelligence: codebase index metadata, projects, project rules, workspace language servers, ignored suggestions;
- MCP installation/environment/running state;
- database lifecycle: pause/remove/reconstruct/terminate.

`Box<dyn CloudStringObject>` and other trait-object variants cannot be sent across IPC directly. Before worker mode can be enabled, each crossing variant needs a bounded owned wire DTO or must be moved entirely into a coordinator-owned model. `Terminate`, schema reconstruction, and database removal are coordinator-local and must never be accepted from an arbitrary worker without a higher-level authorized request.

## Preference mutation inventory

`Setting::set_value` is used broadly across settings, onboarding, import, privacy, context/UI controls, and feature surfaces. Rewriting each call site is not viable. Isolation requires a generic preferences implementation with this behavior:

- worker reads from a revisioned local cache;
- worker write becomes a coordinator request with a bounded key/value payload and idempotency key;
- coordinator validates the setting namespace/type through the existing backend, writes it, and broadcasts a revision;
- a disconnected worker reports durable settings actions unavailable rather than writing the profile itself;
- initialization/default migrations run only in the coordinator.

The same pattern is required for secure storage; secrets and capabilities must not be logged or persisted in worker bootstrap files.

## Process-global window assumptions

The following source areas use `ctx.window_ids()`, `windows().active_window()`, or `WorkspaceRegistry::all_workspaces()` and require classification:

### Must route through the coordinator

- `app/src/lib.rs`: startup restoration, close/quit callbacks, app focus/blur, save-on-lifecycle, updater shutdown.
- `app/src/root_view.rs`: new/open/restored windows.
- `app/src/uri/mod.rs`: deep links, file/open intents, new window/tab routing.
- `app/src/menu.rs` and `app/src/app_menus.rs`: Window menu, active-window actions, app quit.
- `app/src/session_management.rs`: session/window selection.
- `app/src/remote_control/handler.rs` and `app/src/sessions_cli.rs`: enumerate windows/panes and quit/open operations.
- `app/src/workspace/cross_window_tab_drag.rs`: source/destination window lookup and tab transfer.
- `app/src/app_state.rs` and `workspace/global_actions.rs`: process-wide snapshot construction.
- `app/src/auth/mod.rs`: choosing a window for auth UI.
- `app/src/ai/conversation_navigation`, agent management, and notification toast routing.
- local-control metadata/resolution and MCP UI routing.

### Can remain worker-local

- active/focused pane and tab logic inside one `Workspace`;
- renderer/GPU state, input routing, window bounds/fullscreen, per-window panels;
- terminal model/view state and direct supervisor attachment;
- prompt/context-chip state;
- per-window dialogs and toasts once the target worker is chosen.

### Needs a cross-worker transaction

- tab drag/tear-out/adoption;
- undo-close when the closed tab/window belonged to another worker;
- synced input if its scope includes multiple windows;
- quake-window uniqueness;
- profile-wide quit and update restart;
- focus/window enumeration menus;
- session-config lease transfer;
- agent/conversation navigation to an already-open pane in another worker.

## Singleton model classification

### Coordinator-only

- profile ownership/app services;
- writable preferences and secure storage;
- SQLite writer and database lifecycle;
- app-state/recovery/launch-session catalogs;
- auth refresh authority and current-user persistence;
- cloud preference/object synchronization;
- telemetry uploader and profile focus aggregation;
- autoupdate/relaunch owner;
- worker registry and crash budget;
- global session-config lease registry.

### Worker-only

- GPU and OS window state;
- `Workspace`, pane groups, editor/terminal views;
- per-window prompt/context chip models;
- mouse/keyboard/focus and rendering models;
- PID-qualified prompt diagnostics;
- window-local remote-control endpoint.

### Replicated read-only with revision invalidation

- appearance/theme/settings values;
- auth/login state required to render;
- cached workspace/team metadata;
- experiments/feature state after coordinator resolution;
- command history/search inputs where a read-only SQLite view is cheaper than a bootstrap snapshot;
- projects/rules/MCP installation catalog.

### Proxied mutation clients

- `GlobalResourceHandles.model_event_sender`;
- public/private preference backend;
- secure-storage/auth mutation path;
- telemetry collector frontend;
- new/open/close/quit/focus window operations;
- global navigation and tab transfer;
- durable user actions against replicated models.

## Platform lifecycle constraints

### Windows

- The coordinator alone holds the profile-scoped mutex and URI named pipe.
- `command::windows::init` places the process in a kill-on-close Job Object. Worker creation must require `CREATE_BREAKAWAY_FROM_JOB`; the generic fallback without breakaway is unacceptable for isolation.
- Each worker creates its own job for ordinary non-supervised children. Supervised shells retain their existing breakaway/supervisor ownership.
- Named-pipe endpoints require same-user/session ACLs and peer validation.

### Linux/FreeBSD

- The coordinator acquires the profile-local advisory lock before state opens and alone owns the profile D-Bus name.
- Workers need independent process groups/session behavior so coordinator termination signals do not fan out.
- Internal Unix socket paths and files must reject symlinks and use user-only permissions.

### macOS

- The coordinator must be background-only and own profile coordination without a visible Dock window.
- Workers host visible windows. URL/open events delivered by LaunchServices to any process must be forwarded to the coordinator before execution.
- Services provider, login item, updater, menu ownership, activation/focus, and Dock behavior need real-display acceptance; they cannot be proven by unit tests.

## Existing mechanisms safe to reuse

- `WindowSnapshot`, agent resume descriptors, and handoff ready/release/abort mechanics after coordinator authentication.
- Per-process/session recovery claim and supersedes semantics.
- PID-qualified remote-control markers.
- Build-scoped terminal supervisor and `SessionHolder` metadata.
- Profile-scoped Windows/Linux ownership names.
- Bounded V2 restored-session and prompt diagnostic formats.

## Explicitly unsafe shortcuts

- enabling `permits_window_process_handoff()` for WarpSpeed without changing writer ownership;
- skipping ownership whenever a handoff environment variable exists;
- one synthetic `WARPSPEED_PROFILE` per window;
- letting workers independently run SQLite migrations or settings defaults;
- saving a one-window `AppState` directly from a worker;
- spawning Windows workers with fallback into the coordinator Job Object;
- routing raw live PTY streams through coordinator IPC;
- allowing different build identities in one worker pool.

## Implementation status

The feature-disabled foundation now provides:

- coordinator/worker/legacy roles with profile ownership limited to the coordinator;
- exact executable/profile/pool/worker identities, one-shot bootstrap capabilities, authenticated bounded frames, and replay rejection;
- current-user-only Windows named-pipe DACLs plus client/server PID and session validation;
- user-only Unix socket permissions and private one-shot bootstrap files;
- sequential worker restoration with first-frame readiness, heartbeat expiry, bounded restart budgets, and aggregate legacy-readable `AppState` snapshots;
- required Windows job breakaway and independent Unix process groups;
- read-only worker SQLite, preferences, secure storage, settings migration, cloud-preference synchronization, telemetry upload, autoupdate, app services, and log rotation;
- PID-qualified worker logs and a process-level one-window guard;
- coordinator-routed default new-window requests and startup/runtime URLs.

The feature remains disabled because workers still need generic coordinator-backed `ModelEvent`, preference, secure-storage/auth, and telemetry mutation clients. New-window variants that carry launch configurations, URI intent, shell selection, or cross-window state still need typed coordinator transactions. Cross-worker focus, tab transfer, undo-close, quake uniqueness, synced input, global navigation, profile-wide quit/restart, coordinator re-election, macOS lifecycle ownership, and disposable-profile process tests also remain required.

## Task 0 conclusion

The implementation is feasible only if role-specific initialization and generic persistence/preferences proxies land before the feature can spawn production workers. The highest-risk boundaries are generic `ModelEvent` conversion, preference/secure-storage proxying, macOS app lifecycle ownership, and cross-window tab transfer. The feature must remain disabled until instrumentation proves one writer PID for every protected profile resource and process-level tests prove workers survive coordinator and sibling failure.
