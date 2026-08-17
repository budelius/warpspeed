# Lazy shell startup for launch configurations

**Status:** design
**Date:** 2026-08-17
**Scope:** launch configurations only. Session restore keeps its current eager behaviour.

## Problem

Opening a launch configuration forks one login shell per pane, immediately. A 141-pane
configuration forks 141 shells, of which the user sees two or three. On Windows each shell also
drags in a `conhost.exe`, and every process creation pays a Defender real-time scan.

The cost is not theoretical. On a 32-thread workstation, opening such a configuration drives the
process count past 1700 and pins every core until the burst clears.

Most of those shells are never looked at. Defer them.

## Design

### The seam

`TerminalManager::create_model_with_manager` builds the terminal model, surface, event loop, and
channels synchronously, then *schedules* the shell last.
`app/src/terminal/local_tty/terminal_manager.rs:457`:

```rust
let terminal_manager_model = ctx.add_model(|ctx| {
    let terminal_manager = box_manager(terminal_manager);
    ctx.spawn(
        async move { /* resolve shell starter */ },
        move |terminal_manager, shell_starter_source, ctx| {
            on_shell_determined(
                terminal_manager,
                startup_directory,
                env_vars,
                user_default_shell_unsupported_banner_model_handle,
                shell_startup_resources,
                shell_starter_source,
                ctx,
            )
        },
    );
    terminal_manager
});
```

Every input the shell needs is already gathered into that one closure. Nothing else in the
function depends on the shell existing. The deferral therefore needs no new plumbing: it needs a
place to park those inputs and a way to run them later.

### Parking the inputs

Add to `TerminalManager`:

```rust
deferred_shell: Option<DeferredShellStart>,
```

`DeferredShellStart` holds exactly the arguments `on_shell_determined` takes, plus the shell
starter source future's inputs.

`create_model_with_manager` gains a policy argument:

```rust
enum ShellStartPolicy { Eager, Deferred }
```

Under `Eager` it spawns as it does today. Under `Deferred` it stores `DeferredShellStart` and
spawns nothing.

### Starting on demand

```rust
pub fn ensure_shell_started(&mut self, ctx: &mut ModelContext<...>)
```

Takes the option with `Option::take` and runs the same `ctx.spawn` the eager path runs. `take`
makes it idempotent: a second call finds `None` and does nothing. No flag, no separate state
machine, no way to fork twice.

Pane activation calls it. So does the manual control described below.

### Which panes start eagerly

Panes in the active tab of each opened window start eagerly. Panes in every other tab defer.

A launch configuration may define several windows, each with its own `active_tab_index`. Each such
window shows one tab, so each contributes its active tab's panes to the eager set.

"Visible" means the active tab, not the focused pane. A tab with splits renders all of its panes
at once, so starting only the focused one would leave visible panes showing nothing.

For a 141-pane, 62-tab configuration this forks the two or three panes of the active tab instead
of 141.

### Manual start control

The vertical tabs panel already lists individual panes when
`VerticalTabsDisplayGranularity::Panes` is set (`app/src/tab.rs:247`, rendered from
`app/src/workspace/view/vertical_tabs.rs`). Add to each pane row:

- an indicator showing whether the shell has started
- a control that calls `ensure_shell_started` for that pane without switching to it

This turns deferral from something that happens to the user into something the user drives. It
also gives a way to pre-warm a pane before switching, so the first switch feels instant.

### Safety guard

A pane whose template carries a non-empty `commands` list starts eagerly regardless of policy.

Deferring a pane with pending startup commands raises a question this design should not have to
answer: when do the commands run, and what does the user see until they do? Forcing such panes
eager sidesteps it. This also avoids the "lossless re-snapshot" prerequisite that the upstream
proposal needs, because a deferred pane then carries nothing but a `cwd` and an optional shell,
both already in the configuration file.

### Feature gating

`FeatureFlag::LazyShellStartup` in `crates/warp_features/src/lib.rs`, following the pattern the
APP-5257 deferred-scrollback work established in the same file.

## Files changed

| File | Change |
|---|---|
| `app/src/terminal/local_tty/terminal_manager.rs` | `DeferredShellStart`, `ShellStartPolicy`, `ensure_shell_started` |
| `app/src/pane_group/mod.rs` | pass the policy at the launch-config call site; call `ensure_shell_started` on activation |
| `app/src/workspace/view.rs` | mark the active tab's panes eager when opening a launch configuration |
| `app/src/workspace/view/vertical_tabs.rs` | per-pane started indicator and start control |
| `crates/warp_features/src/lib.rs` | the flag |

## Testing

In `app/src/pane_group/mod_tests.rs`:

- a deferred pane forks no shell
- first activation forks exactly one
- second activation forks none, proving `take` idempotency
- a pane with `commands` forks eagerly even under `Deferred`
- with the flag off, every pane forks eagerly

In `app/src/workspace/view_tests.rs`:

- opening a multi-tab configuration forks only the active tab's panes
- switching to a background tab forks that tab's panes

In `app/src/workspace/view/vertical_tabs_tests.rs`:

- the indicator distinguishes started from deferred panes
- the control starts a pane without changing the active tab

## Verification

Measure, do not assume. Open the 141-pane configuration and count:

```powershell
(Get-Process powershell -EA SilentlyContinue).Count
```

Baseline is 141 forks plus 141 `conhost.exe`. Target is the active tab's pane count. Compare
process totals during the open burst as well, since the burst rather than the steady state is what
saturates the machine.

## Out of scope

Reaping shells that have already started. A pane keeps its shell once opened, so no long-running
command is ever killed and no shell state is ever lost. Hibernation of running panes would need a
policy for both, and neither has an answer the user would accept.

Session restore, which keeps forking eagerly. The user this targets runs with
`restore_session = false`.
