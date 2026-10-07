# AgentCtrl Pause and Second Node — Plan C (WarpSpeed)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Pause and resume AgentCtrl runs from WarpSpeed (CLI, fleet pane, viewer), show which node a run is on, accept `claude|pi|codex` for `agent-ctrl start --agent`, and make the GUI terminal manager repaint a detached supervisor session it attaches to.

**Architecture:** `crates/managed_agent_sessions` gains `pause_run`/`resume_run` (one shared POST helper with `stop_run`), typed recognizers for the dashboard's 412 `session_unknown` and 409 `node_at_capacity`, operator-facing failure messages, `RunSummary.node_name`/`paused_at`, an agent-name resolver for `start`, and a paused notice plus `resume` command in the `agent-ctrl open` viewer. `crates/warp_cli` adds the `pause`/`resume` subcommands. The fleet model gets `pause`/`resume` backend calls and ranks paused rows between live and finished ones; the fleet view adds Pause/Resume buttons and the node name. The GUI `TerminalManager<TerminalView>` overrides `attach_detached_session` the way the TUI wrapper does.

**Tech Stack:** Rust; clap (`crates/warp_cli`); reqwest blocking (`crates/managed_agent_sessions`); WarpUI (`app/`); cargo-nextest.

## Global Constraints

- Spec C (verbatim from `docs/superpowers/specs/2026-10-07-agentctrl-pause-and-second-node-design.md`):
  - `managed_agent_sessions`: `pause_run(env, job)`, `resume_run(env, job)`; `RunSummary.node_name` and `paused_at`; `is_finished_state` stays as is (paused is live).
  - CLI: `agent-ctrl pause <job>`, `agent-ctrl resume <job>`; `agent-ctrl start --agent` accepts `claude|pi|codex` (mapped through `agent_ctrl_profile`) as well as a profile name; an unknown name fails locally with the list of accepted values.
  - Fleet pane: rows show `paused` as a live state (sorted after running), row actions Pause (on running rows) and Resume (on paused rows), the node name in the row.
  - Viewer: when the attached run reports `paused`, the viewer prints "paused — resume with …" and offers the resume action; `attach` to a paused run does not fail.
  - GUI terminal manager implements `attach_detached_session` (sets `repaint_screen_on_attach` and restores the persistent session), with a test alongside the restore-resume one.
- Dashboard contract (Spec B): `POST /api/agent-control/runs/:id/pause` and `/resume` → `PauseAgent` / `ResumeAgent`; errors map as for stop (409 `node_at_capacity` on resume, 412 `session_unknown` on pause). `paused` joins `ALLOWED_JOB_STATES`; it is **not** terminal. Capabilities: `pause` when `state == running` and the run is interactive with a known session id; `resume` when `state == paused`. `/runs` items gain `node_name` and `paused_at`. Pause and resume are idempotent on the server (pausing a paused run, resuming a running run return the current state).
- Request shape this plan assumes (Spec B does not fix it): body `{}`, an `Idempotency-Key: warpspeed-<uuid>` header and `X-Stozn-Client: warpspeed`, response `{"status":"accepted", …}` checked with `ensure_command_accepted` — the same as `stop`.
- Pause and Resume are row buttons of the fleet pane (pane-local `FleetViewAction`s), not workspace actions, so they get no palette entry or context flag; the CLI covers keyboard use.
- Conventions: AGENTS.md (ctx last, exhaustive matches, no `_`-prefixed params, inline format args, "why"-only comments, tests in `*_tests.rs`, no `std::process::Command`/`std::time::Instant` in the app crate, never re-lock the TerminalModel while a caller holds it); new actions get palette entries + context flags.
- Branch `feat/agentctrl-pause` (off dev 057429db) in worktree `T:/projects/dev-ops/warp-agentctrl-pause`; push to `gitea` and `gitlab`, never `origin`. Build with `CARGO_TARGET_DIR=T:/projects/dev-ops/warp-dev-release/target`; never restart running WarpSpeed; baseline: ~22 pre-existing `workspace::view`/`StoznAuthManager` failures on dev. Stage explicit paths only (other sessions edit sibling trees).
- Lands after Spec A and Spec B are on staging (Task C8 needs them); Tasks C1–C7 are testable without them.

---

### Task C1: Pause and resume calls, and the run's node and pause time

