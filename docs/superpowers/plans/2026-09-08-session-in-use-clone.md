# Busy Session Modal and Clone Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** When a session config's lease is held by another Warp process, show a dialog that names the holder and offers to clone the session or open its template, instead of returning silently.

**Architecture:** `session_configs.rs` starts recording who holds a lease and returns a typed claim result. `session_store.rs` gains checkpoint cloning, since it owns the checkpoint format and the catalog. A new `launch_configs/clone_session.rs` picks a free identity, copies the TOML with `toml_edit`, and drives the checkpoint copy. A `SessionInUseDialog` view, hosted by `Workspace` exactly like `RemoveTabConfigConfirmationDialog`, turns the failed claim into a choice. Finally the launch config palette keys its map by config source instead of name, so a session config no longer shadows a same-named YAML.

**Tech Stack:** Rust 2024, `anyhow`, `fs4` file locks, `toml_edit` (format-preserving TOML), `serde_json`, `uuid`, `chrono`, `tempfile`, warpui views. Tests run under `cargo nextest`.

**Spec:** `docs/superpowers/specs/2026-09-08-session-in-use-clone-design.md`

## Global Constraints

- Worktree `T:/projects/dev-ops/warp-session-in-use`, branch `feature/session-in-use-clone`, based on `dev`. Do not switch the branch of any other worktree.
- Package name is `warp`. Run tests with `cargo nextest run -p warp <filter>`.
- Before the final commit of each task run `./script/format` and `cargo clippy -p warp --all-targets --tests -- -D warnings`.
- Filesystem code in `app/src` is gated: anything touching `std::fs` for session state sits behind `#[cfg(feature = "local_fs")]` with a non-`local_fs` counterpart, following the existing functions in `session_store.rs`. `local_fs` is not in `[features] default`; `app/build.rs:302` turns it on for every non-wasm target, so on Windows the gated code is the code that compiles and the code the tests exercise.
- Comments are "why" only. Never restate what the code does. See `AGENTS.md`.
- A context parameter is named `ctx` and goes last, except when a closure is last.
- Use inline format arguments: `format!("{name}")`, never `format!("{}", name)`.
- Every commit message ends with:
  ```
  Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01DxxXYaK6sWmHNZg85bg8mP
  ```
- Never `git add -A`. Other sessions edit sibling worktrees; stage the exact paths listed in each task.

## File Structure

| File | Responsibility |
|---|---|
| `app/src/session_configs.rs` (modify) | Lease claiming, owner record, session TOML parsing. Gains `SessionOwner`, `SessionClaim`, and a directory-injectable claim core. |
| `app/src/session_configs_tests.rs` (modify) | Claim and owner round-trip tests. |
| `app/src/launch_configs/session_store.rs` (modify) | Owns the checkpoint format and `stored-sessions.json`. Gains `clone_checkpoints`. |
| `app/src/launch_configs/session_store_tests.rs` (modify) | Checkpoint clone tests, reusing the existing `window()` fixture. |
| `app/src/launch_configs/clone_session.rs` (create) | Free-identity search, format-preserving TOML copy, clone orchestration. |
| `app/src/launch_configs/clone_session_tests.rs` (create) | Identity, TOML, and orchestration tests. |
| `app/src/launch_configs/session_in_use_dialog.rs` (create) | The dialog view: message, three buttons, typed actions. |
| `app/src/launch_configs/mod.rs` (modify) | Module registration and `init`. |
| `app/src/workspace/view.rs` (modify) | Hosts the dialog; handles its events. |
| `app/src/workspace/util.rs` (modify) | `is_session_in_use_dialog_open` flag. |
| `app/src/root_view.rs` (modify) | Replaces the two silent `return`s with the dialog. |
| `app/src/search/command_palette/launch_config/data_source.rs` (modify) | Keys config maps by source. |
| `app/src/search/command_palette/launch_config/renderer.rs` (modify) | Labels each row `Session` or `YAML` instead of the uninformative `Configuration`. |

---

### Task 1: Record and read the lease owner

`claim_session` currently answers `Ok(true)` / `Ok(false)` and records nothing about the holder. Give it a typed result and an `owner.json` written under the exclusive lock.

**Files:**
- Modify: `app/src/session_configs.rs:17-91`
- Modify: `app/src/root_view.rs:586-604` and `:679-700` (call sites only, behavior unchanged this task)
- Test: `app/src/session_configs_tests.rs`

**Interfaces:**
- Consumes: nothing.
- Produces:
  ```rust
  pub struct SessionOwner {
      pub pid: u32,
      pub claimed_at_unix_seconds: i64,
      pub app_id: String,
  }
  pub enum SessionClaim { Claimed, HeldBy(Option<SessionOwner>) }
  pub fn claim_session(source: &LaunchConfigWindowSource) -> anyhow::Result<SessionClaim>
  ```
  Task 6 matches on `SessionClaim`. Task 4 renders `SessionOwner`.

- [ ] **Step 1: Write the failing test**

Append to `app/src/session_configs_tests.rs`:

```rust
use std::collections::HashMap;

use super::{SessionClaim, claim_in_dir, read_owner};

#[test]
fn claiming_a_free_lease_records_the_owner() {
    let dir = tempfile::tempdir().expect("lease directory should be created");
    let mut leases = HashMap::new();

    let claim = claim_in_dir(dir.path(), "obachan", 1, &mut leases)
        .expect("claiming a free lease should succeed");

    assert!(matches!(claim, SessionClaim::Claimed));
    let owner = read_owner(dir.path(), "obachan", 1).expect("owner record should be written");
    assert_eq!(owner.pid, std::process::id());
    assert!(owner.claimed_at_unix_seconds > 0);
}

#[test]
fn a_second_holder_sees_the_recorded_owner() {
    let dir = tempfile::tempdir().expect("lease directory should be created");
    let mut first = HashMap::new();
    let mut second = HashMap::new();

    claim_in_dir(dir.path(), "obachan", 1, &mut first).expect("first claim should succeed");
    let claim = claim_in_dir(dir.path(), "obachan", 1, &mut second)
        .expect("a contended claim should not error");

    let SessionClaim::HeldBy(Some(owner)) = claim else {
        panic!("a held lease should report its owner");
    };
    assert_eq!(owner.pid, std::process::id());
}

#[test]
fn an_already_held_lease_is_reentrant_for_its_holder() {
    let dir = tempfile::tempdir().expect("lease directory should be created");
    let mut leases = HashMap::new();

    claim_in_dir(dir.path(), "obachan", 1, &mut leases).expect("first claim should succeed");
    let claim = claim_in_dir(dir.path(), "obachan", 1, &mut leases)
        .expect("reclaiming should succeed");

    assert!(matches!(claim, SessionClaim::Claimed));
}

#[test]
fn a_corrupt_owner_record_still_reports_the_lease_as_held() {
    let dir = tempfile::tempdir().expect("lease directory should be created");
    let mut first = HashMap::new();
    let mut second = HashMap::new();
    claim_in_dir(dir.path(), "obachan", 1, &mut first).expect("first claim should succeed");
    std::fs::write(dir.path().join("obachan").join("1").join("owner.json"), "{ not json")
        .expect("owner record should be overwritable");

    let claim = claim_in_dir(dir.path(), "obachan", 1, &mut second)
        .expect("a contended claim should not error");

    assert!(matches!(claim, SessionClaim::HeldBy(None)));
}

#[test]
fn different_generations_do_not_contend() {
    let dir = tempfile::tempdir().expect("lease directory should be created");
    let mut first = HashMap::new();
    let mut second = HashMap::new();

    claim_in_dir(dir.path(), "obachan", 1, &mut first).expect("first claim should succeed");
    let claim = claim_in_dir(dir.path(), "obachan", 2, &mut second)
        .expect("a fresh generation should be claimable");

    assert!(matches!(claim, SessionClaim::Claimed));
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo nextest run -p warp session_configs::tests`
Expected: FAIL — `claim_in_dir`, `read_owner`, `SessionClaim` do not exist.

