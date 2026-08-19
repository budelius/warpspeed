# Lazy Session Restore Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Panes restored from a saved session defer their shells like launch-config panes do, and a pane carrying startup commands defers too, running its commands when the tab is activated.

**Architecture:** Both changes reuse the shipped `ShellStartPolicy` mechanism rather than adding one. The commands guard is deleted outright — `refine_for_pane` becomes an identity function once the forcing is gone, so it goes rather than being neutered. `PanesLayout::Snapshot` becomes a struct variant carrying a policy, exactly as `Template` already does, and threads it through `restore_pane_tree` → `restore_pane_leaf` → `create_session`.

**Tech Stack:** Rust, in-house `warpui` entity/model framework, `cargo nextest`, `warp_features` feature flags.

## Global Constraints

- **Feature gated:** `FeatureFlag::LazyShellStartup` — the same flag the shipped mechanism uses. With the flag off, behaviour must be byte-for-byte what it is today.
- **Eager set = the window's active tab.** Every other tab defers. Same rule as launch configs.
- **`PanesLayout::SingleTerminal` always forks eagerly.** A new terminal the user just asked for always gets a shell. Do not touch it.
- **A restored pane must keep its original shell.** The restore path already resolves `chosen_shell` from the snapshot and passes it to `create_session`; it must continue to reach the parked state unchanged, or a deferred pane comes back as the default shell while an eager one keeps its own.
- **`local_tty` is compiled everywhere, including tests.** `app/build.rs:231-235` emits `cargo:rustc-cfg=feature="local_tty"` unconditionally for non-wasm targets and does not emit `remote_tty`. Tests exercise the real `TerminalManager<TerminalView>`; `MockTerminalManager` is dead code on that path. Never add `--features local_tty` to a command — it is already on.
- **The build gate is the workspace, not `-p warp`.** A `pub` signature change already broke `crates/warp_tui` past a `-p warp --lib` check once. A literal `cargo check --workspace` cannot complete on this machine (`command-signatures-v2` needs Node/yarn; `integration` needs libclang), so the gate is:
  `cargo check --workspace --exclude command-signatures-v2 --exclude integration`
- **Cargo package is `warp`, not `app`.** Unit tests: `cargo nextest run -p warp --lib`.
- **Every cargo command needs `PROTOC` exported:**
  `export PROTOC="C:/Users/budel/AppData/Local/Microsoft/WinGet/Packages/Google.Protobuf_Microsoft.Winget.Source_8wekyb3d8bbwe/bin/protoc.exe"`
- **21 of ~6300 lib tests already fail** for environmental reasons (mostly Windows symlink privilege, os error 1314). List: `.superpowers/sdd/2026-08-17-lazy-shell-startup/baseline-failures.txt`. Only failures outside that list are yours.

---

## Naming hazard: two unrelated things are called "deferred"

`restore_pane_tree` and `restore_pane_leaf` already take a parameter named `deferred_panes: &mut Vec<(PaneId, LeafSnapshot)>` (`app/src/pane_group/mod.rs:1571`, `:1652`). **It has nothing to do with shell deferral.** It collects AI-document and conversation panes whose *restoration* is postponed until after terminal panes exist (`mod.rs:1657-1676`).

Do not rename it, do not reuse it, and do not thread the shell policy through it. The shell policy is a separate parameter. Naming your additions `shell_start` (matching the shipped `Template` variant) keeps the two distinguishable.

---

### Task 1: Delete the commands guard

**Files:**
- Modify: `app/src/terminal/shell_start_policy.rs:38-43` (delete `refine_for_pane`) and its `pending_commands_force_eager` test
- Modify: `app/src/pane_group/mod.rs:1412-1414` (drop the call), `:874-876` (doc comment referencing it)
- Test: `app/src/pane_group/mod_tests.rs`

**Interfaces:**
- Consumes: `ShellStartPolicy::{Eager, Deferred}`, `ShellStartPolicy::for_launch_config_tab`, `is_deferred`, `TerminalManager::has_deferred_shell` — all shipped.
- Produces: `ShellStartPolicy` with **no** `refine_for_pane` method. Task 2 and Task 3 must not call it.

- [ ] **Step 1: Write the failing test**

Append to `app/src/pane_group/mod_tests.rs`:

