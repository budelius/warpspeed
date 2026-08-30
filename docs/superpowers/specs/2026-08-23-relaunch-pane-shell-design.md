# Relaunching a Pane's Shell — Design

**Goal:** A pane can restart its shell in place, or switch to a different shell, keeping its scrollback, its working directory, and its agent conversation.

**Status:** Shipped, across `afcf7ea3`, `63f43d47`, `015fb98e`, `0c75afd1`, `eeb68f5a`, `cde0a1df`, `21273eee`, `423d523d`, `5776a675`, `a6f35719`, `27feec5d`, `6c8a4cde`. The Architecture section carries the corrections implementation forced: the first attempt disproved its original phase 3, and three parts of the shipped design — the restart barrier, the generation guard, and where the working directory is converted — were not in the original at all.

**Base:** `dev` at `7f2a6aa2`, which carries the merged lazy-session-restore work. This design depends on that machinery and cannot be built on a base without it.

## Scope

Two user-facing commands, one implementation path:

- **Restart shell** — relaunch the same shell immediately.
- **Change shell…** — open the shell picker and relaunch with the chosen shell.

Both are available from the command palette, the pane-header overflow menu, and terminal tab/pane context menus.

A follow-up command, **Restart window in new process**, snapshots the selected window and restores it in a newly launched process. It deliberately uses a separate handoff path rather than composing pane-level shell restarts.

## Decisions

| Question | Decision |
| --- | --- |
| Something is running in the pane | Warn and confirm, then kill. Prompt **only** when something is actually running. |
| Where the new shell starts | The pane's current directory, translated across shell types. |
| Agent behaviour | Auto-relaunch a **Claude Code** agent onto a fork of its conversation. Claude only — `resolve_agent_resume_command` returns `None` for every other `CLIAgent`, Codex included. |
| Surface | Two separate commands in the command palette and terminal context menus. |

### Window process restart

`Restart window in new process` writes a versioned `WindowSnapshot` handoff under the application's state directory and launches a replacement executable. Installed WarpSpeed processes resolve that executable through `%LOCALAPPDATA%\Programs\WarpSpeed\current.txt`; development builds fall back to their current executable when no installed replacement exists. The command has no default keybinding and is available from the command palette and terminal tab context menus.

The handoff is two-phase. The replacement restores only the handed-off window with every shell deferred, restores built-in agent conversations, queues resumable Claude CLI-agent commands, and writes a ready marker. Only then does the original process preserve supervised Unix PTYs, synchronously stop the old pane event loops, and write the release marker. The replacement starts its deferred shells after observing release, allowing supervised PTYs to reattach without racing their old owners. On platforms without supervised PTY transfer, shells restart from their captured shell and working-directory metadata; resumable Claude sessions continue from a forked transcript.

The original window remains open if the replacement cannot launch or acknowledge readiness. After release, the original closes with `TerminationMode::ContentTransferred`; if it was the process's final window, the original process terminates. Handoff files use unique names, owner-scoped state storage, bounded waits, and are removed after completion or failure.

## Architecture

A relaunch is three phases against one pane's `TerminalManager`. The pane's `blocklist` is never touched, which is what keeps scrollback and agent history safe — not by copying them, but by never putting them at risk.

1. **Guard.** If `block_list().active_block().is_active_and_long_running()` (`Block::is_active_and_long_running`, `app/src/terminal/model/block.rs`), show what is running and confirm. Otherwise proceed silently. A running CLI agent is a foreground process, so it takes this path too.
2. **Capture, then tear down.** Read the live working directory and the agent's tracked session id *before* killing the PTY — both become unreadable afterwards. Mark the teardown as deliberate, then end the session.
3. **Build fresh resources and start.** Construct a new set of per-session event-loop resources, and start a session on the target shell, handing it the captured directory in **host** form. The translation to the target shell's native form happens later, in `on_shell_determined` — see *The cwd relocation* below.

