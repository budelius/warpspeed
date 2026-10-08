# Kimi Code and DeepSeek Harness agents — Plan B (AgentCtrl server)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** AgentCtrl runs Kimi Code (`kimi`) and DeepSeek Harness (`dsh`) as interactive ACP runs and one-shot ticket runs on staging, routed through passiv, with dsh telemetry off, and the ACP driver can resume a session by id.

**Architecture:** The shared ACP driver in `src/agent/process.rs` learns to resume (`session/resume`, else `session/load` with the replayed history discarded) and to apply a session mode before the session counts as open. Two new `CliAdapter`s (`src/agent/kimi.rs`, `src/agent/dsh.rs`) build the CLI arguments and per-run environment. The node passes each isolated run a private run dir (`AGENT_CTRL_RUN_DIR`) so each CLI keeps its session home there and the node removes it with the run. The deploy renderer adds the profiles, the two ticket workers, the Kimi model env and the dsh `--patch` overlays. The image installs both CLIs. passiv gets two OpenRouter aliases.

**Tech Stack:** Rust 1.94 (tokio, tonic, serde_json), Python 3.11+ stdlib (`deploy/k8s/render.py`, `unittest`), Docker (node image), passiv CLI (LiteLLM model zoo).

Spec: `T:/projects/dev-ops/warp-kimi-dsh/docs/superpowers/specs/2026-10-08-kimi-dsh-agents-design.md`, part B.

## Global Constraints

From the spec, verbatim:

- Kimi: command `kimi acp`; profile `permission_mode` `bypassPermissions`/`auto` → after `session/new|resume`, `session/set_mode {modeId: "auto"}`; sessions under `KIMI_CODE_HOME=<run_dir>/kimi-home` so runs never share an index.
- dsh: command `dsh --profile acp`; `DSH_HOME=<run_dir>/dsh-home`; `DSH_PERMISSION_MODE=danger-full-access` for rw profiles (`workspace-write` otherwise); `DSH_TELEMETRY_DISABLED=1`.
- `Backend::{Kimi, Dsh}` (`"kimi"`, `"dsh"`) with `supports_input() = true`; both names in `parse_backend`, plus the missing `"openclaw"`. The note-timing match in `node.rs` puts both with the "note after resume confirmation" group, like Pi.
- Resume: when `resume` is set and `initialize` advertises `sessionCapabilities.resume`, send `session/resume {sessionId, cwd, mcpServers: []}` instead of `session/new`; else, if `agentCapabilities.loadSession`, send `session/load` and discard the replayed `session/update`s until its response. The resumed id is reported as the run's `session_id`. Permission requests stay auto-rejected by the driver.
- `Dockerfile`: ARGs `KIMI_CODE_VERSION=2.1.1`, `DSH_VERSION=0.2.1-alpha.1`; `npm install -g @moonshot-ai/kimi-code@… @deepseek-ai/dsh@…` (both need Node ≥ 22.19). `deploy/staging/obachan/Dockerfile` likewise.
- `configmap.tmpl.yaml`: `kimi-worker`, `dsh-worker`, `agent_by_backend` entries; `staging-kimi-interactive-rw` and `staging-dsh-interactive-rw` (`requires_repo`, same shape as the pi/codex rw profiles).
- `staging.toml`: `kimi_model = "kimi-k2"`, `dsh_model = "deepseek-v4"`, defaults in `_DEFAULTS`. Kimi profiles: `KIMI_MODEL_NAME={kimi_model}`, `KIMI_MODEL_PROVIDER_TYPE=openai`, `KIMI_MODEL_BASE_URL={passiv_base_url}`, `KIMI_MODEL_API_KEY` from `OPENAI_API_KEY` (the adapter copies it at launch), `KIMI_MODEL_MAX_CONTEXT_SIZE=200000`.
- dsh: a pi-ai route `passiv` (`api: openai-completions`, `baseURL: {passiv_base_url}`, `apiKeyEnv: OPENAI_API_KEY`, `models: [{id: {dsh_model}}]`) and the `acp` row's `config: {provider: passiv, model: {dsh_model}}` (a patch replaces the row's whole config). `DSH_TELEMETRY_DISABLED=1` in every dsh profile; `session-log-deepseek` and `plugin-package-inventory-deepseek` disabled (`- id: … / disabled: true`). A render test asserts both.
- LiteLLM: `passiv zoo add kimi-k2 --provider openrouter --backend-model <id>`, `passiv zoo add deepseek-v4 --provider openrouter --backend-model <id>`, `passiv zoo apply --env staging --confirm`; exact OpenRouter ids looked up when applying; the scoped `litellm_api_key` must be allowed to use both aliases.
- Staging only. Prod AgentCtrl is a non-goal.

Plan decisions (where this plan departs from or fills in the spec — see "Spec gaps" at the end):

