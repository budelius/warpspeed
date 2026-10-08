# Kimi Code and DeepSeek Harness — Part D: Move Session and Pause Read-back — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A Kimi Code session in a WarpSpeed pane, or a finished DeepSeek Harness (dsh) headless session, can be moved into a staging AgentCtrl run that resumes the same conversation. The AgentCtrl node can also read a kimi/dsh session back out (for the pause branch's `AgentCompleted.session_file`).

**Architecture:** WarpSpeed finds the session on disk. For Kimi that is a session directory, which it packs into an in-memory tar. For dsh it is the newest `session.vN.jsonl[.zstd]` file, uploaded unchanged. The upload goes through the existing import endpoint. On the node, `place_session` for kimi unpacks the tar under the run's `KIMI_CODE_HOME`, rewrites the recorded paths, and adds a line to Kimi's session index. For dsh it decompresses the file, changes the header `cwd`, recompresses, and writes it where dsh derives the path from that `cwd`. `read_session` does the reverse. The on-disk key algorithms (`workDirKey`, `projectKey`, `encodeSegment`) are ported line by line from the CLIs' TypeScript and pinned by test vectors.

**Tech Stack:** Rust. In warp: `tar` 0.4, `serde_json`, `walkdir` (`app`), `managed_agent_sessions`, `warp_cli`. In stoz3n-agent-ctrl: `tar` 0.4, `zstd` 0.13, `sha2` 0.10, `serde_json`. One TypeScript regex change in stoz3n-agent-dash.

## Global Constraints

- Repos and branches (spec "Parts, specs, plans"): `feat/kimi-dsh-agents` in each repo, based on `dev`.
  - WarpSpeed worktree `T:\projects\dev-ops\warp-kimi-dsh`. Push to `gitea` and `gitlab`, **never `origin`**.
  - agent-ctrl worktree `T:\projects\SOTZ3N\stoz3n-agent-ctrl-kimi-dsh`. Push to `origin` (Gitea).
  - agent-dash worktree `T:\projects\SOTZ3N\stoz3n-agent-dash-kimi-dsh`. Push to `origin`.
- Stage explicit paths only (`git add <path> …`). Never `git add -A` or `git add .`: other sessions edit these trees live.
- Every commit message ends with the line `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Never run `find /` in the Bash tool. Bound searches to a subtree or use Glob/Grep.
- Never restart or kill running WarpSpeed processes. Install the new build and use newly opened windows.
- Session payload limit: 33_554_432 bytes (`MAX_SESSION_FILE_BYTES`, 32 MiB) on both sides. For Kimi it applies to the tar *and* to the sum of the packed files. A move over the limit is refused with the reason.
- Session ids: 1–128 characters of `[A-Za-z0-9_-]`, in all four validators: the warp handoff, the dashboard import, the controller `ImportSession`, and the node `validate_session_id`. Kimi ids are `session_<uuid-v4>`. dsh ids are bare UUIDs (ACP) or `session-<uuid>` (web).
- AgentCtrl profiles (spec D·1): `kimi` → `staging-kimi-interactive-rw`, `dsh` → `staging-dsh-interactive-rw`.
- Kimi payload format: a tar built by the `tar` crate with GNU headers. Each entry is `<sessionId>/<relative path>`, joined with `/`. Only regular files are packed (directory entries are tolerated on read). It must contain `<sessionId>/state.json`.
- dsh payload format: the session file exactly as on disk, zstd or plaintext. The node always writes zstd `session.v<N>.jsonl.zstd` (`session.jsonl.zstd` when N = 0), where N is the header's `version`.
- Node homes (spec B·2): `KIMI_CODE_HOME=<run_dir>/kimi-home`, `DSH_HOME=<run_dir>/dsh-home`. The sessions root is `<home>/sessions` for both. `run_dir` is the `run_dir` argument of `place_session` (`Handoff::run_dir()` = `<worktree>.session`).
- Warp code style (AGENTS.md):
  - `ctx` is named `ctx` and goes last.
  - No wildcard `_` in matches over `CLIAgent` or `ManagedSessionsCommand`.
  - Inline format args.
  - Comments explain why only.
  - Tests go in `*_tests.rs` files.
  - Run `./script/format` and `cargo clippy` before pushing.
- agent-ctrl code style: inline `#[cfg(test)] mod tests`. `cargo fmt -- --check` and `cargo clippy --all-targets -- -D warnings` must be clean.
- Faster warp builds in this fresh worktree: point `CARGO_TARGET_DIR` at a sibling worktree's `target` and create `T:\projects\dev-ops\warp-kimi-dsh\target\debug` first (conpty.dll copy). On `dev` there are 22 pre-existing `StoznAuthManager` test failures; they are not regressions.

## Cross-plan interfaces (assumed — verify before the task that uses them)

**From Plan A (WarpSpeed local; D needs A merged into `feat/kimi-dsh-agents` first):**
- `CLIAgent::Kimi` and `CLIAgent::DeepSeekHarness` exist (`crate::terminal::CLIAgent`, derives `PartialEq, Eq, Copy, Sequence`).
- `command_prefixes().first()` is `"kimi"` / `"dsh"`. `display_name()` is `"Kimi Code"` / `"DeepSeek Harness"`.
- A adds both variants to the `None`/`bail!` arms of every exhaustive match in `app/src/terminal/agent_session_handoff.rs`, because the compiler forces it. D moves them into their own arms.
- `session.session_context.transcript_path` holds an absolute path from the `agent-event` channel (A·4):
  - Kimi: the session **directory** (`SessionStart` → `transcript_path` = session dir).
  - dsh: the session **file** (`agent/created` → `transcript_path` = session file).
- `crate::terminal::agent_session_history::find_all(CLIAgent::DeepSeekHarness) -> Vec<PastAgentSession>` lists dsh sessions with `session_id` and `cwd` from the header (A·3). `PastAgentSession` has pub fields `agent, session_id, cwd, canonical_cwd, title, modified`.
- `agent_session_history::is_safe_session_id` already accepts `_` (it does on `dev`).

