# Move a Local Agent Session to AgentCtrl — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A claude, pi, or codex session in a WarpSpeed pane can be moved to a staging AgentCtrl run that resumes the same conversation on the same code, and the pane then shows that run.

**Architecture:** WarpSpeed commits the working tree to a `handoff/…` branch, pushes it, and uploads the agent's session file to a new dashboard endpoint. The dashboard forwards it over a new `ImportSession` gRPC call to the controller. The controller dispatches a `SpawnCommand` carrying the file and branch to the node. The node creates a per-run git worktree, places the file where the agent CLI expects it, resumes, and emits a `resumed` status event. WarpSpeed then exits the local agent and runs `agent-ctrl open <job>` in the same pane.

**Tech Stack:** Rust (tonic/prost, tokio) in `stoz3n-agent-ctrl`; TypeScript (node:http, @grpc/grpc-js) in `stoz3n-agent-dash`; Rust (WarpUI, reqwest blocking) in `warp`.

## Global Constraints

- Session file ≤ 32 MiB (33_554_432 bytes); gRPC decode limits raised to 40 MiB (41_943_040) where the file travels.
- Handoff branch must match `^handoff/[A-Za-z0-9._-]{1,100}$`.
- `session_id`: 1–128 chars of `[A-Za-z0-9-]`. `source_cwd`: ≤ 1024 chars.
- The repo is accepted only if the normalized `repo_url` equals the normalized `url` of a repository in the node's `AGENT_CTRL_WORKSPACES_FILE`. Normalize: lowercase scheme+host, strip trailing `/` and `.git`, strip userinfo.
- Per-run worktree path: `<workspace root>/.runs/<job_id>`. At run end the worktree's work is committed and pushed back to the handoff branch before removal; a failed push keeps the worktree.
- Audit stores the SHA-256 hex of the session file, never its content. The file is not persisted by the controller.
- Status event content after resume: exactly `resumed`.
- Path note sent as the first input after resume: `This session moved from <source_cwd> to <worktree>. Paths in earlier turns refer to the old location.`
- Moved-session agents: `staging-claude-moved`, `staging-pi-moved`, `staging-codex-moved` (permission mode `workspace-write`, interactive).
- Environment is `staging` only.
- Branches: `feat/import-moved-session` in agent-ctrl and agent-dash (off `origin/dev`), `feat/move-session-to-agentctrl` in warp. agent-ctrl/agent-dash push to `origin`; warp pushes to `gitea` and `gitlab`, never `origin`.

## Spec deviations (found while mapping the code)

1. Repositories are node-local, so there is no `GET /repos`. WarpSpeed sends `repo_url` (the push remote URL) and the node validates it. A rejected repo leaves a pushed handoff branch behind, which is reported.
2. The viewer already exists (`agent-ctrl open <job>`, run in a pane by `Workspace::execute_managed_session_command`). No new pane type.
3. No `warpspeed agent-ctrl move` CLI: a CLI cannot address a pane. The GUI action is the only trigger.
4. Import does not require a `task_id` (moved sessions are not ticket work).
5. `managed_sessions_cli.rs` is superseded on `dev` by `crates/managed_agent_sessions`; no prerequisite merge.

---

## Part A — stoz3n-agent-ctrl

Work in a worktree: `git -C T:/projects/SOTZ3N/stoz3n-agent-ctrl worktree add -b feat/import-moved-session T:/projects/SOTZ3N/stoz3n-agent-ctrl-import origin/dev`. Test: `cargo test`, `cargo clippy -- -D warnings`, `cargo fmt -- --check`.

### Task A1: Proto fields, ImportSession RPC, message limits

**Files:**
- Modify: `proto/agentctrl/v1.proto` (service `FleetControl` ~line 24-35; `SpawnCommand` :82-101)
- Modify: where `FleetControlServer::new` and the node's `NodeControlClient`/stream client are built (grep `FleetControlServer::new`, `connect(` in `src/remote/`)
- Copy the updated proto to `T:/projects/SOTZ3N/stoz3n-agent-dash/server/agentctrl/v1.proto` in Part B.

**Interfaces:**
- Produces: `pb::SpawnCommand { resume_session_file: Vec<u8>, handoff_repo_url: String, handoff_branch: String, source_cwd: String, .. }`; `pb::ImportSessionRequest`; `rpc ImportSession(ImportSessionRequest) returns (SpawnAgentResponse)`; `pub const MAX_SESSION_FILE_BYTES: usize = 33_554_432; pub const MAX_GRPC_MESSAGE_BYTES: usize = 41_943_040;` in `src/remote/mod.rs`.

- [ ] **Step 1: Extend the proto**

```proto
// in service FleetControl
  rpc ImportSession(ImportSessionRequest) returns (SpawnAgentResponse);

// in message SpawnCommand, after job_id = 15
  bytes resume_session_file = 16;
  string handoff_repo_url = 17;
  string handoff_branch = 18;
  string source_cwd = 19;

message ImportSessionRequest {
  string node_name = 1;
  string agent_name = 2;
  string session_id = 3;
  bytes session_file = 4;
  string repo_url = 5;
  string handoff_branch = 6;
  string source_cwd = 7;
  string request_id = 8;
  string actor = 9;
  string source = 10;
}
```

- [ ] **Step 2: Raise decode limits** where the file travels: `FleetControlServer::new(svc).max_decoding_message_size(MAX_GRPC_MESSAGE_BYTES)` and on the node's stream client `.max_decoding_message_size(MAX_GRPC_MESSAGE_BYTES)`. Add the two consts to `src/remote/mod.rs`.

- [ ] **Step 3: Fix struct literals.** `cargo build` will flag every `pb::SpawnCommand { .. }` literal (`remote/mod.rs:304 spawn_command`, tests). Add `..Default::default()` or the four empty fields. Add a stub `import_session` to the `FleetControl` impl in `controller.rs` returning `Err(Status::unimplemented("import_session"))` (replaced in A6).

- [ ] **Step 4: Verify** `cargo build && cargo test` → all existing tests pass.

- [ ] **Step 5: Commit** `git commit -am "feat(proto): carry a moved session file and handoff branch"`

### Task A2: Handoff worktree preparation

**Files:**
- Create: `src/workspace/handoff.rs` (convert `src/workspace.rs` to `src/workspace/mod.rs` + `pub mod handoff;`) — or, if you prefer not to move the file, add `#[path = "workspace_handoff.rs"] pub mod handoff;` at the bottom of `src/workspace.rs` and create `src/workspace_handoff.rs`. Use the `#[path]` variant.
- Test: inline `#[cfg(test)] mod tests` in the same file (repo convention, see `workspace.rs:184`).

