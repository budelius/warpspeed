# Crash recovery and Ojiichan reporting

With **Settings → General → Restore session** enabled, startup restores saved windows and
recovers interrupted named sessions even if another WarpSpeed instance is still running.
An additional process still starts empty when there is nothing interrupted to recover.

Every GUI process checkpoints its own windows under `state_dir()/session-recovery`. Checkpoints
retain pane layouts, shell/CWD metadata, persistent PTY identities, and agent resume descriptors.
Terminal history continues to use the existing SQLite and named-session stores. A process holds
an exclusive file lock for its lifetime; recovery skips live or already-claimed instances. Source
checkpoints are retained until startup has completed and the replacement checkpoint is durable.
A normal exit removes the process's recovery checkpoint. Unreadable checkpoints are left intact.

Named sessions also use their existing session ownership locks. Recovery must not clone, steal,
or terminate a session held by another instance. Surviving supervised terminals are reattached;
otherwise existing resume metadata is used. A terminated process cannot be resurrected, and
metadata that was never saved cannot be recovered. Disable Restore session temporarily if a
particular saved window triggers a repeatable startup crash.

## Ojiichan

Build WarpSpeed with `--features crash_reporting` (the Windows installer includes it). Reporting
uses Ojiichan's **Rustrak/Sentry-compatible ingest DSN**, not a management API token. Configure
one of these, in priority order:

1. `WARPSPEED_OJIICHAN_DSN`
2. `OJIICHAN_SENTRY_DSN`
3. A UTF-8 `ojiichan-dsn.txt` in `warp_core::paths::config_local_dir()`.
   For the standard Windows profile this is
   `%LOCALAPPDATA%\warp\WarpSpeed\config\ojiichan-dsn.txt`.

An explicitly empty environment override disables the destination. Invalid configuration disables
reporting without logging the DSN. Each profile's file is independent; no DSN is checked into the
repository or embedded in the executable. Provision a project with Ojiichan and store only its
project-specific ingest DSN here. Existing **Privacy → Crash reporting** opt-out is respected.

Rust panics and actionable `report_error!` events use the existing Sentry integration. Reports
contain build revision, platform/environment, exception categories and code locations. Event messages,
exception values, user identity, requests, breadcrumbs, local variables, absolute paths, and other
arbitrary context are excluded. Native memory dumps are disabled because they can contain terminal
content or credentials. Recovery emits a minimal interrupted-session event on the next launch,
including after a forced exit or power loss; that is not proof of a Rust panic. Delivery is
best-effort and requires network access. Ordinary warnings are not promoted to remote issues.

## Verification

- Unit tests: checkpoint locking, recovery beside a live process, concurrent recovery, another
  crash during startup, failed writes, corrupt checkpoints, clean exits, and report sanitization.
- Windows runtime acceptance: use a disposable `WARPSPEED_PROFILE`; open multiple windows with
  tabs/splits and an agent with a known resume ID, force-terminate only that test GUI, leave a
  second test GUI running, relaunch, and verify every recoverable window returns exactly once.
  Repeat with two simultaneous relaunches. Never kill a production GUI or its supervisor for this.
- Repeat filesystem/ownership tests on Linux/macOS before claiming cross-platform runtime coverage.
