# Quickstart: Validate WarpSpeed Multi-Window CPU Containment

## Prerequisites

- Use a clean checkout of `002-multi-window-cpu-containment` after the implementation commits are applied.
- Install repository build and test dependencies.
- Use only generated fixtures and an isolated `WARPSPEED_PROFILE`; do not restart, mutate, or benchmark the user's active profile.
- Keep window isolation disabled for feature-off compatibility checks and enable it only through the implementation's explicit test/local override for isolation checks.
- Normalize CPU as percentage of one logical core: `100%` means one fully occupied logical processor.

## 1. Focused deterministic tests

```bash
cargo nextest run -p warpui_core -E 'test(render_admission) or test(presentation_state) or test(deferred_repaint)'
cargo nextest run -p warp --lib -E 'test(window_process) or test(layout_reconciliation) or test(profile_ownership) or test(session_recovery)'
```

Expected:

- presentation-state transitions are idempotent;
- suppressed windows do not re-arm animation timers;
- background demand never exceeds 5 presentations per second under fake time;
- activation cancels stale timers and produces one latest-state frame;
- one coordinator restores one worker per window sequentially;
- stale generations and revisions cannot mutate current state;
- orphaned transient layout items are repaired once outside render-time model locks;
- feature-off behavior remains compatible.

## 2. Generate the disposable reference profile

Use the process-integration fixture generator added by this feature to create:

- 8 top-level windows;
- 95 tabs;
- 208 terminal panes;
- representative inactive progress/animation views;
- bounded persisted output with no copied user content;
- supervisor-backed long-running shell and test-agent sessions.

Record the fixture checksum, generated app-state size, build identity, graphics backend, display configuration, and logical CPU count in the benchmark artifact.

Expected: the fixture restores deterministically and contains no credentials, prompts, commands, repositories, or paths copied from a real profile.

## 3. Feature-off compatibility

Start the disposable profile with window isolation and render quiescence disabled.

Verify:

- the complete profile remains readable;
- existing ownership forwarding still creates one profile owner;
- window, tab, pane, and session counts match the generated fixture;
- no worker-only protected-resource writer is created;
- a clean shutdown writes state readable by the implementation with the feature enabled.

This run establishes a comparison baseline; it is not expected to meet the new CPU target.

## 4. Isolated restore and ownership

Enable the test/local override and start the disposable profile.

Expected after restoration:

- one headless coordinator owns the profile;
- eight GUI worker PIDs each own exactly one real workspace window;
- workers become ready sequentially;
- exactly one PID writes each protected profile resource;
- terminal supervisors remain outside GUI process fate;
- no extra blank window appears.

Repeat with ten simultaneous external launches and one different-build launch.

Expected: requests route to the coordinator, no second writer starts, and the version mismatch is surfaced without state corruption.

## 5. Idle and interaction benchmark

After all windows report first-frame readiness:

1. Leave terminal output idle for 30 seconds to settle.
2. Sample aggregate coordinator and worker CPU once per second for 60 seconds.
3. Record per-window requested/admitted/deferred/coalesced demands, scene builds, and presented frames.
4. Type a deterministic 500-character sequence into the focused test terminal and record input-to-present latency.
5. Repeat after focusing each of the eight windows.

Expected:

- aggregate idle CPU is at most 25% median and 50% p95 of one logical core;
- typed-character latency is below 50 ms p95 and 100 ms p99;
- unchanged background windows do not continuously build scenes;
- the artifact contains only contract-allowed content-free fields.

Exclude any explicitly separate profile, such as `SYS`, from this profile's aggregate.

## 6. Background output and presentation transitions

In one unfocused test window, start deterministic sustained output while another window remains active.

Verify:

- PTY/model output remains complete within existing persistence limits;
- the background worker presents no more than five frames per second;
- the active worker remains within the input-latency target;
- focusing the output window presents its latest state within 250 ms without replaying skipped frames.

Repeat while the output window is minimized, known occluded where supported, restored, moved between displays, and rapidly focused/unfocused.

Expected: minimized/occluded animation frames remain stopped, stale timers no-op, and one catch-up frame restores current state.

## 7. Stale layout reconciliation

Restore the generated checkpoint containing:

- an inline-banner block-list reservation with no render element;
- several distinct missing transient items;
- more missing IDs than one reconciliation batch to exercise bounded full reconciliation.

Expected:

- no blank reserved band remains after reconciliation;
- repair completes before the next stable frame;
- each distinct batch produces at most one content-free outcome;
- no per-frame `Missing banner element` warning appears;
- terminal-model lock instrumentation reports no nested acquisition.

## 8. Failure and recovery matrix

Run controlled test-only actions against one worker at a time:

1. hot render loop;
2. blocked main thread;
3. process crash before first frame;
4. process crash after session attachment;
5. coordinator crash with healthy workers;
6. coordinator loss during a window snapshot;
7. worker crash-budget exhaustion.

Expected:

- healthy windows continue accepting/displaying input within 100 ms;
- healthy workers do not exit with the failed process;
- one replacement coordinator eventually owns the profile after coordinator loss;
- each logical window appears at most once;
- existing shell and agent sessions attach exactly once;
- restoration stops safely if an unstarted worker cannot establish isolation;
- crash-loop suppression retains a recoverable checkpoint.

## 9. Rollback and compatibility

Using only the disposable profile:

1. cleanly stop the isolated run;
2. disable the feature for the next clean launch;
3. restore through the legacy aggregate reader;
4. re-enable and restore again;
5. repeat across one supported upgrade and downgrade boundary;
6. inject interruption before and after aggregate replacement.

Expected: every cycle preserves all logical window structure and leaves eligible supervisor-managed sessions attachable. No mid-startup fallback creates a second writer.

## 10. Repository quality gates

```bash
cargo check --workspace
./script/format
cargo clippy --workspace --all-targets --all-features --tests -- -D warnings
cargo nextest run --no-fail-fast --workspace --exclude command-signatures-v2
cargo test --doc
git diff --check
```

Focused failures must be resolved before workspace-wide checks.

## 11. Cross-platform validation

After local deterministic checks pass, run the smallest representative desktop matrix:

- **Windows x64**: native minimize/occlusion, independent Job Object lifetime, idle CPU, one-writer proof, worker/coordinator crash.
- **macOS arm64**: background coordinator, foreground worker focus/Dock/menu behavior, occlusion/App Nap interaction, crash recovery.
- **Linux x64**: X11 and Wayland visibility fallback, process-group independence, D-Bus ownership, background cadence.
- **FreeBSD**: compile and focused Unix ownership/scheduler tests; GUI runtime if available.

Store bounded benchmark summaries with the exact commit. Do not substitute an ad hoc run against the active user profile.

## 12. Authorized installed-build restart

Run this section only after Sections 1-11 pass. The user explicitly authorized this final normal-profile restart on 2026-11-06.

1. Build and install without launching:

   ```powershell
   powershell -ExecutionPolicy Bypass -File .\script\windows\install-warpspeed.ps1
   ```

2. Record a preflight manifest containing:
   - expected normal-profile logical windows and their order;
   - titles, tab/pane counts, active window, and launch sources;
   - normal-profile GUI/coordinator/worker PIDs;
   - terminal supervisor PIDs/endpoints and eligible shell/agent session identities;
   - installed build identity and recoverable aggregate/checkpoint.
3. Confirm the installed build will enable isolation on the next clean launch and can create independent workers. Abort without shutting down if not.
4. Request graceful shutdown through the normal profile owner/coordinator. Do not terminate terminal supervisors, shells, agents, `OpenConsole`, or the independently owned `SYS` profile.
5. Launch through the installed non-elevated WarpSpeed launcher. Do not invoke the executable from an elevated harness, which would select `WarpSpeedAdmin`.
6. Wait for sequential readiness and verify:
   - exactly one normal-profile coordinator;
   - exactly one GUI worker PID per expected top-level window;
   - exactly one visible workspace window per worker;
   - no process containing all restored windows;
   - no additional profile writer.
7. Compare the restored windows and sessions with the preflight manifest. Every eligible shell and agent must remain alive and attach exactly once.
8. Run the 60-second CPU sample and focused typing-latency check. Verify SC-001 and SC-002 while excluding `SYS` from the normal-profile aggregate.
9. If isolation, ownership, restoration, or session continuity fails, stop further restoration, preserve supervisors and checkpoints, collect bounded diagnostics, and use clean-start rollback. Do not accept a legacy all-windows-in-one-process fallback for this validation.

Expected: all normal-profile windows return, each in its own isolated GUI process, with one coordinator and all eligible existing terminal/agent sessions preserved.
