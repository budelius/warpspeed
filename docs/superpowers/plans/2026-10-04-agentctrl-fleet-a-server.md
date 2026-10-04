# AgentCtrl Fleet — Plan A (agent-ctrl + dashboard)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Several interactive agent runs per node at once, each in its own worktree and branch, startable on any configured repo from the dashboard, with the IWAKAN and JustVibe repositories configured on staging.

**Architecture:** The root-wide `/workspace` lock is replaced by a per-checkout lock (held only around git metadata ops and for shared-checkout workers) and a per-worktree lock (held by each interactive run). Every interactive run — fresh or moved — lives in `<root>/.runs/<job_id>`; fresh runs branch `agent/<job_id>` off `origin/<base>` and push back at run end through the existing handoff push-back. `SpawnAgentRequest` gains `repo_url`/`base_branch`, the node reports its repositories (with a startup reachability check) in its registration, and the dashboard exposes `GET /repos`, the new launch fields, and a 409 for a node at capacity.

**Tech Stack:** Rust (tonic/prost, tokio) in `stoz3n-agent-ctrl`; TypeScript (node:http, @grpc/grpc-js) in `stoz3n-agent-dash`; Python render/tests for k8s config.

## Global Constraints

- Lock files: checkout (working-tree) lock `<root>/.locks/<repo_name>.lock` held by shared-checkout runs for their lifetime; checkout METADATA lock `<root>/.locks/<repo_name>.git.lock` held briefly around fetch / worktree add|remove / branch -D (metadata ops never block on a running shared agent); worktree lock `<root>/.runs/<job_id>.lock`. The root lock `<root>/.agent-ctrl-workspace.lock` remains ONLY for `prepare()` (initial clones). (Spec said `<root>/<repo>/.agent-ctrl-checkout.lock`; moved out of the checkout so the lock file never shows up as an untracked file.)
- Fresh interactive run: worktree `<root>/.runs/<job_id>`, branch `agent/<job_id>` from `origin/<base>`; `<base>` defaults to the repo's configured `branch`.
- `base_branch`: `^[A-Za-z0-9._/-]{1,100}$`, no `..`, no leading `-` or `/`, no component starting with `.`.
- `task_id` is optional when `repo_url` is present (fleet runs are not ticket work); the dashboard skips task resolution and forwards empty task fields, as `import` does.
- `repo_url`: 1..512 chars; accepted only if `normalize_repo_url(repo_url)` equals a configured repository's normalized url; `git@host:path(.git)` normalizes to `https://host/path`.
- Concurrency cap env `AGENT_CTRL_MAX_INTERACTIVE_RUNS` (staging: 4; unset = 4). Over the cap: node NACK `node at capacity` → controller `Status::resource_exhausted("node at capacity")` → dashboard 409 `{"error":"node at capacity","code":"node_at_capacity"}`.
- Run-end push-back unchanged: dirty tree committed as `agent-ctrl run <job_id> end`, pushed to the run's branch; failed push keeps the worktree and emits `handoff push failed; worktree kept at <path>`.
- Startup sweep: delete `.runs/*.session`; remove a `.runs/<job>` worktree only if its lock can be taken AND `git rev-parse HEAD == git rev-parse origin/<branch>`; otherwise keep and log.
- Interactive rw profiles: `staging-claude-interactive-rw`, `staging-pi-interactive-rw`, `staging-codex-interactive-rw` (renamed from `staging-*-moved`), `operator_control.requires_repo: true` → a launch without `repo_url` is rejected `agent requires repo_url`.
- Staging repositories: the 7 existing + IWAKAN (13) + JustVibe (12) listed in Task A7, each with its default branch.
- Node resources: limits memory `12Gi` (fallback 8Gi + cap 3 if the pod will not schedule).
- Branches: `feat/fleet-runs` in both agent-ctrl and agent-dash off `origin/dev`; push to `origin` (git-dev.obachan.dev). Never `git add -A`.

---

## Part A — stoz3n-agent-ctrl

Worktree: `git -C T:/projects/SOTZ3N/stoz3n-agent-ctrl worktree add -b feat/fleet-runs T:/projects/SOTZ3N/stoz3n-agent-ctrl-fleet origin/dev` (origin/dev = 7f92cfb). Cargo: `CARGO_TARGET_DIR=T:/projects/SOTZ3N/stoz3n-agent-ctrl/target`. Tests: `cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt -- --check`, `python -m unittest discover -s deploy/k8s -p "test_*.py"`.

### Task A1: Proto additions

**Files:** Modify `proto/agentctrl/v1.proto`; fix struct literals the build flags.

