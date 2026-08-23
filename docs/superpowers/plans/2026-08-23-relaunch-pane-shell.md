# Relaunching a Pane's Shell Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A pane can restart its shell in place, or switch to a different shell, keeping its scrollback, its working directory, and its agent conversation.

**Architecture:** A relaunch is guard → capture → tear down → re-park → start. Phases after teardown reuse the merged lazy-restore machinery (`DeferredShellStart`, `start_deferred_shell`) rather than adding a second way to start a shell in a pane. The pane's `blocklist` is never touched, which is what keeps scrollback and agent history safe. Restart and switch-shell are one code path differing only in the `ShellLaunchData` passed.

**Tech Stack:** Rust, in-house `warpui` entity/model framework, `cargo nextest`, `warp_features` feature flags.

**Spec:** `docs/superpowers/specs/2026-08-23-relaunch-pane-shell-design.md`

## Global Constraints

- **Base branch is `dev`** at `7f2a6aa2` or later. It carries the merged lazy-restore work. `main` does **not** have `DeferredShellStart`; a branch cut from `main` cannot build this.
- **Every cargo command needs `PROTOC` exported:**
  `export PROTOC="C:/Users/budel/AppData/Local/Microsoft/WinGet/Packages/Google.Protobuf_Microsoft.Winget.Source_8wekyb3d8bbwe/bin/protoc.exe"`
- **Cargo package is `warp`, not `app`.** Unit tests: `cargo nextest run -p warp --lib`. The path converters live in a different crate: `cargo nextest run -p warp_util`.
- **`local_tty` is compiled everywhere, including tests.** `app/build.rs` emits it unconditionally for non-wasm targets. Never add `--features local_tty` — it is already on.
- **The build gate is the workspace, not `-p warp`:**
  `cargo check --workspace --exclude command-signatures-v2 --exclude integration`
  A `pub` signature change has broken `crates/warp_tui` past a `-p warp` check before.
- **Clippy gate:** `cargo clippy -p warp --lib -- -D warnings`
- **21 lib tests fail at baseline** for environmental reasons (Windows symlink privilege, os error 1314). List: `.superpowers/sdd/2026-08-17-lazy-shell-startup/baseline-failures.txt`. One test reports LEAK rather than FAIL. Only failures outside that set are yours.
- **Run every cargo command in the foreground.** Builds take minutes; the full suite takes about seven. There is no background-notification mechanism for subagents.
- **Do not stage `app/Cargo.toml` or `app/src/bin/warpspeed.rs`** unless your task names them.

---

## Naming hazard: two unrelated things are called "deferred"

`restore_pane_tree` and `restore_pane_leaf` take a parameter named `deferred_panes: &mut Vec<(PaneId, LeafSnapshot)>`. **It has nothing to do with shell deferral** — it collects AI-document and conversation panes whose *restoration* is postponed. Do not rename it, reuse it, or thread relaunch state through it.

---

### Task 1: Reverse path converters

Pure functions in a separate crate, with no dependency on the rest of this plan. Start here.

**Files:**
- Modify: `crates/warp_util/src/path.rs`
- Test: `crates/warp_util/src/path_tests.rs` (wired in at `path.rs:954-955` via `#[path = "path_tests.rs"] mod tests;`)

**Interfaces:**
- Consumes: `parse_wsl_unc_path(path: &Path) -> Option<WslUncPath>` (`path.rs:537`), `WslUncPath { distro: String, linux_path: String }`, `WSLPathConversionError` (`path.rs:454`).
- Produces:
  - `pub fn convert_windows_host_to_wsl_path(host_path: &Path, distro_name: &str) -> Result<String, WSLPathConversionError>`
  - `pub fn convert_windows_native_to_msys2_path(host_path: &Path, msys2_root: &Path) -> Option<String>`

  Both return the shell-native path as a `String`, not a `PathBuf`: a WSL path is not a valid `std::path::Path` on Windows, and round-tripping it through `PathBuf` would mangle separators.

**Read first:** `convert_wsl_to_windows_host_path` (`path.rs:465-509`). It has exactly two branches — `/mnt/<drive>/…` → `<DRIVE>:\…`, and everything else → `\\WSL$\<distro>\…`. Your function inverts both. The second inverse already exists as `parse_wsl_unc_path`; you are writing the first and a dispatcher.

- [ ] **Step 1: Write the failing tests**

Append to `crates/warp_util/src/path_tests.rs`:

