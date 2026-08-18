# Lazy Shell Startup Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Opening a launch configuration forks shells only for the panes of each window's active tab; every other pane parks its shell inputs and forks on first activation.

**Architecture:** Split the *policy decision* from the *shell mechanics*. A cfg-independent `ShellStartPolicy` is decided per tab in `Workspace::open_launch_config_window`, carried on `PanesLayout::Template`, refined per pane during template expansion (panes with startup commands force `Eager`), and handed to whichever terminal manager the build compiles. Under `local_tty`, `TerminalManager` parks the `on_shell_determined` inputs in `deferred_shell: Option<DeferredShellStart>` and runs them from `ensure_shell_started`, which uses `Option::take` for idempotency. Tab and pane activation call `ensure_shell_started`.

**Tech Stack:** Rust, in-house `warpui` entity/model framework, `cargo nextest`, `warp_features` feature flags.

## Global Constraints

- **Scope: launch configurations and tab configs only.** Session restore (`PanesLayout::Snapshot`) keeps forking eagerly and must not be touched.
- **Feature gated:** `FeatureFlag::LazyShellStartup`. With the flag off, every pane resolves to `ShellStartPolicy::Eager` — byte-for-byte today's behaviour.
- **Eager set = the active tab's panes**, not the focused pane. A tab with splits renders all its panes at once.
- **A pane whose template carries a non-empty `commands` list is always `Eager`**, regardless of policy or flag.
- **Never reap a started shell.** `ensure_shell_started` is one-way; there is no path back to `Deferred`.
- **CORRECTED DURING EXECUTION — `local_tty` IS enabled everywhere, including tests.** `app/build.rs:231-235` unconditionally emits `cargo:rustc-cfg=feature="local_tty"` for every non-wasm target, and does not emit `remote_tty`. So tests compile the `local_tty` branch of `create_session`, `MockTerminalManager` is dead code on that path, and every assertion here exercises the real `TerminalManager<S>`. There is no separate `--features local_tty` build: `cargo check -p warp --lib` already is it.

  The original constraint claimed the opposite, derived from grepping `Cargo.toml` (which declares `local_tty = []`, referenced by no other feature) without checking `build.rs`. Two consequences follow, and Tasks 2 and 3 were merged into one unit of work because of them:
    1. Task 2's tests cannot pass until Task 3 exists — the default `has_deferred_shell()` returns `false`, so the deferred-pane assertion fails until the real field is implemented. Task 2's "cargo check clean" gate is likewise unachievable in isolation.
    2. Task 3's code is genuinely unit-tested rather than compile-checked only. Coverage is better than planned, not worse.
- Package name for cargo is `warp`, not `app`. Unit tests: `cargo nextest run -p warp --lib`.
- **The build gate is the WORKSPACE, not `-p warp`.** CI runs `cargo nextest run --workspace --locked`. `create_tui_model` is `pub` and consumed by `crates/warp_tui`; `create_model` is consumed by `crates/integration`. A `-p warp --lib` check passes while the workspace is broken — this actually happened in Tasks 2+3 and was caught only in review. Every task's verification must include `cargo check --workspace` before commit. Types crossing that boundary must also be re-exported from `app/src/tui_export.rs`.

---

## Scope note: this plan is the mechanism, not the sidebar control

Spec A contains two independent subsystems:

1. **The deferral mechanism** — terminal lifecycle. Delivers the entire CPU win on its own. This plan.
2. **The per-pane started indicator and manual start control** in the vertical tabs panel (`app/src/workspace/view/vertical_tabs.rs`, 7407 lines of in-house UI DSL). Pure enhancement: without it, deferred panes still start on tab switch.

They are split because each produces working, testable software alone, and because mixing a 7400-line UI file into the same plan would force placeholder UI code. Subsystem 2 gets its own plan (`2026-08-17-lazy-shell-startup-sidebar-control.md`) written after this one lands, when `has_deferred_shell()` already exists to render against.

## Gaps found while planning

Three things in the spec do not survive contact with the code. Two are handled below; one needs a decision.

1. **`FeatureFlag::LazyShellStartup` has no "APP-5257 deferred-scrollback" precedent to follow.** No such flag exists in `crates/warp_features/src/lib.rs`. Task 1 follows the actual pattern: enum variant plus a `DOGFOOD_FLAGS` entry.
2. **~~The spec's tests cannot be written as stated.~~ WITHDRAWN — this was wrong.** I claimed "a deferred pane forks no shell" was unobservable because a test build has no shell. `app/build.rs` force-enables `local_tty`, so the real manager *is* built and the assertions in Tasks 2 and 5 land on it rather than on `MockTerminalManager`. The tests as written are valid; they just needed Task 3's implementation to exist before they could pass, which is why those tasks merged. Actual process-level fork counts still need the manual measurement in the spec's Verification section — a unit test can observe the parked state, not the absence of a `powershell.exe`.
3. **OPEN — a deferred pane's `ShellLaunchState` stays `DeterminingShell` forever.** `create_terminal_model` is handed `ShellLaunchState::DeterminingShell` at `terminal_manager.rs:351`, and only `on_shell_determined` advances it. A deferred pane will therefore display as though its shell is still being determined (`shell_launch_state.rs:7-23` has exactly two variants). Adding a third variant touches `display_name()` and its callers in tab titles. This plan leaves the state as-is and does not paper over it; decide before Task 6 of the sidebar plan whether to add `ShellLaunchState::Deferred`.

---

### Task 1: Feature flag and the `ShellStartPolicy` type