- The node sets `AGENT_CTRL_RUN_DIR=<worktree>.session` in the env of every isolated run (fresh and moved). `<worktree>.session` is the existing `Handoff::run_dir()`, which the node already removes with the run. Adapters create `<run_dir>/kimi-home` / `<run_dir>/dsh-home` from it. Without it (shared-checkout runs) the CLI's default home is used.
- dsh model routing and telemetry opt-outs are two ConfigMap files passed with `--patch` (`/agent-config/dsh-passiv.patch.yml` for every dsh run, `/agent-config/dsh-acp.patch.yml` for ACP runs), not a file seeded into `~/.dsh`. dsh reads a user patch only from `$DSH_HOME/profiles/<profile>/cordis.patch.yml`, and each run has its own `DSH_HOME`.
- An ACP resume against an agent that advertises neither `sessionCapabilities.resume` nor `loadSession` fails the run with `the ACP agent cannot resume session <id>`; it never opens a new session.
- Kimi's never-ask mode is `auto`. The Kimi CLI help (`--auto`: "Never Ask mode"; `-y/--yolo`: "routine edits and commands run automatically; risky actions … still ask") and `agent-core-v2/src/agent/permissionPolicy/policies/{auto-mode-approve,dangerous-command-ask}.ts` agree with the spec. The mode descriptions in `acp-server/src/modes.ts` say the opposite and are wrong.
- Kimi profiles also set `KIMI_DISABLE_TELEMETRY=1` (Kimi's telemetry opt-out, `packages/telemetry/src/bootstrap.ts`).
- One-shot (ticket worker) args: Kimi `kimi [--auto] [--session <id>] -p <prompt>`; dsh `dsh --profile headless [--patch …] [--session-id <id>] <prompt>`.
- The four new profiles and the two `agent_by_backend` rows are rendered for staging only (commented out for prod, using the existing `operator_inspector_path_prefix`).
- `supports_resume()` is `true` for both adapters. `place_session` stays at the trait default, which returns `BackendError::Unsupported { backend, operation: "place_session" }` (`src/agent/process.rs:128-140`). On `dev` the only caller of `AgentRunner::resume` is the moved-session path, which calls `place_session` first. A move of a kimi/dsh session is therefore refused with "unsupported" until Plan D overrides `place_session`. Gemini and OpenCode keep `supports_resume() = false`: their ACP resume support is unverified and nothing on `dev` would call it.

Repository rules:

- Worktree `T:/projects/SOTZ3N/stoz3n-agent-ctrl-kimi-dsh`, branch `feat/kimi-dsh-agents` (already created off `origin/dev` 60fd5ae). Remote `origin` = `https://git-dev.obachan.dev/stozn/stoz3n-agent-ctrl.git` (Gitea). `gitlab-archive` is fetch-only.
- Stage explicit paths only; never `git add -A` or `git add .`.
- Every commit message ends with a blank line and `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Conventions (`CLAUDE.md`): `tracing` for logs, typed errors, tests in `#[cfg(test)]` modules next to the code, `cargo fmt`, `cargo clippy -- -D warnings`.
- Each task ends green on: `cargo test --locked` (whole crate) and, for deploy tasks, `python -m unittest discover -s deploy/k8s -p "test_*.py"`.
- Merging to `dev` builds the image and deploys staging (`.gitea/workflows/deploy.yml`). The merge needs the user's explicit go-ahead (Task 8).
- Never run `find /`. The CLI sources at `C:/Users/budel/AppData/Local/Temp/claude/T--projects-dev-ops-warp/8131b0ce-34a2-4022-9b7b-2246ac664a96/scratchpad/src/{kimi-code,deepseek-harness}` are read-only references; do not run them.
- Branch `feat/pause-and-second-node` (unmerged) also changes the `CliAdapter` trait (`locate_session`) and `src/remote/node.rs`. Whichever branch merges second rebases; when pause lands, kimi/dsh need a `locate_session` (their session lives under the run dir), which belongs with Plan D's `read_session`.

## File map

| File | Change |
|---|---|
| `src/agent/process.rs` | `RUN_DIR_ENV`, `run_home`, fallible `configure_command`, `acp_session_mode` trait method, ACP resume/load/set_mode in `spawn_acp_driver`, fake ACP server cases and tests |
| `src/agent/opencode.rs` | `configure_command` returns `Result` |
| `src/agent/kimi.rs` (new) | `KimiAdapter`, `KimiBackend`, unit tests |
| `src/agent/dsh.rs` (new) | `DshAdapter`, `DshBackend`, unit tests |
| `src/agent/mod.rs` | modules, re-exports, `discover_backends` |
| `src/config/types.rs` | `Backend::{Kimi, Dsh}` |
| `src/config/validation.rs` | backend list in the interactive-policy test |
| `src/remote/mod.rs` | `parse_backend` (`openclaw`, `kimi`, `dsh`) + test |
| `src/remote/node.rs` | `AGENT_CTRL_RUN_DIR` for isolated runs, note-timing match, tests |
| `Dockerfile`, `deploy/staging/obachan/Dockerfile` | install `kimi` and `dsh` |
| `deploy/k8s/render.py` | `_DEFAULTS`, `_kimi_env`, `_dsh_env`, `_subs` |
| `deploy/k8s/templates/configmap.tmpl.yaml` | workers, rw profiles, `agent_by_backend`, dsh patch files |
| `deploy/k8s/instances/staging.toml` | `kimi_model`, `dsh_model` |
| `deploy/k8s/test_shared_workspaces.py` | backend tuples and new render tests |

---

### Task 1: Per-run CLI home (`AGENT_CTRL_RUN_DIR`) and fallible `configure_command`

**Files:**
- Modify: `src/agent/process.rs` (trait `configure_command` :101-103, helpers after `write_placed_file` :183-192, `launch` call site :427, tests module)
- Modify: `src/agent/opencode.rs:48-64`
- Modify: `src/remote/node.rs` (import :4, `run_fresh` after `config.agent_id = …` ~:1158, `run_moved` after `config.agent_id = …` ~:1213, tests module)

**Interfaces:**
- Produces:
  - `pub(crate) const RUN_DIR_ENV: &str = "AGENT_CTRL_RUN_DIR";` in `crate::agent::process`
  - `pub(crate) fn run_home(cfg: &SpawnConfig, name: &str) -> Result<Option<PathBuf>, BackendError>` creates and returns `<run dir>/<name>`, or `None` when the run has no run dir.
  - `CliAdapter::configure_command(&self, cmd: &mut tokio::process::Command, cfg: &SpawnConfig) -> Result<(), BackendError>` (default `Ok(())`). An `Err` fails the spawn.
  - Isolated runs (fresh and moved) carry `env_vars[RUN_DIR_ENV] = <worktree>.session`.

- [ ] **Step 1: Write the failing tests in `src/agent/process.rs` (tests module, after `agent_process_does_not_inherit_control_plane_credentials`)**

```rust
    #[test]
    fn run_home_is_created_inside_the_run_dir_and_absent_without_one() {
        let run_dir = tempfile::tempdir().unwrap();
        let mut cfg = spawn_cfg(true);
        assert_eq!(run_home(&cfg, "kimi-home").unwrap(), None);
        cfg.env_vars.insert(
            RUN_DIR_ENV.to_string(),
            run_dir.path().display().to_string(),
        );
        let home = run_home(&cfg, "kimi-home").unwrap().unwrap();
        assert_eq!(home, run_dir.path().join("kimi-home"));
        assert!(home.is_dir());
    }

    #[derive(Default)]
    struct FailingConfigureAdapter;

    impl CliAdapter for FailingConfigureAdapter {
        fn name(&self) -> &'static str {
            "failing-configure"
        }
        fn binary_names(&self) -> &'static [&'static str] {
            &[]
        }
        fn build_args(&self, _cfg: &SpawnConfig, _resume: Option<&str>) -> Vec<String> {
            vec![]
        }
        fn configure_command(
            &self,
            _cmd: &mut tokio::process::Command,
            _cfg: &SpawnConfig,
        ) -> Result<(), BackendError> {
            Err(BackendError::SpawnFailed("no home".into()))
        }
    }

    #[tokio::test]
    async fn configure_command_error_fails_the_spawn_before_any_process_starts() {
        let backend = CliBackend::<FailingConfigureAdapter>::with_binary(
            std::env::current_exe().unwrap(),
        );
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = spawn_cfg(false);
        cfg.working_directory = dir.path().to_path_buf();
        let error = backend.spawn(cfg).await.err().expect("spawn must fail");
        assert_eq!(error.to_string(), "spawn failed: no home");
    }
```

- [ ] **Step 2: Write the failing node tests in `src/remote/node.rs` (tests module, after `fresh_repo_run_works_in_its_own_worktree_and_pushes_to_its_agent_branch`)**

```rust
    #[tokio::test]
    async fn fresh_run_gets_a_run_dir_outside_its_worktree_that_ends_with_the_run() {
        let tmp = tempfile::tempdir().unwrap();
        let (config, url) = handoff_fixture(tmp.path());
        let run_dir = expected_worktree(&config).with_extension("session");
        let home = run_dir.join("kimi-home");

        let (spawned, _, _) = run_fresh(&config, fresh_command(&url), |_| {
            std::fs::create_dir_all(&home).unwrap();
        })
        .await;

        assert_eq!(
            spawned.env_vars.get(RUN_DIR_ENV).map(PathBuf::from),
            Some(run_dir.clone())
        );
        assert!(!run_dir.exists(), "run dir left behind");
    }

    #[tokio::test]
    async fn moved_run_resumes_with_the_run_dir_its_session_was_placed_in() {
        let tmp = tempfile::tempdir().unwrap();
        let (config, url) = handoff_fixture(tmp.path());
        let mut h = harness().await;
        h.spawn(moved_command(&url), &config).await;
        let ack = command_ack(&mut h.messages).await;
        assert!(ack.accepted, "spawn rejected: {}", ack.error);

        let (_, resumed) = h.resumed().await;
        let run_dir = resumed.working_directory.with_extension("session");
        assert_eq!(
            resumed.env_vars.get(RUN_DIR_ENV).map(PathBuf::from),
            Some(run_dir)
        );
        h.backend.finish();
        chunks_until_completed(&mut h.messages).await;
    }
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test --locked --lib run_dir` (matches the three run-dir tests) and `cargo test --locked --lib configure_command_error`
Expected: compile errors `cannot find value RUN_DIR_ENV` / `cannot find function run_home`, and the `configure_command` signature mismatch.

- [ ] **Step 4: Implement in `src/agent/process.rs`**

Change the trait method (replace lines 101-103):

```rust
    /// Apply trusted adapter-specific process environment that cannot be
    /// represented as a CLI argument. An error fails the spawn.
    fn configure_command(
        &self,
        _cmd: &mut tokio::process::Command,
        _cfg: &SpawnConfig,
    ) -> Result<(), BackendError> {
        Ok(())
    }
```

Add after `write_placed_file`:

```rust
/// Env var naming a directory private to one isolated run, outside its worktree, that the node
/// removes with the run.
pub(crate) const RUN_DIR_ENV: &str = "AGENT_CTRL_RUN_DIR";

/// `<run dir>/<name>`, created, for a CLI that keeps its sessions in a home of its own. `None`
/// when the run has no run dir.
pub(crate) fn run_home(cfg: &SpawnConfig, name: &str) -> Result<Option<PathBuf>, BackendError> {
    let Some(run_dir) = cfg.env_vars.get(RUN_DIR_ENV).filter(|dir| !dir.is_empty()) else {
        return Ok(None);
    };
    let home = Path::new(run_dir).join(name);
    std::fs::create_dir_all(&home)
        .map_err(|e| BackendError::SpawnFailed(format!("create {}: {e}", home.display())))?;
    Ok(Some(home))
}
```

In `launch`, replace `self.adapter.configure_command(&mut cmd, &config);` with:

```rust
        self.adapter.configure_command(&mut cmd, &config)?;
```

- [ ] **Step 5: Update `src/agent/opencode.rs` `configure_command`**

```rust
    fn configure_command(
        &self,
        cmd: &mut tokio::process::Command,
        cfg: &SpawnConfig,
    ) -> Result<(), BackendError> {
        if !cfg.interactive {
            return Ok(());
        }
        let mut settings = serde_json::Map::new();
        if let Some(model) = &cfg.model {
            settings.insert("model".to_string(), serde_json::json!(model));
        }
        if let Some(agent) = cfg.env_vars.get("AGENT_CTRL_OPENCODE_AGENT") {
            settings.insert("default_agent".to_string(), serde_json::json!(agent));
        }
        if !settings.is_empty() {
            cmd.env(
                "OPENCODE_CONFIG_CONTENT",
                serde_json::Value::Object(settings).to_string(),
            );
        }
        Ok(())
    }
```

and change its import line to `use crate::agent::backend::{BackendError, SpawnConfig};`.

- [ ] **Step 6: Implement in `src/remote/node.rs`**

Line 4 becomes `use crate::agent::process::{validate_session_id, RUN_DIR_ENV};`.

In `run_fresh`, directly after `config.agent_id = Some(self.agent_id.clone());`:

```rust
        config.env_vars.insert(
            RUN_DIR_ENV.to_string(),
            handoff.run_dir().to_string_lossy().into_owned(),
        );
```

In `run_moved`, the same three lines directly after its `config.agent_id = Some(self.agent_id.clone());`.

- [ ] **Step 7: Run the tests to verify they pass**

Run: `cargo test --locked --lib run_dir` then `cargo test --locked --lib configure_command_error`
Expected: PASS (3 + 1 tests).

- [ ] **Step 8: Run the whole suite, fmt and clippy**

Run: `cargo fmt && cargo test --locked && cargo clippy --locked --all-targets -- -D warnings`
Expected: all tests pass, no clippy warnings.

- [ ] **Step 9: Commit**

```bash
git add src/agent/process.rs src/agent/opencode.rs src/remote/node.rs
git commit -m "feat(node): give isolated runs a private run dir for CLI session homes

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: ACP driver — resume, load with replay discarded, session mode

**Files:**
- Modify: `src/agent/process.rs` — trait (after `interactive_protocol` :146-148), `launch` ACP arm (:507-514), `spawn_acp_driver` (:617-815), new helpers `AcpSessionSetup`, `ACP_SET_MODE_ID`, `acp_session_request`, `open_acp_session`; tests module (fake server arms, adapters, tests)

**Interfaces:**
- Consumes: nothing from Task 1.
- Produces:
  - `CliAdapter::acp_session_mode(&self, cfg: &SpawnConfig) -> Option<String>` (default `None`): ACP `session/set_mode` mode id applied after the session opens and before it counts as open.
  - Driver behavior: `resume = Some(id)` → `session/resume` when `initialize.result.agentCapabilities.sessionCapabilities.resume` is an object, else `session/load` when `agentCapabilities.loadSession == true`, else the run fails with `the ACP agent cannot resume session <id>`. On success the run's `session_id` is `id`. `session/update`s that arrive before the `session/load` response are not emitted. With a mode, `session/set_mode {sessionId, modeId}` (JSON-RPC id `"set-mode"`) is sent after the session response; the session id is recorded and queued prompts start only after it succeeds; an error response fails the run.

- [ ] **Step 1: Add the fake ACP server arms (tests module, inside `fake_protocol_server`'s `match protocol.as_str()`, before `other => panic!`)**

Add these helpers in the tests module, next to `write_fake_message`:

```rust
    fn read_fake_message(
        lines: &mut std::io::Lines<std::io::StdinLock<'_>>,
    ) -> serde_json::Value {
        serde_json::from_str(&lines.next().unwrap().unwrap()).unwrap()
    }

    /// `initialize` capabilities as `kimi acp` 2.1.1 advertises them
    /// (`packages/acp-server/src/server.ts` `initialize`).
    fn kimi_agent_capabilities() -> serde_json::Value {
        serde_json::json!({
            "loadSession": true,
            "promptCapabilities": { "image": true, "audio": false, "embeddedContext": true },
            "sessionCapabilities": {
                "list": {}, "resume": {}, "close": {}, "delete": {}, "fork": {},
                "additionalDirectories": {}
            },
            "mcpCapabilities": { "http": true, "sse": true },
            "auth": { "logout": {} }
        })
    }

    /// `initialize` capabilities as `dsh --profile acp` 0.2.1-alpha.1 advertises them
    /// (`packages/acp/acp/src/index.ts` `initialize`): resume, no `loadSession`.
    fn dsh_agent_capabilities() -> serde_json::Value {
        serde_json::json!({
            "mcpCapabilities": { "http": true },
            "promptCapabilities": { "image": false, "audio": false, "embeddedContext": false },
            "sessionCapabilities": { "close": {}, "list": {}, "resume": {} }
        })
    }

    fn kimi_modes() -> serde_json::Value {
        serde_json::json!({
            "currentModeId": "default",
            "availableModes": [
                { "id": "default", "name": "Default" },
                { "id": "plan", "name": "Plan" },
                { "id": "auto", "name": "Auto" },
                { "id": "yolo", "name": "YOLO" }
            ]
        })
    }

    fn fake_initialize(
        lines: &mut std::io::Lines<std::io::StdinLock<'_>>,
        output: &mut impl std::io::Write,
        capabilities: serde_json::Value,
    ) {
        let initialize = read_fake_message(lines);
        assert_eq!(initialize["method"], "initialize");
        write_fake_message(
            output,
            serde_json::json!({
                "jsonrpc": "2.0", "id": 1,
                "result": { "protocolVersion": 1, "agentCapabilities": capabilities, "authMethods": [] }
            }),
        );
    }

    /// Answers one `session/prompt` for `session_id` with `text`.
    fn fake_prompt_turn(
        lines: &mut std::io::Lines<std::io::StdinLock<'_>>,
        output: &mut impl std::io::Write,
        session_id: &str,
        text: &str,
    ) {
        let prompt = read_fake_message(lines);
        assert_eq!(prompt["method"], "session/prompt");
        assert_eq!(prompt["params"]["sessionId"], session_id);
        write_fake_message(
            output,
            serde_json::json!({
                "jsonrpc": "2.0", "method": "session/update",
                "params": { "sessionId": session_id, "update": {
                    "sessionUpdate": "agent_message_chunk",
                    "content": { "type": "text", "text": text }
                }}
            }),
        );
        write_fake_message(
            output,
            serde_json::json!({
                "jsonrpc": "2.0", "id": prompt["id"], "result": { "stopReason": "end_turn" }
            }),
        );
    }
```

Then the match arms:

```rust
            "acp-resume-kimi" | "acp-resume-dsh" => {
                let kimi = protocol == "acp-resume-kimi";
                let session_id = std::env::var("AGENT_CTRL_FAKE_EXPECT_SESSION").unwrap();
                let capabilities = if kimi {
                    kimi_agent_capabilities()
                } else {
                    dsh_agent_capabilities()
                };
                fake_initialize(&mut lines, &mut output, capabilities);
                let resume = read_fake_message(&mut lines);
                assert_eq!(resume["id"], 2);
                assert_eq!(resume["method"], "session/resume");
                assert_eq!(resume["params"]["sessionId"], session_id.as_str());
                assert_eq!(
                    resume["params"]["cwd"],
                    std::env::var("AGENT_CTRL_FAKE_EXPECT_CWD").unwrap()
                );
                assert_eq!(resume["params"]["mcpServers"], serde_json::json!([]));
                let result = if kimi {
                    serde_json::json!({ "configOptions": [], "modes": kimi_modes() })
                } else {
                    serde_json::json!({ "configOptions": [] })
                };
                write_fake_message(
                    &mut output,
                    serde_json::json!({ "jsonrpc": "2.0", "id": 2, "result": result }),
                );
                if kimi {
                    let set_mode = read_fake_message(&mut lines);
                    assert_eq!(set_mode["method"], "session/set_mode");
                    assert_eq!(
                        set_mode["params"],
                        serde_json::json!({ "sessionId": session_id, "modeId": "auto" })
                    );
                    write_fake_message(
                        &mut output,
                        serde_json::json!({
                            "jsonrpc": "2.0", "method": "session/update",
                            "params": { "sessionId": session_id, "update": {
                                "sessionUpdate": "current_mode_update", "currentModeId": "auto"
                            }}
                        }),
                    );
                    write_fake_message(
                        &mut output,
                        serde_json::json!({ "jsonrpc": "2.0", "id": set_mode["id"], "result": {} }),
                    );
                }
                fake_prompt_turn(&mut lines, &mut output, &session_id, "resumed-1");
                while lines.next().is_some() {}
            }
            "acp-load" => {
                // An agent with `loadSession` but no `sessionCapabilities.resume`.
                fake_initialize(
                    &mut lines,
                    &mut output,
                    serde_json::json!({ "loadSession": true, "sessionCapabilities": { "list": {} } }),
                );
                let load = read_fake_message(&mut lines);
                assert_eq!(load["id"], 2);
                assert_eq!(load["method"], "session/load");
                assert_eq!(load["params"]["sessionId"], "session_moved");
                // Replayed history (`packages/acp-server/src/replay.ts` shapes) precedes the
                // response and must never reach the run's output.
                for update in [
                    serde_json::json!({ "sessionUpdate": "user_message_chunk",
                        "content": { "type": "text", "text": "old question" } }),
                    serde_json::json!({ "sessionUpdate": "agent_message_chunk",
                        "content": { "type": "text", "text": "OLD-ANSWER" } }),
                    serde_json::json!({ "sessionUpdate": "tool_call_update",
                        "toolCallId": "t1", "title": "old tool", "status": "completed" }),
                ] {
                    write_fake_message(
                        &mut output,
                        serde_json::json!({
                            "jsonrpc": "2.0", "method": "session/update",
                            "params": { "sessionId": "session_moved", "update": update }
                        }),
                    );
                }
                write_fake_message(
                    &mut output,
                    serde_json::json!({
                        "jsonrpc": "2.0", "id": 2,
                        "result": { "configOptions": [], "modes": kimi_modes() }
                    }),
                );
                fake_prompt_turn(&mut lines, &mut output, "session_moved", "loaded-1");
                while lines.next().is_some() {}
            }
            "acp-no-resume" => {
                fake_initialize(
                    &mut lines,
                    &mut output,
                    serde_json::json!({ "sessionCapabilities": { "close": {}, "list": {} } }),
                );
                while lines.next().is_some() {}
            }
            "acp-mode-rejected" => {
                fake_initialize(&mut lines, &mut output, kimi_agent_capabilities());
                let new_session = read_fake_message(&mut lines);
                assert_eq!(new_session["method"], "session/new");
                write_fake_message(
                    &mut output,
                    serde_json::json!({
                        "jsonrpc": "2.0", "id": 2,
                        "result": { "sessionId": "session_new", "configOptions": [], "modes": kimi_modes() }
                    }),
                );
                let set_mode = read_fake_message(&mut lines);
                assert_eq!(set_mode["method"], "session/set_mode");
                write_fake_message(
                    &mut output,
                    serde_json::json!({
                        "jsonrpc": "2.0", "id": set_mode["id"],
                        "error": { "code": -32602, "message": "Unknown modeId: auto" }
                    }),
                );
                while lines.next().is_some() {}
            }
```

- [ ] **Step 2: Add the test adapter and the failing tests (tests module, after `codex_resume_sends_thread_resume_and_adopts_thread_id`)**

```rust
    /// Resumable ACP agent whose `bypassPermissions` maps to a session mode, as for Kimi.
    #[derive(Default)]
    struct FakeResumableAcpAdapter;
    impl CliAdapter for FakeResumableAcpAdapter {
        fn name(&self) -> &'static str {
            "fake-acp-resumable"
        }
        fn binary_names(&self) -> &'static [&'static str] {
            &[]
        }
        fn build_args(&self, _cfg: &SpawnConfig, _resume: Option<&str>) -> Vec<String> {
            fake_server_args()
        }
        fn supports_input(&self) -> bool {
            true
        }
        fn supports_resume(&self) -> bool {
            true
        }
        fn acp_session_mode(&self, cfg: &SpawnConfig) -> Option<String> {
            (cfg.permission_mode.as_deref() == Some("bypassPermissions"))
                .then(|| "auto".to_string())
        }
        fn interactive_protocol(&self) -> InteractiveProtocol {
            InteractiveProtocol::Acp
        }
    }

    fn fake_acp_cfg(protocol: &str, dir: &Path, permission_mode: Option<&str>) -> SpawnConfig {
        let mut cfg = spawn_cfg(true);
        cfg.prompt = String::new();
        cfg.working_directory = dir.to_path_buf();
        cfg.permission_mode = permission_mode.map(str::to_string);
        cfg.env_vars
            .insert("AGENT_CTRL_FAKE_PROTOCOL".to_string(), protocol.to_string());
        cfg.env_vars.insert(
            "AGENT_CTRL_FAKE_EXPECT_CWD".to_string(),
            dir.to_string_lossy().into_owned(),
        );
        cfg
    }

    /// Output contents up to and including the first one containing `needle`.
    async fn outputs_until(running: &mut RunningAgent, needle: &str) -> Vec<String> {
        let mut seen = Vec::new();
        for _ in 0..16 {
            let output = tokio::time::timeout(Duration::from_secs(5), running.output_rx.recv())
                .await
                .expect("no output in time")
                .expect("output closed early");
            let done = output.content.contains(needle);
            seen.push(output.content);
            if done {
                return seen;
            }
        }
        panic!("never saw {needle}: {seen:?}");
    }

    async fn resumed_turn(protocol: &str, session_id: &str, mode: Option<&str>) -> Vec<String> {
        let backend =
            CliBackend::<FakeResumableAcpAdapter>::with_binary(std::env::current_exe().unwrap());
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = fake_acp_cfg(protocol, dir.path(), mode);
        cfg.env_vars.insert(
            "AGENT_CTRL_FAKE_EXPECT_SESSION".to_string(),
            session_id.to_string(),
        );
        let mut running = backend.resume(session_id, cfg).await.unwrap();
        let status = wait_for_state(&backend, &running.agent_id, AgentState::WaitingForInput).await;
        assert_eq!(status.session_id.as_deref(), Some(session_id));
        backend.send_input(&running.agent_id, "next").await.unwrap();
        let seen = outputs_until(&mut running, "-1").await;
        backend.close_input(&running.agent_id).await.unwrap();
        seen
    }

    #[tokio::test]
    async fn acp_resume_sends_session_resume_then_the_mode_and_adopts_the_id() {
        let seen = resumed_turn("acp-resume-kimi", "session_moved", Some("bypassPermissions")).await;
        assert!(seen.iter().any(|content| content == "resumed-1"), "{seen:?}");
    }

    #[tokio::test]
    async fn acp_resume_without_a_mode_opens_the_session_on_the_resume_response() {
        let seen = resumed_turn(
            "acp-resume-dsh",
            "2f0c7a52-0d55-4c55-9e43-7c3a7f2b1d10",
            Some("danger-full-access"),
        )
        .await;
        assert!(seen.iter().any(|content| content == "resumed-1"), "{seen:?}");
    }

    #[tokio::test]
    async fn acp_load_discards_the_replayed_history() {
        let backend =
            CliBackend::<FakeResumableAcpAdapter>::with_binary(std::env::current_exe().unwrap());
        let dir = tempfile::tempdir().unwrap();
        let cfg = fake_acp_cfg("acp-load", dir.path(), None);
        let mut running = backend.resume("session_moved", cfg).await.unwrap();
        let status = wait_for_state(&backend, &running.agent_id, AgentState::WaitingForInput).await;
        assert_eq!(status.session_id.as_deref(), Some("session_moved"));
        backend.send_input(&running.agent_id, "next").await.unwrap();
        let seen = outputs_until(&mut running, "loaded-1").await;
        assert!(
            !seen.iter().any(|c| c.contains("OLD-ANSWER") || c.contains("old tool")),
            "replayed history leaked: {seen:?}"
        );
        backend.close_input(&running.agent_id).await.unwrap();
    }

    #[tokio::test]
    async fn acp_resume_fails_when_the_agent_cannot_resume() {
        let backend =
            CliBackend::<FakeResumableAcpAdapter>::with_binary(std::env::current_exe().unwrap());
        let dir = tempfile::tempdir().unwrap();
        let cfg = fake_acp_cfg("acp-no-resume", dir.path(), None);
        let running = backend.resume("session_moved", cfg).await.unwrap();
        let status = wait_terminal_generic(&backend, &running.agent_id).await;
        assert_eq!(status.state, AgentState::Errored, "{status:?}");
        assert!(
            status
                .error_message
                .as_deref()
                .is_some_and(|e| e.contains("cannot resume session session_moved")),
            "{status:?}"
        );
        assert_eq!(status.session_id, None);
        let _ = backend.stop(&running.agent_id, true).await;
    }

    #[tokio::test]
    async fn acp_rejected_session_mode_fails_the_run_before_the_session_opens() {
        let backend =
            CliBackend::<FakeResumableAcpAdapter>::with_binary(std::env::current_exe().unwrap());
        let dir = tempfile::tempdir().unwrap();
        let cfg = fake_acp_cfg("acp-mode-rejected", dir.path(), Some("bypassPermissions"));
        let running = backend.spawn(cfg).await.unwrap();
        let status = wait_terminal_generic(&backend, &running.agent_id).await;
        assert_eq!(status.state, AgentState::Errored, "{status:?}");
        assert_eq!(status.session_id, None);
        let _ = backend.stop(&running.agent_id, true).await;
    }

    #[test]
    fn acp_session_request_picks_new_resume_or_load_from_the_capabilities() {
        let setup = |resume: Option<&str>| AcpSessionSetup {
            cwd: PathBuf::from("/w"),
            resume: resume.map(str::to_string),
            mode: None,
        };
        let init = |caps: serde_json::Value| serde_json::json!({ "agentCapabilities": caps });
        let new = acp_session_request(&init(kimi_agent_capabilities()), &setup(None)).unwrap();
        assert_eq!(new["method"], "session/new");
        assert!(new["params"].get("sessionId").is_none());
        let resume =
            acp_session_request(&init(kimi_agent_capabilities()), &setup(Some("s1"))).unwrap();
        assert_eq!(resume["method"], "session/resume");
        assert_eq!(resume["params"]["sessionId"], "s1");
        assert_eq!(resume["params"]["cwd"], "/w");
        let load = acp_session_request(
            &init(serde_json::json!({ "loadSession": true, "sessionCapabilities": { "resume": null } })),
            &setup(Some("s1")),
        )
        .unwrap();
        assert_eq!(load["method"], "session/load");
        let none = acp_session_request(&init(serde_json::json!({})), &setup(Some("s1")));
        assert_eq!(none.unwrap_err(), "the ACP agent cannot resume session s1");
    }
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test --locked --lib acp_`
Expected: compile errors (`acp_session_mode` is not a member of trait `CliAdapter`, `AcpSessionSetup`/`acp_session_request` not found).

- [ ] **Step 4: Add the trait method (after `interactive_protocol` in `trait CliAdapter`)**

```rust
    /// ACP `session/set_mode` mode id applied right after the session opens, for an agent
    /// whose permission policy is a session mode rather than a CLI flag.
    fn acp_session_mode(&self, _cfg: &SpawnConfig) -> Option<String> {
        None
    }