**From Plan B (agent-ctrl adapters; D's node tasks need B merged into `feat/kimi-dsh-agents` first):**
- `src/agent/kimi.rs`: `KimiAdapter` (`CliAdapter`, `name() = "kimi"`, `supports_resume() = true`, `interactive_protocol() = Acp`). It launches with `KIMI_CODE_HOME=<run_dir>/kimi-home`, where `<run_dir>` is the same path `place_session` receives.
- `src/agent/dsh.rs`: `DshAdapter` (`name() = "dsh"`). It launches with `DSH_HOME=<run_dir>/dsh-home`, seeded from the node's `~/.dsh` template. **Seeding must not delete an existing `<run_dir>/dsh-home/sessions`.**
- B's ACP driver sends `session/resume {sessionId: <resume_ref>, cwd: <SpawnConfig.working_directory>}`. On a moved run, `working_directory` is the `cwd` that `place_session` received (`handoff.worktree`).
- `Backend::Kimi` (`"kimi"`) and `Backend::Dsh` (`"dsh"`) in `src/config/types.rs`.

**From the pause branch (`T:\projects\SOTZ3N\stoz3n-agent-ctrl-pause`, `feat/pause-and-second-node`):**
- `CliAdapter::locate_session(&self, session_id: &str, cwd: &Path, run_dir: &Path, env: &HashMap<String, String>) -> Option<LocatedSession>` returns `LocatedSession { resume_ref: String, file: PathBuf }`.
- `Handoff::pause` and the reconnect snapshot read `located.file` as bytes through `read_session_file(path)`, then send them in `AgentCompleted.session_file`. D's `read_session` takes the same four parameters, so it slots in where `locate_session` + `read_session_file` is called (Task 12).

**Produced by this plan (later tasks and other plans rely on these):**
- warp:
  - `managed_agent_sessions::AGENT_KINDS: [&str; 5]`
  - `crate::agent_ctrl::fleet_model::STARTABLE_AGENTS: [(CLIAgent, &str); 5]`
  - `agent_session_handoff::{session_content_len(&Path) -> Option<u64>, PreparedHandoff::session_len: u64}`
  - `AgentSessionRoots::{kimi_home, dsh_home}`
  - `warpspeed agent-ctrl move-session --agent dsh --session <id>`
- agent-ctrl:
  - `CliAdapter::read_session` / `AgentBackend::read_session` / `AgentRunner::read_session` (`&self, [backend,] session_id, cwd, run_dir, env) -> Result<Option<Vec<u8>>, BackendError>`)
  - `agent::kimi_session::{work_dir_key, kimi_home, place, read}`
  - `agent::dsh_session::{project_key, encode_segment, generation_file_name, place, read}`

## Spec gaps found while mapping the code (resolved here as noted)

1. **`_` in Kimi ids is rejected in four places, not one.** The four are warp `is_valid_session_id`, the dashboard `SESSION_ID` regex, the controller `moved_session`, and the node `validate_session_id`. The spec names only the last. Tasks 2, 6 and 7 fix all four; the dashboard change lands in Part C's repo.
2. **Handoff branch names.** `handoff_branch_name` takes the first 8 characters of the id, which is `session_` for every Kimi session. The fix strips a `session_`/`session-` prefix first (Task 2).
3. **Cut-over size check.** Cut-over compares `fs::metadata(file).len()` with the upload length. That is meaningless for a Kimi directory and its tar. Replaced by `session_content_len` (the summed file sizes) on both sides (Task 4).
4. **A dsh pane is never movable.** `CLIAgentSessionsModel::remove_session` runs when the headless command's block completes (`terminal/view.rs` ~:12186), so no pane holds a finished dsh session for "Move session" or `move-all` to find. Task 5 adds `warpspeed agent-ctrl move-session --agent dsh --session <id>`, which moves a recorded dsh session without a pane. **Ask the user to approve this before starting Task 5.**
5. **"Keeping N" without a file name.** The upload carries no file name (the multipart part is always named `session.jsonl`). The node therefore takes N from the header's `version`, which is how dsh names generations (`logPath` uses `SESSION_FORMAT_VERSION` = 4 → `session.v4.jsonl`).
6. **`projectKey` wrapping.** dsh's own `projectKey()` already returns `--<slug>--`. The spec's `--<projectKey(cwd)>--` would double the dashes. `project_key` here returns the wrapped form, matching the source.
7. **zstd frame layout.** dsh refuses a file whose first frame is not exactly the header line (`assertZstdHeaderFrame`), and every complete frame must end on a record boundary. The node writes the layout of dsh's `encodeMaterialization`: one checksummed frame with the header line, then one with all event rows.
8. **Kimi tar contents.** The spec lists `state.json` + `agents/**`. The session directory can also hold the legacy `session-meta/state.json` and per-session stores, so every regular file in the session directory is packed. Symlinks are skipped.
9. **`read_session` signature.** The spec says `read_session(session_id, run_dir)`. Both CLIs key the session directory by `cwd`, and the home may come from `env`, so the signature follows the pause branch's `locate_session(session_id, cwd, run_dir, env)`.
10. **The pause branch reads `located.file` as bytes.** For Kimi that path is a directory, so `tokio::fs::read` fails, `session_file` is empty, and the run would be pinned to its node. Task 12 hooks `read_session` into both reads, at pause and at the reconnect snapshot.
11. **B has no `run_dir` at launch.** `SpawnConfig` carries no `run_dir`. B must derive `<run_dir>` exactly as `Handoff::run_dir()` does (`working_directory.with_extension("session")`), or the placed session sits where the CLI never looks. Tasks 9 and 11 verify this before wiring.
12. **Test vectors "computed by the CLIs' own code".** The CLI sources were read but not run. The vectors in `tests/fixtures/session_keys.json` come from a line-by-line Python port of `workdir-slug.ts` and `format.ts` (sha256 checked with `sha256sum`). Task 13 cross-checks the real CLIs' directory names on staging.

## File Structure

warp (`T:\projects\dev-ops\warp-kimi-dsh`):
- `crates/managed_agent_sessions/src/lib.rs`: kinds → profiles (`agent_ctrl_profile`, `AGENT_KINDS`), `MoveSession` refusal arm.
- `crates/managed_agent_sessions/src/tests.rs`: kinds tests.
- `crates/warp_cli/src/managed_sessions.rs`: `--agent` help text, `MoveSession` subcommand.
- `crates/warp_cli/src/lib_tests.rs`: parse test.
- `app/Cargo.toml`: `tar = "0.4"` (non-wasm).
- `app/src/terminal/agent_session_handoff.rs`: ids, branch name, roots, locating, Kimi tar, content length, exit command.
- `app/src/terminal/agent_session_handoff_tests.rs`: tests.
- `app/src/agent_ctrl/fleet_model.rs`: shared `STARTABLE_AGENTS`.
- `app/src/agent_ctrl/fleet_model_tests.rs`: test.
- `app/src/agent_ctrl/fleet_view.rs`: uses the shared list; mouse-state array sized from it.
- `app/src/workspace/view.rs`: uses the shared list; cut-over uses `session_content_len`.
- `app/src/agent_ctrl_move_session_cli.rs` (new) and `app/src/agent_ctrl_move_session_cli_tests.rs` (new): `move-session`.
- `app/src/agent_ctrl_fleet_cli.rs`: `MoveSession` arm in `move_all_request`.
- `app/src/lib.rs`: module and dispatch.

agent-dash (`T:\projects\SOTZ3N\stoz3n-agent-dash-kimi-dsh`):
- `server/agent-control.ts`: `SESSION_ID` allows `_`.
- `scripts/check-agent-control.mjs`: test.

agent-ctrl (`T:\projects\SOTZ3N\stoz3n-agent-ctrl-kimi-dsh`):
- `Cargo.toml`, `Cargo.lock`: `tar`, `zstd`.
- `src/agent/process.rs`: `validate_session_id` allows `_`; `CliAdapter::read_session`; `CliBackend` delegation.
- `src/agent/backend.rs`: `AgentBackend::read_session`.
- `src/agent/runner.rs`: `AgentRunner::read_session`.
- `src/remote/controller.rs`: `moved_session` allows `_`.
- `src/agent/kimi_session.rs` (new): Kimi on-disk format — `work_dir_key`, tar pack/unpack, place, read.
- `src/agent/dsh_session.rs` (new): dsh on-disk format — `project_key`, `encode_segment`, generations, zstd rewrite, place, read.
- `src/agent/mod.rs`: `pub mod kimi_session; pub mod dsh_session;`.
- `src/agent/kimi.rs`, `src/agent/dsh.rs` (B's files): two delegating trait methods each.
- `tests/fixtures/session_keys.json` (new): key vectors.
- `tests/fixtures/dsh/session.v4.jsonl` (new): plaintext dsh session fixture.


> **Cross-plan note (2026-10-08):** Plan A already adds `agent_session_history::{kimi_home, dsh_home, kimi_indexed_session_dirs}` (Plan A Task 5). Task 3 here must call those for the Kimi index lookup and the data roots instead of re-implementing them; `AgentSessionRoots.kimi_home`/`dsh_home` are filled from them. Keep only the handoff-specific pieces (`dsh_generation`, packing, size).

---

## Part W — WarpSpeed (`T:\projects\dev-ops\warp-kimi-dsh`)

Before Task 1: `git -C T:/projects/dev-ops/warp-kimi-dsh status` and `git -C T:/projects/dev-ops/warp-kimi-dsh branch --show-current` → `feat/kimi-dsh-agents`. Confirm Plan A is merged in: `grep -n "Kimi\b" app/src/terminal/cli_agent*.rs` (or wherever `pub enum CLIAgent` lives) shows `Kimi` and `DeepSeekHarness`. If A is not in, stop: every warp task needs the variants.

### Task 1: Kimi and dsh are AgentCtrl agent kinds

**Files:**
- Modify: `crates/managed_agent_sessions/src/lib.rs:501-514`
- Modify: `crates/managed_agent_sessions/src/tests.rs:942-984`
- Modify: `crates/warp_cli/src/managed_sessions.rs:26`
- Modify: `app/src/terminal/agent_session_handoff.rs:104-123`
- Modify: `app/src/terminal/agent_session_handoff_tests.rs:46-61`
- Modify: `app/src/agent_ctrl/fleet_model.rs` (new const near the top), `app/src/agent_ctrl/fleet_model_tests.rs`
- Modify: `app/src/agent_ctrl/fleet_view.rs:33-37,98`
- Modify: `app/src/workspace/view.rs:14743-14747`

**Interfaces:**
- Consumes: `CLIAgent::Kimi`, `CLIAgent::DeepSeekHarness` (Plan A).
- Produces:
  - `managed_agent_sessions::agent_ctrl_profile("kimi" | "dsh")`
  - `pub const AGENT_KINDS: [&str; 5] = ["claude", "pi", "codex", "kimi", "dsh"]`
  - `agent_session_handoff::agent_ctrl_profile(CLIAgent::Kimi | CLIAgent::DeepSeekHarness)`
  - `pub(crate) const STARTABLE_AGENTS: [(CLIAgent, &str); 5]` in `crate::agent_ctrl::fleet_model`

- [ ] **Step 1: Write the failing tests**

In `crates/managed_agent_sessions/src/tests.rs`, change the three existing tests to:

```rust
#[test]
fn agent_ctrl_profiles_are_the_interactive_rw_profiles() {
    assert_eq!(
        agent_ctrl_profile("claude"),
        Some("staging-claude-interactive-rw")
    );
    assert_eq!(agent_ctrl_profile("pi"), Some("staging-pi-interactive-rw"));
    assert_eq!(
        agent_ctrl_profile("codex"),
        Some("staging-codex-interactive-rw")
    );
    assert_eq!(
        agent_ctrl_profile("kimi"),
        Some("staging-kimi-interactive-rw")
    );
    assert_eq!(agent_ctrl_profile("dsh"), Some("staging-dsh-interactive-rw"));
    assert_eq!(agent_ctrl_profile("gemini"), None);
    for kind in AGENT_KINDS {
        assert!(agent_ctrl_profile(kind).is_some(), "{kind}");
    }
}

#[test]
fn agent_kinds_resolve_to_their_profiles_and_profile_names_pass_through() {
    let resolve = |agent| resolve_agent_name(ManagedEnvironment::Staging, agent).unwrap();
    assert_eq!(resolve("claude"), "staging-claude-interactive-rw");
    assert_eq!(resolve("pi"), "staging-pi-interactive-rw");
    assert_eq!(resolve("codex"), "staging-codex-interactive-rw");
    assert_eq!(resolve("kimi"), "staging-kimi-interactive-rw");
    assert_eq!(resolve("dsh"), "staging-dsh-interactive-rw");
    assert_eq!(resolve("staging-pi-lander"), "staging-pi-lander");
    assert_eq!(
        resolve("staging-claude-interactive-rw"),
        "staging-claude-interactive-rw"
    );
}
```

In `an_unknown_agent_fails_with_the_accepted_values`, change the assertion to:

```rust
        assert!(
            error.contains("claude, pi, codex, kimi, dsh"),
            "{unknown:?}: {error}"
        );
```

In `app/src/terminal/agent_session_handoff_tests.rs`, add these two lines to `agent_ctrl_profiles` before the `Gemini` assertion:

```rust
    assert_eq!(
        agent_ctrl_profile(CLIAgent::Kimi),
        Some("staging-kimi-interactive-rw")
    );
    assert_eq!(
        agent_ctrl_profile(CLIAgent::DeepSeekHarness),
        Some("staging-dsh-interactive-rw")
    );
```

Append to `app/src/agent_ctrl/fleet_model_tests.rs`:

```rust
#[cfg(feature = "local_fs")]
#[test]
fn every_startable_agent_has_an_agent_ctrl_profile() {
    for (agent, label) in STARTABLE_AGENTS {
        assert!(agent_ctrl_profile(agent).is_some(), "{label}");
    }
    let agents = STARTABLE_AGENTS.map(|(agent, _)| agent);
    assert!(agents.contains(&CLIAgent::Kimi));
    assert!(agents.contains(&CLIAgent::DeepSeekHarness));
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo nextest run -p managed_agent_sessions agent_ctrl_profiles agent_kinds an_unknown_agent`
Expected: FAIL. `agent_ctrl_profile("kimi")` is `None`, and the error lists only `claude, pi, codex`.

Run: `cargo nextest run -p warp --lib agent_ctrl_profiles every_startable_agent`
Expected: compile error, `cannot find value STARTABLE_AGENTS`.

- [ ] **Step 3: Implement**

`crates/managed_agent_sessions/src/lib.rs`:

```rust
/// The AgentCtrl profile that runs moved and fleet sessions of a CLI agent kind.
pub fn agent_ctrl_profile(agent: &str) -> Option<&'static str> {
    match agent {
        "claude" => Some("staging-claude-interactive-rw"),
        "pi" => Some("staging-pi-interactive-rw"),
        "codex" => Some("staging-codex-interactive-rw"),
        "kimi" => Some("staging-kimi-interactive-rw"),
        "dsh" => Some("staging-dsh-interactive-rw"),
        _ => None,
    }
}

/// The CLI agent kinds that [`agent_ctrl_profile`] maps to an AgentCtrl profile.
pub const AGENT_KINDS: [&str; 5] = ["claude", "pi", "codex", "kimi", "dsh"];
```

`crates/warp_cli/src/managed_sessions.rs:26`:

```rust
        /// Agent kind (claude, pi, codex, kimi, dsh) or configured AgentCtrl profile name.
```

`app/src/terminal/agent_session_handoff.rs` `agent_ctrl_profile`: add these two arms and remove `CLIAgent::Kimi` and `CLIAgent::DeepSeekHarness` from the `=> None` arm:

```rust
        CLIAgent::Kimi => managed_agent_sessions::agent_ctrl_profile("kimi"),
        CLIAgent::DeepSeekHarness => managed_agent_sessions::agent_ctrl_profile("dsh"),
```

`app/src/agent_ctrl/fleet_model.rs`, after the imports:

```rust
/// The agents a fleet or pane start offers, with their button labels.
pub(crate) const STARTABLE_AGENTS: [(CLIAgent, &str); 5] = [
    (CLIAgent::Claude, "Claude"),
    (CLIAgent::Pi, "Pi"),
    (CLIAgent::Codex, "Codex"),
    (CLIAgent::Kimi, "Kimi"),
    (CLIAgent::DeepSeekHarness, "DeepSeek"),
];
```

`app/src/agent_ctrl/fleet_view.rs`:
- Delete the local `const STARTABLE_AGENTS` (:33-37).
- Add `use crate::agent_ctrl::fleet_model::STARTABLE_AGENTS;` to the imports.
- Change the field (:98) to:

```rust
    repo_mouse_states: HashMap<String, [MouseStateHandle; STARTABLE_AGENTS.len()]>,
```

`app/src/workspace/view.rs` (~:14743): delete the local `const STARTABLE_AGENTS` and put this in its place (the function is cfg-guarded, so the import stays local):

```rust
        use crate::agent_ctrl::fleet_model::STARTABLE_AGENTS;
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo nextest run -p managed_agent_sessions agent_ctrl_profiles agent_kinds an_unknown_agent` → PASS.
Run: `cargo nextest run -p warp --lib agent_ctrl_profiles every_startable_agent fleet_view` → PASS.
Run: `cargo clippy -p warp -p managed_agent_sessions -p warp_cli --all-targets --tests -- -D warnings` → no warnings.

- [ ] **Step 5: Commit**

```bash
git -C T:/projects/dev-ops/warp-kimi-dsh add crates/managed_agent_sessions/src/lib.rs crates/managed_agent_sessions/src/tests.rs crates/warp_cli/src/managed_sessions.rs app/src/terminal/agent_session_handoff.rs app/src/terminal/agent_session_handoff_tests.rs app/src/agent_ctrl/fleet_model.rs app/src/agent_ctrl/fleet_model_tests.rs app/src/agent_ctrl/fleet_view.rs app/src/workspace/view.rs
git -C T:/projects/dev-ops/warp-kimi-dsh commit -m "feat(agent-ctrl): kimi and dsh agent kinds and start buttons

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

### Task 2: Kimi session ids in the handoff

**Files:**
- Modify: `app/src/terminal/agent_session_handoff.rs:136-142` (`is_valid_session_id`), `:324-330` (`handoff_branch_name`)
- Test: `app/src/terminal/agent_session_handoff_tests.rs`

**Interfaces:**
- Produces:
  - `is_valid_session_id` accepts `[A-Za-z0-9_-]{1,128}`.
  - `handoff_branch_name(now, "session_<uuid>")` → `handoff/<yyyymmdd-hhmm>-<first 8 of uuid>`.

- [ ] **Step 1: Write the failing tests**

Append to `agent_session_handoff_tests.rs`:

```rust
const KIMI_SESSION_ID: &str = "session_0b6c1234-ffff-4a4a-8b8b-0123456789ab";

#[test]
fn kimi_and_dsh_session_ids_are_valid_but_path_characters_are_not() {
    for id in [
        KIMI_SESSION_ID,
        "session-0b6c1234-ffff-4a4a-8b8b-0123456789ab",
        SESSION_ID,
    ] {
        assert!(is_valid_session_id(id), "{id}");
    }
    for id in ["", "a.b", "../x", r"a\b", "a/b", &"a".repeat(129)] {
        assert!(!is_valid_session_id(id), "{id}");
    }
}

#[test]
fn handoff_branch_name_skips_the_session_prefix() {
    let now = Local.with_ymd_and_hms(2026, 10, 3, 12, 0, 0).unwrap();
    assert_eq!(
        handoff_branch_name(now, KIMI_SESSION_ID),
        "handoff/20261003-1200-0b6c1234"
    );
    assert_eq!(
        handoff_branch_name(now, "session-0b6c1234-ffff-4a4a-8b8b-0123456789ab"),
        "handoff/20261003-1200-0b6c1234"
    );
    assert_eq!(
        handoff_branch_name(now, "session_"),
        "handoff/20261003-1200-session_"
    );
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo nextest run -p warp --lib kimi_and_dsh_session_ids handoff_branch_name_skips`
Expected: FAIL. `KIMI_SESSION_ID` is invalid, and the branch is `handoff/20261003-1200-session_`.

- [ ] **Step 3: Implement**

```rust
fn is_valid_session_id(session_id: &str) -> bool {
    (1..=MAX_SESSION_ID_CHARS).contains(&session_id.len())
        && session_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
}
```

```rust
pub(crate) fn handoff_branch_name(now: DateTime<Local>, session_id: &str) -> String {
    // Kimi and dsh ids start with a fixed `session_`/`session-`, which would name every branch alike.
    let distinct = ["session_", "session-"]
        .into_iter()
        .find_map(|prefix| session_id.strip_prefix(prefix))
        .filter(|rest| !rest.is_empty())
        .unwrap_or(session_id);
    let short_id = distinct
        .char_indices()
        .nth(BRANCH_ID_CHARS)
        .map_or(distinct, |(end, _)| &distinct[..end]);
    format!("handoff/{}-{short_id}", now.format("%Y%m%d-%H%M"))
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo nextest run -p warp --lib agent_session_handoff`
Expected: PASS, including the existing `handoff_branch_name_shape` and `session_file_path_rejects_unsafe_ids_and_unsupported_agents`.

- [ ] **Step 5: Commit**

```bash
git -C T:/projects/dev-ops/warp-kimi-dsh add app/src/terminal/agent_session_handoff.rs app/src/terminal/agent_session_handoff_tests.rs
git -C T:/projects/dev-ops/warp-kimi-dsh commit -m "feat(handoff): accept kimi session ids and keep their branch names distinct

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

### Task 3: Locate Kimi and dsh sessions

**Files:**
- Modify: `app/src/terminal/agent_session_handoff.rs`:
  - `AgentSessionRoots` (:65-99)
  - `session_file_path` (:176-234)
  - `agent_session_root` (:237-262)
  - `is_reported_session_file` (:267-282)
  - new helpers `kimi_session_dir`, `dsh_session_file`, `dsh_generation`, `dsh_current_generation`
- Test: `app/src/terminal/agent_session_handoff_tests.rs`

**Interfaces:**
- Consumes: `transcript_path` semantics from Plan A (Kimi: directory; dsh: file).
- Produces:
  - `AgentSessionRoots { …, pub kimi_home: PathBuf, pub dsh_home: PathBuf }`
  - `session_file_path(CLIAgent::Kimi, …)` → the session **directory**.
  - `session_file_path(CLIAgent::DeepSeekHarness, …)` → the current generation file.
  - `pub(crate) fn dsh_generation(name: &str) -> Option<(u64, bool)>`, giving (version, zstd).

- [ ] **Step 1: Write the failing tests**

Append to `agent_session_handoff_tests.rs`:

```rust
fn kimi_session(roots: &AgentSessionRoots, workspace: &str, id: &str) -> PathBuf {
    let dir = roots.kimi_home.join("sessions").join(workspace).join(id);
    write_file(&dir.join("state.json"), br#"{"id":"x","cwd":"/old"}"#);
    write_file(&dir.join("agents/main/wire.jsonl"), b"{}\n");
    dir
}

#[test]
fn kimi_session_dir_comes_from_the_last_index_line_for_the_id() {
    let home = TempDir::new().unwrap();
    let roots = AgentSessionRoots::under_home(home.path());
    let stale = kimi_session(&roots, "wd_app_aaaaaaaaaaaa", KIMI_SESSION_ID);
    let current = kimi_session(&roots, "wd_app_bbbbbbbbbbbb", KIMI_SESSION_ID);
    let line = |dir: &Path| {
        serde_json::json!({"sessionId": KIMI_SESSION_ID, "sessionDir": dir, "workDir": "/app"})
            .to_string()
    };
    write_file(
        &roots.kimi_home.join("session_index.jsonl"),
        format!(
            "{}\n{{\"sessionId\":\"other\",\"sessionDir\":\"/x\",\"workDir\":\"/x\"}}\n{}\n",
            line(&stale),
            line(&current)
        )
        .as_bytes(),
    );
    let found =
        session_file_path(CLIAgent::Kimi, KIMI_SESSION_ID, Path::new("/app"), &roots).unwrap();
    assert_eq!(found, current);
}

#[test]
fn kimi_session_dir_is_scanned_for_without_an_index_or_with_an_outside_entry() {
    let home = TempDir::new().unwrap();
    let roots = AgentSessionRoots::under_home(home.path());
    let dir = kimi_session(&roots, "wd_app_aaaaaaaaaaaa", KIMI_SESSION_ID);
    assert_eq!(
        session_file_path(CLIAgent::Kimi, KIMI_SESSION_ID, Path::new("/app"), &roots).unwrap(),
        dir
    );

    let outside = home.path().join("elsewhere").join(KIMI_SESSION_ID);
    fs::create_dir_all(&outside).unwrap();
    write_file(
        &roots.kimi_home.join("session_index.jsonl"),
        serde_json::json!({"sessionId": KIMI_SESSION_ID, "sessionDir": outside, "workDir": "/app"})
            .to_string()
            .as_bytes(),
    );
    assert_eq!(
        session_file_path(CLIAgent::Kimi, KIMI_SESSION_ID, Path::new("/app"), &roots).unwrap(),
        dir
    );
    assert!(session_file_path(CLIAgent::Kimi, "session_missing", Path::new("/app"), &roots).is_err());
}

#[test]
fn dsh_generation_names_follow_the_canonical_pattern() {
    assert_eq!(dsh_generation("session.jsonl"), Some((0, false)));
    assert_eq!(dsh_generation("session.jsonl.zstd"), Some((0, true)));
    assert_eq!(dsh_generation("session.v4.jsonl.zstd"), Some((4, true)));
    assert_eq!(dsh_generation("session.v12.jsonl"), Some((12, false)));
    for name in [
        "session.v0.jsonl",
        "session.v04.jsonl.zstd",
        "session.V4.jsonl",
        "session.v4.jsonl.zst",
        "session.v4.jsonl.tmp",
        "session.v.jsonl",
        "other.jsonl",
    ] {
        assert_eq!(dsh_generation(name), None, "{name}");
    }
}

#[test]
fn dsh_session_file_is_the_highest_generation_under_any_project() {
    let home = TempDir::new().unwrap();
    let roots = AgentSessionRoots::under_home(home.path());
    let dir = roots
        .dsh_home
        .join("sessions/--C-Users-budel-src-app--")
        .join(SESSION_ID);
    for name in [
        "session.jsonl",
        "session.v3.jsonl.zstd",
        "session.v4.jsonl.zstd",
        "session.v04.jsonl.zstd",
        "session.v9.jsonl.tmp",
    ] {
        write_file(&dir.join(name), b"x");
    }
    let found =
        session_file_path(CLIAgent::DeepSeekHarness, SESSION_ID, Path::new("/x"), &roots).unwrap();
    assert_eq!(found, dir.join("session.v4.jsonl.zstd"));
    assert!(
        session_file_path(CLIAgent::DeepSeekHarness, "missing", Path::new("/x"), &roots).is_err()
    );
}

#[test]
fn reported_kimi_dir_and_dsh_file_are_trusted_only_under_their_roots() {
    let home = TempDir::new().unwrap();
    let roots = AgentSessionRoots::under_home(home.path());
    let kimi_dir = kimi_session(&roots, "wd_app_aaaaaaaaaaaa", KIMI_SESSION_ID);
    let kimi_source = |transcript_path: PathBuf| HandoffSource {
        agent: CLIAgent::Kimi,
        session_id: KIMI_SESSION_ID.to_owned(),
        cwd: PathBuf::from("/app"),
        transcript_path: Some(transcript_path),
    };
    assert!(is_reported_session_file(&kimi_dir, &kimi_source(kimi_dir.clone()), &roots));
    let other = kimi_session(&roots, "wd_app_aaaaaaaaaaaa", "session_other");
    assert!(!is_reported_session_file(&other, &kimi_source(other.clone()), &roots));
    let escaped = kimi_dir.join("..").join(KIMI_SESSION_ID);
    assert!(!is_reported_session_file(&escaped, &kimi_source(escaped.clone()), &roots));

    let dsh_file = roots
        .dsh_home
        .join("sessions/--app--")
        .join(SESSION_ID)
        .join("session.v4.jsonl.zstd");
    write_file(&dsh_file, b"x");
    let dsh_source = HandoffSource {
        agent: CLIAgent::DeepSeekHarness,
        session_id: SESSION_ID.to_owned(),
        cwd: PathBuf::from("/app"),
        transcript_path: Some(dsh_file.clone()),
    };
    assert!(is_reported_session_file(&dsh_file, &dsh_source, &roots));
    let outside = home.path().join(SESSION_ID).join("session.v4.jsonl.zstd");
    write_file(&outside, b"x");
    assert!(!is_reported_session_file(&outside, &dsh_source, &roots));
}

#[test]
fn session_roots_default_under_home() {
    let roots = AgentSessionRoots::under_home(Path::new("/home/ben"));
    assert_eq!(roots.kimi_home, Path::new("/home/ben/.kimi-code"));
    assert_eq!(roots.dsh_home, Path::new("/home/ben/.dsh"));
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo nextest run -p warp --lib kimi_session_dir dsh_generation dsh_session_file reported_kimi session_roots_default`
Expected: compile errors (`no field kimi_home`, `cannot find function dsh_generation`).

- [ ] **Step 3: Implement**

`AgentSessionRoots`: add the fields after `codex_home`:

```rust
    /// `KIMI_CODE_HOME`, by default `~/.kimi-code`.
    pub kimi_home: PathBuf,
    /// `DSH_HOME`, by default `~/.dsh`.
    pub dsh_home: PathBuf,
```

In `under_home` add `kimi_home: home.join(".kimi-code"), dsh_home: home.join(".dsh"),`. In `from_env` add:

```rust
            kimi_home: env_dir("KIMI_CODE_HOME").unwrap_or(defaults.kimi_home),
            dsh_home: env_dir("DSH_HOME").unwrap_or(defaults.dsh_home),
```

Add `use serde_json::Value;` to the imports. Then add these helpers after `file_under_ending_with`:

```rust
fn has_parent_dir(path: &Path) -> bool {
    path.components()
        .any(|component| matches!(component, Component::ParentDir))
}

fn is_named(path: &Path, name: &str) -> bool {
    path.file_name().and_then(OsStr::to_str) == Some(name)
}

/// Kimi's directory for session `session_id`: the last `session_index.jsonl` entry for it, else
/// the workspace directory under `sessions/` that holds it.
fn kimi_session_dir(kimi_home: &Path, session_id: &str) -> Option<PathBuf> {
    let sessions = kimi_home.join("sessions");
    let is_session_dir = |dir: &Path| {
        !has_parent_dir(dir) && dir.starts_with(&sessions) && is_named(dir, session_id) && dir.is_dir()
    };
    let indexed = fs::read_to_string(kimi_home.join("session_index.jsonl"))
        .ok()
        .and_then(|index| {
            index
                .lines()
                .filter_map(|line| serde_json::from_str::<Value>(line.trim()).ok())
                .filter(|entry| entry.get("sessionId").and_then(Value::as_str) == Some(session_id))
                .filter_map(|entry| {
                    entry
                        .get("sessionDir")
                        .and_then(Value::as_str)
                        .map(PathBuf::from)
                })
                .last()
        })
        .filter(|dir| is_session_dir(dir));
    indexed.or_else(|| {
        fs::read_dir(&sessions)
            .ok()?
            .flatten()
            .map(|workspace| workspace.path().join(session_id))
            .find(|dir| dir.is_dir())
    })
}

/// The format version and compression of a canonical dsh generation file name,
/// `session[.v<N>].jsonl[.zstd]` with N ≥ 1 and no leading zero.
pub(crate) fn dsh_generation(name: &str) -> Option<(u64, bool)> {
    let (stem, zstd) = match name.strip_suffix(".zstd") {
        Some(stem) => (stem, true),
        None => (name, false),
    };
    let version = stem.strip_prefix("session")?.strip_suffix(".jsonl")?;
    if version.is_empty() {
        return Some((0, zstd));
    }
    let digits = version.strip_prefix(".v")?;
    let canonical = !digits.is_empty()
        && !digits.starts_with('0')
        && digits.bytes().all(|byte| byte.is_ascii_digit());
    canonical
        .then(|| digits.parse().ok())
        .flatten()
        .map(|version| (version, zstd))
}

/// The highest generation in a dsh session directory; dsh reads only that one.
fn dsh_current_generation(dir: &Path) -> Option<PathBuf> {
    fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .filter_map(|entry| {
            let generation = dsh_generation(entry.file_name().to_str()?)?;
            Some((generation, entry.path()))
        })
        .max_by_key(|(generation, _)| *generation)
        .map(|(_, path)| path)
}

/// dsh keeps a session at `sessions/<project key>/<id>/`, and the project key is lossy, so the
/// directory is found by id under any project.
fn dsh_session_file(dsh_home: &Path, session_id: &str) -> Option<PathBuf> {
    fs::read_dir(dsh_home.join("sessions"))
        .ok()?
        .flatten()
        .map(|project| project.path().join(session_id))
        .filter(|dir| dir.is_dir())
        .find_map(|dir| dsh_current_generation(&dir))
}
```

`session_file_path`: add these arms and remove `Kimi`/`DeepSeekHarness` from the `bail!` arm:

```rust
        CLIAgent::Kimi => kimi_session_dir(&roots.kimi_home, session_id).ok_or_else(|| {
            anyhow!(
                "no kimi session {session_id} under {}",
                roots.kimi_home.join("sessions").display()
            )
        }),
        CLIAgent::DeepSeekHarness => dsh_session_file(&roots.dsh_home, session_id).ok_or_else(|| {
            anyhow!(
                "no dsh session file for {session_id} under {}",
                roots.dsh_home.join("sessions").display()
            )
        }),
```

`agent_session_root`: add these arms and remove the two variants from the `None` arm:

```rust
        CLIAgent::Kimi => Some(roots.kimi_home.join("sessions")),
        CLIAgent::DeepSeekHarness => Some(roots.dsh_home.join("sessions")),
```

Replace `is_reported_session_file` with:

```rust
/// Whether `path`, as reported by the agent's hooks, is this session's file (Kimi: its
/// directory). The path arrives through terminal output or the agent-event channel, so it is
/// trusted only for an entry named after the session under the agent's own session directory;
/// anything else could make the move upload unrelated data.
fn is_reported_session_file(
    path: &Path,
    source: &HandoffSource,
    roots: &AgentSessionRoots,
) -> bool {
    let id = source.session_id.as_str();
    let named_for_session = match source.agent {
        CLIAgent::Kimi => is_named(path, id) && path.is_dir(),
        CLIAgent::DeepSeekHarness => {
            path.parent().is_some_and(|dir| is_named(dir, id))
                && path
                    .file_name()
                    .and_then(OsStr::to_str)
                    .is_some_and(|name| dsh_generation(name).is_some())
                && path.is_file()
        }
        CLIAgent::Claude
        | CLIAgent::Pi
        | CLIAgent::Codex
        | CLIAgent::Gemini
        | CLIAgent::Amp
        | CLIAgent::Droid
        | CLIAgent::OpenCode
        | CLIAgent::Copilot
        | CLIAgent::OhMyPi
        | CLIAgent::Auggie
        | CLIAgent::CursorCli
        | CLIAgent::Goose
        | CLIAgent::Hermes
        | CLIAgent::Vibe
        | CLIAgent::Antigravity
        | CLIAgent::WarpTui
        | CLIAgent::Unknown => {
            path.file_name()
                .and_then(OsStr::to_str)
                .is_some_and(|name| name.ends_with(&format!("{id}.jsonl")))
                && path.is_file()
        }
    };
    is_valid_session_id(id)
        && !has_parent_dir(path)
        && agent_session_root(source.agent, roots).is_some_and(|root| path.starts_with(root))
        && named_for_session
}
```

If Plan A added more `CLIAgent` variants than those listed, add them to the last arm. The compiler names any that are missing.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo nextest run -p warp --lib agent_session_handoff`
Expected: PASS, including the existing `move_session_file_prefers_the_reported_transcript_path` and `move_session_file_ignores_a_reported_path_that_is_not_this_session`.

- [ ] **Step 5: Commit**

```bash
git -C T:/projects/dev-ops/warp-kimi-dsh add app/src/terminal/agent_session_handoff.rs app/src/terminal/agent_session_handoff_tests.rs
git -C T:/projects/dev-ops/warp-kimi-dsh commit -m "feat(handoff): locate kimi session directories and dsh session files

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

### Task 4: Pack a Kimi session, measure sessions for cut-over, exit Kimi

**Files:**
- Modify: `app/Cargo.toml` (`[target.'cfg(not(target_family = "wasm"))'.dependencies]`, :311): add `tar = "0.4"` (the version `crates/node_runtime` already uses; `Cargo.lock` has 0.4.46).
- Modify: `app/src/terminal/agent_session_handoff.rs`:
  - `PreparedHandoff` (:37-45)
  - `read_session_file` (:381-397)
  - `prepare_handoff_with_roots` (:410-447)
  - `agent_exit_command` (:637-657)
  - new `SessionPayload`, `pack_kimi_session`, `read_session_payload`, `session_content_len`
- Modify: `app/src/workspace/view.rs:398-404` (import), `:14218-14223` (`file_len`), `:14403-14405` (current length)
- Test: `app/src/terminal/agent_session_handoff_tests.rs`

**Interfaces:**
- Consumes: `session_file_path`/`resolve_session_file` from Task 3.
- Produces:
  - `pub(crate) fn session_content_len(path: &Path) -> Option<u64>`
  - `PreparedHandoff::session_len: u64`: the bytes of session data uploaded, comparable with `session_content_len`.
  - `agent_exit_command(CLIAgent::Kimi) == Some("/exit")`
  - `agent_exit_command(CLIAgent::DeepSeekHarness) == None`

- [ ] **Step 1: Write the failing tests**

Append to `agent_session_handoff_tests.rs`:

```rust
fn tar_entries(archive: &[u8]) -> Vec<(String, Vec<u8>)> {
    use std::io::Read as _;
    tar::Archive::new(archive)
        .entries()
        .unwrap()
        .map(|entry| {
            let mut entry = entry.unwrap();
            assert_eq!(entry.header().entry_type(), tar::EntryType::Regular);
            let path = entry.path().unwrap().to_string_lossy().replace('\\', "/");
            let mut content = Vec::new();
            entry.read_to_end(&mut content).unwrap();
            (path, content)
        })
        .collect()
}

#[test]
fn kimi_session_is_packed_as_a_tar_of_every_file_under_its_id() {
    let home = TempDir::new().unwrap();
    let roots = AgentSessionRoots::under_home(home.path());
    let dir = kimi_session(&roots, "wd_app_aaaaaaaaaaaa", KIMI_SESSION_ID);
    write_file(&dir.join("agents/sub-1/wire.jsonl"), b"{\"a\":1}\n");
    write_file(&dir.join("session-meta/state.json"), b"{}");

    let payload = pack_kimi_session(&dir, KIMI_SESSION_ID).unwrap();

    let entries = tar_entries(&payload.bytes);
    let names: Vec<_> = entries.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(
        names,
        [
            format!("{KIMI_SESSION_ID}/agents/main/wire.jsonl"),
            format!("{KIMI_SESSION_ID}/agents/sub-1/wire.jsonl"),
            format!("{KIMI_SESSION_ID}/session-meta/state.json"),
            format!("{KIMI_SESSION_ID}/state.json"),
        ]
    );
    assert_eq!(entries[1].1, b"{\"a\":1}\n");
    let total: u64 = entries.iter().map(|(_, content)| content.len() as u64).sum();
    assert_eq!(payload.content_len, total);
    assert_eq!(session_content_len(&dir), Some(total));
}

#[test]
fn kimi_session_without_state_or_over_the_limit_is_refused() {
    let home = TempDir::new().unwrap();
    let bare = home.path().join(KIMI_SESSION_ID);
    write_file(&bare.join("agents/main/wire.jsonl"), b"{}\n");
    let error = pack_kimi_session(&bare, KIMI_SESSION_ID).unwrap_err();
    assert!(format!("{error:#}").contains("state.json"), "{error:#}");

    let roots = AgentSessionRoots::under_home(home.path());
    let dir = kimi_session(&roots, "wd_app_aaaaaaaaaaaa", KIMI_SESSION_ID);
    fs::File::create(dir.join("agents/main/big.bin"))
        .unwrap()
        .set_len(MAX_SESSION_FILE_BYTES as u64 + 1)
        .unwrap();
    let error = pack_kimi_session(&dir, KIMI_SESSION_ID).unwrap_err();
    assert!(format!("{error:#}").contains("too large"), "{error:#}");
}

#[test]
fn session_content_len_follows_appends_to_a_file_or_a_directory() {
    let home = TempDir::new().unwrap();
    let file = home.path().join("session.jsonl");
    write_file(&file, b"12345");
    assert_eq!(session_content_len(&file), Some(5));
    let dir = home.path().join("dir");
    write_file(&dir.join("a"), b"12");
    write_file(&dir.join("b/c"), b"345");
    assert_eq!(session_content_len(&dir), Some(5));
    write_file(&dir.join("b/c"), b"3456");
    assert_eq!(session_content_len(&dir), Some(6));
    assert_eq!(session_content_len(&home.path().join("missing")), None);
}

#[test]
fn prepare_handoff_uploads_a_kimi_tar_and_a_dsh_file_as_is() {
    let home = TempDir::new().unwrap();
    let remotes = TempDir::new().unwrap();
    let repo = TempDir::new().unwrap();
    let repo = repo.path();
    init_repo(repo);
    add_bare_remote(repo, "gitea", remotes.path());
    let roots = AgentSessionRoots::under_home(home.path());
    let now = Local.with_ymd_and_hms(2026, 10, 3, 12, 0, 0).unwrap();

    let kimi_dir = kimi_session(&roots, "wd_app_aaaaaaaaaaaa", KIMI_SESSION_ID);
    let kimi = HandoffSource {
        agent: CLIAgent::Kimi,
        session_id: KIMI_SESSION_ID.to_owned(),
        cwd: repo.to_path_buf(),
        transcript_path: None,
    };
    let prepared = prepare_handoff_with_roots(&kimi, &roots, now).unwrap();
    assert_eq!(prepared.agent_name, "staging-kimi-interactive-rw");
    assert_eq!(prepared.session_file_path, kimi_dir);
    assert_eq!(prepared.session_len, session_content_len(&kimi_dir).unwrap());
    assert!(tar_entries(&prepared.session_file)
        .iter()
        .any(|(name, _)| *name == format!("{KIMI_SESSION_ID}/state.json")));
    assert_eq!(prepared.branch, "handoff/20261003-1200-0b6c1234");

    let dsh_file = roots
        .dsh_home
        .join("sessions/--app--")
        .join(SESSION_ID)
        .join("session.v4.jsonl.zstd");
    write_file(&dsh_file, b"\x28\xb5\x2f\xfdrest");
    let dsh = HandoffSource {
        agent: CLIAgent::DeepSeekHarness,
        session_id: SESSION_ID.to_owned(),
        cwd: repo.to_path_buf(),
        transcript_path: None,
    };
    let prepared = prepare_handoff_with_roots(&dsh, &roots, now).unwrap();
    assert_eq!(prepared.agent_name, "staging-dsh-interactive-rw");
    assert_eq!(prepared.session_file, b"\x28\xb5\x2f\xfdrest");
    assert_eq!(prepared.session_len, 8);
}
```

Replace the body of `move_session_exit_command_matches_each_agent` with:

```rust
    assert_eq!(agent_exit_command(CLIAgent::Claude), Some("/exit"));
    assert_eq!(agent_exit_command(CLIAgent::Codex), Some("/exit"));
    assert_eq!(agent_exit_command(CLIAgent::Kimi), Some("/exit"));
    assert_eq!(agent_exit_command(CLIAgent::Pi), Some("/quit"));
    assert_eq!(agent_exit_command(CLIAgent::DeepSeekHarness), None);
    assert_eq!(agent_exit_command(CLIAgent::Gemini), None);
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo nextest run -p warp --lib kimi_session_is_packed kimi_session_without session_content_len prepare_handoff_uploads move_session_exit_command`
Expected: compile errors (`cannot find function pack_kimi_session`, `no field session_len`).

- [ ] **Step 3: Implement**

`app/Cargo.toml`, in the non-wasm dependency table, keeping alphabetical order:

```toml
tar = "0.4"
```

`PreparedHandoff`: add after `session_file_path`:

```rust
    /// [`session_content_len`] of `session_file_path` as uploaded.
    pub session_len: u64,
```

Add after `read_session_file`:

```rust
/// The bytes a move uploads for a session, and the [`session_content_len`] they were read at.
struct SessionPayload {
    bytes: Vec<u8>,
    content_len: u64,
}

/// The combined size of the regular files at `path`: the file itself, or every file under a
/// session directory. Agents only ever append, so growth means the session moved on. `None` when
/// it cannot be read.
pub(crate) fn session_content_len(path: &Path) -> Option<u64> {
    let metadata = fs::metadata(path).ok()?;
    if !metadata.is_dir() {
        return Some(metadata.len());
    }
    let mut total = 0;
    for entry in walkdir::WalkDir::new(path) {
        let entry = entry.ok()?;
        if entry.file_type().is_file() {
            total += entry.metadata().ok()?.len();
        }
    }
    Some(total)
}

/// Packs Kimi session directory `dir` as a tar of `<session_id>/<relative path>` regular files,
/// the layout the AgentCtrl node unpacks. Symlinks are skipped.
fn pack_kimi_session(dir: &Path, session_id: &str) -> Result<SessionPayload> {
    let too_large = |len: u64| {
        anyhow!(
            "session {} is too large ({len} bytes, limit {MAX_SESSION_FILE_BYTES})",
            dir.display()
        )
    };
    let mut files = Vec::new();
    for entry in walkdir::WalkDir::new(dir).sort_by_file_name() {
        let entry = entry.with_context(|| format!("failed to read {}", dir.display()))?;
        if entry.file_type().is_file() {
            let len = entry
                .metadata()
                .with_context(|| format!("failed to stat {}", entry.path().display()))?
                .len();
            files.push((entry.into_path(), len));
        }
    }
    ensure!(
        files.iter().any(|(path, _)| *path == dir.join("state.json")),
        "{} has no state.json",
        dir.display()
    );
    let listed: u64 = files.iter().map(|(_, len)| len).sum();
    if listed > MAX_SESSION_FILE_BYTES as u64 {
        return Err(too_large(listed));
    }
    let mut archive = tar::Builder::new(Vec::new());
    let mut content_len = 0;
    for (path, _) in &files {
        let content = fs::read(path).with_context(|| format!("failed to read {}", path.display()))?;
        content_len += content.len() as u64;
        if content_len > MAX_SESSION_FILE_BYTES as u64 {
            return Err(too_large(content_len));
        }
        let name = std::iter::once(session_id.to_owned())
            .chain(
                path.strip_prefix(dir)?
                    .components()
                    .map(|component| component.as_os_str().to_string_lossy().into_owned()),
            )
            .collect::<Vec<_>>()
            .join("/");
        let mut header = tar::Header::new_gnu();
        header.set_size(content.len() as u64);
        header.set_mode(0o644);
        header.set_entry_type(tar::EntryType::Regular);
        archive
            .append_data(&mut header, &name, content.as_slice())
            .with_context(|| format!("failed to pack {}", path.display()))?;
    }
    let bytes = archive.into_inner().context("failed to finish the session archive")?;
    if bytes.len() > MAX_SESSION_FILE_BYTES {
        return Err(too_large(bytes.len() as u64));
    }
    Ok(SessionPayload { bytes, content_len })
}

/// What a move uploads for the session at `path`: a Kimi session directory as a tar, any other
/// agent's session file as is.
fn read_session_payload(source: &HandoffSource, path: &Path) -> Result<SessionPayload> {
    if path.is_dir() {
        return pack_kimi_session(path, &source.session_id);
    }
    let bytes = read_session_file(path)?;
    Ok(SessionPayload {
        content_len: bytes.len() as u64,
        bytes,
    })
}
```

In `prepare_handoff_with_roots`, replace `let session_file = read_session_file(&path)?;` with:

```rust
    let payload = read_session_payload(source, &path)?;
```

Then build the struct with `session_file: payload.bytes, session_len: payload.content_len,`.

`agent_exit_command`: change the first arm to `CLIAgent::Claude | CLIAgent::Codex | CLIAgent::Kimi => Some("/exit"),`. Remove `Kimi` from the `None` arm and keep `DeepSeekHarness` there.

`app/src/workspace/view.rs`:
- Add `session_content_len` to the `use crate::terminal::agent_session_handoff::{…}` list (:398-404).
- In `import_moved_session` set `file_len: prepared.session_len,`.
- In `cut_over_to_agent_ctrl` replace the `std::fs::metadata(&uploaded.file_path)…` expression with:

```rust
        let current_file_len = session_content_len(&uploaded.file_path);
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo nextest run -p warp --lib agent_session_handoff` → PASS.
Run: `cargo check -p warp` → compiles. `UploadedSession.file_len` is still `u64`.
Run: `cargo clippy -p warp --all-targets --tests -- -D warnings` → clean.

- [ ] **Step 5: Commit**

```bash
git -C T:/projects/dev-ops/warp-kimi-dsh add app/Cargo.toml Cargo.lock app/src/terminal/agent_session_handoff.rs app/src/terminal/agent_session_handoff_tests.rs app/src/workspace/view.rs
git -C T:/projects/dev-ops/warp-kimi-dsh commit -m "feat(handoff): upload kimi sessions as a tar and compare session sizes at cut-over

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

### Task 5: `agent-ctrl move-session` for a finished dsh session (needs user approval — spec gap 4)

**Files:**
- Modify: `crates/warp_cli/src/managed_sessions.rs` (new `MoveSession` variant after `MoveAll`)
- Modify: `crates/warp_cli/src/lib_tests.rs` (parse test)
- Modify: `crates/managed_agent_sessions/src/lib.rs:329` (refusal arm)
- Modify: `app/src/agent_ctrl_fleet_cli.rs:174-185` (`move_all_request` arm)
- Create: `app/src/agent_ctrl_move_session_cli.rs`, `app/src/agent_ctrl_move_session_cli_tests.rs`
- Modify: `app/src/lib.rs:4-5` (module), `:868-872` (dispatch)

**Interfaces:**
- Consumes:
  - `agent_session_history::find_all(CLIAgent::DeepSeekHarness)` (Plan A)
  - `prepare_handoff`, `move_idempotency_key` (agent_session_handoff)
  - `managed_agent_sessions::{import_moved_session, MovedSessionImport, wait_until_resumed, stop_run}`
- Produces:
  - `ManagedSessionsCommand::MoveSession { agent: String, session: String, environment: ManagedEnvironment }`
  - `warpspeed agent-ctrl move-session --agent dsh --session <id>`

- [ ] **Step 1: Write the failing tests**

`crates/warp_cli/src/lib_tests.rs`, append:

```rust
#[test]
fn managed_agent_move_session_parses() {
    let command = agent_ctrl_command(&["move-session", "--agent", "dsh", "--session", "0b6c"]);
    let managed_sessions::ManagedSessionsCommand::MoveSession { agent, session, .. } = command
    else {
        panic!("expected move-session, got {command:?}");
    };
    assert_eq!(agent, "dsh");
    assert_eq!(session, "0b6c");
    assert!(Args::try_parse_from(["warpspeed", "agent-ctrl", "move-session", "--agent", "dsh"]).is_err());
}
```

Create `app/src/agent_ctrl_move_session_cli_tests.rs`:

```rust
use std::path::PathBuf;
use std::time::SystemTime;

use super::*;

fn past(agent: CLIAgent, session_id: &str, cwd: &str) -> PastAgentSession {
    PastAgentSession {
        agent,
        session_id: session_id.to_owned(),
        cwd: PathBuf::from(cwd),
        canonical_cwd: None,
        title: None,
        modified: SystemTime::UNIX_EPOCH,
    }
}

#[test]
fn only_dsh_sessions_are_moved_without_a_pane() {
    assert_eq!(paneless_agent("dsh"), Some(CLIAgent::DeepSeekHarness));
    for label in ["kimi", "claude", "pi", "codex", ""] {
        assert_eq!(paneless_agent(label), None, "{label}");
    }
}

#[test]
fn the_recorded_session_supplies_the_directory() {
    let history = [
        past(CLIAgent::DeepSeekHarness, "other", "/x"),
        past(CLIAgent::DeepSeekHarness, "0b6c", r"T:\src\app"),
    ];
    let source = recorded_source(CLIAgent::DeepSeekHarness, "0b6c", &history).unwrap();
    assert_eq!(source.cwd, PathBuf::from(r"T:\src\app"));
    assert_eq!(source.session_id, "0b6c");
    assert_eq!(source.transcript_path, None);

    let error = recorded_source(CLIAgent::DeepSeekHarness, "missing", &history).unwrap_err();
    assert!(error.to_string().contains("missing"), "{error}");
}

#[test]
fn move_session_is_the_only_command_this_module_runs() {
    use clap::Parser as _;
    let parse = |args: &[&str]| {
        let args = warp_cli::Args::try_parse_from(
            ["warpspeed", "agent-ctrl"].iter().chain(args).copied(),
        )
        .unwrap();
        let Some(warp_cli::Command::AgentCtrl(managed)) = args.command else {
            panic!("expected agent-ctrl");
        };
        managed
    };
    let request = move_session_request(&parse(&["move-session", "--agent", "dsh", "--session", "0b6c"]))
        .expect("move-session reaches this module");
    assert_eq!(request.agent, "dsh");
    assert_eq!(request.session_id, "0b6c");
    assert!(move_session_request(&parse(&["repos"])).is_none());
}
```

`app/src/agent_ctrl_fleet_cli_tests.rs:360-382` already builds `ManagedSessionsArgs` for `move-all`. If `warp_cli::Args`/`Command` are spelled differently there, copy that helper instead of the `parse` closure above.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo nextest run -p warp_cli managed_agent_move_session` → FAIL (no variant).
Run: `cargo nextest run -p warp --lib agent_ctrl_move_session_cli` → FAIL (module missing).

- [ ] **Step 3: Implement**

`crates/warp_cli/src/managed_sessions.rs`, after `MoveAll { … }`:

```rust
    /// Move a recorded agent session that no pane holds (a finished DeepSeek Harness run) to
    /// AgentCtrl.
    MoveSession {
        /// Agent kind: dsh.
        #[arg(long)]
        agent: String,

        /// The session id the agent recorded.
        #[arg(long)]
        session: String,

        #[arg(long, value_enum, default_value_t = ManagedEnvironment::Staging)]
        environment: ManagedEnvironment,
    },
```

`crates/managed_agent_sessions/src/lib.rs`, next to the `MoveAll` arm:

```rust
        ManagedSessionsCommand::MoveSession { .. } => {
            bail!("move-session is handled by the application")
        }
```

`app/src/agent_ctrl_fleet_cli.rs` `move_all_request`: add `| ManagedSessionsCommand::MoveSession { .. }` to the `=> None` arm. Run `cargo check -p warp -p managed_agent_sessions -p warp_cli` and add the same arm to any other exhaustive match the compiler reports.

Create `app/src/agent_ctrl_move_session_cli.rs`:

```rust
//! `<warp binary> agent-ctrl move-session` -- moving a recorded agent session that no pane holds.
//!
//! A dsh headless run has ended by the time it can be moved, and its pane drops the agent session
//! when the command finishes, so the pane's "Move session" never sees it.

use std::time::Duration;

use anyhow::{Context, Result};
use chrono::Local;
use managed_agent_sessions::{MovedSessionImport, import_moved_session, stop_run, wait_until_resumed};
use warp_cli::managed_sessions::{ManagedEnvironment, ManagedSessionsArgs, ManagedSessionsCommand};

use crate::terminal::agent_session_handoff::{HandoffSource, move_idempotency_key, prepare_handoff};
use crate::terminal::agent_session_history::{self, PastAgentSession};
use crate::terminal::CLIAgent;

const RESUME_TIMEOUT: Duration = Duration::from_secs(120);

/// A parsed `agent-ctrl move-session`.
#[derive(Debug, Clone)]
pub(crate) struct MoveSessionRequest {
    pub agent: String,
    pub session_id: String,
    pub environment: ManagedEnvironment,
}

/// The `move-session` request in `args`, which this module runs rather than
/// `managed_agent_sessions`.
pub(crate) fn move_session_request(args: &ManagedSessionsArgs) -> Option<MoveSessionRequest> {
    match &args.command {
        ManagedSessionsCommand::MoveSession {
            agent,
            session,
            environment,
        } => Some(MoveSessionRequest {
            agent: agent.clone(),
            session_id: session.clone(),
            environment: *environment,
        }),
        ManagedSessionsCommand::List
        | ManagedSessionsCommand::Start { .. }
        | ManagedSessionsCommand::Repos { .. }
        | ManagedSessionsCommand::MoveAll { .. }
        | ManagedSessionsCommand::Open { .. }
        | ManagedSessionsCommand::Attach { .. }
        | ManagedSessionsCommand::Input { .. }
        | ManagedSessionsCommand::Close { .. }
        | ManagedSessionsCommand::Stop { .. }
        | ManagedSessionsCommand::Pause { .. }
        | ManagedSessionsCommand::Resume { .. } => None,
    }
}

/// The agent named `label` whose sessions end with their command, so no pane holds them.
fn paneless_agent(label: &str) -> Option<CLIAgent> {
    match label {
        "dsh" => Some(CLIAgent::DeepSeekHarness),
        _ => None,
    }
}

/// The handoff source for recorded session `session_id` of `agent`, in the directory it ran in.
fn recorded_source(
    agent: CLIAgent,
    session_id: &str,
    history: &[PastAgentSession],
) -> Result<HandoffSource> {
    let past = history
        .iter()
        .find(|past| past.agent == agent && past.session_id == session_id)
        .with_context(|| format!("no recorded {} session {session_id}", agent.display_name()))?;
    Ok(HandoffSource {
        agent,
        session_id: session_id.to_owned(),
        cwd: past.cwd.clone(),
        transcript_path: None,
    })
}

pub(crate) fn run(request: MoveSessionRequest) -> Result<()> {
    let agent = paneless_agent(&request.agent).with_context(|| {
        format!(
            "move-session moves dsh sessions; move a {} session from its pane",
            request.agent
        )
    })?;
    let source = recorded_source(
        agent,
        &request.session_id,
        &agent_session_history::find_all(agent),
    )?;
    let home = dirs::home_dir().context("no home directory")?;
    let prepared = prepare_handoff(&source, &home, Local::now())?;
    let pushed_branch = format!("{}/{}", prepared.remote, prepared.branch);
    println!("Pushed the working tree to {pushed_branch}.");
    let source_cwd = source.cwd.display().to_string();
    let job_id = import_moved_session(
        request.environment,
        MovedSessionImport {
            agent_name: prepared.agent_name,
            session_id: &source.session_id,
            idempotency_key: move_idempotency_key(&source.session_id, &prepared.commit),
            session_file: prepared.session_file,
            repo_url: &prepared.repo_url,
            handoff_branch: &prepared.branch,
            source_cwd: &source_cwd,
        },
    )
    .with_context(|| format!("import failed; the handoff branch {pushed_branch} can be deleted"))?;
    println!("Imported as {job_id}; waiting for it to resume…");
    if let Err(error) = wait_until_resumed(request.environment, &job_id, RESUME_TIMEOUT) {
        let stopped = match stop_run(request.environment, &job_id, false) {
            Ok(()) => format!("The run {job_id} was asked to stop."),
            Err(stop_error) => {
                format!("Stop it with: warpspeed agent-ctrl stop {job_id} ({stop_error:#})")
            }
        };
        return Err(error.context(format!("{job_id} did not resume. {stopped}")));
    }
    println!("Resumed. Open it with: warpspeed agent-ctrl open {job_id}");
    Ok(())
}

#[cfg(test)]
#[path = "agent_ctrl_move_session_cli_tests.rs"]
mod tests;
```

Check the `stop_run` return type in `crates/managed_agent_sessions/src/lib.rs`. If it returns something other than `Result<()>`, match on its `Ok(_)`.

`app/src/lib.rs`, next to `agent_ctrl_fleet_cli`:

```rust
#[cfg(all(not(target_family = "wasm"), feature = "local_fs"))]
mod agent_ctrl_move_session_cli;
```

In the `warp_cli::Command::AgentCtrl(args)` arm, before `return managed_agent_sessions::run(args);`:

```rust
                #[cfg(feature = "local_fs")]
                if let Some(request) = agent_ctrl_move_session_cli::move_session_request(args) {
                    return agent_ctrl_move_session_cli::run(request);
                }
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo nextest run -p warp_cli managed_agent` → PASS.
Run: `cargo nextest run -p warp --lib agent_ctrl_move_session_cli agent_ctrl_fleet_cli` → PASS.
Run: `cargo nextest run -p managed_agent_sessions` → PASS.

- [ ] **Step 5: Commit**

```bash
git -C T:/projects/dev-ops/warp-kimi-dsh add crates/warp_cli/src/managed_sessions.rs crates/warp_cli/src/lib_tests.rs crates/managed_agent_sessions/src/lib.rs app/src/agent_ctrl_fleet_cli.rs app/src/agent_ctrl_move_session_cli.rs app/src/agent_ctrl_move_session_cli_tests.rs app/src/lib.rs
git -C T:/projects/dev-ops/warp-kimi-dsh commit -m "feat(cli): agent-ctrl move-session moves a finished dsh session

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

- [ ] **Step 6: Format, lint, push**

Run: `./script/format`. Then run `cargo clippy --workspace --all-targets --all-features --tests -- -D warnings` (the presubmit version). Both must be clean. If `./script/format` changed files, commit them by explicit path:

```bash
git -C T:/projects/dev-ops/warp-kimi-dsh push gitea feat/kimi-dsh-agents
git -C T:/projects/dev-ops/warp-kimi-dsh push gitlab feat/kimi-dsh-agents
```

---

## Part S — Dashboard (`T:\projects\SOTZ3N\stoz3n-agent-dash-kimi-dsh`, Part C's worktree)

### Task 6: The import accepts Kimi session ids

**Files:**
- Modify: `server/agent-control.ts:19`
- Test: `scripts/check-agent-control.mjs` (after the import checks, ~:752)

**Interfaces:**
- Produces: `POST /api/agent-control/runs/import` accepts `session_id` matching `^[A-Za-z0-9_-]{1,128}$`.

- [ ] **Step 1: Write the failing test.** Append after `import rejects …` checks:

```js
await check("import accepts a Kimi session id", async () => {
  const id = "session_0b6c1234-5678-4abc-8def-1234567890ab";
  const response = await importCall(importForm({ session_id: id }), "import-request-0090");
  assert.equal(response.status, 202);
  assert.equal(requests.at(-1).session_id, id);
});
```

- [ ] **Step 2: Run** `yarn test:agent-control` → this check FAILS with 422 `session_id is invalid`.
- [ ] **Step 3: Implement:** `const SESSION_ID = /^[A-Za-z0-9_-]{1,128}$/;`
- [ ] **Step 4: Run** `yarn test:agent-control` and `yarn check` → all pass.
- [ ] **Step 5: Commit and push**

```bash
git -C T:/projects/SOTZ3N/stoz3n-agent-dash-kimi-dsh add server/agent-control.ts scripts/check-agent-control.mjs
git -C T:/projects/SOTZ3N/stoz3n-agent-dash-kimi-dsh commit -m "feat(agent-control): accept kimi session ids on import

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
git -C T:/projects/SOTZ3N/stoz3n-agent-dash-kimi-dsh push origin feat/kimi-dsh-agents
```

---

## Part N — AgentCtrl node (`T:\projects\SOTZ3N\stoz3n-agent-ctrl-kimi-dsh`)

Before Task 7: `git -C T:/projects/SOTZ3N/stoz3n-agent-ctrl-kimi-dsh status` → branch `feat/kimi-dsh-agents`. Tasks 7, 8, 9 and 11 do not need B. Task 10 does: `src/agent/kimi.rs` and `src/agent/dsh.rs` must exist.

### Task 7: Dependencies and session ids with `_`

**Files:**
- Modify: `Cargo.toml` (`# Utilities`), `Cargo.lock`
- Modify: `src/agent/process.rs:151-162` (`validate_session_id`) and its tests
- Modify: `src/remote/controller.rs:114-124` (`moved_session`) and its tests (~:3553)

**Interfaces:**
- Produces:
  - `validate_session_id` accepts `[A-Za-z0-9_-]{1,128}`.
  - The controller `ImportSession` accepts the same.
  - Crates `tar` 0.4 and `zstd` 0.13 are available.

- [ ] **Step 1: Write the failing tests**

In `src/agent/process.rs` `mod tests`:

```rust
    #[test]
    fn session_ids_allow_kimi_and_dsh_shapes_but_no_path_characters() {
        let max = "a".repeat(128);
        for id in [
            "session_0b6c1234-5678-4abc-8def-1234567890ab",
            "session-0b6c1234-5678-4abc-8def-1234567890ab",
            "0b6c1234-5678-4abc-8def-1234567890ab",
            max.as_str(),
        ] {
            assert!(validate_session_id(id).is_ok(), "{id}");
        }
        let long = "a".repeat(129);
        for id in ["", "../x", "a/b", "a.b", "a\\b", long.as_str()] {
            assert!(validate_session_id(id).is_err(), "{id}");
        }
    }
```

In `src/remote/controller.rs` tests, next to `import_limits_accept_boundary_values`:

```rust
    #[tokio::test]
    async fn import_accepts_a_kimi_session_id() {
        let Harness {
            controller,
            node_rx,
            _tmp,
        } = harness(None);
        let mut request = valid_request("direct_cli");
        request.session_id = "session_0b6c1234-5678-4abc-8def-1234567890ab".to_string();
        let spawned = acknowledge_spawn(controller.clone(), node_rx);
        let response = FleetControl::import_session(&controller, tonic::Request::new(request))
            .await
            .expect("kimi session id accepted")
            .into_inner();
        assert!(response.success, "{}", response.error);
        assert_eq!(
            spawned.await.unwrap().session_id,
            "session_0b6c1234-5678-4abc-8def-1234567890ab"
        );
    }
```

- [ ] **Step 2: Run** `cargo test --lib session_ids_allow_kimi import_accepts_a_kimi` → both FAIL.

- [ ] **Step 3: Implement**

`Cargo.toml` under `# Utilities`:

```toml
tar = "0.4"
zstd = "0.13"
```

`validate_session_id`: `.all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')`.

`moved_session`: `.all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')`. Set the message to `"session_id must use 1 to 128 ASCII letters, digits, '_' or '-'"`.

`cargo build` updates `Cargo.lock`. The Dockerfile builds with `--locked`, so the lock file must be committed. `zstd-sys` compiles its bundled C source with `cc`; `rust:1.94-bookworm` ships gcc.

- [ ] **Step 4: Run** `cargo test --lib session_ids_allow_kimi import_accepts_a_kimi invalid_imports_are_rejected` → PASS. Then run `cargo clippy --all-targets -- -D warnings`.

- [ ] **Step 5: Commit**

```bash
git -C T:/projects/SOTZ3N/stoz3n-agent-ctrl-kimi-dsh add Cargo.toml Cargo.lock src/agent/process.rs src/remote/controller.rs
git -C T:/projects/SOTZ3N/stoz3n-agent-ctrl-kimi-dsh commit -m "feat(agent): accept kimi session ids; add tar and zstd

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

### Task 8: `read_session` on the backend traits

**Files:**
- Modify: `src/agent/process.rs`: `CliAdapter` (after `place_session`, :128-141), `impl AgentBackend for CliBackend<A>` (after `place_session`, :1398-1408), tests
- Modify: `src/agent/backend.rs` (`AgentBackend`, after `place_session`, :142-155)
- Modify: `src/agent/runner.rs` (after `place_session`, :93-106)

**Interfaces:**
- Produces (one signature everywhere; this is the signature the pause branch needs, see Task 12):

```rust
fn read_session(
    &self,
    session_id: &str,
    cwd: &Path,
    run_dir: &Path,
    env: &HashMap<String, String>,
) -> Result<Option<Vec<u8>>, BackendError>
```

  `AgentRunner::read_session(&self, backend: Backend, session_id, cwd, run_dir, env)` has the same return type. The default is `Ok(None)`: this CLI has no movable session here.

- [ ] **Step 1: Write the failing test** in `src/agent/process.rs` `mod tests`:

```rust
    #[derive(Default)]
    struct ReadBackAdapter;

    impl CliAdapter for ReadBackAdapter {
        fn name(&self) -> &'static str {
            "read-back"
        }
        fn binary_names(&self) -> &'static [&'static str] {
            &["unused"]
        }
        fn build_args(&self, _cfg: &SpawnConfig, _resume: Option<&str>) -> Vec<String> {
            Vec::new()
        }
        fn read_session(
            &self,
            session_id: &str,
            cwd: &Path,
            run_dir: &Path,
            _env: &HashMap<String, String>,
        ) -> Result<Option<Vec<u8>>, BackendError> {
            Ok(Some(
                format!("{session_id}|{}|{}", cwd.display(), run_dir.display()).into_bytes(),
            ))
        }
    }

    #[test]
    fn read_session_reaches_the_adapter_and_defaults_to_none() {
        let backend = CliBackend::<ReadBackAdapter>::with_binary(PathBuf::from("unused"));
        let env = HashMap::new();
        let read = AgentBackend::read_session(&backend, "s1", Path::new("/w"), Path::new("/r"), &env)
            .unwrap();
        assert_eq!(read.as_deref(), Some(b"s1|/w|/r".as_slice()));
        let echo = CliBackend::<EnvironmentProbeAdapter>::with_binary(PathBuf::from("unused"));
        assert_eq!(
            AgentBackend::read_session(&echo, "s1", Path::new("/w"), Path::new("/r"), &env).unwrap(),
            None
        );
    }
```

- [ ] **Step 2: Run** `cargo test --lib read_session_reaches` → compile error (`read_session` is not a member of `CliAdapter`).

- [ ] **Step 3: Implement**

`CliAdapter` (process.rs), after `place_session`:

```rust
    /// The session this CLI keeps for `session_id` in a run in `cwd`, as the bytes
    /// `place_session` takes. `None` when this CLI has no such session here.
    fn read_session(
        &self,
        _session_id: &str,
        _cwd: &Path,
        _run_dir: &Path,
        _env: &HashMap<String, String>,
    ) -> Result<Option<Vec<u8>>, BackendError> {
        Ok(None)
    }
```

`AgentBackend` (backend.rs), after `place_session`: the same method with the doc `/// See [`crate::agent::process::CliAdapter::read_session`].` and the same `Ok(None)` default.

`impl<A: CliAdapter> AgentBackend for CliBackend<A>`:

```rust
    fn read_session(
        &self,
        session_id: &str,
        cwd: &Path,
        run_dir: &Path,
        env: &HashMap<String, String>,
    ) -> Result<Option<Vec<u8>>, BackendError> {
        self.adapter.read_session(session_id, cwd, run_dir, env)
    }
```

`AgentRunner` (runner.rs):

```rust
    /// See [`AgentBackend::read_session`].
    pub fn read_session(
        &self,
        backend: Backend,
        session_id: &str,
        cwd: &Path,
        run_dir: &Path,
        env: &HashMap<String, String>,
    ) -> Result<Option<Vec<u8>>, BackendError> {
        self.get_backend_for(backend)
            .ok_or_else(|| BackendError::BinaryNotFound(backend.as_str().to_string()))?
            .read_session(session_id, cwd, run_dir, env)
    }
```

- [ ] **Step 4: Run** `cargo test --lib` → all pass. The fake backends in `node.rs` tests compile unchanged because the method has a default. Then run `cargo clippy --all-targets -- -D warnings`.

- [ ] **Step 5: Commit**

```bash
git -C T:/projects/SOTZ3N/stoz3n-agent-ctrl-kimi-dsh add src/agent/process.rs src/agent/backend.rs src/agent/runner.rs
git -C T:/projects/SOTZ3N/stoz3n-agent-ctrl-kimi-dsh commit -m "feat(agent): read_session reads a session back in the form place_session takes

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

### Task 9: Kimi `workDirKey` and session archive

**Files:**
- Create: `src/agent/kimi_session.rs`
- Create: `tests/fixtures/session_keys.json`
- Modify: `src/agent/mod.rs` (`pub mod kimi_session;`)

**Interfaces:**
- Produces:
  - `pub(crate) fn work_dir_key(work_dir: &str) -> String`
  - `pub(crate) fn pack_session(session_dir: &Path, session_id: &str) -> Result<Vec<u8>, BackendError>`
  - `pub(crate) fn unpack_session(archive: &[u8], session_id: &str, session_dir: &Path) -> Result<(), BackendError>`

Source of the algorithm: kimi-code `packages/agent-core-v2/src/_base/utils/workdir-slug.ts`:
1. `normalized = workDir.replace(/\\/g,'/').replace(/\/+$/,'')`.
2. `base` = the last `/` segment.
3. `slug` = lowercase, then runs of `[^a-z0-9._-]` → `-`, then trim `-`, cut to 40, trim `-`. If the result is `''`, `.` or `..`, use `workspace`.
4. `hash` = the first 12 hex digits of `sha256(normalized)` over UTF-8.
5. The key is `wd_<slug>_<hash>`.

`workspaceService.ts:84` calls it with the raw root string. Kimi's ACP `session/resume` ignores the requested `cwd` and finds the session by id. `sessionIndexSource.ts` scans `sessions/<ws>/<id>/state.json` and takes the cwd from `state.json`.

- [ ] **Step 1: Write the fixture and the failing tests**

Create `tests/fixtures/session_keys.json`. The vectors come from a line-by-line Python port of the TS; the sha256 values were checked with `sha256sum`.

```json
{
  "kimi_work_dir_key": [
    ["/workspaces/.runs/job-abc123", "wd_job-abc123_2227424d708a"],
    ["C:\\Users\\budel\\Projects\\My App\\", "wd_my-app_3558eb3df80a"],
    ["/srv/\u00dcn\u00efcode Dir/", "wd_n-code-dir_3860836f7a31"],
    ["/", "wd_workspace_e3b0c44298fc"],
    ["/tmp/..", "wd_workspace_76e2b99d0a0e"],
    ["/x/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-bcdef", "wd_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa_c7c52d07e675"],
    ["T:/projects/dev-ops/warp-kimi-dsh", "wd_warp-kimi-dsh_ec2171cbfdda"]
  ],
  "dsh_project_key": [
    ["/workspaces/.runs/job-abc123", "--workspaces-.runs-job-abc123--"],
    ["C:\\Users\\budel\\My App", "--C-Users-budel-My~0020App--"],
    ["/srv/a~b/\u00fc", "--srv-a~007Eb-~00FC--"],
    ["/", "--root--"],
    ["//server\\\\share::x", "--server-share-x--"],
    ["/\ud83d\ude00", "--~D83D~DE00--"]
  ],
  "dsh_encode_segment": [
    ["0b6c1234-5678-4abc-8def-1234567890ab", "0b6c1234-5678-4abc-8def-1234567890ab"],
    ["session-0b6c1234-5678-4abc-8def-1234567890ab", "session-0b6c1234-5678-4abc-8def-1234567890ab"],
    [".", "~002E"],
    ["..", "~002E~002E"],
    ["a/b", "a~002Fb"],
    ["a~b", "a~007Eb"],
    ["\u00e9", "~00E9"]
  ]
}
```

The 39-`a` vector: the base is 39 × `a` + `-bcdef`. The 40-character cut ends on the `-`, which the second trim removes, leaving 39 × `a`.

Create `src/agent/kimi_session.rs` with the tests first (the implementation follows in Step 3):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "session_0b6c1234-5678-4abc-8def-1234567890ab";

    pub(crate) fn raw_tar(entries: &[(&[u8], tar::EntryType, &[u8])]) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        for (name, kind, data) in entries {
            let mut header = tar::Header::new_old();
            header.as_old_mut().name[..name.len()].copy_from_slice(name);
            header.set_entry_type(*kind);
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder.append(&header, *data).unwrap();
        }
        builder.into_inner().unwrap()
    }

    #[test]
    fn work_dir_key_matches_kimi_vectors() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../tests/fixtures/session_keys.json")).unwrap();
        for pair in fixture["kimi_work_dir_key"].as_array().unwrap() {
            let (input, expected) = (pair[0].as_str().unwrap(), pair[1].as_str().unwrap());
            assert_eq!(work_dir_key(input), expected, "{input:?}");
        }
    }

    #[test]
    fn unpack_writes_the_session_files_under_the_session_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("sessions/wd_x/").join(ID);
        let archive = raw_tar(&[
            (format!("{ID}/").as_bytes(), tar::EntryType::Directory, b""),
            (format!("{ID}/state.json").as_bytes(), tar::EntryType::Regular, b"{}"),
            (
                format!("{ID}/agents/main/wire.jsonl").as_bytes(),
                tar::EntryType::Regular,
                b"{\"a\":1}\n",
            ),
        ]);
        unpack_session(&archive, ID, &dir).unwrap();
        assert_eq!(std::fs::read(dir.join("state.json")).unwrap(), b"{}");
        assert_eq!(
            std::fs::read(dir.join("agents/main/wire.jsonl")).unwrap(),
            b"{\"a\":1}\n"
        );
    }

    #[test]
    fn unpack_refuses_escaping_foreign_or_special_entries() {
        let state = format!("{ID}/state.json");
        let cases: Vec<(String, Vec<u8>)> = vec![
            ("parent".into(), raw_tar(&[(state.as_bytes(), tar::EntryType::Regular, b"{}"), (b"../evil", tar::EntryType::Regular, b"x")])),
            ("nested parent".into(), raw_tar(&[(state.as_bytes(), tar::EntryType::Regular, b"{}"), (format!("{ID}/../evil").as_bytes(), tar::EntryType::Regular, b"x")])),
            ("absolute".into(), raw_tar(&[(state.as_bytes(), tar::EntryType::Regular, b"{}"), (b"/tmp/evil", tar::EntryType::Regular, b"x")])),
            ("other session".into(), raw_tar(&[(b"session_other/state.json", tar::EntryType::Regular, b"{}")])),
            ("symlink".into(), raw_tar(&[(state.as_bytes(), tar::EntryType::Regular, b"{}"), (format!("{ID}/link").as_bytes(), tar::EntryType::Symlink, b"")])),
            ("hard link".into(), raw_tar(&[(state.as_bytes(), tar::EntryType::Regular, b"{}"), (format!("{ID}/link").as_bytes(), tar::EntryType::Link, b"")])),
            ("no state".into(), raw_tar(&[(format!("{ID}/agents/main/wire.jsonl").as_bytes(), tar::EntryType::Regular, b"{}")])),
            ("not a tar".into(), b"not a tar archive at all".to_vec()),
        ];
        for (name, archive) in cases {
            let tmp = tempfile::tempdir().unwrap();
            let dir = tmp.path().join("home/sessions/wd_x").join(ID);
            assert!(unpack_session(&archive, ID, &dir).is_err(), "{name}");
            assert!(!tmp.path().join("home/sessions/evil").exists(), "{name}");
            assert!(!tmp.path().join("evil").exists(), "{name}");
        }
    }

    #[test]
    fn pack_and_unpack_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("a").join(ID);
        write_placed_file(&source.join("state.json"), b"{\"cwd\":\"/x\"}").unwrap();
        write_placed_file(&source.join("agents/main/wire.jsonl"), b"1\n2\n").unwrap();
        let archive = pack_session(&source, ID).unwrap();
        let target = tmp.path().join("b").join(ID);
        unpack_session(&archive, ID, &target).unwrap();
        assert_eq!(std::fs::read(target.join("state.json")).unwrap(), b"{\"cwd\":\"/x\"}");
        assert_eq!(std::fs::read(target.join("agents/main/wire.jsonl")).unwrap(), b"1\n2\n");
    }

    #[test]
    fn pack_refuses_a_session_over_the_import_limit() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join(ID);
        write_placed_file(&dir.join("state.json"), b"{}").unwrap();
        std::fs::File::create(dir.join("big"))
            .unwrap()
            .set_len(crate::remote::MAX_SESSION_FILE_BYTES as u64 + 1)
            .unwrap();
        assert!(pack_session(&dir, ID).is_err());
    }
}
```

`cargo fmt` reflows the long `cases` lines.

- [ ] **Step 2: Run** `cargo test --lib kimi_session` → compile errors (functions missing).

- [ ] **Step 3: Implement.** Put this at the top of `src/agent/kimi_session.rs`, above the tests, and register it in `src/agent/mod.rs` with `pub mod kimi_session;`:

```rust
//! Kimi Code's on-disk sessions under `KIMI_CODE_HOME`, for moving one between machines.

use std::io::Read as _;
use std::path::{Component, Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::agent::backend::BackendError;
use crate::agent::process::write_placed_file;
use crate::remote::MAX_SESSION_FILE_BYTES;

const MAX_SLUG_CHARS: usize = 40;
const STATE_FILE: &str = "state.json";

fn invalid(why: impl std::fmt::Display) -> BackendError {
    BackendError::SpawnFailed(format!("invalid kimi session: {why}"))
}

/// Kimi's `slugifyWorkDirName`.
fn slugify_work_dir_name(name: &str) -> String {
    let mut slug = String::new();
    let mut in_replaced_run = false;
    for c in name.to_lowercase().chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-') {
            slug.push(c);
            in_replaced_run = false;
        } else if !in_replaced_run {
            slug.push('-');
            in_replaced_run = true;
        }
    }
    let cut: String = slug.trim_matches('-').chars().take(MAX_SLUG_CHARS).collect();
    match cut.trim_matches('-') {
        "" | "." | ".." => "workspace".to_string(),
        slug => slug.to_string(),
    }
}

/// Kimi's `encodeWorkDirKey`: the `sessions/` directory for sessions started in `work_dir`.
pub(crate) fn work_dir_key(work_dir: &str) -> String {
    let slashed = work_dir.replace('\\', "/");
    let normalized = slashed.trim_end_matches('/');
    let base = normalized.rsplit('/').next().unwrap_or(normalized);
    let hash: String = Sha256::digest(normalized.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("wd_{}_{}", slugify_work_dir_name(base), &hash[..12])
}

/// `path` without its leading `session_id` component, when every component is a plain name.
fn session_relative_path(path: &Path, session_id: &str) -> Option<PathBuf> {
    let mut components = path.components();
    match components.next()? {
        Component::Normal(first) if first == session_id => {}
        Component::Normal(_)
        | Component::CurDir
        | Component::ParentDir
        | Component::RootDir
        | Component::Prefix(_) => return None,
    }
    let mut relative = PathBuf::new();
    for component in components {
        match component {
            Component::Normal(name) => relative.push(name),
            Component::CurDir | Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return None
            }
        }
    }
    Some(relative)
}

/// Writes the regular files of `archive`, a tar of `<session_id>/<path>` entries, under
/// `session_dir`. Refuses the whole archive for an entry outside `<session_id>/`, a `..` or
/// absolute path, a link or device, or a missing `state.json`. The archive came over the network
/// from another machine.
pub(crate) fn unpack_session(
    archive: &[u8],
    session_id: &str,
    session_dir: &Path,
) -> Result<(), BackendError> {
    let mut files = Vec::new();
    let mut archive = tar::Archive::new(archive);
    for entry in archive.entries().map_err(invalid)? {
        let mut entry = entry.map_err(invalid)?;
        let path = entry.path().map_err(invalid)?.into_owned();
        let relative = session_relative_path(&path, session_id)
            .ok_or_else(|| invalid(format!("entry {} is outside {session_id}/", path.display())))?;
        match entry.header().entry_type() {
            tar::EntryType::Directory => continue,
            tar::EntryType::Regular if !relative.as_os_str().is_empty() => {}
            other => {
                return Err(invalid(format!("entry {} is a {other:?}", path.display())));
            }
        }
        let mut content = Vec::new();
        entry.read_to_end(&mut content).map_err(invalid)?;
        files.push((relative, content));
    }
    if !files.iter().any(|(relative, _)| relative == Path::new(STATE_FILE)) {
        return Err(invalid("no state.json"));
    }
    for (relative, content) in files {
        write_placed_file(&session_dir.join(relative), &content)?;
    }
    Ok(())
}

fn collect_files(dir: &Path, relative: &Path, files: &mut Vec<PathBuf>) -> std::io::Result<()> {
    let mut entries = std::fs::read_dir(dir)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let kind = entry.file_type()?;
        let name = relative.join(entry.file_name());
        if kind.is_dir() {
            collect_files(&entry.path(), &name, files)?;
        } else if kind.is_file() {
            files.push(name);
        }
    }
    Ok(())
}

/// Packs `session_dir` as the archive [`unpack_session`] takes: every regular file as
/// `<session_id>/<path>`. Refuses a session over the import limit.
pub(crate) fn pack_session(session_dir: &Path, session_id: &str) -> Result<Vec<u8>, BackendError> {
    let io = |e: std::io::Error| {
        BackendError::SpawnFailed(format!("read kimi session {}: {e}", session_dir.display()))
    };
    let too_large = || {
        BackendError::SpawnFailed(format!(
            "kimi session too large to upload (limit {MAX_SESSION_FILE_BYTES} bytes); resume this run on this node"
        ))
    };
    let mut files = Vec::new();
    collect_files(session_dir, Path::new(""), &mut files).map_err(io)?;
    let mut builder = tar::Builder::new(Vec::new());
    let mut total = 0usize;
    for relative in files {
        let content = std::fs::read(session_dir.join(&relative)).map_err(io)?;
        total += content.len();
        if total > MAX_SESSION_FILE_BYTES {
            return Err(too_large());
        }
        let name = std::iter::once(session_id.to_string())
            .chain(
                relative
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy().into_owned()),
            )
            .collect::<Vec<_>>()
            .join("/");
        let mut header = tar::Header::new_gnu();
        header.set_size(content.len() as u64);
        header.set_mode(0o644);
        header.set_entry_type(tar::EntryType::Regular);
        builder
            .append_data(&mut header, &name, content.as_slice())
            .map_err(io)?;
    }
    let archive = builder.into_inner().map_err(io)?;
    if archive.len() > MAX_SESSION_FILE_BYTES {
        return Err(too_large());
    }
    Ok(archive)
}
```

If `crate::remote::MAX_SESSION_FILE_BYTES` is not reachable from `agent` (check `src/lib.rs` for `pub mod remote`), use the path `src/remote/mod.rs:25` exposes.

- [ ] **Step 4: Run** `cargo test --lib kimi_session` → PASS. Then run `cargo fmt` and `cargo clippy --all-targets -- -D warnings`.

- [ ] **Step 5: Commit**

```bash
git -C T:/projects/SOTZ3N/stoz3n-agent-ctrl-kimi-dsh add src/agent/kimi_session.rs src/agent/mod.rs tests/fixtures/session_keys.json
git -C T:/projects/SOTZ3N/stoz3n-agent-ctrl-kimi-dsh commit -m "feat(agent): kimi workDirKey and a validated session archive

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

### Task 10: Kimi `place_session` and `read_session`

**Files:**
- Modify: `src/agent/kimi_session.rs` (add `kimi_home`, `place`, `read`, `rewrite_state`, `append_index_line`)
- Modify: `src/agent/kimi.rs` (Plan B's `KimiAdapter`: two trait methods)

**Interfaces:**
- Consumes: `unpack_session`, `pack_session`, `work_dir_key` (Task 9); `KimiAdapter` (Plan B); `CliAdapter::read_session` (Task 8).
- Produces:
  - `pub(crate) fn kimi_home(env: &HashMap<String, String>, run_dir: &Path) -> PathBuf`
  - `pub(crate) fn place(archive: &[u8], session_id: &str, cwd: &Path, kimi_home: &Path) -> Result<PlacedSession, BackendError>`
  - `pub(crate) fn read(session_id: &str, cwd: &Path, kimi_home: &Path) -> Result<Option<Vec<u8>>, BackendError>`
  - `KimiAdapter::{place_session, read_session}`

- [ ] **Step 1: Interface check against Plan B.** Open `src/agent/kimi.rs` and find where `KIMI_CODE_HOME` is set at launch (`configure_command` or `build_args`).
  - Confirm it resolves to `<run_dir>/kimi-home`, where `<run_dir>` equals the `run_dir` that `node.rs` `prepare()` passes to `place_session` (`handoff.worktree.with_extension("session")`).
  - If B exposes a helper (for example `kimi_code_home(run_dir)`), make `kimi_home` below call it instead of `run_dir.join("kimi-home")`.
  - If B derives the home some other way, stop and reconcile with B's owner before going on. A mismatch means the moved session is placed where Kimi never looks.

- [ ] **Step 2: Write the failing tests** (append inside `mod tests` of `kimi_session.rs`):

```rust
    const NODE_CWD: &str = "/workspaces/.runs/job-abc123";

    fn moved_archive() -> Vec<u8> {
        let state = serde_json::json!({
            "id": ID,
            "version": 2,
            "title": "Fix the parser",
            "createdAt": 1760000000000u64,
            "updatedAt": 1760000300000u64,
            "archived": false,
            "cwd": "C:\\Users\\budel\\src\\app",
            "agents": {
                "main": {"homedir": "C:/Users/budel/.kimi-code/sessions/wd_app_1/x/agents/main", "type": "main"},
                "sub-1": {"homedir": "C:/old/agents/sub-1", "type": "sub", "parentAgentId": "main"}
            }
        })
        .to_string();
        raw_tar(&[
            (format!("{ID}/state.json").as_bytes(), tar::EntryType::Regular, state.as_bytes()),
            (format!("{ID}/agents/main/wire.jsonl").as_bytes(), tar::EntryType::Regular, b"{\"w\":1}\n"),
        ])
    }

    #[test]
    fn place_unpacks_under_the_work_dir_key_rewrites_paths_and_indexes_it() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("kimi-home");
        let placed = place(&moved_archive(), ID, Path::new(NODE_CWD), &home).unwrap();

        let session_dir = home.join("sessions/wd_job-abc123_2227424d708a").join(ID);
        assert_eq!(placed.resume_ref, ID);
        assert_eq!(placed.cleanup, session_dir);
        assert_eq!(
            std::fs::read(session_dir.join("agents/main/wire.jsonl")).unwrap(),
            b"{\"w\":1}\n"
        );
        let state: serde_json::Value =
            serde_json::from_slice(&std::fs::read(session_dir.join("state.json")).unwrap()).unwrap();
        assert_eq!(state["cwd"], NODE_CWD);
        assert_eq!(state["title"], "Fix the parser");
        assert_eq!(state["createdAt"], 1760000000000u64);
        for agent in ["main", "sub-1"] {
            assert_eq!(
                state["agents"][agent]["homedir"],
                session_dir.join("agents").join(agent).to_string_lossy().as_ref()
            );
        }
        assert_eq!(state["agents"]["sub-1"]["parentAgentId"], "main");

        let index = std::fs::read_to_string(home.join("session_index.jsonl")).unwrap();
        let line: serde_json::Value = serde_json::from_str(index.lines().last().unwrap()).unwrap();
        assert_eq!(line["sessionId"], ID);
        assert_eq!(line["sessionDir"], session_dir.to_string_lossy().as_ref());
        assert_eq!(line["workDir"], NODE_CWD);
    }

    #[test]
    fn place_refuses_bad_ids_states_and_agent_ids() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("kimi-home");
        assert!(place(&moved_archive(), "../x", Path::new(NODE_CWD), &home).is_err());
        let not_object = raw_tar(&[(format!("{ID}/state.json").as_bytes(), tar::EntryType::Regular, b"[1]")]);
        assert!(place(&not_object, ID, Path::new(NODE_CWD), &home).is_err());
        let bad_agent = raw_tar(&[(
            format!("{ID}/state.json").as_bytes(),
            tar::EntryType::Regular,
            br#"{"agents":{"../x":{"homedir":"/a"}}}"#,
        )]);
        assert!(place(&bad_agent, ID, Path::new(NODE_CWD), &home).is_err());
    }

    #[test]
    fn read_returns_what_place_takes_or_none() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("kimi-home");
        place(&moved_archive(), ID, Path::new(NODE_CWD), &home).unwrap();

        let archive = read(ID, Path::new(NODE_CWD), &home).unwrap().unwrap();
        let again = tmp.path().join("again");
        let placed = place(&archive, ID, Path::new("/other/cwd"), &again).unwrap();
        let state: serde_json::Value =
            serde_json::from_slice(&std::fs::read(placed.cleanup.join("state.json")).unwrap()).unwrap();
        assert_eq!(state["cwd"], "/other/cwd");
        assert_eq!(state["title"], "Fix the parser");

        // A session found by id when the run's cwd changed since it was created.
        assert!(read(ID, Path::new("/elsewhere"), &home).unwrap().is_some());
        assert_eq!(read("session_missing", Path::new(NODE_CWD), &home).unwrap(), None);
        assert_eq!(read(ID, Path::new(NODE_CWD), &tmp.path().join("none")).unwrap(), None);
    }

    #[test]
    fn kimi_home_prefers_the_env_then_the_run_dir() {
        let run_dir = Path::new("/w/.runs/job.session");
        assert_eq!(kimi_home(&HashMap::new(), run_dir), run_dir.join("kimi-home"));
        let env = HashMap::from([("KIMI_CODE_HOME".to_string(), "/k".to_string())]);
        assert_eq!(kimi_home(&env, run_dir), PathBuf::from("/k"));
    }