**Interfaces:**
- Consumes: `WorkspaceConfig { root, repositories: Vec<Repository{name,url,branch}> }`, `WorkspaceGuard::acquire(&Path)`, private `git(cwd,&[&str])` (make it `pub(crate)`).
- Produces:
  - `pub fn normalize_repo_url(url: &str) -> String`
  - `pub fn find_repository<'a>(config: &'a WorkspaceConfig, repo_url: &str) -> Option<&'a Repository>`
  - `pub fn valid_handoff_branch(branch: &str) -> bool`
  - `pub async fn prepare_handoff_worktree(config: &WorkspaceConfig, repo_url: &str, branch: &str, job_id: &str) -> Result<PathBuf, BackendError>`
  - `pub async fn remove_handoff_worktree(config: &WorkspaceConfig, repo_url: &str, worktree: &Path)`
  - `pub fn load_workspace_config_from_env() -> Result<Option<WorkspaceConfig>, BackendError>` (reads `AGENT_CTRL_WORKSPACES_FILE`; factor this out of `prepare_from_env`).

- [ ] **Step 1: Write failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::{Repository, WorkspaceConfig};

    fn run(cwd: &std::path::Path, args: &[&str]) {
        let ok = std::process::Command::new("git").args(args).current_dir(cwd)
            .status().unwrap().success();
        assert!(ok, "git {args:?}");
    }

    #[test]
    fn normalizes_equivalent_urls() {
        assert_eq!(normalize_repo_url("https://User@Git-Dev.Obachan.dev/stozn/warpspeed.git/"),
                   "https://git-dev.obachan.dev/stozn/warpspeed");
        assert_eq!(normalize_repo_url("https://git-dev.obachan.dev/stozn/warpspeed"),
                   "https://git-dev.obachan.dev/stozn/warpspeed");
    }

    #[test]
    fn handoff_branch_shape_is_enforced() {
        assert!(valid_handoff_branch("handoff/20261003-1200-abcd1234"));
        assert!(!valid_handoff_branch("main"));
        assert!(!valid_handoff_branch("handoff/../x"));
        assert!(!valid_handoff_branch("handoff/a b"));
        assert!(!valid_handoff_branch(&format!("handoff/{}", "a".repeat(101))));
    }

    #[tokio::test]
    async fn worktree_checks_out_handoff_branch_and_leaves_checkout_untouched() {
        let tmp = tempfile::tempdir().unwrap();
        let origin = tmp.path().join("origin");
        std::fs::create_dir_all(&origin).unwrap();
        run(&origin, &["init", "-b", "dev"]);
        run(&origin, &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "--allow-empty", "-m", "base"]);
        run(&origin, &["checkout", "-b", "handoff/20261003-1200-abcd"]);
        std::fs::write(origin.join("moved.txt"), "local work").unwrap();
        run(&origin, &["add", "."]);
        run(&origin, &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-m", "handoff"]);
        run(&origin, &["checkout", "dev"]);

        let root = tmp.path().join("root");
        std::fs::create_dir_all(&root).unwrap();
        let url = origin.to_string_lossy().to_string();
        run(&root, &["clone", "--branch", "dev", &url, "repo"]);
        std::fs::write(root.join("repo/dirty.txt"), "shared dirt").unwrap();
        let config = WorkspaceConfig {
            root: root.clone(),
            repositories: vec![Repository { name: "repo".into(), url: url.clone(), branch: "dev".into() }],
        };

        let worktree = prepare_handoff_worktree(&config, &url, "handoff/20261003-1200-abcd", "job-1")
            .await.unwrap();

        assert_eq!(worktree, root.canonicalize().unwrap().join(".runs").join("job-1"));
        assert_eq!(std::fs::read_to_string(worktree.join("moved.txt")).unwrap(), "local work");
        assert!(root.join("repo/dirty.txt").exists());
        let head = std::process::Command::new("git").args(["branch", "--show-current"])
            .current_dir(root.join("repo")).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&head.stdout).trim(), "dev");

        remove_handoff_worktree(&config, &url, &worktree).await;
        assert!(!worktree.exists());
    }

    #[tokio::test]
    async fn unknown_repository_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let config = WorkspaceConfig { root: tmp.path().to_path_buf(), repositories: vec![] };
        let err = prepare_handoff_worktree(&config, "https://evil.example/x.git", "handoff/a", "job-1")
            .await.unwrap_err();
        assert!(err.to_string().contains("not an operator-configured repository"));
    }
}
```

- [ ] **Step 2: Run** `cargo test workspace_handoff` → FAIL (functions missing).

- [ ] **Step 3: Implement**

```rust
//! Per-run worktrees for sessions moved from a developer machine. The shared
//! checkout only gains worktree metadata; its branch and dirty files stay as they are.

use super::{git, Repository, WorkspaceConfig, WorkspaceGuard};
use crate::agent::BackendError;
use std::path::{Path, PathBuf};

pub fn normalize_repo_url(url: &str) -> String {
    let url = url.trim().trim_end_matches('/');
    let url = url.strip_suffix(".git").unwrap_or(url);
    match url.split_once("://") {
        Some((scheme, rest)) => {
            let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
            let host = authority.rsplit('@').next().unwrap_or(authority);
            format!("{}://{}/{}", scheme.to_ascii_lowercase(), host.to_ascii_lowercase(), path)
                .trim_end_matches('/').to_string()
        }
        None => url.to_string(),
    }
}

pub fn find_repository<'a>(config: &'a WorkspaceConfig, repo_url: &str) -> Option<&'a Repository> {
    let wanted = normalize_repo_url(repo_url);
    config.repositories.iter().find(|repo| normalize_repo_url(&repo.url) == wanted)
}

pub fn valid_handoff_branch(branch: &str) -> bool {
    branch.strip_prefix("handoff/").is_some_and(|rest| {
        (1..=100).contains(&rest.len())
            && !rest.contains("..")
            && rest.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    })
}

pub async fn prepare_handoff_worktree(
    config: &WorkspaceConfig,
    repo_url: &str,
    branch: &str,
    job_id: &str,
) -> Result<PathBuf, BackendError> {
    let repo = find_repository(config, repo_url).ok_or_else(|| {
        BackendError::SpawnFailed(format!("{repo_url} is not an operator-configured repository"))
    })?;
    if !valid_handoff_branch(branch) {
        return Err(BackendError::SpawnFailed("invalid handoff branch".into()));
    }
    let root = config.root.canonicalize()?;
    let checkout = root.join(&repo.name);
    let runs = root.join(".runs");
    tokio::fs::create_dir_all(&runs).await?;
    let worktree = runs.join(job_id);
    // Held only while git mutates the shared checkout's metadata; the agent
    // process takes the same root lock for its own lifetime afterwards.
    let _guard = WorkspaceGuard::acquire(&root)?;
    let refspec = format!("+refs/heads/{branch}:refs/remotes/origin/{branch}");
    git(&checkout, &["fetch", "origin", &refspec]).await?;
    let target = worktree.to_string_lossy().to_string();
    git(&checkout, &["worktree", "add", "--detach", &target, &format!("origin/{branch}")]).await?;
    Ok(worktree)
}