```rust
/// A pane carrying startup commands now defers like any other. Its commands
/// stay queued and run when the pane is activated, which is the answer the
/// original design lacked when it forced such panes eager.
#[test]
fn test_deferred_pane_with_commands_parks_its_shell() {
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
            app.read(|ctx| manager.read(ctx, |m, _| m.has_deferred_shell())),
            "a pane with commands must now defer instead of forcing eager"
        );

        // Activating it starts the shell; the queued commands drain from there.
        app.update(|ctx| {
            pane_group.update(ctx, |group, ctx| group.start_deferred_shells(ctx));
        });
        assert!(
            !app.read(|ctx| manager.read(ctx, |m, _| m.has_deferred_shell())),
            "activation starts the parked shell"
        );
    });
}

/// A pane with commands under `Eager` is unchanged — this is the case that
/// must not regress, since it is what every existing config does today.
#[test]
fn test_eager_pane_with_commands_still_starts_immediately() {
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
                    shell_start: ShellStartPolicy::Eager,
                },
                ..Default::default()
            },
        );

        let manager = app
            .read(|ctx| pane_group.read(ctx, |group, ctx| group.terminal_manager(0, ctx)))
            .expect("pane 0 has a terminal manager");

        assert!(
            !app.read(|ctx| manager.read(ctx, |m, _| m.has_deferred_shell())),
            "an eager pane with commands starts immediately"
        );
    });
}
```

- [ ] **Step 2: Run the tests to verify the first one fails**

Run: `cargo nextest run -p warp --lib -E 'test(deferred_pane_with_commands) or test(eager_pane_with_commands)'`
Expected: `test_deferred_pane_with_commands_parks_its_shell` FAILS on `assertion failed: ... a pane with commands must now defer`, because `refine_for_pane` still forces `Eager`. The second test passes already.

- [ ] **Step 3: Delete `refine_for_pane` and its test**

In `app/src/terminal/shell_start_policy.rs`, delete the whole method including its doc comment (lines 38-43):

```rust
    /// Narrows a tab-level policy for one pane.
    ///
    /// A pane with pending startup commands always starts eagerly: deferring it
    /// would raise the question of when those commands run and what the user
    /// sees until they do, and forcing it eager sidesteps that entirely.
    pub fn refine_for_pane(self, has_commands: bool) -> Self {
        if has_commands { Self::Eager } else { self }
    }
```

Delete the `pending_commands_force_eager` test from the same file's `mod tests`. It pins behaviour being removed, and there is no weaker version worth keeping.

- [ ] **Step 4: Drop the call site**

In `app/src/pane_group/mod.rs`, replace lines 1412-1414:

```rust
                // Computed before `commands` is moved into the pending-command
                // queue below. A pane with startup commands never defers.
                let pane_shell_start = shell_start.refine_for_pane(!commands.is_empty());
```

with nothing, and change the later use of `pane_shell_start` (the argument to `PaneGroup::create_session` in the `PaneMode::Terminal | PaneMode::Agent` arm) to `shell_start`.

- [ ] **Step 5: Correct the stale doc comment**

In `app/src/pane_group/mod.rs:874-876`, the `PanesLayout::Template` doc still names the deleted method:

```rust
        /// Policy for this tab's panes, refined per pane by
        /// [`ShellStartPolicy::refine_for_pane`]. Only this variant can defer:
        /// `Snapshot` (session restore) and `SingleTerminal` always start eagerly.
```

Replace with:

```rust
        /// Policy for this tab's panes. Panes carrying startup commands defer
        /// too; their commands stay queued and run when the tab is activated.
```

Leave the `Snapshot`/`SingleTerminal` sentence out — Task 2 makes half of it false, and Task 2 rewrites this doc.

- [ ] **Step 6: Run the tests to verify both pass**

Run: `cargo nextest run -p warp --lib -E 'test(deferred_pane_with_commands) or test(eager_pane_with_commands)'`
Expected: 2 tests PASS.

Then: `cargo nextest run -p warp --lib --no-fail-fast`
Expected: no failures outside the baseline 21. Any test asserting the old guard should have been deleted in Step 3; if others surface, they pin removed behaviour and must be updated to assert deferral rather than deleted wholesale.