- [ ] **Step 3: Implement the owner record and typed claim**

In `app/src/session_configs.rs`, replace the whole `claim_session` function (currently lines 54-91) and add the supporting items. Keep the existing `SESSION_LEASES` static.

Add to the imports at the top of the file:

```rust
use serde::Serialize;
```

(`Deserialize` is already imported; extend the existing `use serde::Deserialize;` to `use serde::{Deserialize, Serialize};` rather than adding a second line.)

Then:

```rust
const OWNER_FILE_NAME: &str = "owner.json";

/// Which process holds a session lease. Written by the claimer while it holds
/// the exclusive lock, so concurrent writers cannot interleave.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionOwner {
    pub pid: u32,
    pub claimed_at_unix_seconds: i64,
    pub app_id: String,
}

pub enum SessionClaim {
    Claimed,
    /// The owner record is absent or unreadable on a crash-and-reclaim, but the
    /// held lock still proves someone owns the session.
    HeldBy(Option<SessionOwner>),
}

pub fn claim_session(source: &LaunchConfigWindowSource) -> anyhow::Result<SessionClaim> {
    let session_id = source
        .session_id()
        .context("Session config source has no session id")?;
    let generation = source
        .session_generation()
        .context("Session config source has no generation")?;
    let mut leases = SESSION_LEASES.lock();
    claim_in_dir(&sessions_state_dir(), &session_id, generation, &mut leases)
}

fn sessions_state_dir() -> PathBuf {
    warp_core::paths::state_dir().join("session_config_sessions")
}

fn lease_dir(dir: &Path, session_id: &str, generation: u64) -> PathBuf {
    dir.join(session_id).join(generation.to_string())
}

fn claim_in_dir(
    dir: &Path,
    session_id: &str,
    generation: u64,
    leases: &mut HashMap<(String, u64), File>,
) -> anyhow::Result<SessionClaim> {
    let key = (session_id.to_owned(), generation);
    if leases.contains_key(&key) {
        return Ok(SessionClaim::Claimed);
    }

    let lease_dir = lease_dir(dir, session_id, generation);
    fs::create_dir_all(&lease_dir)
        .with_context(|| format!("Failed to create session lease directory {lease_dir:?}"))?;
    let lease_path = lease_dir.join("owner.lock");
    let lease = File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lease_path)
        .with_context(|| format!("Failed to open session lease {lease_path:?}"))?;
    match lease.try_lock_exclusive() {
        Ok(true) => {
            write_owner(&lease_dir)?;
            leases.insert(key, lease);
            Ok(SessionClaim::Claimed)
        }
        Ok(false) => Ok(SessionClaim::HeldBy(read_owner(dir, session_id, generation))),
        Err(error) => {
            Err(error).with_context(|| format!("Failed to claim session lease {lease_path:?}"))
        }
    }
}

fn write_owner(lease_dir: &Path) -> anyhow::Result<()> {
    let owner = SessionOwner {
        pid: std::process::id(),
        claimed_at_unix_seconds: chrono::Utc::now().timestamp(),
        app_id: warp_core::channel::ChannelState::app_id()
            .application_name()
            .to_owned(),
    };
    let path = lease_dir.join(OWNER_FILE_NAME);
    let contents = serde_json::to_vec(&owner).context("Failed to serialize session owner")?;
    fs::write(&path, contents)
        .with_context(|| format!("Failed to write session owner record {path:?}"))
}

fn read_owner(dir: &Path, session_id: &str, generation: u64) -> Option<SessionOwner> {
    let path = lease_dir(dir, session_id, generation).join(OWNER_FILE_NAME);
    serde_json::from_slice(&fs::read(path).ok()?).ok()
}
```

`read_owner` returns `Option` rather than `Result` because a missing or corrupt record is expected after a crash and must not turn a "held" answer into an error.

- [ ] **Step 4: Update the two call sites to the new type**

In `app/src/root_view.rs`, in `open_launch_config` (around line 586), change:

```rust
        match crate::session_configs::claim_session(source) {
            Ok(true) => {}
            Ok(false) => {
```

to:

```rust
        match crate::session_configs::claim_session(source) {
            Ok(crate::session_configs::SessionClaim::Claimed) => {}
            Ok(crate::session_configs::SessionClaim::HeldBy(_)) => {
```

Make the identical change in `open_stored_session` (around line 679, the arm whose `safe_warn!` says "Stored session is already open in another process"). Behavior is unchanged: both still `return`. Task 6 replaces them.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo nextest run -p warp session_configs::tests`
Expected: PASS, all five new tests plus the existing ones.

- [ ] **Step 6: Format, lint, and commit**

```bash
cd T:/projects/dev-ops/warp-session-in-use
./script/format
cargo clippy -p warp --all-targets --tests -- -D warnings
git add app/src/session_configs.rs app/src/session_configs_tests.rs app/src/root_view.rs
git commit -F - <<'EOF'
feat(session-configs): record which process holds a session lease

claim_session answered with a bare bool, so a caller that lost the race
could not say who won it. Write an owner record under the exclusive lock
and return a typed SessionClaim carrying it.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01DxxXYaK6sWmHNZg85bg8mP
EOF
```

---

### Task 2: Clone a session's checkpoints

`session_store` owns the checkpoint format, the pane snapshot tree, and `stored-sessions.json`, so the copy-and-sanitize step belongs here rather than in the caller.

**Files:**
- Modify: `app/src/launch_configs/session_store.rs`
- Test: `app/src/launch_configs/session_store_tests.rs`

**Interfaces:**
- Consumes: nothing from Task 1.
- Produces:
  ```rust
  pub(crate) fn clone_checkpoints(
      from: &LaunchConfigWindowSource,
      to_path: &Path,
      to_config_name: &str,
  ) -> Result<Vec<LaunchConfigWindowSource>>
  ```
  Returns one source per cloned window, ordered by window index. Task 3 calls it; Task 6 opens the returned sources.

- [ ] **Step 1: Write the failing test**

Append to `app/src/launch_configs/session_store_tests.rs`:

```rust
use std::path::Path;

use super::{clone_checkpoints_in_dir, load_from_dir, read_catalog};

fn session_source(path: &str, generation: usize, window_index: usize) -> LaunchConfigWindowSource {
    LaunchConfigWindowSource {
        path: PathBuf::from(path),
        document_index: generation,
        window_index,
        config_name: "OBACHAN".to_owned(),
    }
}