**Produces:**
```proto
// FleetControl (after ImportSession, :36)
rpc ListRepositories(ListRepositoriesRequest) returns (ListRepositoriesResponse);

message RepositoryInfo { string name = 1; string url = 2; string branch = 3; bool reachable = 4; string error = 5; }
message ListRepositoriesRequest {}
message ListRepositoriesResponse { repeated RepositoryInfo repositories = 1; }

// NodeRegistration (:65-75): next tag 10
repeated RepositoryInfo repositories = 10;

// SpawnCommand (:83-107): next tag 20
string run_repo_url = 20;
string run_base_branch = 21;

// SpawnAgentRequest (:228-243): next tag 11
string repo_url = 11;
string base_branch = 12;

// JobInfo (:387-418): next tag 31
string repo_url = 31;
string branch = 32;
```

- [ ] Step 1: add the proto text above. Step 2: `cargo build`; add `..Default::default()` or the new fields to every `pb::SpawnCommand`/`pb::NodeRegistration`/`pb::JobInfo` literal that fails. Add a `list_repositories` stub to the `FleetControl` impl in `src/remote/controller.rs` returning `Err(Status::unimplemented("list_repositories"))` (replaced in A6). Step 3: `cargo test` green. Step 4: commit `feat(proto): repositories, start-on-repo fields, run branch`.

### Task A2: Per-checkout and per-worktree locks

**Files:** Modify `src/workspace.rs` (`WorkspaceGuard` :30-54, `lock_for_directory` :56-79), `src/agent/process.rs` (:397-409), `src/workspace_handoff.rs` (`prepare_handoff_worktree` :85, `remove_handoff_worktree` :163). Tests inline in `workspace.rs`.

**Produces:**
```rust
impl WorkspaceGuard {
    pub fn acquire(root: &Path) -> Result<Self, BackendError>;              // unchanged: root lock, prepare() only
    pub fn acquire_at(lock_file: &Path) -> Result<Self, BackendError>;     // creates parent dir, try_lock
    pub fn checkout(root: &Path, repo_name: &str) -> Result<Self, BackendError>; // <root>/.locks/<repo_name>.lock
    pub fn worktree(root: &Path, job_id: &str) -> Result<Self, BackendError>;   // <root>/.runs/<job_id>.lock
}
pub enum RunKind { Interactive, SharedCheckout }
/// Interactive runs lock their own `.runs/<job>` worktree; shared-checkout runs lock the repository checkout
/// that contains `cwd`. `cwd` outside every configured root is an error, as before.
pub fn lock_for_run(cwd: &Path, roots: &[PathBuf], kind: RunKind) -> Result<Option<WorkspaceGuard>, BackendError>;
```

