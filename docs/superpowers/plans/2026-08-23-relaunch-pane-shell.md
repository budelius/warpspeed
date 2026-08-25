# Relaunching a Pane's Shell Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A pane can restart its shell in place, or switch to a different shell, keeping its scrollback, its working directory, and its agent conversation.

**Architecture:** A relaunch is guard → capture → tear down → build fresh session resources → start. The pane, its `TerminalView`, and its `blocklist` are kept alive throughout, which is what keeps scrollback and agent history safe. Restart and switch-shell are one code path differing only in the `ShellLaunchData` passed.

This deliberately does **not** reuse `DeferredShellStart`. An earlier version of this plan did, on the assumption that parking and starting were reusable for a second session. They are not: `ShellStartupResources` is one-shot, and ending a session fires the exit path that closes the pane. Both facts are set out in the Self-Review at the end, and Tasks 2 and 3 exist to address them.

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

### Task 2: Survive a deliberate teardown

**Why this exists.** Ending a session fires `ModelEvent::Exit`, and `TerminalView`'s handler for it (`app/src/terminal/view.rs:11868-11916`) does four things that are correct for a shell that died and wrong for one being restarted: it inserts a "shell process terminated" banner, disables the input editor, emits `Event::Exited` — which `app/src/pane_group/pane/terminal_pane.rs:976-989` turns into `close_pane` — and latches `TerminalModel::handled_exit`, which has no reset anywhere (`terminal_model.rs:1089` sets it false at construction, `:1490` sets it true, nothing else) and makes `is_read_only()` (`:1501`) return true forever.

Without this task, the first production call to any restart primitive closes the pane the feature exists to preserve.

`manual_pty_shutdown_requested` (`view.rs:2946`) is **not** the answer despite looking like it: it gates only the telemetry branch at `:11869`, and its own comment says the pane is torn down afterwards.

**Files:**
- Modify: `app/src/terminal/view.rs`, `app/src/terminal/model/terminal_model.rs`
- Test: `app/src/terminal/view_tests.rs` if it exists; otherwise the existing test module in `view.rs`. Check first — do not create a second module.

**Interfaces:**
- Produces:
  - `TerminalView::begin_restart(&mut self)` — marks the next `ModelEvent::Exit` as a deliberate restart.
  - `TerminalView::finish_restart(&mut self)` — clears that mark once the new session is up.
  - `TerminalModel::reset_exit_state(&mut self)` — clears `handled_exit` so the pane is writable again and a later real exit is still handled.

- [ ] **Step 1: Write the failing test**

Assert that with a restart in progress, `ModelEvent::Exit` does **not** emit `Event::Exited` and does not insert the terminated banner, and that after `finish_restart` a subsequent `Exit` behaves normally again.

Read the existing tests that drive a `ModelEvent` into a `TerminalView` and copy their construction exactly. `app/src/ai/agent_sdk/driver/terminal_tests.rs:100` shows one way an `Event::Exited` is driven in a test; find the nearest equivalent that delivers a `ModelEvent::Exit` and mirror it. Report which test you mirrored.

Do not invent a helper. If no test in the crate delivers `ModelEvent::Exit` to a view, say so in your report and assert instead on the flag's effect on the three observable pieces of state the handler touches: banner present, editor interaction state, and `is_read_only()`.

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo nextest run -p warp --lib -E 'test(restart)'`
Expected: compile error — `begin_restart` is not defined.

- [ ] **Step 3: Add the flag and gate the three effects**

Add a `restart_in_progress: bool` field to `TerminalView`, initialised `false` beside `manual_pty_shutdown_requested` (`view.rs:4467`). In the `ModelEvent::Exit` arm, gate on it so that when a restart is in progress:

- the terminated banner is **not** inserted,
- the input editor is **not** disabled,
- `Event::Exited` is **not** emitted.

Leave the telemetry branch as it is — it is already gated by `manual_pty_shutdown_requested` and is about attribution, not teardown.

```rust
    /// Marks the next shell exit as a deliberate restart, so the exit handler
    /// does not tear the pane down.
    ///
    /// `ModelEvent::Exit` cannot distinguish a shell that died from one being
    /// replaced. Without this, a restart inserts a "process terminated" banner,
    /// disables the input, and emits `Event::Exited` -- which closes the pane.
    pub fn begin_restart(&mut self) {
        self.restart_in_progress = true;
    }

    /// Clears the restart mark once the replacement session is up, so a later
    /// real exit is handled normally.
    pub fn finish_restart(&mut self) {
        self.restart_in_progress = false;
    }
