# Relaunching a Pane's Shell — Design

**Goal:** A pane can restart its shell in place, or switch to a different shell, keeping its scrollback, its working directory, and its agent conversation.

**Status:** Design approved. Not yet planned or implemented.

**Base:** `dev` at `7f2a6aa2`, which carries the merged lazy-session-restore work. This design depends on that machinery and cannot be built on a base without it.

## Scope

Two user-facing commands, one implementation path:

- **Restart shell** — relaunch the same shell. No menu; immediate.
- **Change shell…** — open the shell picker and relaunch with the chosen shell.

Out of scope, tracked separately: process-per-window, and opening a launch configuration in a new process. Those share no design surface with this.

## Decisions

| Question | Decision |
| --- | --- |
| Something is running in the pane | Warn and confirm, then kill. Prompt **only** when something is actually running. |
| Where the new shell starts | The pane's current directory, translated across shell types. |
| Agent behaviour | Auto-relaunch a CLI agent (Claude Code, Codex) resumed onto the same conversation. |
| Surface | Two separate commands, as above. |

## Architecture

A relaunch is three phases against one pane's `TerminalManager`. The pane's `blocklist` is never touched, which is what keeps scrollback and agent history safe — not by copying them, but by never putting them at risk.

1. **Guard.** If `block_list().active_block().is_active_and_long_running()` (`app/src/terminal/input.rs:5015`), show what is running and confirm. Otherwise proceed silently. A running CLI agent is a foreground process, so it takes this path too.
2. **Capture, then tear down.** Read the live working directory and the agent resume payload *before* killing the PTY — both become unreadable afterwards. Then end the session.
3. **Re-park and start.** Build a fresh `DeferredShellStart` carrying the target shell and the translated directory, park it, and call `start_deferred_shell`.

Phase 3 is already written and reviewed: it is the machinery merged in `7f2a6aa2`. Restart and switch-shell differ only in the `ShellLaunchData` phase 3 receives, so both commands are one code path.

**The new work is teardown.** `TerminalManager` tears down only via `Drop` → `shutdown_event_loop()` (`app/src/terminal/local_tty/terminal_manager.rs:512`). Nothing today ends a session while keeping the manager alive and re-parkable.

## Components

**`TerminalManager::end_session()`** — new. Extracts the body of `shutdown_event_loop` into a callable method that shuts down the event loop, joins its handle, and clears session state while leaving the manager alive. `shutdown_event_loop` already tolerates an already-dead channel. The constraint it does not yet carry: session-exit handling must not fire twice, once from teardown and once from the PTY dying.

**`TerminalManager::repark_shell(DeferredShellStart)`** — new. Sets `deferred_shell = Some(..)`.

The crate deliberately keeps **exactly one `Option::take` on `deferred_shell`** (`terminal_manager.rs:594-606`), so that deleting the shared body is a compile error at both call sites rather than silent recursion. This adds a write, not a take, so the invariant holds — but the new method belongs beside it, documented with the same reasoning.

**`restart_shell(target: Option<ShellLaunchData>, ctx)`** on `TerminalManagerTrait` — the single entry point. `None` means "same shell".

**`WorkspaceAction::RestartShell` and `WorkspaceAction::ChangeShell`** — both call `restart_shell`; the second opens the shell picker first. Wired like the existing `AddTabWithShell`.

Neither command gets a default keybinding in this work. Restart is the one that would benefit, but every unclaimed chord is a collision risk against user bindings and platform conventions, and the command palette is sufficient to use the feature. Assigning one is a separate, reversible decision.

**Shell picker** — reuses the list the new-tab menu already builds (`app/src/workspace/view.rs:6929-6940`): `AvailableShells::handle(ctx).get_available_shells()`, with `display_name_for_shell` for labels and `ShellIndicatorType` for icons, gated on `FeatureFlag::ShellSelector`. Building a second list would let the two drift.

**Confirmation modal** — follows the established pattern: a `ModalViewState<Modal<T>>` field with `ctx.add_typed_action_view(..)`, as `TransferOwnershipConfirmationModal` does (`app/src/settings_view/transfer_ownership_confirmation_modal.rs`). It names the running command so the choice is informed, rather than asking a generic "are you sure".

