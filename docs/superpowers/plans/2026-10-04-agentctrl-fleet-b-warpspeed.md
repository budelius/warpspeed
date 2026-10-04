# AgentCtrl Fleet — Plan B (WarpSpeed)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Start, move, watch and control AgentCtrl fleet runs from WarpSpeed: a "start on this repo" action and CLI, a "move all sessions under a folder" migration with recovery of each pane's directory and agent, and a Fleet pane.

**Architecture:** `crates/managed_agent_sessions` grows typed dashboard calls (`start_run_on_repo`, `list_repositories`, `list_runs`, `send_input`). The remote-control protocol (v2) exposes each pane's agent kind, session id, agent-reported cwd and transcript path, and accepts a `MoveSessionToAgentCtrl` request plus `MoveStatus` polling, so one CLI driver (`agent-ctrl move-all`) can migrate panes across all local instances sequentially using the existing move flow without its confirm toast. A new `AgentCtrlFleetPane` (same registration pattern as `NetworkLogPane`) renders runs grouped by repo from a polling singleton model.

**Tech Stack:** Rust/WarpUI (`app/`), clap (`crates/warp_cli`), reqwest blocking (`crates/managed_agent_sessions`), bincode remote-control IPC (`app/src/remote_control`).

## Global Constraints

- Profile names come from one place: `agent_ctrl_profile(kind: CLIAgent) -> Option<&'static str>` = `staging-claude-interactive-rw | staging-pi-interactive-rw | staging-codex-interactive-rw` (Plan A renames the `*-moved` profiles; this plan must land together with, or after, Plan A's deploy — moves are broken in between).
- Dashboard contract (Plan A): `POST /api/agent-control/runs` accepts optional `repo_url` (≤512) and `base_branch` (`^(?!-)(?!\/)(?!.*\.\.)(?!.*(^|\/)\.)[A-Za-z0-9._\/-]{1,100}$`); when `repo_url` is present `task_id` may be omitted; 409 `node_at_capacity`; `GET /api/agent-control/repos` → `{items:[{name,url,branch,reachable,error}]}`; `GET /runs` items carry `repo_url` and `branch`.
- Remote-control protocol: bump `PROTOCOL_VERSION` to 2; every `Request`/`Response`/`PaneSummary` change is in this plan's Task B3 only.
- Recovery rule for a pane's source (in order): pane `working_directory` → hook-reported `agent_cwd` → `agent_session_history::find_all(agent)` entry with the pane's session id (its `cwd`); agent kind from `CLIAgentSessionsModel`; session id from `session_id(view)` (hook or recovered). A pane with no agent kind or no session id is skipped with reason `no agent session`; one whose repo is not in `/repos` is skipped with `repo not configured: <url>`.
- Move-all runs moves strictly one at a time; it never starts the next cut-over before the previous move reaches `Done` or `Failed`; per-move timeout 10 min → `Failed{reason:"timed out"}`.
- Fleet pane refresh: every 15 s while a fleet view is attached; grouped by `repo_url` (runs without one under `(no repository)`); row actions Open, Stop; toolbar Start…; "Send" is served by Open (the viewer pane has input) — spec deviation, noted.
- Conventions: AGENTS.md (ctx last, exhaustive matches, no `_`-prefixed params, inline format args, "why"-only comments, tests in `*_tests.rs`, never re-lock the TerminalModel while a caller holds it); new actions get palette entries + context flags.
- Branch `feat/agentctrl-fleet` in worktree `T:/projects/dev-ops/warp-agentctrl-fleet`; push to `gitea` and `gitlab`, never `origin`. Build with `CARGO_TARGET_DIR=T:/projects/dev-ops/warp-dev-release/target`; never restart running WarpSpeed; baseline: ~22 pre-existing `workspace::view`/`StoznAuthManager` failures on dev.

---

### Task B1: Client calls and CLI for fleet runs

**Files:** Modify `crates/managed_agent_sessions/src/lib.rs` (`run` :295, `launch_request` :400, `list` :338, `input` :490), `crates/managed_agent_sessions/src/tests.rs`, `crates/warp_cli/src/managed_sessions.rs` (:18-76), `crates/warp_cli/src/lib_tests.rs` (:16-49).

**Produces:**
```rust
pub struct RepositoryInfo { pub name: String, pub url: String, pub branch: String, pub reachable: bool, pub error: Option<String> }
pub struct RunSummary { pub job_id: String, pub state: String, pub agent_name: String, pub repo_url: Option<String>, pub branch: Option<String>, pub interactive: bool, pub last_activity_at: Option<String> }
pub fn list_repositories(environment: ManagedEnvironment) -> Result<Vec<RepositoryInfo>>;
pub fn list_runs(environment: ManagedEnvironment) -> Result<Vec<RunSummary>>;
pub fn start_run_on_repo(environment: ManagedEnvironment, agent_name: &str, repo_url: &str, base_branch: Option<&str>, prompt: Option<&str>) -> Result<String /* job_id */>;
pub fn send_input(environment: ManagedEnvironment, job_id: &str, text: &str) -> Result<()>;
pub fn agent_ctrl_profile(agent: &str /* "claude"|"pi"|"codex" */) -> Option<&'static str>;
pub fn normalize_repo_url(url: &str) -> String; // same rule as the server: scp → https, strip userinfo, trailing / and .git, lowercase scheme+host
// clap
ManagedSessionsCommand::Start { agent, task: Option<String> /* required_unless_present = "repo" */, repo: Option<String>, branch: Option<String>, prompt, interactive, environment }
ManagedSessionsCommand::Repos { environment }
ManagedSessionsCommand::MoveAll { under: PathBuf, yes: bool, environment }   // handled in app (Task B4)
```

- [ ] **Step 1: failing tests** (`tests.rs`, fixtures `capturing_fixture`/`http_fixture`): `start_on_repo_sends_repo_fields_and_no_task` (body has `agent_name`, `repo_url`, `base_branch`, `interactive:true`, no `task_id`; returns job id); `start_on_repo_rejects_a_bad_branch_locally` (`../x` → Err before any request); `list_repositories_parses_items`; `list_runs_parses_repo_and_branch`; `normalize_repo_url_matches_server_rule` (`git@gitlab.com:pseekoo/x.git` == `https://gitlab.com/pseekoo/x`); clap tests in `lib_tests.rs`: `start --agent a --repo https://x/y.git` parses without `--task`; `start --agent a` without `--task`/`--repo` errors (keep the existing assertion's intent); `repos` and `move-all --under T:\p` parse.
- [ ] Step 2: `cargo nextest run -p managed_agent_sessions -p warp_cli` → FAIL.
- [ ] **Step 3: implement.** `launch_request` gains `repo_url: Option<&str>, base_branch: Option<&str>` and omits `task_id` when `repo_url` is Some; validate `base_branch` with the Global-Constraints regex (hand-rolled check: no `..`, no leading `-`/`/`, no component starting with `.`, allowed chars, ≤100). `list_runs`: `GET …/runs?limit=100`, map items. `list_repositories`: `GET …/repos`. `send_input` = `input(env, job, Some(text), false)` made pub. `run()` arms: `Start{repo: Some(..)}` → `start_run_on_repo` and print `Started <job>. Open with: warpspeed agent-ctrl open <job>`; `Repos` prints a table (name, branch, reachable, url); `MoveAll` → `bail!("move-all is handled by the application")` (the app intercepts it before `run`, Task B4).
- [ ] Step 4: green; clippy for both crates; fmt. Step 5: commit (explicit paths) `feat(agent-ctrl): start a run on a repository, list repositories and runs`.

### Task B2: "Start agent on AgentCtrl in this repo"

**Files:** Modify `app/src/terminal/agent_session_handoff.rs` (rename `moved_agent_name` → `agent_ctrl_profile` returning the rw names; new `pub(crate) fn repo_url_for(cwd: &Path) -> Result<(String /*remote*/, String /*url, userinfo stripped*/)>` built from `select_remote` + `git remote get-url`), `app/src/workspace/action.rs` (new `WorkspaceAction::StartAgentOnAgentCtrl`, `StartAgentOnAgentCtrlForTerminalView { terminal_view_id }`, `ConfirmStartAgentOnAgentCtrl { terminal_view_id, agent: CLIAgent }`), `app/src/workspace/view.rs` (handlers next to the move flow ~13826-14360; context key next to :28596 `Workspace_PaneCanStartAgentOnAgentCtrl`), `app/src/workspace/mod.rs` (binding next to :1031 `workspace:start_agent_on_agent_ctrl` / "Start agent on AgentCtrl in this repo"), `app/src/terminal/view/pane_impl.rs` (menu item next to :824; predicate: local pane with a git repo cwd), `app/src/terminal/view/action.rs`, `app/src/terminal/view.rs` (TerminalAction plumbing as for `MoveSessionToAgentCtrl` :27659). Tests: `app/src/workspace/view_tests.rs` (pattern :6873-7375), `agent_session_handoff_tests.rs`.

- [ ] **Step 1: failing tests**: menu shows `Start agent on AgentCtrl in this repo` for a local pane whose cwd is a git repo (fixture repo with a `gitea` remote), hidden for a non-repo cwd; binding `workspace:start_agent_on_agent_ctrl` exists with predicate `Workspace & Workspace_PaneCanStartAgentOnAgentCtrl`; `repo_url_for` prefers `gitea`, strips userinfo; flow test: handling `ConfirmStartAgentOnAgentCtrl{view, Claude}` with a fake `start_run_on_repo` result (inject through a `#[cfg(test)]` hook like the move tests) runs `agent-ctrl open <job> --environment staging` in that pane and shows `Started <job> on <repo name>`; a 409 error shows `AgentCtrl node is at capacity; stop a run or try later`.
- [ ] Step 2: run → FAIL.
- [ ] **Step 3: implement.** `StartAgentOnAgentCtrlForTerminalView`: off-thread `repo_url_for(cwd)` then `list_repositories` → if the normalized url is not configured, error toast `repo not configured on AgentCtrl: <url>`; else a persistent toast `Start which agent on <remote> <repo name> (<default branch>)?` with three action links Claude / Pi / Codex → `ConfirmStartAgentOnAgentCtrl`. Confirm: progress toast `Starting <Agent> on AgentCtrl…`, off-thread `start_run_on_repo(Staging, agent_ctrl_profile(kind), url, None, None)`, on Ok `execute_managed_session_command_in(view, "agent-ctrl open <job> --environment staging")` + success toast; on Err error toast (409 text as above; `HttpStatusError` 409 detection via the error chain). Context key set in `keymap_context` when the focused pane is local and its cwd is inside a git work tree (use the existing `current_repo_path()` on `TerminalView`, :3015 on dev — verify name).
- [ ] Step 4: `cargo nextest run -p warp start_agent_on_agent_ctrl agent_session_handoff`, clippy, fmt. Step 5: commit `feat(workspace): start an agent on AgentCtrl in the pane's repository`.

### Task B3: Remote-control protocol v2 — agent pane details and move requests

**Files:** Modify `app/src/remote_control/protocol.rs` (:17 version, :21 `Request`, :62 `Response`, :90 `PaneSummary`), `app/src/remote_control/handler.rs` (:71 `list_panes`, new arms), `app/src/workspace/terminal_panes.rs` (`TerminalPaneInfo` :20-60), `app/src/workspace/view.rs` (`terminal_panes` :14481-14520; a `move_session_to_agent_ctrl_unattended(terminal_view_id, ctx)` that starts at `confirm_move_session_to_agent_ctrl` skipping the toast; `agent_ctrl_move_step(terminal_view_id) -> Option<MoveStep>` already used by tests at :7133). Tests: `remote_control/protocol_tests.rs` (round trip), `workspace/view_tests.rs`.

**Produces:**
```rust
pub const PROTOCOL_VERSION: u32 = 2;
// PaneSummary +=
pub terminal_view_id: Option<u64>, pub agent: Option<String> /* "claude"|"pi"|"codex"|other label */, pub agent_session_id: Option<String>, pub agent_cwd: Option<String>, pub transcript_path: Option<String>,
// Request +=
MoveSessionToAgentCtrl { terminal_view_id: u64 }, MoveStatus { terminal_view_id: u64 },
// Response +=
MoveStarted, MoveState { step: String /* "preparing"|"importing"|"waiting_for_resume"|"cutting_over"|"done:<job>"|"failed:<reason>"|"none" */ },
```

- [ ] **Step 1: failing tests**: bincode round trip of the new variants; `list_panes` for a workspace with a seeded `CLIAgentSession` (Claude, session id, hook cwd, transcript path) returns a summary with all five new fields and `terminal_view_id`; `MoveSessionToAgentCtrl` for a view with a qualifying session puts the view into `agent_ctrl_moves` at `Preparing` and replies `MoveStarted`; for a view without a session replies `Failed{error}`; `MoveStatus` reports `none`, then the step string.
- [ ] Step 2: run → FAIL.
- [ ] **Step 3: implement.** `terminal_panes` fills the new `TerminalPaneInfo` fields from `CLIAgentSessionsModel::session(view.id())` (`agent.command_prefixes()[0]` as the label, `session_id(view.id())`, `session_context.cwd`, `transcript_path`) and `view.id()` as `terminal_view_id` (EntityId → u64 via its existing conversion; if none exists, add `EntityId::as_u64`). Handler arms follow `detach` (:112): find the workspace owning the view (`WorkspaceRegistry` + `find_pane_id_for_terminal_view`), then `workspace.update(ctx, |w, ctx| w.move_session_to_agent_ctrl_unattended(view_id, ctx))`. `move_session_to_agent_ctrl_unattended` = the body of `move_session_to_agent_ctrl_for_terminal_view` minus `ask_to_confirm_agent_ctrl_move`, going straight to `confirm_move_session_to_agent_ctrl`. `MoveStatus` maps `agent_ctrl_move_step` to the string.
- [ ] Step 4: `cargo nextest run -p warp remote_control move_session`, clippy, fmt. Step 5: commit `feat(remote-control): expose agent pane details and drive a move`.

### Task B4: `agent-ctrl move-all --under <folder>` and the palette entry

**Files:** Create `app/src/agent_ctrl_fleet_cli.rs`, `app/src/agent_ctrl_fleet_cli_tests.rs`; modify `app/src/lib.rs` (:864 dispatch: intercept `ManagedSessionsCommand::MoveAll` before `managed_agent_sessions::run`), `app/src/workspace/action.rs` + `view.rs` + `mod.rs` (palette `workspace:move_all_agent_sessions_here` / "Move all agent sessions in this project folder to AgentCtrl": confirm toast naming the folder = parent of the focused pane's repo root, then `add_terminal_tab` + `execute_managed_session_command("agent-ctrl move-all --under <folder> --yes")`).

**Produces (pure, tested):**
```rust
pub(crate) struct PaneSource { pub instance_pid: u32, pub terminal_view_id: u64, pub agent: CLIAgent, pub session_id: String, pub cwd: PathBuf, pub recovered_from: SourceOrigin /* PaneCwd | HookCwd | SessionHistory */ }
pub(crate) enum Skip { NoAgentSession, NoDirectory, OutsideFolder, RepoNotConfigured(String), NotAGitRepo }
pub(crate) fn plan_moves(panes: &[(u32, PaneSummary)], under: &Path, history: &dyn Fn(CLIAgent, &str) -> Option<PathBuf>, configured: &[String /* normalized urls */], repo_url_of: &dyn Fn(&Path) -> Option<String>) -> (Vec<PaneSource>, Vec<(PaneSummary, Skip)>);
```
Driver: `live_instances()` → `ListPanes` each → `plan_moves` (history = `agent_session_history::find_all(agent)` lookup by session id; configured = `list_repositories` normalized; repo_url_of = `repo_url_for(cwd)`) → print the plan table → unless `--yes`, ask `Move N sessions? [y/N]` on stdin → for each source: `MoveSessionToAgentCtrl`, poll `MoveStatus` every 2 s until `done:`/`failed:` or 10 min → collect → final table (pane title, folder, agent, job id or failure) printed and appended to `<state_dir>/agentctrl-move-all.log`.

- [ ] **Step 1: failing tests** (`agent_ctrl_fleet_cli_tests.rs`, pure): recovery order (pane cwd present → PaneCwd; empty pane cwd + hook cwd → HookCwd; both empty + history hit → SessionHistory; nothing → `Skip::NoDirectory`); folder filter; `Skip::RepoNotConfigured`; `Skip::NoAgentSession` when agent or session id missing; ordering by instance pid then tab index. Clap/dispatch test: `warpspeed agent-ctrl move-all --under X` reaches the new module (unit test on the dispatch helper).
- [ ] Step 2: run → FAIL. Step 3: implement as above (blocking client calls are fine in the CLI). Step 4: green/clippy/fmt. Step 5: commit `feat(cli): move every agent session under a folder to AgentCtrl`.

### Task B5: Fleet pane

**Files:** Create `app/src/agent_ctrl/fleet_model.rs` (+ `fleet_model_tests.rs`), `app/src/agent_ctrl/fleet_view.rs`, `app/src/agent_ctrl/mod.rs`, `app/src/pane_group/pane/agent_ctrl_fleet_pane.rs` (template: `network_log_pane.rs`); register at every site in the table below; palette `workspace:open_agent_ctrl_fleet` / "Open AgentCtrl fleet"; the move success toast gains an "Open fleet" link.

| Registration site | Location (dev) |
|---|---|
| module list | `pane/mod.rs:11-30` |
| `IPaneType::AgentCtrlFleet` + Display | `pane/mod.rs:148`, `:172` |
| `PaneId::from_agent_ctrl_fleet_pane_{ctx,view}` | `pane/mod.rs:273`, `:374` |
| `ChildView` arm | `pane/mod.rs:497` |
| re-export | `pane_group/mod.rs:205` |
| `LeafContents::AgentCtrlFleet` (`is_persisted` false) | `app_state.rs:212`, `:233` |
| restore arm (Err, like NetworkLog) | `pane_group/mod.rs:2116` |
| persistence arms | `persistence/sqlite.rs:1304`, `:1554` |
| launch config arm | `launch_configs/launch_config.rs:230` |
| `TypedPane::Other` | `workspace/view/vertical_tabs.rs:4342` |
| open handler (dedupe + `add_pane_with_direction(Right)`) | like `open_network_log_pane` `view.rs:17427` |

**Produces:**
```rust
pub struct FleetModel { runs: Vec<RunSummary>, repos: Vec<RepositoryInfo>, last_error: Option<String>, refreshing: bool, attached_views: usize }
impl FleetModel { // SingletonEntity
    pub fn groups(&self) -> Vec<FleetGroup>;            // sorted by repo name; `(no repository)` last
    pub fn refresh(ctx);                                // blocking::unblock(list_runs + list_repositories) → update
    pub fn start(repo_url: String, agent: CLIAgent, ctx); // start_run_on_repo then refresh
    pub fn stop(job_id: String, ctx);                   // stop_run(.., false) then refresh
    fn schedule_refresh(ctx);                           // Timer::after(15s) while attached_views > 0 (pattern: system/info.rs:133)
}
pub struct FleetGroup { pub repo_name: String, pub repo_url: Option<String>, pub rows: Vec<FleetRow> }
pub struct FleetRow { pub job_id: String, pub state: String, pub agent: String, pub branch: Option<String>, pub last_activity: Option<String> }
```
View: header "AgentCtrl fleet · staging" with Refresh and Start… (Start… shows a repo list from `repos` — rows with the three agent buttons; `reachable == false` rows disabled with the error as tooltip); groups as sections; each row: state badge, agent, branch, last activity, buttons Open (→ `WorkspaceAction::OpenManagedSession { job_id }` which runs `agent-ctrl open <job> --environment staging` in a new terminal tab via `execute_managed_session_command`) and Stop (confirm by second click within 5 s, then `FleetModel::stop`). Non-terminal states first.

- [ ] **Step 1: failing tests**: `fleet_model_tests.rs` — grouping (two runs same repo → one group, `(no repository)` last, terminal states after running ones), `refresh` applies a fetched list and clears `last_error`, an error keeps the old rows and sets `last_error`, `schedule_refresh` is a no-op with zero attached views (test the predicate helper); `view_tests.rs` — `workspace:open_agent_ctrl_fleet` binding exists; handling `OpenAgentCtrlFleetPane` adds exactly one fleet pane and a second call focuses it (pattern: network log dedupe); `OpenManagedSession{job}` runs the viewer command in a new tab.
- [ ] Step 2: run → FAIL. Step 3: implement (model, view, pane, registrations, palette, toast link). Step 4: `cargo nextest run -p warp fleet agent_ctrl`, clippy, fmt. Step 5: commit `feat(pane): AgentCtrl fleet pane`.

### Task B6: Install and fleet E2E (needs the user; after Plan A is on staging)

- [ ] `powershell -ExecutionPolicy Bypass -File ./script/windows/install-warpspeed.ps1` from the merged dev (never restart running windows).
- [ ] In a new window: open the fleet pane → Start… `kaizen` / Claude and `battery-cafe` / Pi → both appear Running; Open one; Stop the other → `agent/<job>` branch pushed.
- [ ] `warpspeed agent-ctrl move-all --under T:\projects\JustVibe` (dry plan first, then confirm) → table of moves; verify each in the fleet pane; then IWAKAN.