- [ ] **Step 1: failing tests** (in `workspace.rs` tests; `tempfile`):
```rust
#[test]
fn two_interactive_runs_hold_independent_worktree_locks() {
    let root = tempfile::tempdir().unwrap();
    for j in ["job-a", "job-b"] { std::fs::create_dir_all(root.path().join(".runs").join(j)).unwrap(); }
    let roots = vec![root.path().to_path_buf()];
    let a = lock_for_run(&root.path().join(".runs/job-a"), &roots, RunKind::Interactive).unwrap();
    let b = lock_for_run(&root.path().join(".runs/job-b"), &roots, RunKind::Interactive).unwrap();
    assert!(a.is_some() && b.is_some());
    assert!(lock_for_run(&root.path().join(".runs/job-a"), &roots, RunKind::Interactive).is_err(), "same worktree twice is busy");
}
#[test]
fn shared_checkout_runs_on_one_repo_are_serialized_but_not_across_repos() {
    let root = tempfile::tempdir().unwrap();
    for r in ["repo-x", "repo-y"] { std::fs::create_dir_all(root.path().join(r).join("src")).unwrap(); }
    let roots = vec![root.path().to_path_buf()];
    let x1 = lock_for_run(&root.path().join("repo-x/src"), &roots, RunKind::SharedCheckout).unwrap();
    assert!(x1.is_some());
    assert!(lock_for_run(&root.path().join("repo-x"), &roots, RunKind::SharedCheckout).is_err());
    assert!(lock_for_run(&root.path().join("repo-y"), &roots, RunKind::SharedCheckout).unwrap().is_some());
    drop(x1);
    assert!(lock_for_run(&root.path().join("repo-x"), &roots, RunKind::SharedCheckout).unwrap().is_some());
}
#[test]
fn interactive_and_shared_runs_on_the_same_repo_coexist() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("repo-x")).unwrap();
    std::fs::create_dir_all(root.path().join(".runs/job-a")).unwrap();
    let roots = vec![root.path().to_path_buf()];
    let _shared = lock_for_run(&root.path().join("repo-x"), &roots, RunKind::SharedCheckout).unwrap();
    assert!(lock_for_run(&root.path().join(".runs/job-a"), &roots, RunKind::Interactive).unwrap().is_some());
}
#[test]
fn cwd_outside_roots_is_rejected_and_empty_roots_means_no_lock() {
    let root = tempfile::tempdir().unwrap(); let other = tempfile::tempdir().unwrap();
    assert!(lock_for_run(other.path(), &[root.path().to_path_buf()], RunKind::SharedCheckout).is_err());
    assert!(lock_for_run(other.path(), &[], RunKind::Interactive).unwrap().is_none());
}
```
- [ ] Step 2: run `cargo test lock_for_run` → FAIL (missing items).
- [ ] **Step 3: implement**
```rust
pub fn acquire_at(lock_file: &Path) -> Result<Self, BackendError> {
    if let Some(parent) = lock_file.parent() { std::fs::create_dir_all(parent)?; }
    let file = OpenOptions::new().read(true).write(true).create(true).truncate(false).open(lock_file)?;
    file.try_lock().map_err(|e| BackendError::SpawnFailed(format!("{} is busy or cannot be locked: {e}", lock_file.display())))?;
    Ok(Self(file))
}
pub fn checkout(root: &Path, repo_name: &str) -> Result<Self, BackendError> { Self::acquire_at(&root.join(".locks").join(format!("{repo_name}.lock"))) }
pub fn worktree(root: &Path, job_id: &str) -> Result<Self, BackendError> { Self::acquire_at(&root.join(".runs").join(format!("{job_id}.lock"))) }

pub fn lock_for_run(cwd: &Path, roots: &[PathBuf], kind: RunKind) -> Result<Option<WorkspaceGuard>, BackendError> {
    if roots.is_empty() { return Ok(None); }
    let cwd = cwd.canonicalize()?;
    let root = outermost_root_containing(&cwd, roots)?; // the existing selection logic from lock_for_directory, extracted
    let rel = cwd.strip_prefix(&root).map_err(|_| BackendError::SpawnFailed("working directory is outside the configured shared workspace roots".into()))?;
    let mut comps = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned());
    let first = comps.next().ok_or_else(|| BackendError::SpawnFailed("working directory must be inside a repository checkout or run worktree".into()))?;
    match kind {
        RunKind::Interactive if first == ".runs" => {
            let job = comps.next().ok_or_else(|| BackendError::SpawnFailed("run worktree path has no job id".into()))?;
            WorkspaceGuard::worktree(&root, &job).map(Some)
        }
        RunKind::Interactive => Err(BackendError::SpawnFailed("interactive runs must use a .runs/<job_id> worktree".into())),
        RunKind::SharedCheckout => WorkspaceGuard::checkout(&root, &first).map(Some),
    }
}
```
Delete `lock_for_directory`. In `process.rs:397-409` call `lock_for_run(&config.working_directory, &roots, if interactive { RunKind::Interactive } else { RunKind::SharedCheckout })` (`interactive` is computed at :389). In `workspace_handoff.rs`: `prepare_handoff_worktree` replaces `WorkspaceGuard::acquire(&root)` with `WorkspaceGuard::checkout(&root, &repo.name)`; `remove_handoff_worktree` takes `WorkspaceGuard::checkout` around `worktree remove` (log and skip removal if busy). `prepare()` keeps the root lock. Update the module doc (`workspace.rs:4-6`) and `docs/shared-agent-workspaces.md` (one paragraph: locks are per checkout and per run worktree).
- [ ] Step 4: `cargo test` (the existing node tests that spawn moved runs must still pass — they now take a worktree lock); clippy; fmt. Step 5: commit `feat(workspace): per-checkout and per-worktree locks replace the root lock`.

### Task A3: Node capacity cap and repository reachability in the registration

**Files:** Modify `src/remote/node.rs` (`run_node` :177-180, registration :217-228, `handle_spawn_with` :491-530, `start_moved_session` :699); `src/workspace.rs` (new `check_repositories`); tests in node.rs test module.

**Produces:**
```rust
// workspace.rs
pub struct RepositoryStatus { pub name: String, pub url: String, pub branch: String, pub reachable: bool, pub error: String }
/// `git ls-remote --heads <url> <branch>` per repository, 30 s timeout each, run concurrently.
pub async fn check_repositories(config: &WorkspaceConfig) -> Vec<RepositoryStatus>;
// node.rs
pub(crate) fn max_interactive_runs() -> usize // AGENT_CTRL_MAX_INTERACTIVE_RUNS, default 4, minimum 1
fn interactive_run_count(runtime: &NodeRuntime) -> usize
```