**Reverse path converters** — new `convert_windows_host_to_wsl_path` and `convert_windows_host_to_msys2_path` in `crates/warp_util/src/path.rs`, mirroring the existing forward pair at `:369` and `:465`.

## Data flow

### Working directory

`pwd_if_local()` yields a **host** path: the existing converters normalise shell→host, and `TerminalPaneSnapshot.cwd` is stored in host form.

`startup_directory` for a WSL shell is in **WSL-native** form. Verified: `on_shell_determined` (`terminal_manager.rs:712-719`) substitutes `wsl_shell_starter.home_directory()` only when `startup_directory` is `None`, and otherwise passes the value through untouched. Handing a host path to a WSL shell would therefore land the user in the wrong directory, or fail outright.

So the flow is: `pwd_if_local()` → host path → convert to the target shell's native form → `DeferredShellStart.startup_directory`. The conversion is the identity for `Executable` shells, and the new converters for WSL and MSYS2.

### Agent resume

Capture `ResumeOptions::ThirdParty(ResumePayload)` (`app/src/ai/agent_sdk/driver.rs:452`) before teardown. Relaunch only once the new shell has bootstrapped.

That sequencing already exists and should be reused rather than rebuilt: the pending-command queue is a "run this once the shell is bootstrapped" primitive. `set_pending_command_queue` parks a command, and `execute_pending_command` refuses to submit until `block_list().is_bootstrapped()`. Task 1 of the lazy-restore work is what made that queue survive deferral. Agent resume enqueues like any other startup command.

## Error handling

**Teardown does not complete.** `shutdown_event_loop` joins the event-loop thread and reports a failure to join. A relaunch must not proceed on a half-dead session: if the join fails, surface the error in the pane and leave the pane as it is, rather than parking a second shell over a live PTY.

**The new shell fails to start.** The existing failure path applies — the "shell unsupported" banner and fallback-shell selection in `on_shell_determined`. A failed *switch* must not leave the pane shell-less: the pane ends up either on the requested shell or on the fallback, never parked forever.

**Directory no longer exists.** The captured directory may have been deleted, or may not resolve in the target shell (a Windows path with no WSL mount). Fall back to the target shell's default — `home_directory()` for WSL, the existing default otherwise — and say so in the pane rather than failing the relaunch.

**Resume against a truncated transcript.** Killing an agent mid-turn truncates its transcript. `rehydrate_claude_transcript` (`claude_code.rs:279`) reconstructs the envelope. Resume must mean *resume from the last complete turn*; a partial turn is exactly the case that would corrupt a resume, so this needs verifying against the harness rather than assuming.

**No agent was running.** Relaunch is a plain restart. No resume, no error.

## Testing

Unit tests observe model state, not real PTYs — the limit inherited from the lazy-restore work.

- Restarting a pane with no running process tears down and re-parks without a prompt, and the new session starts.
- Restarting with a long-running block set prompts, and declining leaves the session untouched.
- The blocklist is identical across a relaunch — same block count, same conversation id. This is the guarantee the whole design rests on.
- Switching shells re-parks with the requested `ShellLaunchData`, not the previous one.
- The captured directory reaches `DeferredShellStart.startup_directory` in the target shell's native form. Table-driven across Executable / WSL / MSYS2.
- The new path converters round-trip against the existing forward converters, and handle UNC paths, drive letters, and non-`/mnt` mount roots.
- A pane with no agent relaunches without attempting resume.

**Known gap, inherited:** no test asserts a shell actually *started* — assertions observe parked state flipping. A relaunch that tore down and never respawned would still pass. Closing this needs a new observable and belongs with the sidebar-indicator work, not here.

## Risks

**Teardown is the whole risk.** Everything else composes from reviewed code. A teardown that leaks the PTY, double-fires the exit handler, or races the new session's spawn produces symptoms — a zombie process, a pane that reports exit twice, a shell that starts and immediately dies — that are timing-dependent and awkward to test.

**Mid-turn resume is unverified.** The behaviour of `rehydrate_claude_transcript` against a transcript truncated mid-turn must be checked, not assumed.

**The reverse path converters are new code on a correctness-critical path.** They have a symmetric counterpart and its tests to mirror, which bounds the work, but a wrong conversion silently lands the user in the wrong directory rather than failing loudly.