The shipped sequence lives in `TerminalManager::restart_session_inner` (`app/src/terminal/local_tty/terminal_manager.rs`) as eight numbered steps whose order is load-bearing; the three subsections after the correction below explain the parts of that order that are not obvious.

### Corrected after implementation

Phase 3 originally read *"Re-park and start"*, and this section claimed it was "already written and reviewed" machinery. **That was wrong**, and the first attempt at implementing it is what found out. Two facts, each verified in the code:

- `ShellStartupResources` holds a `mio_channel::Receiver<Message>`. It was constructed at exactly one site — inside `create_model_with_manager` — and consumed in `on_shell_determined`. A re-parked `DeferredShellStart` can never be handed valid resources; a second session needs a fresh channel pair and a re-wired `PtyController`. `DeferredShellStart` was built for a pane that has *never* started.
- Ending a session fires `ModelEvent::Exit`, and its handler (the `ModelEvent::Exit` arm of `TerminalView::handle_terminal_event`) inserts a "process terminated" banner, disables the input editor, and emits `Event::Exited` — which `TerminalPane`'s `Event::Exited` arm turns into `close_pane`. The first production call would have closed the pane the feature exists to preserve. `TerminalModel::handled_exit` also latched with no reset anywhere, freezing `is_read_only()` at true.

`manual_pty_shutdown_requested` (`TerminalView::shutdown_pty`) looks like the answer and is not: it gates only the telemetry branch of the same arm, and its own comment says the pane is torn down afterwards. `restart_session_inner` step 1 therefore deliberately avoids `shutdown_pty`: that flag stays true for the view's whole lifetime, which would permanently suppress `AgentExitedShellProcess` telemetry for every genuine agent-caused exit in the pane afterwards.

**So the new work is two primitives, not one:** a teardown a pane can survive, and a way to construct session resources more than once. `TerminalManager::build_session_resources` is the second; it is now called from both `create_model_with_manager` and `restart_session_inner` step 6, and the latter also repoints the `PtyController`'s own clone of the sender, which otherwise addresses the event loop just joined.

### The restart barrier

Suppressing the teardown is easy. Knowing *when to stop suppressing* is the hard part, and it is not solvable on a timeline.

`ModelEvent::Exit` reaches the view asynchronously — `TerminalModel::exit` → the event-proxy channel → the singleton dispatcher → `TerminalView::handle_terminal_event`. So the obvious shape, `begin_restart(); spawn the replacement; finish_restart();`, is wrong: the replaced shell's own `Exit` can still be sitting in the channel when the mark is cleared, at which point it is treated as a real exit and closes the pane — the exact failure the feature exists to prevent, reintroduced by call-site ordering.

The fix is a marker sent down the same channel. `TerminalModel::exit_for_restart` posts an `Event::Exit { reason: ExitReason::Restarting }` — a reason the PTY event loop never produces, so it can only ever mean "deliberate replacement". `restart_session_inner` posts it in step 5, *after* step 2 has joined the event loop thread. That ordering is what makes it a proof rather than a hope: the event loop's last act before its thread ends is `TerminalModel::exit`, so once the join returns, whatever `Exit` the replaced session queued is already in the channel and nothing more can be queued behind it. Everything travels one FIFO channel, so a view that observes the barrier knows every event the replaced session produced has already been delivered — and suppressed.

The `ModelEvent::Exit` arm therefore classifies the barrier *before* it consults `restart_in_progress`, and calls `TerminalView::finish_restart` only on seeing it. That is the only site in the crate that can establish `finish_restart`'s documented precondition. Two other details fall out of the same design:

- Back-to-back restarts can put two barriers in the channel at once. The first clears the flag; the second is still classified as a barrier and returned on, rather than becoming an ordinary exit that inserts a banner and emits `Event::Exited`.
- `exit_for_restart` posts exactly one barrier whether or not the replaced shell had already exited. When `handled_exit` is already set it sends the bare event rather than going through `exit`, because `exit`'s latch would swallow it entirely and un-latching first would re-run the teardown and its lifecycle telemetry for a single session. Either way it leaves the model un-latched, because the replacement writes into that same model.

