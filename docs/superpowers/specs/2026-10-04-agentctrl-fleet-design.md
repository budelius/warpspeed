# AgentCtrl Fleet — Design

Date: 2026-10-04
Status: Spec A·1 approved in discussion; rest pending review
Builds on: `2026-10-02-move-session-to-agentctrl-design.md` (moves) — same repos, same endpoints.
Target: the Hetzner staging cluster (`agent-ctrl` namespace), dashboard `dash-backend-dev.stozn.ai`.

## Goal

Run the agent sessions for the IWAKAN and JustVibe repositories as a fleet on AgentCtrl: several
interactive sessions at once, started fresh in the cloud or moved from a WarpSpeed pane, each on its
own branch, all visible and controllable from WarpSpeed.

## Non-goals

- Prod AgentCtrl (staging only).
- Ticket/scheduled workers: they keep the shared-checkout model.
- Moving a cloud session back to a local pane.
- Agents other than claude, pi, codex.

## Two specs, two plans

- **Spec A (stoz3n-agent-ctrl + dashboard):** fleet runs — per-run worktrees for every interactive
  run, per-worktree locking, a node concurrency cap, start-on-repo, staging config. Prerequisite for B.
- **Spec B (warp):** fleet from WarpSpeed — start-on-repo action and CLI, move-all with recovery,
  fleet view.

---

## Spec A — fleet runs (agent-ctrl)

### A·1 Run model and locking (approved)

Every interactive run gets its own worktree:
- Fresh: in the repo's checkout `git fetch origin <base>`, then
  `git worktree add -b agent/<job_id> <root>/.runs/<job_id> origin/<base>`; `<base>` defaults to the
  repo's configured branch.
- Moved: unchanged (`origin/handoff/…`, detached).
- Non-interactive workers keep the shared checkout.

Locks (the root-wide `/workspace` lock is removed):
- **Checkout lock** `<root>/<repo>/.agent-ctrl-checkout.lock`: held around `fetch`, `worktree add`,
  `worktree remove`, and for the lifetime of a shared-checkout (non-interactive) run.
- **Worktree lock** `<root>/.runs/<job_id>.lock`: held by the interactive agent process for its
  lifetime; the startup sweep skips worktrees whose lock is taken.
- `lock_for_directory` is replaced by `lock_for_run(kind, cwd)`: interactive → worktree lock;
  shared-checkout → checkout lock of the repo containing `cwd`.
- **Concurrency cap**: `AGENT_CTRL_MAX_INTERACTIVE_RUNS` on the node (staging: 4). Over the cap the
  node NACKs `node at capacity`; the controller returns it as the command's rejection reason and the
  dashboard maps it to 409 `node_at_capacity`.

Run end (any terminal state): dirty tree committed as `agent-ctrl run <job_id> end`, pushed to
`agent/<job_id>` (fresh) or the handoff branch (moved); worktree removed; a failed push keeps the
worktree and tells the viewer. Startup sweep: delete `.runs/*.session`; prune worktrees whose lock is
free and whose branch tip equals `origin/<branch>`; log and keep the rest.

### A·2 Start on a repo

- `SpawnAgentRequest` gains `repo_url` (string, 1..512, allowlist-checked with `normalize_repo_url`)
  and `base_branch` (optional, `^[A-Za-z0-9._/-]{1,100}$`, no `..`, no leading `-`).
- `normalize_repo_url` treats `git@gitlab.com:pseekoo/x.git` and `https://gitlab.com/pseekoo/x.git`
  as the same repo (scp-style → `https://<host>/<path>`).
- `SpawnCommand` gains `run_repo_url` and `run_base_branch`; the node creates the worktree before
  spawning (same background-preparation path as moves: ack first, prepare in a task, failures end the
  run Errored).
- Agents: the per-kind interactive profiles `staging-claude-moved|pi-moved|codex-moved` are renamed
  `staging-claude-interactive-rw|pi-…|codex-…` and used for both fresh starts and moves; a start
  without `repo_url` on these profiles is rejected (they have no fixed working directory).