```

Add `use std::collections::HashMap;` to the test module.

- [ ] **Step 3: Run** `cargo test --lib kimi_session` → compile errors (`place`, `read`, `kimi_home` missing).

- [ ] **Step 4: Implement** (add to `kimi_session.rs` above the tests):

```rust
use std::collections::HashMap;
use std::io::Write as _;

use serde_json::Value;

use crate::agent::backend::PlacedSession;
use crate::agent::process::{env_path, validate_session_id};

const INDEX_FILE: &str = "session_index.jsonl";

/// The run's Kimi home: `KIMI_CODE_HOME` from the run's env, else `<run_dir>/kimi-home`, where
/// the adapter points Kimi at launch.
pub(crate) fn kimi_home(env: &HashMap<String, String>, run_dir: &Path) -> PathBuf {
    env_path(env, "KIMI_CODE_HOME").unwrap_or_else(|| run_dir.join("kimi-home"))
}

fn is_plain_name(name: &str) -> bool {
    let mut components = Path::new(name).components();
    matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none()
}

/// Points the moved session's recorded paths at this machine: `cwd`, and each agent's
/// `homedir`, which Kimi keeps under the session directory.
fn rewrite_state(session_dir: &Path, work_dir: &str) -> Result<(), BackendError> {
    let path = session_dir.join(STATE_FILE);
    let text = std::fs::read(&path).map_err(invalid)?;
    let mut state: Value = serde_json::from_slice(&text).map_err(invalid)?;
    let object = state
        .as_object_mut()
        .ok_or_else(|| invalid("state.json is not an object"))?;
    object.insert("cwd".to_string(), Value::String(work_dir.to_string()));
    if let Some(agents) = object.get_mut("agents").and_then(Value::as_object_mut) {
        for (agent_id, agent) in agents.iter_mut() {
            if !is_plain_name(agent_id) {
                return Err(invalid(format!("agent id {agent_id:?}")));
            }
            if let Some(agent) = agent.as_object_mut() {
                let homedir = session_dir.join("agents").join(agent_id);
                agent.insert(
                    "homedir".to_string(),
                    Value::String(homedir.to_string_lossy().into_owned()),
                );
            }
        }
    }
    write_placed_file(&path, &serde_json::to_vec(&state).map_err(invalid)?)
}

