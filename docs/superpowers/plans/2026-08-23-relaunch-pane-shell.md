# Relaunching a Pane's Shell Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A pane can restart its shell in place, or switch to a different shell, keeping its scrollback, its working directory, and its agent conversation.

**Status:** All eight tasks shipped, across `afcf7ea3`, `63f43d47`, `015fb98e`, `0c75afd1`, `eeb68f5a`, `cde0a1df`, `21273eee`, `423d523d`, `5776a675`, `a6f35719`, `27feec5d`, `6c8a4cde`. Where implementation diverged from a task as written, the task carries a **Shipped as** note; the Self-Review at the end summarises the divergences.

**Architecture:** A relaunch is guard → capture → tear down → build fresh session resources → start. The pane, its `TerminalView`, and its `blocklist` are kept alive throughout, which is what keeps scrollback and agent history safe. Restart and switch-shell are one code path differing only in the `ShellLaunchData` passed.

Three parts of the shipped design were not in this plan and were found while executing it: the **restart barrier** that tells the view when it is safe to stop suppressing the teardown (Task 2), the **generation guard** that makes an in-flight shell start inert when a restart supersedes it (Task 4), and the **relocation of the cwd conversion** out of `restart_session` into `on_shell_determined` (Task 5). The spec describes each in full.

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
- Test: `crates/warp_util/src/path_tests.rs` (wired in at the bottom of `path.rs` via `#[path = "path_tests.rs"] mod tests;`)

**Interfaces:**
- Consumes: `parse_wsl_unc_path(path: &Path) -> Option<WslUncPath>`, `WslUncPath { distro: String, linux_path: String }`, `WSLPathConversionError` — all in `crates/warp_util/src/path.rs`.
- Produces:
  - `pub fn convert_windows_host_to_wsl_path(host_path: &Path, distro_name: &str) -> Result<String, WSLPathConversionError>`
  - `pub fn convert_windows_native_to_msys2_path(host_path: &Path, msys2_root: &Path) -> Option<String>`

  Both return the shell-native path as a `String`, not a `PathBuf`: a WSL path is not a valid `std::path::Path` on Windows, and round-tripping it through `PathBuf` would mangle separators.

**Read first:** `convert_wsl_to_windows_host_path`. It has exactly two branches — `/mnt/<drive>/…` → `<DRIVE>:\…`, and everything else → `\\WSL$\<distro>\…`. Your function inverts both. The second inverse already exists as `parse_wsl_unc_path`; you are writing the first and a dispatcher.

- [x] **Step 1: Write the failing tests**

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

**On the round-trip test:** `convert_wsl_to_windows_host_path` calls `std::fs::read_link` on its drive-letter branch, so a path that is a symlink on this machine will not round-trip. `C:\projects\warp` is chosen because it is the repo root and a real directory. If this test proves flaky on a machine where that path is a symlink, replace the literal with a `tempfile::TempDir` path rather than deleting the test.

- [x] **Step 2: Run the tests to verify they fail**

Run: `cargo nextest run -p warp_util -E 'test(wsl)'`
Expected: compile error — `convert_windows_host_to_wsl_path` is not defined.

- [x] **Step 3: Implement the WSL converter**

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

`Utf8WindowsPath`, `Utf8WindowsComponent`, and `Utf8WindowsPrefix` are already imported in this file — `parse_wsl_unc_path` uses all three. `WSLPathConversionError` has no variant meaning "not a drive path"; reusing `NonUnixPath` is the closest fit. If you would rather add a variant, add it — it is a `thiserror` enum and adding a variant is cheap — but then update every match on it that the compiler reports.

- [x] **Step 4: Run the tests to verify they pass**

Run: `cargo nextest run -p warp_util -E 'test(wsl)'`
Expected: all six PASS.

- [x] **Step 5: Write the failing MSYS2 test**

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

- [x] **Step 6: Run to verify it fails, then implement the MSYS2 converter**

Run: `cargo nextest run -p warp_util -E 'test(msys2)'` — expect a compile error.

**Read first:** `convert_msys2_to_windows_native_path` and `msys2_exe_to_root`. Mirror the root-handling exactly: whatever the forward function treats as the root prefix, this must strip, and whatever drive spelling it emits, this must produce. Write the implementation to satisfy the three tests above, matching the forward function's conventions rather than inventing new ones.

Note the ordering that matters: a path under the MSYS2 root must be tested **before** the drive-letter branch, or `C:\msys64\home\me` becomes `/c/msys64/home/me` instead of `/home/me`.

- [x] **Step 7: Run all converter tests**

Run: `cargo nextest run -p warp_util`
Expected: all pass, no failures.

Then: `cargo check --workspace --exclude command-signatures-v2 --exclude integration` — clean.
Then: `cargo clippy -p warp_util -- -D warnings` — clean.

- [x] **Step 8: Commit**

```bash
git add crates/warp_util/src/path.rs crates/warp_util/src/path_tests.rs
git commit -m "feat(path): convert Windows host paths to WSL and MSYS2 form"
```