**Files:**
- Modify: `crates/warp_features/src/lib.rs:144` (enum, after `ShellSelector`) and `:1046` (end of `DOGFOOD_FLAGS`)
- Create: `app/src/terminal/shell_start_policy.rs`
- Modify: `app/src/terminal/mod.rs:77` (module declaration, alphabetical among the `shell_*` entries)
- Test: `app/src/terminal/shell_start_policy.rs` (inline `#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: nothing.
- Produces: `pub enum ShellStartPolicy { Eager, Deferred }` (derives `Clone, Copy, Debug, PartialEq, Eq`), `ShellStartPolicy::for_launch_config_tab(is_eager_tab: bool) -> ShellStartPolicy`, `ShellStartPolicy::refine_for_pane(self, has_commands: bool) -> ShellStartPolicy`, `FeatureFlag::LazyShellStartup`.

- [ ] **Step 1: Write the failing test**

Create `app/src/terminal/shell_start_policy.rs` containing only the test module for now:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::FeatureFlag;

    #[test]
    fn eager_tab_is_eager_and_background_tab_defers_when_flag_on() {
        let _flag = FeatureFlag::LazyShellStartup.override_enabled(true);
        assert_eq!(
            ShellStartPolicy::for_launch_config_tab(true),
            ShellStartPolicy::Eager
        );
        assert_eq!(
            ShellStartPolicy::for_launch_config_tab(false),
            ShellStartPolicy::Deferred
        );
    }

    #[test]
    fn every_tab_is_eager_when_flag_off() {
        let _flag = FeatureFlag::LazyShellStartup.override_enabled(false);
        assert_eq!(
            ShellStartPolicy::for_launch_config_tab(true),
            ShellStartPolicy::Eager
        );
        assert_eq!(
            ShellStartPolicy::for_launch_config_tab(false),
            ShellStartPolicy::Eager
        );
    }

    #[test]
    fn pending_commands_force_eager() {
        assert_eq!(
            ShellStartPolicy::Deferred.refine_for_pane(true),
            ShellStartPolicy::Eager
        );
        assert_eq!(
            ShellStartPolicy::Deferred.refine_for_pane(false),
            ShellStartPolicy::Deferred
        );
        assert_eq!(
            ShellStartPolicy::Eager.refine_for_pane(false),
            ShellStartPolicy::Eager
        );
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run -p warp --lib -E 'test(shell_start_policy)'`
Expected: compile error — `cannot find type ShellStartPolicy`, `no variant named LazyShellStartup`.

- [ ] **Step 3: Add the feature flag**

In `crates/warp_features/src/lib.rs`, immediately after the `ShellSelector` variant (line 144):

```rust
    /// Defers forking a login shell for launch-configuration panes that are
    /// not in their window's active tab until the pane is first activated.
    LazyShellStartup,
```

And as the last entry of `DOGFOOD_FLAGS` (before the closing `];` at line 1047):

```rust
    FeatureFlag::LazyShellStartup,
```

- [ ] **Step 4: Write the implementation**

Prepend to `app/src/terminal/shell_start_policy.rs`, above the test module:

```rust
use crate::FeatureFlag;

/// Whether a terminal pane forks its login shell at construction time or parks
/// the inputs and forks on first activation.
///
/// Decided per tab when a launch configuration is opened, then refined per pane
/// by [`ShellStartPolicy::refine_for_pane`]. Deliberately free of any
/// `local_tty` gating so the decision is observable in test builds, where
/// `MockTerminalManager` records it and no shell exists to fork.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShellStartPolicy {
    /// Fork the shell as part of building the terminal manager.
    Eager,
    /// Park the shell inputs; fork on the first `ensure_shell_started` call.
    Deferred,
}

impl ShellStartPolicy {
    /// The policy for every pane of one launch-configuration tab.
    ///
    /// `is_eager_tab` is true for the tab the window will activate. With
    /// [`FeatureFlag::LazyShellStartup`] disabled this always returns
    /// [`ShellStartPolicy::Eager`], preserving the pre-feature behaviour.
    pub fn for_launch_config_tab(is_eager_tab: bool) -> Self {
        if !FeatureFlag::LazyShellStartup.is_enabled() || is_eager_tab {
            Self::Eager
        } else {
            Self::Deferred
        }
    }

    /// Narrows a tab-level policy for one pane.
    ///
    /// A pane with pending startup commands always starts eagerly: deferring it
    /// would raise the question of when those commands run and what the user
    /// sees until they do, and forcing it eager sidesteps that entirely.
    pub fn refine_for_pane(self, has_commands: bool) -> Self {
        if has_commands { Self::Eager } else { self }
    }

    /// Whether this policy parks the shell instead of forking it.
    pub fn is_deferred(self) -> bool {
        matches!(self, Self::Deferred)
    }
}
```

In `app/src/terminal/mod.rs`, next to the existing `mod shell_launch_state;` at line 77:

```rust
pub mod shell_start_policy;
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo nextest run -p warp --lib -E 'test(shell_start_policy)'`
Expected: 3 tests PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/warp_features/src/lib.rs app/src/terminal/shell_start_policy.rs app/src/terminal/mod.rs
git commit -m "feat(terminal): add ShellStartPolicy and LazyShellStartup flag"
```

---

### Task 2: Carry the policy through template expansion to the manager

**Files:**
- Modify: `app/src/pane_group/mod.rs:868-873` (`PanesLayout::Template` becomes a struct variant), `:1347-1381` (`pane_tree_from_template`), `:1384-1541` (`pane_tree_from_template_recursive`), `:3469-3478` (the `initial_layout` match arm), `:6079-6186` (`create_session`)
- Modify: `app/src/terminal/mock_terminal_manager.rs:19-37` (record the policy)
- Modify: `app/src/terminal/terminal_manager.rs:24-40` (two default trait methods)
- Modify: `app/src/workspace/view.rs:3882` and `:7111` (the two production `PanesLayout::Template` construction sites — pass `ShellStartPolicy::Eager` for now; Task 4 replaces the launch-config one)
- Test: `app/src/pane_group/mod_tests.rs` (append)

**Interfaces:**
- Consumes: `ShellStartPolicy`, `ShellStartPolicy::refine_for_pane` from Task 1.
- Produces: `PanesLayout::Template { template: PaneTemplateType, shell_start: ShellStartPolicy }`; `TerminalManager::has_deferred_shell(&self) -> bool` (default `false`); `TerminalManager::ensure_shell_started(&mut self, ctx: &mut ModelContext<Box<dyn TerminalManager>>)` (default no-op); `PaneGroup::terminal_manager(pane_index: usize, app: &AppContext) -> Option<ModelHandle<Box<dyn TerminalManager>>>` (already exists at `:7754`, unchanged).

Why a struct variant rather than a new parameter: the policy would otherwise have to be threaded through `add_tab_with_pane_layout`, `PaneGroup::new_with_panes_layout`, `new_internal`, the `initial_layout` closure, `pane_tree_from_template`, and `pane_tree_from_template_recursive` — six signatures with many unrelated callers. Attaching it to the one layout variant that can defer confines the change to that variant's construction sites and keeps `Snapshot` and `SingleTerminal` provably untouched.

- [ ] **Step 1: Write the failing test**

Append to `app/src/pane_group/mod_tests.rs`:

```rust
/// A background-tab template parks its shell instead of forking it, and the
/// first `ensure_shell_started` consumes the parked start while the second is a
/// no-op. Asserted against the real `local_tty` `TerminalManager<TerminalView>`:
/// `local_tty` is unconditionally active on this platform (see `app/build.rs`),
/// so this reaches `deferred_shell`/`spawn_shell_start` for real, not a mock.
#[test]
fn test_deferred_template_pane_parks_shell_until_started() {
    let _flag = FeatureFlag::LazyShellStartup.override_enabled(true);
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let pane_group = mock_pane_group(
            &mut app,
            MockOptions {
                layout: PanesLayout::Template {
                    template: PaneTemplateType::PaneTemplate {
                        is_focused: Some(true),
                        cwd: "/".into(),
                        commands: vec![],
                        pane_mode: PaneMode::Terminal,
                        shell: None,
                    },
                    shell_start: ShellStartPolicy::Deferred,
                },
                ..Default::default()
            },
        );

        let manager = app
            .read(|ctx| pane_group.read(ctx, |group, ctx| group.terminal_manager(0, ctx)))
            .expect("pane 0 has a terminal manager");

        assert!(
            app.read(|ctx| manager.read(ctx, |m, _| m.has_deferred_shell())),
            "a deferred pane parks its shell start"
        );

        app.update(|ctx| {
            manager.update(ctx, |m, ctx| m.ensure_shell_started(ctx));
        });
        assert!(
            !app.read(|ctx| manager.read(ctx, |m, _| m.has_deferred_shell())),
            "the first activation consumes the parked start"
        );

        // Idempotency: Option::take left None, so this must do nothing rather
        // than fork a second shell.
        app.update(|ctx| {
            manager.update(ctx, |m, ctx| m.ensure_shell_started(ctx));
        });
        assert!(
            !app.read(|ctx| manager.read(ctx, |m, _| m.has_deferred_shell())),
            "a second activation starts nothing"
        );
    });
}