fn only_terminal(window: &WindowSnapshot) -> &TerminalPaneSnapshot {
    let PaneNodeSnapshot::Leaf(leaf) = &window.tabs[0].root else {
        panic!("fixture window has a single leaf pane");
    };
    let LeafContents::Terminal(terminal) = &leaf.contents else {
        panic!("fixture leaf is a terminal");
    };
    terminal
}

fn seed_source_checkpoint(dir: &Path) -> LaunchConfigWindowSource {
    let source = session_source("/cfg/OBACHAN.toml", 1, 0);
    save_to_dir(
        dir,
        &source,
        &window(source.clone()),
        &HashMap::new(),
        false,
        1_767_225_600,
    )
    .expect("the source checkpoint should be written");
    source
}

#[test]
fn cloning_rewrites_the_source_and_keeps_the_layout() {
    let dir = tempfile::tempdir().expect("session directory should be created");
    let source = seed_source_checkpoint(dir.path());

    let cloned = clone_checkpoints_in_dir(
        dir.path(),
        &source,
        Path::new("/cfg/OBACHAN-2.toml"),
        "OBACHAN (2)",
    )
    .expect("cloning should succeed");

    assert_eq!(cloned.len(), 1);
    assert_eq!(cloned[0].path, PathBuf::from("/cfg/OBACHAN-2.toml"));
    assert_eq!(cloned[0].document_index, 1);
    assert_eq!(cloned[0].window_index, 0);
    assert_eq!(cloned[0].config_name, "OBACHAN (2)");

    let restored = load_from_dir(dir.path(), &cloned[0])
        .expect("the clone should load")
        .expect("the clone should exist");
    let original = load_from_dir(dir.path(), &source)
        .expect("the original should load")
        .expect("the original should exist");
    assert_eq!(restored.window.tabs.len(), original.window.tabs.len());
    assert_eq!(
        only_terminal(&restored.window).cwd,
        only_terminal(&original.window).cwd
    );
    assert_eq!(
        only_terminal(&restored.window).shell_launch_data,
        only_terminal(&original.window).shell_launch_data
    );
}

#[test]
fn cloning_releases_every_identity_owned_by_the_live_window() {
    let dir = tempfile::tempdir().expect("session directory should be created");
    let source = seed_source_checkpoint(dir.path());

    let cloned = clone_checkpoints_in_dir(
        dir.path(),
        &source,
        Path::new("/cfg/OBACHAN-2.toml"),
        "OBACHAN (2)",
    )
    .expect("cloning should succeed");

    let restored = load_from_dir(dir.path(), &cloned[0])
        .expect("the clone should load")
        .expect("the clone should exist");
    let original = load_from_dir(dir.path(), &source)
        .expect("the original should load")
        .expect("the original should exist");
    let clone_pane = only_terminal(&restored.window);
    let original_pane = only_terminal(&original.window);

    assert_ne!(clone_pane.uuid, original_pane.uuid);
    assert!(!clone_pane.uuid.is_empty());
    assert_eq!(clone_pane.persistent_session_id, None);
    assert_eq!(clone_pane.cli_agent_resume_descriptor, None);
    assert!(clone_pane.conversation_ids_to_restore.is_empty());
    assert_eq!(clone_pane.active_conversation_id, None);
    assert!(restored.block_lists.is_empty());

    assert!(original_pane.persistent_session_id.is_some());
    assert!(original_pane.cli_agent_resume_descriptor.is_some());
}

#[test]
fn cloning_registers_the_clone_in_the_catalog() {
    let dir = tempfile::tempdir().expect("session directory should be created");
    let source = seed_source_checkpoint(dir.path());

    clone_checkpoints_in_dir(
        dir.path(),
        &source,
        Path::new("/cfg/OBACHAN-2.toml"),
        "OBACHAN (2)",
    )
    .expect("cloning should succeed");

    let catalog = read_catalog(dir.path()).expect("the catalog should be readable");
    let names = catalog
        .iter()
        .map(|window| window.source.config_name.as_str())
        .collect::<Vec<_>>();
    assert!(names.contains(&"OBACHAN"));
    assert!(names.contains(&"OBACHAN (2)"));
    assert_eq!(catalog.len(), 2);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo nextest run -p warp session_store`
Expected: FAIL — `clone_checkpoints_in_dir` does not exist.

- [ ] **Step 3: Implement checkpoint cloning**

In `app/src/launch_configs/session_store.rs`, add after `load_stored_session` (near line 852). The pane walk needs `LeafSnapshot` in scope; extend the existing `use crate::app_state::{...}` import to include it:

```rust
/// Frees every identity the live window owns so the clone cannot contend with
/// it: a persistent process has exactly one owner, restoring a window detaches
/// its conversations from the previous holder, and block lists are keyed by
/// pane uuid.
#[cfg(feature = "local_fs")]
fn release_owned_identities(node: &mut PaneNodeSnapshot) {
    match node {
        PaneNodeSnapshot::Branch(branch) => {
            for (_, child) in &mut branch.children {
                release_owned_identities(child);
            }
        }
        PaneNodeSnapshot::Leaf(LeafSnapshot {
            contents: LeafContents::Terminal(terminal),
            ..
        }) => {
            terminal.uuid = uuid::Uuid::new_v4().as_bytes().to_vec();
            terminal.persistent_session_id = None;
            terminal.cli_agent_resume_descriptor = None;
            terminal.conversation_ids_to_restore.clear();
            terminal.active_conversation_id = None;
        }
        PaneNodeSnapshot::Leaf(_) => {}
    }
}

#[cfg(feature = "local_fs")]
fn clone_checkpoints_in_dir(
    dir: &Path,
    from: &LaunchConfigWindowSource,
    to_path: &Path,
    to_config_name: &str,
) -> Result<Vec<LaunchConfigWindowSource>> {
    let session_id = from
        .session_id()
        .context("Cloning requires a session config source")?;
    let generation = from
        .session_generation()
        .context("Cloning requires a session config generation")?;
    let source_dir = dir.join(&session_id).join(generation.to_string());

    let mut window_indexes = std::fs::read_dir(&source_dir)
        .with_context(|| format!("Failed to read session checkpoints in {source_dir:?}"))?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name();
            let name = name.to_str()?;
            name.strip_prefix("window-")?
                .strip_suffix(".json")?
                .parse::<usize>()
                .ok()
        })
        .collect::<Vec<_>>();
    window_indexes.sort_unstable();
    if window_indexes.is_empty() {
        bail!("Session {session_id:?} has no checkpoints to clone");
    }

    let saved_at_unix_seconds = chrono::Utc::now().timestamp();
    let mut sources = Vec::with_capacity(window_indexes.len());
    let mut summaries = Vec::with_capacity(window_indexes.len());
    for window_index in window_indexes {
        let from_window = LaunchConfigWindowSource {
            window_index,
            ..from.clone()
        };
        let mut stored = read_stored_window(&session_path(dir, &from_window))?;
        let to_window = LaunchConfigWindowSource {
            path: to_path.to_path_buf(),
            document_index: 1,
            window_index,
            config_name: to_config_name.to_owned(),
        };

        for tab in &mut stored.window.tabs {
            release_owned_identities(&mut tab.root);
        }
        stored.window.launch_config_source = Some(to_window.clone());
        stored.source = to_window.clone();
        stored.block_lists.clear();
        stored.saved_at_unix_seconds = saved_at_unix_seconds;

        let path = session_path(dir, &to_window);
        write_json_atomically(&path, &stored)?;
        summaries.push(window_summary(dir, &path, &stored, None)?);
        sources.push(to_window);
    }

    update_catalog(dir, summaries, Vec::new())?;
    Ok(sources)
}