---

### Task 2: Survive a deliberate teardown

**Why this exists.** Ending a session fires `ModelEvent::Exit`, and `TerminalView`'s handler for it (the `ModelEvent::Exit` arm of `TerminalView::handle_terminal_event`, `app/src/terminal/view.rs`) does four things that are correct for a shell that died and wrong for one being restarted: it inserts a "shell process terminated" banner, disables the input editor, emits `Event::Exited` — which the `Event::Exited` arm in `app/src/pane_group/pane/terminal_pane.rs` turns into `close_pane` — and latches `TerminalModel::handled_exit`, which at the time this was written had no reset anywhere — `TerminalModel::new` set it false, `TerminalModel::exit` set it true, and nothing else touched it — and makes `TerminalModel::is_read_only()` return true forever.

Without this task, the first production call to any restart primitive closes the pane the feature exists to preserve.

`manual_pty_shutdown_requested` (set by `TerminalView::shutdown_pty`) is **not** the answer despite looking like it: it gates only the telemetry branch of the same arm, and its own comment says the pane is torn down afterwards.

**Files:**
- Modify: `app/src/terminal/view.rs`, `app/src/terminal/model/terminal_model.rs`
- Test: `app/src/terminal/view_tests.rs` if it exists; otherwise the existing test module in `view.rs`. Check first — do not create a second module.

**Interfaces:**
- Produces:
  - `TerminalView::begin_restart(&mut self)` — marks the next `ModelEvent::Exit` as a deliberate restart.
  - `TerminalView::finish_restart(&mut self)` — clears that mark once the new session is up.
  - `TerminalModel::reset_exit_state(&mut self)` — clears `handled_exit` so the pane is writable again and a later real exit is still handled.

- [x] **Step 1: Write the failing test**

Assert that with a restart in progress, `ModelEvent::Exit` does **not** emit `Event::Exited` and does not insert the terminated banner, and that after `finish_restart` a subsequent `Exit` behaves normally again.

Read the existing tests that drive a `ModelEvent` into a `TerminalView` and copy their construction exactly. `app/src/ai/agent_sdk/driver/terminal_tests.rs` shows one way an `Event::Exited` is driven in a test; find the nearest equivalent that delivers a `ModelEvent::Exit` and mirror it. Report which test you mirrored.

Do not invent a helper. If no test in the crate delivers `ModelEvent::Exit` to a view, say so in your report and assert instead on the flag's effect on the three observable pieces of state the handler touches: banner present, editor interaction state, and `is_read_only()`.

- [x] **Step 2: Run the test to verify it fails**

Run: `cargo nextest run -p warp --lib -E 'test(restart)'`
Expected: compile error — `begin_restart` is not defined.

- [x] **Step 3: Add the flag and gate the three effects**

Add a `restart_in_progress: bool` field to `TerminalView`, initialised `false` beside `manual_pty_shutdown_requested` in `TerminalView`'s constructor. In the `ModelEvent::Exit` arm, gate on it so that when a restart is in progress:

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

- [x] **Step 4: Reset the exit latch**

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

- [x] **Step 5: Run the tests to verify they pass**

Run: `cargo nextest run -p warp --lib -E 'test(restart)'` — PASS.

Then: `cargo nextest run -p warp --lib --no-fail-fast` — compare the failing set against `.superpowers/sdd/2026-08-17-lazy-shell-startup/baseline-failures.txt`; no failures outside it.
Then: `cargo check --workspace --exclude command-signatures-v2 --exclude integration` — clean.
Then: `cargo clippy -p warp --lib -- -D warnings` — clean.

- [x] **Step 6: Commit**

```bash
git add app/src/terminal
git commit -m "feat(terminal): let a pane survive a deliberate shell teardown"
```

**Shipped as** (`afcf7ea3`, then corrected by `63f43d47`). The flag and the three gated effects are as written. What this task did **not** anticipate is *when the flag can be cleared*, and that turned out to be the hard part.

`ModelEvent::Exit` reaches the view asynchronously, so `begin_restart(); spawn the replacement; finish_restart();` is wrong — the replaced shell's own `Exit` can still be queued behind the `finish_restart` call, at which point it is treated as a real exit and closes the pane. The fix is a **restart barrier**: `TerminalModel::exit_for_restart` posts an `Event::Exit { reason: ExitReason::Restarting }` down the same FIFO channel, and the `ModelEvent::Exit` arm calls `finish_restart` only on observing it. `Restarting` is a reason the PTY event loop never produces, so it can only mean "deliberate replacement", and it is classified *before* `restart_in_progress` is consulted so that back-to-back restarts cannot turn a second barrier into an ordinary exit. `restart_session_inner` posts the barrier only after joining the event loop thread, which is what makes observing it a proof rather than a hope. See the spec's *The restart barrier*.