/// A pane carrying startup commands ignores `Deferred`, because deferring it
/// would leave the commands with no defined moment to run.
#[test]
fn test_template_pane_with_commands_starts_eagerly_when_deferred() {
    let _flag = FeatureFlag::LazyShellStartup.override_enabled(true);
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let pane_group = mock_pane_group(
            &mut app,
            MockOptions {
                layout: PanesLayout::Template {
                    template: PaneTemplateType::PaneTemplate {
                        is_focused: Some(true),
                        cwd: "/".into(),
                        commands: vec![crate::launch_configs::launch_config::CommandTemplate {
                            exec: "echo hello".to_string(),
                        }],
                        pane_mode: PaneMode::Terminal,
                        shell: None,
                    },
                    shell_start: ShellStartPolicy::Deferred,
                },
                ..Default::default()
            },
        );

        let manager = app
            .read(|ctx| pane_group.read(ctx, |group, ctx| group.terminal_manager(0, ctx)))
            .expect("pane 0 has a terminal manager");

        assert!(
            !app.read(|ctx| manager.read(ctx, |m, _| m.has_deferred_shell())),
            "a pane with pending commands must not defer"
        );
    });
}

/// With the flag off, a background tab's panes still fork eagerly. Drives the
/// real template pipeline rather than re-asserting `for_launch_config_tab`,
/// which Task 1 already covers: the requirement is about what the pane does,
/// not about what the constructor returns.
#[test]
fn test_flag_off_starts_every_template_pane_eagerly() {
    let _flag = FeatureFlag::LazyShellStartup.override_enabled(false);
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let pane_group = mock_pane_group(
            &mut app,
            MockOptions {
                layout: PanesLayout::Template {
                    template: PaneTemplateType::PaneTemplate {
                        is_focused: Some(true),
                        cwd: "/".into(),
                        commands: vec![],
                        pane_mode: PaneMode::Terminal,
                        shell: None,
                    },
                    // The policy a background tab would receive. With the flag
                    // off this resolves to `Eager`.
                    shell_start: ShellStartPolicy::for_launch_config_tab(false),
                },
                ..Default::default()
            },
        );

        let manager = app
            .read(|ctx| pane_group.read(ctx, |group, ctx| group.terminal_manager(0, ctx)))
            .expect("pane 0 has a terminal manager");

        assert!(
            !app.read(|ctx| manager.read(ctx, |m, _| m.has_deferred_shell())),
            "with the flag off no pane may park its shell"
        );
    });
}
```

Add to the imports at the top of `mod_tests.rs`:

```rust
use crate::terminal::shell_start_policy::ShellStartPolicy;
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo nextest run -p warp --lib -E 'test(deferred_template_pane) or test(template_pane_with_commands) or test(flag_off_starts_every_template)'`
Expected: compile error — `PanesLayout::Template` is a tuple variant, `has_deferred_shell` not found on `dyn TerminalManager`.

- [ ] **Step 3: Add the two default trait methods**

In `app/src/terminal/terminal_manager.rs`, inside `pub trait TerminalManager: Any` before `fn as_any`:

```rust
    /// Starts this manager's parked shell if it has one, and does nothing
    /// otherwise.
    ///
    /// Idempotent: implementations consume the parked start with
    /// `Option::take`, so any number of calls fork at most one shell. Managers
    /// that never defer keep the default no-op.
    fn ensure_shell_started(&mut self, _ctx: &mut ModelContext<Box<dyn TerminalManager>>) {}

    /// Whether a shell start is parked and has not run yet.
    fn has_deferred_shell(&self) -> bool {
        false
    }
```

Add `ModelContext` to that file's `warpui` import list if it is not already there.

Default bodies are what keep this task small: the six implementors (`local_tty::TerminalManager`, `TuiTerminalManager`, `MockTerminalManager`, `remote_tty`, the shared-session viewer, and the `warp_tui` managers) compile unchanged, and only two of them override.

- [ ] **Step 4: Make `MockTerminalManager` record the policy**

In `app/src/terminal/mock_terminal_manager.rs`, extend the struct at line 19:

```rust
pub struct MockTerminalManager {
    model: Arc<FairMutex<TerminalModel>>,
    view: ViewHandle<TerminalView>,
    /// Mirrors the real manager's parked-start bookkeeping so that policy and
    /// activation wiring are testable without `local_tty`. `Some(())` means a
    /// start is parked; `ensure_shell_started` takes it.
    deferred_shell: Option<()>,
}
```

Add a parameter to `create_model` (line 29), after `shell_state`:

```rust
        shell_start: ShellStartPolicy,