- [ ] **Step 1: failing tests**
  - `interactive_spawn_over_the_cap_is_rejected_as_node_at_capacity`: with the cap forced to 1 (make `handle_spawn_with` take the cap as a parameter like it takes `load_workspaces`), a second interactive spawn (moved or fresh) while one interactive run is active → NACK whose `error` contains `node at capacity`; a non-interactive spawn is unaffected.
  - `registration_reports_repositories_with_reachability`: `check_repositories` against a temp bare repo (reachable=true) and a nonexistent path (reachable=false, non-empty error); `registration_snapshot` includes them as `pb::RepositoryInfo`.
- [ ] Step 2: run → FAIL.
- [ ] **Step 3: implement.** `check_repositories`: `futures::future::join_all` over repos, each `tokio::time::timeout(Duration::from_secs(30), git(root, &["ls-remote", "--heads", &repo.url, &repo.branch]))`; reachable iff Ok and stdout non-empty; error = trimmed stderr/timeout text (redact userinfo with `redacted_repo_url` if the url appears). In `run_node` after `load_workspace_config_from_env()`: `let repositories = check_repositories(&ws).await;` stored on `NodeRuntime` (`repositories: Vec<RepositoryStatus>`), copied into `pb::NodeRegistration.repositories` at :217-228 and in `registration_snapshot`. Cap: in `handle_spawn_with`, before the moved branch and before the plain `runner.spawn`, when `cmd.interactive` and `interactive_run_count(&runtime) >= cap` → `reject(runtime, command_id, BackendError::SpawnFailed("node at capacity".into()))`. The count is `runtime.active.iter().filter(|r| r.interactive).count()`.
- [ ] Step 4: tests green, clippy, fmt. Step 5: commit `feat(node): interactive run cap and repository reachability`.

### Task A4: Fresh run worktrees and the sweep rule

**Files:** Modify `src/workspace_handoff.rs` (new fns next to `prepare_handoff_worktree`), `src/remote/node.rs` sweep (`sweep_stale_session_dirs`). Tests inline (real git fixture `test_support::handoff_fixture` :211).