**Files:** Modify `crates/managed_agent_sessions/src/lib.rs` (`RunSummary` :413-427, `stop_run_with` :824-848, `is_node_at_capacity` :1510-1520), `crates/managed_agent_sessions/src/tests.rs` (`full_managed_flow_has_dashboard_only_network_authority` :353-391, `list_runs_parses_repo_and_branch` :849-899, append at the end :958), `app/src/agent_ctrl/fleet_model_tests.rs` (`run` helper :7-17, so the app's tests keep compiling).

**Interfaces:**
- Consumes: `send`, `ensure_command_accepted`, `validate_job_id`, `dashboard_token`, `profile`, `HttpStatusError`, `EnvironmentProfile` (all existing in `lib.rs`).
- Produces:
```rust
// RunSummary +=
#[serde(default)] pub node_name: Option<String>,
#[serde(default)] pub paused_at: Option<String>,

pub fn pause_run(environment: ManagedEnvironment, job_id: &str) -> Result<()>;
pub fn resume_run(environment: ManagedEnvironment, job_id: &str) -> Result<()>;
pub fn is_session_unknown(error: &anyhow::Error) -> bool;
pub fn pause_failure_message(error: &anyhow::Error) -> String;
pub fn resume_failure_message(error: &anyhow::Error) -> String;
fn pause_run_with(profile: &EnvironmentProfile, token: &str, job_id: &str) -> Result<Value>;
fn resume_run_with(profile: &EnvironmentProfile, token: &str, job_id: &str) -> Result<Value>;
fn post_run_command_with(profile: &EnvironmentProfile, token: &str, job_id: &str, command: &str, body: &Value) -> Result<Value>;
fn has_error_code(error: &anyhow::Error, status: StatusCode, code: &str) -> bool;
```

- [ ] **Step 1: Write the failing tests.** In `tests.rs`, add `("POST", "/api/agent-control/runs/job-123/pause")` and `("POST", "/api/agent-control/runs/job-123/resume")` to the route list of `full_managed_flow_has_dashboard_only_network_authority`; add `node_name: None, paused_at: None,` to both `RunSummary` literals of `list_runs_parses_repo_and_branch`; append:

```rust
#[test]
fn pause_and_resume_post_to_the_run_routes() {
    for (call, route) in [
        (
            pause_run_with as fn(&EnvironmentProfile, &str, &str) -> Result<Value>,
            "pause",
        ),
        (resume_run_with, "resume"),
    ] {
        let (base, request) = capturing_fixture(json!({ "status": "accepted" }));
        call(&fixture_profile(base), "token", "job-1").unwrap();
        let raw = String::from_utf8_lossy(&request.recv().unwrap()).into_owned();
        assert!(
            raw.starts_with(&format!("POST /api/agent-control/runs/job-1/{route} ")),
            "{raw}"
        );
        assert!(
            raw.to_ascii_lowercase()
                .contains("idempotency-key: warpspeed-"),
            "{raw}"
        );
        assert!(raw.ends_with("{}"), "{raw}");
    }
}

#[test]
fn a_rejected_pause_fails_with_the_dashboard_reason() {
    let base = http_fixture(
        "200 OK",
        json!({ "status": "rejected", "error": { "error": "run is not interactive" } }),
    );
    let error = pause_run_with(&fixture_profile(base), "token", "job-1").unwrap_err();
    assert!(error.to_string().contains("run is not interactive"), "{error}");
}

#[test]
fn a_pause_without_a_known_session_is_recognized() {
    let base = http_fixture(
        "412 Precondition Failed",
        json!({ "error": "session id unknown", "code": "session_unknown" }),
    );
    let error = pause_run_with(&fixture_profile(base), "token", "job-1").unwrap_err();
    assert!(is_session_unknown(&error), "{error:#}");
    assert!(!is_node_at_capacity(&error));
    let message = pause_failure_message(&error);
    assert!(message.contains("cannot be paused"), "{message}");
    assert!(!is_session_unknown(&anyhow::anyhow!("other")));
}

#[test]
fn a_resume_with_every_node_full_is_recognized() {
    let base = http_fixture(
        "409 Conflict",
        json!({ "error": "node at capacity", "code": "node_at_capacity" }),
    );
    let error = resume_run_with(&fixture_profile(base), "token", "job-1").unwrap_err();
    assert!(is_node_at_capacity(&error), "{error:#}");
    assert!(!is_session_unknown(&error));
    let message = resume_failure_message(&error);
    assert!(message.contains("at capacity"), "{message}");
}

#[test]
fn other_pause_and_resume_failures_keep_their_cause() {
    let error = anyhow::anyhow!("dashboard request failed");
    assert_eq!(
        pause_failure_message(&error),
        "Could not pause the run: dashboard request failed"
    );
    assert_eq!(
        resume_failure_message(&error),
        "Could not resume the run: dashboard request failed"
    );
}

#[test]
fn pause_and_resume_reject_a_bad_job_id_before_any_request() {
    for result in [
        pause_run(ManagedEnvironment::Staging, "../x"),
        resume_run(ManagedEnvironment::Staging, "job-a b"),
    ] {
        let error = result.unwrap_err();
        assert!(error.to_string().contains("invalid AgentCtrl job id"), "{error}");
    }
}

#[test]
fn list_runs_parses_node_and_pause_time() {
    let base = http_fixture(
        "200 OK",
        json!({ "items": [{
            "job_id": "job-paused",
            "state": "paused",
            "agent_name": "staging-claude-interactive-rw",
            "interactive": true,
            "repo_url": "https://gitlab.com/pseekoo/kaizen",
            "branch": "agent/job-paused",
            "last_activity_at": "2026-10-07T09:00:00.000Z",
            "node_name": "agent-ctrl-node-1",
            "paused_at": "2026-10-07T09:05:00.000Z"
        }] }),
    );
    let runs = list_runs_with(&fixture_profile(base), "token").unwrap();
    assert_eq!(runs[0].state, "paused");
    assert_eq!(runs[0].node_name.as_deref(), Some("agent-ctrl-node-1"));
    assert_eq!(runs[0].paused_at.as_deref(), Some("2026-10-07T09:05:00.000Z"));
}
```

In `app/src/agent_ctrl/fleet_model_tests.rs`, extend the `run` helper's literal with `node_name: None,` and `paused_at: None,`.

- [ ] **Step 2: Run to see it fail.**
`CARGO_TARGET_DIR=T:/projects/dev-ops/warp-dev-release/target cargo nextest run -p managed_agent_sessions pause resume list_runs full_managed_flow`
Expected: compile errors (`pause_run_with`, `is_session_unknown`, `node_name` … not found).

- [ ] **Step 3: Implement.** In `lib.rs`:

`RunSummary` (after `last_activity_at`):
```rust
    /// The AgentCtrl node the run is on, or last ran on while paused.
    #[serde(default)]
    pub node_name: Option<String>,
    #[serde(default)]
    pub paused_at: Option<String>,
```

Replace `stop_run_with`'s body and add the pause/resume calls next to `stop_run`:
```rust
fn stop_run_with(
    profile: &EnvironmentProfile,
    token: &str,
    job_id: &str,
    force: bool,
) -> Result<Value> {
    post_run_command_with(profile, token, job_id, "stop", &json!({ "force": force }))
}

/// Asks the dashboard to pause a live interactive run: its agent exits, its work is pushed to the
/// run's branch, and it stops holding a node slot. A run whose agent session AgentCtrl does not
/// know fails with an error [`is_session_unknown`] recognizes.
pub fn pause_run(environment: ManagedEnvironment, job_id: &str) -> Result<()> {
    validate_job_id(job_id)?;
    let token = dashboard_token()?;
    pause_run_with(profile(environment), &token, job_id).map(drop)
}

/// Asks the dashboard to resume a paused run on a node with a free slot. With every node full it
/// fails with an error [`is_node_at_capacity`] recognizes.
pub fn resume_run(environment: ManagedEnvironment, job_id: &str) -> Result<()> {
    validate_job_id(job_id)?;
    let token = dashboard_token()?;
    resume_run_with(profile(environment), &token, job_id).map(drop)
}

fn pause_run_with(profile: &EnvironmentProfile, token: &str, job_id: &str) -> Result<Value> {
    post_run_command_with(profile, token, job_id, "pause", &json!({}))
}

fn resume_run_with(profile: &EnvironmentProfile, token: &str, job_id: &str) -> Result<Value> {
    post_run_command_with(profile, token, job_id, "resume", &json!({}))
}

/// Posts `body` to the run's `command` route and requires the dashboard to accept it.
fn post_run_command_with(
    profile: &EnvironmentProfile,
    token: &str,
    job_id: &str,
    command: &str,
    body: &Value,
) -> Result<Value> {
    let response = send(
        profile,
        Client::new()
            .post(format!(
                "{}/api/agent-control/runs/{job_id}/{command}",
                profile.base_url
            ))
            .bearer_auth(token)
            .header("X-Stozn-Client", "warpspeed")
            .header(
                "Idempotency-Key",
                format!("warpspeed-{}", uuid::Uuid::new_v4()),
            )
            .json(body),
    )?;
    let result: Value = response
        .json()
        .with_context(|| format!("invalid {command} response"))?;
    ensure_command_accepted(&result)?;
    Ok(result)
}
```

Replace `is_node_at_capacity` and add the rest after it:
```rust
/// Whether the dashboard refused a launch or resume because no node has a free interactive slot.
pub fn is_node_at_capacity(error: &anyhow::Error) -> bool {
    has_error_code(error, StatusCode::CONFLICT, "node_at_capacity")
}

/// Whether the dashboard refused a pause because AgentCtrl does not know the run's agent session,
/// without which the run could never be resumed.
pub fn is_session_unknown(error: &anyhow::Error) -> bool {
    has_error_code(error, StatusCode::PRECONDITION_FAILED, "session_unknown")
}

fn has_error_code(error: &anyhow::Error, status: StatusCode, code: &str) -> bool {
    error
        .downcast_ref::<HttpStatusError>()
        .is_some_and(|error| {
            error.status == status
                && serde_json::from_str::<Value>(&error.body)
                    .is_ok_and(|body| body.get("code").and_then(Value::as_str) == Some(code))
        })
}

pub fn pause_failure_message(error: &anyhow::Error) -> String {
    if is_session_unknown(error) {
        "AgentCtrl does not know this run's agent session, so it cannot be paused".to_owned()
    } else {
        format!("Could not pause the run: {error:#}")
    }
}

pub fn resume_failure_message(error: &anyhow::Error) -> String {
    if is_node_at_capacity(error) {
        "Every AgentCtrl node is at capacity; stop or pause a run, then resume again".to_owned()
    } else {
        format!("Could not resume the run: {error:#}")
    }
}
```

- [ ] **Step 4: Run to pass.** Same command as Step 2 → all pass; then `CARGO_TARGET_DIR=T:/projects/dev-ops/warp-dev-release/target cargo nextest run -p managed_agent_sessions` (whole crate, `stop_run_posts_a_stop_request` still green) and `CARGO_TARGET_DIR=T:/projects/dev-ops/warp-dev-release/target cargo nextest run -p warp fleet_model` (helper compiles).
- [ ] **Step 5: Lint and format.** `CARGO_TARGET_DIR=T:/projects/dev-ops/warp-dev-release/target cargo clippy -p managed_agent_sessions --all-targets --tests -- -D warnings`; `./script/format`.
- [ ] **Step 6: Commit.**
```bash
git add crates/managed_agent_sessions/src/lib.rs crates/managed_agent_sessions/src/tests.rs app/src/agent_ctrl/fleet_model_tests.rs
git commit -m "feat(agent-ctrl): pause and resume runs, and read their node and pause time"
```

### Task C2: `agent-ctrl pause <job>` and `agent-ctrl resume <job>`

**Files:** Modify `crates/warp_cli/src/managed_sessions.rs` (after `Stop` :109-118), `crates/warp_cli/src/lib_tests.rs` (after `managed_agent_repos_and_move_all_parse` :129-141), `crates/managed_agent_sessions/src/lib.rs` (`run` :295-351; new `pause`/`resume` next to `stop` :800-816), `crates/managed_agent_sessions/src/tests.rs` (append), `app/src/agent_ctrl_fleet_cli.rs` (`move_all_request` match :175-183).

**Interfaces:**
- Consumes: `pause_run_with`, `resume_run_with`, `pause_failure_message`, `resume_failure_message` (C1).
- Produces:
```rust
// clap
ManagedSessionsCommand::Pause { job_id: String, environment: ManagedEnvironment }
ManagedSessionsCommand::Resume { job_id: String, environment: ManagedEnvironment }
// managed_agent_sessions
fn pause(environment: ManagedEnvironment, job_id: &str, json_output: bool) -> Result<()>;
fn resume(environment: ManagedEnvironment, job_id: &str, json_output: bool) -> Result<()>;
```

- [ ] **Step 1: Write the failing tests.** `lib_tests.rs`:
```rust
#[test]
fn managed_agent_pause_and_resume_parse() {
    let command = agent_ctrl_command(&["pause", "job-1"]);
    let managed_sessions::ManagedSessionsCommand::Pause {
        job_id,
        environment,
    } = command
    else {
        panic!("expected pause, got {command:?}");
    };
    assert_eq!(job_id, "job-1");
    assert_eq!(environment, managed_sessions::ManagedEnvironment::Staging);

    let command = agent_ctrl_command(&["resume", "job-2"]);
    let managed_sessions::ManagedSessionsCommand::Resume { job_id, .. } = command else {
        panic!("expected resume, got {command:?}");
    };
    assert_eq!(job_id, "job-2");

    assert!(Args::try_parse_from(["warpspeed", "agent-ctrl", "pause"]).is_err());
    assert!(Args::try_parse_from(["warpspeed", "agent-ctrl", "resume"]).is_err());
}
```
`managed_agent_sessions/src/tests.rs`:
```rust
#[test]
fn pause_and_resume_commands_check_the_job_id_before_any_request() {
    for command in [
        ManagedSessionsCommand::Pause {
            job_id: "../x".to_owned(),
            environment: ManagedEnvironment::Staging,
        },
        ManagedSessionsCommand::Resume {
            job_id: "../x".to_owned(),
            environment: ManagedEnvironment::Staging,
        },
    ] {
        let error = run(&ManagedSessionsArgs {
            command,
            json: false,
        })
        .unwrap_err();
        assert!(error.to_string().contains("invalid AgentCtrl job id"), "{error}");
    }
}
```

- [ ] **Step 2: Run to see it fail.**
`CARGO_TARGET_DIR=T:/projects/dev-ops/warp-dev-release/target cargo nextest run -p warp_cli -p managed_agent_sessions pause_and_resume`
Expected: compile errors (no variant `Pause`/`Resume`).

- [ ] **Step 3: Implement.** `managed_sessions.rs`, after `Stop`:
```rust
    /// Pause a live interactive run: its agent exits, its work is pushed to the run's branch, and
    /// it frees its node slot until resumed.
    Pause {
        job_id: String,

        #[arg(long, value_enum, default_value_t = ManagedEnvironment::Staging)]
        environment: ManagedEnvironment,
    },

    /// Resume a paused run, with its conversation, on a node with a free slot.
    Resume {
        job_id: String,

        #[arg(long, value_enum, default_value_t = ManagedEnvironment::Staging)]
        environment: ManagedEnvironment,
    },
```
`lib.rs` `run`, after the `Stop` arm:
```rust
        ManagedSessionsCommand::Pause {
            job_id,
            environment,
        } => pause(*environment, job_id, args.json),
        ManagedSessionsCommand::Resume {
            job_id,
            environment,
        } => resume(*environment, job_id, args.json),
```
`lib.rs`, after `stop`:
```rust
fn pause(environment: ManagedEnvironment, job_id: &str, json_output: bool) -> Result<()> {
    validate_job_id(job_id)?;
    let token = dashboard_token()?;
    let result = pause_run_with(profile(environment), &token, job_id)
        .map_err(|error| anyhow::Error::msg(pause_failure_message(&error)))?;
    if json_output {
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else {
        println!("Pause requested for {job_id}. Resume with: warpspeed agent-ctrl resume {job_id}");
    }
    Ok(())
}

fn resume(environment: ManagedEnvironment, job_id: &str, json_output: bool) -> Result<()> {
    validate_job_id(job_id)?;
    let token = dashboard_token()?;
    let result = resume_run_with(profile(environment), &token, job_id)
        .map_err(|error| anyhow::Error::msg(resume_failure_message(&error)))?;
    if json_output {
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else {
        println!("Resume requested for {job_id}. Open with: warpspeed agent-ctrl open {job_id}");
    }
    Ok(())
}
```
`app/src/agent_ctrl_fleet_cli.rs` `move_all_request`, extend the `None` arm:
```rust
        | ManagedSessionsCommand::Stop { .. }
        | ManagedSessionsCommand::Pause { .. }
        | ManagedSessionsCommand::Resume { .. } => None,
```

- [ ] **Step 4: Run to pass.** Step 2 command → pass; `CARGO_TARGET_DIR=T:/projects/dev-ops/warp-dev-release/target cargo nextest run -p warp agent_ctrl_fleet_cli` → pass.
- [ ] **Step 5: Lint and format.** `CARGO_TARGET_DIR=T:/projects/dev-ops/warp-dev-release/target cargo clippy -p warp_cli -p managed_agent_sessions -p warp --all-targets --tests -- -D warnings`; `./script/format`.
- [ ] **Step 6: Commit.**
```bash
git add crates/warp_cli/src/managed_sessions.rs crates/warp_cli/src/lib_tests.rs crates/managed_agent_sessions/src/lib.rs crates/managed_agent_sessions/src/tests.rs app/src/agent_ctrl_fleet_cli.rs
git commit -m "feat(cli): pause and resume an AgentCtrl run"
```

### Task C3: `agent-ctrl start --agent claude|pi|codex`

**Files:** Modify `crates/managed_agent_sessions/src/lib.rs` (`run` Start arm :297-320; `agent_ctrl_profile` :482-492), `crates/managed_agent_sessions/src/tests.rs` (append after `agent_ctrl_profiles_are_the_interactive_rw_profiles` :934-946), `crates/warp_cli/src/managed_sessions.rs` (`--agent` doc :26-28), `crates/warp_cli/src/lib_tests.rs` (append).

**Interfaces:**
- Consumes: `agent_ctrl_profile(&str) -> Option<&'static str>`, `profile(environment).alias`.
- Produces:
```rust
pub const AGENT_KINDS: [&str; 3] = ["claude", "pi", "codex"];
pub fn resolve_agent_name(environment: ManagedEnvironment, agent: &str) -> Result<String>;
```
A configured profile name is recognized by the environment's profile prefix (`staging-` followed by `[A-Za-z0-9._-]+`), the naming every AgentCtrl profile follows; the client has no list of configured profiles.

- [ ] **Step 1: Write the failing tests.** `tests.rs`:
```rust
#[test]
fn agent_kinds_resolve_to_their_profiles_and_profile_names_pass_through() {
    let resolve = |agent| resolve_agent_name(ManagedEnvironment::Staging, agent).unwrap();
    assert_eq!(resolve("claude"), "staging-claude-interactive-rw");
    assert_eq!(resolve("pi"), "staging-pi-interactive-rw");
    assert_eq!(resolve("codex"), "staging-codex-interactive-rw");
    assert_eq!(resolve("staging-pi-lander"), "staging-pi-lander");
    assert_eq!(
        resolve("staging-claude-interactive-rw"),
        "staging-claude-interactive-rw"
    );
}

#[test]
fn an_unknown_agent_fails_with_the_accepted_values() {
    for unknown in ["gemini", "Claude", "staging-", "prod-claude", "staging-a b", ""] {
        let error = resolve_agent_name(ManagedEnvironment::Staging, unknown)
            .unwrap_err()
            .to_string();
        assert!(error.contains("claude, pi, codex"), "{unknown:?}: {error}");
        assert!(error.contains("staging-"), "{unknown:?}: {error}");
    }
}

#[test]
fn start_with_an_unknown_agent_fails_before_any_request() {
    let error = run(&ManagedSessionsArgs {
        command: ManagedSessionsCommand::Start {
            agent: "gemini".to_owned(),
            task: Some("task-1".to_owned()),
            repo: None,
            branch: None,
            prompt: None,
            interactive: false,
            environment: ManagedEnvironment::Staging,
        },
        json: false,
    })
    .unwrap_err();
    assert!(error.to_string().contains("unknown agent"), "{error}");
}
```
`lib_tests.rs`:
```rust
#[test]
fn managed_agent_start_takes_an_agent_kind() {
    let command = agent_ctrl_command(&["start", "--agent", "claude", "--repo", "https://x/y"]);
    let managed_sessions::ManagedSessionsCommand::Start { agent, .. } = command else {
        panic!("expected start, got {command:?}");
    };
    assert_eq!(agent, "claude");
}
```

- [ ] **Step 2: Run to see it fail.**
`CARGO_TARGET_DIR=T:/projects/dev-ops/warp-dev-release/target cargo nextest run -p managed_agent_sessions -p warp_cli agent`
Expected: compile error (`resolve_agent_name` not found); `managed_agent_start_takes_an_agent_kind` already passes (clap does not validate the value).

- [ ] **Step 3: Implement.** `lib.rs`, next to `agent_ctrl_profile` (whose match stays the single mapping):
```rust
/// The CLI agent kinds that [`agent_ctrl_profile`] maps to an AgentCtrl profile.
pub const AGENT_KINDS: [&str; 3] = ["claude", "pi", "codex"];

/// The AgentCtrl profile that `agent` names: a CLI agent kind maps through
/// [`agent_ctrl_profile`], and a name with the environment's profile prefix is taken as a
/// configured profile. Anything else fails before any request, listing what is accepted.
pub fn resolve_agent_name(environment: ManagedEnvironment, agent: &str) -> Result<String> {
    if let Some(profile_name) = agent_ctrl_profile(agent) {
        return Ok(profile_name.to_owned());
    }
    let alias = profile(environment).alias;
    let is_profile_name = agent
        .strip_prefix(alias)
        .and_then(|rest| rest.strip_prefix('-'))
        .is_some_and(|rest| {
            !rest.is_empty()
                && rest
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
        });
    if is_profile_name {
        return Ok(agent.to_owned());
    }
    bail!(
        "unknown agent {agent:?}; use {} or a profile name starting with {alias}- (for example {alias}-claude-interactive-rw)",
        AGENT_KINDS.join(", ")
    )
}
```
`run`, Start arm — resolve before dispatching:
```rust
        ManagedSessionsCommand::Start {
            agent,
            task,
            repo,
            branch,
            prompt,
            interactive,
            environment,
        } => {
            let agent = resolve_agent_name(*environment, agent)?;
            match (repo, task) {
                (Some(repo), _) => start_on_repo(
                    *environment,
                    &agent,
                    repo,
                    branch.as_deref(),
                    prompt.as_deref(),
                    args.json,
                ),
                (None, Some(task)) => start(
                    *environment,
                    &agent,
                    task,
                    prompt.as_deref(),
                    *interactive,
                    args.json,
                ),
                (None, None) => bail!("start requires --task or --repo"),
            }
        }
```
`managed_sessions.rs` `--agent` doc: `/// Agent kind (claude, pi, codex) or configured AgentCtrl profile name.`

- [ ] **Step 4: Run to pass.** Step 2 command → pass; `CARGO_TARGET_DIR=T:/projects/dev-ops/warp-dev-release/target cargo nextest run -p managed_agent_sessions -p warp_cli managed_agent start` → existing start tests green.
- [ ] **Step 5: Lint and format.** `CARGO_TARGET_DIR=T:/projects/dev-ops/warp-dev-release/target cargo clippy -p managed_agent_sessions -p warp_cli --all-targets --tests -- -D warnings`; `./script/format`.
- [ ] **Step 6: Commit.**
```bash
git add crates/managed_agent_sessions/src/lib.rs crates/managed_agent_sessions/src/tests.rs crates/warp_cli/src/managed_sessions.rs crates/warp_cli/src/lib_tests.rs
git commit -m "feat(cli): start an AgentCtrl run by agent kind"
```

### Task C4: Viewer notice and resume command for a paused run

The viewer is `ui_session` (`agent-ctrl open`) and `attach` in `crates/managed_agent_sessions/src/lib.rs`. Nothing local rejects a paused run today (`validate_descriptor` does not check the state, `session_is_finished("paused")` is false); this task adds the notice and the command and pins both with tests.

**Files:** Modify `crates/managed_agent_sessions/src/lib.rs` (`ui_session` :695-728, `session_is_finished` :730-732, `attach` banner :891-897), `crates/managed_agent_sessions/src/tests.rs` (`waiting_input_remains_controllable_in_the_ui` :113-118; append).

**Interfaces:**
- Consumes: `resume_run`, `resume_failure_message` (C1), `SessionDescriptor`, `fetch_descriptor`.
- Produces:
```rust
pub fn is_paused_state(state: &str) -> bool;
fn attach_banner(descriptor: &SessionDescriptor) -> String;
fn paused_notice(job_id: &str) -> String;
fn viewer_commands(state: &str) -> &'static str;
```

- [ ] **Step 1: Write the failing tests.** Add `assert!(!session_is_finished("paused"));` to `waiting_input_remains_controllable_in_the_ui`; append:
```rust
#[test]
fn a_paused_run_is_attached_with_a_resume_notice() {
    let mut body = descriptor_json();
    body["state"] = json!("paused");
    let base = http_fixture("200 OK", body);
    let descriptor =
        fetch_descriptor(&Client::new(), &fixture_profile(base), "job-123", "test-token").unwrap();

    assert!(is_paused_state(&descriptor.state));
    let banner = attach_banner(&descriptor);
    assert!(
        banner.contains("Paused — resume with: warpspeed agent-ctrl resume job-123"),
        "{banner}"
    );
    let commands = viewer_commands(&descriptor.state);
    assert!(commands.contains("resume"), "{commands}");
    assert!(!commands.contains("send"), "{commands}");
}

#[test]
fn a_running_run_is_attached_without_a_pause_notice() {
    let base = http_fixture("200 OK", descriptor_json());
    let descriptor =
        fetch_descriptor(&Client::new(), &fixture_profile(base), "job-123", "test-token").unwrap();

    let banner = attach_banner(&descriptor);
    assert!(banner.starts_with("Attached to Inspector (running, capabilities: watch,input,close)."));
    assert!(!banner.contains("Paused"), "{banner}");
    assert!(viewer_commands("running").contains("send <text>"));
}
```

- [ ] **Step 2: Run to see it fail.**
`CARGO_TARGET_DIR=T:/projects/dev-ops/warp-dev-release/target cargo nextest run -p managed_agent_sessions attached waiting_input`
Expected: compile errors (`attach_banner`, `is_paused_state`, `viewer_commands` not found).

- [ ] **Step 3: Implement.** After `session_is_finished`:
```rust
/// A paused run is live (not finished) but runs no agent until it is resumed.
pub fn is_paused_state(state: &str) -> bool {
    state == "paused"
}

fn paused_notice(job_id: &str) -> String {
    format!("Paused — resume with: warpspeed agent-ctrl resume {job_id}")
}

fn attach_banner(descriptor: &SessionDescriptor) -> String {
    let capabilities = descriptor.capabilities.join(",");
    let mut banner = format!(
        "Attached to {} ({}, capabilities: {capabilities}). Closing this view does not stop the run.",
        descriptor.display_name, descriptor.state
    );
    if is_paused_state(&descriptor.state) {
        banner.push('\n');
        banner.push_str(&paused_notice(&descriptor.job_id));
    }
    banner
}

fn viewer_commands(state: &str) -> &'static str {
    if is_paused_state(state) {
        "Commands: Enter=refresh, resume, stop, detach"
    } else {
        "Commands: Enter=refresh, send <text>, close, stop, detach"
    }
}
```
`attach`: replace the `if !json_output { … println!("Attached to …") }` block with:
```rust
    if !json_output {
        println!("{}", attach_banner(&descriptor));
    }
```
`ui_session`: replace the `println!("\nCommands: …")` line with `println!("\n{}", viewer_commands(&descriptor.state));`, add before the `send ` branch:
```rust
        if command == "resume" {
            match resume_run(environment, job_id) {
                Ok(()) => println!("Resume requested for {job_id}."),
                Err(error) => println!("{}", resume_failure_message(&error)),
            }
            continue;
        }
```
and change the last line to `println!("Unknown command. Use send <text>, close, resume, stop, refresh, or detach.");`.

- [ ] **Step 4: Run to pass.** Step 2 command → pass; whole crate `CARGO_TARGET_DIR=T:/projects/dev-ops/warp-dev-release/target cargo nextest run -p managed_agent_sessions` → pass.
- [ ] **Step 5: Lint and format.** `CARGO_TARGET_DIR=T:/projects/dev-ops/warp-dev-release/target cargo clippy -p managed_agent_sessions --all-targets --tests -- -D warnings`; `./script/format`.
- [ ] **Step 6: Commit.**
```bash
git add crates/managed_agent_sessions/src/lib.rs crates/managed_agent_sessions/src/tests.rs
git commit -m "feat(agent-ctrl): tell the viewer a run is paused and let it resume the run"
```

### Task C5: Fleet model — paused rows, node names, pause and resume

**Files:** Modify `app/src/agent_ctrl/fleet_model.rs` (import :6, `FleetBackend` :24-57, `FleetRow` :67-82, `stop` :228-242, row sort :336-338, `fleet_row` :342-353), `app/src/agent_ctrl/fleet_model_tests.rs` (`run` :7-17, `backend` :162-176, append), `app/src/workspace/view_tests.rs` (`initialize_app_for_fleet` :8057-8083).

**Interfaces:**
- Consumes: `pause_run`, `resume_run`, `pause_failure_message`, `resume_failure_message`, `RunSummary.node_name` (C1); `is_paused_state` (C4).
- Produces:
```rust
// FleetBackend +=
pub pause: fn(&str) -> anyhow::Result<()>,
pub resume: fn(&str) -> anyhow::Result<()>,
// FleetRow +=
pub node_name: Option<String>,
pub interactive: bool,
impl FleetRow {
    pub fn is_paused(&self) -> bool;
    pub fn can_pause(&self) -> bool;   // running && interactive
    pub fn can_resume(&self) -> bool;  // paused
    fn display_rank(&self) -> u8;      // live 0, paused 1, finished 2
}
impl FleetModel {
    pub fn pause(&mut self, job_id: String, ctx: &mut ModelContext<Self>);
    pub fn resume(&mut self, job_id: String, ctx: &mut ModelContext<Self>);
    fn run_command(&mut self, job_id: String, command: fn(&str) -> anyhow::Result<()>, failure_message: fn(&anyhow::Error) -> String, ctx: &mut ModelContext<Self>);
}
```

- [ ] **Step 1: Write the failing tests.** `fleet_model_tests.rs`: in `run`, set `node_name: Some("agent-ctrl-node-0".to_owned()),`; in `backend`, add `pause: no_pause, resume: no_resume,` with
```rust
fn no_pause(_: &str) -> anyhow::Result<()> {
    panic!("not paused in these tests")
}

fn no_resume(_: &str) -> anyhow::Result<()> {
    panic!("not resumed in these tests")
}
```
append:
```rust
#[test]
fn paused_runs_follow_the_running_ones_and_precede_the_finished_ones() {
    let groups = group_runs(
        &[
            run("job-done", "completed", Some(WARP)),
            run("job-paused", "paused", Some(WARP)),
            run("job-live", "running", Some(WARP)),
        ],
        &[repository("warp", WARP)],
    );

    assert_eq!(job_ids(&groups[0]), ["job-live", "job-paused", "job-done"]);
    assert!(!groups[0].rows[1].is_finished());
}

#[test]
fn rows_carry_the_node_name() {
    let mut on_second_node = run("job-a", "running", Some(WARP));
    on_second_node.node_name = Some("agent-ctrl-node-1".to_owned());
    let groups = group_runs(&[on_second_node], &[]);

    assert_eq!(groups[0].rows[0].node_name.as_deref(), Some("agent-ctrl-node-1"));
}

#[test]
fn only_running_interactive_runs_can_be_paused_and_only_paused_ones_resumed() {
    let mut worker = run("job-worker", "running", None);
    worker.interactive = false;
    let groups = group_runs(
        &[
            run("job-live", "running", None),
            run("job-paused", "paused", None),
            run("job-waiting", "waiting_input", None),
            worker,
        ],
        &[],
    );
    let row = |job_id: &str| {
        groups[0]
            .rows
            .iter()
            .find(|row| row.job_id == job_id)
            .unwrap()
            .clone()
    };

    assert!(row("job-live").can_pause() && !row("job-live").can_resume());
    assert!(row("job-paused").can_resume() && !row("job-paused").can_pause());
    assert!(!row("job-waiting").can_pause());
    assert!(!row("job-worker").can_pause());
}

fn pause_unknown_session(_: &str) -> anyhow::Result<()> {
    Err(managed_agent_sessions::HttpStatusError {
        status: reqwest::StatusCode::PRECONDITION_FAILED,
        body: r#"{"error":"session id unknown","code":"session_unknown"}"#.to_owned(),
    }
    .into())
}

fn resume_at_capacity(_: &str) -> anyhow::Result<()> {
    Err(managed_agent_sessions::HttpStatusError {
        status: reqwest::StatusCode::CONFLICT,
        body: r#"{"error":"node at capacity","code":"node_at_capacity"}"#.to_owned(),
    }
    .into())
}

/// Runs `act` on a model whose backend is `backend`, waits for the refresh that follows, and
/// returns the action error it leaves.
fn action_error_after(
    backend: FleetBackend,
    act: fn(&mut FleetModel, &mut ModelContext<FleetModel>),
) -> Option<String> {
    App::test((), |mut app| async move {
        let model = app.add_model(|_| FleetModel::with_backend(backend));
        model.update(&mut app, |model, ctx| act(model, ctx));
        assert_eventually_within!(
            std::time::Duration::from_secs(5) => model.read(&app, |model, _| {
                model.runs.len() == 2 && !model.is_refreshing()
            }),
            "the refresh after the action never landed"
        );
        model.read(&app, |model, _| model.action_error().map(str::to_owned))
    })
}

#[test]
fn a_pause_refused_for_an_unknown_session_says_why() {
    let error = action_error_after(
        FleetBackend {
            pause: pause_unknown_session,
            ..backend(list_two_runs)
        },
        |model, ctx| model.pause("job-a".to_owned(), ctx),
    )
    .expect("the pause failure is kept");
    assert!(error.contains("cannot be paused"), "{error}");
}

#[test]
fn a_resume_with_every_node_full_says_so() {
    let error = action_error_after(
        FleetBackend {
            resume: resume_at_capacity,
            ..backend(list_two_runs)
        },
        |model, ctx| model.resume("job-a".to_owned(), ctx),
    )
    .expect("the resume failure is kept");
    assert!(error.contains("at capacity"), "{error}");
}

#[test]
fn a_successful_pause_and_resume_leave_no_error() {
    let accept: fn(&str) -> anyhow::Result<()> = |_| Ok(());
    let paused = action_error_after(
        FleetBackend {
            pause: accept,
            ..backend(list_two_runs)
        },
        |model, ctx| model.pause("job-a".to_owned(), ctx),
    );
    let resumed = action_error_after(
        FleetBackend {
            resume: accept,
            ..backend(list_two_runs)
        },
        |model, ctx| model.resume("job-a".to_owned(), ctx),
    );
    assert_eq!((paused, resumed), (None, None));
}
```

`view_tests.rs` `initialize_app_for_fleet`: add `pause: pause_nothing, resume: resume_nothing,` to the backend literal with
```rust
fn pause_nothing(_: &str) -> anyhow::Result<()> {
    anyhow::bail!("tests never pause a run")
}

fn resume_nothing(_: &str) -> anyhow::Result<()> {
    anyhow::bail!("tests never resume a run")
}
```

- [ ] **Step 2: Run to see it fail.**
`CARGO_TARGET_DIR=T:/projects/dev-ops/warp-dev-release/target cargo nextest run -p warp fleet`
Expected: compile errors (no field `pause`/`node_name`, no method `can_pause`).

- [ ] **Step 3: Implement.** `fleet_model.rs`:
- import: `use managed_agent_sessions::{RepositoryInfo, RunSummary, is_paused_state, normalize_repo_url};`
- `FleetBackend` fields after `stop`: `pub pause: fn(&str) -> anyhow::Result<()>,` and `pub resume: fn(&str) -> anyhow::Result<()>,`; in `staging()`:
```rust
            pause: |job_id| managed_agent_sessions::pause_run(ManagedEnvironment::Staging, job_id),
            resume: |job_id| {
                managed_agent_sessions::resume_run(ManagedEnvironment::Staging, job_id)
            },
```
- `FleetRow` fields after `agent`:
```rust
    /// The AgentCtrl node the run is on, or last ran on while paused.
    pub node_name: Option<String>,
    pub interactive: bool,
```
and methods:
```rust
impl FleetRow {
    pub fn is_finished(&self) -> bool {
        is_finished_state(&self.state)
    }

    pub fn is_paused(&self) -> bool {
        is_paused_state(&self.state)
    }

    /// Only a running interactive run has an agent session AgentCtrl can resume later.
    pub fn can_pause(&self) -> bool {
        self.interactive && self.state == "running"
    }

    pub fn can_resume(&self) -> bool {
        self.is_paused()
    }

    fn display_rank(&self) -> u8 {
        if self.is_finished() {
            2
        } else if self.is_paused() {
            1
        } else {
            0
        }
    }
}
```
- `fleet_row`: add `node_name: run.node_name.clone(),` and `interactive: run.interactive,`.
- `group_runs`: `group.rows.sort_by_key(FleetRow::display_rank);` and doc line "…the live runs of each group first, then the paused ones, then the finished ones."
- Replace `stop` and add `pause`/`resume`:
```rust
    /// Asks AgentCtrl to stop `job_id` gracefully, then refreshes.
    pub fn stop(&mut self, job_id: String, ctx: &mut ModelContext<Self>) {
        let stop = self.backend.stop;
        self.run_command(
            job_id,
            stop,
            |error| format!("Could not stop the run: {error:#}"),
            ctx,
        );
    }

    /// Asks AgentCtrl to pause `job_id`, then refreshes.
    pub fn pause(&mut self, job_id: String, ctx: &mut ModelContext<Self>) {
        let pause = self.backend.pause;
        self.run_command(
            job_id,
            pause,
            managed_agent_sessions::pause_failure_message,
            ctx,
        );
    }

    /// Asks AgentCtrl to resume `job_id`, then refreshes.
    pub fn resume(&mut self, job_id: String, ctx: &mut ModelContext<Self>) {
        let resume = self.backend.resume;
        self.run_command(
            job_id,
            resume,
            managed_agent_sessions::resume_failure_message,
            ctx,
        );
    }

    fn run_command(
        &mut self,
        job_id: String,
        command: fn(&str) -> anyhow::Result<()>,
        failure_message: fn(&anyhow::Error) -> String,
        ctx: &mut ModelContext<Self>,
    ) {
        self.action_error = None;
        ctx.spawn(
            async move { blocking::unblock(move || command(&job_id)).await },
            move |me, result, ctx| {
                if let Err(error) = result {
                    me.action_error = Some(failure_message(&error));
                    ctx.emit(FleetModelEvent::Updated);
                }
                me.refresh(ctx);
            },
        );
    }
```
- Leave `is_finished_state` (:279-281) unchanged.

- [ ] **Step 4: Run to pass.** `CARGO_TARGET_DIR=T:/projects/dev-ops/warp-dev-release/target cargo nextest run -p warp fleet agent_ctrl` → pass (including the existing `finished_runs_follow_the_live_ones` and `a_failed_start_outlives_the_refresh_that_follows_it`).
- [ ] **Step 5: Lint and format.** `CARGO_TARGET_DIR=T:/projects/dev-ops/warp-dev-release/target cargo clippy -p warp --all-targets --tests -- -D warnings`; `./script/format`.
- [ ] **Step 6: Commit.**
```bash
git add app/src/agent_ctrl/fleet_model.rs app/src/agent_ctrl/fleet_model_tests.rs app/src/workspace/view_tests.rs
git commit -m "feat(fleet): pause and resume runs, and rank paused runs after live ones"
```

### Task C6: Fleet pane — Pause, Resume and the node name in each row

**Files:** Modify `app/src/agent_ctrl/fleet_view.rs` (module doc :1-2, `FleetViewAction` :46-53, `RowMouseStates` :59-64, `render_row` :354-458, `handle_action` :543-567, test module at the end after :611); create `app/src/agent_ctrl/fleet_view_tests.rs`; modify `app/src/workspace/view_tests.rs` (append after the fleet tests, ~:8240).

**Interfaces:**
- Consumes: `FleetRow::{can_pause, can_resume, is_paused, node_name}`, `FleetModel::{pause, resume}` (C5); `AgentCtrlFleetPane::fleet_view` (`app/src/pane_group/pane/agent_ctrl_fleet_pane.rs:38`).
- Produces:
```rust
// FleetViewAction +=
Pause { job_id: String },
Resume { job_id: String },
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PauseControl { Pause, Resume, Unavailable }
pub(crate) fn pause_control(row: &FleetRow) -> PauseControl;
```

- [ ] **Step 1: Write the failing tests.** `fleet_view_tests.rs`:
```rust
use super::*;

fn row(state: &str, interactive: bool) -> FleetRow {
    FleetRow {
        job_id: "job-1".to_owned(),
        state: state.to_owned(),
        agent: "staging-claude-interactive-rw".to_owned(),
        node_name: Some("agent-ctrl-node-0".to_owned()),
        interactive,
        branch: None,
        branch_url: None,
        last_activity: None,
    }
}

#[test]
fn running_rows_offer_pause_and_paused_rows_offer_resume() {
    assert_eq!(pause_control(&row("running", true)), PauseControl::Pause);
    assert_eq!(pause_control(&row("paused", true)), PauseControl::Resume);
    assert_eq!(pause_control(&row("running", false)), PauseControl::Unavailable);
    assert_eq!(pause_control(&row("waiting_input", true)), PauseControl::Unavailable);
    assert_eq!(pause_control(&row("completed", true)), PauseControl::Unavailable);
}
```
`view_tests.rs`:
```rust
static FLEET_PAUSED: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
static FLEET_RESUMED: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

fn pause_recording(job_id: &str) -> anyhow::Result<()> {
    FLEET_PAUSED.lock().unwrap().push(job_id.to_owned());
    Ok(())
}

fn resume_recording(job_id: &str) -> anyhow::Result<()> {
    FLEET_RESUMED.lock().unwrap().push(job_id.to_owned());
    Ok(())
}

#[test]
fn fleet_row_pause_and_resume_reach_agent_ctrl() {
    use crate::agent_ctrl::FleetPaneManager;
    use crate::agent_ctrl::fleet_model::{FleetBackend, FleetModel};
    use crate::agent_ctrl::fleet_view::FleetViewAction;
    use warpui::TypedActionView as _;

    App::test((), |mut app| async move {
        initialize_app_for_move_session(&mut app);
        app.add_singleton_model(|_| {
            FleetModel::with_backend(FleetBackend {
                list: list_no_runs,
                start: start_nothing,
                stop: stop_nothing,
                pause: pause_recording,
                resume: resume_recording,
            })
        });
        app.add_singleton_model(|_| FleetPaneManager::default());
        let workspace = mock_workspace(&mut app);
        workspace.update(&mut app, |workspace, ctx| {
            workspace.handle_action(&WorkspaceAction::OpenAgentCtrlFleetPane, ctx)
        });
        let [pane_id] = fleet_pane_ids(&workspace, &mut app)[..] else {
            panic!("expected one fleet pane");
        };
        let fleet_view = workspace.read(&app, |workspace, ctx| {
            workspace
                .active_tab_pane_group()
                .as_ref(ctx)
                .downcast_pane_by_id::<crate::pane_group::AgentCtrlFleetPane>(pane_id)
                .expect("the fleet pane")
                .fleet_view(ctx)
        });

        fleet_view.update(&mut app, |view, ctx| {
            view.handle_action(
                &FleetViewAction::Pause {
                    job_id: "job-live".to_owned(),
                },
                ctx,
            );
            view.handle_action(
                &FleetViewAction::Resume {
                    job_id: "job-paused".to_owned(),
                },
                ctx,
            );
        });

        assert_eventually_within!(
            std::time::Duration::from_secs(5) =>
                FLEET_PAUSED.lock().unwrap().as_slice() == ["job-live"]
                    && FLEET_RESUMED.lock().unwrap().as_slice() == ["job-paused"],
            "the row actions never reached AgentCtrl"
        );
    });
}
```

- [ ] **Step 2: Run to see it fail.**
`CARGO_TARGET_DIR=T:/projects/dev-ops/warp-dev-release/target cargo nextest run -p warp fleet_row_pause running_rows_offer`
Expected: compile errors (no variant `FleetViewAction::Pause`, no `pause_control`).

- [ ] **Step 3: Implement.** `fleet_view.rs`:
- module doc: `//! The fleet pane's view: AgentCtrl staging's runs, grouped by repository, with Open, Pause or` / `//! Resume, and Stop on each run and a Start… menu listing the repositories a run can start on.`
- `FleetViewAction` += `Pause { job_id: String },` `Resume { job_id: String },`
- `RowMouseStates` += `pause: MouseStateHandle,` `resume: MouseStateHandle,`
- after `STARTABLE_AGENTS`:
```rust
/// Which of Pause or Resume a row offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PauseControl {
    Pause,
    Resume,
    Unavailable,
}

pub(crate) fn pause_control(row: &FleetRow) -> PauseControl {
    if row.can_resume() {
        PauseControl::Resume
    } else if row.can_pause() {
        PauseControl::Pause
    } else {
        PauseControl::Unavailable
    }
}
```
- `render_row`: badge background
```rust
        let badge_background = if row.is_finished() {
            theme.surface_2()
        } else if row.is_paused() {
            theme.surface_3()
        } else {
            theme.accent()
        };
```
  node name, as the first child of the second details row (before `branch`):
```rust
                    .with_child(
                        Self::secondary_text(
                            row.node_name
                                .as_deref()
                                .map(|node| format!("on {node}"))
                                .unwrap_or_else(|| "no node yet".to_owned()),
                            appearance,
                        )
                        .finish(),
                    )
```
  the control, built before `stop` and added between `open` and `stop`:
```rust
        let pause_or_resume = match pause_control(row) {
            PauseControl::Pause => Self::button(
                "Pause",
                &mouse_states.pause,
                Some(FleetViewAction::Pause {
                    job_id: row.job_id.clone(),
                }),
                appearance,
            ),
            PauseControl::Resume => Self::button(
                "Resume",
                &mouse_states.resume,
                Some(FleetViewAction::Resume {
                    job_id: row.job_id.clone(),
                }),
                appearance,
            ),
            PauseControl::Unavailable => {
                Self::button("Pause", &mouse_states.pause, None, appearance)
            }
        };
```
  `.with_child(open).with_child(pause_or_resume).with_child(stop)`.
- `handle_action` arms:
```rust
            FleetViewAction::Pause { job_id } => {
                let job_id = job_id.clone();
                self.model.update(ctx, |model, ctx| model.pause(job_id, ctx));
            }
            FleetViewAction::Resume { job_id } => {
                let job_id = job_id.clone();
                self.model.update(ctx, |model, ctx| model.resume(job_id, ctx));
            }
```
- end of file:
```rust
#[cfg(test)]
#[path = "fleet_view_tests.rs"]
mod tests;
```

- [ ] **Step 4: Run to pass.** Step 2 command → pass; `CARGO_TARGET_DIR=T:/projects/dev-ops/warp-dev-release/target cargo nextest run -p warp fleet agent_ctrl` → pass.
- [ ] **Step 5: Lint and format.** `CARGO_TARGET_DIR=T:/projects/dev-ops/warp-dev-release/target cargo clippy -p warp --all-targets --tests -- -D warnings`; `./script/format`.
- [ ] **Step 6: Commit.**
```bash
git add app/src/agent_ctrl/fleet_view.rs app/src/agent_ctrl/fleet_view_tests.rs app/src/workspace/view_tests.rs
git commit -m "feat(pane): pause and resume fleet runs and show their node"
```

### Task C7: GUI terminal manager attaches a detached session with a repaint

`PaneGroup::initial_single_terminal_pane` (`app/src/pane_group/mod.rs:3577-3582`) calls `attach_detached_session` on the pane's manager. The GUI `TerminalManager<TerminalView>` only has the trait default (`app/src/terminal/terminal_manager.rs:76-82`), which restores the session without setting `repaint_screen_on_attach`, so a pane opened onto a detached session in the GUI shows nothing until the program redraws. The TUI wrapper (`app/src/terminal/local_tty/terminal_manager.rs:360-367`) already does it right.

**Files:** Modify `app/src/terminal/local_tty/terminal_manager.rs` (field :166-169 → `pub(super)`; `#[cfg(test)]` accessor next to `cli_agent_resume_command` :885-889), `app/src/terminal/local_tty/terminal_view_adaptor.rs` (after `restore_persistent_session` :2031-2045), `app/src/pane_group/mod_tests.rs` (after `test_restored_pane_hands_its_resume_command_to_the_terminal_manager`, ending :4813).

**Interfaces:**
- Consumes: `TerminalManagerTrait::restore_persistent_session`, `TerminalManagerTrait::persistent_session_id`.
- Produces:
```rust
// TerminalManager<S>
pub(super) repaint_screen_on_attach: bool,
#[cfg(test)] pub(crate) fn repaint_screen_on_attach(&self) -> bool;
// impl TerminalManagerTrait for TerminalManager<TerminalView>
fn attach_detached_session(&mut self, session_id: warp_core::SessionId, ctx: &mut ModelContext<Box<dyn TerminalManagerTrait>>);
```

- [ ] **Step 1: Write the failing test** (and its test-only accessor). `terminal_manager.rs`, after `cli_agent_resume_command`:
```rust
    /// Whether the next attach repaints the session's screen.
    #[cfg(test)]
    pub(crate) fn repaint_screen_on_attach(&self) -> bool {
        self.repaint_screen_on_attach
    }
```
`mod_tests.rs`:
```rust
/// A pane opened onto a detached supervisor session has no scrollback of its own, so the GUI
/// terminal manager must repaint the session's screen when it attaches, as the TUI one does.
#[test]
fn test_detached_session_attach_repaints_on_the_gui_terminal_manager() {
    let _flag = FeatureFlag::LazyShellStartup.override_enabled(true);
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let cwd = std::env::current_dir()
            .expect("the test process has a current directory")
            .to_string_lossy()
            .into_owned();
        let pane_group = mock_pane_group(
            &mut app,
            MockOptions {
                layout: PanesLayout::Snapshot {
                    snapshot: Box::new(terminal_leaf_snapshot(Some(cwd), None)),
                    shell_start: ShellStartPolicy::Deferred,
                },
                ..Default::default()
            },
        );
        let manager = app
            .read(|ctx| pane_group.read(ctx, |group, ctx| group.terminal_manager(0, ctx)))
            .expect("pane 0 has a terminal manager");
        let session_id = warp_core::SessionId::from(42_u64);

        manager.update(&mut app, |manager, ctx| {
            manager.attach_detached_session(session_id, ctx)
        });

        let (persistent_session_id, repaint) = app.read(|ctx| {
            manager.read(ctx, |manager, _| {
                let gui = manager
                    .as_any()
                    .downcast_ref::<TerminalManager<crate::terminal::TerminalView>>()
                    .expect("the pane runs on the GUI terminal manager");
                (manager.persistent_session_id(), gui.repaint_screen_on_attach())
            })
        });
        assert_eq!(persistent_session_id, Some(session_id));
        assert!(
            repaint,
            "attaching a detached session must repaint its screen on the GUI manager"
        );
    });
}
```

- [ ] **Step 2: Run to see it fail.**
`CARGO_TARGET_DIR=T:/projects/dev-ops/warp-dev-release/target cargo nextest run -p warp test_detached_session_attach_repaints test_restored_pane_hands_its_resume_command`
Expected: the new test FAILS on `attaching a detached session must repaint its screen` (the default only restores); the restore-resume test passes.

- [ ] **Step 3: Implement.** `terminal_manager.rs` field: `pub(super) repaint_screen_on_attach: bool,`. `terminal_view_adaptor.rs`, after `restore_persistent_session`:
```rust
    fn attach_detached_session(
        &mut self,
        session_id: warp_core::SessionId,
        ctx: &mut ModelContext<Box<dyn TerminalManagerTrait>>,
    ) {
        self.restore_persistent_session(session_id, ctx);
        self.repaint_screen_on_attach = true;
    }
```

- [ ] **Step 4: Run to pass.** Step 2 command → both pass; `CARGO_TARGET_DIR=T:/projects/dev-ops/warp-dev-release/target cargo nextest run -p warp pane_group terminal_manager attach_detached` → no new failures against the baseline.
- [ ] **Step 5: Lint and format.** `CARGO_TARGET_DIR=T:/projects/dev-ops/warp-dev-release/target cargo clippy -p warp --all-targets --tests -- -D warnings`; `./script/format`.
- [ ] **Step 6: Commit.**
```bash
git add app/src/terminal/local_tty/terminal_manager.rs app/src/terminal/local_tty/terminal_view_adaptor.rs app/src/pane_group/mod_tests.rs
git commit -m "fix(terminal): repaint a detached session the GUI terminal manager attaches to"
```

### Task C8: Presubmit, PR, install and manual check (needs the user; after Specs A and B are on staging)

- [ ] **Step 1: Whole-branch checks.** In `T:/projects/dev-ops/warp-agentctrl-pause`: `./script/format`; `CARGO_TARGET_DIR=T:/projects/dev-ops/warp-dev-release/target cargo clippy --workspace --all-targets --all-features --tests -- -D warnings`; `CARGO_TARGET_DIR=T:/projects/dev-ops/warp-dev-release/target cargo nextest run --no-fail-fast --workspace --exclude command-signatures-v2 --exclude integration` → only the ~22 baseline `workspace::view`/`StoznAuthManager` failures.
- [ ] **Step 2: Push.** `git push -u gitea feat/agentctrl-pause` and `git push gitlab feat/agentctrl-pause` (never `origin`).
- [ ] **Step 3: Open the PR** with the template at `.github/pull_request_template.md`:
```bash
tea pr create --login obachan-dev-budelius --repo stozn/warpspeed --head feat/agentctrl-pause --base dev \
  --title "feat: pause and resume AgentCtrl runs from WarpSpeed" \
  --description "$(cat <<'EOF'
## Description
Pause and resume AgentCtrl runs from the CLI (`agent-ctrl pause|resume <job>`), the fleet pane (Pause on running rows, Resume on paused rows, node name per row, paused rows between running and finished) and the `agent-ctrl open` viewer (paused notice and `resume` command). `agent-ctrl start --agent` accepts claude, pi or codex as well as a profile name. The GUI terminal manager now repaints a detached supervisor session it attaches to. Spec C of docs/superpowers/specs/2026-10-07-agentctrl-pause-and-second-node-design.md; needs Specs A and B on staging.

## Linked Issue
- [ ] The linked issue is labeled `ready-to-spec` or `ready-to-implement`.
- [ ] Where appropriate, screenshots or a short video of the implementation are included below (especially for user-visible or UI changes).

## Testing
Unit tests in managed_agent_sessions, warp_cli, the fleet model and view, workspace view tests, and pane_group (`test_detached_session_attach_repaints_on_the_gui_terminal_manager`). Manual check on staging after install (pause/resume from the CLI and fleet pane across both nodes).

- [ ] I have manually tested my changes locally with `./script/run`

## Agent Mode
- [ ] Warp Agent Mode - This PR was created via Warp's AI Agent Mode

CHANGELOG-IMPROVEMENT: Pause and resume AgentCtrl runs from the fleet pane and the CLI, see which node each run is on, and start runs with `--agent claude|pi|codex`.

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```
- [ ] **Step 4: Install after merge.** In `T:/projects/dev-ops/warp-dev-release`: `git pull gitea dev`, then `powershell -NoProfile -ExecutionPolicy Bypass -File ./script/windows/install-warpspeed.ps1`. Do not restart or kill running WarpSpeed windows; report that existing windows keep the old build until the user opens a new one.
- [ ] **Step 5: Manual check (new WarpSpeed window, staging with two nodes).**
  - `warpspeed agent-ctrl start --agent gemini --repo <kaizen url>` → fails at once with "unknown agent … claude, pi, codex …"; `--agent claude` starts a run.
  - Fleet pane: the run shows `on agent-ctrl-node-N`; Pause → row turns `paused`, sorts after running rows, offers Resume; Gitea branch shows the `agent-ctrl run <job> pause` commit.
  - Open the paused run → viewer prints `Paused — resume with: warpspeed agent-ctrl resume <job>`; type `resume` → run returns to running (possibly on the other node) and answers a question about its earlier turn.
  - `warpspeed agent-ctrl pause <job>` / `resume <job>` from a shell → same results; with every slot full, resume prints the at-capacity message.
  - Pause a run whose session id is unknown (if any) → "cannot be paused".
  - Attach a detached supervisor session in the GUI (agent sessions picker → a detached session) → the pane shows the session's screen immediately.