pub(crate) fn clone_checkpoints(
    from: &LaunchConfigWindowSource,
    to_path: &Path,
    to_config_name: &str,
) -> Result<Vec<LaunchConfigWindowSource>> {
    #[cfg(feature = "local_fs")]
    {
        clone_checkpoints_in_dir(&sessions_dir(), from, to_path, to_config_name)
    }
    #[cfg(not(feature = "local_fs"))]
    {
        let _ = (from, to_path, to_config_name);
        bail!("Cloning a session requires local filesystem support")
    }
}
```

`read_catalog` is currently private; the test calls it. Change its signature line from `fn read_catalog` to `pub(crate) fn read_catalog` (line 497).

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo nextest run -p warp session_store`
Expected: PASS, including the three new tests.

- [ ] **Step 5: Format, lint, and commit**

```bash
cd T:/projects/dev-ops/warp-session-in-use
./script/format
cargo clippy -p warp --all-targets --tests -- -D warnings
git add app/src/launch_configs/session_store.rs app/src/launch_configs/session_store_tests.rs
git commit -F - <<'EOF'
feat(session-store): clone a session's checkpoints under a new source

Copy each window checkpoint to a new session identity, reissuing pane
uuids and dropping the persistent session, agent resume descriptor,
conversations, and block lists that the live window owns. Register the
copies in the catalog so the palette can see them.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01DxxXYaK6sWmHNZg85bg8mP
EOF
```

---

### Task 3: Pick a free identity and copy the session TOML

**Files:**
- Create: `app/src/launch_configs/clone_session.rs`
- Create: `app/src/launch_configs/clone_session_tests.rs`
- Modify: `app/src/launch_configs/mod.rs`

**Interfaces:**
- Consumes: `session_store::clone_checkpoints` from Task 2.
- Produces:
  ```rust
  pub(crate) struct ClonedSession {
      pub name: String,
      pub config_path: PathBuf,
      pub windows: Vec<LaunchConfigWindowSource>,
  }
  pub(crate) fn clone_session(from: &LaunchConfigWindowSource) -> anyhow::Result<ClonedSession>
  ```
  Task 6 calls `clone_session` and opens `windows`.

- [ ] **Step 1: Write the failing test**

Create `app/src/launch_configs/clone_session_tests.rs`:

```rust
use std::path::{Path, PathBuf};

use super::{clone_config_file, free_identity};

fn write_config(dir: &Path, file_stem: &str, id: &str, name: &str) -> PathBuf {
    let path = dir.join(format!("{file_stem}.toml"));
    std::fs::write(
        &path,
        format!(
            "# keep me\nversion = 1\nid = \"{id}\"\nname = \"{name}\"\ngeneration = 3\nactive_tab = \"shield\"\n\n[[tabs]]\nid = \"shield\"\nconfig = \"sessions/OBACHAN/05-shield.toml\"\n"
        ),
    )
    .expect("the config should be written");
    path
}

#[test]
fn the_first_free_identity_is_suffixed_with_two() {
    let dir = tempfile::tempdir().expect("config directory should be created");
    write_config(dir.path(), "OBACHAN", "obachan", "OBACHAN");

    let identity = free_identity(dir.path(), "OBACHAN").expect("an identity should be free");

    assert_eq!(identity.file_stem, "OBACHAN-2");
    assert_eq!(identity.id, "obachan-2");
    assert_eq!(identity.name, "OBACHAN (2)");
}

#[test]
fn a_taken_identity_advances_to_the_next_free_one() {
    let dir = tempfile::tempdir().expect("config directory should be created");
    write_config(dir.path(), "OBACHAN", "obachan", "OBACHAN");
    write_config(dir.path(), "OBACHAN-2", "obachan-2", "OBACHAN (2)");

    let identity = free_identity(dir.path(), "OBACHAN").expect("an identity should be free");

    assert_eq!(identity.file_stem, "OBACHAN-3");
    assert_eq!(identity.id, "obachan-3");
    assert_eq!(identity.name, "OBACHAN (3)");
}

#[test]
fn copying_rewrites_the_identity_and_keeps_everything_else() {
    let dir = tempfile::tempdir().expect("config directory should be created");
    let from = write_config(dir.path(), "OBACHAN", "obachan", "OBACHAN");
    let identity = free_identity(dir.path(), "OBACHAN").expect("an identity should be free");

    let path = clone_config_file(&from, dir.path(), &identity).expect("the copy should succeed");

    let contents = std::fs::read_to_string(&path).expect("the copy should be readable");
    assert_eq!(path, dir.path().join("OBACHAN-2.toml"));
    assert!(contents.contains("id = \"obachan-2\""));
    assert!(contents.contains("name = \"OBACHAN (2)\""));
    assert!(contents.contains("generation = 1"));
    assert!(contents.contains("active_tab = \"shield\""));
    assert!(contents.contains("config = \"sessions/OBACHAN/05-shield.toml\""));
    assert!(contents.contains("# keep me"));
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo nextest run -p warp clone_session`
Expected: FAIL — the module does not exist.

- [ ] **Step 3: Implement the module**

Create `app/src/launch_configs/clone_session.rs`:

```rust
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::launch_configs::launch_config::LaunchConfigWindowSource;
use crate::launch_configs::session_store;

/// The identity a clone will take: a file stem, the session id derived from it,
/// and the display name.
pub(crate) struct CloneIdentity {
    pub file_stem: String,
    pub id: String,
    pub name: String,
}

pub(crate) struct ClonedSession {
    pub name: String,
    pub config_path: PathBuf,
    pub windows: Vec<LaunchConfigWindowSource>,
}

/// The highest suffix worth trying before giving up. A user with this many
/// clones of one session has a different problem.
const MAX_CLONE_SUFFIX: usize = 100;

fn free_identity(session_configs_dir: &Path, from_stem: &str) -> Result<CloneIdentity> {
    for suffix in 2..=MAX_CLONE_SUFFIX {
        let file_stem = format!("{from_stem}-{suffix}");
        if !session_configs_dir
            .join(format!("{file_stem}.toml"))
            .exists()
        {
            return Ok(CloneIdentity {
                id: file_stem.to_lowercase(),
                name: format!("{from_stem} ({suffix})"),
                file_stem,
            });
        }
    }
    bail!("No free name for a clone of {from_stem:?}")
}

fn clone_config_file(
    from: &Path,
    session_configs_dir: &Path,
    identity: &CloneIdentity,
) -> Result<PathBuf> {
    let contents = std::fs::read_to_string(from)
        .with_context(|| format!("Failed to read session config {from:?}"))?;
    let mut document = contents
        .parse::<toml_edit::DocumentMut>()
        .with_context(|| format!("Failed to parse session config {from:?}"))?;
    document["id"] = toml_edit::value(identity.id.as_str());
    document["name"] = toml_edit::value(identity.name.as_str());
    // A clone starts its own history rather than inheriting the generation the
    // original is on, so its lease and checkpoints never alias the original's.
    document["generation"] = toml_edit::value(1_i64);

    let path = session_configs_dir.join(format!("{}.toml", identity.file_stem));
    std::fs::write(&path, document.to_string())
        .with_context(|| format!("Failed to write cloned session config {path:?}"))?;
    Ok(path)
}

pub(crate) fn clone_session(from: &LaunchConfigWindowSource) -> Result<ClonedSession> {
    let session_configs_dir = crate::user_config::session_configs_dir();
    let from_stem = from
        .path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .context("Session config path has no file name")?;

    let identity = free_identity(&session_configs_dir, from_stem)?;
    let config_path = clone_config_file(&from.path, &session_configs_dir, &identity)?;
    let windows = session_store::clone_checkpoints(from, &config_path, &identity.name)?;

    Ok(ClonedSession {
        name: identity.name,
        config_path,
        windows,
    })
}

#[cfg(test)]
#[path = "clone_session_tests.rs"]
mod tests;
```