fn append_index_line(
    kimi_home: &Path,
    session_id: &str,
    session_dir: &Path,
    work_dir: &str,
) -> Result<(), BackendError> {
    let path = kimi_home.join(INDEX_FILE);
    let io = |e: std::io::Error| {
        BackendError::SpawnFailed(format!("index kimi session at {}: {e}", path.display()))
    };
    let line = serde_json::json!({
        "sessionId": session_id,
        "sessionDir": session_dir.to_string_lossy(),
        "workDir": work_dir,
    });
    std::fs::create_dir_all(kimi_home).map_err(io)?;
    let mut index = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(io)?;
    writeln!(index, "{line}").map_err(io)
}

/// Places a Kimi session moved from another machine in `kimi_home`, under the workspace
/// directory Kimi derives from `cwd`. Kimi's ACP `session/resume` finds it by id through its
/// session index.
pub(crate) fn place(
    archive: &[u8],
    session_id: &str,
    cwd: &Path,
    kimi_home: &Path,
) -> Result<PlacedSession, BackendError> {
    validate_session_id(session_id)?;
    let work_dir = cwd.to_string_lossy();
    let session_dir = kimi_home
        .join("sessions")
        .join(work_dir_key(&work_dir))
        .join(session_id);
    unpack_session(archive, session_id, &session_dir)?;
    rewrite_state(&session_dir, &work_dir)?;
    append_index_line(kimi_home, session_id, &session_dir, &work_dir)?;
    Ok(PlacedSession {
        resume_ref: session_id.to_string(),
        cleanup: session_dir,
    })
}

