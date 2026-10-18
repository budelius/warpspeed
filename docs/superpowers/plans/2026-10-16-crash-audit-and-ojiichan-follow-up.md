# Crash audit and Ojiichan follow-up

## Scope and evidence

Audit snapshot: 2026-10-16. Reviewed project 90's complete issue/event inventory, retained GUI and
supervisor logs, the October 10 diagnostic archive, and Windows Application crash/hang reports for
the preceding 14 days. Also inspected the separate installed Warp, WarpOss, and WarpIntegration
logs. Rotated logs are incomplete historical evidence, not proof that unrecorded failures never
happened. Evidence is retained locally under
`T:/projects/logs/warpspeed-crash-20261016/`; raw logs and event payloads must not be uploaded or
committed. No production process was stopped or deliberately crashed.

The current installation observed during the audit was `20261015-194200986-4e7e6ae41e98-dirty`.
Merging fixes into `dev` does not update installed binaries or already-running instances.

## Confirmed crash fix

The October 15 08:46:39 UTC PTY-reader panic and 08:46:54 UTC main-thread panic occurred in
`0a2bde8116e4`. Restart Shell finished and truncated the old block, then made the model writable
without replacing that block. Startup bytes arriving before `InitShell` reached the finished grid.
A model regression reproduces the invalid grid index before the fix; this is not merely a bounds
check added to hide corrupt state.

Prepare a fresh bootstrap block before replacement PTY output can arrive. Preserve finished blocks
rather than finishing them twice: a second finish also overwrites their exit status and execution
state. Cover pre-initialization output, repeated pre-bootstrap restarts, initialization afterward,
old output/status preservation, and subsequent exit. Retain existing pane/restart/ownership tests.

The earlier Save/command-palette crashes were in `4bf1af503e32`; the action-deferral fix is already
on `dev`. They are not evidence of a new palette regression.

## Ojiichan inventory

Issue links use https://rustrak.pseekoo.io/projects/90/issues/ followed by the ID below.

| Issue ID | Events | Classification at audit time |
| --- | ---: | --- |
| `269a2ea9-58f2-4e1d-8782-ae6588acfc7b` | 2 | The confirmed Restart Shell panic cascade above, one process and two threads. |
| `c7b70d7a-8d95-4522-8f2f-cd8c772e4d9b` | 9 | Four disposable recovery-fixture invariant reports; five logged-out cloud authentication failures. Not nine crashes. |
| `e6d6283d-5c01-441b-813e-b3044a163bde` | 7 | Four disposable recovery tests; one recovery after the confirmed panic; two later unclean-session recoveries without an identified panic. |
| `78981181-bb24-4e90-8673-c687665baaca` | 1 | Historical manually submitted palette diagnostic, not an additional automatic crash. Its original release label predates the build-stamping correction. |

The four fixture errors at October 14 09:53:05 and 09:57:02 UTC were “more than one focused pane”
and “more than one active session.” The five remaining Error events correlate with
`CloudObjects::Listener websocket connection failed: missing authentication credentials`.
Event detail confirms that the current sanitized release reports have no useful source-file frames.
Do not resolve the mixed Error issue wholesale or treat recovery events as proof of panics.

## Follow-up work, in priority order

### P1 — Stop unauthenticated cloud subscriptions

Files: `app/src/server/cloud_objects/listener.rs`, `crates/warp_server_client/src/auth/session.rs`.

Status: implemented on `investigate/crash-followups`. Subscription intent survives logout, but the
websocket and pending refresh work stop until `AuthStateProvider` reports a logged-in user. Missing
credentials and skipped login are typed non-actionable errors. Focused lifecycle and auth-session
tests pass.

- Require usable credentials before starting or retrying a subscription; cached cloud objects alone
  must not start it while logged out. Preserve the desire to subscribe until login becomes possible.
- Observe authentication changes. Login starts at most one subscription; logout cancels the active
  subscription, delayed reconnects, and refresh work. Network/wake callbacks obey the same gate.
- Classify missing credentials with a typed non-actionable error where appropriate; do not suppress
  genuine authenticated transport/server failures or compare arbitrary error-message strings.
- Tests: logged-out startup with cached objects, login, logout during retry, and wake/network recovery.
  Acceptance: no unauthenticated connection attempts or corresponding Ojiichan issues; authorized
  updates still arrive. Use a test account/profile, not a production login.

### P1 — Make non-panic reports actionable without weakening privacy

Files: `crates/warp_errors/`, `app/src/crash_reporting/ojiichan.rs`.

Status: implemented on `investigate/crash-followups`. `report_error!` adds a synthetic call-site
frame; Ojiichan retains only the component, basename, line and column. SDK-path tests confirm that
payloads, private paths, attachments and debug-derived error types remain absent.

- Preserve a static report call-site location and an allowlisted component/error category. The panic
  hook location improvement in this changeset covers panics, not arbitrary `report_error!` failures.
- Group by safe code identity; never restore arbitrary exception values, formatted error messages,
  breadcrumbs, commands, prompts, user identity, environment variables, or absolute paths.
- Add release/no-debug-symbol and privacy regression tests through the real SDK and filtered
  transport. Verify one sanitized test event against Ojiichan before deployment acceptance.
- Separate synthetic diagnostics from production with a fixed test environment/category, never an
  arbitrary profile name. Repair the disposable recovery fixture to have one focused pane and one
  active session; its invalid-input reports are not production crashes.

### P1 — Close supervisor and shell observability gaps

Files: `app/src/lib.rs`, `crates/warp_terminal/src/local_tty/{supervisor,windows,unix_supervisor}`,
`crates/warp_terminal/src/local_tty/event_loop.rs`.