In `app/src/launch_configs/mod.rs`, add the module declaration alongside the existing ones:

```rust
pub mod clone_session;
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo nextest run -p warp clone_session`
Expected: PASS, all three tests.

- [ ] **Step 5: Format, lint, and commit**

```bash
cd T:/projects/dev-ops/warp-session-in-use
./script/format
cargo clippy -p warp --all-targets --tests -- -D warnings
git add app/src/launch_configs/clone_session.rs app/src/launch_configs/clone_session_tests.rs app/src/launch_configs/mod.rs
git commit -F - <<'EOF'
feat(launch-configs): clone a session config to a free identity

Pick the first unused OBACHAN-N stem, copy the TOML with toml_edit so
comments survive, reset the generation, and clone the checkpoints onto
the new source.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01DxxXYaK6sWmHNZg85bg8mP
EOF
```

---

### Task 4: The busy-session dialog

Mirrors `app/src/tab_configs/remove_confirmation_dialog.rs`, which is the house pattern for a small decision dialog.

**Files:**
- Create: `app/src/launch_configs/session_in_use_dialog.rs`
- Modify: `app/src/launch_configs/mod.rs`

**Interfaces:**
- Consumes: `SessionOwner` from Task 1.
- Produces:
  ```rust
  pub(crate) struct SessionInUseDialog;
  impl SessionInUseDialog {
      pub fn new(ctx: &mut ViewContext<Self>) -> Self;
      pub fn set_session(&mut self, name: String, owner: Option<SessionOwner>);
  }
  pub(crate) enum SessionInUseEvent { Clone, OpenFromTemplate, Cancel }
  pub(crate) fn init(app: &mut AppContext);
  ```
  Task 5 hosts the view and matches on `SessionInUseEvent`.

- [ ] **Step 1: Write the dialog**

Create `app/src/launch_configs/session_in_use_dialog.rs`:

```rust
use pathfinder_geometry::vector::vec2f;
use warp_core::ui::theme::Fill;
use warpui::elements::{
    Align, ChildAnchor, ChildView, Container, OffsetPositioning, ParentAnchor, ParentOffsetBounds,
    Stack,
};
use warpui::keymap::{FixedBinding, Keystroke};
use warpui::ui_components::components::{UiComponent, UiComponentStyles};
use warpui::{
    AppContext, Element, Entity, FocusContext, SingletonEntity, TypedActionView, View, ViewContext,
    ViewHandle,
};

use crate::appearance::Appearance;
use crate::session_configs::SessionOwner;
use crate::ui_components::dialog::{Dialog, dialog_styles};
use crate::view_components::action_button::{
    ActionButton, KeystrokeSource, NakedTheme, PrimaryTheme, SecondaryTheme,
};

pub(crate) fn init(app: &mut AppContext) {
    use warpui::keymap::macros::*;

    app.register_fixed_bindings([
        FixedBinding::new(
            "escape",
            SessionInUseAction::Cancel,
            id!(SessionInUseDialog::ui_name()),
        ),
        FixedBinding::new(
            "enter",
            SessionInUseAction::Clone,
            id!(SessionInUseDialog::ui_name()),
        ),
    ]);
}

const DIALOG_WIDTH: f32 = 480.;

pub(crate) enum SessionInUseEvent {
    Clone,
    OpenFromTemplate,
    Cancel,
}

#[derive(Debug)]
pub(crate) enum SessionInUseAction {
    Clone,
    OpenFromTemplate,
    Cancel,
}

pub(crate) struct SessionInUseDialog {
    cancel_button: ViewHandle<ActionButton>,
    template_button: ViewHandle<ActionButton>,
    clone_button: ViewHandle<ActionButton>,
    session_name: String,
    owner: Option<SessionOwner>,
}

impl SessionInUseDialog {
    pub fn new(ctx: &mut ViewContext<Self>) -> Self {
        let cancel_button = ctx.add_typed_action_view(|_| {
            ActionButton::new("Cancel", NakedTheme).on_click(|ctx| {
                ctx.dispatch_typed_action(SessionInUseAction::Cancel);
            })
        });
        let template_button = ctx.add_typed_action_view(|_| {
            ActionButton::new("Open from Template", SecondaryTheme).on_click(|ctx| {
                ctx.dispatch_typed_action(SessionInUseAction::OpenFromTemplate);
            })
        });
        let enter_keystroke = Keystroke::parse("enter").expect("Valid keystroke");
        let clone_button = ctx.add_typed_action_view(|ctx| {
            ActionButton::new("Clone Session", PrimaryTheme)
                .with_keybinding(KeystrokeSource::Fixed(enter_keystroke), ctx)
                .on_click(|ctx| {
                    ctx.dispatch_typed_action(SessionInUseAction::Clone);
                })
        });

        Self {
            cancel_button,
            template_button,
            clone_button,
            session_name: String::new(),
            owner: None,
        }
    }

    pub fn set_session(&mut self, name: String, owner: Option<SessionOwner>) {
        self.session_name = name;
        self.owner = owner;
    }

    fn body(&self) -> String {
        let holder = match &self.owner {
            Some(owner) => {
                let claimed_at = chrono::DateTime::from_timestamp(owner.claimed_at_unix_seconds, 0)
                    .map(|time| {
                        chrono::DateTime::<chrono::Local>::from(time)
                            .format("%b %d at %H:%M")
                            .to_string()
                    });
                match claimed_at {
                    Some(claimed_at) => format!(
                        "Held by {} process {}, since {claimed_at}.",
                        owner.app_id, owner.pid
                    ),
                    None => format!("Held by {} process {}.", owner.app_id, owner.pid),
                }
            }
            None => "Held by another Warp process.".to_owned(),
        };
        format!(
            "{holder}\n\nClone copies the layout, working directories and shells. Running \
             processes, scrollback and agent conversations stay with the original window."
        )
    }
}

impl Entity for SessionInUseDialog {
    type Event = SessionInUseEvent;
}

impl View for SessionInUseDialog {
    fn ui_name() -> &'static str {
        "SessionInUseDialog"
    }

    fn on_focus(&mut self, _focus_ctx: &FocusContext, ctx: &mut ViewContext<Self>) {
        ctx.focus_self();
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);

        let cancel_button = Container::new(ChildView::new(&self.cancel_button).finish())
            .with_margin_right(12.)
            .finish();
        let template_button = Container::new(ChildView::new(&self.template_button).finish())
            .with_margin_right(12.)
            .finish();

        let dialog = Dialog::new(
            format!("'{}' is already open", self.session_name),
            Some(self.body()),
            UiComponentStyles {
                width: Some(DIALOG_WIDTH),
                ..dialog_styles(appearance)
            },
        )
        .with_bottom_row_child(cancel_button)
        .with_bottom_row_child(template_button)
        .with_bottom_row_child(ChildView::new(&self.clone_button).finish())
        .build()
        .finish();

        let mut stack = Stack::new();
        stack.add_positioned_child(
            dialog,
            OffsetPositioning::offset_from_parent(
                vec2f(0., 0.),
                ParentOffsetBounds::WindowByPosition,
                ParentAnchor::Center,
                ChildAnchor::Center,
            ),
        );

        Container::new(Align::new(stack.finish()).finish())
            .with_background_color(Fill::blur().into())
            .with_corner_radius(app.windows().window_corner_radius())
            .finish()
    }
}

impl TypedActionView for SessionInUseDialog {
    type Action = SessionInUseAction;

    fn handle_action(&mut self, action: &SessionInUseAction, ctx: &mut ViewContext<Self>) {
        match action {
            SessionInUseAction::Clone => ctx.emit(SessionInUseEvent::Clone),
            SessionInUseAction::OpenFromTemplate => ctx.emit(SessionInUseEvent::OpenFromTemplate),
            SessionInUseAction::Cancel => ctx.emit(SessionInUseEvent::Cancel),
        }
    }
}
```