```

- [ ] **Step 4: Reset the exit latch**

Add to `TerminalModel`:

```rust
    /// Clears the exit latch so a replaced shell's model is writable again.
    ///
    /// `exit` sets `handled_exit` and nothing resets it, so without this a
    /// restarted pane stays `is_read_only()` forever and the new shell's own
    /// exit is swallowed by the latch.
    pub fn reset_exit_state(&mut self) {
        self.handled_exit = false;
    }
```

Call it from `finish_restart` — read how `TerminalView` reaches its model and follow that path. If the model is behind a lock, hold it for as short a scope as possible.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo nextest run -p warp --lib -E 'test(restart)'` — PASS.

Then: `cargo nextest run -p warp --lib --no-fail-fast` — compare the failing set against `.superpowers/sdd/2026-08-17-lazy-shell-startup/baseline-failures.txt`; no failures outside it.
Then: `cargo check --workspace --exclude command-signatures-v2 --exclude integration` — clean.
Then: `cargo clippy -p warp --lib -- -D warnings` — clean.

- [ ] **Step 6: Commit**

```bash
git add app/src/terminal
git commit -m "feat(terminal): let a pane survive a deliberate shell teardown"
```

---

### Task 3: Make session resources constructible more than once

**Why this exists.** `ShellStartupResources` (`app/src/terminal/local_tty/terminal_manager.rs:170-176`) holds a `mio_channel::Receiver<Message>` and a `ChannelEventListener`. It is constructed at exactly one site — `:510`, inside `create_model_with_manager` — and consumed at `:830`. A second session needs a fresh pair, and there is no way to make one. This task extracts that construction so a restart can call it.

Pure refactor. No behaviour change.

**Files:**
- Modify: `app/src/terminal/local_tty/terminal_manager.rs`

**Interfaces:**
- Produces: a private function returning the freshly-constructed resources plus the matching `event_loop_tx`, callable both from `create_model_with_manager` and from a restart. Name it `build_session_resources`.

- [ ] **Step 1: Read before changing anything**

Read `create_model_with_manager` around `:490-530` and identify every value derived from the channel pair: the `event_loop_tx` stored on the manager, the copy held by `PtyController` (`:457`), and anything `enqueue_init_script` (`:938`) depends on. Write down in your report which of these must be rebuilt for a second session and which are stable across one.

This reading is the task. The extraction is mechanical once it is done; getting it wrong silently cross-wires two sessions.

- [ ] **Step 2: Extract the construction**

Move the construction into a function shaped like:

```rust
/// Builds one session's worth of event-loop plumbing.
///
/// Extracted from `create_model_with_manager` so a restart can build a second
/// set. `ShellStartupResources` is consumed when the PTY starts, so a manager
/// that has already run a shell cannot reuse its first set.
fn build_session_resources(/* the inputs your Step 1 reading identified */)
    -> (ShellStartupResources, mio_channel::Sender<Message>)
```

The parameter list is deliberately left to you: which inputs are needed is exactly what Step 1 determines, and a signature written before that reading is a guess.

Call it from `create_model_with_manager` in place of the inline construction. Nothing else changes.

- [ ] **Step 3: Verify nothing changed**

This step has no new test. The refactor is correct exactly when the existing suite is unmoved.

Run: `cargo nextest run -p warp --lib --no-fail-fast` — the failing set must be identical to the baseline file, with no additions and no removals.
Then: `cargo check --workspace --exclude command-signatures-v2 --exclude integration` — clean.
Then: `cargo clippy -p warp --lib -- -D warnings` — clean.

Report the before and after failing sets explicitly.

- [ ] **Step 4: Commit**

```bash
git add app/src/terminal/local_tty/terminal_manager.rs
git commit -m "refactor(terminal): extract per-session event-loop resource construction"
```

---

### Task 4: `restart_session`

The primitive. Combines Tasks 2 and 3.

**Files:**
- Modify: `app/src/terminal/local_tty/terminal_manager.rs`, `app/src/terminal/terminal_manager.rs` (trait), `app/src/terminal/local_tty/terminal_view_adaptor.rs`
- Test: `app/src/pane_group/mod_tests.rs`

**Interfaces:**
- Consumes: `begin_restart` / `finish_restart` / `reset_exit_state` (Task 2), `build_session_resources` (Task 3).
- Produces: `fn restart_session(&mut self, target: Option<ShellLaunchData>, cwd: Option<PathBuf>, ctx)` on `TerminalManagerTrait`, defaulted to a no-op exactly as `ensure_shell_started` is (`app/src/terminal/terminal_manager.rs:42`), with the body living once in `TerminalManager` and both impls forwarding to it.