Then: `cargo check --workspace --exclude command-signatures-v2 --exclude integration` — clean.
Then: `cargo clippy -p warp --lib -- -D warnings` — clean.

- [ ] **Step 7: Commit**

```bash
git add app/src/terminal/shell_start_policy.rs app/src/pane_group/mod.rs app/src/pane_group/mod_tests.rs
git commit -m "feat(terminal): let panes with startup commands defer their shell"
```

---

### Task 2: Thread the policy through session restore

**Files:**
- Modify: `app/src/pane_group/mod.rs:871` (`PanesLayout::Snapshot` → struct variant), `:1562-1573` (`restore_pane_tree`), `:1642-1652` (`restore_pane_leaf`), `:1750` (the `ShellStartPolicy::Eager` literal), `:3501-3515` (the `initial_layout` Snapshot arm)
- Modify: every other construction site of `PanesLayout::Snapshot` the compiler reports
- Test: `app/src/pane_group/mod_tests.rs`

**Interfaces:**
- Consumes: `ShellStartPolicy` with no `refine_for_pane` (Task 1).
- Produces: `PanesLayout::Snapshot { snapshot: Box<PaneNodeSnapshot>, shell_start: ShellStartPolicy }`; `restore_pane_tree(..., shell_start: ShellStartPolicy, ...)` and `restore_pane_leaf(..., shell_start: ShellStartPolicy, ...)`.

Why a struct variant: it mirrors what `Template` already does, so both deferrable layouts read the same way, and it forces every construction site to state its policy rather than inheriting a default. **Read the naming hazard section above before starting** — `deferred_panes` is an existing, unrelated parameter on both functions.

- [ ] **Step 1: Write the failing test**

Append to `app/src/pane_group/mod_tests.rs`:

```rust
/// A restored pane defers when its policy says to. Restored panes carry
/// scrollback that renders without a PTY, so a parked restored pane shows its
/// history rather than being blank.
#[test]
fn test_restored_pane_defers_when_policy_is_deferred() {
    let _flag = FeatureFlag::LazyShellStartup.override_enabled(true);
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let pane_group = mock_pane_group(
            &mut app,
            MockOptions {
                layout: PanesLayout::Snapshot {
                    snapshot: Box::new(single_terminal_leaf_snapshot()),
                    shell_start: ShellStartPolicy::Deferred,
                },
                ..Default::default()
            },
        );

        let manager = app
            .read(|ctx| pane_group.read(ctx, |group, ctx| group.terminal_manager(0, ctx)))
            .expect("restored pane 0 has a terminal manager");

        assert!(
            app.read(|ctx| manager.read(ctx, |m, _| m.has_deferred_shell())),
            "a restored pane must defer when the policy says Deferred"
        );

        app.update(|ctx| {
            pane_group.update(ctx, |group, ctx| group.start_deferred_shells(ctx));
        });
        assert!(
            !app.read(|ctx| manager.read(ctx, |m, _| m.has_deferred_shell())),
            "activation starts the restored pane's shell"
        );
    });
}

/// Session restore under `Eager` is unchanged. This is the regression guard for
/// every existing restore path.
#[test]
fn test_restored_pane_starts_eagerly_under_eager_policy() {
    let _flag = FeatureFlag::LazyShellStartup.override_enabled(true);
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let pane_group = mock_pane_group(
            &mut app,
            MockOptions {
                layout: PanesLayout::Snapshot {
                    snapshot: Box::new(single_terminal_leaf_snapshot()),
                    shell_start: ShellStartPolicy::Eager,
                },
                ..Default::default()
            },
        );

        let manager = app
            .read(|ctx| pane_group.read(ctx, |group, ctx| group.terminal_manager(0, ctx)))
            .expect("restored pane 0 has a terminal manager");

        assert!(
            !app.read(|ctx| manager.read(ctx, |m, _| m.has_deferred_shell())),
            "an eagerly-restored pane starts its shell immediately"
        );
    });
}
```

Add this helper to the same file. It follows the snapshot shape already used at
`app/src/workspace/view_tests.rs:3101`, with `LeafContents::Terminal` in place of that one's
`GetStarted`. Field list is the real `TerminalPaneSnapshot` (`app/src/app_state.rs:206-219`):