Status: implemented on `investigate/crash-followups` for in-process unexpected exits and Rust
panics. After exclusively claiming its local channel, each supervisor initializes Rust-only reporting
when a persisted opt-in marker exists. The marker is synchronized with the privacy setting and is
checked again before every headless event, so missing or removed state fails closed. Ojiichan retains
only fixed process/exit enums and bounded live-session state; revision remains the sanitized Sentry
release. Idle shutdowns are not reported. Control-channel loss, returned daemon errors and Rust
panics are reportable, without native minidumps, terminal bytes, session IDs, paths, code locations
or error payloads. Windows and Unix supervisor tests, crash-reporting tests and Unix idle-shutdown
runtime acceptance pass. Abrupt external process termination remains inherently unreportable from
inside that process; explicit shutdown intent and connection-generation correlation remain open.

- Supervisor startup currently bypasses GUI crash-reporting initialization. Its available logs show
  startup and client-requested kills, but no panic. Absence of remote reports is not evidence that
  supervisors cannot crash.
- Design a headless diagnostic initialization path that reads persisted privacy settings and fails
  closed if they cannot be read. Do not silently enable reporting for supervisors or shell children.
- Record bounded metadata for component, revision, exit cause/status, explicit shutdown intent and
  ownership/connection generation. Preserve no terminal output or process memory.
- Distinguish shell exit, user-requested restart/close, transport loss, supervisor disappearance and
  Rust panic. A nonzero shell exit alone is not a crash. Keep native minidumps disabled.
- If IPC needs new fields, handle protocol-version compatibility explicitly. Never start another
  agent to compensate for an uncertain owner.
- Disposable-process tests: normal close produces no crash report; intentional supervisor/pane
  failure produces the right diagnostic without stealing surviving sessions; privacy opt-out sends
  nothing; forced termination is distinguishable from a captured panic. Run Windows and Unix
  variants before claiming cross-platform coverage.

### P1 — Reattach and checkpoint reliability investigations

- The retained GUI logs contain 72 missing-session reattach warnings. A stale ID can mean an exited,
  expired, or different-supervisor session; it does not by itself establish a supervisor crash.
  Reproduce GUI loss with a surviving supervisor, build changes, expiration and system restart.
  Verify exact ownership and protocol selection before changing fallback behavior. Coordinate with
  the separate `fix/attach-first-frame` work rather than duplicating or overwriting it.
- One October 15 08:31:18 UTC saved-session replacement failed with Windows access denied. A
  disposable-file regression reproduced error 5 by holding the existing catalog open without delete
  sharing. Atomic replacement now retries only access-denied, sharing-violation, and lock-violation
  errors with bounded backoff totaling at most 310 ms. The destination remains untouched until a
  rename succeeds, and permanent failures still return an error after the retry budget.
- Acceptance: no duplicated agents, no lost old checkpoint on failed writes, and no false success
  when the new checkpoint was not persisted.

### P2 — Bootstrap ordering, log volume, and external shell exceptions

- Status: root-caused on `fix/bootstrap-hook-recovery`. The failed Git Bash session received all 24
  key-value updates and the end marker for its `Bootstrapped` hook, but not the start marker. The
  Windows attachment relay discarded an overlapped read if it completed between the timeout and
  cancellation. The fix preserves those bytes and covers the cancellation race directly.
- One October 16 13:04:42 UTC `next_stage` error and bootstrap timeouts need replay/order tests for
  restored sessions. Unknown-session and duplicate-hook warnings are not proven shell crashes.
- Three prompt generators repeatedly fail to prepare execution context, flooding logs. Gate work on
  session readiness and rate-limit repeated diagnostics without hiding the first actionable failure.
- Port 9280 conflicts occur with multiple GUI instances. Define ownership/fallback behavior and
  classify an existing legitimate listener separately from unexpected bind errors.
- Windows recorded two PowerShell `System.InvalidOperationException` / `System.Console.ReadKey`
  reports at October 14 07:58:52 UTC. No retained parent-process evidence attributes them to Warp.
  Reproduce redirected-input versus interactive-console cases before assigning a Warp fix.
- The separate installed Warp/WarpOss logs contain snapshot-index synchronization and telemetry
  queue-limit errors, but no additional recorded Rust panic. WarpIntegration contains an earlier
  command-palette assertion failure, not a production crash. Keep vendor Warp diagnostics and test
  harness failures separate from WarpSpeed project 90; do not redirect their reporting or modify
  their installation as part of this fix.

## Release and closure gates

1. Run focused regressions, existing restart/pane/supervisor tests, formatting and all three Clippy
   commands from `script/presubmit`, plus the crash-reporting feature configuration.
2. Merge only passing changes through the private repository's `dev` workflow. Run Linux CI; record
   that it does not replace Linux/macOS GUI and supervisor runtime acceptance. Oz runner discovery
   is unavailable locally (`oz-dev` is not installed).
3. Build/install only when requested. Verify installed and actually-running revisions separately;
   do not close live windows or kill supervisors as an upgrade mechanism.
4. Validate a disposable Restart Shell flow and sanitized reporting from the new release. Monitor
   new events by release and source location. Close the panic issue only after release acceptance;
   keep mixed/unclassified historical issues documented rather than calling all of them fixed.

For subsequent Ojiichan arrivals, retrieve all event pages, correlate event time/revision/component
with locally retained evidence, and distinguish a new failure from an old running executable or a
second event in the same failure cascade. Add each genuinely new cause to this queue with severity,
a failing regression or explicit reproduction blocker, an owner, and a release acceptance criterion.
Do not automatically resolve a report merely because a candidate fix merged.