**Sequence:** mark the restart on the view, shut the event loop down, build fresh resources, start a new session with `target` (or the current shell when `None`) and `cwd`, then clear the mark and reset the exit latch.

**Do not reuse `DeferredShellStart` for this.** It exists for a pane that has never started, and its resources are one-shot. Build the new session through the same path `create_model_with_manager` uses, with the resources from Task 3.

- [ ] **Step 1: Write the failing test**

Assert that after `restart_session`, the pane is still open and its blocklist is unchanged — same block count, same conversation id. That is the guarantee the whole feature rests on.

**Two traps the previous attempt at this task fell into. Avoid both:**

1. `App::test`'s executor is cooperative. `ctx.spawn`ed work does not advance unless the test awaits, so a test with no `.await` silently never runs the code it claims to test. Use `ctx.await_spawned_future(..)`; the pattern is at `crates/warpui_core/src/core/mod_tests.rs:2166-2187`.
2. A test that passes when the implementation is stubbed out is not a test. Before committing, replace `restart_session`'s body with an empty one and confirm your test fails. Report that you did this and what you observed.

Use `mock_pane_group`, as the lazy-restore tests do — see `test_deferred_pane_with_commands_parks_its_shell`.

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo nextest run -p warp --lib -E 'test(restart_session)'`
Expected: compile error — `restart_session` is not defined.

- [ ] **Step 3: Implement it**

Add the defaulted trait method, the shared body on `TerminalManager`, and the forwarding impls. Follow how `ensure_shell_started` is declared, bodied, and forwarded — the comment at `terminal_manager.rs:600-604` explains why the body is shared rather than duplicated, and the same reasoning applies here.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo nextest run -p warp --lib -E 'test(restart_session)'` — PASS.
Then the stub check from Step 1. Then the full suite, workspace check, and clippy as in Task 2 Step 5.

- [ ] **Step 5: Commit**

```bash
git add app/src/terminal app/src/pane_group/mod_tests.rs
git commit -m "feat(terminal): restart a pane's session in place"
```

---

### Task 5: Relaunch with the right shell and directory

**Files:**
- Modify: `app/src/terminal/local_tty/terminal_manager.rs`
- Test: `app/src/pane_group/mod_tests.rs`

**Interfaces:**
- Consumes: `restart_session` (Task 4); `convert_windows_host_to_wsl_path`, `convert_windows_native_to_msys2_path` (Task 1, in `crates/warp_util/src/path.rs`); `ShellLaunchData::{Executable, WSL, MSYS2}` (`crates/warp_terminal/src/shell/mod.rs:783`).
- Produces: `native_startup_directory(cwd: &Path, shell: &ShellLaunchData) -> Option<PathBuf>`.

**Verified, not assumed:** `startup_directory` for a WSL shell is in **WSL-native** form. `on_shell_determined` (`terminal_manager.rs:713-719`) substitutes `wsl_shell_starter.home_directory()` only when it is `None`, and otherwise passes the value straight through. A host path handed to a WSL shell lands the user in the wrong directory.

- [ ] **Step 1: Write the failing test**

Table-driven across the three shell kinds: a host path converts to the target shell's native form. `Executable` is the identity; WSL yields `/mnt/<drive>/…`; MSYS2 yields the MSYS2 form.

- [ ] **Step 2: Run to verify it fails**

Run: `cargo nextest run -p warp --lib -E 'test(native_startup_directory)'`
Expected: compile error.

- [ ] **Step 3: Implement**

