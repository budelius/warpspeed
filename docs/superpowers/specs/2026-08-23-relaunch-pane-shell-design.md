# Relaunching a Pane's Shell — Design

**Goal:** A pane can restart its shell in place, or switch to a different shell, keeping its scrollback, its working directory, and its agent conversation.

**Status:** Approved, in implementation. The Architecture section carries a correction made after the first implementation attempt disproved its original phase 3.

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

1. **Guard.** If `block_list().active_block().is_active_and_long_running()` (`app/src/terminal/input.rs:5016`), show what is running and confirm. Otherwise proceed silently. A running CLI agent is a foreground process, so it takes this path too.
2. **Capture, then tear down.** Read the live working directory and the agent resume payload *before* killing the PTY — both become unreadable afterwards. Mark the teardown as deliberate, then end the session.
3. **Build fresh resources and start.** Construct a new set of per-session event-loop resources, and start a session on the target shell in the translated directory.

### Corrected after implementation

Phase 3 originally read *"Re-park and start"*, and this section claimed it was "already written and reviewed" machinery. **That was wrong**, and the first attempt at implementing it is what found out. Two facts, each verified in the code:

- `ShellStartupResources` (`terminal_manager.rs:170-176`) holds a `mio_channel::Receiver<Message>`. It is constructed at exactly one site — `:510`, inside `create_model_with_manager` — and consumed at `:830`. A re-parked `DeferredShellStart` can never be handed valid resources; a second session needs a fresh channel pair and a re-wired `PtyController`. `DeferredShellStart` was built for a pane that has *never* started.
- Ending a session fires `ModelEvent::Exit`, and its handler (`app/src/terminal/view.rs:11868-11916`) inserts a "process terminated" banner, disables the input editor, and emits `Event::Exited` — which `terminal_pane.rs:976-989` turns into `close_pane`. The first production call would have closed the pane the feature exists to preserve. `TerminalModel::handled_exit` also latches with no reset anywhere (`terminal_model.rs:1089` false at construction, `:1490` true, nothing else), freezing `is_read_only()` at true.

`manual_pty_shutdown_requested` (`view.rs:2946`) looks like the answer and is not: it gates only the telemetry branch at `:11869`, and its own comment says the pane is torn down afterwards.

**So the new work is two primitives, not one:** a teardown a pane can survive, and a way to construct session resources more than once.

## Components

**Deliberate-teardown marking** — `TerminalView::begin_restart` / `finish_restart`, plus `TerminalModel::reset_exit_state`. Between them they suppress the three teardown effects (banner, editor disable, `Event::Exited`) for an exit that is a restart rather than a death, and clear the exit latch so the replacement session is writable.

**`build_session_resources`** — extracts the per-session event-loop construction out of `create_model_with_manager` so a restart can build a second set.

**`restart_session(target, cwd, ctx)`** on `TerminalManagerTrait` — the single entry point, defaulted to a no-op exactly as `ensure_shell_started` is, with the body living once in `TerminalManager` and both impls forwarding to it. `target: None` means "same shell". `cwd` arrives in **host** form and is converted inside, because only this method knows which shell is starting.

**`WorkspaceAction::RestartShell` and `WorkspaceAction::ChangeShell`** — both call `restart_session`; the second opens the shell picker first. Wired like the existing `AddTabWithShell`.

Neither command gets a default keybinding in this work. Restart is the one that would benefit, but every unclaimed chord is a collision risk against user bindings and platform conventions, and the command palette is sufficient to use the feature. Assigning one is a separate, reversible decision.

**Shell picker** — reuses the list the new-tab menu already builds (`app/src/workspace/view.rs:6931`): `AvailableShells::handle(ctx).get_available_shells()`, with `display_name_for_shell` for labels and `ShellIndicatorType` for icons, gated on `FeatureFlag::ShellSelector`. Building a second list would let the two drift.

**Confirmation modal** — follows the established pattern: a `ModalViewState<Modal<T>>` field with `ctx.add_typed_action_view(..)`, as `TransferOwnershipConfirmationModal` does (`app/src/settings_view/transfer_ownership_confirmation_modal.rs`). It names the running command so the choice is informed, rather than asking a generic "are you sure".

**Reverse path converters** in `crates/warp_util/src/path.rs`. Less new code than it first appears — the forward WSL converter has two branches, and one already has an inverse:

| Forward (`convert_wsl_to_windows_host_path`) | Inverse |
| --- | --- |
| `/mnt/<drive>/…` → `<DRIVE>:\…` | **New.** `<DRIVE>:\…` → `/mnt/<drive>/…` |
| anything else → `\\WSL$\<distro>\…` | **Exists:** `parse_wsl_unc_path` (`path.rs:537`), which handles the `wsl$`, `wsl.localhost`, verbatim-UNC and forward-slash spellings, case-insensitively |