In `app/src/launch_configs/mod.rs`, add the module and call its `init` beside `save_modal::init(app);`:

```rust
pub mod session_in_use_dialog;
```

```rust
    session_in_use_dialog::init(app);
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check -p warp`
Expected: clean. `NakedTheme`, `SecondaryTheme` and `PrimaryTheme` are all exported from `app/src/view_components/action_button.rs` (lines 1090, 1168, 1227).

- [ ] **Step 3: Format, lint, and commit**

```bash
cd T:/projects/dev-ops/warp-session-in-use
./script/format
cargo clippy -p warp --all-targets --tests -- -D warnings
git add app/src/launch_configs/session_in_use_dialog.rs app/src/launch_configs/mod.rs
git commit -F - <<'EOF'
feat(launch-configs): add the busy-session dialog

A dialog naming the process that holds a session lease, offering to
clone the session, open its template, or cancel.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01DxxXYaK6sWmHNZg85bg8mP
EOF
```

---

### Task 5: Host the dialog in the workspace

**Files:**
- Modify: `app/src/workspace/util.rs:80-210`
- Modify: `app/src/workspace/view.rs`

**Interfaces:**
- Consumes: `SessionInUseDialog`, `SessionInUseEvent` from Task 4; `clone_session::clone_session` from Task 3.
- Produces:
  ```rust
  impl Workspace {
      pub(crate) fn open_session_in_use_dialog(
          &mut self,
          name: String,
          source: LaunchConfigWindowSource,
          window_template: WindowTemplate,
          owner: Option<SessionOwner>,
          ctx: &mut ViewContext<Self>,
      );
  }
  ```
  Task 6 calls this from `root_view`.

- [ ] **Step 1: Add the workspace state flag**

In `app/src/workspace/util.rs`, add a field to `WorkspaceState` next to `is_remove_tab_config_dialog_open` (line 118):

```rust
    pub is_session_in_use_dialog_open: bool,
```

Add it to the `||` chain that reports whether anything is open (near line 161):

```rust
            || self.is_session_in_use_dialog_open
```

And to the reset block (near line 205):

```rust
        self.is_session_in_use_dialog_open = false;
```

- [ ] **Step 2: Add the field, builder, and event handler**

In `app/src/workspace/view.rs`:

Import beside the existing tab_configs dialog import (line 377):

```rust
use crate::launch_configs::session_in_use_dialog::{SessionInUseDialog, SessionInUseEvent};
```

Field beside `remove_tab_config_confirmation_dialog` (line 1275):

```rust
    session_in_use_dialog: ViewHandle<SessionInUseDialog>,
    /// What the dialog acts on when the user picks Clone or Open from Template.
    session_in_use_request: Option<SessionInUseRequest>,
```

Above `impl Workspace`, next to the other small workspace types:

```rust
struct SessionInUseRequest {
    source: LaunchConfigWindowSource,
    window_template: WindowTemplate,
}
```

`LaunchConfigWindowSource` and `WindowTemplate` are already imported unqualified at `app/src/workspace/view.rs:288`; use the bare names throughout this file.

Builder beside `build_remove_tab_config_confirmation_dialog` (line 2334):

```rust
    fn build_session_in_use_dialog(ctx: &mut ViewContext<Self>) -> ViewHandle<SessionInUseDialog> {
        let dialog = ctx.add_typed_action_view(SessionInUseDialog::new);
        ctx.subscribe_to_view(&dialog, |me, _, event, ctx| {
            me.handle_session_in_use_event(event, ctx);
        });
        dialog
    }

    fn handle_session_in_use_event(
        &mut self,
        event: &SessionInUseEvent,
        ctx: &mut ViewContext<Self>,
    ) {
        self.current_workspace_state.is_session_in_use_dialog_open = false;
        ctx.notify();
        let Some(request) = self.session_in_use_request.take() else {
            return;
        };
        match event {
            SessionInUseEvent::Cancel => {}
            SessionInUseEvent::OpenFromTemplate => {
                crate::root_view::open_window_from_template(request.window_template, ctx);
            }
            SessionInUseEvent::Clone => self.clone_session_in_use(&request.source, ctx),
        }
    }

    #[cfg(feature = "local_fs")]
    fn clone_session_in_use(
        &mut self,
        source: &LaunchConfigWindowSource,
        ctx: &mut ViewContext<Self>,
    ) {
        match crate::launch_configs::clone_session::clone_session(source) {
            Ok(cloned) => crate::root_view::open_cloned_session(cloned, ctx),
            Err(error) => {
                report_error!(error.context("Failed to clone session config"));
                self.toast_stack.update(ctx, |toast_stack, ctx| {
                    toast_stack.add_ephemeral_toast(
                        DismissibleToast::error("Failed to clone the session.".to_owned()),
                        ctx,
                    );
                });
            }
        }
    }

    #[cfg(not(feature = "local_fs"))]
    fn clone_session_in_use(
        &mut self,
        _source: &LaunchConfigWindowSource,
        _ctx: &mut ViewContext<Self>,
    ) {
        report_error!("Cannot clone a session config from the web");
    }

    pub(crate) fn open_session_in_use_dialog(
        &mut self,
        name: String,
        source: LaunchConfigWindowSource,
        window_template: WindowTemplate,
        owner: Option<crate::session_configs::SessionOwner>,
        ctx: &mut ViewContext<Self>,
    ) {
        self.close_palette(true, None, ctx);
        self.session_in_use_request = Some(SessionInUseRequest {
            source,
            window_template,
        });
        self.session_in_use_dialog.update(ctx, |dialog, ctx| {
            dialog.set_session(name, owner);
            ctx.notify();
        });
        self.current_workspace_state.is_session_in_use_dialog_open = true;
        ctx.focus(&self.session_in_use_dialog);
        ctx.notify();
    }
```