```rust
#[test]
fn windows_drive_path_converts_to_wsl_mnt_path() {
    let result = convert_windows_host_to_wsl_path(Path::new(r"C:\projects\warp"), "Ubuntu")
        .expect("a drive-letter path converts");
    assert_eq!(result, "/mnt/c/projects/warp");
}

#[test]
fn windows_drive_letter_is_lowercased_for_wsl() {
    let result = convert_windows_host_to_wsl_path(Path::new(r"D:\Data"), "Ubuntu")
        .expect("an upper-case drive converts");
    assert_eq!(result, "/mnt/d/Data", "the drive lowercases; the rest keeps its case");
}

#[test]
fn wsl_unc_path_converts_back_to_its_linux_path() {
    let result = convert_windows_host_to_wsl_path(Path::new(r"\\wsl$\Ubuntu\home\me\src"), "Ubuntu")
        .expect("a UNC path converts");
    assert_eq!(result, "/home/me/src");
}

#[test]
fn wsl_localhost_spelling_is_accepted() {
    let result =
        convert_windows_host_to_wsl_path(Path::new(r"\\wsl.localhost\Ubuntu\home\me"), "Ubuntu")
            .expect("the wsl.localhost spelling converts");
    assert_eq!(result, "/home/me");
}

#[test]
fn relative_path_is_rejected() {
    assert!(
        convert_windows_host_to_wsl_path(Path::new(r"projects\warp"), "Ubuntu").is_err(),
        "a relative path has no WSL equivalent"
    );
}

#[test]
fn drive_path_round_trips_through_the_forward_converter() {
    let wsl = convert_windows_host_to_wsl_path(Path::new(r"C:\projects\warp"), "Ubuntu")
        .expect("converts to WSL form");
    let back = convert_wsl_to_windows_host_path(&TypedPath::unix(&wsl), "Ubuntu")
        .expect("converts back");
    assert_eq!(back, PathBuf::from(r"C:\projects\warp"));
}
```

Add whatever `use` items the file lacks — at minimum `std::path::{Path, PathBuf}` and `typed_path::TypedPath`. Check the top of `path_tests.rs` before adding: several are likely already imported.

**On the round-trip test:** `convert_wsl_to_windows_host_path` calls `std::fs::read_link` on its drive-letter branch (`path.rs:492-495`), so a path that is a symlink on this machine will not round-trip. `C:\projects\warp` is chosen because it is the repo root and a real directory. If this test proves flaky on a machine where that path is a symlink, replace the literal with a `tempfile::TempDir` path rather than deleting the test.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo nextest run -p warp_util -E 'test(wsl)'`
Expected: compile error — `convert_windows_host_to_wsl_path` is not defined.

- [ ] **Step 3: Implement the WSL converter**

Add to `crates/warp_util/src/path.rs`, directly below `parse_wsl_unc_path`:

```rust
/// Converts a Windows host path to the path a shell inside the given WSL distribution would use.
///
/// The inverse of [`convert_wsl_to_windows_host_path`], which has two branches and therefore so
/// does this: a drive-letter path maps to `/mnt/<drive>/…`, and a `\\wsl$\<distro>\…` UNC path
/// maps back to its Linux path via [`parse_wsl_unc_path`].
///
/// Returns a `String` rather than a `PathBuf` because a Linux path is not a valid Windows
/// `Path`; converting would mangle the separators.
///
/// Not a perfect inverse: the forward converter resolves symlinks with `std::fs::read_link` on
/// the drive-letter branch, and that cannot be undone here.
pub fn convert_windows_host_to_wsl_path(
    host_path: &Path,
    distro_name: &str,
) -> Result<String, WSLPathConversionError> {
    // A UNC path inside the distribution round-trips through the existing parser.
    if let Some(parsed) = parse_wsl_unc_path(host_path) {
        return Ok(parsed.linux_path);
    }

    let path_str = host_path
        .to_str()
        .ok_or(WSLPathConversionError::NonUnixPath)?;
    let windows_path = Utf8WindowsPath::new(path_str);
    if !windows_path.is_absolute() {
        return Err(WSLPathConversionError::PathNotAbsolute);
    }

    let mut components = windows_path.components();
    let drive = match components.next() {
        Some(Utf8WindowsComponent::Prefix(prefix)) => match prefix.kind() {
            Utf8WindowsPrefix::Disk(letter) | Utf8WindowsPrefix::VerbatimDisk(letter) => {
                (letter as char).to_ascii_lowercase()
            }
            _ => return Err(WSLPathConversionError::NonUnixPath),
        },
        _ => return Err(WSLPathConversionError::PathNotAbsolute),
    };

    let mut linux_path = format!("/mnt/{drive}");
    for component in components {
        match component {
            Utf8WindowsComponent::RootDir => {}
            component => {
                linux_path.push('/');
                linux_path.push_str(component.as_str());
            }
        }
    }
    Ok(linux_path)
}
```

`Utf8WindowsPath`, `Utf8WindowsComponent`, and `Utf8WindowsPrefix` are already imported in this file — `parse_wsl_unc_path` uses all three. `WSLPathConversionError` has no variant meaning "not a drive path"; reusing `NonUnixPath` is the closest fit. If you would rather add a variant, add it — it is a `thiserror` enum at `path.rs:454` and adding a variant is cheap — but then update every match on it that the compiler reports.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo nextest run -p warp_util -E 'test(wsl)'`
Expected: all six PASS.