`finish_restart` also re-enables the input editor. `reset_exit_state` restores model writability, but the editor's `InteractionState` is separate state that only the exit handler sets and nothing else clears.

Commits `afcf7ea3`, `63f43d47`. Observable in tests through `TerminalView::is_restart_in_progress`.

### The generation guard

A restart can land while the *previous* session's shell start is still resolving. `ShellStarter::init`'s async resolution is not instantaneous — for a WSL shell it shells out to `wsl … printenv SHELL`, which is seconds, and for every eager pane it is at least one tick.

If that superseded start were allowed to run on to `on_shell_determined`, it would fork a PTY for a session that no longer exists and install its `event_loop_handle` **over** the live one — detaching that thread, orphaning its shell process for the app's lifetime, and interleaving two shells into one blocklist. Nothing in production observes this: no log line, no error, just a stray shell.

`restart_session_inner` step 3 bumps `TerminalManager::session_generation`. `spawn_shell_start` captures the counter's value at spawn time, and its callback compares before doing anything else, discarding itself with a log line when the manager has moved on. Step 3 runs after step 2's join, and `join` blocks the main thread, so no spawned callback can be dispatched in between.

The manager also retains the in-flight start as `pending_shell_start` and aborts it in step 3 — but that is **best effort only**, and is not what makes this correct. `ModelContext::spawn` honours an abort at the future's next poll, so a start whose future has already resolved still delivers its callback. The counter is the guarantee; the abort merely stops background work early where it still can. Step 3 also clears `deferred_shell`, since a parked start is superseded too and would otherwise fork a second shell the next time the pane is activated.

Because production has no symptom to assert on, the guard needs a test-only hook: `TerminalManager::established_session_count` counts event loops installed over the manager's lifetime, and `test_two_restarts_in_flight_establish_only_one_session` (`app/src/pane_group/mod_tests.rs`) is the assertion.

Commits `63f43d47`, `015fb98e`.

### The cwd relocation

The original design put the host→native directory conversion inside `restart_session`, on the reasoning that "only this method knows which shell is starting". That reasoning is false, and shipping it produced a bug.

`restart_session_inner` step 7 resolves a `ShellStarter` — but a `ShellStarter` is not yet a shell. `Config::SystemDefault` is the out-of-the-box setting for any user who has not customized their shell, and it still has to fall through `compute_fallback_shell()` inside `ShellStarter::init`'s async resolution before the real shell is known. Converting against the *setting* rather than the resolved shell lands the user in the wrong directory whenever the two differ.

So `cwd` stays in **host** form for the whole of `restart_session_inner` and is handed to `spawn_shell_start` that way. `on_shell_determined` computes the real `ShellLaunchData` from the resolved `ShellStarter` and only then calls `native_startup_directory` against it. The conversion is the identity for `Executable`, `convert_windows_host_to_wsl_path` for `WSL`, `convert_windows_native_to_msys2_path` for `MSYS2`, and `None` for `DockerSandbox` — paths inside a sandbox container are not host paths at all, so there is nothing meaningful to translate.

`test_restart_with_system_default_shell_still_lands_in_the_requested_directory` (`app/src/pane_group/mod_tests.rs`) is the regression guard, observing the final directory through the test-only `TerminalManager::last_resolved_startup_directory` hook — the value is otherwise unobservable once it crosses into `PtyOptions` and becomes an environment variable read by the shell's own bootstrap.

Commits `0c75afd1`, `eeb68f5a`.

## Components

**Deliberate-teardown marking** — `TerminalView::begin_restart` / `finish_restart`, plus `TerminalModel::exit_for_restart` and `reset_exit_state`. Between them they suppress the teardown effects (banner, editor disable, `Event::Exited`, failing the in-flight conversation) for an exit that is a restart rather than a death, and clear the exit latch so the replacement session is writable. `finish_restart` is driven off the barrier, not off a call-site timeline — see *The restart barrier*.