```rust
    /// Converts a host-form directory to the form the target shell expects.
    ///
    /// Returns `None` when conversion fails, which starts the shell with no
    /// startup directory -- it then uses its own default, the documented
    /// fallback for a directory that no longer resolves.
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

`ShellLaunchData` has variants beyond these three; `_ => None` covers them, and the shell then starts in its own default directory. The `msys2_exe_to_root` and `WindowsPath` handling mirrors `ShellLaunchData::maybe_convert_absolute_path` (`crates/warp_terminal/src/shell/mod.rs:811-830`) — read that and mirror its imports.

Wire it into `restart_session` so the directory it passes is already in native form.

- [ ] **Step 4: Verify and commit**

Tests, full suite, workspace check, clippy as before.

```bash
git add app/src/terminal app/src/pane_group/mod_tests.rs
git commit -m "feat(terminal): start a relaunched shell in the translated directory"
```

---

### Task 6: The two commands and the confirmation

**Files:**
- Modify: `app/src/workspace/view.rs`, and the file declaring `WorkspaceAction`
- Create: `app/src/workspace/restart_shell_confirmation_modal.rs`
- Test: `app/src/workspace/view_tests.rs`

**Interfaces:**
- Consumes: `restart_session` (Task 4); `AvailableShells::handle(ctx).get_available_shells()` and `display_name_for_shell` (used at `app/src/workspace/view.rs:6931`); `block_list().active_block().is_active_and_long_running()` (`app/src/terminal/input.rs:5016`).
- Produces: `WorkspaceAction::RestartShell` and `WorkspaceAction::ChangeShell { shell: ShellLaunchData }`.

Neither command gets a default keybinding. The command palette is the entry point.

**Read first:** `app/src/settings_view/transfer_ownership_confirmation_modal.rs` in full — the pattern for the confirmation is a small `Entity` with a `ModalViewState<Modal<T>>` field on the host view, added via `ctx.add_typed_action_view(..)`. Mirror it.

- [ ] **Step 1: Write the failing test**

Assert that restarting a pane with nothing running does **not** prompt. The confirmation exists to protect work in progress; it must not appear when there is none.

Add a test-only `pub(crate) fn` on `Workspace` reading the modal state if the assertion needs one. A read-only accessor is acceptable; a production method existing only to make the feature testable is not.

Match `mock_workspace` and `handle_action` usage to the existing tests in this file.

- [ ] **Step 2: Run to verify it fails**

Run: `cargo nextest run -p warp --lib -E 'test(restart_shell_does_not_prompt)'`
Expected: compile error — `WorkspaceAction::RestartShell` is not defined.

- [ ] **Step 3: Add the actions and the guard**

Both variants resolve the focused pane's terminal manager, then:

```rust
// Prompt only when there is work to lose. A running CLI agent is a foreground
// process and takes this path like any other command.
let is_busy = /* the focused pane's active block via is_active_and_long_running();
                 see app/src/terminal/input.rs:5016 */;
if is_busy {
    // open the confirmation, carrying the target shell so confirm can complete it
} else {
    manager.update(ctx, |m, ctx| m.restart_session(target, cwd, ctx));
}
```

`cwd` comes from the focused terminal view's `pwd_if_local(ctx)` — host form, which is what Task 5's conversion expects.

- [ ] **Step 4: Add the confirmation modal**

Create the modal following the pattern above. It names the running command rather than asking a generic "are you sure". On confirm it calls `restart_session`; on cancel nothing happens and the session is untouched.

- [ ] **Step 5: Verify and commit**

Tests, full suite, workspace check, clippy.

```bash
git add app/src/workspace
git commit -m "feat(workspace): add restart-shell and change-shell commands"
```

---

### Task 7: Fork a Claude transcript at its last complete turn

**Why this replaces the original Task 7.** The first version assumed a restart could capture `ResumeOptions::ThirdParty(ResumePayload)` and hand it back after the shell returned. It cannot. `ResumePayload::Claude` has exactly one producer — `fetch_resume_payload` (`app/src/ai/agent_sdk/driver/harness/claude_code.rs:115-124`) — which takes a server-minted `AIConversationId` and fetches the envelope over HTTP. That is the headless cloud driver's mechanism. A `claude` the user typed into a pane has neither a server conversation id nor a server-stored envelope.

The second assumption was that `rehydrate_claude_transcript` guaranteed resumption from the last complete turn. It does not: `claude_transcript.rs:243-258` sets `envelope.cwd`, writes the envelope, writes a session-index entry, and returns a resume command. No entry is inspected and nothing is dropped. Worse, for a pane-local agent that function is never reached at all — Claude writes its own JSONL incrementally to `~/.claude/projects/<encoded-cwd>/<session-uuid>.jsonl`, so `claude --resume <uuid>` picks up whatever partial turn was flushed when the shell was killed.

This task builds the missing piece.

**Decision, already made — do not revisit.** Trim by **forking**: read the session file, trim a copy to the last complete turn, and write it under a **new** UUID. Never mutate Claude's original file. A bad trim must be recoverable, and the CLI's on-disk format is not ours to own.

**Files:**
- Modify: `app/src/ai/agent_sdk/driver/harness/claude_transcript.rs`
- Test: the existing test module for that file. Find it before adding one; do not create a second module.

**Interfaces:**
- Consumes: `ClaudeTranscriptEnvelope` (`claude_transcript.rs:36-50`) with `entries: Vec<Value>`; `write_envelope(envelope, config_root)` (`:200`); `claude_config_dir()` (`:89`); `encode_cwd`.
- Produces: `pub(crate) fn fork_transcript_at_last_complete_turn(session_id: Uuid, cwd: &Path, config_root: &Path) -> Result<Option<ClaudeLocalContinuation>>` — reads the session's JSONL, trims, writes a new session under a fresh UUID, and returns the resume command for it. `Ok(None)` when there is no session file or nothing resumable survives the trim.

- [ ] **Step 1: Determine what a turn boundary actually is**

`entries` is `Vec<Value>` — untyped JSON. **You cannot write the trim until you know the entry shape.** Read `entries_to_jsonl` and the surrounding parsing in `claude_transcript.rs`, and read any test fixtures in the repo that contain real Claude JSONL entries (`grep -rl '"type":"assistant"' --include=*.jsonl --include=*.rs .` is a reasonable start).

Write down in your report: which field(s) identify an entry's role, what marks the end of an assistant turn, and how an interrupted turn differs from a complete one on disk. If the repo contains no real fixture, say so — and derive the boundary from the writer side (what Warp itself serialises) rather than inventing a schema.

This reading is the task. The trim is mechanical once it is done, and a guessed schema silently drops good turns or keeps bad ones.

- [ ] **Step 2: Write the failing tests**

Table-driven over synthetic entry sequences built from what Step 1 established. At minimum:

- a transcript ending on a complete assistant turn is returned unchanged in content, under a new UUID
- a transcript ending mid-assistant-turn has that trailing partial turn dropped
- a transcript containing only a partial first turn yields `Ok(None)` — nothing resumable
- a session id with no file on disk yields `Ok(None)`, not an error
- the **original file is byte-identical afterwards** — this is the decision above, and it needs a test, not a comment

Use `tempfile::TempDir` as `config_root`. Do not write into the real `~/.claude`.

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo nextest run -p warp --lib -E 'test(fork_transcript)'`
Expected: compile error — the function is not defined.