```

Set the field where the struct is built, and add the trait overrides in its `impl TerminalManager for MockTerminalManager` block:

```rust
            deferred_shell: shell_start.is_deferred().then_some(()),
```

```rust
    fn ensure_shell_started(&mut self, _ctx: &mut ModelContext<Box<dyn TerminalManager>>) {
        self.deferred_shell.take();
    }

    fn has_deferred_shell(&self) -> bool {
        self.deferred_shell.is_some()
    }
```

Pass `ShellStartPolicy::Eager` at the existing test-helper call site in the same file (`create_new_terminal_view_window_for_test`, line 186), and import `ShellStartPolicy`.

- [ ] **Step 5: Convert `PanesLayout::Template` to a struct variant**

In `app/src/pane_group/mod.rs`, replace line 871:

```rust
    Template {
        template: PaneTemplateType,
        /// Policy for this tab's panes, refined per pane by
        /// [`ShellStartPolicy::refine_for_pane`]. Only this variant can defer:
        /// `Snapshot` (session restore) and `SingleTerminal` always start eagerly.
        shell_start: ShellStartPolicy,
    },
```

- [ ] **Step 6: Thread the policy to `create_session`**

In `app/src/pane_group/mod.rs`, update the `initial_layout` match arm at line 3469:

```rust
                PanesLayout::Template {
                    template,
                    shell_start,
                } => Self::pane_tree_from_template(
                    template,
                    shell_start,
                    resources,
                    ctx,
                    pane_contents,
                    true, // initialize as the leftmost pane
                    unsupported_banner_model_handle,
                    view_bounds.size(),
                    model_event_sender_clone,
                ),