**Clearing the replaced shell's post-mortem** — `TerminalSurface::on_session_restart_started`, the first thing `restart_session_inner` does. Four pieces of state are written by a shell dying and cleared by nothing else, so without this the replacement inherits the replaced session's verdict about itself:

- `inline_banners_state.shell_process_terminated_banner` — the pinned "shell exited" banner. Assigned in exactly one place and never cleared, so a pane restarted out of a broken shell config keeps it above the healthy new prompt permanently. **Both halves have to go**: `insert_shell_process_terminated_banner` writes this state *and* inserts a blocklist entry, and only the state drives rendering. Clearing the state alone leaves the entry as an `is_hidden: false` item of `INLINE_BANNER_HEIGHT`, so the pane grows a blank 2.5-line band and the layout logs "Missing banner element for ID" every pass — partly reinstating the symptom. Every other dismissal in `terminal/view.rs` pairs the two for the same reason.
- the forced-on initialization block. `show_initialization_block` only ever sets it to `true`, overriding the user's `should_show_bootstrap_block` setting; the restart puts it back to *what the setting says*, not to a flat `false`, so a user who asked to see the bootstrap block keeps seeing it.
- `is_login_shell_bootstrapped` — gates `TerminalPane`'s keep-open branch for `Event::Exited`. A stale `true` closes a pane whose replacement died before bootstrapping, destroying the scrollback and conversation the restart existed to preserve. Restarting *into* a misconfigured shell is the whole point of the switch path, so this case is the norm, not the edge.
- `pty_spawn_failed` — suppresses both the terminated banner and the `Event::Exited` emission. Restarting is the obvious recovery from a failed spawn, so a stale `true` leaves the recovered pane unable to ever report a later clean shell death: no banner, no close, a pane sitting open around a dead shell.

What it deliberately does *not* clear is scrollback. An unbootstrapped death inserts its explanation as a rich-content block rather than an inline banner, and that block is output — it stays, exactly as the surrounding command output does.

**`build_session_resources`** — extracts the per-session event-loop construction out of `create_model_with_manager` so a restart can build a second set.

**`restart_session(target, cwd, ctx)`** on `TerminalManagerTrait` (the `TerminalManager` trait in `app/src/terminal/terminal_manager.rs`, aliased in the impl files) — the single entry point, defaulted to a no-op exactly as `ensure_shell_started` is, with the body living once in `TerminalManager::restart_session_inner` and both impls forwarding to it. `target: None` means "same shell". `cwd` arrives in **host** form and **stays** in host form: the conversion happens in `on_shell_determined`, once the shell that will actually launch is settled. See *The cwd relocation*.

It returns `bool` — whether a session was actually replaced — and is `#[must_use]`. A manager with no local session to replace (the remote-TTY viewer, the mock) keeps the no-op default and answers `false`, and acting on a restart that was only *asked* for would park a `claude --resume <uuid>` in the pending-command queue of a live, untouched shell. `supports_session_restart()` is the before-the-fact form of the same question, which is what the command handlers consult so the user is never shown a confirmation modal or a shell picker promising something that will not happen. It is consulted in three places, which are three different lines of defence. `Workspace::keymap_context` publishes `Workspace_PaneCanRestartShell` when the focused pane's manager answers `true`, and both bindings carry that as a **context predicate** — the palette builds its list through `bindings_for_context`, which filters candidates on exactly that predicate (empty-trigger bindings are not exempt), so on an SSH pane the two commands are not offered at all. `start_shell_restart` and `toggle_change_shell_menu` then decline anyway, for a command reached by a rebind or a stale context, so nothing is ever put in front of the user that cannot happen. `EnabledPredicate` could not have expressed this — it is a bare `fn() -> bool` (`crates/warpui_core/src/keymap.rs`) with no context to read a pane from — but a context predicate can, and needs no change to the shared binding registry.