- [ ] **Step 4: Implement**

Read the session JSONL for `session_id` under `config_root`, parse into `Vec<Value>`, trim to the last complete turn per Step 1, mint a fresh `Uuid`, build a `ClaudeTranscriptEnvelope` with the trimmed entries, and hand it to the existing `write_envelope`. Return `ClaudeLocalContinuation { command: format!("claude --resume {new_id}") }`.

Reuse `write_envelope` rather than writing JSONL yourself — it already handles the projects directory, the cwd encoding, subagents, and todos.

- [ ] **Step 5: Verify and commit**

Run: `cargo nextest run -p warp --lib -E 'test(fork_transcript)'` — all pass.
Then: `cargo nextest run -p warp --lib --no-fail-fast` — failing SET matches `.superpowers/sdd/2026-08-17-lazy-shell-startup/baseline-failures.txt` exactly. Note `test_restart_session_keeps_the_pane_and_rearms_its_exit_handling` is known timing-sensitive; if it fails once and passes in isolation, say so.
Then: `cargo check --workspace --exclude command-signatures-v2 --exclude integration` — clean.
Then: `cargo clippy -p warp --lib -- -D warnings` — clean.

```bash
git add app/src/ai/agent_sdk/driver/harness/claude_transcript.rs
git commit -m "feat(agent): fork a Claude transcript at its last complete turn"
```

---

### Task 8: Resume the agent after the shell comes back

**Files:**
- Modify: wherever Task 6 placed the restart call site in `app/src/workspace/view.rs`
- Test: `app/src/workspace/view_tests.rs`

**Interfaces:**
- Consumes: `fork_transcript_at_last_complete_turn` (Task 7); `set_pending_command_queue(&mut self, commands: Vec<String>, ctx)` (`app/src/terminal/view.rs:9537`); `CLIAgentSession.session_context.session_id`.
- Produces: no new public API.

**Sequencing is already solved — do not rebuild it.** The pending-command queue is a "run this once the shell has bootstrapped" primitive: `set_pending_command_queue` parks commands and `execute_pending_command` refuses to submit until `block_list().is_bootstrapped()`. Enqueue the resume command like any other startup command. Do not detect bootstrap yourself.

**Capture before teardown.** The session id is unreadable once the PTY is gone. Read it in the action handler, before `restart_session` is called.