/// The archive [`place`] takes for Kimi session `session_id` of a run in `cwd`, or `None` when
/// `kimi_home` has no such session.
pub(crate) fn read(
    session_id: &str,
    cwd: &Path,
    kimi_home: &Path,
) -> Result<Option<Vec<u8>>, BackendError> {
    validate_session_id(session_id)?;
    let sessions = kimi_home.join("sessions");
    let keyed = sessions
        .join(work_dir_key(&cwd.to_string_lossy()))
        .join(session_id);
    let found = if keyed.join(STATE_FILE).is_file() {
        Some(keyed)
    } else {
        std::fs::read_dir(&sessions).ok().and_then(|workspaces| {
            workspaces
                .flatten()
                .map(|workspace| workspace.path().join(session_id))
                .find(|dir| dir.join(STATE_FILE).is_file())
        })
    };
    found.map(|dir| pack_session(&dir, session_id)).transpose()
}
```

Merge the new `use` lines into the existing import block at the top of the file.

In `src/agent/kimi.rs` (`impl CliAdapter for KimiAdapter`):

```rust
    fn place_session(
        &self,
        file: &[u8],
        session_id: &str,
        cwd: &Path,
        run_dir: &Path,
        env: &HashMap<String, String>,
    ) -> Result<PlacedSession, BackendError> {
        kimi_session::place(file, session_id, cwd, &kimi_session::kimi_home(env, run_dir))
    }

    fn read_session(
        &self,
        session_id: &str,
        cwd: &Path,
        run_dir: &Path,
        env: &HashMap<String, String>,
    ) -> Result<Option<Vec<u8>>, BackendError> {
        kimi_session::read(session_id, cwd, &kimi_session::kimi_home(env, run_dir))
    }