**`WorkspaceAction::RestartShell`, `ToggleChangeShellMenu` and `ChangeShell { shell }`** — three variants, two commands. `RestartShell` and `ChangeShell` both route into `Workspace::start_shell_restart`, the first with `target: None`; `ToggleChangeShellMenu` is the picker toggle, and selecting an entry dispatches `ChangeShell`. Wired like the existing `AddTabWithShell`.

Neither command gets a default keybinding in this work. Restart is the one that would benefit, but every unclaimed chord is a collision risk against user bindings and platform conventions, and the command palette is sufficient to use the feature. Assigning one is a separate, reversible decision.

**Shell picker** — reuses the list the new-tab menu already builds (`Workspace::unified_new_session_menu_items`, `app/src/workspace/view.rs`): `AvailableShells::handle(ctx).get_available_shells()`, with `display_name_for_shell` for labels and `ShellIndicatorType` for icons. Building a second list would let the two drift. Shells that do not resolve to a launchable `ShellLaunchData` — the "System default" placeholder — are filtered out, because restarting with "the same shell" is already what `RestartShell` does.

**Not gated on `FeatureFlag::ShellSelector`, and not gated on `target_os = "windows"`.** `Workspace::change_shell_menu_items` is gated only on `#[cfg(feature = "local_tty")]`, with a `Vec::new()` counterpart for builds without it, and the `RestartShell` / `ToggleChangeShellMenu` bindings in `app/src/workspace/mod.rs` are registered for every platform (they do carry a `Workspace_PaneCanRestartShell` context predicate, which is about pane *capability*, not platform or flag). This is a decision, not an oversight, and it settles the disagreement here rather than leaving it open: restarting and switching a pane's shell is useful on every platform, and `FeatureFlag::ShellSelector` gates the *new-session* shell dropdown — a different surface, with a different rollout. Coupling the two would hide a working feature behind an unrelated flag. Per-pane suitability is handled by `supports_session_restart()` at the call site, which is the axis that actually varies.

**Confirmation modal** — follows the established pattern: a `ModalViewState<Modal<T>>` field with `ctx.add_typed_action_view(..)`, as `TransferOwnershipConfirmationModal` does (`app/src/settings_view/transfer_ownership_confirmation_modal.rs`). Shipped as `RestartShellConfirmationModal` (`app/src/workspace/restart_shell_confirmation_modal.rs`). It names the running command so the choice is informed, rather than asking a generic "are you sure".

The pending restart it carries — manager handle, terminal view, target and cwd — is dropped by a single close path, `close_restart_shell_confirmation_modal`, reached from Confirm, Cancel, Escape and the X button alike. Escape and the X route through `ModalEvent::Close`, which originally bypassed the clearing that only the explicit Cancel handler did, leaving a live `ModelHandle` to the terminal manager uncleared (`423d523d`).

**Reverse path converters** in `crates/warp_util/src/path.rs`. Less new code than it first appears — the forward WSL converter has two branches, and one already has an inverse:

| Forward (`convert_wsl_to_windows_host_path`) | Inverse |
| --- | --- |
| `/mnt/<drive>/…` → `<DRIVE>:\…` | **New.** `<DRIVE>:\…` → `/mnt/<drive>/…` |
| anything else → `\\WSL$\<distro>\…` | **Exists:** `parse_wsl_unc_path`, which handles the `wsl$`, `wsl.localhost`, verbatim-UNC and forward-slash spellings, case-insensitively |

So the WSL work is one new branch plus a dispatcher that routes to it or to `parse_wsl_unc_path`. The MSYS2 reverse has no counterpart and is genuinely new.

One asymmetry the inverse cannot undo: the forward converter calls `std::fs::read_link` on the drive-letter branch, so a `/mnt/c/…` path may come back already resolved through a symlink. Round-tripping such a path will not return the original.

## Data flow

### Working directory

`pwd_if_local()` yields a **host** path: the existing converters normalise shell→host, and `TerminalPaneSnapshot.cwd` is stored in host form.