```

- [ ] **Step 5: Pass the setup from `launch`**

Replace the `InteractiveProtocol::Acp => spawn_acp_driver(…)` arm with:

```rust
                InteractiveProtocol::Acp => spawn_acp_driver(
                    stdin,
                    stdout,
                    stdin_rx,
                    AcpSessionSetup {
                        cwd: config.working_directory.clone(),
                        resume: resume.clone(),
                        mode: self.adapter.acp_session_mode(&config),
                    },
                    out_tx.clone(),
                    control.clone(),
                ),
```

- [ ] **Step 6: Replace `spawn_acp_driver` (whole function) and add its helpers above it**

```rust
/// What the ACP driver opens once the agent is initialized.
struct AcpSessionSetup {
    cwd: PathBuf,
    /// A session to resume instead of creating one.
    resume: Option<String>,
    /// `session/set_mode` mode applied before the session counts as open.
    mode: Option<String>,
}

/// JSON-RPC id of the driver's `session/set_mode` request; prompt ids are numeric.
const ACP_SET_MODE_ID: &str = "set-mode";

/// The request (id 2) that opens the run's session after `initialize`. A resume prefers
/// `session/resume`, which does not replay history.
fn acp_session_request(
    initialize: &serde_json::Value,
    setup: &AcpSessionSetup,
) -> Result<serde_json::Value, String> {
    let mut params = serde_json::json!({
        "cwd": setup.cwd.to_string_lossy(),
        "mcpServers": []
    });
    let method = match setup.resume.as_deref() {
        None => "session/new",
        Some(session_id) => {
            let capabilities = &initialize["agentCapabilities"];
            let method = if capabilities["sessionCapabilities"]["resume"].is_object() {
                "session/resume"
            } else if capabilities["loadSession"].as_bool() == Some(true) {
                "session/load"
            } else {
                return Err(format!("the ACP agent cannot resume session {session_id}"));
            };
            params["sessionId"] = serde_json::json!(session_id);
            method
        }
    };
    Ok(serde_json::json!({ "jsonrpc": "2.0", "id": 2, "method": method, "params": params }))
}