```

Add `use crate::agent::kimi_session;` plus whatever of `PlacedSession`, `HashMap` and `Path` `kimi.rs` does not import yet. If B already wrote a stub `place_session` in `kimi.rs`, replace it.

- [ ] **Step 5: Add an adapter-level test** in `kimi.rs`'s `mod tests`:

```rust
    #[test]
    fn moved_session_is_placed_in_the_run_kimi_home() {
        let run_dir = tempfile::tempdir().unwrap();
        let id = "session_0b6c1234-5678-4abc-8def-1234567890ab";
        let mut builder = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_gnu();
        header.set_size(2);
        header.set_mode(0o644);
        builder
            .append_data(&mut header, format!("{id}/state.json"), b"{}".as_slice())
            .unwrap();
        let archive = builder.into_inner().unwrap();
        let cwd = Path::new("/workspaces/.runs/job-abc123");
        let placed = KimiAdapter
            .place_session(&archive, id, cwd, run_dir.path(), &HashMap::new())
            .unwrap();
        assert_eq!(
            placed.cleanup,
            run_dir.path().join("kimi-home/sessions/wd_job-abc123_2227424d708a").join(id)
        );
        assert!(KimiAdapter
            .read_session(id, cwd, run_dir.path(), &HashMap::new())
            .unwrap()
            .is_some());
    }