**Decision, already made — do not revisit.** A bare `claude` with no plugin has `session_id: None` and is unresumable by any mechanism. In that case the pane **restarts normally and says nothing**: no banner, no warning, no extra confirmation. The rendered conversation stays on screen; the user relaunches the agent themselves.

- [ ] **Step 1: Write the failing tests**

Two cases:
- a pane with no agent session restarts and enqueues nothing
- a pane with a tracked agent session enqueues exactly one `claude --resume <new-uuid>` command, where the uuid is the forked one, not the original

The second is the one that must discriminate: removing the enqueue has to make it fail. If the enqueue genuinely cannot be observed in the harness, say so explicitly with what you tried, rather than leaving an assertion that passes against a no-op. Earlier tasks added `#[cfg(test)]` observation hooks where needed — check whether one already gives you what you need before adding another.

- [ ] **Step 2: Run to verify they fail**

Run: `cargo nextest run -p warp --lib -E 'test(agent_resume) or test(restart_shell)'`

The no-agent case may pass immediately, since nothing enqueues yet. That is expected — it is a regression guard, not a red-first test. Say which it was.

- [ ] **Step 3: Implement**

In the restart action handler, before calling `restart_session`:

1. Read the focused pane's `CLIAgentSession.session_context.session_id`. If `None`, restart and stop — no enqueue, no message.
2. If `Some(id)`, call `fork_transcript_at_last_complete_turn`. If it returns `Ok(None)`, restart and stop.
3. Restart.
4. Enqueue the returned command via `set_pending_command_queue`.

A failure inside the fork must not fail the restart. Log it and restart without resuming — the shell coming back matters more than the agent coming back.

- [ ] **Step 4: Verify and commit**

Same four gates as Task 7.

```bash
git add app/src/workspace app/src/terminal
git commit -m "feat(workspace): resume a CLI agent after restarting its shell"
```

---

## Self-Review

**What changed from earlier versions of this plan, and why.**

Tasks 2 onward were rewritten once already, after the original Task 2's review disproved the approach: `ShellStartupResources` is one-shot (`terminal_manager.rs:170-176`, built at `:510`, consumed at `:830`), so a re-parked `DeferredShellStart` can never be handed valid resources; and ending a session fires `ModelEvent::Exit`, whose handler emits `Event::Exited` and closes the pane. Tasks 2 and 3 exist to address those.

Task 7 was then rewritten a second time, after its own investigation found the resume mechanism it named does not exist for pane-local agents, and the last-complete-turn guarantee it relied on was never there. That investigation is summarised in Task 7's preamble. The replacement forks the transcript rather than mutating Claude's file, and does nothing when a session is untracked.

**Spec coverage.** Guard on a running process → Task 6. Capture before teardown → Task 8 Step 3. Restart in place → Task 4. cwd translated to native form → Tasks 1 and 5. Auto-resume a CLI agent → Tasks 7 and 8. Two separate commands → Task 6. Shell picker reusing `AvailableShells` → Task 6. Confirmation modal → Task 6. No default keybinding → Task 6.

**Error handling.** The spec's "teardown failure aborts the relaunch" requirement was dropped deliberately: `shutdown_event_loop` takes the join handle before joining and never restores it, so a boolean success signal is unconditionally true and the guard would be vacuous. Recorded as a follow-up rather than faked. Directory no longer resolves → `native_startup_directory` returns `None` and the shell uses its default. Transcript fork fails → Task 8 Step 3 restarts without resuming. Agent session untracked → Task 8 restarts silently, by decision.

**Placeholder scan.** Task 7 Step 1 deliberately does not specify the entry schema, because determining it from real data is the task — a guessed schema silently drops good turns or keeps bad ones, and guessing an unverified interface is what forced both rewrites of this plan.

**Type consistency.** `fork_transcript_at_last_complete_turn(session_id, cwd, config_root) -> Result<Option<ClaudeLocalContinuation>>` is spelled identically at its definition (Task 7) and its call site (Task 8 Step 3). `ClaudeLocalContinuation { command: String }` is the existing type returned by `rehydrate_claude_transcript`. `set_pending_command_queue` takes `Vec<String>`, which is what `command` provides.

**Known gaps.** No test asserts the replacement shell reaches a working prompt — that needs a real PTY. Codex has the same transcript problem as Claude and is **not** covered by these tasks; only Claude sessions resume. And the fork leaves a duplicate transcript per restart, which nothing prunes.
