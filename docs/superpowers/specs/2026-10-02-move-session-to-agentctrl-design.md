# Move a Local Agent Session to AgentCtrl — Design

Date: 2026-10-02
Status: approved; revised 2026-10-03 after mapping the code (see Revisions)
Repos: `warp` (WarpSpeed client), `stoz3n-agent-ctrl` (controller, node, adapters), dashboard backend
(`dash-backend-dev.stozn.ai`, `/api/agent-control/*`)

## Goal

A Claude, pi, or codex session running in a WarpSpeed pane can be moved to an AgentCtrl managed
run in staging. The cloud run resumes the *same* conversation on the *same* code state, and the
local pane becomes a viewer of that run. Nothing is lost if any step fails before cut-over.

## Non-goals

- Moving back from AgentCtrl to a local pane.
- Environments other than `staging` (the only `ManagedEnvironment` today).
- Context-summary handoff; this design is true resume only.
- Agents other than claude, pi, codex.

## Prerequisites

- `6c985252 feat: add self-hosted managed sessions and Windows relays` (`app/src/managed_sessions_cli.rs`,
  the `agent-ctrl` CLI) exists only on `wip/windows-to-mac-2026-09-11`. It must land on `dev` first.
- `dev` already tracks the agent session id per pane (`CliAgentSessionsModel::session_id`).

## 1. Move flow (WarpSpeed)

1. **Trigger.** "Move to AgentCtrl" in the pane context menu and command palette, plus
   `warpspeed agent-ctrl move [--pane <id>]`. The GUI entry is shown only when the pane's CLI agent
   is claude, pi, or codex and has a known session id. One confirmation dialog at trigger time
   names the agent, repo, and target environment.
2. **Preflight** (read-only; any failure aborts with a specific message):
   - Map the pane's cwd git repo and its remotes to an operator-configured AgentCtrl repo
     (`GET /api/agent-control/repos`, new, returns name + clone URL). No match → refuse.
   - Resolve the agent's session file from its session id:
     - claude: `~/.claude/projects/<slug(cwd)>/<id>.jsonl`
     - pi: `~/.pi/agent/sessions/<slug(cwd)>/*<id>*.jsonl`
     - codex: `~/.codex/sessions/YYYY/MM/DD/rollout-*-<id>.jsonl`
   - File ≤ 32 MB; dashboard token valid (`STOZN_TOKEN` or `~/.stozn.json`).
3. **Handoff branch.** Build a commit of the full working tree (tracked + untracked, respecting
   `.gitignore`) using a temporary `GIT_INDEX_FILE`, parent = `HEAD`. The user's working tree,
   index, and current branch are not touched. Push it as `handoff/<yyyymmdd-hhmm>-<short-session-id>`
   to the remote whose URL matches the operator repo.
4. **Import.** `POST /api/agent-control/runs/import` (section 2). Follow events until
   `status: resumed` or a terminal/error state, with a 120 s timeout.
5. **Cut over** (only after `resumed`): send the local agent its graceful exit (claude/codex:
   `/exit` input; pi: `/quit`), wait up to 10 s for process exit, write the bookmark
   (`agentctrl-managed-sessions.json`), and replace the pane content with the AgentCtrl viewer.
6. **Rollback.** Any failure in steps 3–4 leaves the local agent running untouched. The toast
   names the pushed branch (if any) so the user can retry or delete it. Failure after `resumed`
   but before the local agent exits: the cloud run continues; the pane still switches to the
   viewer and reports that the local process had to be force-closed.

## 2. AgentCtrl server

### Endpoint

`POST /api/agent-control/runs/import`, `multipart/form-data`:

| field | rule |
|---|---|
| `agent_name` | configured interactive agent for the agent kind (e.g. `staging-claude-interactive`) |
| `repo` | name of an operator-configured repo; anything else → 422 |
| `handoff_branch` | must match `^handoff/[A-Za-z0-9._-]{1,100}$` |
| `session_id` | 1–128 chars, `[A-Za-z0-9-]` |
| `source_cwd` | ≤ 1024 chars, used only for the path note |
| `session_file` | ≤ 32 MB, else 413 |

Same bearer auth, `X-Stozn-Client`, `Idempotency-Key`, and audit trail as `start`. The
idempotency key is `move-<session_id>-<handoff commit sha>`, so a double move is rejected. The
audit record stores the SHA-256 of the session file, never its content. Response is the same
`{status: "accepted", job_id}` shape as `start`.