So the WSL work is one new branch plus a dispatcher that routes to it or to `parse_wsl_unc_path`. The MSYS2 reverse has no counterpart and is genuinely new.

One asymmetry the inverse cannot undo: the forward converter calls `std::fs::read_link` on the drive-letter branch (`path.rs:492-495`), so a `/mnt/c/…` path may come back already resolved through a symlink. Round-tripping such a path will not return the original.

## Data flow

### Working directory

`pwd_if_local()` yields a **host** path: the existing converters normalise shell→host, and `TerminalPaneSnapshot.cwd` is stored in host form.

`startup_directory` for a WSL shell is in **WSL-native** form. Verified: `on_shell_determined` (`terminal_manager.rs:713-719`) substitutes `wsl_shell_starter.home_directory()` only when `startup_directory` is `None`, and otherwise passes the value through untouched. Handing a host path to a WSL shell would therefore land the user in the wrong directory, or fail outright.

So the flow is: `pwd_if_local()` → host path → convert to the target shell's native form → the new session's startup directory. The conversion is the identity for `Executable` shells, and the new converters for WSL and MSYS2.

### Agent resume

Capture `ResumeOptions::ThirdParty(ResumePayload)` (`app/src/ai/agent_sdk/driver.rs:452`) before teardown. Relaunch only once the new shell has bootstrapped.

That sequencing already exists and should be reused rather than rebuilt: the pending-command queue is a "run this once the shell is bootstrapped" primitive. `set_pending_command_queue` parks a command, and `execute_pending_command` refuses to submit until `block_list().is_bootstrapped()`. Task 1 of the lazy-restore work is what made that queue survive deferral. Agent resume enqueues like any other startup command.

## Error handling

**Teardown does not complete.** Originally specified as: abort the relaunch if teardown fails. **Dropped, deliberately.** `shutdown_event_loop` takes the join handle out of its `Option` *before* attempting the join and never restores it on failure, so any boolean success signal is unconditionally true and the guard would be vacuous. A genuine signal means propagating the `join()` `Result` — worth doing, but it is not what makes this feature work, and faking it would be worse than recording it. Tracked as a follow-up.

**The new shell fails to start.** The existing failure path applies — the "shell unsupported" banner and fallback-shell selection in `on_shell_determined`. A failed *switch* must not leave the pane shell-less: the pane ends up either on the requested shell or on the fallback, never parked forever.

**Directory no longer exists.** The captured directory may have been deleted, or may not resolve in the target shell (a Windows path with no WSL mount). Fall back to the target shell's default — `home_directory()` for WSL, the existing default otherwise — and say so in the pane rather than failing the relaunch.

**Resume against a truncated transcript.** Killing an agent mid-turn truncates its transcript. `rehydrate_claude_transcript` (`claude_code.rs:282`) reconstructs the envelope. Resume must mean *resume from the last complete turn*; a partial turn is exactly the case that would corrupt a resume, so this needs verifying against the harness rather than assuming.

**No agent was running.** Relaunch is a plain restart. No resume, no error.

## Testing

Unit tests observe model state, not real PTYs — the limit inherited from the lazy-restore work.

- Restarting a pane with no running process tears down and starts a fresh session without a prompt, and the pane stays open.
- Restarting with a long-running block set prompts, and declining leaves the session untouched.
- The blocklist is identical across a relaunch — same block count, same conversation id. This is the guarantee the whole design rests on.
- Switching shells starts the requested `ShellLaunchData`, not the previous one.
- The captured directory reaches the new session in the target shell's native form. Table-driven across Executable / WSL / MSYS2.
- The new path converters round-trip against the existing forward converters, and handle UNC paths, drive letters, and non-`/mnt` mount roots.
- A pane with no agent relaunches without attempting resume.

**Known gap, inherited:** no test asserts a shell actually *started* — assertions observe parked state flipping. A relaunch that tore down and never respawned would still pass. Closing this needs a new observable and belongs with the sidebar-indicator work, not here.

## Risks

**Teardown is the whole risk.** Everything else composes from reviewed code. A teardown that leaks the PTY, double-fires the exit handler, or races the new session's spawn produces symptoms — a zombie process, a pane that reports exit twice, a shell that starts and immediately dies — that are timing-dependent and awkward to test.

**Mid-turn resume is unverified.** The behaviour of `rehydrate_claude_transcript` against a transcript truncated mid-turn must be checked, not assumed.

**The reverse path converters sit on a correctness-critical path.** Half the WSL case already exists (`parse_wsl_unc_path`), and the rest has a symmetric counterpart to mirror, which bounds the work. But a wrong conversion silently lands the user in the wrong directory rather than failing loudly, and the `read_link` asymmetry means round-trip tests cannot be the only check.