`startup_directory` for a WSL shell is in **WSL-native** form. Verified: `on_shell_determined` substitutes `wsl_shell_starter.home_directory()` only when `startup_directory` is `None`, and otherwise passes the value through untouched. Handing a host path to a WSL shell would therefore land the user in the wrong directory, or fail outright.

So the flow is: `pwd_if_local()` → host path → `restart_session_inner` → `spawn_shell_start` (**still host form**) → `on_shell_determined` resolves the real shell → `native_startup_directory` converts → the new session's startup directory. The conversion is the identity for `Executable` shells, the new converters for WSL and MSYS2, and `None` for `DockerSandbox`. The reason it happens at the far end rather than in `restart_session` is *The cwd relocation* above.

### Agent resume

**Not `ResumeOptions::ThirdParty`.** That is the headless cloud driver's mechanism: `ResumePayload::Claude` has one producer, `fetch_resume_payload` in `claude_code.rs`, which takes a server-minted `AIConversationId` and fetches the envelope over HTTP. A `claude` the user typed into a pane has neither a server conversation id nor a server-stored envelope, so there is nothing to capture.

The shipped mechanism is **transcript forking**. `Workspace::resolve_agent_resume_command` (`app/src/workspace/view.rs`) runs *before* the teardown, because the session id lives nowhere else once the PTY is gone:

1. Read the pane's tracked session from `CLIAgentSessionsModel`. Bail if there is none, or if `session.agent != CLIAgent::Claude` — Codex and Gemini are not supported.
2. Take `session_context.session_id` and parse it as a `Uuid`; take `session_context.cwd`; resolve `claude_transcript::claude_config_dir()`.
3. Call `claude_transcript::fork_transcript_at_last_complete_turn(uuid, &cwd, &config_root)`.

That function reads `<config_root>/projects/<encoded_cwd>/<session_id>.jsonl` **read-only**, trims it to the last complete assistant turn, writes the trimmed copy under a **fresh** uuid via `write_envelope`, best-effort adds a `sessions-index.json` entry for it, and returns `claude --resume <new-uuid>`. Claude's original file is never modified, so a bad trim is always recoverable by resuming the original session id by hand. `Ok(None)` when there is no session file, or when nothing resumable survives the trim.

The returned command is parked with `TerminalView::set_pending_command_queue` and fires once the replacement shell bootstraps. That sequencing already existed and is reused rather than rebuilt: the pending-command queue is a "run this once the shell is bootstrapped" primitive — `set_pending_command_queue` parks a command, and `execute_pending_command` refuses to submit until `block_list().is_bootstrapped()`. Task 1 of the lazy-restore work is what made that queue survive deferral. Agent resume enqueues like any other startup command.

Two ordering rules fall out, both load-bearing:

- The fork must run **before** `restart_session`, per above.
- The tracked session must be dropped (`clear_tracked_agent_session`) **after** the fork and after a successful restart. Otherwise the agent UI shows a live session for a dead process, and — worse — a *second* restart before the resumed agent's `SessionStart` hook re-registers would read the pre-restart id again and fork the pre-restart transcript, silently discarding everything the resumed session added.

Commits `5776a675` (which replaced the resume approach with forking), `a6f35719`, `27feec5d`, `6c8a4cde`.

## Error handling

**Teardown does not complete.** Originally specified as: abort the relaunch if teardown fails. **Dropped, deliberately.** `stop_event_loop` (which a restart calls, rather than the irreversible `shutdown_event_loop` — a replacement session publishes to the same PTY reads broadcast channel, and closing it cannot be undone) takes the join handle out of its `Option` *before* attempting the join and never restores it on failure, so any boolean success signal is unconditionally true and the guard would be vacuous. A genuine signal means propagating the `join()` `Result` — worth doing, but it is not what makes this feature work, and faking it would be worse than recording it. Tracked as a follow-up.

**The new shell fails to start.** The existing failure path applies — the "shell unsupported" banner and fallback-shell selection in `on_shell_determined`. A failed *switch* must not leave the pane shell-less: the pane ends up either on the requested shell or on the fallback, never parked forever.