`GET /api/agent-control/repos` returns `[{name, clone_urls}]` for the preflight repo mapping.

### Controller / node

- The controller stores the session blob with the run (same retention as run events) and
  launches with `resume: Some(session_id)` plus the blob over the existing gRPC launch, extended
  with optional `resume_blob` and `handoff_branch` fields.
- The node creates a per-run worktree: `git fetch origin <handoff_branch>` in the persistent
  checkout, then `git worktree add <run-dir>/repo FETCH_HEAD`. The shared checkout's branch and
  dirty files are unaffected; the worktree is removed when the run ends (branch left on the forge).
- After the agent confirms the resumed session, the node emits run event `status` with content
  `resumed`, then sends one system input:
  *"This session moved from `<source_cwd>` to `<worktree>`. Paths in earlier turns refer to the
  old location."*

## 3. Adapters

New `CliAdapter` method:

```rust
fn place_session(&self, blob: &[u8], session_id: &str, cwd: &Path, run_dir: &Path)
    -> Result<ResumeRef, BackendError>;
```

`build_args(cfg, resume)` consumes the returned `ResumeRef`. `supports_resume()` becomes true for all three.

| agent | placement | resume |
|---|---|---|
| claude | `$HOME/.claude/projects/<slug(cwd)>/<id>.jsonl` | `--resume <id>` (exists today) |
| pi | `<run_dir>/session.jsonl` | `--session <path>` with `--mode rpc`; overrides the `AGENT_CTRL_PI_SESSION` default-off for this run |
| codex | `$CODEX_HOME/sessions/<today>/rollout-<id>.jsonl` | app-server resume request over `--stdio` |

**Spike first:** confirm the codex app-server resume call and its parameters (thread id vs.
rollout path) against the installed codex version before building the codex path.

## 4. WarpSpeed viewer pane

- Extract the attach loop from `managed_sessions_cli.rs` into a reusable async model
  (`ManagedSessionModel`: descriptor, cursor, event stream, input/close/stop) used by both the CLI
  and the pane.
- New pane content: event list (assistant, tool_use, stdout/stderr, user_input, status) plus an
  input box. Input/close/stop map to the existing endpoints.
- Session restore reopens a viewer pane from its bookmark instead of a shell.

## 5. Failure handling and security

- Before cut-over, the local agent stays authoritative; every error is reported with its step.
- Session transcripts can contain secrets the agent has seen. They travel only to the stozn
  dashboard over TLS, never to the forge or to Warp services. Retention follows run events.
- The repo allow-list on the server is the trust boundary; the client-side mapping is only UX.
- Handoff branches are not deleted automatically; they are the user's code.

## 6. Testing

WarpSpeed:
- Preflight: repo-remote matching, session-file resolution per agent, size cap, missing token.
- Handoff commit via temporary index includes untracked files and leaves the working tree, index,
  and branch byte-identical.
- Cut-over state machine against a fake dashboard: success, import rejected, resume timeout,
  local exit timeout.

AgentCtrl:
- `place_session` + `build_args` per adapter.
- Import endpoint: field validation, repo allow-list, 413, idempotency.
- Node per-run worktree leaves the persistent checkout unchanged.
- Staging end-to-end per agent: move a real session, then ask it about an earlier turn and get
  the right answer.

## Revisions (2026-10-03)

Mapping the three codebases changed these points; the implementation plan
(`docs/superpowers/plans/2026-10-03-move-session-to-agentctrl.md`) follows the revised form.

1. Repositories are known only to the AgentCtrl node (`AGENT_CTRL_WORKSPACES_FILE`), so there is no
   `GET /repos`. WarpSpeed sends the push remote URL as `repo_url`; the node rejects unknown repos.
2. The dashboard reaches the controller over gRPC; a new `ImportSession` RPC carries the file, and
   decode limits are raised to 40 MiB where it travels. The controller does not persist the file.
3. The viewer already exists: after cut-over the pane runs `agent-ctrl open <job>`. No new pane type.
4. No `warpspeed agent-ctrl move` CLI; only the GUI action can address a pane.
5. Import needs no `task_id`. Moved runs use dedicated agents `staging-{claude,pi,codex}-moved`
   with `workspace-write`, and staging gains the `warpspeed` workspace repository.
6. The `6c985252` prerequisite is moot: `dev` has `crates/managed_agent_sessions`.
7. Codex resume verified: `thread/resume {threadId}` loads a rollout copied into any date folder of
   a fresh `CODEX_HOME` (codex-cli 0.144.4).