```

- [ ] **Step 6: Run** `cargo test --lib kimi` → PASS. Then run `cargo fmt` and `cargo clippy --all-targets -- -D warnings`.

- [ ] **Step 7: Commit**

```bash
git -C T:/projects/SOTZ3N/stoz3n-agent-ctrl-kimi-dsh add src/agent/kimi_session.rs src/agent/kimi.rs
git -C T:/projects/SOTZ3N/stoz3n-agent-ctrl-kimi-dsh commit -m "feat(kimi): place and read moved kimi sessions

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

### Task 11: dsh keys, `place_session` and `read_session`

**Files:**
- Create: `src/agent/dsh_session.rs`
- Create: `tests/fixtures/dsh/session.v4.jsonl`
- Modify: `src/agent/mod.rs` (`pub mod dsh_session;`)
- Modify: `src/agent/dsh.rs` (Plan B's `DshAdapter`: two trait methods)

**Interfaces:**
- Consumes: `tests/fixtures/session_keys.json` (Task 9); `DshAdapter` (Plan B); `CliAdapter::read_session` (Task 8).
- Produces:
  - `pub(crate) fn project_key(cwd: &str) -> String` (returns `--<slug>--`)
  - `pub(crate) fn encode_segment(raw: &str) -> String`
  - `pub(crate) fn generation_file_name(version: u64) -> String`
  - `pub(crate) fn dsh_home(env, run_dir) -> PathBuf`
  - `pub(crate) fn place(file: &[u8], session_id: &str, cwd: &Path, dsh_home: &Path) -> Result<PlacedSession, BackendError>`
  - `pub(crate) fn read(session_id: &str, cwd: &Path, dsh_home: &Path) -> Result<Option<Vec<u8>>, BackendError>`

Source: deepseek-harness `packages/session/session-persistence-jsonl/src/`:
- `format.ts`: `projectKey`, `encodeSegment`, `sessionDir = join(projectDir(root, cwd), encodeSegment(id))`, header keys, `isHeaderLine` (`cwd` must be absolute).
- `index.ts`: `assertZstdHeaderFrame` requires the first frame to be exactly one header line; `assertStoredIdentity` requires the path to equal `generationLogPath(root, header.cwd, header.id, version)`; `encodeMaterialization` writes a header frame plus one event frame.
- `zstd.ts`: frames are compressed with `ZSTD_c_checksumFlag = 1`.
- `session-format/src/filename.ts`: `session.jsonl` for version 0, otherwise `session.v<N>.jsonl`; `.zstd` is appended when compressed.

- [ ] **Step 1: Interface check against Plan B.** Open `src/agent/dsh.rs`. Confirm launch sets `DSH_HOME=<run_dir>/dsh-home` with the same `run_dir` `place_session` receives. Confirm the template seeding copies files into that directory without deleting it (a `remove_dir_all` before the copy would delete the placed session). If B exposes a helper for the home, make `dsh_home` below call it. Reconcile with B's owner before going on if either check fails.

- [ ] **Step 2: Write the fixture and the failing tests**

`tests/fixtures/dsh/session.v4.jsonl` (plaintext; the event rows are placeholders, because our code never parses them):

```
{"type":"session","version":4,"id":"0b6c1234-5678-4abc-8def-1234567890ab","createdAt":1760000000000,"cwd":"C:\\Users\\budel\\src\\app","isSeeded":false,"delegationDepth":0}
{"type":"user/message","text":"The codeword is OBSIDIAN-17."}
{"type":"assistant/message","text":"OK"}
{"type":"turn/end"}
```

The file ends with a newline.

`src/agent/dsh_session.rs` tests:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "0b6c1234-5678-4abc-8def-1234567890ab";
    const NODE_CWD: &str = "/workspaces/.runs/job-abc123";
    const FIXTURE: &[u8] = include_bytes!("../../tests/fixtures/dsh/session.v4.jsonl");

    fn fixture_vectors(key: &str) -> Vec<(String, String)> {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../tests/fixtures/session_keys.json")).unwrap();
        fixture[key]
            .as_array()
            .unwrap()
            .iter()
            .map(|pair| (pair[0].as_str().unwrap().to_string(), pair[1].as_str().unwrap().to_string()))
            .collect()
    }

    /// The frames of a zstd stream, each decoded on its own, as dsh's reader sees them.
    fn frames(stream: &[u8]) -> Vec<(Vec<u8>, bool)> {
        let mut out = Vec::new();
        let mut rest = stream;
        while !rest.is_empty() {
            let len = zstd::zstd_safe::find_frame_compressed_size(rest).unwrap();
            let checksum = rest[4] & 0x04 != 0;
            out.push((zstd::stream::decode_all(&rest[..len]).unwrap(), checksum));
            rest = &rest[len..];
        }
        out
    }

    #[test]
    fn keys_match_dsh_vectors() {
        for (input, expected) in fixture_vectors("dsh_project_key") {
            assert_eq!(project_key(&input), expected, "{input:?}");
        }
        let long = format!("/{}", "a".repeat(300));
        assert_eq!(project_key(&long), format!("--{}--", "a".repeat(251)));
        for (input, expected) in fixture_vectors("dsh_encode_segment") {
            assert_eq!(encode_segment(&input), expected, "{input:?}");
        }
        assert_eq!(generation_file_name(0), "session.jsonl.zstd");
        assert_eq!(generation_file_name(4), "session.v4.jsonl.zstd");
    }

    #[test]
    fn place_rewrites_the_header_cwd_into_two_checksummed_frames() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("dsh-home");
        let uploaded = compress(&FIXTURE[..FIXTURE.iter().position(|&b| b == b'\n').unwrap() + 1], &FIXTURE[FIXTURE.iter().position(|&b| b == b'\n').unwrap() + 1..]).unwrap();
        let placed = place(&uploaded, ID, Path::new(NODE_CWD), &home).unwrap();

        let session_dir = home.join("sessions/--workspaces-.runs-job-abc123--").join(ID);
        assert_eq!(placed.cleanup, session_dir);
        assert_eq!(placed.resume_ref, ID);
        let written = std::fs::read(session_dir.join("session.v4.jsonl.zstd")).unwrap();
        let frames = frames(&written);
        assert_eq!(frames.len(), 2);
        assert!(frames.iter().all(|(_, checksum)| *checksum));
        let (header, _) = &frames[0];
        assert_eq!(header.iter().filter(|&&b| b == b'\n').count(), 1);
        assert!(header.ends_with(b"\n"));
        let header: serde_json::Value = serde_json::from_slice(header).unwrap();
        let original: serde_json::Value =
            serde_json::from_slice(&FIXTURE[..FIXTURE.iter().position(|&b| b == b'\n').unwrap()]).unwrap();
        assert_eq!(header["cwd"], NODE_CWD);
        for key in ["type", "version", "id", "createdAt", "isSeeded", "delegationDepth"] {
            assert_eq!(header[key], original[key], "{key}");
        }
        assert_eq!(header.as_object().unwrap().len(), original.as_object().unwrap().len());
        let body_start = FIXTURE.iter().position(|&b| b == b'\n').unwrap() + 1;
        assert_eq!(frames[1].0, &FIXTURE[body_start..]);
    }

    #[test]
    fn place_accepts_a_plaintext_upload_and_names_version_zero() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("dsh-home");
        place(FIXTURE, ID, Path::new(NODE_CWD), &home).unwrap();
        assert!(home
            .join("sessions/--workspaces-.runs-job-abc123--")
            .join(ID)
            .join("session.v4.jsonl.zstd")
            .is_file());

        let v0 = br#"{"type":"session","version":0,"id":"0b6c1234-5678-4abc-8def-1234567890ab","createdAt":1}
"#;
        let placed = place(v0, ID, Path::new("/w"), &home).unwrap();
        let written = std::fs::read(placed.cleanup.join("session.jsonl.zstd")).unwrap();
        assert_eq!(frames(&written).len(), 1);
    }

    #[test]
    fn place_refuses_what_dsh_would_refuse() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("dsh-home");
        let other_id = String::from_utf8(FIXTURE.to_vec()).unwrap().replace(ID, "ffffffff-5678-4abc-8def-1234567890ab");
        let torn = &FIXTURE[..FIXTURE.len() - 1];
        let cases: [(&str, &[u8]); 5] = [
            ("other id", other_id.as_bytes()),
            ("torn tail", torn),
            ("no newline", br#"{"type":"session","version":4,"id":"x"}"#),
            ("not a header", b"{\"type\":\"user/message\"}\n"),
            ("corrupt zstd", b"\x28\xb5\x2f\xfd\x00garbage"),
        ];
        for (name, file) in cases {
            assert!(place(file, ID, Path::new(NODE_CWD), &home).is_err(), "{name}");
        }
        assert!(place(FIXTURE, "../x", Path::new(NODE_CWD), &home).is_err());
    }

    #[test]
    fn read_returns_the_highest_generation_or_none() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("dsh-home");
        let placed = place(FIXTURE, ID, Path::new(NODE_CWD), &home).unwrap();
        write_placed_file(&placed.cleanup.join("session.v3.jsonl.zstd"), b"old").unwrap();
        write_placed_file(&placed.cleanup.join("session.v04.jsonl.zstd"), b"bad").unwrap();
        let read_back = read(ID, Path::new(NODE_CWD), &home).unwrap().unwrap();
        assert_eq!(read_back, std::fs::read(placed.cleanup.join("session.v4.jsonl.zstd")).unwrap());
        let again = place(&read_back, ID, Path::new("/other"), &tmp.path().join("again")).unwrap();
        assert!(again.cleanup.join("session.v4.jsonl.zstd").is_file());
        assert_eq!(read("missing", Path::new(NODE_CWD), &home).unwrap(), None);
    }
}
```

`cargo fmt` reflows the long lines. Pull `FIXTURE.iter().position(…)` into a `let header_end` in the implementation pass if clippy flags the repetition.

- [ ] **Step 3: Run** `cargo test --lib dsh_session` → compile errors.

- [ ] **Step 4: Implement** `src/agent/dsh_session.rs` (above the tests) and add `pub mod dsh_session;` to `src/agent/mod.rs`:

```rust
//! DeepSeek Harness's on-disk sessions under `DSH_HOME`, for moving one between machines.

use std::collections::HashMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::agent::backend::{BackendError, PlacedSession};
use crate::agent::process::{env_path, validate_session_id, write_placed_file};
use crate::remote::MAX_SESSION_FILE_BYTES;

const ZSTD_MAGIC: [u8; 4] = [0x28, 0xB5, 0x2F, 0xFD];
const MAX_PROJECT_SLUG_CHARS: usize = 251;

fn invalid(why: impl std::fmt::Display) -> BackendError {
    BackendError::SpawnFailed(format!("invalid dsh session file: {why}"))
}