/// Records `sid` as the run's session and starts the first queued prompt. Returns whether a
/// prompt is now in flight.
async fn open_acp_session(
    stdin: &mut tokio::process::ChildStdin,
    control: &ProcessControl,
    sid: &str,
    queued: &mut std::collections::VecDeque<String>,
    next_id: &mut u64,
) -> std::io::Result<bool> {
    control.set_session(sid.to_string()).await;
    let Some(prompt) = queued.pop_front() else {
        control.set_waiting().await;
        return Ok(false);
    };
    send_acp_prompt(stdin, *next_id, sid, &prompt).await?;
    *next_id += 1;
    Ok(true)
}

fn spawn_acp_driver(
    mut stdin: tokio::process::ChildStdin,
    stdout: tokio::process::ChildStdout,
    mut input_rx: mpsc::Receiver<String>,
    setup: AcpSessionSetup,
    tx: mpsc::Sender<AgentOutput>,
    control: Arc<ProcessControl>,
) {
    tokio::spawn(async move {
        let initialize = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": 1,
                "clientCapabilities": {
                    "fs": { "readTextFile": false, "writeTextFile": false },
                    "terminal": false
                },
                "clientInfo": { "name": "agent-ctrl", "version": env!("CARGO_PKG_VERSION") }
            }
        });
        if write_line(&mut stdin, &initialize.to_string())
            .await
            .is_err()
        {
            control
                .fail("failed to initialize ACP subprocess".to_string())
                .await;
            return;
        }

        let mut stdin = Some(stdin);
        let mut lines = BufReader::new(stdout).lines();
        let mut session_id: Option<String> = None;
        // Opened by the agent, waiting for `session/set_mode` to be accepted.
        let mut pending_session: Option<String> = None;
        // `session/load` replays the history as `session/update`s before its response.
        let mut replaying = false;
        let mut queued = std::collections::VecDeque::<String>::new();
        let mut prompt_in_flight = false;
        let mut next_id = 3_u64;
        let mut input_open = true;

        loop {
            tokio::select! {
                input = input_rx.recv(), if input_open => {
                    let Some(input) = input else {
                        // Drop the write half but retain stdout until the
                        // protocol server exits so final events are not lost.
                        drop(stdin.take());
                        input_open = false;
                        continue;
                    };
                    if let Some(sid) = session_id.as_deref() {
                        if !prompt_in_flight {
                            if send_acp_prompt(
                                stdin.as_mut().expect("ACP input open"),
                                next_id,
                                sid,
                                &input,
                            ).await.is_err() {
                                control.fail("failed to send ACP prompt".to_string()).await;
                                return;
                            }
                            next_id += 1;
                            prompt_in_flight = true;
                        } else {
                            // Public control rejects input while a turn runs;
                            // retaining this queue also makes direct backend use race-safe.
                            queued.push_back(input);
                        }
                    } else {
                        queued.push_back(input);
                    }
                }
                line = lines.next_line() => {
                    let line = match line {
                        Ok(Some(line)) => line,
                        Ok(None) => return,
                        Err(error) => {
                            control.fail(format!("failed reading ACP output: {error}")).await;
                            return;
                        }
                    };
                    let Ok(message) = serde_json::from_str::<serde_json::Value>(&line) else {
                        emit_output(&tx, "ACP subprocess emitted invalid JSON".to_string(), OutputType::Stderr).await;
                        continue;
                    };

                    if let Some(method) = message.get("method").and_then(serde_json::Value::as_str) {
                        if message.get("id").is_some() {
                            if respond_to_acp_request(
                                stdin.as_mut().expect("ACP input open"),
                                &message,
                                method,
                                &tx,
                            ).await.is_err() {
                                control.fail("failed to answer ACP request".to_string()).await;
                                return;
                            }
                        } else if method == "session/update" && !replaying {
                            emit_acp_update(&message, &tx).await;
                        }
                        continue;
                    }

                    let set_mode_reply = message.get("id").and_then(serde_json::Value::as_str)
                        == Some(ACP_SET_MODE_ID);
                    let response_id = message.get("id").and_then(serde_json::Value::as_u64);
                    if let Some(error) = message.get("error") {
                        let description = error
                            .get("message")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or("unknown protocol error");
                        emit_output(
                            &tx,
                            format!("ACP request failed: {description}"),
                            OutputType::Stderr,
                        ).await;
                        if set_mode_reply || matches!(response_id, Some(1 | 2)) {
                            control.fail("ACP session initialization failed".to_string()).await;
                            return;
                        }
                        if response_id.is_some_and(|id| id >= 3) {
                            prompt_in_flight = false;
                            control.set_waiting().await;
                        }
                        continue;
                    }

                    if set_mode_reply {
                        let Some(sid) = pending_session.take() else {
                            continue;
                        };
                        match open_acp_session(
                            stdin.as_mut().expect("ACP input open"),
                            &control,
                            &sid,
                            &mut queued,
                            &mut next_id,
                        ).await {
                            Ok(in_flight) => prompt_in_flight = in_flight,
                            Err(_) => {
                                control.fail("failed to send initial ACP prompt".to_string()).await;
                                return;
                            }
                        }
                        session_id = Some(sid);
                        continue;
                    }

                    match response_id {
                        Some(1) => {
                            let request = match acp_session_request(&message["result"], &setup) {
                                Ok(request) => request,
                                Err(reason) => {
                                    emit_output(&tx, reason.clone(), OutputType::Stderr).await;
                                    control.fail(reason).await;
                                    return;
                                }
                            };
                            replaying = request["method"] == "session/load";
                            if write_line(
                                stdin.as_mut().expect("ACP input open"),
                                &request.to_string(),
                            ).await.is_err() {
                                control.fail("failed to create ACP session".to_string()).await;
                                return;
                            }
                        }
                        Some(2) => {
                            replaying = false;
                            let reported = message
                                .get("result")
                                .and_then(|result| result.get("sessionId"))
                                .and_then(serde_json::Value::as_str)
                                .map(str::to_string);
                            // `session/resume` and `session/load` answer without a sessionId.
                            let Some(sid) = setup.resume.clone().or(reported) else {
                                control.fail("ACP session response omitted sessionId".to_string()).await;
                                return;
                            };
                            if let Some(mode) = setup.mode.as_deref() {
                                let request = serde_json::json!({
                                    "jsonrpc": "2.0",
                                    "id": ACP_SET_MODE_ID,
                                    "method": "session/set_mode",
                                    "params": { "sessionId": sid, "modeId": mode }
                                });
                                if write_line(
                                    stdin.as_mut().expect("ACP input open"),
                                    &request.to_string(),
                                ).await.is_err() {
                                    control.fail("failed to set the ACP session mode".to_string()).await;
                                    return;
                                }
                                pending_session = Some(sid);
                                continue;
                            }
                            match open_acp_session(
                                stdin.as_mut().expect("ACP input open"),
                                &control,
                                &sid,
                                &mut queued,
                                &mut next_id,
                            ).await {
                                Ok(in_flight) => prompt_in_flight = in_flight,
                                Err(_) => {
                                    control.fail("failed to send initial ACP prompt".to_string()).await;
                                    return;
                                }
                            }
                            session_id = Some(sid);
                        }
                        Some(id) if id >= 3 => {
                            prompt_in_flight = false;
                            control.set_waiting().await;
                            if let (Some(prompt), Some(sid)) =
                                (queued.pop_front(), session_id.as_deref())
                            {
                                if send_acp_prompt(
                                    stdin.as_mut().expect("ACP input open"),
                                    next_id,
                                    sid,
                                    &prompt,
                                ).await.is_err() {
                                    control.fail("failed to send queued ACP prompt".to_string()).await;
                                    return;
                                }
                                next_id += 1;
                                prompt_in_flight = true;
                                control.begin_turn().await;
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    });
}
```

- [ ] **Step 7: Run the new tests and the existing ACP lifecycle test**

Run: `cargo test --locked --lib acp_`
Expected: PASS — the 6 new tests plus `acp_two_turn_permission_deny_and_close_lifecycle` and `acp_projection_emits_answer_and_safe_tool_summary_but_not_thoughts`.

- [ ] **Step 8: Whole suite, fmt, clippy**

Run: `cargo fmt && cargo test --locked && cargo clippy --locked --all-targets -- -D warnings`
Expected: all pass, no warnings.

- [ ] **Step 9: Commit**

```bash
git add src/agent/process.rs
git commit -m "feat(acp): resume ACP sessions by id and apply a session mode before the first turn

session/resume when the agent advertises it, else session/load with the
replayed history kept off the run's output. An agent advertising neither
fails the run instead of opening a new session.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: Kimi adapter and `Backend::Kimi` (plus `parse_backend` for `openclaw`)

**Files:**
- Create: `src/agent/kimi.rs`
- Modify: `src/agent/mod.rs`, `src/config/types.rs:290-325`, `src/config/validation.rs:202`, `src/remote/mod.rs:246-255` (+ tests module), `src/remote/node.rs:1249-1259`

**Interfaces:**
- Consumes: `run_home`, `RUN_DIR_ENV` (Task 1); `CliAdapter::acp_session_mode` and the ACP resume (Task 2).
- Produces: `crate::agent::kimi::{KimiAdapter, KimiBackend}`; `crate::agent::KimiBackend`; `Backend::Kimi` (`"kimi"`), registered as `"kimi"` by `discover_backends`.

- [ ] **Step 1: Create `src/agent/kimi.rs` containing only the tests module below, and register the module**

The implementation follows in Step 3, above this module.

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::process::RUN_DIR_ENV;
    use std::path::PathBuf;

    fn config(interactive: bool, permission_mode: Option<&str>) -> SpawnConfig {
        SpawnConfig {
            prompt: "inspect".to_string(),
            working_directory: PathBuf::from("/workspace/.runs/job-1"),
            system_prompt: None,
            model: Some("ignored".to_string()),
            max_turns: None,
            allowed_tools: vec![],
            denied_tools: vec![],
            env_vars: HashMap::new(),
            session_id: None,
            interactive,
            permission_mode: permission_mode.map(str::to_string),
            shared_checkout: false,
            agent_id: None,
        }
    }

    fn env_of(cmd: &tokio::process::Command, key: &str) -> Option<String> {
        cmd.as_std()
            .get_envs()
            .find(|(name, _)| *name == key)
            .and_then(|(_, value)| value)
            .map(|value| value.to_string_lossy().into_owned())
    }

    #[test]
    fn interactive_runs_speak_acp_and_carry_no_prompt() {
        let cfg = config(true, Some("bypassPermissions"));
        assert_eq!(KimiAdapter.build_args(&cfg, None), ["acp"]);
        assert_eq!(KimiAdapter.build_args(&cfg, Some("session_x")), ["acp"]);
        assert_eq!(KimiAdapter.interactive_protocol(), InteractiveProtocol::Acp);
        assert!(KimiAdapter.supports_input());
        assert!(KimiAdapter.supports_resume());
    }

    #[test]
    fn one_shot_runs_print_and_never_ask_only_when_the_profile_bypasses() {
        assert_eq!(
            KimiAdapter.build_args(&config(false, Some("bypassPermissions")), None),
            ["--auto", "-p", "inspect"]
        );
        assert_eq!(
            KimiAdapter.build_args(&config(false, None), Some("session_x")),
            ["--session", "session_x", "-p", "inspect"]
        );
    }

    #[test]
    fn never_ask_profiles_select_the_auto_session_mode() {
        for mode in ["bypassPermissions", "auto"] {
            assert_eq!(
                KimiAdapter.acp_session_mode(&config(true, Some(mode))).as_deref(),
                Some("auto")
            );
        }
        assert_eq!(KimiAdapter.acp_session_mode(&config(true, None)), None);
        assert_eq!(
            KimiAdapter.acp_session_mode(&config(true, Some("workspace-write"))),
            None
        );
    }

    #[test]
    fn sessions_live_in_the_run_dir_and_the_gateway_key_is_copied() {
        let run_dir = tempfile::tempdir().unwrap();
        let mut cfg = config(true, None);
        cfg.env_vars.insert(
            RUN_DIR_ENV.to_string(),
            run_dir.path().display().to_string(),
        );
        cfg.env_vars
            .insert("OPENAI_API_KEY".to_string(), "sk-gateway".to_string());
        let mut cmd = tokio::process::Command::new("kimi");
        KimiAdapter.configure_command(&mut cmd, &cfg).unwrap();
        let home = run_dir.path().join("kimi-home");
        assert!(home.is_dir());
        assert_eq!(env_of(&cmd, "KIMI_CODE_HOME"), Some(home.display().to_string()));
        assert_eq!(env_of(&cmd, "KIMI_MODEL_API_KEY").as_deref(), Some("sk-gateway"));
    }

    #[test]
    fn an_explicit_kimi_key_is_kept() {
        let mut cfg = config(true, None);
        cfg.env_vars
            .insert("OPENAI_API_KEY".to_string(), "sk-gateway".to_string());
        cfg.env_vars
            .insert("KIMI_MODEL_API_KEY".to_string(), "sk-kimi".to_string());
        let mut cmd = tokio::process::Command::new("kimi");
        KimiAdapter.configure_command(&mut cmd, &cfg).unwrap();
        assert_eq!(env_of(&cmd, "KIMI_MODEL_API_KEY"), None);
        assert_eq!(env_of(&cmd, "KIMI_CODE_HOME"), None);
    }
}
```

(`an_explicit_kimi_key_is_kept` checks that the adapter sets nothing: `launch` already applied `cfg.env_vars`, so the configured `sk-kimi` reaches the child.)

Add to `src/agent/mod.rs`: `pub mod kimi;` (alphabetical, after `pub mod gemini;`) and `pub use kimi::KimiBackend;` (after `pub use gemini::GeminiBackend;`).

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --locked --lib agent::kimi`
Expected: compile errors (`KimiAdapter` not found).

- [ ] **Step 3: Implement `src/agent/kimi.rs` (above the tests module)**

```rust
//! Kimi Code backend: `kimi acp` for interactive runs, `kimi -p` for one-shot runs.
//!
//! Kimi takes its model from the `KIMI_MODEL_*` env the profile sets, and keeps its sessions
//! under `KIMI_CODE_HOME`, which each isolated run gets inside its run dir so runs never share a
//! session index.

use crate::agent::backend::{BackendError, SpawnConfig};
use crate::agent::process::{run_home, CliAdapter, CliBackend, InteractiveProtocol};
use std::collections::HashMap;

/// Backend driving the `kimi` CLI.
pub type KimiBackend = CliBackend<KimiAdapter>;

#[derive(Default)]
pub struct KimiAdapter;

/// Kimi's "Never Ask" mode (`--auto`, ACP mode `auto`). Its `yolo` mode still asks before risky
/// actions, which the ACP driver would reject.
const NEVER_ASK_MODE: &str = "auto";

fn never_asks(cfg: &SpawnConfig) -> bool {
    matches!(
        cfg.permission_mode.as_deref(),
        Some("bypassPermissions" | "auto")
    )
}

/// Non-empty value of `key` from the run's env map, else the node's environment.
fn env_value(env: &HashMap<String, String>, key: &str) -> Option<String> {
    env.get(key)
        .filter(|value| !value.is_empty())
        .cloned()
        .or_else(|| std::env::var(key).ok().filter(|value| !value.is_empty()))
}

impl CliAdapter for KimiAdapter {
    fn name(&self) -> &'static str {
        "kimi"
    }

    fn binary_names(&self) -> &'static [&'static str] {
        &["kimi"]
    }

    fn build_args(&self, cfg: &SpawnConfig, resume: Option<&str>) -> Vec<String> {
        if cfg.interactive {
            return vec!["acp".to_string()];
        }
        let mut args = Vec::new();
        if never_asks(cfg) {
            args.push("--auto".to_string());
        }
        if let Some(session_id) = resume {
            args.extend(["--session".to_string(), session_id.to_string()]);
        }
        args.extend(["-p".to_string(), cfg.prompt.clone()]);
        args
    }

    fn configure_command(
        &self,
        cmd: &mut tokio::process::Command,
        cfg: &SpawnConfig,
    ) -> Result<(), BackendError> {
        if let Some(home) = run_home(cfg, "kimi-home")? {
            cmd.env("KIMI_CODE_HOME", home);
        }
        // The gateway key reaches the node as OPENAI_API_KEY; Kimi's env provider reads its own
        // variable.
        if env_value(&cfg.env_vars, "KIMI_MODEL_API_KEY").is_none() {
            if let Some(key) = env_value(&cfg.env_vars, "OPENAI_API_KEY") {
                cmd.env("KIMI_MODEL_API_KEY", key);
            }
        }
        Ok(())
    }

    fn supports_resume(&self) -> bool {
        true
    }

    fn supports_input(&self) -> bool {
        true
    }

    fn interactive_protocol(&self) -> InteractiveProtocol {
        InteractiveProtocol::Acp
    }

    fn acp_session_mode(&self, cfg: &SpawnConfig) -> Option<String> {
        never_asks(cfg).then(|| NEVER_ASK_MODE.to_string())
    }
}
```

Note for the `an_explicit_kimi_key_is_kept` test: if the developer machine has `KIMI_CODE_HOME` unset (normal), the second assertion holds because the test sets no run dir.

- [ ] **Step 4: Register the backend**

`src/config/types.rs` — enum and both methods:

```rust
pub enum Backend {
    Claude,
    Codex,
    Gemini,
    OpenCode,
    Pi,
    OpenClaw,
    Kimi,
}
```

`as_str`: add `Backend::Kimi => "kimi",`. `supports_input`: add `| Backend::Kimi` to the `matches!`.

`src/agent/mod.rs` `discover_backends`, after the OpenClaw block:

```rust
    if let Some(b) = KimiBackend::discover() {
        backends.insert("kimi".to_string(), Arc::new(b));
    }
```

and extend its doc comment's list to `` (`"claude"`, `"codex"`, `"gemini"`, `"opencode"`, `"pi"`, `"openclaw"`, `"kimi"`) ``.

`src/remote/mod.rs` `parse_backend`:

```rust
pub(crate) fn parse_backend(s: &str) -> Option<Backend> {
    match s {
        "claude" => Some(Backend::Claude),
        "codex" => Some(Backend::Codex),
        "gemini" => Some(Backend::Gemini),
        "opencode" => Some(Backend::OpenCode),
        "pi" => Some(Backend::Pi),
        "openclaw" => Some(Backend::OpenClaw),
        "kimi" => Some(Backend::Kimi),
        _ => None,
    }
}
```

`src/remote/node.rs` note timing (replace the `match self.backend` and the comment above it):

```rust
        // Claude reports nothing until it receives a turn, so its note goes to the agent before
        // confirmation. Pi confirms through its state probe and an ACP agent resuming by id
        // (Kimi) on its resume response, so nothing is written to them until then. Either way the
        // note's echo is emitted only after `resumed`.
        let note_before_confirmation = match self.backend {
            Backend::Pi | Backend::Kimi => false,
            Backend::Claude
            | Backend::Codex
            | Backend::Gemini
            | Backend::OpenCode
            | Backend::OpenClaw => true,
        };
```

`src/config/validation.rs:202`: `for backend in ["claude", "codex", "gemini", "opencode", "pi", "openclaw", "kimi"] {`

- [ ] **Step 5: Add the `parse_backend` test (`src/remote/mod.rs` tests module, first test)**

```rust
    #[test]
    fn parse_backend_accepts_every_backend_name() {
        for backend in [
            Backend::Claude,
            Backend::Codex,
            Backend::Gemini,
            Backend::OpenCode,
            Backend::Pi,
            Backend::OpenClaw,
            Backend::Kimi,
        ] {
            assert_eq!(parse_backend(backend.as_str()), Some(backend));
        }
        assert_eq!(parse_backend("unknown"), None);
    }
```

- [ ] **Step 6: Run the tests**

Run: `cargo test --locked --lib agent::kimi` then `cargo test --locked --lib parse_backend_accepts_every_backend_name` then `cargo test --locked --lib interactive_operator_policy_requires_launch`
Expected: PASS (5 + 1 + 1).

- [ ] **Step 7: Whole suite, fmt, clippy**

Run: `cargo fmt && cargo test --locked && cargo clippy --locked --all-targets -- -D warnings`
Expected: all pass.

- [ ] **Step 8: Commit**

```bash
git add src/agent/kimi.rs src/agent/mod.rs src/config/types.rs src/config/validation.rs src/remote/mod.rs src/remote/node.rs
git commit -m "feat(agent): Kimi Code backend over ACP; accept openclaw in node spawns

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: DeepSeek Harness adapter and `Backend::Dsh`

**Files:**
- Create: `src/agent/dsh.rs`
- Modify: `src/agent/mod.rs`, `src/config/types.rs`, `src/config/validation.rs:202`, `src/remote/mod.rs` (`parse_backend` + its test), `src/remote/node.rs` (note timing)

**Interfaces:**
- Consumes: `run_home` (Task 1), ACP resume (Task 2).
- Produces: `crate::agent::dsh::{DshAdapter, DshBackend, DSH_PATCH_ENV, DSH_ACP_PATCH_ENV}`; `Backend::Dsh` (`"dsh"`). Env contract used by Task 6: `AGENT_CTRL_DSH_PATCH` (every run) and `AGENT_CTRL_DSH_ACP_PATCH` (ACP runs) name `--patch` files.

- [ ] **Step 1: Write the tests module at the bottom of `src/agent/dsh.rs`, plus `pub mod dsh;` / `pub use dsh::DshBackend;` in `src/agent/mod.rs` (alphabetical: `pub mod dsh;` after `pub mod codex;`, `pub use dsh::DshBackend;` after `pub use codex::CodexBackend;`)**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::process::RUN_DIR_ENV;
    use std::collections::HashMap;
    use std::path::PathBuf;

    fn config(interactive: bool, permission_mode: Option<&str>) -> SpawnConfig {
        SpawnConfig {
            prompt: "inspect".to_string(),
            working_directory: PathBuf::from("/workspace/.runs/job-1"),
            system_prompt: None,
            model: None,
            max_turns: None,
            allowed_tools: vec![],
            denied_tools: vec![],
            env_vars: HashMap::from([
                (DSH_PATCH_ENV.to_string(), "/agent-config/dsh-passiv.patch.yml".to_string()),
                (DSH_ACP_PATCH_ENV.to_string(), "/agent-config/dsh-acp.patch.yml".to_string()),
            ]),
            session_id: None,
            interactive,
            permission_mode: permission_mode.map(str::to_string),
            shared_checkout: false,
            agent_id: None,
        }
    }

    fn env_of(cmd: &tokio::process::Command, key: &str) -> Option<String> {
        cmd.as_std()
            .get_envs()
            .find(|(name, _)| *name == key)
            .and_then(|(_, value)| value)
            .map(|value| value.to_string_lossy().into_owned())
    }

    #[test]
    fn interactive_runs_boot_the_acp_profile_with_both_patches() {
        let cfg = config(true, None);
        assert_eq!(
            DshAdapter.build_args(&cfg, Some("2f0c7a52-0d55-4c55-9e43-7c3a7f2b1d10")),
            [
                "--profile",
                "acp",
                "--patch",
                "/agent-config/dsh-passiv.patch.yml",
                "--patch",
                "/agent-config/dsh-acp.patch.yml",
            ]
        );
        assert_eq!(DshAdapter.interactive_protocol(), InteractiveProtocol::Acp);
        assert!(DshAdapter.supports_input());
        assert!(DshAdapter.supports_resume());
        assert_eq!(DshAdapter.acp_session_mode(&cfg), None);
    }

    #[test]
    fn one_shot_runs_use_the_headless_profile_with_the_task_last() {
        assert_eq!(
            DshAdapter.build_args(&config(false, None), Some("2f0c7a52-0d55-4c55-9e43-7c3a7f2b1d10")),
            [
                "--profile",
                "headless",
                "--patch",
                "/agent-config/dsh-passiv.patch.yml",
                "--session-id",
                "2f0c7a52-0d55-4c55-9e43-7c3a7f2b1d10",
                "inspect",
            ]
        );
    }

    #[test]
    fn missing_patch_env_adds_no_patch_flags() {
        let mut cfg = config(true, None);
        cfg.env_vars.clear();
        assert_eq!(DshAdapter.build_args(&cfg, None), ["--profile", "acp"]);
    }

    #[test]
    fn environment_sets_home_permissions_and_telemetry_off() {
        let run_dir = tempfile::tempdir().unwrap();
        for (mode, expected) in [
            (Some("danger-full-access"), "danger-full-access"),
            (Some("bypassPermissions"), "danger-full-access"),
            (Some("read-only"), "read-only"),
            (Some("workspace-write"), "workspace-write"),
            (None, "workspace-write"),
        ] {
            let mut cfg = config(true, mode);
            cfg.env_vars.insert(
                RUN_DIR_ENV.to_string(),
                run_dir.path().display().to_string(),
            );
            let mut cmd = tokio::process::Command::new("dsh");
            DshAdapter.configure_command(&mut cmd, &cfg).unwrap();
            let home = run_dir.path().join("dsh-home");
            assert!(home.is_dir());
            assert_eq!(env_of(&cmd, "DSH_HOME"), Some(home.display().to_string()));
            assert_eq!(env_of(&cmd, "DSH_PERMISSION_MODE").as_deref(), Some(expected), "{mode:?}");
            assert_eq!(env_of(&cmd, "DSH_TELEMETRY_DISABLED").as_deref(), Some("1"));
        }
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --locked --lib agent::dsh`
Expected: compile errors (`DshAdapter` not found).

- [ ] **Step 3: Implement `src/agent/dsh.rs` (above the tests module)**

```rust
//! DeepSeek Harness backend: `dsh --profile acp` for interactive runs, `dsh --profile headless`
//! for one-shot runs.
//!
//! The model route and the telemetry plugin opt-outs come from `--patch` overlays the profile
//! names, because each run has its own `DSH_HOME` and dsh reads a user patch layer only from
//! inside it.

use crate::agent::backend::{BackendError, SpawnConfig};
use crate::agent::process::{run_home, CliAdapter, CliBackend, InteractiveProtocol};

/// Backend driving the `dsh` CLI.
pub type DshBackend = CliBackend<DshAdapter>;

/// Env var naming the patch overlay every dsh run gets.
pub const DSH_PATCH_ENV: &str = "AGENT_CTRL_DSH_PATCH";
/// Env var naming the patch overlay only ACP runs get (it targets the `acp` row).
pub const DSH_ACP_PATCH_ENV: &str = "AGENT_CTRL_DSH_ACP_PATCH";

#[derive(Default)]
pub struct DshAdapter;

/// The dsh sandbox preset (`DSH_PERMISSION_MODE`) for a profile's permission mode. Only
/// `danger-full-access` approves without asking; anything else asks, and the ACP driver rejects.
fn sandbox_mode(cfg: &SpawnConfig) -> &'static str {
    match cfg.permission_mode.as_deref() {
        Some("danger-full-access" | "bypassPermissions") => "danger-full-access",
        Some("read-only") => "read-only",
        _ => "workspace-write",
    }
}

impl CliAdapter for DshAdapter {
    fn name(&self) -> &'static str {
        "dsh"
    }

    fn binary_names(&self) -> &'static [&'static str] {
        &["dsh"]
    }

    fn build_args(&self, cfg: &SpawnConfig, resume: Option<&str>) -> Vec<String> {
        let profile = if cfg.interactive { "acp" } else { "headless" };
        let mut args = vec!["--profile".to_string(), profile.to_string()];
        let patches: &[&str] = if cfg.interactive {
            &[DSH_PATCH_ENV, DSH_ACP_PATCH_ENV]
        } else {
            &[DSH_PATCH_ENV]
        };
        for key in patches {
            if let Some(path) = cfg.env_vars.get(*key).filter(|path| !path.is_empty()) {
                args.extend(["--patch".to_string(), path.clone()]);
            }
        }
        if !cfg.interactive {
            if let Some(session_id) = resume {
                args.extend(["--session-id".to_string(), session_id.to_string()]);
            }
            args.push(cfg.prompt.clone());
        }
        args
    }

    fn configure_command(
        &self,
        cmd: &mut tokio::process::Command,
        cfg: &SpawnConfig,
    ) -> Result<(), BackendError> {
        if let Some(home) = run_home(cfg, "dsh-home")? {
            cmd.env("DSH_HOME", home);
        }
        cmd.env("DSH_PERMISSION_MODE", sandbox_mode(cfg));
        cmd.env("DSH_TELEMETRY_DISABLED", "1");
        Ok(())
    }

    fn supports_resume(&self) -> bool {
        true
    }

    fn supports_input(&self) -> bool {
        true
    }

    fn interactive_protocol(&self) -> InteractiveProtocol {
        InteractiveProtocol::Acp
    }
}
```

- [ ] **Step 4: Register the backend**

`src/config/types.rs`: add `Dsh,` after `Kimi,`; `as_str`: `Backend::Dsh => "dsh",`; `supports_input`: `| Backend::Dsh`.

`src/agent/mod.rs` `discover_backends`, after the Kimi block:

```rust
    if let Some(b) = DshBackend::discover() {
        backends.insert("dsh".to_string(), Arc::new(b));
    }
```

and add `"dsh"` to the doc comment list.

`src/remote/mod.rs` `parse_backend`: add `"dsh" => Some(Backend::Dsh),` after the kimi arm; in `parse_backend_accepts_every_backend_name` add `Backend::Dsh,` to the array.

`src/remote/node.rs`: `Backend::Pi | Backend::Kimi | Backend::Dsh => false,` and in the comment `(Kimi, dsh)` instead of `(Kimi)`.

`src/config/validation.rs:202`: add `"dsh"` to the list.

- [ ] **Step 5: Run the tests**

Run: `cargo test --locked --lib agent::dsh` then `cargo test --locked --lib parse_backend_accepts_every_backend_name`
Expected: PASS (4 + 1).

- [ ] **Step 6: Whole suite, fmt, clippy**

Run: `cargo fmt && cargo test --locked && cargo clippy --locked --all-targets -- -D warnings`
Expected: all pass.

- [ ] **Step 7: Commit**

```bash
git add src/agent/dsh.rs src/agent/mod.rs src/config/types.rs src/config/validation.rs src/remote/mod.rs src/remote/node.rs
git commit -m "feat(agent): DeepSeek Harness backend over ACP

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: Node images install `kimi` and `dsh`

**Files:**
- Modify: `Dockerfile` (ARG block :14-25, `npm install -g` :55-61)
- Modify: `deploy/staging/obachan/Dockerfile` (ARG block :14-20, `npm install -g` :27-30)

**Interfaces:**
- Produces: `kimi` 2.1.1 and `dsh` 0.2.1-alpha.1 on `PATH` in the node image (base `node:24-bookworm-slim` already satisfies `>=22.19.0`, so the base stays).

- [ ] **Step 1: Confirm the pinned packages exist and their engines**

Run:
```bash
npm view @moonshot-ai/kimi-code@2.1.1 version bin engines
npm view @deepseek-ai/dsh@0.2.1-alpha.1 version bin engines
```
Expected: versions `2.1.1` / `0.2.1-alpha.1`; bins `kimi` / `dsh`; `node >=22.19.0` (or lower).

- [ ] **Step 2: Edit `Dockerfile`**

After `ARG OPENCLAW_VERSION=2026.9.4` add:

```dockerfile
# Kimi Code and DeepSeek Harness drive their ACP servers (`kimi acp`, `dsh --profile acp`).
# dsh is an alpha; keep both pinned.
ARG KIMI_CODE_VERSION=2.1.1
ARG DSH_VERSION=0.2.1-alpha.1
```

Replace the `npm install -g` RUN with:

```dockerfile
RUN npm install -g \
      "@anthropic-ai/claude-code@${CLAUDE_CODE_VERSION}" \
      "@openai/codex@${CODEX_VERSION}" \
      "@google/gemini-cli@${GEMINI_VERSION}" \
      "opencode-ai@${OPENCODE_VERSION}" \
      "@mariozechner/pi-coding-agent@${PI_VERSION}" \
      "openclaw@${OPENCLAW_VERSION}" \
      "@moonshot-ai/kimi-code@${KIMI_CODE_VERSION}" \
      "@deepseek-ai/dsh@${DSH_VERSION}" \
    && kimi --version \
    && dsh --version
```

- [ ] **Step 3: Edit `deploy/staging/obachan/Dockerfile`**

After `ARG OPENCODE_VERSION=1.17.13` add the same two ARGs and comment. Replace its `npm install -g` RUN with:

```dockerfile
RUN npm install -g \
      "@openai/codex@${CODEX_VERSION}" \
      "opencode-ai@${OPENCODE_VERSION}" \
      "@moonshot-ai/kimi-code@${KIMI_CODE_VERSION}" \
      "@deepseek-ai/dsh@${DSH_VERSION}" \
    && kimi --version \
    && dsh --version \
    && if [ -n "${OPENCLAW_NPM_SPEC}" ]; then npm install -g "${OPENCLAW_NPM_SPEC}"; fi
```

- [ ] **Step 4: Build the main image locally and check both CLIs**

Run (Docker Desktop must be running; the build includes a release `cargo build`, ~15 min):
```bash
docker build --platform linux/amd64 -t agent-ctrl:kimi-dsh-check .
docker run --rm --entrypoint sh agent-ctrl:kimi-dsh-check -c 'kimi --version && dsh --version && command -v kimi dsh'
```
Expected: `2.1.1`, `0.2.1-alpha.1` (dsh may print more text after the version), and the two paths under `/usr/local/bin`.

If Docker is unavailable on the machine, say so in the task report. The PR's push to `dev` is then the first image build, and Task 8 Step 4 is where a broken build shows.

- [ ] **Step 5: Commit**

```bash
git add Dockerfile deploy/staging/obachan/Dockerfile
git commit -m "build: install Kimi Code 2.1.1 and DeepSeek Harness 0.2.1-alpha.1 in the node images

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 6: Staging profiles, workers, Kimi model env, dsh patches

**Files:**
- Modify: `deploy/k8s/render.py` (`_DEFAULTS` :62-150, new `_kimi_env` / `_dsh_env` after `_openai_gateway_env` :239-250, `_subs` :403-482)
- Modify: `deploy/k8s/templates/configmap.tmpl.yaml`
- Modify: `deploy/k8s/instances/staging.toml`
- Test: `deploy/k8s/test_shared_workspaces.py`

**Interfaces:**
- Consumes: `AGENT_CTRL_DSH_PATCH`, `AGENT_CTRL_DSH_ACP_PATCH` (Task 4); Kimi copies `OPENAI_API_KEY` into `KIMI_MODEL_API_KEY` (Task 3). The ConfigMap is mounted at `/agent-config` in the node container (`node-deployment.tmpl.yaml`, volume `agent-config`).
- Produces: profiles `kimi-worker`, `dsh-worker`, `staging-kimi-interactive-rw`, `staging-dsh-interactive-rw`; `agent_by_backend` rows `kimi`, `dsh`; ConfigMap files `dsh-passiv.patch.yml`, `dsh-acp.patch.yml`; instance keys `kimi_model`, `dsh_model`.

- [ ] **Step 1: Write the failing render tests (`deploy/k8s/test_shared_workspaces.py`)**

In `test_interactive_rw_agents_and_warpspeed_repo_are_staging_only`, extend the tuple list and the mode line:

```python
        for name, backend, label in (
            ("staging-claude-interactive-rw", "claude", "Claude"),
            ("staging-pi-interactive-rw", "pi", "Pi"),
            ("staging-codex-interactive-rw", "codex", "Codex"),
            ("staging-kimi-interactive-rw", "kimi", "Kimi Code"),
            ("staging-dsh-interactive-rw", "dsh", "DeepSeek Harness"),
        ):
```

and replace

```python
            mode = "bypassPermissions" if backend == "claude" else "workspace-write"
```

with

```python
            mode = {"claude": "bypassPermissions", "kimi": "bypassPermissions",
                    "dsh": "danger-full-access"}.get(backend, "workspace-write")
```

In `test_staging_fleet_repositories_and_rw_profiles`, extend the loop tuple:

```python
        for backend, mode in (("claude", "bypassPermissions"), ("pi", "workspace-write"),
                              ("codex", "workspace-write"), ("kimi", "bypassPermissions"),
                              ("dsh", "danger-full-access")):
```

In `test_read_only_inspectors_keep_the_shared_checkout`, the last loop becomes `for backend in ("claude", "pi", "codex", "kimi", "dsh"):`.

Add two tests to the same class (after `test_read_only_inspectors_keep_the_shared_checkout`):

```python
    def test_kimi_and_dsh_route_through_passiv_with_telemetry_off(self):
        cfg = render.load_instance("staging", None)
        cm = dict(render.render(cfg))["configmap"]
        profiles = agent_profiles(cm)
        base = cfg["passiv_base_url"]
        for name in ("kimi-worker", "staging-kimi-interactive-rw"):
            body = profiles[name]
            self.assertIn("    backend: kimi\n", body)
            self.assertIn("    permission_mode: bypassPermissions\n", body)
            self.assertIn('      KIMI_MODEL_NAME: "kimi-k2"\n', body)
            self.assertIn('      KIMI_MODEL_PROVIDER_TYPE: "openai"\n', body)
            self.assertIn(f'      KIMI_MODEL_BASE_URL: "{base}"\n', body)
            self.assertIn('      KIMI_MODEL_MAX_CONTEXT_SIZE: "200000"\n', body)
            self.assertIn('      KIMI_DISABLE_TELEMETRY: "1"\n', body)
            # The adapter copies OPENAI_API_KEY; no key ever appears in the ConfigMap.
            self.assertNotIn("KIMI_MODEL_API_KEY", body)
        for name in ("dsh-worker", "staging-dsh-interactive-rw"):
            body = profiles[name]
            self.assertIn("    backend: dsh\n", body)
            self.assertIn("    permission_mode: danger-full-access\n", body)
            self.assertIn('      DSH_TELEMETRY_DISABLED: "1"\n', body)
            self.assertIn('      AGENT_CTRL_DSH_PATCH: "/agent-config/dsh-passiv.patch.yml"\n', body)
            self.assertIn('      AGENT_CTRL_DSH_ACP_PATCH: "/agent-config/dsh-acp.patch.yml"\n', body)
        passiv = cm.split("  dsh-passiv.patch.yml: |\n", 1)[1].split("  dsh-acp.patch.yml: |\n", 1)[0]
        for line in (
            "    - id: llm-pi-ai",
            "          passiv:",
            "            apiKeyEnv: OPENAI_API_KEY",
            "            api: openai-completions",
            f'            baseURL: "{base}"',
            '              - id: "deepseek-v4"',
            "    - id: agent-default-model",
            "        provider: passiv",
            '        model: "deepseek-v4"',
        ):
            self.assertIn(line + "\n", passiv)
        for plugin in ("session-log-deepseek", "plugin-package-inventory-deepseek"):
            self.assertIn(f"    - id: {plugin}\n      disabled: true\n", passiv)
        acp = cm.split("  dsh-acp.patch.yml: |\n", 1)[1]
        self.assertIn('    - id: acp\n      config:\n        provider: passiv\n        model: "deepseek-v4"\n', acp)
        self.assertIn("        kimi: kimi-worker\n", cm)
        self.assertIn("        dsh: dsh-worker\n", cm)
        for name in ("kimi-worker", "dsh-worker", "staging-kimi-interactive-rw", "staging-dsh-interactive-rw"):
            self.assertIn(f"      - path: {name}.yaml\n", cm)

    def test_kimi_and_dsh_are_staging_only(self):
        prod = dict(render.render(render.load_instance("prod", None)))["configmap"]
        for name in ("kimi-worker", "dsh-worker", "staging-kimi-interactive-rw", "staging-dsh-interactive-rw"):
            self.assertIn(f"      # - path: {name}.yaml\n", prod)
        self.assertIn("        # kimi: kimi-worker\n", prod)
        self.assertIn("        # dsh: dsh-worker\n", prod)
```

- [ ] **Step 2: Run to verify failure**

Run: `python -m unittest discover -s deploy/k8s -p "test_*.py"`
Expected: FAIL / ERROR in the three extended tests and the two new ones (`KeyError: 'staging-kimi-interactive-rw'`, missing substrings).

- [ ] **Step 3: `deploy/k8s/render.py`**

In `_DEFAULTS`, after `"codex_model": "gpt-5.4",`:

```python
    # passiv aliases for the Kimi Code and DeepSeek Harness workers (OpenRouter-backed).
    "kimi_model": "kimi-k2",
    "dsh_model": "deepseek-v4",
```

After `_openai_gateway_env`:

```python
def _kimi_env(cfg: dict, indent: str = "      ") -> str:
    """Kimi's env-defined model on passiv. The adapter supplies KIMI_MODEL_API_KEY from the
    inherited OPENAI_API_KEY, so no key is rendered here."""
    return _env_lines(
        [
            ("KIMI_MODEL_NAME", cfg["kimi_model"]),
            ("KIMI_MODEL_PROVIDER_TYPE", "openai"),
            ("KIMI_MODEL_BASE_URL", cfg["passiv_base_url"]),
            ("KIMI_MODEL_MAX_CONTEXT_SIZE", "200000"),
            ("KIMI_DISABLE_TELEMETRY", "1"),
        ],
        indent,
    )


def _dsh_env(cfg: dict, indent: str = "      ") -> str:
    """dsh reads its model route from the ConfigMap patch overlays (mounted at /agent-config)."""
    return _env_lines(
        [
            ("AGENT_CTRL_DSH_PATCH", "/agent-config/dsh-passiv.patch.yml"),
            ("AGENT_CTRL_DSH_ACP_PATCH", "/agent-config/dsh-acp.patch.yml"),
            ("DSH_TELEMETRY_DISABLED", "1"),
        ],
        indent,
    )
```

In `_subs`, after `"openai_gateway_env": _openai_gateway_env(cfg),`:

```python
        "kimi_env": _kimi_env(cfg),
        "dsh_env": _dsh_env(cfg),
```

- [ ] **Step 4: `deploy/k8s/templates/configmap.tmpl.yaml`**

`agent_by_backend` — after `opencode: opencode-worker`:

```yaml
        {operator_inspector_path_prefix}kimi: kimi-worker
        {operator_inspector_path_prefix}dsh: dsh-worker
```

`agents:` — after `- path: openclaw-worker.yaml`:

```yaml
      {operator_inspector_path_prefix}- path: kimi-worker.yaml
      {operator_inspector_path_prefix}- path: dsh-worker.yaml
```

and after `{operator_inspector_path_prefix}- path: staging-codex-interactive-rw.yaml`:

```yaml
      {operator_inspector_path_prefix}- path: staging-kimi-interactive-rw.yaml
      {operator_inspector_path_prefix}- path: staging-dsh-interactive-rw.yaml
```

After the `openclaw-worker.yaml` block (before the `codex-config.toml` comment), add:

```yaml
  # Kimi Code and DeepSeek Harness run on passiv through OPENAI_API_KEY. Kimi's model is env
  # (KIMI_MODEL_*); dsh's comes from the dsh-*.patch.yml files below.
  kimi-worker.yaml: |
    name: kimi-worker
    description: Kimi Code ticket worker for Matrix-created work items
    backend: kimi
    node: {node_name}
    working_directory: "{default_working_directory}"
    permission_mode: bypassPermissions
    default_prompt: "Inspect the ticket context and shared checkout; use ojiichan (Rustrak/ELK/OTLP) and shirudo when useful for diagnosis; then implement, test, push, and open a draft GitLab merge request."
    env_vars:
{kimi_env}{worker_hooks_block}    instances:
      max_concurrent: 1

  dsh-worker.yaml: |
    name: dsh-worker
    description: DeepSeek Harness ticket worker for Matrix-created work items
    backend: dsh
    node: {node_name}
    working_directory: "{default_working_directory}"
    permission_mode: danger-full-access
    default_prompt: "Inspect the ticket context and shared checkout; use ojiichan (Rustrak/ELK/OTLP) and shirudo when useful for diagnosis; then implement, test, push, and open a draft GitLab merge request."
    env_vars:
{dsh_env}{worker_hooks_block}    instances:
      max_concurrent: 1

  staging-kimi-interactive-rw.yaml: |
    name: staging-kimi-interactive-rw
    description: Interactive Kimi Code session on an operator repository
    backend: kimi
    node: {node_name}
    working_directory: "{default_working_directory}"
    permission_mode: bypassPermissions
    interactive: true
    env_vars:
{kimi_env}    operator_control:
      visible: true
      launchable: true
      allow_interactive: true
      requires_repo: true
    instances:
      max_concurrent: 1

  staging-dsh-interactive-rw.yaml: |
    name: staging-dsh-interactive-rw
    description: Interactive DeepSeek Harness session on an operator repository
    backend: dsh
    node: {node_name}
    working_directory: "{default_working_directory}"
    permission_mode: danger-full-access
    interactive: true
    env_vars:
{dsh_env}    operator_control:
      visible: true
      launchable: true
      allow_interactive: true
      requires_repo: true
    instances:
      max_concurrent: 1
```

At the end of the file (after `pi-models.json`), add:

```yaml
  # dsh patch overlays (`dsh --patch`), applied after the profile's own layer. A patch row
  # replaces that row's whole config. Non-sensitive: the route names OPENAI_API_KEY.
  dsh-passiv.patch.yml: |
    - id: llm-pi-ai
      config:
        providers:
          passiv:
            displayName: passiv
            apiKeyEnv: OPENAI_API_KEY
            api: openai-completions
            baseURL: "{passiv_base_url}"
            models:
              - id: "{dsh_model}"
    - id: agent-default-model
      config:
        provider: passiv
        model: "{dsh_model}"
    - id: session-log-deepseek
      disabled: true
    - id: plugin-package-inventory-deepseek
      disabled: true
  dsh-acp.patch.yml: |
    - id: acp
      config:
        provider: passiv
        model: "{dsh_model}"
```

- [ ] **Step 5: `deploy/k8s/instances/staging.toml`**

After the `openai_gateway_model = "chat-default"` line:

```toml

# Kimi Code and DeepSeek Harness use passiv aliases backed by OpenRouter (added with
# `passiv zoo add`, see the kimi/dsh plan's operator task). The litellm_api_key above must be
# allowed to use both.
kimi_model = "kimi-k2"
dsh_model = "deepseek-v4"
```

- [ ] **Step 6: Run the render tests**

Run: `python -m unittest discover -s deploy/k8s -p "test_*.py"`
Expected: OK (all tests, including the five touched).

- [ ] **Step 7: Check that both instances render and that the new profile shape validates**

Run:
```bash
python deploy/k8s/render.py --instance staging --image-tag check > /dev/null
python deploy/k8s/render.py --instance prod --image-tag check > /dev/null
```
Expected: both exit 0 (an unescaped brace or a missing key fails the render).

Add this test to the `src/config/validation.rs` tests module. It feeds the rendered rw profile shape to the crate's validator:

```rust
    #[test]
    fn kimi_and_dsh_rw_profiles_validate() {
        for (name, backend, mode) in [
            ("staging-kimi-interactive-rw", "kimi", "bypassPermissions"),
            ("staging-dsh-interactive-rw", "dsh", "danger-full-access"),
        ] {
            let profile = agent(&format!(
                "name: {name}\nbackend: {backend}\npermission_mode: {mode}\ninteractive: true\noperator_control:\n  visible: true\n  launchable: true\n  allow_interactive: true\n  requires_repo: true\n"
            ));
            validate_agent(&profile).unwrap();
        }
    }
```

Run: `cargo test --locked --lib kimi_and_dsh_rw_profiles_validate`
Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add deploy/k8s/render.py deploy/k8s/templates/configmap.tmpl.yaml deploy/k8s/instances/staging.toml deploy/k8s/test_shared_workspaces.py src/config/validation.rs
git commit -m "deploy(staging): Kimi Code and DeepSeek Harness workers and rw profiles on passiv

dsh gets its passiv route and telemetry plugin opt-outs as --patch overlays;
Kimi gets its model through KIMI_MODEL_* env. Prod renders them commented out.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 7 (operator): passiv aliases `kimi-k2` and `deepseek-v4` on staging, and the agent-ctrl key scope

No repository change. Run from `T:/projects/dev-ops/passiv`. This must be done before Task 8's merge; a run against a missing alias fails on its first turn.

**Interfaces:**
- Produces: LiteLLM (staging, `https://gateway-dev.mezzanin.eu`) routes `kimi-k2` and `deepseek-v4`, both allowed for the key in OpenBao `obachan/staging/agent-ctrl` field `litellm_api_key`.

- [ ] **Step 1: Look up the exact OpenRouter ids**

```bash
curl -s https://openrouter.ai/api/v1/models \
  | jq -r '.data[] | select(.id | test("kimi|deepseek"; "i")) | [.id, .context_length, .pricing.prompt, .pricing.completion] | @tsv' \
  | sort
```

Pick the Kimi K2 id (the `moonshotai/kimi-k2…` entry the user wants; prefer the newest dated K2 if several exist) and the DeepSeek V4 id (`deepseek/deepseek-v4…`). If OpenRouter lists no DeepSeek V4, stop and ask the user which DeepSeek model the `deepseek-v4` alias should serve, since the alias name would otherwise be wrong. Record both ids and their `context_length` for the PR description. If Kimi's `context_length` is below 200000, ask the user before rollout whether to lower `KIMI_MODEL_MAX_CONTEXT_SIZE` (the spec fixes 200000).

- [ ] **Step 2: Operator environment**

```bash
cd T:/projects/dev-ops/passiv
OBX="T:/projects/dev-ops/obachan/.venv/Scripts/obachan.exe"
export PASSIV_LITELLM_URL=https://gateway-dev.mezzanin.eu
export LITELLM_MASTER_KEY="$("$OBX" vault get --env staging --field litellm_master_key passiv-litellm)"
```

Export `PASSIV_MONGO_URI` and its mTLS material (`PASSIV_MONGO_TLS_CA`/`PASSIV_MONGO_TLS_KEY` or the `_PEM` variants) for the staging `passiv-model-zoo` user as described in `docs/model-db-routing.md` ("Secret boundary"). `obachan vault get` needs staging cluster access (`~/.kube/obachan-staging`).

Check: `uv run passiv zoo list --env staging` prints the current staging aliases. Keep the output; Step 4 compares against it.

- [ ] **Step 3: Add both aliases (staging only, OpenRouter key by reference)**

```bash
uv run passiv zoo add kimi-k2 --provider openrouter --backend-model <kimi id from Step 1> \
  --key-ref providers/openrouter/api_key --env staging
uv run passiv zoo add deepseek-v4 --provider openrouter --backend-model <deepseek id from Step 1> \
  --key-ref providers/openrouter/api_key --env staging
```

`zoo add` without `--key-ref` stores no key reference and without `--env` serves the alias on prod as well; both flags are required here. Expected output: `added kimi-k2 (chat)` and `added deepseek-v4 (chat)`.

- [ ] **Step 4: Reconcile staging**

`zoo apply` reconciles the whole staging environment, not just these two rows: it also deletes managed rows that are no longer in Mongo. Compare `uv run passiv zoo list --env staging` with Step 2's output first; the only difference must be the two new aliases.

```bash
uv run passiv zoo apply --env staging --confirm
```

Expected: `+ kimi-k2`, `+ deepseek-v4`, `0 deleted`, exit 0. Any `! unmanaged runtime route` or error line: stop and report it.

- [ ] **Step 5: Allow the agent-ctrl key to use both aliases**

```bash
export AGENT_CTRL_LITELLM_KEY="$("$OBX" vault get --env staging --field litellm_api_key agent-ctrl)"
uv run passiv stack keys allow-model --key-env AGENT_CTRL_LITELLM_KEY kimi-k2 --confirm
uv run passiv stack keys allow-model --key-env AGENT_CTRL_LITELLM_KEY deepseek-v4 --confirm
```

Expected: `key_alias=… now allows: …,kimi-k2` (then `…,deepseek-v4`), or `already allows`. If the key's `models` list is empty (all models allowed), `allow-model` would narrow it to the listed models; check `info.models` first with `curl -s "$PASSIV_LITELLM_URL/key/info?key=$AGENT_CTRL_LITELLM_KEY" -H "Authorization: Bearer $LITELLM_MASTER_KEY" | jq '.info.models'` and skip this step if it prints `[]`.

- [ ] **Step 6: Prove both aliases answer with the agent-ctrl key**

```bash
for model in kimi-k2 deepseek-v4; do
  curl -s "$PASSIV_LITELLM_URL/v1/chat/completions" \
    -H "Authorization: Bearer $AGENT_CTRL_LITELLM_KEY" -H "Content-Type: application/json" \
    -d "{\"model\":\"$model\",\"messages\":[{\"role\":\"user\",\"content\":\"Reply with OK\"}],\"max_tokens\":16}" \
    | jq -r '.choices[0].message.content // .error.message'
done
unset AGENT_CTRL_LITELLM_KEY LITELLM_MASTER_KEY
```

Expected: two short replies, no `error`. If a probe fails, delete that alias (`uv run passiv zoo delete <alias> --env staging --confirm`) and report.

---

### Task 8: Push, PR, staging rollout (requires the user's go-ahead) and smoke

**Files:** none.

- [ ] **Step 1: Final local gate**

```bash
cd T:/projects/SOTZ3N/stoz3n-agent-ctrl-kimi-dsh
cargo fmt -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
python -m unittest discover -s deploy/k8s -p "test_*.py"
node --test deploy/k8s/gitlab-scoped.test.cjs
git status --short
```

Expected: everything passes (the three test commands are what CI runs); `git status` shows nothing uncommitted from this plan.

- [ ] **Step 2: Push the branch and open the PR into `dev`**

```bash
git push -u origin feat/kimi-dsh-agents
tea pulls create --repo stozn/stoz3n-agent-ctrl --base dev --head feat/kimi-dsh-agents \
  --title "Kimi Code and DeepSeek Harness agents" \
  --description "$(cat <<'EOF'
Adds the kimi and dsh backends (ACP), ACP session resume (session/resume, else
session/load with the replay discarded) and session modes, a per-run run dir for
CLI session homes, staging workers and rw profiles on passiv, and both CLIs in
the node image.

passiv aliases (Task 7): kimi-k2 -> <openrouter id>, deepseek-v4 -> <openrouter id>.
Moving a kimi/dsh session to AgentCtrl is still refused (place_session) until Plan D.

Spec: warp docs/superpowers/specs/2026-10-08-kimi-dsh-agents-design.md (part B)

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

Fill the two OpenRouter ids from Task 7 Step 1 into the description before running it. Expected: a PR URL. The PR's CI run (tests only) must pass.

- [ ] **Step 3: STOP — ask the user for the go-ahead to merge**

Merging into `dev` builds the image and deploys staging AgentCtrl (controller and node restart; live interactive runs on the node end). State that, the PR URL, the CI result, and that Task 7 is done. Do not merge without an explicit yes.

- [ ] **Step 4: After the merge, follow the deploy**

The `dev` push runs `.gitea/workflows/deploy.yml`: builds and pushes `harbor.pseekoo.io/pseekoo-stoz3n-agent-ctrl/app:<short sha>`, applies `render.py --instance staging --image-tag <sha>`, waits for both rollouts. Watch it in Gitea (`tea actions runs list --repo stozn/stoz3n-agent-ctrl` or the Actions tab). Then:

```bash
export KUBECONFIG=~/.kube/obachan-staging
kubectl --server https://staging-gateway:6443 --tls-server-name 10.96.0.1 \
  -n agent-ctrl rollout status deploy/agent-ctrl-node --timeout=600s
```

(The `--server`/`--tls-server-name` override is needed when the kubeconfig's default server is unreachable.)

- [ ] **Step 5: In-pod smoke**

```bash
K="kubectl --server https://staging-gateway:6443 --tls-server-name 10.96.0.1 -n agent-ctrl"
$K exec deploy/agent-ctrl-node -c node -- sh -c '
  kimi --version && dsh --version &&
  DSH_HOME=/tmp/dsh-check dsh --profile acp \
    --patch /agent-config/dsh-passiv.patch.yml --patch /agent-config/dsh-acp.patch.yml \
    --dump-config | grep -nE "passiv|deepseek-v4|session-log-deepseek|plugin-package-inventory-deepseek" ;
  rm -rf /tmp/dsh-check'
```

Expected: both versions; the dumped tree names the `passiv` provider and `deepseek-v4` on the `acp` row, and shows both telemetry plugins disabled. Then confirm the controller sees the agents: `$K logs deploy/agent-ctrl-node -c node | grep -i backends` lists `kimi` and `dsh`.

- [ ] **Step 6: Staging fresh runs (one per agent)**

From the WarpSpeed CLI (profile names are accepted as `--agent`):

```bash
warpspeed agent-ctrl repos
warpspeed agent-ctrl start --agent staging-kimi-interactive-rw --repo <a reachable repo URL from the list>
warpspeed agent-ctrl open <job>
```

In the viewer send: "Create a file KIMI_SMOKE.md containing the word codeword-kimi, then reply DONE." Expect an answer, then stop the run. Repeat with `staging-dsh-interactive-rw` and `DSH_SMOKE.md` / `codeword-dsh`. For each: the repo's origin gets `agent/<job>` with an `agent-ctrl run <job> end` commit containing the file; `$K exec deploy/agent-ctrl-node -c node -- ls /workspace/.runs` shows neither `<job>` nor `<job>.session` afterwards.

If a run errors on its first turn with an auth or model error, check Task 7 Step 6 again with the same alias. If a Kimi run fails at `session/new` with an ACP auth error, Kimi's `ensureAuthed` did not accept the env credentials; report it (risk listed below) rather than adding a login.

- [ ] **Step 7: Report**

PR merged sha, image tag, smoke results per agent, and anything that deviated.

---

## Spec gaps and contradictions found while planning

1. **dsh patch location (B·3/B·4).** The spec seeds `dsh-home/cordis.patch.yml` into `~/.dsh`. dsh reads the user patch layer from `$DSH_HOME/profiles/<profile>/cordis.patch.yml` (`packages/boot/app-boot/src/profile.ts`), never from `$DSH_HOME/cordis.patch.yml`, and each run has its own `DSH_HOME`, so a seed in `~/.dsh` would not be read. The plan passes two `--patch` overlays from the ConfigMap instead (no init-container change).
2. **One patch cannot serve both profiles.** The `acp` row exists only in the ACP profile, and headless runs select their model through `agent-default-model`. The plan splits the overlay into `dsh-passiv.patch.yml` (route, default model, telemetry plugins) and `dsh-acp.patch.yml` (the `acp` row).
3. **Run dir at spawn.** Adapters do not receive `run_dir` at spawn (only `place_session` does). The plan adds `AGENT_CTRL_RUN_DIR`, set by the node for fresh and moved runs.
4. **"Neither capability falls back to session/new and the run errors".** The plan fails the run directly and opens no session.
5. **Gemini and OpenCode resume** (B·1 says they gain it). Not enabled: their CLIs' ACP resume support is unverified and nothing on `dev` calls it. Enabling it later means flipping `supports_resume()` once verified.
6. **Resume is not reachable for kimi/dsh on `dev`.** The only resume caller is the moved-session path, which calls `place_session` first; the trait default returns `Unsupported`. The pause branch resumes in place through `locate_session` (default `None`), so a paused kimi/dsh run would restart as a new conversation until Plan D (or the pause merge) adds `locate_session` for them. Their sessions live under `<run_dir>/kimi-home` and `<run_dir>/dsh-home`. Plan D should also check whether a pause keeps `<worktree>.session`.
7. **Kimi telemetry.** The spec turns off only dsh telemetry. Kimi also sends telemetry (`KIMI_DISABLE_TELEMETRY`); the plan sets it in the Kimi profiles.
8. **Kimi mode semantics.** `acp-server/src/modes.ts` describes `auto` as "auto-approve safe operations" and `yolo` as "auto-approve everything". The CLI help and the policy code confirm the spec instead (`auto` never asks). The plan follows the spec.
9. **Ticket workers** (`kimi-worker`, `dsh-worker`) need one-shot argument forms the spec does not give. The plan uses `kimi [--auto] -p` and `dsh --profile headless … <task>`. All four new profiles are rendered for staging only.
10. **dsh permission mode.** The spec puts `DSH_PERMISSION_MODE` per profile. The plan derives it in the adapter from the profile's `permission_mode` (`danger-full-access`/`bypassPermissions` → `danger-full-access`; `read-only`; else `workspace-write`), and the profiles set `permission_mode: danger-full-access`.
11. **passiv CLI details.** `passiv zoo add` sets no `key_ref` by default and serves on staging and prod unless `--env` is given. The plan passes `--key-ref providers/openrouter/api_key --env staging`. `zoo apply` reconciles the whole environment, so the plan diffs `zoo list` first.
12. **For Plan D:** `validate_session_id` accepts only `[A-Za-z0-9-]`, so it rejects Kimi's `session_<uuid>` ids.

Risks not resolved by this plan: Kimi's ACP `ensureAuthed` must accept env credentials (`auth.ensureReady()` with the `KIMI_MODEL_*` provider; read from source, not run); dsh is an alpha; OpenRouter ids and context sizes are looked up at apply time.