```rust
/// One restored terminal pane, with no blocks, no conversations, and the
/// default shell. `shell_launch_data: None` means "whatever the user's default
/// is" — the same thing an un-customised pane records.
fn single_terminal_leaf_snapshot() -> PaneNodeSnapshot {
    PaneNodeSnapshot::Leaf(LeafSnapshot {
        is_focused: true,
        custom_vertical_tabs_title: None,
        contents: LeafContents::Terminal(TerminalPaneSnapshot {
            uuid: Uuid::new_v4().as_bytes().to_vec(),
            cwd: Some("/".to_string()),
            shell_launch_data: None,
            is_active: true,
            is_read_only: false,
            input_config: None,
            llm_model_override: None,
            active_profile_id: None,
            conversation_ids_to_restore: vec![],
            active_conversation_id: None,
        }),
    })
}
```

Import `TerminalPaneSnapshot`, `LeafSnapshot`, `LeafContents`, and `PaneNodeSnapshot` from
`crate::app_state` if they are not already in scope in that file.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo nextest run -p warp --lib -E 'test(restored_pane_defers) or test(restored_pane_starts_eagerly)'`
Expected: compile error — `PanesLayout::Snapshot` is a tuple variant and does not have fields `snapshot` / `shell_start`.

- [ ] **Step 3: Convert `PanesLayout::Snapshot` to a struct variant**

In `app/src/pane_group/mod.rs`, replace line 871:

```rust
    Snapshot(Box<PaneNodeSnapshot>),
```

with:

```rust
    Snapshot {
        snapshot: Box<PaneNodeSnapshot>,
        /// Policy for this restored tab's panes. Restored panes carry scrollback
        /// that renders without a PTY, so a parked one shows its history.
        shell_start: ShellStartPolicy,
    },
```

And update the `Template` doc comment you edited in Task 1 Step 5 to drop any claim about which variants can defer — both can now.

- [ ] **Step 4: Add the parameter to the two restore functions**

In `restore_pane_tree` (`mod.rs:1562-1573`), add after `model_event_sender`:

```rust
        shell_start: ShellStartPolicy,
```

Do the same for `restore_pane_leaf` (`mod.rs:1642-1652`), and forward `shell_start` from `restore_pane_tree` to both the `Leaf` arm's `restore_pane_leaf` call and every recursive `restore_pane_tree` call in the branch arm. `ShellStartPolicy` is `Copy`, so no clone is needed.

Leave `deferred_panes` exactly as it is — it is unrelated (see the naming hazard section).

- [ ] **Step 5: Use the policy at the `create_session` call**

In `restore_pane_leaf`, replace the hardcoded literal at `mod.rs:1750`:

```rust
                    ShellStartPolicy::Eager,
```

with:

```rust
                    shell_start,