- Dashboard: `POST /api/agent-control/runs` accepts optional `repo_url` and `base_branch` (same
  validation), forwards them; `GET /api/agent-control/repos` lists the node's configured repositories
  (new controller RPC `ListRepositories`, served from the node's registration snapshot, which gains
  the repo list) so WarpSpeed can offer only valid targets. `GET /runs` items gain `repo_url` and
  `branch` (persisted on the controller's job record and in the Mongo read model).

### A·3 Staging configuration

- Workspace repositories for staging gain the IWAKAN and JustVibe repositories (all gitlab.com/pseekoo
  except two github.com/budelius mirrors), each with its default branch as `branch`:
  - IWAKAN: be-curious.io (main), kaizen (main), martin-wieser.me (develop), me (main),
    mist-kibl.at (develop), pseekoo.ai (develop), pseekoo.at (main), pseekoo.com (develop),
    pseekoos-economics-and-philosophies (main), cdn.pseekoo (master), www-pseekoo-com (master),
    be-curious-io [github] (main), www-pseekoo-ai-attic (master).
  - JustVibe: battery-cafe (main), cefa (main), gakushu (master), headless-mailing-webcomponent (main),
    headless-shop-webcomponent (main), headless-tryon-webcomponent (main), images (master),
    gummitechnik-mihet (main), ng-history (main), shop-admin (main), solscrap (main),
    staats-druckerei (main).
  - Repos with no upstream or marked attic (`truth-voted`, `be-curious-io-attic/-wsl`) are left out.
- Node credentials: staging keeps `GITLAB_TOKEN`/`GITHUB_TOKEN` in `~/.git-credentials`; the deploy
  verifies the node can `git ls-remote` every configured repo at startup and reports failures in the
  registration snapshot (visible in `GET /repos`).
- The workspace-sync CronJob's `repos.txt` is generated from the same repository list (removes the
  drift noted in review).
- Node resources: memory limit raised from 8Gi to 12Gi with `AGENT_CTRL_MAX_INTERACTIVE_RUNS=4` (staging hosts have ~15 GiB allocatable each; if scheduling fails, fall back to 8Gi and a cap of 3).

### A·4 Testing

- Lock tests: two interactive runs on the same repo run concurrently; a shared-checkout worker and an
  interactive run on the same repo run concurrently; two shared-checkout workers on one repo are
  serialized; cap exceeded → NACK with the exact message.
- Fresh-start test: worktree on `agent/<job>` from `origin/<base>`; run end pushes the commit;
  sweep leaves a locked worktree and prunes a pushed, unlocked one.
- URL normalization tests for scp-style vs https.
- Dashboard: `repo_url`/`base_branch` validation, `/repos`, 409 `node_at_capacity`.
- Staging E2E: start claude on `kaizen` and pi on `battery-cafe` concurrently, each edits a file,
  both branches receive push-back commits.

---

## Spec B — fleet from WarpSpeed (warp)

### B·1 Start on AgentCtrl in this repo

- Pane menu / palette `Start agent on AgentCtrl in this repo` (`workspace:start_agent_on_agent_ctrl`):
  resolves the pane's repo remote (gitea, else origin), checks it against `GET /repos`, asks which
  agent (claude/pi/codex) and optional base branch, calls `POST /runs` with `repo_url`, and opens the
  viewer (`agent-ctrl open <job>`) in the pane.
- CLI: `warpspeed agent-ctrl start --repo <url> --agent <claude|pi|codex> [--branch <b>] [--prompt …]`;
  `agent-ctrl repos` lists valid targets.

### B·2 Move all sessions in a folder

- Palette `Move all agent sessions under <folder> to AgentCtrl` (and
  `warpspeed agent-ctrl move-all --under T:\projects\IWAKAN`): enumerates running agent panes across
  all local WarpSpeed instances (`sessions panes`), and for each pane resolves agent kind, session id
  and cwd. **Recovery rule** (from the user): when the pane/launch record has no cwd or agent, recover
  them from the agent-session tracking — `CLIAgentSessionsModel` (agent, hook-reported cwd and
  session id), the agent-session history store, and the agent's own transcript path — and use the
  transcript's project directory as the repo path.
- Panes whose repo is not a configured target are listed as skipped with the reason.
- Moves run sequentially (one cut-over at a time) using the existing move flow; a summary table
  (pane, repo, agent, job id or failure) is shown at the end and written to the session log.

### B·3 Fleet view

- New pane type `AgentCtrlFleetPane`: table of cloud runs from `GET /runs` grouped by repo — state,
  agent, branch, last event time — refreshed every 15 s while visible. Row actions: Open (viewer in
  the adjacent pane or a new tab), Stop, Send input (one line). Toolbar: Start… (same dialog as B·1
  with a repo picker from `/repos`), filter by folder/repo.
- Opened from the palette (`workspace:open_agent_ctrl_fleet`) and from the success toast of a move.
- Registered like `NetworkLogPane` (pane type, PaneId, restore arm, vertical-tabs arm).

### B·4 Testing

- Unit: remote → repo matching against `/repos`, recovery rule with each source present/absent,
  move-all ordering and skip reasons, fleet table grouping/refresh model.
- Flow tests on the workspace for start/open/stop wiring, as for the move feature.
- Manual: start two IWAKAN sessions from the fleet view, move one JustVibe session, confirm both show
  in the view and can be opened from a fresh window.

## Rollout order

1. Spec A on agent-ctrl + dashboard, merged to `dev` (deploys staging).
2. Staging repo list + node resources; verify `GET /repos` shows all IWAKAN/JustVibe repos reachable.
3. Spec B in WarpSpeed; install locally.
4. Move the JustVibe sessions first (user request), then IWAKAN, from the fleet view.
