# Kimi Code and DeepSeek Harness agents — Design

Date: 2026-10-08
Status: Scope approved in discussion (2026-10-07/08); spec pending review
Builds on: `2026-10-02-move-session-to-agentctrl-design.md`, `2026-10-04-agentctrl-fleet-design.md`,
`2026-10-07-agentctrl-pause-and-second-node-design.md`.
Target: WarpSpeed `dev`, the staging AgentCtrl (`agent-ctrl` namespace), the staging LiteLLM
(`gateway-dev.mezzanin.eu`, passiv).

## Goal

Run Kimi Code and DeepSeek Harness at the same level as claude, pi and codex: in a WarpSpeed pane,
as AgentCtrl runs (start, start-on-repo, fleet, pause/resume), and moved from a pane to AgentCtrl.

## The two agents

| | Kimi Code | DeepSeek Harness |
|---|---|---|
| Source | github.com/MoonshotAI/kimi-code (TS, MIT), npm `@moonshot-ai/kimi-code` 2.1.1 | github.com/deepseek-ai/deepseek-harness (TS, MIT), npm `@deepseek-ai/dsh` (alpha 0.2.1-alpha.1); "Cordis" is its plugin framework |
| Command | `kimi` | `dsh` (also `npx @deepseek-ai/dsh`) |
| Interactive terminal UI | yes (pi-tui fork; `regular` mode by default, optional fullscreen) | **none shipped** — base profiles are web, headless, acp, sdk; the "turtle-ui" TUI is not public |
| Resume | `kimi -S/--session <id>` (refuses another cwd), `kimi -c` (newest in cwd) | `dsh headless --session-id <id>`; ACP `session/resume` |
| Data root | `$KIMI_CODE_HOME` or `~/.kimi-code` | `$DSH_HOME` or `~/.dsh` |
| Session on disk | dir `sessions/<workDirKey>/<sessionId>/` with `state.json` + `agents/<id>/wire.jsonl`; index `session_index.jsonl` | one file `sessions/--<projectKey>--/<encodedId>/session.vN.jsonl.zstd` (highest N is current) |
| Session id | `session_<uuid-v4>` | ACP: bare UUID; web: `session-<uuid>` |
| ACP | `kimi acp`: `session/load` (replays), `session/resume`; modes default/plan/auto/yolo | `dsh --profile acp`: `session/resume` only (same cwd, no replay) |
| Lifecycle signals | TOML `[[hooks]]` (stdout captured, no terminal); TUI emits OSC 9 under Warp | in-process Cordis plugin events |