**Directory no longer exists.** The captured directory may have been deleted, or may not resolve in the target shell (a Windows path with no WSL mount). `native_startup_directory` returns `None`, and the shell then starts in **its own default directory — silently**. There is no banner and no warning. An earlier draft of this spec said the pane reports the fallback; it does not, and the code does not have the hook to. WSL is the one partial exception, and it is not a message either: `on_shell_determined` substitutes `wsl_shell_starter.home_directory()` when the directory came out `None`, which is a better default, not a notification.

**Resume against a truncated transcript.** Killing an agent mid-turn truncates its transcript, and `claude --resume <original-id>` would hand Claude a partial turn to continue from. `trim_to_last_complete_turn` (`claude_transcript.rs`) is what prevents that: a turn is complete at the last `"type": "assistant"` entry whose `message.stop_reason` is one of a small **allowlist** — `end_turn`, `max_tokens`, `stop_sequence`, `refusal`. Everything else means the turn continues: `tool_use` (queued tool calls), `pause_turn` (explicitly "not done"), absent/null (still streaming), and anything added to the Messages API after this was written. Deliberately an allowlist rather than a blacklist, because under-trimming corrupts the conversation while over-trimming merely loses recoverable scrollback. When in doubt, trim more.

**Every failure on the agent path is silent.** Untracked session, an agent other than Claude, a bare `claude` with no plugin (`session_id: None`, unresumable by any mechanism), no cwd on record, a fork error, a nothing-survived-the-trim `Ok(None)` — all of them log at most and otherwise do nothing. The shell still restarts: no banner, no warning, no extra confirmation. The rendered conversation stays on screen and the user relaunches the agent themselves. Getting the shell back matters more than getting the agent back.

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
- A pane with a tracked Claude session enqueues exactly one `claude --resume <uuid>`, and the uuid is the *forked* one, not the original.
- Two restarts in flight establish exactly one session — the generation guard.
- A restart under `Config::SystemDefault` still lands in the requested directory — the cwd relocation.

Two of these need test-only observables, because the quantities they assert on are invisible in production: `TerminalManager::established_session_count` (a superseded start installing a second event loop produces no log line and no error, just a stray shell) and `TerminalManager::last_resolved_startup_directory` (the final directory becomes an environment variable read by the shell's own bootstrap and is unobservable afterwards). Both are `#[cfg(test)]`.

**Known gap, partly closed:** the original gap was that no test asserted a shell actually *started* — a relaunch that tore down and never respawned would have passed. `test_restart_session_keeps_the_pane_and_rearms_its_exit_handling` now waits on the restart barrier coming back round *with a live replacement event loop*, and asserts the established-session count rose. What is still untested is that the replacement reaches a working prompt; that needs a real PTY.

## Risks

**Teardown was the whole risk, and it is where the design actually went.** A teardown that leaks the PTY, double-fires the exit handler, or races the new session's spawn produces symptoms — a zombie process, a pane that reports exit twice, a shell that starts and immediately dies — that are timing-dependent and awkward to test. The barrier and the generation guard are the two answers, and both were found during implementation rather than designed up front.

**The Claude JSONL entry schema is unverified against a real fixture.** The repo contains no real Claude Code transcript to check against, so `trim_to_last_complete_turn` derives its boundary from what `write_envelope`/`read_jsonl` already treat as ground truth plus the publicly documented format. The allowlist is the mitigation, not a substitute for a fixture: an unrecognised `stop_reason` trims more rather than less. If the format changes, the failure is over-trimming, which is recoverable.

**The fork leaves a duplicate transcript per restart, and nothing prunes them.**

**The reverse path converters sit on a correctness-critical path.** Half the WSL case already exists (`parse_wsl_unc_path`), and the rest has a symmetric counterpart to mirror, which bounds the work. But a wrong conversion silently lands the user in the wrong directory rather than failing loudly, and the `read_link` asymmetry means round-trip tests cannot be the only check.