Construction, in the struct literal beside `remove_tab_config_confirmation_dialog:` (line 3711):

```rust
            session_in_use_dialog: Self::build_session_in_use_dialog(ctx),
            session_in_use_request: None,
```

Render, beside the `is_remove_tab_config_dialog_open` block (line 28758):

```rust
        if self.current_workspace_state.is_session_in_use_dialog_open {
            stack.add_positioned_overlay_child(
                ChildView::new(&self.session_in_use_dialog).finish(),
                OffsetPositioning::offset_from_parent(
                    Vector2F::zero(),
                    ParentOffsetBounds::WindowByPosition,
                    ParentAnchor::Center,
                    ChildAnchor::Center,
                ),
            );
        }
```

- [ ] **Step 3: Add the two root_view helpers the handler calls**

In `app/src/root_view.rs`, beside `open_new_with_workspace_source` (line 1123):

```rust
/// Opens a launch config window that claims no lease and writes no checkpoints,
/// so it cannot contend with the process that owns the session.
pub(crate) fn open_window_from_template(
    window_template: launch_config::WindowTemplate,
    ctx: &mut AppContext,
) {
    open_new_with_workspace_source(
        NewWorkspaceSource::FromTemplate {
            window_template,
            launch_config_source: None,
        },
        ctx,
    );
}

pub(crate) fn open_cloned_session(
    cloned: crate::launch_configs::clone_session::ClonedSession,
    ctx: &mut AppContext,
) {
    for source in cloned.windows {
        #[cfg(feature = "local_fs")]
        match crate::session_configs::claim_session(&source) {
            Ok(crate::session_configs::SessionClaim::Claimed) => {}
            Ok(crate::session_configs::SessionClaim::HeldBy(_)) => {
                safe_warn!(
                    safe: ("A freshly cloned session was already claimed"),
                    full: ("A freshly cloned session was already claimed: {source:?}")
                );
                continue;
            }
            Err(error) => {
                report_error!(error.context("Failed to claim the cloned session"));
                continue;
            }
        }

        match crate::launch_configs::session_store::load(&source) {
            Ok(Some(session)) => {
                crate::launch_configs::session_store::begin_run(&source);
                open_new_with_workspace_source(
                    restored_launch_config_workspace_source(session, ctx),
                    ctx,
                );
            }
            Ok(None) => safe_warn!(
                safe: ("A cloned session has no checkpoint"),
                full: ("A cloned session has no checkpoint: {source:?}")
            ),
            Err(error) => report_error!(error.context("Failed to open the cloned session")),
        }
    }
}
```

A `ViewContext<Workspace>` derefs to `AppContext`, so the workspace handler can pass `ctx` to both.

- [ ] **Step 4: Verify it compiles**

Run: `cargo check -p warp`
Expected: clean. `DismissibleToast` (line 489) and `report_error!` (line 2402) are already in scope in `workspace/view.rs`; `safe_warn!`/`safe_error!` are not, which is why the clone failure path uses `report_error!`.

- [ ] **Step 5: Format, lint, and commit**

```bash
cd T:/projects/dev-ops/warp-session-in-use
./script/format
cargo clippy -p warp --all-targets --tests -- -D warnings
git add app/src/workspace/view.rs app/src/workspace/util.rs app/src/root_view.rs
git commit -F - <<'EOF'
feat(workspace): host the busy-session dialog

Wire the dialog's Clone and Open from Template choices to a cloned
session and to a lease-free template window respectively.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01DxxXYaK6sWmHNZg85bg8mP
EOF
```

---

### Task 6: Replace the silent returns

**Files:**
- Modify: `app/src/root_view.rs:571-604` (`open_launch_config`) and `:677-700` (`open_stored_session`)

**Interfaces:**
- Consumes: `SessionClaim` (Task 1), `Workspace::open_session_in_use_dialog` (Task 5), `open_window_from_template` (Task 5).
- Produces: nothing further.

- [ ] **Step 1: Add the shared handler**

In `app/src/root_view.rs`, beside `open_window_from_template`:

```rust
/// A session another process owns cannot be opened here, but the user still has
/// two ways forward. Offer them in the active window; with no window to host a
/// dialog, open the template rather than doing nothing at all.
fn offer_session_in_use_choices(
    name: String,
    source: launch_config::LaunchConfigWindowSource,
    window_template: launch_config::WindowTemplate,
    owner: Option<crate::session_configs::SessionOwner>,
    ctx: &mut AppContext,
) {
    match active_workspace(ctx) {
        Some(workspace) => workspace.update(ctx, |workspace, ctx| {
            workspace.open_session_in_use_dialog(name, source, window_template, owner, ctx);
        }),
        None => open_window_from_template(window_template, ctx),
    }
}
```

- [ ] **Step 2: Use it in `open_launch_config`**

Replace the `HeldBy` arm added in Task 1 (around line 589):

```rust
            Ok(crate::session_configs::SessionClaim::HeldBy(owner)) => {
                let Some(window_template) = arg.launch_config.windows.first().cloned() else {
                    return;
                };
                offer_session_in_use_choices(
                    arg.launch_config.name.clone(),
                    source.clone(),
                    window_template,
                    owner,
                    ctx,
                );
                return;
            }
```

- [ ] **Step 3: Use it in `open_stored_session`**

`open_stored_session` has a summary rather than a `LaunchConfig`, so it has no template to fall back on. Replace its `HeldBy` arm (around line 682) with a dialog that offers only cloning, by passing the template from the config the summary points at. Look the config up through `WarpConfig`:

```rust
            Ok(crate::session_configs::SessionClaim::HeldBy(owner)) => {
                let window_template = WarpConfig::as_ref(ctx)
                    .launch_and_session_configs()
                    .find(|config| config.window_source(0).as_ref() == Some(source))
                    .and_then(|config| config.windows.first().cloned());
                let Some(window_template) = window_template else {
                    safe_warn!(
                        safe: ("Stored session is already open in another process"),
                        full: (
                            "Stored session {:?} is already open in another process",
                            source.session_id()
                        )
                    );
                    return;
                };
                offer_session_in_use_choices(
                    arg.summary.name().to_owned(),
                    source.clone(),
                    window_template,
                    owner,
                    ctx,
                );
                return;
            }
```

`WarpConfig` is already imported in `root_view.rs`; if it is not, add `use crate::user_config::WarpConfig;`.

- [ ] **Step 4: Verify it compiles and the suite still passes**

Run: `cargo check -p warp`
Expected: clean.

Run: `cargo nextest run -p warp launch_config session_config session_store clone_session`
Expected: PASS.