- [ ] **Step 5: Write the failing MSYS2 test**

Append to `crates/warp_util/src/path_tests.rs`:

```rust
#[test]
fn windows_drive_path_converts_to_msys2_path() {
    let root = Path::new(r"C:\msys64");
    let result = convert_windows_native_to_msys2_path(Path::new(r"C:\projects\warp"), root)
        .expect("a drive-letter path converts");
    assert_eq!(result, "/c/projects/warp");
}

#[test]
fn path_inside_the_msys2_root_converts_to_an_absolute_msys2_path() {
    let root = Path::new(r"C:\msys64");
    let result = convert_windows_native_to_msys2_path(Path::new(r"C:\msys64\home\me"), root)
        .expect("a path under the root converts");
    assert_eq!(result, "/home/me", "the MSYS2 root is the filesystem root");
}

#[test]
fn msys2_conversion_rejects_a_relative_path() {
    let root = Path::new(r"C:\msys64");
    assert!(convert_windows_native_to_msys2_path(Path::new(r"projects\warp"), root).is_none());
}
```

- [ ] **Step 6: Run to verify it fails, then implement the MSYS2 converter**

Run: `cargo nextest run -p warp_util -E 'test(msys2)'` — expect a compile error.

**Read first:** `convert_msys2_to_windows_native_path` (`path.rs:369`) and `msys2_exe_to_root`. Mirror the root-handling exactly: whatever the forward function treats as the root prefix, this must strip, and whatever drive spelling it emits, this must produce. Write the implementation to satisfy the three tests above, matching the forward function's conventions rather than inventing new ones.

Note the ordering that matters: a path under the MSYS2 root must be tested **before** the drive-letter branch, or `C:\msys64\home\me` becomes `/c/msys64/home/me` instead of `/home/me`.

- [ ] **Step 7: Run all converter tests**

Run: `cargo nextest run -p warp_util`
Expected: all pass, no failures.

Then: `cargo check --workspace --exclude command-signatures-v2 --exclude integration` — clean.
Then: `cargo clippy -p warp_util -- -D warnings` — clean.

- [ ] **Step 8: Commit**

```bash
git add crates/warp_util/src/path.rs crates/warp_util/src/path_tests.rs
git commit -m "feat(path): convert Windows host paths to WSL and MSYS2 form"
```

---

### Task 2: End a session without dropping the manager

The one genuinely new primitive. Everything after this composes from reviewed code.

**Files:**
- Modify: `app/src/terminal/local_tty/terminal_manager.rs`
- Test: `app/src/terminal/local_tty/terminal_manager_tests.rs` if it exists; otherwise add the tests to the existing `mod tests` in `terminal_manager.rs`. Check first — do not create a second test module.

**Interfaces:**
- Consumes: `shutdown_event_loop(&mut self)` (`terminal_manager.rs:629`), `DeferredShellStart` (`:190`), the `deferred_shell` field.
- Produces:
  - `pub(super) fn end_session(&mut self) -> bool` — tears the session down, leaves the manager alive. Returns `false` if the event loop could not be joined.
  - `pub(super) fn repark_shell(&mut self, parked: DeferredShellStart)` — sets `deferred_shell = Some(parked)`.

**Read first:** `shutdown_event_loop` (`:629-652`) and the `Drop` impl above it. `shutdown_event_loop` sends `Message::Shutdown`, tolerates an already-closed channel, and joins `event_loop_handle` via `Option::take`. Because it takes the handle, calling it twice is already safe — the second call logs a warning and returns. Confirm that by reading it rather than trusting this sentence.

- [ ] **Step 1: Write the failing test**

Add to the terminal manager's test module:

```rust
/// Ending a session leaves the manager usable and re-parkable. This is the
/// property the whole relaunch feature rests on: `Drop` is not an option,
/// because the pane and its blocklist must outlive the shell.
#[test]
fn end_session_leaves_the_manager_re_parkable() {
    // Build a manager with an eagerly started shell, end its session, then
    // re-park it and confirm the parked state is observable again.
    // Follow the construction used by the existing tests in this module.
}
```