Key formats (from source):
- Kimi `workDirKey` = `wd_<slug>_<hash>`: normalize the cwd (`\`→`/`, strip trailing `/`, keep case);
  `slug` = last segment lowercased, runs of `[^a-z0-9._-]`→`-`, trim `-`, cut to 40, trim, empty/`.`/`..`
  → `workspace`; `hash` = first 12 hex of sha256(normalized path).
- dsh `projectKey`: `/`, `\`, `:` → `-` (runs collapsed); keep `[A-Za-z0-9._-]`, others → `~XXXX`
  (4 hex); strip leading `-`; empty → `root`; cut to 251. Line 1 of the session file is
  `{"type":"session","version":4,"id","createdAt","cwd",…}`; loading rejects a file whose location
  does not match `header.cwd`.

## Non-goals

- A local interactive dsh pane (there is no dsh TUI to host). Pane-level resume and composer for dsh
  activate automatically if a TUI profile is installed later; no further work is planned for that.
- A WarpSpeed-hosted ACP client UI, or writing our own dsh TUI.
- Warp's cloud `Harness` enum (Oz orchestration): pi is not there either.
- Prod AgentCtrl (staging only, as for the other agents).
- Changing the user's local Kimi/dsh model or telemetry settings (only the hook/plugin entries are
  added locally).

## Parts, specs, plans

One spec (this file), four plans, built in order. A and B are independent; C needs B; D needs A and B.

- **A (warp):** local WarpSpeed integration, including the out-of-band agent event channel.
- **B (stoz3n-agent-ctrl):** adapters, ACP resume, node image, profiles, LiteLLM, credentials.
- **C (stoz3n-agent-dash):** labels, badges, session-id prefixes.
- **D (warp + stoz3n-agent-ctrl):** move a pane session to AgentCtrl; session read-back for pause.

Branches: `feat/kimi-dsh-agents` in each repo, based on `dev`, pushed to Gitea (and `gitlab` for warp).
WarpSpeed worktree: `T:\projects\dev-ops\warp-kimi-dsh`.

---

## A — WarpSpeed local

### A·1 Agent kinds

Add `CLIAgent::Kimi` and `CLIAgent::DeepSeekHarness`; display names "Kimi Code" and "DeepSeek
Harness". `command_prefixes`: Kimi `["kimi"]`; dsh `["dsh"]`, plus detection of
`npx [-y] @deepseek-ai/dsh …` in `detect`. The OSC `agent` names the events carry (`kimi`, `dsh`) are
among the prefixes so `event::v1::resolve_agent` maps them.

Every exhaustive match gets both variants (the compiler finds them); the non-exhaustive lists that
must be extended by hand:
- `listener::is_agent_supported` and `create_handler` (both → `DefaultSessionListener`; Kimi gets a
  `KimiSessionHandler` that also accepts its OSC 9 fallback, see A·4).
- `cli_agent_sessions::recover_session_id_from_history` allow-list (`mod.rs` ~:421).
- `supports_bash_mode`: Kimi yes (`!` on an empty prompt enters shell mode); dsh no.
- `input_classifier::util` one-off command keywords: `kimi`, `dsh`.
- `brand_icon_color` if the brand colour is light.

Per-agent values:

| | Kimi | dsh |
|---|---|---|
| Skill providers | `Agents` (Kimi reads `~/.agents/skills` and project `.agents/skills`; not `.claude/skills`) | `[]` until a TUI exists |
| Skill prefix | `/` (`/skill:<name>`, bare `/name` falls back to the skill) | `/` |
| Rich-input submit | `BracketedPaste` (pi-tui supports it) | `Inline` |
| Windows image paste | Alt+V (same branch as Claude in `view.rs`) | none |
| Notification title | "Notification from Kimi Code" | "Notification from DeepSeek Harness" |
| Session pill / tab label | `display_name()` | `display_name()` |
| Telemetry `CLIAgentType` | `Kimi` | `DeepSeekHarness` |

Icons: `kimi.svg`, `deepseek.svg` in `app/assets/bundled/svg/`, `Icon::Kimi`, `Icon::DeepSeek`.

### A·2 Resume

`CliAgentResumeDescriptor::resume_command`:
- Kimi → `kimi --session <id>` (Kimi refuses a session from another cwd; restore always reopens the pane
  in its recorded cwd).
- dsh → `None` (no interactive surface); changes only if a TUI profile appears.

`session_id_from_command`: Kimi `-S/--session/-r/--resume <id>`; dsh `--session-id <id>`. A bare
`kimi -c` or `kimi` records no id; it is recovered from history (A·3) at capture time, as for Pi.

`workspace::view::resolve_agent_resume_command` (fork-at-last-turn) stays Claude/Pi only; Kimi falls
back to the plain resume command.

### A·3 Session history

`agent_session_history` (`describe`, `candidates`, `scan`, `session_id_from_command`):
- **Kimi:** read `<home>/session_index.jsonl` (`{"sessionId","sessionDir","workDir"}` per line; the
  last line for an id wins) for id → dir → cwd; title, `lastPrompt` and `updatedAt` from
  `<sessionDir>/state.json`. Fall back to scanning `sessions/*/*/state.json` when the index is missing.
- **dsh:** walk `<home>/sessions/--*--/*/`, take the highest `session.vN.jsonl[.zstd]`, read only the
  first frame/line (`zstd` crate, already in `Cargo.lock`, made a direct dependency of `app`) for id,
  `cwd`, `createdAt`; file mtime for "last active". Title: first user message if found in the first
  64 KiB of decompressed data, otherwise none.

Both feed the session picker (`find_all`) and recovery (`find_recent`).

### A·4 Agent events without a terminal: `warp agent-event`

Kimi hooks run with captured stdout (on Windows under `cmd.exe` with no console), and a dsh plugin
runs inside a process whose stdout is a protocol. Neither can write OSC 777 to the pane, so both send
their events to WarpSpeed over the remote-control pipe instead.

- **Pane token:** every local pane's shell environment gets `WARP_AGENT_EVENT_TOKEN` (random 128-bit
  hex, generated at pane creation, kept with the pane's session so it survives detach/attach through
  the supervisor). Set next to `WARP_CLI_AGENT_PROTOCOL_VERSION` in both `local_tty` environments.
- **Binary path:** the pane env also gets `WARP_AGENT_EVENT_BIN`, the absolute path of the running
  WarpSpeed binary, so hooks never hard-code a versioned install path. Outside a WarpSpeed pane the
  variable is empty, the hook command fails, and Kimi's hooks fail open.
- **CLI:** `warp agent-event --format <kimi-hook|protocol>` reads one JSON object from stdin. With
  `kimi-hook` it maps a raw Kimi hook payload to a `CLIAgentNotification` (protocol v1, table below);
  with `protocol` the input already is one (the dsh plugin builds it). It then sends
  `Request::CliAgentEvent { token, notification }` to every live instance (`read_markers`). Exits 0
  silently when the token is missing, no instance answers, or no pane owns the token — a hook must
  never fail or block the agent. Timeout 2 s.
- **Instance:** the handler finds the pane whose token matches and feeds the notification into the
  same path OSC 777 events take (`CLIAgentSessionsModel`), so the status, notifications, footer and
  `transcript_path` capture behave exactly as for Claude.
- The token never leaves the machine and only addresses a pane; the pipe keeps its current-user
  access rules.

Kimi hook payload → protocol event:

| Kimi hook | protocol `event` | extra fields |
|---|---|---|
| `SessionStart` (`source` startup/resume) | `session_start` | `session_id`, `cwd`, `transcript_path` = session dir |
| `UserPromptSubmit` | `prompt_submit` | `query` |
| `PermissionRequest` | `permission_request` | `tool_name`, `tool_input`, `summary` = `display` |
| `PermissionResult` | `permission_replied` | |
| `PostToolUse` | `tool_complete` | `tool_name`, `tool_input` |
| `Stop` | `stop` | |
| `StopFailure` | `stop_failure` | `error_type` |
| `Interrupt` | `idle_prompt` | |
| `SessionEnd` | (ends the session) | |

Kimi's own OSC 9 (sent under `TERM_PROGRAM=WarpTerminal` on approval, question, turn done) stays as a
fallback through `KimiSessionHandler`, which treats it like Codex's OSC 9 (→ `stop`) unless a hook
event for the same turn already arrived.

### A·5 Installers

Both in `plugin_manager`, each behind a feature flag (`KimiNotifications`, `DshNotifications`, in
`DOGFOOD_FLAGS`), with the install/update UX the Codex and OpenCode installers have.

- **Kimi:** add `[[hooks]]` entries (only `event`, `matcher`, `command`, `timeout`; anything else stops
  Kimi loading its config) to `<home>/config.toml` for the events in A·4, each
  `command = "\"%WARP_AGENT_EVENT_BIN%\" agent-event --format kimi-hook"` on Windows (hooks run
  under `cmd.exe`) and `"\"$WARP_AGENT_EVENT_BIN\" agent-event --format kimi-hook"` elsewhere,
  `timeout = 5`. Idempotent: entries are
  recognised by that command and rewritten on update; the user's other entries and formatting are kept
  (`toml_edit`). Installed version recorded in a comment line next to the entries.
- **dsh:** a Warp-owned Cordis plugin package `@stozn/dsh-warp` (source in the warp repo under
  `app/resources/dsh-warp/`, published to the Gitea npm registry; installed with
  `dsh plugin --profile <p> add @stozn/dsh-warp` for each installed profile, which needs pnpm on PATH
  — the installer says so if it is missing). The plugin subscribes to:

  | Cordis event | protocol `event` |
  |---|---|
  | `agent/created` (`source` startup/resume) | `session_start` (`session.header.id`, `.cwd`, `transcript_path` = session file) |
  | `agent/inbox/claimed` | `prompt_submit` |
  | `tools/post-execute` | `tool_complete` |
  | `approval/asked` | `permission_request` |
  | `approval/decided` | `permission_replied` |
  | `agent/turn-stopping` | `stop` |
  | `agent/status` → `idle` | `idle_prompt` |

  It builds the protocol v1 JSON itself (`agent: "dsh"`) and spawns
  `$WARP_AGENT_EVENT_BIN agent-event --format protocol` with it on stdin, fire-and-forget, so it
  works the same under web, headless and ACP profiles. Without `WARP_AGENT_EVENT_BIN` the plugin does
  nothing.

### A·6 Tests

- Unit: detection (incl. `npx @deepseek-ai/dsh`), resume commands, `session_id_from_command`, footer
  strategy, classifier keywords, `agent-event` payload mapping for every row of both tables, the
  remote-control `CliAgentEvent` handler (token match, no match, unknown token), Kimi `config.toml`
  install/update/idempotence (with existing user hooks), history scanners against fixture trees for
  both agents (including a zstd fixture and a missing Kimi index).
- Real machine (Windows): install both CLIs; in a pane run `kimi`, check icon, footer, status changes
  (working → needs approval → done), image paste, `!` shell mode, restart WarpSpeed and confirm the
  pane resumes the Kimi session; run `dsh headless "…"` in a pane, confirm icon, status events from the
  plugin, and that the session appears in the picker.

---

## B — AgentCtrl server (stoz3n-agent-ctrl)

### B·1 ACP driver: resume

`spawn_acp_driver` (`src/agent/process.rs`) gains resume: when `resume` is set and the agent's
`initialize` result advertises `sessionCapabilities.resume`, send `session/resume {sessionId, cwd,
mcpServers: []}` instead of `session/new`; else, if `agentCapabilities.loadSession`, send
`session/load` and discard the replayed `session/update`s until its response. The resumed id is
reported as the run's `session_id`, which `await_resume` already waits for. Gemini and OpenCode gain
resume from the same change where their CLIs support it (no profile changes for them in this work).

Permission requests stay auto-rejected by the driver; the rw profiles avoid them via mode (B·2).

### B·2 Adapters

New `src/agent/kimi.rs` and `src/agent/dsh.rs` implementing `CliAdapter` with
`interactive_protocol() = Acp`, registered in `src/agent/mod.rs` (`mod`, `pub use`,
`discover_backends`), `Backend::{Kimi, Dsh}` (`"kimi"`, `"dsh"`) in `src/config/types.rs` with
`supports_input() = true`, and both names in `parse_backend` (`src/remote/mod.rs`; add the missing
`"openclaw"` there too). The note-timing match in `node.rs` puts both with the "note after resume
confirmation" group, like Pi.

- **kimi:** command `kimi acp`. Permission mode: profile `permission_mode` `bypassPermissions`/`auto`
  → after `session/new|resume`, `session/set_mode {modeId: "auto"}` (`auto` is Kimi's never-ask mode;
  `yolo` still asks for dangerous commands). Model via env (B·4). Sessions under
  `KIMI_CODE_HOME=<run_dir>/kimi-home` so runs never share an index.
- **dsh:** command `dsh --profile acp`. Env `DSH_HOME=<run_dir>/dsh-home` seeded from the node's
  `~/.dsh` template (profiles + `cordis.patch.yml`), `DSH_PERMISSION_MODE=danger-full-access` for rw
  profiles (`workspace-write` otherwise), `DSH_TELEMETRY_DISABLED=1`.
- `place_session` / `read_session` per D·2.

Tests: adapter unit tests; fake ACP server cases in `process.rs` for `session/resume`,
`session/load` (with replay to discard), and neither capability (falls back to `session/new` and the
run errors as "cannot resume"); `validation.rs` backend list.

### B·3 Image, profiles, workers

- `Dockerfile`: ARGs `KIMI_CODE_VERSION=2.1.1`, `DSH_VERSION=0.2.1-alpha.1`; `npm install -g
  @moonshot-ai/kimi-code@… @deepseek-ai/dsh@…` (both need Node ≥ 22.19; bump the base image's Node if
  it is older). `deploy/staging/obachan/Dockerfile` likewise.
- `configmap.tmpl.yaml`: `kimi-worker`, `dsh-worker`, `agent_by_backend` entries;
  `staging-kimi-interactive-rw` and `staging-dsh-interactive-rw` (`requires_repo`, same shape as the
  pi/codex rw profiles); path-list entries; a `dsh-home/cordis.patch.yml` file (B·4, B·5) seeded into
  `~/.dsh` by the `seed-agent-auth` init container.
- `test_shared_workspaces.py`: extend the backend tuples, loops and shared-checkout set.

### B·4 LiteLLM

Models are added to passiv's model zoo (MongoDB → LiteLLM DB), staging:
`passiv zoo add kimi-k2 --provider openrouter --backend-model <moonshotai/kimi-k2 id>` and
`passiv zoo add deepseek-v4 --provider openrouter --backend-model <deepseek id>`, then
`passiv zoo apply --env staging --confirm`. Exact OpenRouter ids are looked up when applying. The
scoped `litellm_api_key` must be allowed to use both aliases.

New `staging.toml` keys `kimi_model = "kimi-k2"`, `dsh_model = "deepseek-v4"`, rendered by
`render.py` (defaults in `_DEFAULTS`):
- kimi profiles: `KIMI_MODEL_NAME={kimi_model}`, `KIMI_MODEL_PROVIDER_TYPE=openai`,
  `KIMI_MODEL_BASE_URL={passiv_base_url}`, `KIMI_MODEL_API_KEY` from `OPENAI_API_KEY` (the
  `model_credentials` secret; the adapter copies it at launch), `KIMI_MODEL_MAX_CONTEXT_SIZE=200000`.
- dsh: the seeded `cordis.patch.yml` defines a pi-ai route `passiv` (`api: openai-completions`,
  `baseURL: {passiv_base_url}`, `apiKeyEnv: OPENAI_API_KEY`, `models: [{id: {dsh_model}}]`) and
  overrides the `acp` row's `config: {provider: passiv, model: {dsh_model}}` (a patch replaces the
  row's whole config).

### B·5 dsh telemetry off on nodes

`DSH_TELEMETRY_DISABLED=1` in every dsh profile, and the seeded `cordis.patch.yml` disables
`session-log-deepseek` and `plugin-package-inventory-deepseek` (`- id: … / disabled: true`). Because
dsh uses the pi-ai route, no `dsh_session_log` field is sent even if one were enabled. A render test
asserts both.

---

## C — Dashboard (stoz3n-agent-dash)

- `server/agent-control.ts` `backendLabel`: `kimi: "Kimi"`, `dsh: "DeepSeek"`.
- `src/pages/AgentRunners.tsx` `AgentBadge` colours (Kimi blue `#1783FF`, DeepSeek indigo `#4D6BFE`);
  page text lists all five agents.
- `server/app.ts` `canonicalAgentRunnerSessionId` prefix regex adds `kimi|dsh`.
- `scripts/check-agent-control.mjs`: display-name cases for both.

---

## D — Move session and pause read-back

### D·1 WarpSpeed side

- `managed_agent_sessions`: `agent_ctrl_profile` and `AGENT_KINDS` gain `kimi`, `dsh`
  (`staging-{kimi,dsh}-interactive-rw`); `warp_cli` `--agent` help text lists all five.
- `agent_session_handoff.rs`:
  - `agent_ctrl_profile(CLIAgent)`: Kimi, DeepSeekHarness.
  - `agent_session_root`: Kimi `<home>/sessions`; dsh `<home>/sessions`.
  - `session_file_path` / `resolve_session_file`: Kimi → the session dir (from the index), packed into
    a tar (`<sessionId>/state.json`, `<sessionId>/agents/**`) in memory; dsh → the highest
    `session.vN.jsonl[.zstd]` as is. Both stay under the 32 MiB import cap or the move is refused with
    that reason.
  - `is_reported_session_file`: accepts the reported `transcript_path` (session dir for Kimi, file for
    dsh) under the root.
  - Exit command: Kimi `/exit`; dsh (no pane TUI) — a dsh pane runs `headless`, which has ended by the
    time a move is offered, so no exit is sent.
- `STARTABLE_AGENTS` in `fleet_view.rs` and `workspace/view.rs` gain Kimi and DeepSeekHarness (start
  on repo, fleet start). `move-all` needs no change (`agent_from_label`).
- A dsh pane is movable only after its headless run ended (a recorded, inactive session), which is
  what ACP `session/resume` requires anyway.

### D·2 Node side

`place_session(bytes, session_id, cwd, run_dir)`:
- **kimi:** untar into `<KIMI_CODE_HOME>/sessions/<workDirKey(cwd)>/<sessionId>/`; rewrite
  `state.json` `cwd` and each `agents.*.homedir` to the node paths; append
  `{"sessionId","sessionDir","workDir"}` to `session_index.jsonl`. Resume via ACP `session/resume`
  (B·1), which locates the session by id and takes the cwd from `state.json`.
- **dsh:** decompress, replace line 1's `cwd` with the node worktree path, recompress (fresh zstd
  frames with checksums), write to `<DSH_HOME>/sessions/--<projectKey(cwd)>--/<encodedId>/session.vN.jsonl.zstd`
  keeping N. Resume via ACP `session/resume` with the same `cwd`.
- Validation reuses `validate_session_id` (extended for `session_<uuid>` and bare/`session-` UUIDs)
  and rejects tar entries that are absolute, contain `..`, or are links.

Other absolute paths inside the transcripts (system prompt, tool events) keep the local paths; the
path note sent as the first turn already tells the agent where it now is.

`read_session(session_id, run_dir)` (the reverse, for the pause branch's `AgentCompleted.session_file`):
kimi → tar of the session dir; dsh → the current session file. Added on the pause branch's hook when
that lands; on `dev` it is implemented and unit-tested but unused.

### D·3 Tests and E2E

- Unit (warp): tar packing, dsh file pick, profile mapping, size cap.
- Unit (agent-ctrl): `workDirKey` and `projectKey` against vectors computed by the CLIs' own code
  (fixtures checked in), kimi untar + rewrite, dsh rewrite round-trip (decompress → cwd → recompress →
  dsh-compatible), tar path validation.
- Staging E2E, same as the earlier agents: for each of kimi and dsh — start a fresh run on a repo,
  send a prompt, get an answer; import a local session (Kimi from a pane, dsh from a headless run),
  confirm `resumed` and that the agent recalls a codeword from before the move; edit a file, end the
  run, confirm the push-back commit on the handoff branch.

## Risks and open items

- **dsh is alpha** (0.2.1-alpha.1) and its interfaces may change; versions are pinned in the image
  and the plugin declares its tested `@deepseek-ai/cordis` range.
- **Kimi config strictness:** an unknown key in `[[hooks]]` makes Kimi refuse its whole config; the
  installer writes only the four allowed keys and a test parses the result with Kimi's schema rules.
- **Moved Kimi sessions:** whether Kimi re-renders its system prompt for the new cwd on resume is
  unverified; the path note covers it either way.
- **pnpm requirement** for installing the dsh plugin locally.
- **Model ids** on OpenRouter for Kimi K2 and DeepSeek are looked up at apply time.