**Also in this task (owns the Global-Constraints URL rule):** extend `normalize_repo_url` in `src/workspace_handoff.rs:8-26` so scp-style `git@host:path(.git)` (and `user@host:path`) normalizes to `https://host/path` — tests: `git@gitlab.com:pseekoo/x.git` == `https://gitlab.com/pseekoo/x` == `https://user:tok@gitlab.com/pseekoo/x.git/`; `redacted_repo_url` for an scp url must then equal its normalized form, and `RepositoryInfo.url` reported by the node (A3, `workspace.rs:~662`) must equal `normalize_repo_url(configured)` for scp AND https inputs (add that assertion to A3's reachability test). Also tighten the A3 `ls-remote` pattern to `refs/heads/<branch>` (exact match).

**Produces:**
```rust
pub fn valid_base_branch(branch: &str) -> bool;                   // Global Constraints regex
pub fn agent_branch(job_id: &str) -> String;                      // format!("agent/{job_id}")
/// Fetches `origin/<base>` into the repository checkout and creates `<root>/.runs/<job_id>` on new branch `agent/<job_id>`.
pub async fn prepare_run_worktree(config: &WorkspaceConfig, repo_url: &str, base_branch: Option<&str>, job_id: &str) -> Result<PathBuf, BackendError>;
/// Removes `.runs/*.session` dirs; removes worktrees whose lock is free and whose HEAD equals origin/<branch> (and their `.runs/<job>.lock` and `.runs/<job>.branch` files); keeps and logs the rest.
pub async fn sweep_runs(config: &WorkspaceConfig) -> SweepReport { removed_worktrees: Vec<String>, kept_worktrees: Vec<(String, String)>, removed_session_dirs: usize }
```

- [ ] **Step 1: failing tests**
  - `fresh_run_worktree_branches_from_the_base`: fixture repo with `dev` → `prepare_run_worktree(.., None, "job-1")` → worktree at `<root>/.runs/job-1`, `git branch --show-current` = `agent/job-1`, HEAD == origin/dev; shared checkout branch unchanged; `prepare_run_worktree(.., Some("dev"), "job-2")` coexists.
  - `invalid_base_branch_is_rejected`: `..`, leading `-`, `refs/../x`, `.hidden` → Err; `feature/x.y-1` → Ok shape.
  - `push_back_on_the_agent_branch`: after a commit in the worktree, `push_handoff_work(&wt, &agent_branch("job-1"), "job-1")` → origin has `agent/job-1` with the commit.
  - `sweep_removes_only_pushed_unlocked_worktrees`: three worktrees — (a) pushed + free → removed; (b) pushed but lock held → kept; (c) unpushed commit + free → kept with reason; plus a stray `.runs/x.session` dir → removed.
- [ ] Step 2: run → FAIL.
- [ ] **Step 3: implement** `prepare_run_worktree`: `find_repository` else Err; base = `base_branch.unwrap_or(&repo.branch)`; `valid_base_branch` else Err; `let _guard = WorkspaceGuard::checkout(&root, &repo.name)?;` `run_git(&checkout, ["fetch", "origin", &format!("+refs/heads/{base}:refs/remotes/origin/{base}")])`; `run_git(&checkout, ["worktree", "add", "-b", &agent_branch(job_id), <runs/job_id as local_path>, &format!("origin/{base}")])`. `sweep_runs`: list `.runs/*`; for `*.session` dirs remove; for worktree dirs try `WorkspaceGuard::worktree(root, job)` — Err → keep ("locked"); else branch = `git branch --show-current` (or detached → read `handoff` branch from `.git` worktree config: store the branch name in `<root>/.runs/<job>.branch` when creating any worktree, read it here); compare `git rev-parse HEAD` with `git rev-parse origin/<branch>` (fetch first); equal → `remove_handoff_worktree`; else keep ("unpushed work"). Replace `sweep_stale_session_dirs` in `run_node` with `sweep_runs` and log the report.
- [ ] Step 4: green/clippy/fmt. Step 5: commit `feat(workspace): fresh run worktrees on agent/<job> and a safe sweep`.

### Task A5: Node runs a fresh repo run

**Files:** Modify `src/remote/node.rs` (`handle_spawn_with` :491-530, `start_moved_session`/`MovedRun` :699-900 generalized), `src/remote/mod.rs` (`spawn_config_from` leaves working_directory; node overrides). Tests in node.rs module.

**Also (spec A·1 "every interactive run gets its own worktree"):** an interactive spawn with EMPTY `run_repo_url` whose `working_directory` lies inside a configured repository checkout (e.g. the existing `staging-*-interactive`/`*-inspector` profiles with a fixed `working_directory`) is treated as a fresh repo run on that repository with its configured `branch` as base — the node derives `repo_url` from the matching `Repository.url`. Without this, A2's lock rule rejects those profiles (`interactive runs must use a .runs/<job_id> worktree`). Test: an interactive spawn with cwd `<root>/<repo>` and no `run_repo_url` ends up in `<root>/.runs/<job_id>` on `agent/<job_id>`.

**Produces:** `SpawnCommand.run_repo_url`/`run_base_branch` handled: `IsolatedRun { mode: RunMode::Moved(MovedState) | RunMode::Fresh { repo_url, base_branch } }` (rename `MovedRun` → `IsolatedRun`, `MovedState` kept inside `RunMode::Moved`), `Handoff.branch` = `agent/<job_id>` for fresh runs.

- [ ] **Step 1: failing tests** (fake backend + real git fixture, as the moved tests): (a) a `SpawnCommand` with `interactive=true, run_repo_url=<fixture url>, run_base_branch=""` → ack success quickly; backend `spawn` (not `resume`) called with `working_directory == <root>/.runs/<job_id>`; no `resumed` status emitted; after a terminal state with a dirty file, origin has `agent/<job_id>` with `agent-ctrl run <job_id> end`; worktree removed. (b) unknown repo → NACK `not an operator-configured repository`; (c) `run_repo_url` with `interactive=false` → NACK `repo runs must be interactive`; (d) ordinary spawn unchanged (`ordinary_spawn_is_unchanged_and_never_loads_workspaces` still passes).
- [ ] Step 2: run → FAIL.
- [ ] **Step 3: implement.** In `handle_spawn_with`: `if !cmd.run_repo_url.is_empty() { validate (interactive, valid_job_id, workspaces loaded, find_repository, valid_base_branch if given) → start_isolated_run(RunMode::Fresh{..}) ; return }`. `IsolatedRun::run`: Fresh → `prepare_run_worktree`, `config.working_directory = worktree`, `runner.spawn` (prompt kept — fresh runs may carry a prompt), `monitor_agent_status`, then straight to `stream_agent(.., Some(handoff))` with `Handoff { branch: agent_branch(job_id), transcript: None, .. }`. Moved path unchanged. Both count toward the cap (A3). Run end for a fresh run: after `push_handoff_work` (which pushes nothing when the tree is clean and no commits were made) ALWAYS remove the worktree when the push step succeeded or was a no-op, and delete the local `agent/<job_id>` branch from the shared checkout (`git branch -D agent/<job_id>` under the checkout lock; the remote branch stays) — otherwise a clean run leaves a worktree the sweep can never prove pushed. Test: a fresh run that makes no changes ends with no `.runs/<job>` entry and no local `agent/<job>` branch.
- [ ] Step 4: green/clippy/fmt. Step 5: commit `feat(node): start an interactive run on a repository in its own worktree`.

### Task A6: Controller — start-on-repo, requires_repo, ListRepositories, capacity status

**Files:** Modify `src/remote/controller.rs` (`spawn_agent` :1630-1757, `DispatchOptions` :63, `dispatch_remote` :488-614, `finish_launch` :387, `job_info` :2395, `list_configured_agents` :2198 as model), `src/state/mod.rs` (`Job` :65 → `repo_url: Option<String>`, `branch: Option<String>`), `src/config/` agent YAML schema (`operator_control.requires_repo: bool`, default false), `src/fleet/mod.rs` (expose it on the configured-agent summary). Tests beside existing `spawn_agent`/`import_session` tests + `tests/remote_import.rs` style gRPC test.

- [ ] **Step 1: failing tests**
  - `spawn_with_repo_url_dispatches_a_fresh_repo_run`: request `{agent_name, prompt:"", interactive_override:true, repo_url, base_branch:"dev"}` → node receives `SpawnCommand{interactive:true, run_repo_url, run_base_branch:"dev"}`; `job.repo_url == Some(url)`, `job.branch == Some("agent/<job_id>")`; `JobInfo.repo_url/branch` set in `GetJob`.
  - `repo_url_is_validated`: 513 chars → invalid_argument; bad base_branch → invalid_argument; `repo_url` with `interactive_override:false` → invalid_argument `repo runs must be interactive`.
  - `requires_repo_agent_rejects_a_launch_without_repo_url`: profile with `operator_control.requires_repo: true` → `invalid_argument("agent requires repo_url")`.
  - `node_at_capacity_becomes_resource_exhausted`: fake node NACKs `spawn failed: node at capacity` → `spawn_agent` returns `Err(Status::resource_exhausted(..))` (not `success:false`), and `import_session` likewise.
  - `list_repositories_merges_node_registrations`: two registered nodes with overlapping repos → deduped by normalized url, `reachable` = any node reachable.
  - moved job: `job.branch == Some(handoff_branch)`.
- [ ] Step 2: run → FAIL.
- [ ] **Step 3: implement.** `DispatchOptions.run_repo: Option<&'a RunRepo>` with `pub(crate) struct RunRepo { url: String, base_branch: Option<String> }`; in `dispatch_remote` after `spawn_command`: `cmd.interactive = true; cmd.run_repo_url = r.url.clone(); cmd.run_base_branch = r.base_branch.clone().unwrap_or_default(); job.repo_url = Some(url); job.branch = Some(agent_branch(&job.job_id));` (moved: `job.repo_url = moved.repo_url; job.branch = Some(moved.branch)`). In `spawn_agent` after the override block: validate `req.repo_url`/`req.base_branch` (lengths/regex; `valid_base_branch` from A4), require `interactive_override == Some(true)` when repo_url is set, and `agent.operator_control.requires_repo` ⇒ repo_url must be present. Fingerprint (:1710-1724) includes both fields. `finish_launch`: when the dispatch error ends with `node at capacity` return `Err(Status::resource_exhausted("node at capacity"))`. `list_repositories`: iterate `self.nodes` registrations' `repositories`, dedupe by `normalize_repo_url(url)`, `reachable |=`. `job_info` adds the two fields. Mongo: `Job` fields are serialized by `bson::to_document` — `#[serde(default)]` makes old records load.
- [ ] Step 4: `cargo test` (incl. Mongo-gated with a throwaway instance if available), clippy, fmt. Step 5: commit `feat(controller): start a run on a repository; list repositories; capacity as RESOURCE_EXHAUSTED`.

### Task A7: Staging configuration

**Files:** Modify `deploy/k8s/instances/staging.toml` (:34-42), `deploy/k8s/templates/configmap.tmpl.yaml` (the three `staging-*-moved` blocks ~338-395), `deploy/k8s/templates/node-deployment.tmpl.yaml` (env :127-130, resources :146-152), `deploy/k8s/render.py` (new `--workspace-repos-txt` output), `deploy/staging/obachan-k8s/workspace-repos.txt` (generated), `deploy/k8s/test_shared_workspaces.py`.

- [ ] **Step 1: failing tests** in `test_shared_workspaces.py`: staging repo count = 32 and contains each name below with the given branch; profiles `staging-claude-interactive-rw` (`permission_mode: acceptEdits`), `staging-pi-interactive-rw`, `staging-codex-interactive-rw` (`workspace-write`) exist with `operator_control.requires_repo: true`, `interactive: true`, no `default_prompt`, no `allowed_tools`; no profile named `*-moved` remains; node env has `AGENT_CTRL_MAX_INTERACTIVE_RUNS: "4"` and memory limit `12Gi`; `workspace-repos.txt` equals the render output for staging (one `<name> <url>` line per repo, sorted).
- [ ] Step 2: run → FAIL.
- [ ] **Step 3: implement.** Repositories to add to `staging.toml` (name = folder name lowercased where it differs; url; branch):
```
IWAKAN:  be-curious.io https://gitlab.com/pseekoo/be-curious.io.git main
         kaizen https://gitlab.com/pseekoo/kaizen.git main
         martin-wieser.me https://gitlab.com/pseekoo/martin-wieser.me.git develop
         me https://gitlab.com/pseekoo/me.git main
         mist-kibl.at https://gitlab.com/pseekoo/mist-kibl.at.git develop
         pseekoo.ai https://gitlab.com/pseekoo/pseekoo.ai.git develop
         pseekoo.at https://gitlab.com/pseekoo/pseekoo.at.git main
         pseekoo.com https://gitlab.com/pseekoo/pseekoo.com.git develop
         pseekoos-economics-and-philosophies https://gitlab.com/pseekoo/pseekoos-economics-and-philosophies.git main
         cdn.pseekoo https://gitlab.com/budelius/cdn.pseekoo.git master
         www-pseekoo-com https://gitlab.com/pseekoo/www-pseekoo-com.git master
         www-pseekoo-ai-attic https://gitlab.com/pseekoo/www-pseekoo-ai-attic.git master
         be-curious-io https://github.com/budelius/be-curious-io.git main
JustVibe: battery-cafe https://gitlab.com/pseekoo/battery-cafe.git main
         cefa https://gitlab.com/pseekoo/cefa.git main
         gakushu https://gitlab.com/pseekoo/gakushu.git master
         headless-mailing-webcomponent https://gitlab.com/pseekoo/headless-mailing-webcomponent.git main
         headless-shop-webcomponent https://gitlab.com/pseekoo/headless-shop-webcomponent.git main
         headless-tryon-webcomponent https://gitlab.com/pseekoo/headless-tryon-webcomponent.git main
         images https://gitlab.com/pseekoo/images.git master
         gummitechnik-mihet https://gitlab.com/pseekoo/gummitechnik-mihet.git main
         ng-history https://gitlab.com/pseekoo/ng-history.git main
         shop-admin https://gitlab.com/pseekoo/shop-admin.git main
         solscrap https://gitlab.com/pseekoo/solscrap.git main
         staats-druckerei https://gitlab.com/pseekoo/staats-druckerei.git main
```
Repo names must satisfy `render.py`'s policy (`[A-Za-z0-9._-]`, unique). Rename the three profiles, add `requires_repo: true` under `operator_control`, keep descriptions `Interactive <Agent> session on an operator repository`. Node env + resources as constrained. `render.py --workspace-repos-txt` prints the `<name> <url>` lines for an instance; regenerate `workspace-repos.txt` and make the test compare file vs render.
- [ ] Step 4: Python tests green. Step 5: commit `deploy(staging): IWAKAN and JustVibe repositories, rw interactive profiles, run cap`. Push `feat/fleet-runs`.

---

## Part B — stoz3n-agent-dash

Worktree: `git -C T:/projects/SOTZ3N/stoz3n-agent-dash worktree add -b feat/fleet-runs T:/projects/SOTZ3N/stoz3n-agent-dash-fleet origin/dev`; `yarn install --immutable`; tests `yarn test:agent-control`, `yarn lint`, `yarn check`, `npx tsc -p tsconfig.server.json --noEmit`.

### Task B1: Launch on a repo, `/repos`, capacity 409, run fields

**Files:** `server/agentctrl/v1.proto` (copy from agent-ctrl `feat/fleet-runs`), `server/agent-control.ts` (dispatch :64-78, `launchRun` :210-253, `publicRun` :478-523, `mapGrpcError` :681-691), `server/openapi.ts` (`ManagedLaunchInput` :611-621, new `/api/agent-control/repos`), `scripts/check-agent-control.mjs` (fake service :197-202, `mutation` :176).

- [ ] **Step 1: failing checks**
```js
check("launch forwards repo_url and base_branch", async () => {
  const r = await call("/api/agent-control/runs", jsonMutation({ agent_name: "staging-claude-interactive-rw", interactive: true, task_id: "task-1", repo_url: "https://gitlab.com/pseekoo/kaizen.git", base_branch: "main" }, "repo-launch-request-0001"));
  assert.equal(r.status, 202); const sent = requests.at(-1);
  assert.equal(sent.repo_url, "https://gitlab.com/pseekoo/kaizen.git"); assert.equal(sent.base_branch, "main");
});
check("launch rejects a bad base_branch and an oversized repo_url", async () => {
  assert.equal((await call("/api/agent-control/runs", jsonMutation({ agent_name: "a", interactive: true, task_id: "task-1", repo_url: "https://x/y.git", base_branch: "../evil" }, "repo-launch-request-0002"))).status, 422);
  assert.equal((await call("/api/agent-control/runs", jsonMutation({ agent_name: "a", interactive: true, task_id: "task-1", repo_url: "https://x/" + "y".repeat(520) }, "repo-launch-request-0003"))).status, 422);
});
check("node at capacity is a 409", async () => {
  const r = await call("/api/agent-control/runs", jsonMutation({ agent_name: "a", interactive: true, task_id: "task-1" }, "atcapacity-launch-request-0004"));
  assert.equal(r.status, 409); assert.equal((await r.json()).code, "node_at_capacity");
});
check("repos lists the controller's repositories", async () => {
  const r = await call("/api/agent-control/repos", { headers: adminHeaders() });
  assert.equal(r.status, 200); const body = await r.json();
  assert.deepEqual(body.items[0], { name: "kaizen", url: "https://gitlab.com/pseekoo/kaizen.git", branch: "main", reachable: true, error: null });
});
check("runs expose repo_url and branch", async () => { /* seed a job with repo_url/branch in `records.jobs`; GET /runs; assert fields */ });
```
Fake service: `ListRepositories: (call, cb) => cb(null, { repositories: [{ name: "kaizen", url: "https://gitlab.com/pseekoo/kaizen.git", branch: "main", reachable: true, error: "" }] })`; `mutation` gets an `atcapacity` prefix branch calling `cb({ code: grpc.status.RESOURCE_EXHAUSTED, message: "node at capacity" })`.
- [ ] Step 2: run → FAIL.
- [ ] **Step 3: implement.** `exactKeys` list += `repo_url`, `base_branch` (both optional); when `repo_url` is present `task_id` becomes optional (skip `resolveTaskAttachment`, send empty `task_*`); add a check `launch on a repo needs no task_id` (202) and `launch without repo_url still requires task_id` (422); `repo_url` = `optionalString(.., 512)`; `base_branch` = `optionalString(.., 100)` and must match `/^(?!-)(?!\/)(?!.*\.\.)(?!.*(^|\/)\.)[A-Za-z0-9._\/-]{1,100}$/` else 422 `base_branch is invalid`; forward `repo_url: repoUrl ?? "", base_branch: baseBranch ?? ""`. `mapGrpcError`: `RESOURCE_EXHAUSTED → serviceError(409, message || "node at capacity", "node_at_capacity")`. `GET /api/agent-control/repos` (admin, no mutation gate) → `unary(config, "ListRepositories", {})` → `{ items: repositories.map(r => ({ name, url, branch, reachable: !!r.reachable, error: r.error || null })) }`. `publicRun` += `repo_url: nullableString(job.repo_url), branch: nullableString(job.branch)`. OpenAPI: `ManagedLaunchInput` gains `repo_url` (maxLength 512), `base_branch` (pattern), and `node_name`; new path `/api/agent-control/repos` with `ManagedRepository` schema; `ManagedRun` documents the two new fields; 409 `node_at_capacity` on the launch route.
- [ ] Step 4: all checks + lint + typecheck green. Step 5: commit `feat(agent-control): launch on a repository, list repositories, node capacity 409`; push `feat/fleet-runs`.

---

## Part C — Rollout (needs the user's go-ahead before merging; merging to dev deploys staging)

- [ ] Open PRs into `dev` on git-dev.obachan.dev for both repos (templates as before); merge agent-ctrl first, then dash; watch the two pipelines; confirm staging `GET /repos` lists all 32 repositories with `reachable: true` (fix credentials for any `false` before continuing).
- [ ] Staging E2E: `agent-ctrl start --agent staging-claude-interactive-rw --task <t> --repo https://gitlab.com/pseekoo/kaizen.git --interactive` and the same for pi on `battery-cafe` CONCURRENTLY (requires Plan B's CLI, or curl the dashboard directly); each edits a file; stop both; both repos get `agent/<job>` branches with `agent-ctrl run <job> end` commits; `/workspace/.runs` is empty afterwards. A third start while two run and the cap is 2 (temporarily) returns 409.