**This test body must be written against the module's existing helpers, not invented.** Before writing it, read how the other tests in this module construct a `TerminalManager` and drive it. If the module has no such helper, the manager is only reachable through `PaneGroup`, in which case write this test in `app/src/pane_group/mod_tests.rs` using `mock_pane_group` — the same harness the lazy-restore tests use (see `test_deferred_pane_with_commands_parks_its_shell`) — and assert through `has_deferred_shell()`.

Report in your task report which of the two locations you used and why.

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo nextest run -p warp --lib -E 'test(end_session)'`
Expected: compile error — `end_session` is not defined.

- [ ] **Step 3: Implement `end_session` and `repark_shell`**

Add to `impl<S> TerminalManager<S>`, directly below `shutdown_event_loop`:

```rust
    /// Ends the running session while keeping the manager alive.
    ///
    /// [`Self::shutdown_event_loop`] does the same teardown from `Drop`, but a
    /// relaunch cannot drop the manager: the pane, its blocklist, and its agent
    /// conversation must outlive the shell. Returns whether teardown completed;
    /// a caller must not park a new shell over a session that failed to end.
    ///
    /// Safe to call more than once. `shutdown_event_loop` takes
    /// `event_loop_handle` out of its `Option`, so a second call finds nothing
    /// to join and returns.
    pub(super) fn end_session(&mut self) -> bool {
        if self.event_loop_handle.is_none() {
            // Already torn down, or never started. Nothing to do, and reporting
            // success is correct: the postcondition (no live session) holds.
            return true;
        }
        self.shutdown_event_loop();
        self.event_loop_handle.is_none()
    }

    /// Parks a shell start on a manager whose session has ended, so
    /// [`Self::start_deferred_shell`] can fork it.
    ///
    /// The write counterpart to the crate's single `Option::take` on
    /// `deferred_shell` (see [`Self::start_deferred_shell`]). Parking over an
    /// existing parked shell would silently discard it, so this asserts in debug
    /// builds rather than losing a start.
    pub(super) fn repark_shell(&mut self, parked: DeferredShellStart) {
        debug_assert!(
            self.deferred_shell.is_none(),
            "re-parking over an already-parked shell discards the first"
        );
        self.deferred_shell = Some(parked);
    }
```

If `shutdown_event_loop` turns out not to leave `event_loop_handle` as `None` on failure, the `is_none()` return is wrong — read it and adjust the success check to match what it actually guarantees. Say so in your report if you change it.

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo nextest run -p warp --lib -E 'test(end_session)'`
Expected: PASS.

- [ ] **Step 5: Full verification**

Run: `cargo nextest run -p warp --lib --no-fail-fast` — no failures outside the baseline 21.
Then: `cargo check --workspace --exclude command-signatures-v2 --exclude integration` — clean.
Then: `cargo clippy -p warp --lib -- -D warnings` — clean.

- [ ] **Step 6: Commit**

```bash
git add app/src/terminal/local_tty/terminal_manager.rs
git commit -m "feat(terminal): end a session without dropping the manager"
```

---

### Task 3: Relaunch a pane's shell

Wires Tasks 1 and 2 together. No UI yet — this is the model-layer operation the commands will call.

**Files:**
- Modify: `app/src/terminal/terminal_manager.rs` (the trait), `app/src/terminal/local_tty/terminal_view_adaptor.rs` (the concrete impl), `app/src/terminal/local_tty/terminal_manager.rs`
- Test: `app/src/pane_group/mod_tests.rs`

**Interfaces:**
- Consumes: `end_session`, `repark_shell` (Task 2); `convert_windows_host_to_wsl_path`, `convert_windows_native_to_msys2_path` (Task 1); `start_deferred_shell` (`terminal_manager.rs:597`); `ShellLaunchData::{Executable, WSL, MSYS2}` (`crates/warp_terminal/src/shell/mod.rs:783`).
- Produces: `fn relaunch_shell(&mut self, target: Option<ShellLaunchData>, cwd: Option<PathBuf>, ctx: &mut ModelContext<Box<dyn TerminalManager>>)` on `TerminalManagerTrait`, defaulting to a no-op like `ensure_shell_started` does (`app/src/terminal/terminal_manager.rs:42`). `target: None` means "same shell".

**Read first:** how `ensure_shell_started` and `has_deferred_shell` are declared on the trait (`app/src/terminal/terminal_manager.rs:36-47`) and implemented in `terminal_view_adaptor.rs`. Follow that shape exactly: a defaulted trait method, with the real body living once in `TerminalManager` and both impls forwarding to it. The existing comment explains why the body is shared rather than duplicated — the same reasoning applies here.