```

Add `shell_start: ShellStartPolicy` as the second parameter of both `pane_tree_from_template` (line 1348) and `pane_tree_from_template_recursive` (line 1384), forwarding it unchanged from the former to the latter, and from the branch arm to each recursive call (line 1517 — add `shell_start,` as the second argument; `ShellStartPolicy` is `Copy`, so no clone is needed).

In the leaf arm, immediately after the destructuring block that ends at line 1401 (`} => {`) and before `let uuid = Uuid::new_v4();`:

```rust
                // Computed before `commands` is moved into the pending-command
                // queue below. A pane with startup commands never defers.
                let pane_shell_start = shell_start.refine_for_pane(!commands.is_empty());
```

Then pass `pane_shell_start` to `PaneGroup::create_session` as its final argument before `ctx`, in the `PaneMode::Terminal | PaneMode::Agent` arm at line 1418.

- [ ] **Step 7: Accept and forward the policy in `create_session`**

In `app/src/pane_group/mod.rs`, add to `create_session`'s parameter list (line 6079) after `initial_input_config`:

```rust
        shell_start: ShellStartPolicy,
```

In the `local_tty` branch, pass it to `LocalTtyTerminalManager::<TerminalView>::create_model` after `chosen_shell` (line 6142). In the mock branch, pass it to `MockTerminalManager::create_model` after the `ShellLaunchState` argument (line 6172). In the `remote_tty` branch, discard it explicitly so the parameter is not dead:

```rust
                let _ = shell_start;
```

`LocalTtyTerminalManager::create_model` does not accept the argument yet — Task 3 adds it. Until then this branch is `cfg`-disabled in the default build, so `cargo check -p warp --lib` passes and `--features local_tty` does not. That is expected and is the reason Task 3 immediately follows.

- [ ] **Step 8: Fix the two production construction sites**

In `app/src/workspace/view.rs`, line 3882:

```rust
                    PanesLayout::Template {
                        template: tab_template.layout_with_tab_commands(),
                        shell_start: ShellStartPolicy::Eager,
                    },
```

and line 7111:

```rust
            PanesLayout::Template {
                template: pane_template,
                shell_start: ShellStartPolicy::Eager,
            },
```

Add `use crate::terminal::shell_start_policy::ShellStartPolicy;` to that file's imports. Task 4 replaces the first one; the tab-config site at 7111 stays `Eager` permanently, because a tab config opens exactly one tab and that tab is the one being activated.

- [ ] **Step 9: Fix the remaining `PanesLayout::Template` call sites**

Run: `cargo check -p warp --lib --tests 2>&1 | grep -A3 'PanesLayout::Template'`

Every hit is a test constructing the old tuple variant (`mod_tests.rs:3129`, `:3649`, `:3775`, and any others the compiler reports). Convert each to the struct form with `shell_start: ShellStartPolicy::Eager`, which preserves the behaviour those tests already assert.

- [ ] **Step 10: Run tests to verify they pass**

Run: `cargo nextest run -p warp --lib -E 'test(deferred_template_pane) or test(template_pane_with_commands) or test(flag_off_starts_every_template)'`
Expected: 3 tests PASS.

Run: `cargo nextest run -p warp --lib -E 'package(warp)'`
Expected: no new failures against the pre-task baseline.

- [ ] **Step 11: Commit**

```bash
git add app/src/pane_group/mod.rs app/src/terminal/mock_terminal_manager.rs app/src/terminal/terminal_manager.rs app/src/workspace/view.rs app/src/pane_group/mod_tests.rs
git commit -m "feat(pane_group): carry ShellStartPolicy from template to terminal manager"
```

---

### Task 3: Park and replay the shell start under `local_tty`

**Files:**
- Modify: `app/src/terminal/local_tty/terminal_manager.rs:78-112` (field), `:159-165` (near `ShellStartupResources`, add `DeferredShellStart`), `:197-233` (`create_model`), `:238-274` (`create_tui_model`), `:278-495` (`create_model_with_manager`), and a new `ensure_shell_started` in the `impl<S> TerminalManager<S>` block
- Test: `local_tty` is unconditionally active in test builds too (see Global Constraints' "CORRECTED DURING EXECUTION" note), so this task is genuinely unit-tested against the real `TerminalManager<S>`, not just compile-checked: `test_deferred_template_pane_parks_shell_until_started` and `test_template_pane_with_commands_starts_eagerly_when_deferred` in `app/src/pane_group/mod_tests.rs` exercise `deferred_shell`/`spawn_shell_start` directly, in addition to compilation and the manual measurement in Task 5's final step.

**Interfaces:**
- Consumes: `ShellStartPolicy` (Task 1); the trait methods added in Task 2.
- Produces: `LocalTtyTerminalManager::create_model(..., chosen_shell, shell_start, ctx, create_surface)` and `create_tui_model(..., chosen_shell, shell_start, block_spacing, ctx, create_surface)`; `TerminalManager<S>::ensure_shell_started`; `struct DeferredShellStart`.

- [ ] **Step 1: Add the parked-inputs struct**

In `app/src/terminal/local_tty/terminal_manager.rs`, after `struct ShellStartupResources` (line 165):

```rust
/// The complete set of inputs [`on_shell_determined`] needs, parked so the
/// shell can be forked later.
///
/// Holds exactly what the eager path's `ctx.spawn` closure captures. Nothing
/// else in `create_model_with_manager` depends on the shell existing, which is
/// why deferral needs no other plumbing.
struct DeferredShellStart {
    startup_directory: Option<PathBuf>,
    env_vars: HashMap<OsString, OsString>,
    user_default_shell_unsupported_banner_model_handle: ModelHandle<BannerState>,
    shell_startup_resources: ShellStartupResources,
    /// The async block's input: either a resolved starter source or a WSL
    /// distro name that still needs a WSL instance to resolve.
    wsl_name_or_shell_starter: Option<ShellStarterSourceOrWslName>,
}
```

`ShellStarterSourceOrWslName` is declared at `app/src/terminal/local_tty/shell.rs:407` and is what `ShellStarter::init` returns (`shell.rs:75`, called at `terminal_manager.rs:336`). Import it alongside the existing `ShellStarter, ShellStarterSource` import at `terminal_manager.rs:24`. Note the field is already `Option<...>` — do not wrap it again.

- [ ] **Step 2: Add the field**

In the same file, at the end of `pub struct TerminalManager<S>` (after `session_sharer`, line 112):

```rust
    /// Inputs for a shell that has not been forked yet. `None` once the shell
    /// has started, or immediately for [`ShellStartPolicy::Eager`] managers.
    ///
    /// `ensure_shell_started` consumes this with `Option::take`, which is what
    /// makes starting idempotent — there is no separate flag or state machine
    /// that could disagree with it, and no way to fork twice.
    deferred_shell: Option<DeferredShellStart>,
```

Initialise it to `None` in the struct literal at line 430.

- [ ] **Step 3: Accept the policy and branch on it**

Add `shell_start: ShellStartPolicy` to `create_model` (after `chosen_shell`, line 205), to `create_tui_model` (after `chosen_shell`, line 246), and to `create_model_with_manager` (after `chosen_shell`, line 286); forward it from the two public constructors to the private one.

Replace the `ctx.add_model` block at lines 457-489 with:

```rust
        let terminal_manager_model = ctx.add_model(|ctx| {
            match shell_start {
                ShellStartPolicy::Deferred => {
                    log::info!("Deferring shell startup for this pane until activation");
                    terminal_manager.deferred_shell = Some(DeferredShellStart {
                        startup_directory,
                        env_vars,
                        user_default_shell_unsupported_banner_model_handle,
                        shell_startup_resources,
                        wsl_name_or_shell_starter,
                    });
                    box_manager(terminal_manager)
                }
                ShellStartPolicy::Eager => {
                    let terminal_manager = box_manager(terminal_manager);
                    Self::spawn_shell_start(
                        DeferredShellStart {
                            startup_directory,
                            env_vars,
                            user_default_shell_unsupported_banner_model_handle,
                            shell_startup_resources,
                            wsl_name_or_shell_starter,
                        },
                        ctx,
                    );
                    terminal_manager
                }
            }
        });
```

Note `terminal_manager` must be `mut` in the `Deferred` arm; it already is (`let mut terminal_manager` at line 430).

- [ ] **Step 4: Extract the spawn into a reusable function**

Both paths now run the same spawn, so give it one home. Add to the `impl<S> TerminalManager<S>` block, next to `create_model_with_manager`, keeping the existing bounds (`S: TerminalSurface`, `<S as Entity>::Event: PtyIntentEvent`):

```rust
    /// Resolves the shell starter off the main thread and hands the result to
    /// [`on_shell_determined`]. This is the body the eager path runs at
    /// construction and the deferred path runs from `ensure_shell_started`.
    fn spawn_shell_start(
        parked: DeferredShellStart,
        ctx: &mut ModelContext<Box<dyn TerminalManagerTrait>>,
    ) {
        let DeferredShellStart {
            startup_directory,
            env_vars,
            user_default_shell_unsupported_banner_model_handle,
            shell_startup_resources,
            wsl_name_or_shell_starter,
        } = parked;

        ctx.spawn(
            async move {
                match wsl_name_or_shell_starter {
                    Some(starter_source) => starter_source.to_shell_starter_source().await,
                    None => None,
                }
            },
            move |terminal_manager: &mut Box<dyn TerminalManagerTrait>,
                  shell_starter_source,
                  ctx| {
                let Some(terminal_manager) =
                    TerminalManagerTrait::as_any_mut(terminal_manager.as_mut())
                        .downcast_mut::<Self>()
                else {
                    return;
                };

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
    }
```

`on_shell_determined` already guards against a closed window (`:548`), so a pane activated and then closed before the starter resolves is handled.

- [ ] **Step 5: Implement the trait methods**

In the `impl<S> TerminalManagerTrait for TerminalManager<S>` block (and the identical pair on `TuiTerminalManager`, which forwards to `self.0`):

```rust
    fn ensure_shell_started(&mut self, ctx: &mut ModelContext<Box<dyn TerminalManagerTrait>>) {
        let Some(parked) = self.deferred_shell.take() else {
            // Already started, or never deferred. Both are no-ops.
            return;
        };
        Self::spawn_shell_start(parked, ctx);
    }

    fn has_deferred_shell(&self) -> bool {
        self.deferred_shell.is_some()
    }
```

- [ ] **Step 6: Verify it compiles both ways**

Run: `cargo check -p warp --lib`
Expected: clean (the `local_tty` code is not compiled).

Run: `cargo check -p warp --lib --features local_tty`
Expected: clean. This is the only automated check this task gets, so it must be clean, not merely free of new errors. Record the pre-task output of the same command first if the branch is not already green under this feature.

Run: `cargo clippy -p warp --lib --features local_tty -- -D warnings`
Expected: clean.

- [ ] **Step 7: Commit**

```bash
git add app/src/terminal/local_tty/terminal_manager.rs
git commit -m "feat(local_tty): park deferred shell inputs and start them on demand"
```

---

### Task 4: Defer every tab but the one the launch config activates

**Files:**
- Modify: `app/src/workspace/view.rs:3869-3903` (`open_launch_config_window`)
- Test: `app/src/workspace/view_tests.rs` (append)

**Interfaces:**
- Consumes: `PanesLayout::Template { template, shell_start }` (Task 2), `ShellStartPolicy::for_launch_config_tab` (Task 1).
- Produces: no new API. `open_launch_config_window`'s signature is unchanged.

The ordering constraint that shapes this task: tabs are created — and today's shells forked — in the `for_each` at line 3880, *before* the active index is resolved and applied at lines 3895-3901. The eager tab must therefore be computed up front, using the same clamp the activation uses, so that the tab which starts eagerly is exactly the tab which ends up active.

- [ ] **Step 1: Write the failing test**

Append to `app/src/workspace/view_tests.rs`:

```rust
/// Opening a multi-tab launch configuration parks the shells of every tab but
/// the one it activates.
#[test]
fn test_launch_config_defers_all_but_active_tab() {
    let _flag = FeatureFlag::LazyShellStartup.override_enabled(true);
    App::test((), |mut app| async move {
        let workspace = mock_workspace(&mut app);

        let window_template = WindowTemplate {
            active_tab_index: Some(1),
            tabs: vec![
                tab_template_with_single_pane("tab-0"),
                tab_template_with_single_pane("tab-1"),
                tab_template_with_single_pane("tab-2"),
            ],
        };

        app.update(|ctx| {
            workspace.update(ctx, |ws, ctx| {
                ws.open_launch_config_window(window_template, ctx);
            });
        });

        let deferred: Vec<bool> = app.read(|ctx| {
            workspace.read(ctx, |ws, ctx| {
                ws.tabs
                    .iter()
                    .map(|tab| {
                        tab.pane_group
                            .read(ctx, |group, ctx| group.terminal_manager(0, ctx))
                            .expect("each tab has one terminal pane")
                            .read(ctx, |m, _| m.has_deferred_shell())
                    })
                    .collect()
            })
        });

        assert_eq!(
            deferred,
            vec![true, false, true],
            "only the activated tab (index 1) starts its shell eagerly"
        );
    });
}

/// An out-of-range `active_tab_index` falls back to the first tab for both
/// activation and the eager set, so the two never disagree.
#[test]
fn test_launch_config_out_of_range_active_index_starts_first_tab() {
    let _flag = FeatureFlag::LazyShellStartup.override_enabled(true);
    App::test((), |mut app| async move {
        let workspace = mock_workspace(&mut app);

        let window_template = WindowTemplate {
            active_tab_index: Some(99),
            tabs: vec![
                tab_template_with_single_pane("tab-0"),
                tab_template_with_single_pane("tab-1"),
            ],
        };

        app.update(|ctx| {
            workspace.update(ctx, |ws, ctx| {
                ws.open_launch_config_window(window_template, ctx);
            });
        });

        let deferred: Vec<bool> = app.read(|ctx| {
            workspace.read(ctx, |ws, ctx| {
                ws.tabs
                    .iter()
                    .map(|tab| {
                        tab.pane_group
                            .read(ctx, |group, ctx| group.terminal_manager(0, ctx))
                            .expect("each tab has one terminal pane")
                            .read(ctx, |m, _| m.has_deferred_shell())
                    })
                    .collect()
            })
        });

        assert_eq!(deferred, vec![false, true]);
    });
}
```

Add this helper next to the tests:

```rust
fn tab_template_with_single_pane(title: &str) -> TabTemplate {
    TabTemplate {
        title: Some(title.to_string()),
        layout: PaneTemplateType::PaneTemplate {
            is_focused: Some(true),
            cwd: "/".into(),
            commands: vec![],
            pane_mode: PaneMode::Terminal,
            shell: None,
        },
        commands: Vec::new(),
        color: None,
    }
}
```

Match the file's existing conventions for `mock_workspace` and imports; if `view_tests.rs` names its workspace fixture differently, use that name rather than introducing a second one.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo nextest run -p warp --lib -E 'test(launch_config_defers_all_but_active_tab) or test(launch_config_out_of_range_active_index)'`
Expected: FAIL — `assertion failed: left: [false, false, false], right: [true, false, true]`, because Task 2 hardcoded `Eager` at this site.

- [ ] **Step 3: Write the implementation**

Replace the body of `open_launch_config_window` (`app/src/workspace/view.rs:3873-3902`):

```rust
        let start_index = self.tabs.len();

        // Resolve the eager tab before creating anything. Tabs fork their
        // shells as they are constructed below, while the active index is only
        // applied afterwards, so this has to be decided up front — and with the
        // same clamp the activation uses, or the tab that starts eagerly would
        // not be the tab that ends up visible.
        let eager_tab_index = window
            .active_tab_index
            .filter(|index| *index < window.tabs.len())
            .unwrap_or_default();

        window
            .tabs
            .iter()
            .enumerate()
            .for_each(|(tab_index, tab_template)| {
                self.add_tab_with_pane_layout(
                    PanesLayout::Template {
                        template: tab_template.layout_with_tab_commands(),
                        shell_start: ShellStartPolicy::for_launch_config_tab(
                            tab_index == eager_tab_index,
                        ),
                    },
                    Arc::new(HashMap::new()),
                    tab_template.title.clone(),
                    ctx,
                );
                self.tabs[start_index + tab_index].selected_color = tab_template
                    .color
                    .map_or(SelectedTabColor::Unset, SelectedTabColor::Color);
            });

        if !window.tabs.is_empty() {
            // Focus the active tab from the launch config.
            let mut index = start_index + eager_tab_index;

            if index >= self.tab_count() {
                index = start_index;
            }

            self.activate_tab_internal(index, ctx);
        }
```

The `index >= self.tab_count()` guard is kept: `eager_tab_index` is clamped against `window.tabs.len()`, but `tab_count()` reflects the whole workspace and can still disagree if tab creation fails.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo nextest run -p warp --lib -E 'test(launch_config_defers_all_but_active_tab) or test(launch_config_out_of_range_active_index)'`
Expected: 2 tests PASS.

- [ ] **Step 5: Commit**

```bash
git add app/src/workspace/view.rs app/src/workspace/view_tests.rs
git commit -m "feat(workspace): defer shell startup for non-active launch config tabs"
```

---

### Task 5: Start deferred shells on tab and pane activation

**Files:**
- Create: nothing
- Modify: `app/src/pane_group/mod.rs` (new `start_deferred_shells`, near `terminal_manager` at `:7754`), `:5850-5866` (`focus_pane_on_mouse_event`), `app/src/workspace/view.rs:5320-5338` (`activate_tab_internal`)
- Test: `app/src/pane_group/mod_tests.rs` and `app/src/workspace/view_tests.rs` (append)

**Interfaces:**
- Consumes: `TerminalManager::ensure_shell_started` (Task 2), `PaneGroup::panes_of::<TerminalPane>` (`:2271`), `TerminalPane::terminal_manager` (`terminal_pane.rs:184`).
- Produces: `PaneGroup::start_deferred_shells(&self, ctx: &mut ViewContext<Self>)`.

Tab activation, not pane focus, is the load-bearing hook: switching tabs makes every pane of that tab visible at once, so all of them must start. Pane focus is the secondary hook, for a pane activated within an already-active tab.

- [ ] **Step 1: Write the failing test**

Append to `app/src/workspace/view_tests.rs`:

```rust
/// Switching to a background tab starts that tab's shells, and leaves the
/// other background tabs parked.
#[test]
fn test_activating_background_tab_starts_its_deferred_shells() {
    let _flag = FeatureFlag::LazyShellStartup.override_enabled(true);
    App::test((), |mut app| async move {
        let workspace = mock_workspace(&mut app);

        let window_template = WindowTemplate {
            active_tab_index: Some(0),
            tabs: vec![
                tab_template_with_single_pane("tab-0"),
                tab_template_with_single_pane("tab-1"),
                tab_template_with_single_pane("tab-2"),
            ],
        };

        app.update(|ctx| {
            workspace.update(ctx, |ws, ctx| {
                ws.open_launch_config_window(window_template, ctx);
            });
        });

        let read_deferred = |app: &mut App| -> Vec<bool> {
            app.read(|ctx| {
                workspace.read(ctx, |ws, ctx| {
                    ws.tabs
                        .iter()
                        .map(|tab| {
                            tab.pane_group
                                .read(ctx, |group, ctx| group.terminal_manager(0, ctx))
                                .expect("each tab has one terminal pane")
                                .read(ctx, |m, _| m.has_deferred_shell())
                        })
                        .collect()
                })
            })
        };

        assert_eq!(read_deferred(&mut app), vec![false, true, true]);

        app.update(|ctx| {
            workspace.update(ctx, |ws, ctx| {
                ws.activate_tab_internal(2, ctx);
            });
        });

        assert_eq!(
            read_deferred(&mut app),
            vec![false, true, false],
            "activating tab 2 starts only tab 2's shell"
        );
    });
}
```

Append to `app/src/pane_group/mod_tests.rs`:

```rust
/// `start_deferred_shells` starts every pane in the group, not just the focused
/// one: a split tab renders all of its panes simultaneously, so starting one
/// would leave the others blank.
#[test]
fn test_start_deferred_shells_starts_every_pane_in_the_group() {
    let _flag = FeatureFlag::LazyShellStartup.override_enabled(true);
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let leaf = |focused: bool| PaneTemplateType::PaneTemplate {
            is_focused: Some(focused),
            cwd: "/".into(),
            commands: vec![],
            pane_mode: PaneMode::Terminal,
            shell: None,
        };

        let pane_group = mock_pane_group(
            &mut app,
            MockOptions {
                layout: PanesLayout::Template {
                    template: PaneTemplateType::PaneBranchTemplate {
                        split_direction:
                            crate::launch_configs::launch_config::SplitDirection::Horizontal,
                        panes: vec![leaf(true), leaf(false)],
                    },
                    shell_start: ShellStartPolicy::Deferred,
                },
                ..Default::default()
            },
        );

        let deferred_count = |app: &mut App| -> usize {
            app.read(|ctx| {
                pane_group.read(ctx, |group, ctx| {
                    (0..2)
                        .filter_map(|index| group.terminal_manager(index, ctx))
                        .filter(|manager| manager.read(ctx, |m, _| m.has_deferred_shell()))
                        .count()
                })
            })
        };

        assert_eq!(deferred_count(&mut app), 2, "both panes start parked");

        app.update(|ctx| {
            pane_group.update(ctx, |group, ctx| group.start_deferred_shells(ctx));
        });

        assert_eq!(deferred_count(&mut app), 0, "both panes started");
    });
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo nextest run -p warp --lib -E 'test(activating_background_tab_starts) or test(start_deferred_shells_starts_every_pane)'`
Expected: compile error — `no method named start_deferred_shells`.

- [ ] **Step 3: Add `PaneGroup::start_deferred_shells`**

In `app/src/pane_group/mod.rs`, next to `terminal_manager` (line 7754):

```rust
    /// Starts the parked shell of every terminal pane in this group.
    ///
    /// Called when the group's tab becomes active. All of a tab's panes render
    /// at once, so all of them start — starting only the focused pane would
    /// leave the visible splits showing nothing. Panes that are already
    /// running are unaffected: `ensure_shell_started` is a no-op for them.
    pub fn start_deferred_shells(&self, ctx: &mut ViewContext<Self>) {
        // Collect first: the iteration borrows `self.pane_contents`, while
        // `update` needs the context mutably.
        let managers: Vec<_> = self
            .panes_of::<TerminalPane>()
            .map(|pane| pane.terminal_manager(ctx))
            .collect();

        for manager in managers {
            manager.update(ctx, |manager, ctx| manager.ensure_shell_started(ctx));
        }
    }
```

- [ ] **Step 4: Call it from tab activation**

In `app/src/workspace/view.rs`, inside `activate_tab_internal`, immediately after `self.set_active_tab_index(index, ctx);` (line 5334):

```rust
            // The newly active tab renders every one of its panes, so start any
            // whose shell was deferred when the launch configuration opened.
            let pane_group = self.active_tab_pane_group().clone();
            pane_group.update(ctx, |pane_group, ctx| {
                pane_group.start_deferred_shells(ctx);
            });
```

- [ ] **Step 5: Call it from pane activation**

In `app/src/pane_group/mod.rs`, in `focus_pane_on_mouse_event`, after the early-return guards and immediately before the function focuses the pane, start that one pane. Insert after the `ActivationReason::Hover` guard block (ends line 5866 region) and before the focusing logic:

```rust
        // A pane activated inside an already-active tab may still be parked
        // (for instance a pane the user pre-warms from the sidebar and then
        // clicks). Starting one pane here is enough; its siblings were started
        // when the tab was activated.
        if let Some(pane) = self
            .pane_contents
            .get(&id)
            .and_then(|content| content.as_any().downcast_ref::<TerminalPane>())
        {
            let manager = pane.terminal_manager(ctx);
            manager.update(ctx, |manager, ctx| manager.ensure_shell_started(ctx));
        }
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo nextest run -p warp --lib -E 'test(activating_background_tab_starts) or test(start_deferred_shells_starts_every_pane)'`
Expected: 2 tests PASS.

Run: `cargo nextest run -p warp --lib`
Expected: no new failures against the pre-task baseline.

Run: `cargo check -p warp --lib --features local_tty && cargo clippy -p warp --lib -- -D warnings`
Expected: clean.

- [ ] **Step 7: Commit**

```bash
git add app/src/pane_group/mod.rs app/src/workspace/view.rs app/src/pane_group/mod_tests.rs app/src/workspace/view_tests.rs
git commit -m "feat(workspace): start deferred shells on tab and pane activation"
```

- [ ] **Step 8: Measure the real thing**

Automated tests cannot observe fork counts, so this step is the only evidence that the feature does what it exists to do. Build with the real PTY and measure, per the spec's Verification section.

Build: `cargo build -p warp --release --features local_tty`

Before opening anything, record the baseline:

```powershell
@(Get-Process powershell -ErrorAction SilentlyContinue).Count
@(Get-Process conhost -ErrorAction SilentlyContinue).Count
(Get-Process).Count
```

Open `DUMMY-18` (141 panes, 63 tabs, whose active tab has 6 panes) and re-run all three counts. Then switch to a background tab and run them again.

Expected: the open adds ~6 shells rather than 141; the process total does not spike by ~1700; switching to a background tab adds that tab's pane count.

Record the three numbers in the commit message or the PR body. If the open still adds 141, the eager set is wrong — check `eager_tab_index` before assuming the deferral itself failed.

---

### Task 5: As shipped

Steps 4-5 above are what was planned. They are left in place because the planned-vs-shipped delta is itself useful record; what follows is what the code actually does.

**Step 4 as planned was wrong.** Hooking `activate_tab_internal` directly forks every tab in a launch configuration up front: `add_tab_with_pane_layout` reaches `activate_tab_internal` during tab *construction* (via `add_tab_with_pane_layout_with_activation`), not only on a genuine user-visible switch. `Workspace::open_launch_config_window` builds its tab list by calling this in a loop, one insertion per tab, entirely within a single update before the window ever shows the tab the user will actually see. A hook at `activate_tab_internal` cannot tell construction-time activation apart from a real switch, so it would start every pane's shell during that loop — the exact up-front fork this feature exists to prevent.

**What shipped instead:**
- A `TabActivation` enum (`app/src/workspace/view.rs:1025`), `{ Visible, Bookkeeping }`, distinguishes a tab activation that makes the tab genuinely visible (`Visible` — start its deferred shells) from one that's purely internal bookkeeping during construction (`Bookkeeping` — leave them parked). `Workspace::open_launch_config_window`'s tab-construction loop is the sole `Bookkeeping` producer, reached via `add_tab_with_pane_layout_with_activation` (`view.rs:12930`); every other caller goes through `add_tab_with_pane_layout`, which always passes `Visible`.
- The hook lives in `set_active_tab_index_with_activation` (`view.rs:5495`, the shared body `set_active_tab_index` calls into), gated on `activation == TabActivation::Visible`, not in `activate_tab_internal`.
- Step 5's inherent-method sketch shipped under different, narrower-visibility names. The public trait methods are as declared here (`TerminalManager::ensure_shell_started`, `has_deferred_shell`), but the concrete `local_tty` implementation routes through renamed `pub(super)` inherent methods instead: `start_deferred_shell` (`app/src/terminal/local_tty/terminal_manager.rs:594`) and `is_shell_deferred` (`:610`).
- A further gap this design doesn't cover was found and closed in the final whole-branch review's fix wave: a pane group adopted into a *new* window by a tab tear-out (`Workspace::adopt_transferred_pane_group`) never passes through `set_active_tab_index_with_activation` at all — it's swapped directly into an already-visible placeholder tab. That path now calls `start_deferred_shells` at the adoption site itself; see that function's doc comment in `app/src/workspace/view.rs`.

Do not read the steps above as a description of the current code; read this section for that.

---

## Self-Review

**Spec coverage.** The seam (Task 3), parking the inputs (Task 3), starting on demand (Task 3), which panes start eagerly (Task 4), the safety guard for `commands` (Tasks 1-2), feature gating (Task 1), and pane/tab activation (Task 5) are all covered. `app/src/workspace/view.rs` is in the spec's file table for "mark the active tab's panes eager" — Task 4. The spec's fifth file, `vertical_tabs.rs`, is the manual start control, deliberately split into its own plan and flagged above rather than dropped.

Of the spec's Testing section: "deferred pane forks no shell", "first activation forks exactly one", "second activation forks none", "pane with commands forks eagerly", and "flag off forks eagerly" map to Task 2 Step 1 and Task 5 Step 1, restated against `MockTerminalManager` for the reason given in Global Constraints. "Opening a multi-tab configuration forks only the active tab's panes" and "switching to a background tab forks that tab's panes" map to Tasks 4 and 5. The `vertical_tabs_tests.rs` cases belong to the sidebar plan.

**Type consistency.** `ShellStartPolicy` spelled identically throughout; `for_launch_config_tab` takes `is_eager_tab: bool`; `refine_for_pane` takes `has_commands: bool`; `is_deferred()` is used by the mock in Task 2 and defined in Task 1. `has_deferred_shell` and `ensure_shell_started` are declared with default bodies in Task 2 and overridden in Task 2 (mock) and Task 3 (real). `PanesLayout::Template { template, shell_start }` field names match at every construction and destructuring site. `DeferredShellStart`'s five fields are destructured by the same names in `spawn_shell_start`. `start_deferred_shells` is defined in Task 5 Step 3 and called in Steps 4 and the test in Step 1.

**Placeholder scan.** Clean. Every code step carries the actual code; every test step carries the actual assertions; every run step carries the exact command and the expected result. The two steps that are instructions rather than code — Task 2 Step 9 (convert the remaining tuple-variant call sites the compiler reports) and Task 5 Step 8 (measure) — are mechanical compiler-driven cleanup and manual measurement respectively, neither of which can be enumerated in advance without inventing line numbers.

One deliberate non-assertion is worth restating so it is not mistaken for an oversight: nothing in this plan proves a shell was actually forked or not forked, because no test configuration in this repo compiles the PTY. Task 5 Step 8 is the only evidence of the actual behaviour, and it is manual.
