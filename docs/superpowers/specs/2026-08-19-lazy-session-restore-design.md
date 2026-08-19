# Lazy shell startup for session restore and command panes

**Status:** design
**Date:** 2026-08-19
**Scope:** extends the deferral mechanism shipped in `2026-08-17-lazy-shell-startup-design.md` to two
cases that design deliberately excluded. Depends on that branch having landed.

## Problem

The shipped feature defers forking a login shell for launch-configuration panes outside the active
tab. Two exclusions remain, and both matter now.

**Session restore still forks everything.** `PanesLayout::Snapshot` always passes
`ShellStartPolicy::Eager` (`app/src/pane_group/mod.rs:872-875`). Restoring a 141-pane session forks
141 shells, which is the same burst the shipped feature removed from launch configs. The original
spec excluded it on the grounds that the target user runs with `restore_session = false`; that is no
longer true, because session recovery is wanted for resilience — an editor freeze should not lose
running agent work.

**Panes with startup commands still fork eagerly.** A pane whose template carries a non-empty
`commands` list is forced `Eager` regardless of policy. That guard exists because the original design
could not answer a question it had no mechanism for: when do a deferred pane's commands run, and what
does the user see until they do?

The second exclusion is now the more damaging one. Resuming an external agent is expressed as a
startup command (`claude --resume <uuid>`), so under the current guard every agent pane forks
immediately — precisely the panes a user has most of.

## Design

### The answer the original spec lacked

A deferred pane's commands run when the pane is activated, immediately after its shell starts.

Nothing new is needed to make that true. `set_pending_command_queue` already queues commands on the
terminal view without running them (`app/src/pane_group/mod.rs:1443`), and the queue drains when the
shell produces its first prompt. The eager path relies on exactly this ordering today: commands are
queued during pane construction, before `on_shell_determined` has run. A deferred pane differs only
in when the shell arrives.

So the guard is not load-bearing. It was a placeholder for a mechanism that already existed.

### Removing the commands guard

`ShellStartPolicy::refine_for_pane(self, has_commands: bool)` currently forces `Eager` when
`has_commands` is true. With the forcing gone the method would only return `self`, so **delete it
entirely** rather than leave an identity function behind, and drop its call site:

```rust
// app/src/pane_group/mod.rs, leaf arm — before:
let pane_shell_start = shell_start.refine_for_pane(!commands.is_empty());
// after: use `shell_start` directly.
```

Delete `pending_commands_force_eager` from `shell_start_policy.rs` too; it pins behaviour that is
being removed, and there is no weaker version of it worth keeping.

A pane with commands then follows its tab's policy like any other. Its commands sit queued until the
tab is activated, at which point the shell starts and the queue drains.

Until activation the pane renders as it does today for any deferred pane: its `cwd` and title are
known, its blocklist is empty. The user sees an idle pane, not a running command. That is the correct
depiction, because the command genuinely has not run.

### Deferring session restore

`restore_pane_tree` builds panes from a `PaneNodeSnapshot`. Give it the same
`ShellStartPolicy` parameter `pane_tree_from_template` takes, and thread it to the same
`create_session` call.

The policy decision matches launch configs: the window's active tab is eager, every other tab defers.
`Workspace::configure_new_workspace`'s `NewWorkspaceSource::Restored` arm already has
`window_snapshot.active_tab_index` in hand (`app/src/workspace/view.rs:3923`) and already resolves it
before activating, so the eager tab is available without the up-front-clamp problem launch configs
had.

A restored pane carries scrollback and command blocks. These are display-only: they are rehydrated
into the `TerminalModel` from SQLite and render without a PTY. The pane therefore shows its history
while parked, which is a better deferred state than a launch-config pane's empty one.

### What still forks eagerly

`PanesLayout::SingleTerminal` — a new terminal the user just asked for. There is no case where the
user opens a new terminal and does not want a shell.

Ambient/cloud panes, which have no local shell to defer.

### Feature gating

Reuse `FeatureFlag::LazyShellStartup`. Both changes are the same feature reaching two more code
paths, and a separate flag would multiply the states that need testing without buying isolation —
the mechanism is already shared.

## Files changed

| File | Change |
|---|---|
| `app/src/terminal/shell_start_policy.rs` | delete `refine_for_pane` and its test |
| `app/src/pane_group/mod.rs` | thread policy into `restore_pane_tree`; drop the `refine_for_pane` call |
| `app/src/workspace/view.rs` | choose the eager tab in the `Restored` arm |
| `app/src/pane_group/mod_tests.rs` | deferred-pane-with-commands tests |
| `app/src/workspace/view_tests.rs` | session-restore deferral tests |

## Testing

In `app/src/pane_group/mod_tests.rs`:

- a deferred pane with `commands` parks its shell rather than forcing eager
- activating that pane starts the shell and leaves the queued commands intact for the drain
- a pane with commands under `Eager` behaves exactly as before

In `app/src/workspace/view_tests.rs`:

- restoring a multi-tab session parks every tab but the restored active one
- activating a restored background tab starts that tab's panes
- with the flag off, every restored pane forks eagerly

Existing tests that assert the commands guard must be updated rather than deleted; the behaviour they
pin is being changed deliberately, and the replacement assertion is that such a pane now defers.

## Verification

Session restore is the case unit tests cannot prove, for the same reason the shipped feature could
not: the assertions observe parked state, not forks. Measure with the real binary.

Open a large session, quit, reopen, and count shells before the first tab switch:

```powershell
.\measure-lazy-shell.ps1 -Exe warpspeed -Phase restored
```

Baseline is one shell per restored pane. Target is the active tab's pane count.

## Risks

**A restored pane's shell may differ from the one it had.** Session restore records
`shell_launch_data` per pane, so the parked pane must carry it. If it is dropped, a deferred pane
restores with the default shell while an eager one keeps its original — a silent divergence between
the two paths. The parked `DeferredShellStart` already holds a resolved shell starter for launch
configs; the restore path must populate it the same way.

**Removing the guard changes behaviour for existing configs.** Any launch config with `commands:` on
a background-tab pane currently forks at open and will now wait for activation. That is the intended
change, but it is a behaviour change for configs nobody edited. The flag gates it.

**`ShellLaunchState` stays `DeterminingShell` for parked panes.** Carried over from the shipped
design and still unresolved. It matters slightly more here: a restored pane displays real scrollback,
so a stale "determining shell" indicator sits next to visible history. Still cosmetic; still worth
deciding when the sidebar indicator lands.

## Out of scope

Capturing agent sessions into launch configs, and multi-process launch configs. Both are separate
sub-projects with their own specs. This one is a prerequisite for the first: without the guard
removal, every resumed-agent pane would fork eagerly.