- [ ] **Step 1: Write the failing tests**

Append to `app/src/pane_group/mod_tests.rs`:

```rust
/// Relaunching preserves the pane's blocklist. This is the guarantee the whole
/// feature rests on: scrollback and agent history survive because nothing in the
/// relaunch path touches them.
#[test]
fn test_relaunch_preserves_the_blocklist() {
    let _flag = FeatureFlag::LazyShellStartup.override_enabled(true);
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let pane_group = mock_pane_group(&mut app, MockOptions::default());
        let manager = app
            .read(|ctx| pane_group.read(ctx, |group, ctx| group.terminal_manager(0, ctx)))
            .expect("pane 0 has a terminal manager");

        let before = app.read(|ctx| {
            manager.read(ctx, |m, ctx| m.block_list().as_ref(ctx).len())
        });

        app.update(|ctx| {
            manager.update(ctx, |m, ctx| m.relaunch_shell(None, None, ctx));
        });

        let after = app.read(|ctx| {
            manager.read(ctx, |m, ctx| m.block_list().as_ref(ctx).len())
        });
        assert_eq!(before, after, "relaunching must not disturb the blocklist");
    });
}

/// A relaunch parks a new shell start, which is what `start_deferred_shell` then forks.
#[test]
fn test_relaunch_parks_a_new_shell_start() {
    let _flag = FeatureFlag::LazyShellStartup.override_enabled(true);
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let pane_group = mock_pane_group(&mut app, MockOptions::default());
        let manager = app
            .read(|ctx| pane_group.read(ctx, |group, ctx| group.terminal_manager(0, ctx)))
            .expect("pane 0 has a terminal manager");

        assert!(
            !app.read(|ctx| manager.read(ctx, |m, _| m.has_deferred_shell())),
            "a fresh eager pane has no parked shell"
        );

        app.update(|ctx| {
            manager.update(ctx, |m, ctx| m.relaunch_shell(None, None, ctx));
        });

        assert!(
            app.read(|ctx| manager.read(ctx, |m, _| m.has_deferred_shell())),
            "relaunch parks a start for the new shell"
        );
    });
}
```

`block_list()` accessor spelling and `len()` may differ — read how the existing tests in this file reach a manager's block list and match that. If no such accessor is reachable from the test, assert on the pane's conversation id instead, which `TerminalPaneSnapshot` exposes. Do not delete the test; report what you used.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo nextest run -p warp --lib -E 'test(relaunch)'`
Expected: compile error — `relaunch_shell` is not defined.

- [ ] **Step 3: Add the trait method**

In `app/src/terminal/terminal_manager.rs`, beside `ensure_shell_started`:

```rust
    /// Ends this manager's session and parks a fresh start for `target`, or for
    /// the current shell when `target` is `None`.
    ///
    /// `cwd` is the directory the new shell should start in, in **host** form.
    /// It is converted to the target shell's native form here, because only this
    /// method knows which shell is being started. Managers that cannot relaunch
    /// keep the default no-op.
    fn relaunch_shell(
        &mut self,
        _target: Option<ShellLaunchData>,
        _cwd: Option<PathBuf>,
        _ctx: &mut ModelContext<Box<dyn TerminalManager>>,
    ) {
    }
```

- [ ] **Step 4: Implement the body once in `TerminalManager`**

In `app/src/terminal/local_tty/terminal_manager.rs`, beside `start_deferred_shell`:

```rust
    /// The sole body of `TerminalManagerTrait::relaunch_shell`. Shared for the
    /// same reason `start_deferred_shell` is: one implementation, two impls
    /// forwarding to it, so deleting it is a compile error rather than silent
    /// divergence.
    pub(super) fn relaunch_shell_impl(
        &mut self,
        target: Option<ShellLaunchData>,
        cwd: Option<PathBuf>,
        ctx: &mut ModelContext<Box<dyn TerminalManagerTrait>>,
    ) where
        S: TerminalSurface,
        <S as Entity>::Event: PtyIntentEvent,
    {
        if !self.end_session() {
            log::error!("Not relaunching: the previous session did not end cleanly");
            return;
        }

        let parked = match self.build_relaunch_parked_state(target, cwd) {
            Some(parked) => parked,
            None => {
                log::error!("Not relaunching: could not build a shell start");
                return;
            }
        };
        self.repark_shell(parked);
        self.start_deferred_shell(ctx);
    }