fn is_safe_unit(c: char) -> bool {
    c != '~' && (c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// dsh's `encodeSegment`: `raw` as one path segment. Safe UTF-16 code units are kept and every
/// other unit, `~` included, is written as `~XXXX`.
pub(crate) fn encode_segment(raw: &str) -> String {
    match raw {
        "." => return "~002E".to_string(),
        ".." => return "~002E~002E".to_string(),
        _ => {}
    }
    raw.encode_utf16()
        .map(|unit| match char::from_u32(u32::from(unit)) {
            Some(c) if is_safe_unit(c) => c.to_string(),
            Some(_) | None => format!("~{unit:04X}"),
        })
        .collect()
}

/// dsh's `projectKey`: the `sessions/` directory, `--<slug>--`, for sessions whose header `cwd`
/// is `cwd`.
pub(crate) fn project_key(cwd: &str) -> String {
    let mut readable = String::new();
    let mut separator_run = false;
    for unit in cwd.encode_utf16() {
        match char::from_u32(u32::from(unit)) {
            Some('/' | '\\' | ':') => {
                if !separator_run {
                    readable.push('-');
                }
                separator_run = true;
            }
            Some(c) if is_safe_unit(c) => {
                readable.push(c);
                separator_run = false;
            }
            Some(_) | None => {
                readable.push_str(&format!("~{unit:04X}"));
                separator_run = false;
            }
        }
    }
    let slug = match readable.trim_start_matches('-') {
        "" => "root",
        slug => slug,
    };
    format!("--{}--", &slug[..slug.len().min(MAX_PROJECT_SLUG_CHARS)])
}

/// The zstd file name of session format generation `version`.
pub(crate) fn generation_file_name(version: u64) -> String {
    match version {
        0 => "session.jsonl.zstd".to_string(),
        version => format!("session.v{version}.jsonl.zstd"),
    }
}

/// The format version and compression a canonical generation file name carries.
fn generation_of(name: &str) -> Option<(u64, bool)> {
    let (stem, zstd) = match name.strip_suffix(".zstd") {
        Some(stem) => (stem, true),
        None => (name, false),
    };
    let version = stem.strip_prefix("session")?.strip_suffix(".jsonl")?;
    if version.is_empty() {
        return Some((0, zstd));
    }
    let digits = version.strip_prefix(".v")?;
    let canonical = !digits.is_empty()
        && !digits.starts_with('0')
        && digits.bytes().all(|byte| byte.is_ascii_digit());
    canonical
        .then(|| digits.parse().ok())
        .flatten()
        .map(|version| (version, zstd))
}

/// The run's dsh home: `DSH_HOME` from the run's env, else `<run_dir>/dsh-home`, where the
/// adapter points dsh at launch.
pub(crate) fn dsh_home(env: &HashMap<String, String>, run_dir: &Path) -> PathBuf {
    env_path(env, "DSH_HOME").unwrap_or_else(|| run_dir.join("dsh-home"))
}

fn session_dir(dsh_home: &Path, cwd: &str, session_id: &str) -> PathBuf {
    dsh_home
        .join("sessions")
        .join(project_key(cwd))
        .join(encode_segment(session_id))
}

fn decompress(file: &[u8]) -> Result<Vec<u8>, BackendError> {
    if file.starts_with(&ZSTD_MAGIC) {
        zstd::stream::decode_all(file).map_err(invalid)
    } else {
        Ok(file.to_vec())
    }
}

/// The header line with `cwd` set, newline-terminated, and its format version.
fn rewrite_header(line: &[u8], session_id: &str, cwd: &str) -> Result<(Vec<u8>, u64), BackendError> {
    let mut header: Value = serde_json::from_slice(line).map_err(invalid)?;
    let object = header
        .as_object_mut()
        .filter(|header| header.get("type").and_then(Value::as_str) == Some("session"))
        .ok_or_else(|| invalid("the first line is not a session header"))?;
    if object.get("id").and_then(Value::as_str) != Some(session_id) {
        return Err(invalid(format!("the header is not session {session_id}")));
    }
    let version = object
        .get("version")
        .and_then(Value::as_u64)
        .ok_or_else(|| invalid("the header has no version"))?;
    object.insert("cwd".to_string(), Value::String(cwd.to_string()));
    let mut line = serde_json::to_vec(&header).map_err(invalid)?;
    line.push(b'\n');
    Ok((line, version))
}

fn frame(data: &[u8]) -> Result<Vec<u8>, BackendError> {
    let io = |e: std::io::Error| BackendError::SpawnFailed(format!("compress dsh session: {e}"));
    let mut encoder = zstd::stream::Encoder::new(Vec::new(), 0).map_err(io)?;
    encoder.include_checksum(true).map_err(io)?;
    encoder.write_all(data).map_err(io)?;
    encoder.finish().map_err(io)
}

/// dsh's own materialized layout: a checksummed frame holding only the header line, then one
/// holding every event row. dsh refuses a first frame that is anything but the header.
fn compress(header: &[u8], body: &[u8]) -> Result<Vec<u8>, BackendError> {
    let mut out = frame(header)?;
    if !body.is_empty() {
        out.extend(frame(body)?);
    }
    Ok(out)
}

/// Places a dsh session file moved from another machine in `dsh_home`, with its header `cwd` set
/// to `cwd`: dsh derives the file's location from that `cwd` and refuses a file found elsewhere.
pub(crate) fn place(
    file: &[u8],
    session_id: &str,
    cwd: &Path,
    dsh_home: &Path,
) -> Result<PlacedSession, BackendError> {
    validate_session_id(session_id)?;
    let cwd = cwd.to_string_lossy();
    let plaintext = decompress(file)?;
    let header_end = plaintext
        .iter()
        .position(|&byte| byte == b'\n')
        .ok_or_else(|| invalid("no header line"))?;
    let (header, version) = rewrite_header(&plaintext[..header_end], session_id, &cwd)?;
    let body = &plaintext[header_end + 1..];
    if !body.is_empty() && !body.ends_with(b"\n") {
        return Err(invalid("it ends in a torn record"));
    }
    let dir = session_dir(dsh_home, &cwd, session_id);
    write_placed_file(
        &dir.join(generation_file_name(version)),
        &compress(&header, body)?,
    )?;
    Ok(PlacedSession {
        resume_ref: session_id.to_string(),
        cleanup: dir,
    })
}

/// The current generation file of dsh session `session_id` of a run in `cwd`, as [`place`]
/// takes it, or `None` when `dsh_home` has no such session.
pub(crate) fn read(
    session_id: &str,
    cwd: &Path,
    dsh_home: &Path,
) -> Result<Option<Vec<u8>>, BackendError> {
    validate_session_id(session_id)?;
    let dir = session_dir(dsh_home, &cwd.to_string_lossy(), session_id);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Ok(None);
    };
    let current = entries
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .filter_map(|entry| Some((generation_of(entry.file_name().to_str()?)?, entry.path())))
        .max_by_key(|(generation, _)| *generation);
    let Some((_, path)) = current else {
        return Ok(None);
    };
    let io = |e: std::io::Error| {
        BackendError::SpawnFailed(format!("read dsh session {}: {e}", path.display()))
    };
    let len = std::fs::metadata(&path).map_err(io)?.len();
    if len > MAX_SESSION_FILE_BYTES as u64 {
        return Err(BackendError::SpawnFailed(format!(
            "dsh session file too large to upload ({len} bytes, limit {MAX_SESSION_FILE_BYTES}); resume this run on this node"
        )));
    }
    std::fs::read(&path).map(Some).map_err(io)
}
```

`src/agent/dsh.rs` (`impl CliAdapter for DshAdapter`): the same two methods as Kimi's, calling `dsh_session::place(file, session_id, cwd, &dsh_session::dsh_home(env, run_dir))` and `dsh_session::read(session_id, cwd, &dsh_session::dsh_home(env, run_dir))`. Add an adapter-level test like Task 10 Step 5: place `tests/fixtures/dsh/session.v4.jsonl` and assert `placed.cleanup == run_dir.join("dsh-home/sessions/--workspaces-.runs-job-abc123--").join("0b6c1234-5678-4abc-8def-1234567890ab")`.

- [ ] **Step 5: Run** `cargo test --lib dsh` → PASS. Run the whole suite with `cargo test`, then `cargo fmt -- --check` and `cargo clippy --all-targets -- -D warnings`.

- [ ] **Step 6: Commit and push**

```bash
git -C T:/projects/SOTZ3N/stoz3n-agent-ctrl-kimi-dsh add src/agent/dsh_session.rs src/agent/mod.rs src/agent/dsh.rs tests/fixtures/dsh/session.v4.jsonl
git -C T:/projects/SOTZ3N/stoz3n-agent-ctrl-kimi-dsh commit -m "feat(dsh): place and read moved dsh sessions with the header cwd rewritten

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
git -C T:/projects/SOTZ3N/stoz3n-agent-ctrl-kimi-dsh push origin feat/kimi-dsh-agents
```

### Task 12 (conditional): Hook `read_session` into the pause branch

Do this when both `feat/pause-and-second-node` and `feat/kimi-dsh-agents` are on agent-ctrl `dev`, in whichever merge lands second. On `dev` before then, `read_session` is implemented and tested but has no caller (spec D·2).

**Files (pause-branch code):**
- `src/agent/process.rs`: `CliAdapter::read_session` default
- `src/agent/kimi_session.rs`, `src/agent/dsh_session.rs`, `src/agent/kimi.rs`, `src/agent/dsh.rs`: `locate_session`
- `src/remote/node.rs`: the pause read at ~:975-1011 and the reconnect snapshot read at ~:1178

**Interfaces:**
- Consumes: pause's `locate_session`, `LocatedSession`, `read_session_file`; this plan's `read_session`.
- Produces: `AgentCompleted.session_file` carries the kimi tar or the dsh file, so a paused kimi/dsh run can resume on another node.

- [ ] **Step 1: Failing test** in `node.rs` tests. Use the pause branch's fake backend (`locate_session` mock ~:2798). Give the fake a `read_session` that returns `Ok(Some(b"archive".to_vec()))`, and a `locate_session` whose `file` is a **directory**. Pause the run and assert the `AgentCompleted.session_file` sent to the controller equals `b"archive"`. Before the change it is empty, because reading a directory fails.
- [ ] **Step 2: Change the `CliAdapter::read_session` default** so the existing adapters keep their behaviour:

```rust
    fn read_session(
        &self,
        session_id: &str,
        cwd: &Path,
        run_dir: &Path,
        env: &HashMap<String, String>,
    ) -> Result<Option<Vec<u8>>, BackendError> {
        let Some(located) = self.locate_session(session_id, cwd, run_dir, env) else {
            return Ok(None);
        };
        let len = std::fs::metadata(&located.file)
            .map_err(|e| BackendError::SpawnFailed(format!("read {}: {e}", located.file.display())))?
            .len();
        if len > crate::remote::MAX_SESSION_FILE_BYTES as u64 {
            return Err(BackendError::SpawnFailed(format!(
                "session file too large to upload ({len} bytes, limit {}); resume this run on this node",
                crate::remote::MAX_SESSION_FILE_BYTES
            )));
        }
        std::fs::read(&located.file)
            .map(Some)
            .map_err(|e| BackendError::SpawnFailed(format!("read {}: {e}", located.file.display())))
    }
```

- [ ] **Step 3: Add `locate_session` for kimi and dsh** so the paused marker and same-node resume work:
  - kimi: `LocatedSession { resume_ref: session_id, file: <session dir> }` when `<session dir>/state.json` exists. Find the directory as `kimi_session::read` does; factor out a `pub(crate) fn find_session_dir(session_id, cwd, kimi_home) -> Option<PathBuf>` and have `read` call it.
  - dsh: `LocatedSession { resume_ref: session_id, file: <current generation> }`. Factor out `pub(crate) fn current_generation(session_id, cwd, dsh_home) -> Option<PathBuf>` and have `read` call it.
- [ ] **Step 4: Replace both `read_session_file(&located.file)` byte reads in `node.rs`** (pause ~:1001, snapshot ~:1178) with `runner.read_session(self.backend, id, &self.worktree, &self.run_dir(), &self.env)`, run through `tokio::task::spawn_blocking`.
  - `Ok(Some(bytes))` → the file.
  - `Ok(None)` → empty.
  - `Err(error)` → `emit_chunk` the error text and push it to `notes`, exactly as the old `Err(note)` branch did.
  - Keep `PauseDetails.session_file = located.file` for the marker.
- [ ] **Step 5: Run** `cargo test` → all pass, including the pause tests. Commit as `feat(node): paused kimi and dsh runs upload their session` with the trailer.

---

## Part E — Staging end-to-end (needs deployed Plan B and installed Plan A)

### Task 13: Prove kimi and dsh moves on staging

**Preconditions (all required; ask the user before any merge or deploy, because merging agent-ctrl or agent-dash to `dev` deploys staging):**
- Plan B merged and deployed:
  - The node image has `kimi` 2.1.1 and `dsh` 0.2.1-alpha.1.
  - The profiles `staging-kimi-interactive-rw` and `staging-dsh-interactive-rw` exist.
  - The LiteLLM aliases `kimi-k2` and `deepseek-v4` answer.
- Tasks 7–11 merged into agent-ctrl `dev` and deployed. Task 6 merged into agent-dash `dev` and deployed.
- Plan A and Tasks 1–5 installed locally: `powershell -ExecutionPolicy Bypass -File ./script/windows/install-warpspeed.ps1` from `T:\projects\dev-ops\warp-kimi-dsh`. Do not restart running WarpSpeed windows; open a **new** window from the Start Menu shortcut.
- `kimi` and `dsh` installed locally (A·6), and the dsh Warp plugin installed.
- kubectl reaches staging through `--context admin@obachan-staging --server https://10.10.1.5:6443 --tls-server-name 10.96.0.1` (alias it `K` below; pass `--context` every time).

Scratch checkout (the repo must be configured on staging; `warpspeed` is):

```bash
git clone --depth 1 --origin gitea https://git-dev.obachan.dev/stozn/warpspeed.git C:/Users/budel/AppData/Local/Temp/claude/e2e-warpspeed
```

- [ ] **Step 1: Fresh kimi run.** Run `warpspeed agent-ctrl start --agent kimi --repo https://git-dev.obachan.dev/stozn/warpspeed.git` and note `<job>`. Then send `warpspeed agent-ctrl input <job> "Reply with the single word PONG."` and run `warpspeed agent-ctrl attach <job> --once`. Expected: an assistant event containing `PONG`. If the input returns 409 `input_not_allowed`, the agent is still answering; retry.
- [ ] **Step 2: Real-CLI key check (spec gap 12).** List the run's Kimi home on the node:

  `K -n agent-ctrl exec agent-ctrl-node-0 -- sh -c 'ls -d /workspace*/.runs/<job>; ls /workspace*/.runs/<job>.session/kimi-home/sessions/'`

  Compute the expected key from the printed worktree path:

  `python -I -c "import hashlib,re,sys;p=sys.argv[1].rstrip('/');b=p.split('/')[-1].lower();s=re.sub(r'[^a-z0-9._-]+','-',b).strip('-')[:40].strip('-') or 'workspace';print('wd_%s_%s'%(s,hashlib.sha256(p.encode()).hexdigest()[:12]))" <worktree>`

  The directory name must equal the output. If it differs, `work_dir_key` is wrong; fix the vectors and the code before going on.
- [ ] **Step 3: Move a Kimi pane session.**
  1. In the new WarpSpeed window, open a pane in the scratch checkout and run `kimi`.
  2. Send `The codeword is HELIOTROPE-42. Just acknowledge.` and wait for the answer.
  3. In another pane, run `echo kimi-move > C:/Users/budel/AppData/Local/Temp/claude/e2e-warpspeed/e2e-kimi-local.txt`.
  4. Run "Move session to AgentCtrl" from the pane menu or the palette.

  Expected:
  - The progress banner shows, then the local Kimi receives `/exit`.
  - The pane runs `agent-ctrl open <job>`, and the run's status shows `resumed`.
  - `git -C C:/Users/budel/AppData/Local/Temp/claude/e2e-warpspeed ls-remote gitea "handoff/*"` lists `handoff/<yyyymmdd-hhmm>-<8 hex>`, with no `session_` in the name.
- [ ] **Step 4: Kimi recall and push-back.**
  1. In the viewer, `send What was the codeword?` → the answer contains `HELIOTROPE-42`.
  2. `send Create a file e2e-kimi.txt containing the word moved, then stop.`
  3. Run `warpspeed agent-ctrl close <job>` and wait for a terminal state (`warpspeed agent-ctrl attach <job> --once`).
  4. Run `git -C …/e2e-warpspeed fetch gitea <handoff branch>` and `git -C …/e2e-warpspeed show --stat FETCH_HEAD`.

  Expected: the commit `agent-ctrl run <job> end` with `e2e-kimi.txt`, and `e2e-kimi-local.txt` in its parent handoff commit.
- [ ] **Step 5: Fresh dsh run.** Same as Step 1 with `--agent dsh` (expect `PONG`). Then the key check:

  `K … exec agent-ctrl-node-0 -- sh -c 'ls /workspace*/.runs/<job>.session/dsh-home/sessions/'`

  Expected: `--` + the worktree path with `/` replaced by `-`, leading `-` dropped, + `--` (for example `--workspace-.runs-<job>--`).
- [ ] **Step 6: Move a finished dsh session.**
  1. In a pane in the scratch checkout, run `dsh headless "The codeword is OBSIDIAN-17. Reply OK."` and wait for it to end.
  2. Take the session id from the picker (A·3) or from `ls ~/.dsh/sessions/--*e2e-warpspeed--/`.
  3. Run `warpspeed agent-ctrl move-session --agent dsh --session <id>`.

  Expected:
  - `Pushed the working tree to gitea/handoff/…`, then `Imported as <job>`, then `Resumed. Open it with: warpspeed agent-ctrl open <job>`.
  - `warpspeed agent-ctrl input <job> "What was the codeword?"` followed by `attach --once` shows `OBSIDIAN-17`.
- [ ] **Step 7: dsh push-back.** `input <job> "Create a file e2e-dsh.txt containing moved, then stop."` → `close <job>` → fetch the handoff branch. Expected: an `agent-ctrl run <job> end` commit with `e2e-dsh.txt`.
- [ ] **Step 8: Refusal drill.** `warpspeed agent-ctrl move-session --agent kimi --session x` → error `move-session moves dsh sessions; move a kimi session from its pane`, and no branch is pushed.
- [ ] **Step 9: Clean up and record.**
  - Check `K … exec agent-ctrl-node-0 -- ls /workspace*/.runs` shows no leftover `<job>` or `<job>.session` for the four runs.
  - Delete the scratch clone.
  - List the handoff branches the E2E left on gitea in the report; delete them only if the user asks.
  - Update the memory note on the feature status (moved kimi/dsh E2E passed, date, job ids).

---

## Self-review

- **Spec coverage:**

  | Spec item | Task |
  |---|---|
  | D·1 `agent_ctrl_profile`/`AGENT_KINDS`/`--agent` help | 1 |
  | D·1 handoff `agent_ctrl_profile` | 1 |
  | D·1 `agent_session_root` | 3 |
  | D·1 `session_file_path`/`resolve_session_file` (Kimi dir via index, tar; dsh highest generation) | 3, 4 |
  | D·1 32 MiB cap | 4 |
  | D·1 `is_reported_session_file` | 3 |
  | D·1 exit commands | 4 |
  | D·1 `STARTABLE_AGENTS` (both lists) | 1 |
  | D·1 dsh movable after its run ended | 5 (gap 4) |
  | D·2 kimi `place_session` (untar, rewrite `cwd` + `homedir`, index) | 9, 10 |
  | D·2 dsh `place_session` (decompress, header `cwd`, recompress with checksums, keep N) | 11 |
  | D·2 validation (ids, tar entries) | 7, 9 |
  | D·2 `read_session` | 8, 10, 11, 12 |
  | D·3 unit (warp): tar packing, dsh file pick, profile mapping, size cap | 4, 3, 1, 4 |
  | D·3 unit (agent-ctrl): `workDirKey`/`projectKey` vectors with fixtures, kimi untar + rewrite, dsh round trip, tar validation | 9, 11, 10, 11, 9 |
  | D·3 staging E2E | 13 |

  `move-all` needs no change (spec D·1). Only the `MoveSession` arm is added to its exhaustive match.
- **Placeholder scan:** runtime values in Task 13 (`<job>`, `<id>`, `<worktree>`) come from earlier steps of the same task. Task 12 gives code for the trait default and exact instructions for the two `node.rs` call sites. Its test is described rather than written out, because the fake backend's shape exists only on the pause branch.
- **Type consistency:** `read_session(&self, session_id: &str, cwd: &Path, run_dir: &Path, env: &HashMap<String, String>) -> Result<Option<Vec<u8>>, BackendError>` is used in Tasks 8, 10, 11 and 12. `PlacedSession { resume_ref, cleanup }` matches `backend.rs`. `PreparedHandoff::session_len: u64` feeds `UploadedSession::file_len: u64`. `dsh_generation` (warp) and `generation_of` (agent-ctrl) implement the same rule in two repos.