- [ ] **Step 5: Manual verification**

With two Warp instances running against the same data profile:

1. Open a session config in the first instance.
2. Open the same one in the second instance.
3. Expected: the dialog appears naming the first instance's pid, instead of nothing happening.
4. Choose Clone. Expected: a new window opens with the same tabs, titles, colors and working directories, fresh shells, and a new `<NAME>-2.toml` appears in `session_configurations/`.
5. Reopen the palette. Expected: `<NAME> (2)` is listed.
6. Repeat and choose Open from Template. Expected: a window opens from the tab configs and no new files appear under `session_config_sessions/`.

- [ ] **Step 6: Format, lint, and commit**

```bash
cd T:/projects/dev-ops/warp-session-in-use
./script/format
cargo clippy -p warp --all-targets --tests -- -D warnings
git add app/src/root_view.rs
git commit -F - <<'EOF'
fix(launch-configs): stop opening a busy session silently

Opening a session config held by another process returned with only a
log line, so the palette appeared to ignore the selection. Offer the
choice to clone it or open its template instead.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01DxxXYaK6sWmHNZg85bg8mP
EOF
```

---

### Task 7: Stop session configs shadowing same-named launch configs

`launch_and_session_configs()` chains session configs last and the palette collects into a map keyed by lowercased name, so `OBACHAN.toml` overwrites `OBACHAN.yaml` and the YAML becomes unreachable.

**Files:**
- Modify: `app/src/search/command_palette/launch_config/data_source.rs:145-175` and `:206-275`
- Modify: `app/src/search/command_palette/launch_config/renderer.rs`

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces: nothing further.

- [ ] **Step 1: Write the failing test**

Add to the existing `mod tests` in `app/src/search/command_palette/launch_config/data_source.rs` (it already imports `LaunchConfig` at line 186):

```rust
    use std::path::PathBuf;

    use crate::launch_configs::launch_config::LaunchConfigSource;

    fn config_named(name: &str, path: &str) -> LaunchConfig {
        LaunchConfig {
            name: name.to_owned(),
            active_window_index: Some(0),
            windows: Vec::new(),
            source: Some(LaunchConfigSource {
                path: PathBuf::from(path),
                document_index: 1,
            }),
        }
    }

    #[test]
    fn a_yaml_and_a_session_config_sharing_a_name_are_both_searchable() {
        let mut searcher = FuzzyLaunchConfigSearcher::default();
        searcher.configs = [
            config_named("OBACHAN", "/cfg/OBACHAN.yaml"),
            config_named("OBACHAN", "/cfg/OBACHAN.toml"),
        ]
        .into_iter()
        .map(|config| (super::config_key(&config), config))
        .collect();

        let results = searcher.search("obachan").expect("search should succeed");

        assert_eq!(results.len(), 2);
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo nextest run -p warp a_yaml_and_a_session_config_sharing_a_name`
Expected: FAIL — `config_key` does not exist.

- [ ] **Step 3: Key the maps by source**

In `app/src/search/command_palette/launch_config/data_source.rs`, add above `FuzzyLaunchConfigSearcher`:

```rust
/// Two configs can share a name — a YAML template and the session config
/// derived from it commonly do — so the map has to key on where each one came
/// from or the later one silently replaces the earlier.
fn config_key(config: &LaunchConfig) -> String {
    match &config.source {
        Some(source) => format!(
            "{}\u{1f}{}",
            source.path.to_string_lossy(),
            source.document_index
        ),
        None => config.name.to_lowercase(),
    }
}
```

In `FuzzyLaunchConfigSearcher::refresh_search_index`, replace the key:

```rust
    fn refresh_search_index(&mut self, app: &AppContext) {
        self.configs = WarpConfig::as_ref(app)
            .launch_and_session_configs()
            .map(|config| (config_key(config), config.clone()))
            .collect();
    }
```

In `full_text_searcher::FullTextLaunchConfigSearcher::refresh_search_index`, the same change (the module already has `use super::config_key;` to add alongside its other `use crate::...` imports):

```rust
        fn refresh_search_index(&mut self, app: &AppContext) {
            self.configs = WarpConfig::as_ref(app)
                .launch_and_session_configs()
                .map(|config| (config_key(config), config.clone()))
                .collect();
```

Its `rebuild_search_index` currently indexes the map key as both the searched text and the id. The key is no longer the display name, so split them — otherwise the fuzzy highlight indices returned in `match_result.highlights.name` would point into a file path rather than the name shown:

```rust
        fn rebuild_search_index(&mut self) -> Result<(), anyhow::Error> {
            let documents = self.configs.iter().map(|(id, config)| LaunchConfigDocument {
                name: config.name.to_lowercase(),
                name_id: id.clone(),
            });
            self.searcher.rebuild_index_async(documents)
        }
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo nextest run -p warp a_yaml_and_a_session_config_sharing_a_name`
Expected: PASS.

- [ ] **Step 5: Add the source pill**

`LaunchConfig::render_config_description` (`app/src/search/command_palette/launch_config/renderer.rs:116-136`) already builds the pill row, and its first pill is a hardcoded `"Configuration"` that says nothing. Replace that label with the source kind rather than adding a fourth pill:

```rust
        Flex::row()
            .with_children(vec![
                Self::render_string_with_pill_styling(self.source_label(), appearance),
                Self::render_string_with_pill_styling(windows_str, appearance),
                Self::render_string_with_pill_styling(tabs_str, appearance),
            ])
            .finish()
    }

    /// A YAML template and the session config derived from it commonly share a
    /// name, so the row has to say which one it is.
    fn source_label(&self) -> &'static str {
        match self.source.as_ref().and_then(|source| source.path.extension()) {
            Some(extension) if extension == "toml" => "Session",
            Some(_) => "YAML",
            None => "Configuration",
        }
    }
```

`render_string_with_pill_styling` takes `(impl Into<String>, &Appearance)` and no highlight state, so `&'static str` is accepted directly. `search_item.rs` needs no change: the pill is derived from the config's own source.

- [ ] **Step 6: Verify and check the palette by hand**

Run: `cargo nextest run -p warp launch_config`
Expected: PASS.

Launch the app, open the launch config palette, and type the name of a config that exists as both a YAML and a session config. Expected: two rows, one pilled `Session` and one pilled `YAML`.

- [ ] **Step 7: Format, lint, and commit**

```bash
cd T:/projects/dev-ops/warp-session-in-use
./script/format
cargo clippy -p warp --all-targets --tests -- -D warnings
git add app/src/search/command_palette/launch_config/data_source.rs app/src/search/command_palette/launch_config/renderer.rs
git commit -F - <<'EOF'
fix(palette): stop a session config from hiding a same-named YAML

The palette keyed its config map by lowercased name, and session configs
are chained last, so OBACHAN.toml silently replaced OBACHAN.yaml. Key by
source instead and label each row with where it came from.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01DxxXYaK6sWmHNZg85bg8mP
EOF
```

---

## Final verification

- [ ] Run the full presubmit: `./script/presubmit`
- [ ] Confirm the six manual checks in Task 6 Step 5 and the palette check in Task 7 Step 6 all pass against two live instances.