```

Then write `build_relaunch_parked_state`, which constructs a `DeferredShellStart` from the manager's current fields plus the requested target and directory. `DeferredShellStart`'s five fields are listed at `terminal_manager.rs:190-198`: `startup_directory`, `env_vars`, `user_default_shell_unsupported_banner_model_handle`, `shell_startup_resources`, and `wsl_name_or_shell_starter`.

Two of those need thought and the rest carry over from the manager unchanged:

- `wsl_name_or_shell_starter` is where the shell choice lives. For `target: None`, reuse what the manager already resolved. For `Some(shell)`, derive it from the `ShellLaunchData` — read how `create_model_with_manager` builds this value from `chosen_shell` and mirror it.
- `startup_directory` must be in the target shell's **native** form, not host form. This is verified, not assumed: `on_shell_determined` (`terminal_manager.rs:713-719`) substitutes `wsl_shell_starter.home_directory()` only when `startup_directory` is `None`, and otherwise passes the value straight through. Convert with Task 1's functions:

```rust
    /// Converts a host-form directory to the form the target shell expects.
    ///
    /// Verified against `on_shell_determined`, which passes a `Some`
    /// `startup_directory` straight to the shell: a host path handed to a WSL
    /// shell lands the user in the wrong directory.
    ///
    /// Returns `None` when conversion fails, which parks the shell with no
    /// startup directory -- the shell then uses its own default, which is the
    /// documented fallback for a directory that no longer resolves.
    fn native_startup_directory(cwd: &Path, shell: &ShellLaunchData) -> Option<PathBuf> {
        match shell {
            ShellLaunchData::Executable { .. } => Some(cwd.to_path_buf()),
            ShellLaunchData::WSL { distro } => {
                convert_windows_host_to_wsl_path(cwd, distro).ok().map(PathBuf::from)
            }
            ShellLaunchData::MSYS2 { executable_path, .. } => {
                let root = msys2_exe_to_root(WindowsPath::new(
                    executable_path.as_os_str().as_encoded_bytes(),
                ));
                convert_windows_native_to_msys2_path(cwd, Path::new(&root)).map(PathBuf::from)
            }
            _ => None,
        }
    }
```

`ShellLaunchData` has variants beyond these three (`crates/warp_terminal/src/shell/mod.rs:783-796` includes a Docker sandbox variant). The `_ => None` arm covers them: no conversion is known, so the shell starts in its own default directory. If the compiler warns the arm is unreachable, list the remaining variants explicitly instead.

The `msys2_exe_to_root` / `WindowsPath` handling is copied from `ShellLaunchData::maybe_convert_absolute_path` (`crates/warp_terminal/src/shell/mod.rs:811-830`); read that and mirror its imports.

- [ ] **Step 5: Forward from both trait impls**

Add `relaunch_shell` to the concrete impl in `terminal_view_adaptor.rs` and to `TuiTerminalManager`'s forwarding impl, both calling `relaunch_shell_impl`. Follow exactly how those two files forward `ensure_shell_started`.

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo nextest run -p warp --lib -E 'test(relaunch)'`
Expected: 2 PASS.

Then: `cargo nextest run -p warp --lib --no-fail-fast` — no failures outside the baseline 21.
Then: `cargo check --workspace --exclude command-signatures-v2 --exclude integration` — clean.
Then: `cargo clippy -p warp --lib -- -D warnings` — clean.

- [ ] **Step 7: Commit**

```bash
git add app/src/terminal app/src/pane_group/mod_tests.rs
git commit -m "feat(terminal): relaunch a pane's shell in place"
```

---

### Task 4: The two commands and the confirmation

**Files:**
- Modify: `app/src/workspace/view.rs` (actions), and the file declaring `WorkspaceAction`
- Create: `app/src/workspace/restart_shell_confirmation_modal.rs`
- Test: `app/src/workspace/view_tests.rs`

**Interfaces:**
- Consumes: `relaunch_shell` (Task 3); `AvailableShells::handle(ctx).get_available_shells()` and `display_name_for_shell` (used at `app/src/workspace/view.rs:6931`); `block_list().active_block().is_active_and_long_running()` (`app/src/terminal/input.rs:5016`).
- Produces: `WorkspaceAction::RestartShell` and `WorkspaceAction::ChangeShell { shell: ShellLaunchData }`.

Neither command gets a default keybinding. The command palette is the entry point.

**Read first:** `app/src/settings_view/transfer_ownership_confirmation_modal.rs` in full. It is the pattern for the confirmation: a small `Entity` with a `ModalViewState<Modal<T>>` field on the host view, added with `ctx.add_typed_action_view(..)`. Mirror its structure rather than inventing one.

- [ ] **Step 1: Write the failing test**

Append to `app/src/workspace/view_tests.rs`:

```rust
/// Relaunching a pane with nothing running does not prompt. The confirmation
/// exists to protect work in progress, so it must not appear when there is none.
#[test]
fn test_restart_shell_does_not_prompt_when_nothing_is_running() {
    let _flag = FeatureFlag::LazyShellStartup.override_enabled(true);
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let workspace = mock_workspace(&mut app);

        app.update(|ctx| {
            workspace.update(ctx, |ws, ctx| {
                ws.handle_action(&WorkspaceAction::RestartShell, ctx);
            });
        });

        assert!(
            !app.read(|ctx| workspace.read(ctx, |ws, _| ws.restart_shell_modal_is_open())),
            "an idle pane restarts without a prompt"
        );
    });
}
```

`restart_shell_modal_is_open()` is a test-only accessor you add on `Workspace` for this assertion — a one-line `pub(crate) fn` reading the modal state. Adding a read-only accessor is acceptable; adding a production method that exists only to make the feature testable is not.

Match `mock_workspace` and `handle_action` usage to the existing tests in this file rather than the shapes above, which are illustrative of intent, not verified call syntax.

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo nextest run -p warp --lib -E 'test(restart_shell_does_not_prompt)'`
Expected: compile error — `WorkspaceAction::RestartShell` is not defined.

- [ ] **Step 3: Add the actions and the guard**

Add both variants to `WorkspaceAction` and handle them in `Workspace::handle_action`. Both resolve the focused pane's terminal manager, then:

```rust
// The guard: prompt only when there is work to lose. A running CLI agent is a
// foreground process and takes this path like any other command.
let is_busy = /* read the focused pane's active block via
                 is_active_and_long_running(); see app/src/terminal/input.rs:5016 */;