pub async fn remove_handoff_worktree(config: &WorkspaceConfig, repo_url: &str, worktree: &Path) {
    let Some(repo) = find_repository(config, repo_url) else { return };
    let Ok(root) = config.root.canonicalize() else { return };
    let target = worktree.to_string_lossy().to_string();
    if let Err(error) = git(&root.join(&repo.name), &["worktree", "remove", "--force", &target]).await {
        tracing::warn!(%error, worktree = %target, "failed to remove handoff worktree");
    }
}
```

Factor `load_workspace_config_from_env()` out of `prepare_from_env` (same parsing, returns `Ok(None)` when the env var is unset). Make `git` `pub(crate)`.

- [ ] **Step 4: Run** `cargo test workspace_handoff` → PASS. (`--detach` keeps the shared checkout free to own the branch name.)

- [ ] **Step 5: Commit** `git add src/workspace.rs src/workspace_handoff.rs && git commit -m "feat(workspace): per-run worktree for a moved session's handoff branch"`

### Task A3: Adapters place the session file and resume (claude, pi)

**Files:**
- Modify: `src/agent/process.rs` (`CliAdapter` trait ~:88-125)
- Modify: `src/agent/claude.rs`, `src/agent/pi.rs`, `src/agent/codex.rs`
- Tests: inline test modules in those files.

**Interfaces:**
- Produces on `CliAdapter`:
  ```rust
  /// Writes a session file moved from another machine where this CLI looks it up,
  /// returning the value later passed to `build_args` as `resume`.
  fn place_session(&self, _file: &[u8], _session_id: &str, _cwd: &Path, _run_dir: &Path)
      -> Result<String, BackendError> {
      Err(BackendError::Unsupported { backend: self.name().to_string(), operation: "place_session".to_string() })
  }
  ```
  (Match the real `BackendError` unsupported variant used in `process.rs:1291-1295`.)
- Home directory: `fn agent_home(cfg_env: &HashMap<String,String>) -> PathBuf` — `HOME` from `cfg.env_vars`, else `std::env::var_os("HOME")`; codex uses `CODEX_HOME` the same way, else `<home>/.codex`. Since `place_session` has no `SpawnConfig`, add the env map parameter: final signature
  `fn place_session(&self, file: &[u8], session_id: &str, cwd: &Path, run_dir: &Path, env: &HashMap<String, String>) -> Result<String, BackendError>`.

- [ ] **Step 1: Failing tests**

`claude.rs`:
```rust
#[test]
fn moved_session_is_placed_in_the_project_dir_for_the_new_cwd() {
    let home = tempfile::tempdir().unwrap();
    let env = HashMap::from([("HOME".to_string(), home.path().display().to_string())]);
    let cwd = std::path::Path::new("/workspaces/.runs/job-1");
    let resume = ClaudeAdapter::default()
        .place_session(b"{\"x\":1}\n", "0b6c-uuid", cwd, home.path(), &env).unwrap();
    assert_eq!(resume, "0b6c-uuid");
    let placed = home.path().join(".claude/projects/-workspaces--runs-job-1/0b6c-uuid.jsonl");
    assert_eq!(std::fs::read(placed).unwrap(), b"{\"x\":1}\n");
}
```
Claude's project slug replaces every non-ASCII-alphanumeric char with `-` (observed: `C:\Users\budel\AppData\Local\Programs\WarpSpeed` -> `C--Users-budel-AppData-Local-Programs-WarpSpeed`). WarpSpeed's `encode_cwd` only replaces `/` and `.` and is wrong for this; implement `fn claude_project_slug(cwd: &Path) -> String` with the full rule.

`pi.rs`:
```rust
#[test]
fn moved_session_resumes_from_its_file_path() {
    let run_dir = tempfile::tempdir().unwrap();
    let path = PiAdapter.place_session(b"line\n", "abc", std::path::Path::new("/w"), run_dir.path(), &HashMap::new()).unwrap();
    assert_eq!(std::path::Path::new(&path), run_dir.path().join("session.jsonl"));
    let mut cfg = test_config(); // existing helper in pi.rs tests; else build SpawnConfig with interactive: true
    cfg.interactive = true;
    let args = PiAdapter.build_args(&cfg, Some(&path));
    let i = args.iter().position(|a| a == "--session").unwrap();
    assert_eq!(args[i + 1], path);
    assert!(!args.contains(&"--no-session".to_string()));
}
```

`codex.rs`:
```rust
#[test]
fn moved_rollout_is_placed_under_codex_home_sessions() {
    let codex_home = tempfile::tempdir().unwrap();
    let env = HashMap::from([("CODEX_HOME".to_string(), codex_home.path().display().to_string())]);
    let id = "019f1829-f9a1-7b21-9ce3-7d6bc7dfa09e";
    let resume = CodexAdapter::default().place_session(b"{}\n", id, std::path::Path::new("/w"), codex_home.path(), &env).unwrap();
    assert_eq!(resume, id);
    let found = walkdir_jsonl(codex_home.path().join("sessions")); // collect *.jsonl recursively with std::fs
    assert_eq!(found.len(), 1);
    assert!(found[0].file_name().unwrap().to_string_lossy().ends_with(&format!("-{id}.jsonl")));
}
```
(Codex finds rollouts by thread id regardless of date folder — verified 2026-10-03 against codex-cli 0.144.4: a rollout copied into `sessions/2026/10/03/` of a fresh `CODEX_HOME` resumed via `thread/resume {threadId}`.)

- [ ] **Step 2: Run** `cargo test place_session moved_` → FAIL.

- [ ] **Step 3: Implement**
  - Trait method as above; `supports_resume()` → `true` for pi and codex.
  - claude: write to `<home>/.claude/projects/<slug>/<session_id>.jsonl` (create dirs), return `session_id`.
  - pi: write `<run_dir>/session.jsonl`, return its path string. In `build_args`, when `resume` is `Some(path)`: push `--session`, `path` and skip the `--no-session` push.
  - codex: write `<codex_home>/sessions/<YYYY>/<MM>/<DD>/rollout-<YYYY-MM-DD>T<HH-MM-SS>-<session_id>.jsonl` using `chrono::Utc::now()`; return `session_id`. `build_args` unchanged (resume happens over the protocol, A4).
  - All three validate `session_id` again: `[A-Za-z0-9-]{1,128}` else `BackendError::SpawnFailed("invalid session id")` — the id becomes a path component.

- [ ] **Step 4: Run** `cargo test` → PASS.

- [ ] **Step 5: Commit** `git commit -am "feat(agents): place a moved session file and resume it for claude, pi and codex"`

### Task A4: Codex app-server resumes a thread

**Files:**
- Modify: `src/agent/process.rs` `spawn_codex_app_server_driver` (~:855; `thread/start` sent at ~:1055-1066; id-2 response handled ~:1075)
- Test: extend the fake protocol server test (~:1647, asserts `thread/start` at ~:1734).

**Interfaces:**
- Consumes: `resume: Option<String>` already passed to `launch(config, resume)`; thread it into the codex driver as `resume_thread_id: Option<String>`.

- [ ] **Step 1: Failing test** — copy the existing codex fake-server test, launch via `backend.resume("thread-moved", config)` and assert the second request is
  `{"method":"thread/resume","params":{"threadId":"thread-moved","cwd":<working_directory>, "approvalPolicy":"never", ...}}` with id 2, and that the fake's response `{"id":2,"result":{"thread":{"id":"thread-moved"}}}` results in `status.session_id == Some("thread-moved")`.

- [ ] **Step 2: Run** → FAIL (driver always sends `thread/start`).

- [ ] **Step 3: Implement** — at the `thread/start` send site:

```rust
let (method, params) = match resume_thread_id.as_deref() {
    Some(thread_id) => ("thread/resume", json!({
        "threadId": thread_id,
        "cwd": cwd,
        "model": model,
        "approvalPolicy": "never",
        "sandbox": sandbox,
        "developerInstructions": developer_instructions,
    })),
    None => ("thread/start", /* existing params unchanged */),
};
```
The id-2 response handler already reads `result.thread.id`; `thread/resume` returns the same `{thread:{id}}` shape (verified in the spike output), so no change there.

- [ ] **Step 4: Run** `cargo test codex` → PASS.

- [ ] **Step 5: Commit** `git commit -am "feat(codex): resume a moved thread over app-server"`

### Task A5: Node runs a moved session

**Files:**
- Modify: `src/agent/runner.rs` (add `resume`), `src/remote/mod.rs` (`spawn_config_from` / `spawn_command` carry the new fields), `src/remote/node.rs` (`handle_spawn` ~:399-460)
- Test: `tests/moved_session.rs` (integration, fake backend) or inline in node.rs if a node test harness exists (`grep -n "#\[cfg(test)\]" src/remote/node.rs`). Prefer the existing harness.

**Interfaces:**
- Consumes: A1 fields, A2 `prepare_handoff_worktree`/`remove_handoff_worktree`/`load_workspace_config_from_env`, A3 `place_session`, A4.
- Produces: `AgentRunner::resume(&self, agent_name: &str, backend: &str, session_id: &str, config: SpawnConfig) -> Result<..same as spawn..>` calling `backend_impl.resume(session_id, config)`; `place_session` exposed through the backend trait (`AgentBackend::place_session(&self, file, session_id, cwd, run_dir, env) -> Result<String, BackendError>`, default `Unsupported`; `CliBackend<A>` forwards to its adapter).

- [ ] **Step 1: Failing test** — with a fake backend that records `resume(session_id, config)` calls and `place_session` args: send a `SpawnCommand` with `resume_session_file = b"x"`, `session_id = "s-1"`, `handoff_repo_url`, `handoff_branch`, `source_cwd = "C:\\src\\warp"` to the node (workspace config pointing at a temp git origin as in A2). Assert:
  1. the backend saw `resume("s-1", config)` with `config.working_directory == <root>/.runs/<job_id>`,
  2. output chunks include `{output_type: Status, content: "resumed"}` before any agent output,
  3. the first input delivered to the run is `This session moved from C:\src\warp to <worktree>. Paths in earlier turns refer to the old location.`
  4. a `SpawnCommand` with a file but an unknown repo URL is NACKed with an error containing `not an operator-configured repository` and the backend saw no call.

- [ ] **Step 2: Run** → FAIL.

- [ ] **Step 3: Implement** in `handle_spawn`, before `runner.spawn`:

```rust
let moved = !cmd.resume_session_file.is_empty();
let mut config = super::spawn_config_from(&cmd);
let mut handoff = None;
if moved {
    let workspaces = crate::workspace::load_workspace_config_from_env()?
        .ok_or_else(|| BackendError::SpawnFailed("node has no workspace configuration".into()))?;
    let worktree = crate::workspace::handoff::prepare_handoff_worktree(
        &workspaces, &cmd.handoff_repo_url, &cmd.handoff_branch, &cmd.job_id).await?;
    // Outside the worktree so the agent never sees or commits it.
    let run_dir = worktree.with_extension("session");
    tokio::fs::create_dir_all(&run_dir).await?;
    let resume_ref = runner.place_session(&cmd.agent_name, backend, &cmd.resume_session_file,
        &cmd.session_id, &worktree, &run_dir, &config.env_vars)?;
    config.working_directory = worktree.clone();
    config.prompt = String::new(); // no first turn; the path note is sent after `resumed`
    handoff = Some((workspaces, worktree, resume_ref));
}
let spawned = match &handoff {
    Some((_, _, resume_ref)) => runner.resume(&cmd.agent_name, backend, resume_ref, config).await,
    None => runner.spawn(&cmd.agent_name, backend, config).await,
};
```
After the existing `runtime.active.insert(..)` and ack: emit the status chunk exactly like `node.rs:366-389` with `content: "resumed".into()`, then send the path note through the same path `SendInputCommand` uses (`ProcessControl::send_input`). When the run's monitor observes a terminal state: commit any dirty files in the worktree as `agent-ctrl run <job_id> end`, push `HEAD` to `refs/heads/<handoff_branch>` on origin, then call `remove_handoff_worktree` and delete `<worktree>.session/` (hook into the place that removes the run from `runtime.active`). If the push fails, keep the worktree, emit a `status` chunk `handoff push failed; worktree kept at <path>`, and still delete `<worktree>.session/`. (Ruling 2026-10-03: the agent's unpushed work must never be discarded.) On any error after the worktree was created, remove it before NACKing. Interactive empty prompt: confirm `launch` skips the first stdin turn when `config.prompt` is empty (`process.rs:425-429`); if it doesn't, guard it with `if !config.prompt.is_empty()`.

- [ ] **Step 4: Run** `cargo test` → PASS.

- [ ] **Step 5: Commit** `git commit -am "feat(node): run a moved session in a handoff worktree and report resumed"`

### Task A6: Controller ImportSession

**Files:**
- Modify: `src/remote/controller.rs` (`spawn_agent` :1505 as the model; `DispatchOptions` :53-59; `dispatch_remote` :372-455)
- Test: alongside existing `spawn_agent` tests (grep `spawn_agent(` in `controller.rs` tests / `tests/remote_*.rs`).

**Interfaces:**
- Consumes: A1 proto.
- Produces: `DispatchOptions.moved: Option<&'a MovedSession>` where
  `pub(crate) struct MovedSession { pub session_id: String, pub file: Vec<u8>, pub repo_url: String, pub branch: String, pub source_cwd: String }`; `dispatch_remote` copies those into the outgoing `SpawnCommand` (after `super::spawn_command(..)` at :442) and forces `interactive = true`.

- [ ] **Step 1: Failing tests** for `import_session`:
  - valid request → `SpawnAgentResponse { success: true, job_id }` and the node receives a `SpawnCommand` with all four new fields set and `interactive == true`;
  - `session_file` > `MAX_SESSION_FILE_BYTES` → `Status::invalid_argument`;
  - bad `handoff_branch` / bad `session_id` / `source_cwd` > 1024 → `invalid_argument`;
  - agent without `allow_interactive` → same rejection `spawn_agent` gives.
  - the audit/control record (whatever `mutation_context` + command persistence writes for `spawn_agent`) contains `session_sha256` = hex SHA-256 of the file and does not contain the file bytes.

- [ ] **Step 2: Run** → FAIL.

- [ ] **Step 3: Implement** `import_session` by mirroring `spawn_agent`: `mutation_context(&req.request_id, &req.actor, &req.source)?`, validation per Global Constraints (reuse `crate::workspace::handoff::valid_handoff_branch`), no task attachment, `interactive_override: Some(true)`, `moved: Some(&moved)`. Compute `sha2::Sha256` (add `sha2` if not already a dependency — check `Cargo.toml`) and store it in the command record's details the same way `spawn_agent` stores its target/agent. Never log the file.

- [ ] **Step 4: Run** `cargo test && cargo clippy -- -D warnings && cargo fmt -- --check` → clean.

- [ ] **Step 5: Commit** `git commit -am "feat(controller): import a moved session as an interactive run"`

### Task A7: Staging config for moved sessions

**Files:**
- Modify: `deploy/k8s/render.py` (`workspace_repositories` :100-104; agent configs), `deploy/k8s/templates/configmap.tmpl.yaml` (next to `staging-claude-interactive` :310)
- Test: `deploy/k8s/test_shared_workspaces.py`

- [ ] **Step 1: Failing test** in `test_shared_workspaces.py`: rendered staging config contains repository `{"name": "warpspeed", "url": "https://git-dev.obachan.dev/stozn/warpspeed.git", "branch": "dev"}` and agents `staging-claude-moved`, `staging-pi-moved`, `staging-codex-moved` each with `interactive: true`, `allow_interactive: true`, `permission_mode: workspace-write`, `working_directory: "{default_working_directory}"`, and for pi `AGENT_CTRL_PI_SESSION: "true"`.
- [ ] **Step 2: Run** `python -m pytest deploy/k8s/test_shared_workspaces.py -q` → FAIL.
- [ ] **Step 3: Implement** by copying `staging-claude-interactive`'s block (and the pi/codex inspector blocks) with the new names, `workspace-write`, and descriptions `Interactive <agent> session moved from WarpSpeed`.
- [ ] **Step 4: Run** → PASS.
- [ ] **Step 5: Commit** `git commit -am "deploy(staging): warpspeed workspace and moved-session agents"`. Push the branch: `git push -u origin feat/import-moved-session`. **Do not deploy** — rollout to staging is a separate, confirmed step (Task D1).

---

## Part B — stoz3n-agent-dash

Worktree: `git -C T:/projects/SOTZ3N/stoz3n-agent-dash worktree add -b feat/import-moved-session T:/projects/SOTZ3N/stoz3n-agent-dash-import origin/dev`. Test: `yarn test:agent-control`.

### Task B1: `POST /api/agent-control/runs/import`

**Files:**
- Modify: `server/agentctrl/v1.proto` (copy of A1's proto)
- Modify: `server/agent-control.ts` (route before the job-id regex at ~:69; new `importRun` beside `launchRun` :199; `grpcClient()` :572 options)
- Modify: `server/openapi.ts` (path next to :155; schema next to `ManagedLaunchInput` :590)
- Test: `scripts/check-agent-control.mjs` (fake service registration ~:172-205)

**Interfaces:**
- Consumes: gRPC `ImportSession(ImportSessionRequest) returns (SpawnAgentResponse)`.
- Produces: HTTP 202 `commandResponse(requestId, "import", result, headers, job_id, agent_id)`; 413 `payload_too_large`; 415 unless `multipart/form-data`; 422 `validation_error`.

- [ ] **Step 1: Failing checks** in `check-agent-control.mjs`: register `ImportSession: mutation` in the fake service, then:

```js
function importForm(overrides = {}) {
  const form = new FormData();
  const fields = { agent_name: "staging-claude-moved", session_id: "0b6c-uuid",
    repo_url: "https://git-dev.obachan.dev/stozn/warpspeed.git",
    handoff_branch: "handoff/20261003-1200-0b6c", source_cwd: "C:\\src\\warp", ...overrides };
  for (const [k, v] of Object.entries(fields)) if (v !== undefined) form.set(k, v);
  if (!("session_file" in overrides)) form.set("session_file", new Blob(["{\"x\":1}\n"]), "session.jsonl");
  return form;
}
function importCall(form, id) {
  return call("/api/agent-control/runs/import", { method: "POST", body: form,
    headers: { authorization: "Bearer admin-token", "idempotency-key": id, "x-stozn-client": "warpspeed" } });
}

check("import forwards the session file and handoff to ImportSession", async () => {
  const response = await importCall(importForm(), "import-request-0001");
  assert.equal(response.status, 202);
  assert.equal((await response.json()).status, "accepted");
  const sent = requests.at(-1);
  assert.equal(sent.agent_name, "staging-claude-moved");
  assert.equal(Buffer.from(sent.session_file).toString(), "{\"x\":1}\n");
  assert.equal(sent.handoff_branch, "handoff/20261003-1200-0b6c");
  assert.equal(sent.source, "warpspeed");
});
check("import rejects a bad handoff branch", async () => {
  assert.equal((await importCall(importForm({ handoff_branch: "main" }), "import-request-0002")).status, 422);
});
check("import rejects unknown fields", async () => {
  assert.equal((await importCall(importForm({ prompt: "x" }), "import-request-0003")).status, 422);
});
check("import rejects files over 32 MiB", async () => {
  const form = importForm({ session_file: undefined });
  form.set("session_file", new Blob([new Uint8Array(33_554_433)]), "session.jsonl");
  assert.equal((await importCall(form, "import-request-0004")).status, 413);
});
check("import is not routed as a job id", async () => {
  const response = await importCall(importForm(), "import-request-0005");
  assert.notEqual((await response.json()).error?.code, "invalid_job_id");
});
```
(Match the existing helpers' real auth token / headers — copy them from `jsonMutation`.)

- [ ] **Step 2: Run** `yarn test:agent-control` → FAIL.

- [ ] **Step 3: Implement**

```ts
const MAX_SESSION_FILE_BYTES = 33_554_432;
const MAX_IMPORT_BODY_BYTES = MAX_SESSION_FILE_BYTES + 65_536;
const HANDOFF_BRANCH = /^handoff\/(?!.*\.\.)[A-Za-z0-9._-]{1,100}$/;
const SESSION_ID = /^[A-Za-z0-9-]{1,128}$/;
const IMPORT_FIELDS = new Set(["agent_name", "session_id", "repo_url", "handoff_branch", "source_cwd", "session_file", "node_name"]);

async function importRun(request, claims, config, db, headers) {
  requireMutationReady(config);
  await requireFreshMutationTarget(db, config);
  if (!(request.headers.get("content-type") ?? "").startsWith("multipart/form-data")) {
    throw serviceError(415, "multipart/form-data required", "unsupported_media_type");
  }
  const declared = Number(request.headers.get("content-length") ?? "0");
  if (declared > MAX_IMPORT_BODY_BYTES) throw serviceError(413, "session file too large", "payload_too_large");
  rateLimit(claims.sub);
  const requestId = requireIdempotencyKey(request);
  const form = await request.formData();
  for (const key of form.keys()) if (!IMPORT_FIELDS.has(key)) throw serviceError(422, `unknown field ${key}`, "validation_error");
  const text = (name: string, max: number, required = true) => { /* string field, trimmed length check, else 422 */ };
  const agentName = text("agent_name", 128);
  const sessionId = text("session_id", 128);
  if (!SESSION_ID.test(sessionId)) throw serviceError(422, "session_id is invalid", "validation_error");
  const repoUrl = text("repo_url", 512);
  const branch = text("handoff_branch", 108);
  if (!HANDOFF_BRANCH.test(branch)) throw serviceError(422, "handoff_branch is invalid", "validation_error");
  const sourceCwd = text("source_cwd", 1024);
  const nodeName = text("node_name", 128, false);
  const file = form.get("session_file");
  if (!(file instanceof Blob) || file.size === 0) throw serviceError(422, "session_file is required", "validation_error");
  if (file.size > MAX_SESSION_FILE_BYTES) throw serviceError(413, "session file too large", "payload_too_large");
  const result = await unary(config, "ImportSession", {
    node_name: nodeName ?? "", agent_name: agentName, session_id: sessionId,
    session_file: Buffer.from(await file.arrayBuffer()), repo_url: repoUrl,
    handoff_branch: branch, source_cwd: sourceCwd, request_id: requestId,
    actor: claims.sub.slice(0, 128), source: requestSource(request),
  });
  return commandResponse(requestId, "import", result, headers, nullableString(result.job_id), nullableString(result.agent_id));
}
```
Route it in `handleAgentControlApi` before the job-id regex:
`if (request.method === "POST" && url.pathname === "/api/agent-control/runs/import") return importRun(...)`.
In `grpcClient()` pass channel options `{"grpc.max_send_message_length": 41_943_040, "grpc.max_receive_message_length": 41_943_040}`. Also bound `readBody` in `server/index.ts` to `MAX_IMPORT_BODY_BYTES` for this path, or globally if every other route stays far below it: reject with 413 once the buffered size exceeds it (it currently buffers unbounded).
OpenAPI: add the path with `requestBody.content["multipart/form-data"]` schema `ManagedImportInput` (`session_file` `format: binary`), responses 202/413/415/422/503 like the start route.

- [ ] **Step 4: Run** `yarn test:agent-control` (and `yarn typecheck`/`yarn lint` if present in package.json) → PASS.

- [ ] **Step 5: Commit & push** `git commit -am "feat(agent-control): import a moved WarpSpeed session" && git push -u origin feat/import-moved-session`

---

## Part C — warp (WarpSpeed)

Worktree: `T:/projects/dev-ops/warp-move-to-agentctrl` (branch `feat/move-session-to-agentctrl`). Build cache: set `CARGO_TARGET_DIR` to a sibling worktree's target and `mkdir <wt>/target/debug` first. Never restart running WarpSpeed. Stage explicit paths only.

### Task C1: Client import and resume wait

**Files:**
- Modify: `crates/managed_agent_sessions/src/lib.rs`
- Test: `crates/managed_agent_sessions/src/tests.rs` (uses `http_fixture` / `fixture_profile`)

**Interfaces:**
- Produces:
  ```rust
  pub struct MovedSessionImport<'a> {
      pub agent_name: &'a str,
      pub session_id: &'a str,
      pub session_file: Vec<u8>,
      pub repo_url: &'a str,
      pub handoff_branch: &'a str,
      pub source_cwd: &'a str,
      pub idempotency_key: String,
  }
  pub const MAX_SESSION_FILE_BYTES: usize = 33_554_432;
  pub fn import_moved_session(environment: ManagedEnvironment, import: MovedSessionImport<'_>) -> Result<String /* job_id */>;
  pub fn wait_until_resumed(environment: ManagedEnvironment, job_id: &str, timeout: Duration) -> Result<()>;
  ```
  Internally: `fn import_moved_session_with(profile: &EnvironmentProfile, token: &str, import) -> Result<String>` and `fn wait_until_resumed_with(client, profile, token, job_id, deadline) -> Result<()>` so tests can use the fixtures.

- [ ] **Step 1: Failing tests**
  - `import_sends_multipart_and_returns_job_id`: fixture server returns `{"status":"accepted","job_id":"job-1"}`; assert returned `"job-1"`; assert the captured request has `Content-Type: multipart/form-data`, `Idempotency-Key` equal to the given key, `X-Stozn-Client: warpspeed`, and parts `agent_name`, `session_id`, `repo_url`, `handoff_branch`, `source_cwd`, `session_file`. (If `http_fixture` cannot capture requests, extend it to return the raw request bytes through a channel.)
  - `import_refuses_oversized_file_before_sending`: `session_file` of `MAX_SESSION_FILE_BYTES + 1` bytes → error contains `32 MiB`, no request made.
  - `resume_wait_succeeds_on_resumed_status`: event page `{"items":[{"event_id":"e1","job_id":"job-1","sequence":1,"timestamp":"t","type":"status","content":"resumed","redacted":false,"truncated":false}],"next_cursor":"c1","terminal":false,"gap":false,"earliest_cursor":null}` → `Ok(())`.
  - `resume_wait_fails_on_terminal_page`: `terminal: true` without `resumed` → error contains `ended before resuming`.

- [ ] **Step 2: Run** `cargo nextest run -p managed_agent_sessions` → FAIL.

- [ ] **Step 3: Implement** with `reqwest::blocking::multipart::Form` (`.text(..)` per field, `.part("session_file", Part::bytes(file).file_name("session.jsonl"))`), `send(profile, ..)` for destination validation, `ensure_command_accepted`. `wait_until_resumed_with` loops `fetch_event_page(.., wait_ms = 15_000)` until a `status` event with content `resumed` (→ Ok), a terminal page (→ `bail!("managed run {job_id} ended before resuming")`), or the deadline (→ `bail!("managed run {job_id} did not resume within {secs}s")`). Use a `Client::builder().timeout(Duration::from_secs(25))` like `attach`.

- [ ] **Step 4: Run** → PASS.

- [ ] **Step 5: Commit** `git add crates/managed_agent_sessions/src/lib.rs crates/managed_agent_sessions/src/tests.rs && git commit -m "feat(agent-ctrl): upload a moved session and wait for it to resume"`

### Task C2: Handoff preparation (session file + handoff branch)

**Files:**
- Create: `app/src/terminal/agent_session_handoff.rs`, `app/src/terminal/agent_session_handoff_tests.rs`
- Modify: `app/src/terminal/mod.rs` (`pub(crate) mod agent_session_handoff;` behind `#[cfg(feature = "local_fs")]` like neighbouring fs modules)

**Interfaces:**
- Consumes: `CLIAgent`, `claude_transcript::{claude_config_dir, encode_cwd}`, `warp_util::git::run_git_command_with_env`-style helpers (or `std::process::Command` run inside a background task — this module is called off the main thread).
- Produces:
  ```rust
  pub(crate) struct HandoffSource { pub agent: CLIAgent, pub session_id: String, pub cwd: PathBuf }
  pub(crate) struct PreparedHandoff {
      pub agent_name: &'static str,     // staging-{claude,pi,codex}-moved
      pub session_file: Vec<u8>,
      pub repo_url: String,
      pub remote: String,
      pub branch: String,
      pub commit: String,
  }
  pub(crate) fn moved_agent_name(agent: CLIAgent) -> Option<&'static str>;
  pub(crate) fn session_file_path(agent: CLIAgent, session_id: &str, cwd: &Path, home: &Path) -> Result<PathBuf>;
  pub(crate) fn handoff_branch_name(now: chrono::DateTime<chrono::Local>, session_id: &str) -> String;
  pub(crate) fn create_handoff_commit(repo: &Path) -> Result<String>; // returns commit sha
  pub(crate) fn prepare_handoff(source: &HandoffSource, home: &Path, now: chrono::DateTime<chrono::Local>) -> Result<PreparedHandoff>; // reads file, creates commit, pushes
  ```

- [ ] **Step 1: Failing tests** (tempdir repos with `std::process::Command::new("git")`):
  - `moved_agent_names`: Claude → `staging-claude-moved`, Pi → `staging-pi-moved`, Codex → `staging-codex-moved`, Gemini → None.
  - `claude_session_path_uses_project_slug`: `home/.claude/projects/<slug>/<id>.jsonl` where slug replaces EVERY non-ASCII-alphanumeric char with `-` (observed on this machine: `C:\Users\budel\AppData\Local\Programs\WarpSpeed` -> `C--Users-budel-AppData-Local-Programs-WarpSpeed`; `T:\projects\SOTZ3N\stoz3n-agent-dash\.claude\worktrees\x` -> `T--projects-SOTZ3N-stoz3n-agent-dash--claude-worktrees-x`). Do NOT use the existing `encode_cwd` (it only replaces `/` and `.`, wrong on Windows); add `claude_project_slug` in the handoff module. Test with a Windows-style path string and a Unix one (create the file; assert found). Respect `CLAUDE_CONFIG_DIR` the same way `claude_config_dir()` does — pass the resolved config dir in rather than reading env inside the test.
  - `pi_session_path_matches_id_in_project_dir`: create `home/.pi/agent/sessions/<pi slug>/2026-10-03T10-00-00-000Z_<id>.jsonl`; pi slug = `"--" + cwd with every '/', '\\' and ':' replaced by '-' + "--"` (matches the observed `--T--projects-dev-ops--`; verify against the real dir listing on this machine before finalizing). Assert found; a missing id → error contains `no pi session file`.
  - `codex_session_path_found_by_id_anywhere_under_sessions`: create `home/.codex/sessions/2026/06/30/rollout-2026-06-30T12-53-51-<id>.jsonl`; assert found.
  - `handoff_branch_name_shape`: `handoff/20261003-1200-0b6c1234` for session id `0b6c1234-ffff-…` (first 8 chars), matches the server regex.
  - `handoff_commit_includes_untracked_and_leaves_repo_untouched`: repo with one commit, a modified tracked file, an untracked file, an ignored file (`.gitignore`), and one staged change. Record `git status --porcelain=v1`, `git rev-parse HEAD`, `git diff --cached`, branch name. Call `create_handoff_commit`. Assert: all four are byte-identical afterwards; `git show --name-only <sha>` lists the modified and untracked file, not the ignored one; `git rev-parse <sha>^` == HEAD.

- [ ] **Step 2: Run** `cargo nextest run -p warp agent_session_handoff` → FAIL.

- [ ] **Step 3: Implement**
  - `create_handoff_commit`: temp index file in `std::env::temp_dir()` (`tempfile::NamedTempFile`), env `GIT_INDEX_FILE=<tmp>`; `git read-tree HEAD`; `git add -A`; `tree = git write-tree`; `sha = git commit-tree <tree> -p HEAD -m "WarpSpeed handoff of <session_id>"` (pass the message as an argument; set `GIT_AUTHOR_*`/`GIT_COMMITTER_*` only if unset config makes it fail — rely on user config). The real index and working tree are never written.
  - remote selection: `git remote` list; pick `gitea` if present else `origin`; `repo_url = git remote get-url <remote>`. (The node rejects URLs it doesn't know, so no client-side allow-list.)
  - push: `git push <remote> <sha>:refs/heads/<branch>`.
  - file read: refuse > `managed_agent_sessions::MAX_SESSION_FILE_BYTES`.
  - Order in `prepare_handoff`: resolve + read file and check size first, then commit, then push — nothing is pushed if the file is missing or too large.

- [ ] **Step 4: Run** → PASS.

- [ ] **Step 5: Commit** explicit paths: `git add app/src/terminal/agent_session_handoff.rs app/src/terminal/agent_session_handoff_tests.rs app/src/terminal/mod.rs && git commit -m "feat(terminal): prepare a handoff branch and session file for a moved agent"`

### Task C3: "Move to AgentCtrl" action, menu, palette, cut-over

**Files:**
- Modify: `app/src/terminal/view/action.rs` (`TerminalAction` :206 area + Debug :572), `app/src/terminal/view.rs` (handler near :27415; list near :27076), `app/src/terminal/view/pane_impl.rs` (overflow menu :779 area; predicate next to `can_detach_session` :654)
- Modify: `app/src/workspace/action.rs` (:285-288 area + "requires save" match :1066), `app/src/workspace/view.rs` (handler near :25551; context key near :27880; new `move_session_to_agent_ctrl_for_terminal_view` near `detach_session_for_terminal_view` :13722), `app/src/workspace/mod.rs` (binding near :1011)
- Test: `app/src/workspace/view_tests.rs` (pattern ~:5455 menu tests, ~:5660 session tests), `app/src/workspace/action_tests.rs`

**Interfaces:**
- Consumes: C1 `import_moved_session`, `wait_until_resumed`; C2 `prepare_handoff`, `moved_agent_name`; `CLIAgentSessionsModel::session` / `session_id`; `TerminalView::active_session_path_if_local`; `submit_text_to_cli_agent_pty` (`use_agent_footer/mod.rs:745`); `Workspace::execute_managed_session_command` (:18874); toasts as in `detach_session_for_terminal_view`.
- Produces: `TerminalAction::MoveSessionToAgentCtrl`; `WorkspaceAction::{MoveSessionToAgentCtrl, MoveSessionToAgentCtrlForTerminalView { terminal_view_id: EntityId }}`; context key `Workspace_PaneCanMoveSessionToAgentCtrl`; binding `workspace:move_session_to_agent_ctrl` labelled `Move session to AgentCtrl`.

- [ ] **Step 1: Failing tests**
  - menu: a terminal with a `CLIAgentSession { agent: CLIAgent::Claude, session_context: { session_id: Some("0b6c…"), cwd: Some(..) }, remote_host: None }` shows `Move session to AgentCtrl` mapped to `TerminalAction::MoveSessionToAgentCtrl`; with `CLIAgent::Gemini`, with no session id, or with `remote_host: Some(..)` it is absent.
  - context key: `Workspace_PaneCanMoveSessionToAgentCtrl` set iff the focused pane qualifies.
  - state machine (pure function, no network): extract
    ```rust
    pub(crate) enum MoveStep { Preparing, Importing, WaitingForResume { job_id: String }, CuttingOver { job_id: String }, Done { job_id: String }, Failed { reason: String, pushed_branch: Option<String> } }
    ```
    and `fn move_failure_message(step_reached, error, pushed_branch) -> String`; test that a failure before `CuttingOver` produces a message containing `local session is still running` and the pushed branch name when present.

- [ ] **Step 2: Run** `cargo nextest run -p warp move_session` → FAIL.

- [ ] **Step 3: Implement**
  - Predicate `can_move_session_to_agent_ctrl(ctx)`: local session (`remote_host.is_none()`), `moved_agent_name(agent).is_some()`, `session_id` known, `active_session_path_if_local(ctx).is_some()`.
  - Menu item + `TerminalAction` → `ctx.dispatch_typed_action_deferred(WorkspaceAction::MoveSessionToAgentCtrlForTerminalView { terminal_view_id: self.view_id })` (same shape as DetachSession).
  - Workspace handler: show a confirm dialog (reuse the existing confirmation dialog used elsewhere in `workspace/view.rs` — grep `ConfirmationDialog`/`show_confirmation`; if none fits, use an ephemeral toast with an action button) saying `Move this <Agent> session to AgentCtrl staging? The local agent exits once the cloud run has resumed.` On confirm:
    ```rust
    let source = HandoffSource { agent, session_id, cwd };
    ctx.spawn(async move {
        let home = dirs::home_dir().context("home directory is unavailable")?;
        let prepared = prepare_handoff(&source, &home, chrono::Local::now())?;
        let job_id = import_moved_session(ManagedEnvironment::Staging, MovedSessionImport {
            agent_name: prepared.agent_name, session_id: &source.session_id,
            session_file: prepared.session_file, repo_url: &prepared.repo_url,
            handoff_branch: &prepared.branch, source_cwd: &source.cwd.display().to_string(),
            idempotency_key: format!("move-{}-{}", source.session_id, &prepared.commit[..12]),
        }).map_err(|e| (e, Some(prepared.branch.clone())))?;
        wait_until_resumed(ManagedEnvironment::Staging, &job_id, Duration::from_secs(120))
            .map_err(|e| (e, Some(prepared.branch.clone())))?;
        Ok(job_id)
    }, move |workspace, result, ctx| { /* on Ok: cut over; on Err: error toast via move_failure_message */ });
    ```
    (Wrap the blocking calls the way `app/src/stozn_auth.rs:125,137` does.)
  - Cut-over on Ok(job_id): find the terminal view by id; `submit_text_to_cli_agent_pty("/exit")` for Claude and Codex, `"/quit"` for Pi; then after the CLI agent session ends (subscribe to the `CLIAgentSessionsModel` removal for that view, 10 s timer fallback → if still running, toast `local agent did not exit; close it manually` and stop) run `execute_managed_session_command` with `ManagedSessionDeepLink`-equivalent arguments `["agent-ctrl", "open", job_id, "--environment", "staging"]` in that pane, and show a success toast `Moved to AgentCtrl as <job_id>`.
  - Palette binding in `workspace/mod.rs` with context predicate `id!("Workspace") & id!("Workspace_PaneCanMoveSessionToAgentCtrl")`; add both workspace actions to the "requires save" match next to DetachSession.

- [ ] **Step 4: Run** `cargo nextest run -p warp move_session`, then `./script/format` and `cargo clippy --workspace --all-targets --all-features --tests -- -D warnings` (WSL/Docker if libclang is needed for some crate — see memory) → clean.

- [ ] **Step 5: Commit + push** explicit paths, then `git push gitea feat/move-session-to-agentctrl && git push gitlab feat/move-session-to-agentctrl`.

---

## Part D — Rollout and end-to-end (needs the user's go-ahead)

### Task D1: Deploy and prove it on staging

- [ ] Merge A and B into their `dev` branches through PRs on git-dev.obachan.dev once reviewed; let their pipelines deploy staging (or deploy by hand as before). **Ask the user before deploying.**
- [ ] Install WarpSpeed locally via `script/windows/install-warpspeed.ps1` (never restart running WarpSpeed windows; the new build applies to newly opened windows).
- [ ] For each of claude, pi, codex: start the agent in a pane inside the warpspeed repo, tell it a codeword, make an uncommitted edit, run "Move session to AgentCtrl". Expect: `handoff/…` branch on gitea containing the edit; dashboard run of `staging-<agent>-moved`; `resumed` status event; the local agent exits; the pane shows `agent-ctrl open <job>`; `send what was the codeword?` → the run answers it.
- [ ] Failure drill: move from a repo not in the staging workspace list → error toast names the pushed branch and says the local session is still running.