```

Do not touch the `chosen_shell` argument two lines above it. That is what carries the restored pane's original shell into the parked state, and it must keep flowing unchanged — otherwise a deferred pane comes back as the default shell while an eager one keeps its own.

- [ ] **Step 6: Update the `initial_layout` Snapshot arm**

In `mod.rs:3501-3515`, destructure the new fields and forward the policy:

```rust
                PanesLayout::Snapshot {
                    snapshot,
                    shell_start,
                } => {
                    let mut deferred_panes = Vec::new();
                    let mut pending_restorations = Vec::new();
                    let result = Self::restore_pane_tree(
                        *snapshot,
                        block_lists,
                        resources.clone(),
                        ctx,
                        pane_contents,
                        unsupported_banner_model_handle.clone(),
                        view_bounds.size(),
                        model_event_sender_clone.clone(),
                        &mut deferred_panes,
                        &mut pending_restorations,
                        shell_start,
                    )
```

Match the real argument order of `restore_pane_tree` after your Step 4 edit — the block above assumes `shell_start` went last; if you put it elsewhere, place it consistently.

- [ ] **Step 7: Fix the remaining construction sites**

Run: `cargo check -p warp --lib --tests 2>&1 | grep -A3 'PanesLayout::Snapshot'`

Convert every hit to the struct form. Production sites (session restore in `app/src/workspace/view.rs`) get `ShellStartPolicy::Eager` **for now** — Task 3 replaces the one that matters. Test sites get `ShellStartPolicy::Eager`, which preserves whatever those tests already assert.

- [ ] **Step 8: Run the tests to verify they pass**

Run: `cargo nextest run -p warp --lib -E 'test(restored_pane_defers) or test(restored_pane_starts_eagerly)'`
Expected: 2 tests PASS.

Then: `cargo nextest run -p warp --lib --no-fail-fast` — no failures outside the baseline 21.
Then: `cargo check --workspace --exclude command-signatures-v2 --exclude integration` — clean.
Then: `cargo clippy -p warp --lib -- -D warnings` — clean.

- [ ] **Step 9: Commit**

```bash
git add app/src/pane_group/mod.rs app/src/pane_group/mod_tests.rs app/src/workspace/view.rs
git commit -m "feat(pane_group): carry ShellStartPolicy through session restore"
```

---

### Task 3: Defer every restored tab but the active one

**Files:**
- Modify: `app/src/workspace/view.rs` — the `NewWorkspaceSource::Restored` arm of `configure_new_workspace` (around `:3919-4011`, locate by surrounding code)
- Test: `app/src/workspace/view_tests.rs`

**Interfaces:**
- Consumes: `PanesLayout::Snapshot { snapshot, shell_start }` (Task 2), `ShellStartPolicy::for_launch_config_tab(is_eager_tab: bool)`, `TerminalManager::has_deferred_shell`.
- Produces: no new API.

`for_launch_config_tab` is named for its first caller but its contract is general — "is this the tab that will be visible" — and it already returns `Eager` unconditionally when the flag is off. Reuse it rather than adding a near-duplicate. Do **not** rename it as part of this task; a rename would touch the shipped launch-config path for no behavioural gain.

Unlike `open_launch_config_window`, this arm already has `active_tab_index` in hand before the tabs are built (`view.rs:3923`), so no up-front clamp is needed — read the surrounding code and use the same index the arm later passes to `activate_tab_internal`.

- [ ] **Step 1: Write the failing test**

Append to `app/src/workspace/view_tests.rs`, following the conventions the existing launch-config tests in that file already use (`mock_workspace`, `initialize_app`, `start_index` slicing, `let _flag = ...` guard bindings):

```rust
/// Restoring a multi-tab session parks every tab but the one it activates.
#[test]
fn test_session_restore_defers_all_but_active_tab() {
    let _flag = FeatureFlag::LazyShellStartup.override_enabled(true);
    App::test((), |mut app| async move {
        let workspace = mock_workspace(&mut app);

        let snapshot = window_snapshot_with_tabs(3, /* active_tab_index */ 1);

        app.update(|ctx| {
            workspace.update(ctx, |ws, ctx| {
                ws.restore_window_snapshot_for_test(snapshot, ctx);
            });
        });

        let deferred: Vec<bool> = app.read(|ctx| {
            workspace.read(ctx, |ws, ctx| {
                ws.tabs
                    .iter()
                    .map(|tab| {
                        tab.pane_group
                            .read(ctx, |group, ctx| group.terminal_manager(0, ctx))
                            .expect("each restored tab has one terminal pane")
                            .read(ctx, |m, _| m.has_deferred_shell())
                    })
                    .collect()
            })
        });

        assert_eq!(
            deferred,
            vec![true, false, true],
            "only the restored active tab (index 1) starts its shells"
        );
    });
}
```

**On the two helpers this test names.** `add_get_started_tab` (`view_tests.rs:3099-3110`) shows the
established way to add a snapshot-backed tab to an existing workspace: build a
`PanesLayout::Snapshot` and hand it to `workspace.add_tab_with_pane_layout`. Prefer that shape.

If driving `configure_new_workspace`'s `Restored` arm directly from a test turns out to be
impractical, **do not add a production method purely to make it testable.** Instead build the three
tabs with `add_tab_with_pane_layout`, passing the policy each tab should get — which tests the same
decision this task implements, one layer down — and say in your report that you did so and why. The
assertion stays `vec![true, false, true]` either way.

Reuse the `single_terminal_leaf_snapshot()` helper from Task 2 rather than writing a second builder;
if it lives in `mod_tests.rs` and is not reachable from `view_tests.rs`, copy it rather than making
it `pub`.

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo nextest run -p warp --lib -E 'test(session_restore_defers_all_but_active_tab)'`
Expected: FAIL with `left: [false, false, false], right: [true, false, true]`, because Task 2 left this site hardcoded to `Eager`.

- [ ] **Step 3: Choose the eager tab in the Restored arm**

In the `NewWorkspaceSource::Restored` arm, the tabs are built from `window_snapshot`. For each tab, pass:

```rust
                        PanesLayout::Snapshot {
                            snapshot: Box::new(tab_snapshot),
                            shell_start: ShellStartPolicy::for_launch_config_tab(
                                tab_index == active_tab_index,
                            ),
                        },
```

where `tab_index` is the enumeration index of the tab being restored and `active_tab_index` is the value the arm already reads from `window_snapshot.active_tab_index` and later hands to `activate_tab_internal`. Read the surrounding code and use the existing binding rather than re-reading the field, so the eagerly-started tab and the activated tab cannot drift apart.

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo nextest run -p warp --lib -E 'test(session_restore_defers_all_but_active_tab)'`
Expected: PASS.

Then: `cargo nextest run -p warp --lib --no-fail-fast` — no failures outside the baseline 21.
Then: `cargo check --workspace --exclude command-signatures-v2 --exclude integration` — clean.
Then: `cargo clippy -p warp --lib -- -D warnings` — clean.

- [ ] **Step 5: Commit**

```bash
git add app/src/workspace/view.rs app/src/workspace/view_tests.rs
git commit -m "feat(workspace): defer shell startup for non-active restored tabs"
```

- [ ] **Step 6: Measure**

Unit tests observe parked state, not forks — the same limit the shipped feature had. This step is the only evidence the feature does what it exists to do, and it is manual.

Build: `cargo build -p warp --release --bin warpspeed`

Then, with `restore_session = true` in WarpSpeed's settings: open a large session, quit, reopen, and before switching tabs run:

```powershell
.\measure-lazy-shell.ps1 -Exe warpspeed -Phase restored
```

Baseline is one shell per restored pane. Target is the active tab's pane count. Record the numbers in the commit message or PR body. If the count still matches the pane total, the policy is not reaching the restore path — check the `Restored` arm before assuming the deferral itself failed.

---

## Self-Review

**Spec coverage.** The commands guard removal is Task 1 (spec's "Removing the commands guard"). Session-restore threading is Task 2 ("Deferring session restore"). Eager-tab selection is Task 3. Feature gating is inherited — every task reuses `FeatureFlag::LazyShellStartup` via `for_launch_config_tab`, which returns `Eager` when the flag is off, so no task needs its own gate. `SingleTerminal` staying eager is covered by not touching it, and Task 2 Step 7 explicitly leaves non-restore sites `Eager`. The spec's shell-fidelity risk is addressed in Task 2 Step 5, which forbids touching `chosen_shell`. The spec's verification section is Task 3 Step 6.

**Placeholder scan.** Clean. Both fixture hedges from the first draft were replaced with real code after checking the tree: Task 2 Step 1 now carries a complete `single_terminal_leaf_snapshot()` built from the actual `TerminalPaneSnapshot` fields (`app/src/app_state.rs:206-219`), and Task 3 Step 1 points at the existing `add_get_started_tab` pattern (`view_tests.rs:3099-3110`) with a named fallback and an instruction not to widen production visibility for testing.

**Type consistency.** `ShellStartPolicy` spelled identically throughout. `PanesLayout::Snapshot { snapshot, shell_start }` field names match at the definition (Task 2 Step 3), the `initial_layout` destructuring (Step 6), and both test construction sites (Step 1). `restore_pane_tree` and `restore_pane_leaf` both gain `shell_start: ShellStartPolicy` (Step 4) and it is consumed at `create_session` (Step 5). `for_launch_config_tab(is_eager_tab: bool)` matches the shipped signature. `has_deferred_shell()` and `start_deferred_shells(ctx)` are used as shipped. No task references `refine_for_pane` after Task 1 deletes it.

**Known risk this plan does not close.** Nothing here asserts a shell actually *started* — every assertion checks `has_deferred_shell()` flipping. Mutating the spawn to `drop(parked)` would keep these tests green while every deferred pane went permanently blank. That gap is inherited from the shipped design, is recorded in its plan, and needs a new observable to close; it belongs with the sidebar-indicator work, not here.