if is_busy {
    // open the confirmation modal, carrying the target shell so the confirm
    // handler can complete the relaunch
} else {
    manager.update(ctx, |m, ctx| m.relaunch_shell(target, cwd, ctx));
}
```

`cwd` comes from the focused terminal view's `pwd_if_local(ctx)` — host form, which is what Task 3's `relaunch_shell` expects.

- [ ] **Step 4: Add the confirmation modal**

Create `app/src/workspace/restart_shell_confirmation_modal.rs` following `transfer_ownership_confirmation_modal.rs`. The modal names the running command rather than asking a generic "are you sure" — the point is an informed choice. On confirm, it calls `relaunch_shell`; on cancel, nothing happens and the session is untouched.

- [ ] **Step 5: Run the test to verify it passes**

Run: `cargo nextest run -p warp --lib -E 'test(restart_shell_does_not_prompt)'`
Expected: PASS.

Then: `cargo nextest run -p warp --lib --no-fail-fast` — no failures outside the baseline 21.
Then: `cargo check --workspace --exclude command-signatures-v2 --exclude integration` — clean.
Then: `cargo clippy -p warp --lib -- -D warnings` — clean.

- [ ] **Step 6: Commit**

```bash
git add app/src/workspace
git commit -m "feat(workspace): add restart-shell and change-shell commands"
```

---

### Task 5: Resume the agent after the shell comes back

**Files:**
- Modify: `app/src/workspace/view.rs` or wherever Task 4 placed the relaunch call site
- Test: `app/src/pane_group/mod_tests.rs`

**Interfaces:**
- Consumes: `ResumeOptions::ThirdParty(Box<ResumePayload>)` (`app/src/ai/agent_sdk/driver.rs:452`); `set_pending_command_queue(&mut self, commands: Vec<String>, ctx: &mut ViewContext<Self>)` (`app/src/terminal/view.rs:9537`).
- Produces: no new public API.

**The sequencing is already solved — do not rebuild it.** The pending-command queue is a "run this once the shell has bootstrapped" primitive: `set_pending_command_queue` parks commands, and `execute_pending_command` refuses to submit until `block_list().is_bootstrapped()`. Enqueue the agent's resume invocation like any other startup command.

**Capture before teardown.** The resume payload is unreadable once the PTY is gone. Read it in Task 4's action handler, before calling `relaunch_shell`.

- [ ] **Step 1: Write the failing test**

Append to `app/src/pane_group/mod_tests.rs`:

```rust
/// A pane with no agent relaunches without trying to resume one. The common
/// case must not acquire an agent dependency.
#[test]
fn test_relaunch_without_an_agent_enqueues_nothing() {
    let _flag = FeatureFlag::LazyShellStartup.override_enabled(true);
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let pane_group = mock_pane_group(&mut app, MockOptions::default());
        let manager = app
            .read(|ctx| pane_group.read(ctx, |group, ctx| group.terminal_manager(0, ctx)))
            .expect("pane 0 has a terminal manager");

        app.update(|ctx| {
            manager.update(ctx, |m, ctx| m.relaunch_shell(None, None, ctx));
        });

        assert!(
            !app.read(|ctx| pane_group.read(ctx, |group, ctx| group
                .terminal_view(0, ctx)
                .is_some_and(|v| v.read(ctx, |view, _| view
                    .has_pending_command_or_awaiting_completion())))),
            "no agent means nothing queued"
        );
    });
}
```

Match the accessor names to what the file already uses — `has_pending_command_or_awaiting_completion` is at `app/src/terminal/view.rs:16176`; the `terminal_view(0, ctx)` spelling is illustrative and must be checked against this file's existing helpers.

- [ ] **Step 2: Run the test to verify it fails or passes**

Run: `cargo nextest run -p warp --lib -E 'test(relaunch_without_an_agent)'`

This one may pass immediately, since nothing enqueues yet. That is fine and expected: it is a regression guard for Step 3, not a red-first test. Note in your report which it was.

- [ ] **Step 3: Capture and enqueue the resume**

At the call site from Task 4, before `relaunch_shell`:

1. Read the focused pane's agent resume state. If there is none, relaunch and stop.
2. If there is one, build `ResumeOptions::ThirdParty(..)` from it.
3. Relaunch.
4. Enqueue the resume invocation via `set_pending_command_queue`.

**Read `rehydrate_claude_transcript` (`app/src/ai/agent_sdk/driver/harness/claude_code.rs:282`) before writing step 2.** Killing an agent mid-turn truncates its transcript, and resume must mean *resume from the last complete turn*. If that function does not already guarantee this, say so in your report and stop rather than shipping a resume that can replay a partial turn — the spec flags this as unverified, and it is the one case that can corrupt a conversation.

- [ ] **Step 4: Run the tests**

Run: `cargo nextest run -p warp --lib -E 'test(relaunch)'` — all pass.

Then: `cargo nextest run -p warp --lib --no-fail-fast` — no failures outside the baseline 21.
Then: `cargo check --workspace --exclude command-signatures-v2 --exclude integration` — clean.
Then: `cargo clippy -p warp --lib -- -D warnings` — clean.

- [ ] **Step 5: Commit**

```bash
git add app/src
git commit -m "feat(terminal): resume a CLI agent after relaunching its shell"
```

---

## Self-Review

**Spec coverage.** Guard on a running process → Task 4 Step 3. Capture-then-teardown ordering → Task 2 (`end_session`) and Task 5 Step 3 (capture before relaunch). Re-park and start → Task 3. cwd translated to native form → Task 1 (converters) and Task 3 Step 4 (`native_startup_directory`). Auto-resume a CLI agent → Task 5. Two separate commands → Task 4. Shell picker reusing `AvailableShells` → Task 4 Step 3. Confirmation modal following the established pattern → Task 4 Step 4. No default keybinding → stated in Task 4's preamble.

**Error handling coverage.** Teardown failure → Task 3 Step 4, which returns early rather than parking over a live session. Shell fails to start → inherited from `on_shell_determined`'s fallback path, unchanged by this plan. Directory no longer resolves → `native_startup_directory` returns `None`, parking with no startup directory so the shell uses its default. Truncated transcript → Task 5 Step 3, which instructs the implementer to verify and stop rather than guess. No agent running → Task 5 Step 1's test.

**Placeholder scan.** Three test bodies are deliberately not fully specified: Task 2 Step 1, and the accessor spellings in Tasks 3 and 5. These are not "figure it out" hedges — each names the exact file and existing test to mirror, states what must be asserted, and requires the implementer to report what they used. The alternative would be inventing helper signatures I have not verified, which is how the previous plan on this branch produced a test naming two functions that did not exist.

**Type consistency.** `relaunch_shell(target, cwd, ctx)` is spelled identically at its trait declaration (Task 3 Step 3), both forwarding impls (Step 5), and all four call sites in Tasks 3-5. `end_session() -> bool` and `repark_shell(DeferredShellStart)` match between Task 2's definition and Task 3's use. `convert_windows_host_to_wsl_path` and `convert_windows_native_to_msys2_path` match between Task 1's definitions and Task 3's `native_startup_directory`. Both return `String`; both call sites wrap in `PathBuf::from`.

**Known gap this plan does not close.** No test asserts a shell actually *started* after a relaunch — every assertion observes parked state or blocklist identity. A `relaunch_shell` that tore the session down and never respawned would keep these tests green while leaving the pane permanently dead. This is inherited from the lazy-restore design, is recorded in that plan and in this feature's spec, and needs a new observable to close.