Two further deltas from the task as written: `finish_restart` takes a `ctx` and also re-enables the input editor (`reset_exit_state` restores model writability, but the editor's `InteractionState` is separate state nothing else clears), and the suppression covers a fourth effect the task did not list — failing the pane's in-flight conversation, which the restart exists to preserve.

---

### Task 3: Make session resources constructible more than once

**Why this exists.** `ShellStartupResources` (`app/src/terminal/local_tty/terminal_manager.rs`) holds a `mio_channel::Receiver<Message>` and a `ChannelEventListener`. It is constructed at exactly one site — inside `create_model_with_manager` — and consumed in `on_shell_determined`. A second session needs a fresh pair, and there is no way to make one. This task extracts that construction so a restart can call it.

Pure refactor. No behaviour change.

**Files:**
- Modify: `app/src/terminal/local_tty/terminal_manager.rs`

**Interfaces:**
- Produces: a private function returning the freshly-constructed resources plus the matching `event_loop_tx`, callable both from `create_model_with_manager` and from a restart. Name it `build_session_resources`.

- [x] **Step 1: Read before changing anything**

Read `create_model_with_manager` around `:490-530` and identify every value derived from the channel pair: the `event_loop_tx` stored on the manager, the copy held by `PtyController` (`:457`), and anything `enqueue_init_script` (`:938`) depends on. Write down in your report which of these must be rebuilt for a second session and which are stable across one.

This reading is the task. The extraction is mechanical once it is done; getting it wrong silently cross-wires two sessions.

- [x] **Step 2: Extract the construction**

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

- [x] **Step 3: Verify nothing changed**

This step has no new test. The refactor is correct exactly when the existing suite is unmoved.

Run: `cargo nextest run -p warp --lib --no-fail-fast` — the failing set must be identical to the baseline file, with no additions and no removals.
Then: `cargo check --workspace --exclude command-signatures-v2 --exclude integration` — clean.
Then: `cargo clippy -p warp --lib -- -D warnings` — clean.

Report the before and after failing sets explicitly.

- [x] **Step 4: Commit**

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
- Produces: `fn restart_session(&mut self, target: Option<ShellLaunchData>, cwd: Option<PathBuf>, ctx)` on `TerminalManagerTrait`, defaulted to a no-op exactly as `ensure_shell_started` is (the `TerminalManager` trait in `app/src/terminal/terminal_manager.rs`), with the body living once in `TerminalManager` and both impls forwarding to it.

**Sequence:** mark the restart on the view, shut the event loop down, build fresh resources, start a new session with `target` (or the current shell when `None`) and `cwd`, then clear the mark and reset the exit latch.

> **Shipped as** (`afcf7ea3`, `63f43d47`, `015fb98e`). The last clause of that sequence is wrong and shipping it would close the pane: the mark is cleared by the **restart barrier** coming back round, not at the end of this function. See the Task 2 shipped note.
>
> `restart_session_inner` is eight ordered steps, and four of them are not in the sequence above:
>
> - **Step 3 bumps `session_generation`.** A restart can land while the *previous* session's `ShellStarter::init` is still resolving — seconds, for a WSL shell that shells out to `wsl … printenv SHELL`. `spawn_shell_start` stamps each start with the counter, and its callback discards itself when the manager has moved on, rather than forking a PTY whose `event_loop_handle` would overwrite — and thereby detach — the live one. Aborting the pending future is best effort only: `ModelContext::spawn` honours an abort at the future's next poll, so a start whose future already resolved still delivers its callback. **The counter is what makes it correct.** Step 3 also clears `deferred_shell`, since a parked start would otherwise fork a second shell on next activation. Proven by `test_two_restarts_in_flight_establish_only_one_session` against the test-only `established_session_count` hook — production has no symptom to assert on.
> - **Step 2 calls `stop_event_loop`, not `shutdown_event_loop`.** The latter also closes the PTY reads broadcast channel, which is irreversible and which the replacement publishes to.
> - **Step 4 stops the terminal-attributes poller** (unix), which holds the old PTY fd on a repeating timer and would otherwise `tcgetattr` a closed, possibly recycled descriptor.
> - **Step 6 repoints the `PtyController`'s own clone of the event-loop sender.** It still addresses the loop joined in step 2, so every write would fail until repointed.
>
> The signature also gained a return value: `-> bool`, `#[must_use]`, plus a sibling `supports_session_restart()`. A manager that only *was asked* to restart would otherwise get a `claude --resume <uuid>` parked in the pending-command queue of a live, untouched shell. `step 1` deliberately avoids `TerminalView::shutdown_pty` for the reason the Task 2 note gives about `manual_pty_shutdown_requested`.

**Do not reuse `DeferredShellStart` for this.** It exists for a pane that has never started, and its resources are one-shot. Build the new session through the same path `create_model_with_manager` uses, with the resources from Task 3.

- [x] **Step 1: Write the failing test**

Assert that after `restart_session`, the pane is still open and its blocklist is unchanged — same block count, same conversation id. That is the guarantee the whole feature rests on.

**Two traps the previous attempt at this task fell into. Avoid both:**

1. `App::test`'s executor is cooperative. `ctx.spawn`ed work does not advance unless the test awaits, so a test with no `.await` silently never runs the code it claims to test. Use `ctx.await_spawned_future(..)`; the pattern is in `test_await_spawned_future` (`crates/warpui_core/src/core/mod_tests.rs`).
2. A test that passes when the implementation is stubbed out is not a test. Before committing, replace `restart_session`'s body with an empty one and confirm your test fails. Report that you did this and what you observed.

Use `mock_pane_group`, as the lazy-restore tests do — see `test_deferred_pane_with_commands_parks_its_shell`.

- [x] **Step 2: Run the test to verify it fails**

Run: `cargo nextest run -p warp --lib -E 'test(restart_session)'`
Expected: compile error — `restart_session` is not defined.

- [x] **Step 3: Implement it**

Add the defaulted trait method, the shared body on `TerminalManager`, and the forwarding impls. Follow how `ensure_shell_started` is declared, bodied, and forwarded — the doc comment on `TerminalManager::start_deferred_shell` explains why the body is shared rather than duplicated, and the same reasoning applies here.

- [x] **Step 4: Run the tests to verify they pass**

Run: `cargo nextest run -p warp --lib -E 'test(restart_session)'` — PASS.
Then the stub check from Step 1. Then the full suite, workspace check, and clippy as in Task 2 Step 5.

- [x] **Step 5: Commit**

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
- Consumes: `restart_session` (Task 4); `convert_windows_host_to_wsl_path`, `convert_windows_native_to_msys2_path` (Task 1, in `crates/warp_util/src/path.rs`); `ShellLaunchData::{Executable, WSL, MSYS2}` (`crates/warp_terminal/src/shell/mod.rs`).
- Produces: `native_startup_directory(cwd: &Path, shell: &ShellLaunchData) -> Option<PathBuf>`.

**Verified, not assumed:** `startup_directory` for a WSL shell is in **WSL-native** form. `on_shell_determined` substitutes `wsl_shell_starter.home_directory()` only when it is `None`, and otherwise passes the value straight through. A host path handed to a WSL shell lands the user in the wrong directory.

- [x] **Step 1: Write the failing test**

Table-driven across the three shell kinds: a host path converts to the target shell's native form. `Executable` is the identity; WSL yields `/mnt/<drive>/…`; MSYS2 yields the MSYS2 form.

- [x] **Step 2: Run to verify it fails**

Run: `cargo nextest run -p warp --lib -E 'test(native_startup_directory)'`
Expected: compile error.

- [x] **Step 3: Implement**

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

`ShellLaunchData` has variants beyond these three; `_ => None` covers them, and the shell then starts in its own default directory. The `msys2_exe_to_root` and `WindowsPath` handling mirrors `ShellLaunchData::maybe_convert_absolute_path` (`crates/warp_terminal/src/shell/mod.rs`) — read that and mirror its imports.

Wire it into `restart_session` so the directory it passes is already in native form.

> **Shipped as** (`0c75afd1`, then corrected by `eeb68f5a`). That last line is wrong, and `0c75afd1` shipped it before `eeb68f5a` moved it.
>
> `restart_session_inner` does **not** know which shell is starting. Step 7 resolves a `ShellStarter`, but `Config::SystemDefault` — the out-of-the-box setting for anyone who has not customized their shell — still falls through `compute_fallback_shell()` inside `ShellStarter::init`'s async resolution before the real shell is known. Converting against the setting lands the user in the wrong directory whenever the two differ.
>
> So `cwd` stays in **host** form through `restart_session_inner` and into `spawn_shell_start`, and `on_shell_determined` calls `native_startup_directory` once it has computed the real `ShellLaunchData` from the resolved `ShellStarter`. `test_restart_with_system_default_shell_still_lands_in_the_requested_directory` is the regression guard, reading the test-only `TerminalManager::last_resolved_startup_directory` hook — the value is otherwise unobservable once it crosses into `PtyOptions`.
>
> Two smaller deltas: `native_startup_directory` shipped as a module-level `pub(crate) fn`, not an inherent method; and the `_ => None` arm shipped as an explicit `ShellLaunchData::DockerSandbox { .. } => None` with a comment, since paths inside a sandbox container are not host paths at all. The MSYS2 arm also goes through `PathBuf::try_from(root)` rather than `Path::new(&root)`, because `WindowsPathBuf` has no `AsRef<OsStr>` impl on this platform.

- [x] **Step 4: Verify and commit**

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
- Consumes: `restart_session` (Task 4); `AvailableShells::handle(ctx).get_available_shells()` and `display_name_for_shell` (used by `Workspace::unified_new_session_menu_items`, `app/src/workspace/view.rs`); `block_list().active_block().is_active_and_long_running()` (`Block::is_active_and_long_running`, `app/src/terminal/model/block.rs`).
- Produces: `WorkspaceAction::RestartShell` and `WorkspaceAction::ChangeShell { shell: ShellLaunchData }`.

Neither command gets a default keybinding. The command palette is the entry point.

**Read first:** `app/src/settings_view/transfer_ownership_confirmation_modal.rs` in full — the pattern for the confirmation is a small `Entity` with a `ModalViewState<Modal<T>>` field on the host view, added via `ctx.add_typed_action_view(..)`. Mirror it.

- [x] **Step 1: Write the failing test**

Assert that restarting a pane with nothing running does **not** prompt. The confirmation exists to protect work in progress; it must not appear when there is none.

Add a test-only `pub(crate) fn` on `Workspace` reading the modal state if the assertion needs one. A read-only accessor is acceptable; a production method existing only to make the feature testable is not.

Match `mock_workspace` and `handle_action` usage to the existing tests in this file.

- [x] **Step 2: Run to verify it fails**

Run: `cargo nextest run -p warp --lib -E 'test(restart_shell_does_not_prompt)'`
Expected: compile error — `WorkspaceAction::RestartShell` is not defined.

- [x] **Step 3: Add the actions and the guard**

Both variants resolve the focused pane's terminal manager, then:

```rust
// Prompt only when there is work to lose. A running CLI agent is a foreground
// process and takes this path like any other command.
let is_busy = /* the focused pane's active block via is_active_and_long_running();
                 see `Block::is_active_and_long_running` */;
if is_busy {
    // open the confirmation, carrying the target shell so confirm can complete it
} else {
    manager.update(ctx, |m, ctx| m.restart_session(target, cwd, ctx));
}
```

`cwd` comes from the focused terminal view's `pwd_if_local(ctx)` — host form, which is what Task 5's conversion expects.

- [x] **Step 4: Add the confirmation modal**

Create the modal following the pattern above. It names the running command rather than asking a generic "are you sure". On confirm it calls `restart_session`; on cancel nothing happens and the session is untouched.

- [x] **Step 5: Verify and commit**

Tests, full suite, workspace check, clippy.

```bash
git add app/src/workspace
git commit -m "feat(workspace): add restart-shell and change-shell commands"
```

**Shipped as** (`cde0a1df`, then `21273eee` and `423d523d`). Three variants rather than two: `RestartShell`, `ToggleChangeShellMenu` (opens/closes the picker) and `ChangeShell { shell }` (dispatched by selecting an entry). Both commands route into `Workspace::start_shell_restart`. Modal file is `app/src/workspace/restart_shell_confirmation_modal.rs`.

**Not gated on `FeatureFlag::ShellSelector`, and not gated on `target_os = "windows"`.** `change_shell_menu_items` is gated only on `#[cfg(feature = "local_tty")]` with a `Vec::new()` counterpart, and both bindings are registered in `app/src/workspace/mod.rs` unconditionally, with no `.with_enabled(..)` predicate. That is the decision, recorded in the spec with its rationale: restarting and switching a pane's shell is useful on every platform, and `ShellSelector` gates the *new-session* shell dropdown, a different surface.

What *does* vary per pane is handled by `supports_session_restart()`. An SSH pane, the shared-session viewer and the mock all answer `false`, and that answer is used twice: `Workspace::keymap_context` publishes `Workspace_PaneCanRestartShell` from it and both bindings carry that as a context predicate, so the palette (which filters candidates through `bindings_for_context`) does not list either command on such a pane; and `start_shell_restart` / `toggle_change_shell_menu` decline regardless, covering a rebind or a stale context. `EnabledPredicate` is a bare `fn() -> bool` and could not have done this; a context predicate can.

Two follow-up fixes: `21273eee` closes the picker after a selection, and `423d523d` routes Confirm, Cancel, Escape and the X button through one `close_restart_shell_confirmation_modal` path, so no dismissal can leave a live `ModelHandle` to the terminal manager in `pending_shell_restart` (`ModalEvent::Close` previously bypassed the clearing that only the explicit Cancel handler did).

Tests shipped in `app/src/workspace/view_tests.rs`: `restart_shell_does_not_prompt`, `restart_shell_reaches_restart_session_when_idle` (the discriminating one — the first only proves the prompt stays closed), and `restart_shell_confirmation_dismiss_clears_pending_restart`.

---

### Task 7: Fork a Claude transcript at its last complete turn

**Why this replaces the original Task 7.** The first version assumed a restart could capture `ResumeOptions::ThirdParty(ResumePayload)` and hand it back after the shell returned. It cannot. `ResumePayload::Claude` has exactly one producer — `fetch_resume_payload` (`app/src/ai/agent_sdk/driver/harness/claude_code.rs`) — which takes a server-minted `AIConversationId` and fetches the envelope over HTTP. That is the headless cloud driver's mechanism. A `claude` the user typed into a pane has neither a server conversation id nor a server-stored envelope.

The second assumption was that `rehydrate_claude_transcript` guaranteed resumption from the last complete turn. It does not: `rehydrate_claude_transcript` sets `envelope.cwd`, writes the envelope, writes a session-index entry, and returns a resume command. No entry is inspected and nothing is dropped. Worse, for a pane-local agent that function is never reached at all — Claude writes its own JSONL incrementally to `~/.claude/projects/<encoded-cwd>/<session-uuid>.jsonl`, so `claude --resume <uuid>` picks up whatever partial turn was flushed when the shell was killed.

This task builds the missing piece.

**Decision, already made — do not revisit.** Trim by **forking**: read the session file, trim a copy to the last complete turn, and write it under a **new** UUID. Never mutate Claude's original file. A bad trim must be recoverable, and the CLI's on-disk format is not ours to own.

**Files:**
- Modify: `app/src/ai/agent_sdk/driver/harness/claude_transcript.rs`
- Test: the existing test module for that file. Find it before adding one; do not create a second module.

**Interfaces:**
- Consumes: `ClaudeTranscriptEnvelope` with `entries: Vec<Value>`; `write_envelope(envelope, config_root)`; `claude_config_dir()`; `encode_cwd` — all in `app/src/ai/agent_sdk/driver/harness/claude_transcript.rs`.
- Produces: `pub(crate) fn fork_transcript_at_last_complete_turn(session_id: Uuid, cwd: &Path, config_root: &Path) -> Result<Option<ClaudeLocalContinuation>>` — reads the session's JSONL, trims, writes a new session under a fresh UUID, and returns the resume command for it. `Ok(None)` when there is no session file or nothing resumable survives the trim.

- [x] **Step 1: Determine what a turn boundary actually is**

`entries` is `Vec<Value>` — untyped JSON. **You cannot write the trim until you know the entry shape.** Read `entries_to_jsonl` and the surrounding parsing in `claude_transcript.rs`, and read any test fixtures in the repo that contain real Claude JSONL entries (`grep -rl '"type":"assistant"' --include=*.jsonl --include=*.rs .` is a reasonable start).

Write down in your report: which field(s) identify an entry's role, what marks the end of an assistant turn, and how an interrupted turn differs from a complete one on disk. If the repo contains no real fixture, say so — and derive the boundary from the writer side (what Warp itself serialises) rather than inventing a schema.

This reading is the task. The trim is mechanical once it is done, and a guessed schema silently drops good turns or keeps bad ones.

- [x] **Step 2: Write the failing tests**

Table-driven over synthetic entry sequences built from what Step 1 established. At minimum:

- a transcript ending on a complete assistant turn is returned unchanged in content, under a new UUID
- a transcript ending mid-assistant-turn has that trailing partial turn dropped
- a transcript containing only a partial first turn yields `Ok(None)` — nothing resumable
- a session id with no file on disk yields `Ok(None)`, not an error
- the **original file is byte-identical afterwards** — this is the decision above, and it needs a test, not a comment

Use `tempfile::TempDir` as `config_root`. Do not write into the real `~/.claude`.

- [x] **Step 3: Run the tests to verify they fail**

Run: `cargo nextest run -p warp --lib -E 'test(fork_transcript)'`
Expected: compile error — the function is not defined.

- [x] **Step 4: Implement**

Read the session JSONL for `session_id` under `config_root`, parse into `Vec<Value>`, trim to the last complete turn per Step 1, mint a fresh `Uuid`, build a `ClaudeTranscriptEnvelope` with the trimmed entries, and hand it to the existing `write_envelope`. Return `ClaudeLocalContinuation { command: format!("claude --resume {new_id}") }`.

Reuse `write_envelope` rather than writing JSONL yourself — it already handles the projects directory, the cwd encoding, subagents, and todos.

- [x] **Step 5: Verify and commit**

Run: `cargo nextest run -p warp --lib -E 'test(fork_transcript)'` — all pass.
Then: `cargo nextest run -p warp --lib --no-fail-fast` — failing SET matches `.superpowers/sdd/2026-08-17-lazy-shell-startup/baseline-failures.txt` exactly. Note `test_restart_session_keeps_the_pane_and_rearms_its_exit_handling` is known timing-sensitive; if it fails once and passes in isolation, say so.
Then: `cargo check --workspace --exclude command-signatures-v2 --exclude integration` — clean.
Then: `cargo clippy -p warp --lib -- -D warnings` — clean.

```bash
git add app/src/ai/agent_sdk/driver/harness/claude_transcript.rs
git commit -m "feat(agent): fork a Claude transcript at its last complete turn"
```

**Shipped as** (`a6f35719`, then `27feec5d`). Signature and semantics as specified.

Step 1's question — what a turn boundary is — resolved as follows, and it is worth recording because the repo contains **no real Claude Code JSONL fixture** to check against. The boundary is derived from what `write_envelope`/`read_jsonl` already treat as ground truth (one JSON object per line, opaque beyond `"type"`) plus the publicly documented Claude Code transcript format: a `"type": "assistant"` entry carries a nested `"message"` mirroring the Anthropic Messages API, including `message.stop_reason`. `trim_to_last_complete_turn` keeps everything up to and including the last assistant entry whose `stop_reason` is in an **allowlist** — `end_turn`, `max_tokens`, `stop_sequence`, `refusal` — and returns `None` if there is none.

`27feec5d` made that an allowlist rather than a blacklist, deliberately: `tool_use` (queued tool calls), `pause_turn` (explicitly "not done"), absent/null (still streaming) and any future value all mean the turn continues. Under-trimming resumes a partial turn, which is the corruption this task exists to prevent; over-trimming merely loses recoverable scrollback. When in doubt, trim more.

`27feec5d` also added a `sessions-index.json` entry for the new uuid, mirroring `rehydrate_claude_transcript` — without it `claude --resume` can fail with "No conversation found" even though the jsonl was written correctly. Best-effort: logged rather than failing the fork.

Tests in `app/src/ai/agent_sdk/driver/harness/claude_transcript_tests.rs`, including `fork_transcript_never_mutates_the_original_file` — the "never mutate Claude's file" decision as an assertion, not a comment.

---

### Task 8: Resume the agent after the shell comes back

**Files:**
- Modify: wherever Task 6 placed the restart call site in `app/src/workspace/view.rs`
- Test: `app/src/workspace/view_tests.rs`

**Interfaces:**
- Consumes: `fork_transcript_at_last_complete_turn` (Task 7); `TerminalView::set_pending_command_queue(&mut self, commands: Vec<String>, ctx)` (`app/src/terminal/view.rs`); `CLIAgentSession.session_context.session_id`.
- Produces: no new public API.

**Sequencing is already solved — do not rebuild it.** The pending-command queue is a "run this once the shell has bootstrapped" primitive: `set_pending_command_queue` parks commands and `execute_pending_command` refuses to submit until `block_list().is_bootstrapped()`. Enqueue the resume command like any other startup command. Do not detect bootstrap yourself.

**Capture before teardown.** The session id is unreadable once the PTY is gone. Read it in the action handler, before `restart_session` is called.

**Decision, already made — do not revisit.** A bare `claude` with no plugin has `session_id: None` and is unresumable by any mechanism. In that case the pane **restarts normally and says nothing**: no banner, no warning, no extra confirmation. The rendered conversation stays on screen; the user relaunches the agent themselves.

- [x] **Step 1: Write the failing tests**

Two cases:
- a pane with no agent session restarts and enqueues nothing
- a pane with a tracked agent session enqueues exactly one `claude --resume <new-uuid>` command, where the uuid is the forked one, not the original

The second is the one that must discriminate: removing the enqueue has to make it fail. If the enqueue genuinely cannot be observed in the harness, say so explicitly with what you tried, rather than leaving an assertion that passes against a no-op. Earlier tasks added `#[cfg(test)]` observation hooks where needed — check whether one already gives you what you need before adding another.

- [x] **Step 2: Run to verify they fail**

Run: `cargo nextest run -p warp --lib -E 'test(agent_resume) or test(restart_shell)'`

The no-agent case may pass immediately, since nothing enqueues yet. That is expected — it is a regression guard, not a red-first test. Say which it was.

- [x] **Step 3: Implement**

In the restart action handler, before calling `restart_session`:

1. Read the focused pane's `CLIAgentSession.session_context.session_id`. If `None`, restart and stop — no enqueue, no message.
2. If `Some(id)`, call `fork_transcript_at_last_complete_turn`. If it returns `Ok(None)`, restart and stop.
3. Restart.
4. Enqueue the returned command via `set_pending_command_queue`.

A failure inside the fork must not fail the restart. Log it and restart without resuming — the shell coming back matters more than the agent coming back.

- [x] **Step 4: Verify and commit**

Same four gates as Task 7.

```bash
git add app/src/workspace app/src/terminal
git commit -m "feat(workspace): resume a CLI agent after restarting its shell"
```

**Shipped as** (`6c8a4cde`). The four numbered steps are as written, in `Workspace::restart_shell_and_resume_agent` and `Workspace::resolve_agent_resume_command`, with three additions:

- **Claude only, explicitly.** `resolve_agent_resume_command` returns `None` immediately when `session.agent != crate::terminal::CLIAgent::Claude`. Codex and Gemini are not supported by this path and nothing about it is agent-generic.
- **The restart's return value is checked.** If `restart_session` returns `false`, nothing was torn down, the tracked session is still the live one, and parking the resume command would fire `claude --resume <uuid>` into an untouched shell on its next `BlockCompleted`. Log and return.
- **`clear_tracked_agent_session` after a successful restart**, and deliberately after `resolve_agent_resume_command` (the session id lives nowhere else). Dropping it stops the agent UI showing a live session for a dead process, and — the real reason — stops a *second* restart, before the resumed agent's `SessionStart` hook re-registers, from reading the pre-restart id again and forking the pre-restart transcript, silently discarding everything the resumed session added.

There is also a `#[cfg(target_family = "wasm")]` counterpart returning `None`.

Both tests shipped and both discriminate: `restart_shell_without_agent_session_enqueues_nothing` and `restart_shell_resumes_tracked_claude_session_via_forked_transcript` (asserting the uuid is the *forked* one, not the original).

---

## Self-Review

**What changed from earlier versions of this plan, and why.**

Tasks 2 onward were rewritten once already, after the original Task 2's review disproved the approach: `ShellStartupResources` is one-shot (built in `create_model_with_manager`, consumed in `on_shell_determined`), so a re-parked `DeferredShellStart` can never be handed valid resources; and ending a session fires `ModelEvent::Exit`, whose handler emits `Event::Exited` and closes the pane. Tasks 2 and 3 exist to address those.

Task 7 was then rewritten a second time, after its own investigation found the resume mechanism it named does not exist for pane-local agents, and the last-complete-turn guarantee it relied on was never there. That investigation is summarised in Task 7's preamble. The replacement forks the transcript rather than mutating Claude's file, and does nothing when a session is untracked.

**What changed during execution.** Each task carries a **Shipped as** note where the code diverged from it. Three of those are design, not detail, and the spec describes each in full:

- **The restart barrier** (Task 2, `afcf7ea3` → `63f43d47`). Task 4's sequence said to clear the restart mark after starting the replacement. Doing so closes the pane: the replaced shell's `Exit` can still be in flight. `TerminalModel::exit_for_restart` posts a marker down the same FIFO channel, joined-thread-first, and the exit handler clears the mark only on seeing it.
- **The generation guard** (Task 4, `63f43d47` → `015fb98e`). Neither this plan nor the spec anticipated a restart racing an in-flight `ShellStarter::init`. `session_generation` makes the superseded start discard itself; aborting the pending future is best effort only.
- **The cwd relocation** (Task 5, `0c75afd1` → `eeb68f5a`). Task 5's last line, "wire it into `restart_session` so the directory it passes is already in native form", is wrong and shipped once before being moved. `restart_session_inner` cannot know which shell will launch — `Config::SystemDefault` still resolves through `compute_fallback_shell()` — so `cwd` stays in host form and `on_shell_determined` converts.

All three were found by implementing, not by reviewing. That is two of three found by a *test* the plan asked for (`015fb98e`, `eeb68f5a`) and one by a code review of the first attempt.

**Spec coverage.** Guard on a running process → Task 6. Capture before teardown → Task 8 Step 3. Restart in place → Task 4. cwd translated to native form → Tasks 1 and 5. Auto-resume a CLI agent → Tasks 7 and 8. Two separate commands → Task 6. Shell picker reusing `AvailableShells` → Task 6. Confirmation modal → Task 6. No default keybinding → Task 6.

**Error handling.** The spec's "teardown failure aborts the relaunch" requirement was dropped deliberately: `stop_event_loop` (which a restart calls; `shutdown_event_loop` also closes the irreversible PTY reads broadcast channel the replacement publishes to) takes the join handle before joining and never restores it, so a boolean success signal is unconditionally true and the guard would be vacuous. Recorded as a follow-up rather than faked.

Directory no longer resolves → `native_startup_directory` returns `None` and the shell starts in its own default directory, **silently**: no banner, no warning. An earlier draft of the spec said the pane reports the fallback; it does not, and there is no hook for it. The WSL `home_directory()` substitution in `on_shell_determined` is a better default, not a notification. Transcript fork fails → Task 8 Step 3 restarts without resuming. Agent session untracked, or the agent is not Claude → Task 8 restarts silently, by decision. Every failure on the agent path is logged at most and otherwise ignored: getting the shell back matters more than getting the agent back.

**Placeholder scan.** Task 7 Step 1 deliberately does not specify the entry schema, because determining it from real data is the task — a guessed schema silently drops good turns or keeps bad ones, and guessing an unverified interface is what forced both rewrites of this plan. In the event there was no real data: the repo has no Claude Code JSONL fixture, so the boundary was derived from the writer side plus the documented Messages API shape, and the allowlist is the mitigation. See Task 7's shipped note.

**Type consistency.** `fork_transcript_at_last_complete_turn(session_id, cwd, config_root) -> Result<Option<ClaudeLocalContinuation>>` is spelled identically at its definition (Task 7) and its call site (Task 8 Step 3). `ClaudeLocalContinuation { command: String }` is the existing type returned by `rehydrate_claude_transcript`. `set_pending_command_queue` takes `Vec<String>`, which is what `command` provides.

**Known gaps.** No test asserts the replacement shell reaches a working prompt — that needs a real PTY. (`test_restart_session_keeps_the_pane_and_rearms_its_exit_handling` does now wait on the barrier coming back round *with a live replacement event loop*, which closes the weaker version of this gap: a relaunch that tore down and never respawned no longer passes.) Codex has the same transcript problem as Claude and is **not** covered by these tasks; only Claude sessions resume, and `resolve_agent_resume_command` returns `None` for every other agent. The fork leaves a duplicate transcript per restart, which nothing prunes. The Claude JSONL entry schema is unverified against a real fixture. And a directory that no longer resolves produces no user-visible signal at all.
