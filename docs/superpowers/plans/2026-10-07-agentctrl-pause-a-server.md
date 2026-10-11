# AgentCtrl Pause and Second Node — Plan A (agent-ctrl)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Run more sessions than one node holds, and let the operator pause a session so it stops costing a slot and resume it later with its conversation intact, from the dashboard and from WarpSpeed.

**Architecture:** An agent's `node:` value becomes a pool prefix. Nodes report their interactive cap in the registration and their running interactive count in every health report, and the controller schedules `SpawnAgent`, `ImportSession` and `ResumeAgent` onto the connected pool node with the most free interactive slots. Pause is a `StopCommand` with `pause = true`: the node stops the agent gracefully, commits `agent-ctrl run <job> pause`, pushes the run branch, writes `.runs/<job>.paused`, lets the worktree lock go with the process, and reports `AgentCompleted { paused, session_id, session_file }`; the controller records `JobState::Paused` and keeps the session file in its import store (Mongo GridFS bucket `paused_sessions`, state-dir copy beside it). Resume is a `SpawnCommand` with `resume_paused = true` for the same job: the original node reuses its worktree, another node builds it from `origin/<branch>` and places the stored session file, and the backend starts with its resume flag. Staging moves the node to a two-replica StatefulSet with one workspace volume per pod.

**Tech Stack:** Rust (tonic 0.12/prost 0.13, tokio, mongodb 3.5.1 GridFS) in `stoz3n-agent-ctrl`; Python render/tests for k8s config; kubectl and `tea` for the rollout.

## Global Constraints

Copied verbatim from the spec (`2026-10-07-agentctrl-pause-and-second-node-design.md`, Spec A), then the coordinator rulings of 2026-10-07 and the decisions this plan makes where the spec is silent.

- An agent's `node:` value is a **pool prefix**: every connected node whose `node_name` starts with it is a candidate (`agent-ctrl-node` matches `agent-ctrl-node-0` and `agent-ctrl-node-1`; the single node of today, named exactly `agent-ctrl-node`, still matches).
- `NodeRegistration` gains `max_interactive_runs` (the node's `AGENT_CTRL_MAX_INTERACTIVE_RUNS`); `HeartbeatRequest` gains `running_interactive` (current count). The controller keeps both per node.
- `SpawnAgent`, `ImportSession` and `ResumeAgent` pick the connected candidate with the most free interactive slots (ties: lowest name). Only when no candidate has a free slot does the call fail with `RESOURCE_EXHAUSTED "node at capacity"` (the dashboard keeps mapping it to 409 `node_at_capacity`). The node-side cap check stays as the last line of defence; its NACK is surfaced unchanged.
- `req.node_name`, when given, must name a candidate in the pool (it replaces the "must equal the configured node" check).
- `StopAgent`, `SendInput`, `GetAgentStatus`, `TailAgentLogs` keep routing by the job's recorded `node_name`.
- `ListRepositories` returns the union over connected pool nodes: a repository is `reachable` only if every node that lists it reports it reachable; `error` names the failing node.
- New `JobState::Paused` (`"paused"`), non-terminal, excluded from every capacity count. Persisted like the other states (controller job record, Mongo read model, `GET /runs`).
- New RPCs `PauseAgent { job_id }` and `ResumeAgent { job_id, request_id }` on FleetControl, both operator mutations (same audit/actor/source handling as `StopAgent`), both idempotent: pausing a paused run and resuming a running run are no-ops that return the current state.
- **Pause** (controller → node holding the job): the node asks the agent process to exit cleanly (the platform `graceful_stop`; `force` after 60 s), waits for exit, then runs the same work as run end minus the removal: commit any dirty tree as `agent-ctrl run <job> pause`, push to the run's branch (`agent/<job>` or the handoff branch), write `.runs/<job>.paused`, release the worktree lock, report `Paused` with the agent's session id. A pause that cannot push keeps the worktree, reports `Paused` with `push_failed` in the status message, and the next resume on the same node still works from the local worktree.
- A paused run's worktree is never swept: the startup sweep skips `.runs/<job>` while `.runs/<job>.paused` exists, however old and however pushed.
- **Resume** (controller): scheduled like a new interactive run on the pool. The `SpawnCommand` carries `job_id` (same job), `resume_session_id`, the run's `repo_url` and `branch`, and whether the worktree is expected locally. On the original node: removes the marker, reacquires the worktree lock, spawns the backend in `.runs/<job>` with its resume flag (`claude --resume <id>`, `pi --session <id>`, `codex resume <id>`). On another node: creates `.runs/<job>` from `origin/<branch>` (the move path's worktree creation), places the session file the way `ImportSession` does (the pause step uploads the session file to the controller's import store so another node can fetch it), then spawns with the resume flag. The job returns to `Running` with the same job id; its `node_name` is updated.
- **Session ids** must be known for every interactive run, not only moved ones: Claude is spawned with `--session-id <uuid>` generated by the node; Pi and Codex report theirs through the existing status hooks. A run whose session id is unknown cannot be paused: `PauseAgent` fails with `FAILED_PRECONDITION "session id unknown"` and the dashboard disables Pause for it.
- Dashboard run view (`GET /runs`, `/runs/:id`): `state: paused`, `paused_at`, `node_name`.
- `node-deployment.tmpl.yaml` becomes a StatefulSet (`serviceName: agent-ctrl-node`, headless Service added) with `replicas = node_replicas` and a `volumeClaimTemplates` entry for the workspace (`shared_checkouts_storage_size`, `shared_checkouts_storage_class`). The pod's hostname (`agent-ctrl-node-0`, `-1`) is passed as `--node-name`. `workspace-sync` runs its fetch loop once per node volume (one CronJob per replica, rendered from `node_replicas`). `staging.toml`: `node_replicas = 2`, `max_interactive_runs = 8` (per node → 16 interactive slots). Memory limit per pod stays 12 GiB; the two pods must land on different hosts (`podAntiAffinity` on the app label, `requiredDuringScheduling`).
- **Rollout**: the existing Deployment and its PVC `agent-ctrl-workspace` are retired. Before switching, every live run is paused; after the StatefulSet is up, the runs are resumed and land on the new volumes. The old PVC is deleted one day later.

**Coordinator rulings (2026-10-07):**
- `PauseAgentRequest` and `ResumeAgentRequest` both carry `job_id`, `request_id`, `actor`, `source` and go through `mutation_context` exactly like `StopAgent` (audit). Responses carry `job_id`, `state`, `accepted`, `node_name` (plus `error`, `request_id`, `replayed`; `ResumeAgentResponse` also `agent_id`).
- `StopAgent` on a paused run works: the job becomes `Stopped`, and the node that holds the worktree removes `.runs/<job>.paused` and the worktree (nothing new to push).
- Every interactive run records its agent session id on the controller job right after spawn (Claude via the node-generated `--session-id`, which the node's first status update carries; Pi and Codex from their first status hook). The Mongo job's `node` field is updated on resume; `paused_at` is written to Mongo as a BSON date; `GET /runs` exposes `paused_at` and `node_name` (from `node`).
- The repositories union is computed by the controller; the dashboard passes it through.

**Plan decisions (spec gaps, see "Open questions" at the end):**
- Nodes send their heartbeat as a `NodeHealthReport` on the stream; `node.rs` never calls the `Heartbeat` RPC. `running_interactive` is added to both messages so the protos stay aligned, but the controller reads it only from `NodeHealthReport` (ruling 2026-10-07).
- The node-registry server's tonic decode limit is raised to `MAX_GRPC_MESSAGE_BYTES` = 41,943,040 bytes (40 MiB): the 32 MiB `MAX_SESSION_FILE_BYTES` session-file cap plus 8 MiB of envelope headroom, the same limit FleetControl and the node client already use (ruling 2026-10-07).
- **Pool rule everywhere** (ruling 2026-10-07): every place that matches a node by name — `SpawnAgent`, `ImportSession`, `ResumeAgent`, `TriggerAgent`, `route_trigger` / `route_trigger_with_prompt` (work-source dispatch), controller-startup recovery — treats a connected node whose name starts with the agent's `node:` value as a match, so `agent-ctrl-node-0/-1` satisfy `agent-ctrl-node`; the ticket worker's `obachan-worker` matches itself as before.
- A resume that lands on a node other than the one that paused the run sends the original node `StopCommand { discard_paused_job_id }` once the resume is Running, so the stale worktree and marker go (ruling 2026-10-07).
- A pause whose push failed is retried by the node every `AGENT_CTRL_PROGRESS_PUSH_SECS` while `.runs/<job>.paused` exists (also after a node restart); on success the node sends `AgentStatusUpdate { state: AGENT_STATE_PAUSED, push_failed: false }` and the controller clears `pause_push_failed`, lifting the node pin (ruling 2026-10-07).
- Pause rides on `StopCommand.pause`; discarding a paused worktree on stop rides on `StopCommand.discard_paused_job_id`. `StopAgentRequest` gains `job_id` so a paused run (no live agent) can be stopped by job.
- `AgentState` gains `AGENT_STATE_PAUSED = 7` so `JobInfo.live_state` is honest for a paused job.
- Controller import store: Mongo GridFS bucket `paused_sessions` (file name = job id; a session file is up to 32 MiB, above the 16 MiB BSON document limit) when Mongo is configured, plus `<state>/paused/<job_id>.session` always (the controller's state dir is an emptyDir, so Mongo is the durable copy). Deleted when the job becomes terminal.
- A pause whose push failed pins the resume to the node holding the worktree (`Job.pause_push_failed`); otherwise any pool node.
- A resume that fails before its agent starts returns the job to `Paused` (the node reports `paused` again); it never fails the job.
- One-shot dispatch (`TriggerAgent`, work source) picks the connected pool node with the fewest running agents, ties lowest name.
- The StatefulSet is opt-in per instance (`node_workload = "statefulset"`); prod keeps its Deployment (spec non-goal: prod AgentCtrl). The render support ships in PR 1; staging flips in PR 2, after every run is paused.
- Branches: `feat/pause-and-second-node` (PR 1) and `deploy/staging-node-statefulset` (PR 2), both off `origin/dev` in `stoz3n-agent-ctrl`; push to `origin` (git-dev.obachan.dev). Never `git add -A`.

---

## Part A — stoz3n-agent-ctrl

Worktree: `T:/projects/SOTZ3N/stoz3n-agent-ctrl-pause` on `feat/pause-and-second-node` (off `origin/dev` = 05f4b6f). Cargo: `CARGO_TARGET_DIR=T:/projects/SOTZ3N/stoz3n-agent-ctrl/target`. Full gate after every task: `cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt -- --check`, `python -m unittest discover -s deploy/k8s -p "test_*.py"`. Mongo-gated tests run when `AGENT_CTRL_TEST_MONGO_URI` is set (they print "skipping" otherwise).

Existing test layout followed here (inline `#[cfg(test)] mod tests` per file): `src/state/mod.rs` (`mod tests` :431), `src/remote/mod.rs` (`mod tests` :378), `src/remote/controller.rs` (`mod auth_tests` :2813, `mod import_session_tests` :3161 with `harness()` :3195, `acknowledge_spawn`/`answer_spawn` :3247-3287, `repo_spawn_request` :3535, `register_node` :3924), `src/remote/node.rs` (`mod tests` :1749 with `FakeBackend` :1804, `harness()` :2017, `plain_command` :2833, `fresh_command` :3163, `run_fresh` :3211), `src/workspace_handoff.rs` (`test_support::handoff_fixture` :570, `mod tests` :602, `commit_file` :744), `src/agent/claude.rs` (`mod tests` :222), `deploy/k8s/test_shared_workspaces.py` (`RenderTests` :60). The fleet-runs PRs (#29-#32, commits `5449921`..`2f8b879`) used one commit per task with this same test placement.

### Task A1: Proto additions

**Files:** Modify `proto/agentctrl/v1.proto` (FleetControl :23-38, `NodeRegistration` :66-77, `SpawnCommand` :85-115, `StopCommand` :130-134, `AgentStatusUpdate` :160-173, `AgentState` :175-183, `AgentCompleted` :205-214, `NodeHealthReport` :216-223, `StopAgentRequest` :268-275, `NodeInfo` :307-317, `HeartbeatRequest` :360-365, `JobInfo` :400-433, new messages after `ListRepositoriesResponse` :457-459); `src/remote/controller.rs` (`agent_state_label` :1301-1311, `list_nodes` literal :1669-1679, `stop_agent` literal :1901-1905, `job_info` :2525-2595, `run_controller` :2761, stubs after `get_job` :2426-2441); `src/remote/mod.rs` (`state_label` :31-41); `src/remote/node.rs` (`AgentCompleted` literals :1434, :1670; `StopCommand` test literals :2523, :2549).

Current max field numbers (none reused): `NodeRegistration` 10, `SpawnCommand` 22, `StopCommand` 3, `AgentStatusUpdate` 12, `AgentState` 6, `AgentCompleted` 8, `NodeHealthReport` 6, `StopAgentRequest` 6, `NodeInfo` 9, `HeartbeatRequest` 4, `JobInfo` 32.

**Interfaces — Produces:**
```proto
// FleetControl (after ListRepositories, :37)
rpc PauseAgent(PauseAgentRequest) returns (PauseAgentResponse);
rpc ResumeAgent(ResumeAgentRequest) returns (ResumeAgentResponse);

// NodeRegistration: next tag 11
// The node's AGENT_CTRL_MAX_INTERACTIVE_RUNS; 0 from a node built before it was reported.
uint32 max_interactive_runs = 11;

// SpawnCommand: next tag 23
// Resume a paused run of `job_id`: `session_id` is the session to resume, `run_repo_url` its
// repository, `resume_session_file` the controller's copy of the transcript (may be empty).
bool resume_paused = 23;
string resume_branch = 24;
// The controller expects `.runs/<job_id>` on this node (the run paused here).
bool resume_local = 25;

// StopCommand: next tag 4
// Pause instead of stop: keep the worktree for a later resume.
bool pause = 4;
// With an empty agent_id: discard this paused job's worktree and marker (the run is stopped).
string discard_paused_job_id = 5;

// AgentStatusUpdate: next tag 13
// With state AGENT_STATE_PAUSED: the paused run's push state; false once a retried push landed.
bool push_failed = 13;

// AgentState: next value 7
AGENT_STATE_PAUSED = 7;

// AgentCompleted: next tag 9
bool paused = 9;
string session_id = 10;
// The paused session's transcript, for a resume on another node. Empty when none was found.
bytes session_file = 11;
bool push_failed = 12;

// NodeHealthReport: next tag 7
uint32 running_interactive = 7;

// StopAgentRequest: next tag 7
// Names a paused run, which has no live agent.
string job_id = 7;

// NodeInfo: next tag 10
uint32 max_interactive_runs = 10;
uint32 running_interactive = 11;

// HeartbeatRequest: next tag 5
// Mirrors NodeHealthReport.running_interactive; the controller reads the health report.
uint32 running_interactive = 5;

// JobInfo: next tag 33
google.protobuf.Timestamp paused_at = 33;
bool pause_push_failed = 34;

message PauseAgentRequest {
  string job_id = 1;
  string request_id = 2;
  string actor = 3;
  string source = 4;
}

message PauseAgentResponse {
  string job_id = 1;
  string state = 2;
  bool accepted = 3;
  string node_name = 4;
  string error = 5;
  string request_id = 6;
  bool replayed = 7;
}

message ResumeAgentRequest {
  string job_id = 1;
  string request_id = 2;
  string actor = 3;
  string source = 4;
}

message ResumeAgentResponse {
  string job_id = 1;
  string state = 2;
  bool accepted = 3;
  string node_name = 4;
  string error = 5;
  string request_id = 6;
  bool replayed = 7;
  string agent_id = 8;
}
```

- [ ] **Step 1: failing test** (append to `mod tests` in `src/remote/mod.rs`):
```rust
    #[test]
    fn paused_agent_state_has_a_label() {
        assert_eq!(pb::AgentState::Paused as i32, 7);
        assert_eq!(state_label(pb::AgentState::Paused as i32), "paused");
    }
```
- [ ] **Step 2:** `cargo test paused_agent_state_has_a_label` → FAIL (`no variant named Paused`).
- [ ] **Step 3: implement.** Add the proto text above, then fix the build:
  - `src/remote/mod.rs` `state_label`: `pb::AgentState::Paused => "paused",` before `Unknown`.
  - `controller.rs` `agent_state_label`: `pb::AgentState::Paused => "paused",`.
  - `controller.rs` `list_nodes` literal: `max_interactive_runs: 0, running_interactive: 0,` (filled in A4).
  - `controller.rs` `job_info`: `paused_at: None, pause_push_failed: false,` (filled in A2).
  - `controller.rs` `stop_agent` `pb::StopCommand` literal and node tests :2523, :2549: add `..Default::default()`.
  - `node.rs` both `pb::AgentCompleted { .. }` literals: `..Default::default()` after `last_event_sequence`.
  - `node.rs` `pb::AgentStatusUpdate` literals in `registration_snapshot` (:362, :400) and `monitor_agent_status` (:1553); controller tests build theirs with `..Default::default()`: `push_failed: false,` after `interactive`.
  - `run_controller` :2761: `let node_svc = NodeRegistryServer::new(controller.clone()).max_decoding_message_size(super::MAX_GRPC_MESSAGE_BYTES);` — 41,943,040 bytes (40 MiB): the 32 MiB `MAX_SESSION_FILE_BYTES` cap plus 8 MiB envelope headroom (`src/remote/mod.rs` :25-28), up from tonic's 4 MiB default, which would reject a paused `AgentCompleted` carrying a session file.
  - Stubs in `impl FleetControl for Controller` (replaced in A10/A11):
```rust
    async fn pause_agent(
        &self,
        _request: Request<pb::PauseAgentRequest>,
    ) -> std::result::Result<Response<pb::PauseAgentResponse>, Status> {
        Err(Status::unimplemented("pause_agent"))
    }

    async fn resume_agent(
        &self,
        _request: Request<pb::ResumeAgentRequest>,
    ) -> std::result::Result<Response<pb::ResumeAgentResponse>, Status> {
        Err(Status::unimplemented("resume_agent"))
    }
```
- [ ] **Step 4:** `cargo test` green; clippy; fmt.
- [ ] **Step 5: commit**
```bash
git add proto/agentctrl/v1.proto src/remote/mod.rs src/remote/controller.rs src/remote/node.rs
git commit -m "feat(proto): pause/resume RPCs, node capacity reporting, paused completion"
```

### Task A2: `JobState::Paused`, paused job fields, reconciliation skips paused jobs

**Files:** Modify `src/state/mod.rs` (`JobState` :24-62, `Job` :65-141, `Job::new` :143-185, tests :431); `src/mongo/mod.rs` (`mod mongo_optional_datetime` :81 → `pub(crate)`, `nonterminal_jobs_for_node` :1036-1052, `upsert_job` :1059-1077, tests :1250); `src/remote/controller.rs` (`open_stream` filter :1359-1368, grace loop :1530-1533, `job_info` :2525-2595, startup reconciliation :2644-2675).

**Interfaces — Produces:**
```rust
// state/mod.rs
pub enum JobState { Pending, Running, Completed, WaitingInput, Failed, Stopped, Interrupted, Paused }
// JobState::Paused.as_str() == "paused"; is_terminal() == false
pub struct Job {
    // … existing fields …
    /// When the run was last paused; `None` while it runs.
    #[serde(default, deserialize_with = "crate::mongo::mongo_optional_datetime::deserialize")]
    pub paused_at: Option<DateTime<Utc>>,
    /// The pause could not push the run's work, so only the node holding its worktree resumes it.
    #[serde(default)]
    pub pause_push_failed: bool,
    /// The controller holds the paused session's transcript for a resume on another node.
    #[serde(default)]
    pub paused_session_stored: bool,
}
// controller.rs
/// Whether `node_name` must report `job` when it registers: a live job it runs. A paused job has
/// no process to reconcile.
fn awaits_reconciliation(job: &Job, node_name: &str) -> bool;
```

- [ ] **Step 1: failing tests**
```rust
// src/state/mod.rs, mod tests
    #[test]
    fn paused_is_live_and_round_trips_with_its_pause_time() {
        let tmp = tempfile::tempdir().unwrap();
        let sd = StateDir::resolve(Some(tmp.path()), None);
        let mut job = Job::new("runner", "claude");
        job.state = JobState::Paused;
        job.paused_at = Some(Utc::now());
        job.pause_push_failed = true;
        sd.save_job(&job).unwrap();
        let loaded = sd.load_job(&job.job_id).unwrap();
        assert_eq!(loaded.state, JobState::Paused);
        assert_eq!(loaded.state.as_str(), "paused");
        assert!(!loaded.state.is_terminal());
        assert_eq!(
            loaded.paused_at.map(|t| t.timestamp_millis()),
            job.paused_at.map(|t| t.timestamp_millis())
        );
        assert!(loaded.pause_push_failed);
        let legacy: Job = serde_json::from_str(&format!(
            r#"{{"job_id":"j","agent_name":"a","backend":"claude","state":"running","created_at":"{}"}}"#,
            Utc::now().to_rfc3339()
        ))
        .unwrap();
        assert!(legacy.paused_at.is_none() && !legacy.paused_session_stored);
    }

// src/remote/controller.rs, mod import_session_tests
    #[test]
    fn paused_jobs_are_not_awaited_on_reconnect() {
        let mut job = crate::state::Job::new("repo-runner", "claude");
        job.node = Some("worker".into());
        job.state = crate::state::JobState::Running;
        assert!(super::awaits_reconciliation(&job, "worker"));
        assert!(!super::awaits_reconciliation(&job, "worker-2"));
        job.state = crate::state::JobState::Paused;
        assert!(!super::awaits_reconciliation(&job, "worker"));
        job.state = crate::state::JobState::Stopped;
        assert!(!super::awaits_reconciliation(&job, "worker"));
    }

    #[test]
    fn paused_job_info_reports_state_live_state_and_pause_time() {
        let mut job = crate::state::Job::new("repo-runner", "claude");
        job.state = crate::state::JobState::Paused;
        job.live_state = Some("paused".into());
        let at = chrono::Utc::now();
        job.paused_at = Some(at);
        job.pause_push_failed = true;
        let info = super::job_info(&job);
        assert_eq!(info.state, "paused");
        assert_eq!(info.live_state, pb::AgentState::Paused as i32);
        assert_eq!(info.paused_at.unwrap().seconds, at.timestamp());
        assert!(info.pause_push_failed);
    }

// src/mongo/mod.rs, mod tests (Mongo-gated)
    #[tokio::test]
    async fn paused_jobs_store_paused_at_as_a_date_and_skip_reconciliation() {
        let Ok(uri) = std::env::var("AGENT_CTRL_TEST_MONGO_URI") else {
            eprintln!("skipping Mongo paused-job test; AGENT_CTRL_TEST_MONGO_URI is unset");
            return;
        };
        let database = format!("agent_ctrl_pause_test_{}", uuid::Uuid::new_v4().simple());
        let store = MongoStore::connect(&uri, &database, &MongoTlsSettings::default())
            .await
            .unwrap();
        let mut job = Job::new("runner", "claude");
        job.fleet_id = "fleet".into();
        job.node = Some("agent-ctrl-node-0".into());
        job.state = crate::state::JobState::Paused;
        job.paused_at = Some(Utc::now());
        store.upsert_job(&job).await.unwrap();
        let raw = mongodb::Client::with_uri_str(&uri)
            .await
            .unwrap()
            .database(&database)
            .collection::<Document>("jobs")
            .find_one(doc! { "job_id": &job.job_id })
            .await
            .unwrap()
            .unwrap();
        assert!(raw.get_datetime("paused_at").is_ok(), "{raw:?}");
        let loaded = store.get_job(&job.job_id).await.unwrap().unwrap();
        assert_eq!(
            loaded.paused_at.map(|t| t.timestamp_millis()),
            job.paused_at.map(|t| t.timestamp_millis())
        );
        assert!(store
            .nonterminal_jobs_for_node("fleet", "agent-ctrl-node-0")
            .await
            .unwrap()
            .is_empty());
    }
```
- [ ] **Step 2:** `cargo test paused` → FAIL (no `Paused`, no fields, no `awaits_reconciliation`).
- [ ] **Step 3: implement.**
  - `JobState`: add `Paused` (`rename_all = "lowercase"` gives `"paused"`); `as_str`: `JobState::Paused => "paused"`; `is_terminal` list unchanged, with the doc line "Paused is live: its worktree and session wait for a resume."
  - `Job`: the three fields above; `Job::new`: `paused_at: None, pause_push_failed: false, paused_session_stored: false`.
  - `mongo/mod.rs`: `pub(crate) mod mongo_optional_datetime` (its deserializer already accepts both the RFC 3339 string the state dir writes and a BSON date, and `Bson::Null`). In `upsert_job` after `fields.remove("last_activity_at");` (`use mongodb::bson::Bson;`):
```rust
        // A Mongo date, so the dashboard can sort and filter paused runs by it.
        fields.insert(
            "paused_at",
            job.paused_at
                .map(|at| Bson::DateTime(mongodb::bson::DateTime::from_millis(at.timestamp_millis())))
                .unwrap_or(Bson::Null),
        );
```
    `nonterminal_jobs_for_node`: `$nin` gains `"paused"`; doc gains "Paused jobs have no process to reconcile."
  - `controller.rs`:
```rust
fn awaits_reconciliation(job: &Job, node_name: &str) -> bool {
    job.node.as_deref() == Some(node_name)
        && !job.state.is_terminal()
        && job.state != JobState::Paused
}
```
    Use it in `open_stream` (:1363-1366 → `.filter(|job| awaits_reconciliation(job, &node_name))`) and in the startup task (:2649-2652, `node_name.as_str()`). In the grace loop (:1531 `if job.state.is_terminal()`) and the startup loop (:2673 `if current.state.is_terminal()`) also `continue` on `JobState::Paused` (a job paused during the grace window is not interrupted).
  - `job_info`: `live_state` gains `Some("paused") => pb::AgentState::Paused as i32,` and the fallback arm `JobState::Paused => pb::AgentState::Paused as i32,`; `paused_at: j.paused_at.map(super::to_pb_timestamp), pause_push_failed: j.pause_push_failed,`.
- [ ] **Step 4:** green (Mongo test with a throwaway instance if available); clippy; fmt.
- [ ] **Step 5: commit**
```bash
git add src/state/mod.rs src/mongo/mod.rs src/remote/controller.rs
git commit -m "feat(state): paused job state, pause fields, reconciliation skips paused jobs"
```

### Task A3: Controller import store for paused sessions

**Files:** Modify `src/state/mod.rs` (`impl StateDir` :220-300), `src/mongo/mod.rs` (beside the collection accessors :598-621, methods after `upsert_job`), `src/remote/controller.rs` (methods after `load_durable_job` :519-537).

**Interfaces — Produces:**
```rust
// state/mod.rs
impl StateDir {
    /// `<root>/paused/<job_id>.session`.
    pub fn paused_session_path(&self, job_id: &str) -> PathBuf;
    pub fn save_paused_session(&self, job_id: &str, file: &[u8]) -> std::io::Result<()>;
    /// `None` when no copy exists.
    pub fn load_paused_session(&self, job_id: &str) -> std::io::Result<Option<Vec<u8>>>;
    /// A missing copy is not an error.
    pub fn remove_paused_session(&self, job_id: &str) -> std::io::Result<()>;
}
// mongo/mod.rs
impl MongoStore {
    /// Replaces the paused session stored for `job_id` (GridFS bucket `paused_sessions`).
    pub async fn put_paused_session(&self, job_id: &str, file: &[u8]) -> Result<()>;
    pub async fn get_paused_session(&self, job_id: &str) -> Result<Option<Vec<u8>>>;
    pub async fn delete_paused_session(&self, job_id: &str) -> Result<()>;
}
// controller.rs
impl ControllerInner {
    /// Keeps a paused session's transcript; returns whether a durable (Mongo) copy exists.
    async fn store_paused_session(&self, job_id: &str, file: &[u8]) -> bool;
    /// The stored transcript, empty when there is none.
    async fn load_paused_session(&self, job_id: &str) -> Vec<u8>;
    async fn forget_paused_session(&self, job_id: &str);
}
```

- [ ] **Step 1: failing tests**
```rust
// src/state/mod.rs, mod tests
    #[test]
    fn paused_sessions_are_saved_replaced_and_removed() {
        let tmp = tempfile::tempdir().unwrap();
        let sd = StateDir::resolve(Some(tmp.path()), None);
        assert_eq!(sd.load_paused_session("job-1").unwrap(), None);
        sd.save_paused_session("job-1", b"first").unwrap();
        sd.save_paused_session("job-1", b"second").unwrap();
        assert_eq!(sd.load_paused_session("job-1").unwrap().as_deref(), Some(&b"second"[..]));
        assert_eq!(
            sd.paused_session_path("job-1"),
            tmp.path().join("paused").join("job-1.session")
        );
        sd.remove_paused_session("job-1").unwrap();
        sd.remove_paused_session("job-1").unwrap();
        assert_eq!(sd.load_paused_session("job-1").unwrap(), None);
    }

// src/mongo/mod.rs, mod tests (Mongo-gated)
    #[tokio::test]
    async fn paused_sessions_live_in_gridfs_and_are_replaced() {
        let Ok(uri) = std::env::var("AGENT_CTRL_TEST_MONGO_URI") else {
            eprintln!("skipping Mongo paused-session test; AGENT_CTRL_TEST_MONGO_URI is unset");
            return;
        };
        let database = format!("agent_ctrl_pause_test_{}", uuid::Uuid::new_v4().simple());
        let store = MongoStore::connect(&uri, &database, &MongoTlsSettings::default())
            .await
            .unwrap();
        assert_eq!(store.get_paused_session("job-1").await.unwrap(), None);
        let big = vec![7_u8; 20 * 1024 * 1024]; // above the 16 MiB document limit
        store.put_paused_session("job-1", &big).await.unwrap();
        store.put_paused_session("job-1", b"newer").await.unwrap();
        assert_eq!(
            store.get_paused_session("job-1").await.unwrap().as_deref(),
            Some(&b"newer"[..])
        );
        store.delete_paused_session("job-1").await.unwrap();
        store.delete_paused_session("job-1").await.unwrap();
        assert_eq!(store.get_paused_session("job-1").await.unwrap(), None);
    }

// src/remote/controller.rs, mod import_session_tests
    #[tokio::test]
    async fn paused_sessions_round_trip_through_the_state_dir_without_mongo() {
        let Harness { controller, _tmp, .. } = harness(None);
        assert!(!controller.store_paused_session("job-1", b"transcript").await);
        assert_eq!(controller.load_paused_session("job-1").await, b"transcript");
        controller.forget_paused_session("job-1").await;
        assert!(controller.load_paused_session("job-1").await.is_empty());
    }
```
- [ ] **Step 2:** `cargo test paused_sessions` → FAIL.
- [ ] **Step 3: implement.**
```rust
// state/mod.rs (job ids reaching here come from durable jobs the controller loaded)
    pub fn paused_session_path(&self, job_id: &str) -> PathBuf {
        self.root.join("paused").join(format!("{job_id}.session"))
    }

    pub fn save_paused_session(&self, job_id: &str, file: &[u8]) -> std::io::Result<()> {
        let path = self.paused_session_path(job_id);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        atomic_write(&path, file)
    }

    pub fn load_paused_session(&self, job_id: &str) -> std::io::Result<Option<Vec<u8>>> {
        match std::fs::read(self.paused_session_path(job_id)) {
            Ok(file) => Ok(Some(file)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    pub fn remove_paused_session(&self, job_id: &str) -> std::io::Result<()> {
        match std::fs::remove_file(self.paused_session_path(job_id)) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        }
    }
```
```rust
// mongo/mod.rs
// use futures::io::{AsyncReadExt, AsyncWriteExt};
// use mongodb::gridfs::GridFsBucket;
// use mongodb::options::GridFsBucketOptions;
const PAUSED_SESSIONS_BUCKET: &str = "paused_sessions";

    fn paused_sessions(&self) -> GridFsBucket {
        self.db.gridfs_bucket(
            GridFsBucketOptions::builder()
                .bucket_name(PAUSED_SESSIONS_BUCKET.to_string())
                .build(),
        )
    }

    pub async fn put_paused_session(&self, job_id: &str, file: &[u8]) -> Result<()> {
        self.delete_paused_session(job_id).await?;
        let mut upload = self.paused_sessions().open_upload_stream(job_id).await?;
        upload.write_all(file).await?;
        upload.close().await?;
        Ok(())
    }

    pub async fn get_paused_session(&self, job_id: &str) -> Result<Option<Vec<u8>>> {
        let bucket = self.paused_sessions();
        if bucket.find_one(doc! { "filename": job_id }).await?.is_none() {
            return Ok(None);
        }
        let mut download = bucket.open_download_stream_by_name(job_id).await?;
        let mut file = Vec::new();
        download.read_to_end(&mut file).await?;
        Ok(Some(file))
    }

    pub async fn delete_paused_session(&self, job_id: &str) -> Result<()> {
        let bucket = self.paused_sessions();
        let files: Vec<_> = bucket
            .find(doc! { "filename": job_id })
            .await?
            .try_collect()
            .await?;
        for file in files {
            bucket.delete(file.id).await?;
        }
        Ok(())
    }
```
  (`?` on the `std::io::Error` from the stream helpers needs `From<std::io::Error> for AgentCtrlError` in `src/error.rs` (the Mongo conversion is at :35); add it if absent, mapping to `AgentCtrlError::Other(error.to_string())`.)
```rust
// controller.rs, impl ControllerInner
    async fn store_paused_session(&self, job_id: &str, file: &[u8]) -> bool {
        if let Err(error) = self.fleet.state().save_paused_session(job_id, file) {
            tracing::warn!(%job_id, %error, "failed to keep the paused session in the state dir");
        }
        let Some(mongo) = self.fleet.mongo() else {
            return false;
        };
        match mongo.put_paused_session(job_id, file).await {
            Ok(()) => true,
            Err(error) => {
                tracing::warn!(%job_id, %error, "failed to store the paused session in MongoDB");
                false
            }
        }
    }

    async fn load_paused_session(&self, job_id: &str) -> Vec<u8> {
        if let Ok(Some(file)) = self.fleet.state().load_paused_session(job_id) {
            return file;
        }
        let Some(mongo) = self.fleet.mongo() else {
            return Vec::new();
        };
        match mongo.get_paused_session(job_id).await {
            Ok(file) => file.unwrap_or_default(),
            Err(error) => {
                tracing::warn!(%job_id, %error, "failed to load the paused session from MongoDB");
                Vec::new()
            }
        }
    }

    async fn forget_paused_session(&self, job_id: &str) {
        if let Err(error) = self.fleet.state().remove_paused_session(job_id) {
            tracing::warn!(%job_id, %error, "failed to remove the paused session from the state dir");
        }
        if let Some(mongo) = self.fleet.mongo() {
            if let Err(error) = mongo.delete_paused_session(job_id).await {
                tracing::warn!(%job_id, %error, "failed to delete the paused session from MongoDB");
            }
        }
    }
```
- [ ] **Step 4:** green; clippy; fmt.
- [ ] **Step 5: commit**
```bash
git add src/state/mod.rs src/mongo/mod.rs src/remote/controller.rs src/error.rs
git commit -m "feat(controller): import store for paused session transcripts"
```

### Task A4: Nodes report their interactive cap and running count

**Files:** Modify `src/remote/node.rs` (`health_report` :137-146, `registration_snapshot` return :426-430, test `health_report_contains_liveness_time_and_running_count` :3692); `src/remote/controller.rs` (`NodeHandle` :281-286 and every literal :1407, :2848, :2986, :3213, :3932; `HealthReport` arm :932-958; `list_nodes` :1661-1682).

**Interfaces — Produces:**
```rust
// controller.rs
struct NodeHandle {
    registration: pb::NodeRegistration,
    tx: mpsc::Sender<CtrlMsgResult>,
    running_agents: AtomicU32,
    /// Interactive runs the node last reported.
    running_interactive: AtomicU32,
    /// Interactive dispatches chosen for this node and not yet answered.
    reserved_interactive: AtomicU32,
    health: Mutex<Option<pb::NodeHealthReport>>,
}
impl NodeHandle {
    fn new(registration: pb::NodeRegistration, tx: mpsc::Sender<CtrlMsgResult>) -> Self;
}
```

- [ ] **Step 1: failing tests**
```rust
// node.rs: extend health_report_contains_liveness_time_and_running_count — after the existing
// interactive "agent-1", insert a one-shot "agent-2" (interactive: false) and assert
        let Some(NodePayload::HealthReport(report)) = runtime.health_report().payload else {
            panic!("expected a health report")
        };
        assert_eq!(report.running_agent_count, 2);
        assert_eq!(report.running_interactive, 1);

// node.rs, new
    #[tokio::test]
    async fn registration_reports_the_interactive_cap() {
        let h = harness().await;
        let registration = registration_snapshot(&h.runner, &h.runtime).await;
        assert_eq!(registration.max_interactive_runs, max_interactive_runs() as u32);
    }

// controller.rs, mod import_session_tests
    #[tokio::test]
    async fn the_health_report_is_the_running_interactive_count() {
        let Harness { controller, _tmp, .. } = harness(None);
        controller
            .handle_node_message(
                "worker",
                pb::NodeMessage {
                    payload: Some(super::NodePayload::HealthReport(pb::NodeHealthReport {
                        running_interactive: 3,
                        ..Default::default()
                    })),
                },
            )
            .await;
        let handle = controller.nodes.get("worker").unwrap().clone();
        assert_eq!(handle.running_interactive.load(std::sync::atomic::Ordering::SeqCst), 3);
        let nodes = FleetControl::list_nodes(&controller, tonic::Request::new(pb::ListNodesRequest {}))
            .await
            .unwrap()
            .into_inner()
            .nodes;
        assert_eq!(nodes[0].running_interactive, 3);
    }
```
- [ ] **Step 2:** run → FAIL.
- [ ] **Step 3: implement.**
  - `node.rs` `health_report`: `running_interactive: interactive_run_count(self) as u32,`. `registration_snapshot` return: `max_interactive_runs: max_interactive_runs() as u32,` (the `run_node` literal keeps `..registration_snapshot(..)`).
  - `controller.rs`:
```rust
impl NodeHandle {
    fn new(registration: pb::NodeRegistration, tx: mpsc::Sender<CtrlMsgResult>) -> Self {
        Self {
            registration,
            tx,
            running_agents: AtomicU32::new(0),
            running_interactive: AtomicU32::new(0),
            reserved_interactive: AtomicU32::new(0),
            health: Mutex::new(None),
        }
    }
}
```
    Replace every `NodeHandle { .. }` literal with `NodeHandle::new(registration, tx)`. `HealthReport` arm, first line inside `if let Some(handle)`: `handle.running_interactive.store(h.running_interactive, Ordering::SeqCst);`. The `heartbeat` handler is unchanged: `HeartbeatRequest.running_interactive` exists only to keep the protos aligned, and nodes do not call the RPC. `list_nodes`: `max_interactive_runs: r.max_interactive_runs, running_interactive: h.running_interactive.load(Ordering::SeqCst),`.
- [ ] **Step 4:** green; clippy; fmt.
- [ ] **Step 5: commit**
```bash
git add src/remote/node.rs src/remote/controller.rs
git commit -m "feat(node): report the interactive cap and running interactive count"
```

### Task A5: Pool scheduling and the repositories union

**Files:** Modify `src/remote/controller.rs`: `ControllerInner` :301-315 and `new` :318-335 (`schedule_lock`), free fns after `replayed_launch` :226-248, methods after `node_connected` :483-485, `route_trigger` :758-790, `route_trigger_with_prompt` :792-831, `spawn_agent` :1764-1851, `trigger_agent` :2174-2216, `list_repositories` :2294-2307, `import_session` :2361-2424, `merge_repositories` :2490-2509, startup recovery task :2627-2693; `src/mongo/mod.rs` (new `nonterminal_jobs` after `nonterminal_jobs_for_node` :1036-1052); tests in `mod import_session_tests` (rewrite assertions of `list_repositories_merges_node_registrations` :3979-4004).

**Interfaces — Consumes:** `NodeHandle::new`, `running_interactive`, `reserved_interactive` (A4).

**Produces:**
```rust
/// Interactive runs a node takes when it did not report its cap (the node's own default).
const DEFAULT_NODE_INTERACTIVE_CAP: u32 = 4;
/// Whether `node_name` belongs to the pool an agent's `node:` value names.
fn in_pool(node_name: &str, pool: &str) -> bool;
/// The candidate with the most free slots; ties go to the lowest name. `None` when none is free.
fn most_free(candidates: &[(String, u32)]) -> Option<&str>;
/// The candidate running the fewest agents; ties go to the lowest name.
fn least_loaded(candidates: &[(String, u32)]) -> Option<&str>;
/// Holds one interactive slot on a node from scheduling until the dispatch has been answered.
struct SlotReservation(Arc<NodeHandle>);
impl ControllerInner {
    fn free_interactive_slots(&self, handle: &NodeHandle) -> u32;
    async fn schedule(&self, pool: &str, pinned: Option<&str>, interactive: bool)
        -> std::result::Result<(String, Option<SlotReservation>), String>;
}
fn merge_repositories<'a>(reported: impl Iterator<Item = (&'a str, &'a pb::RepositoryInfo)>) -> Vec<pb::RepositoryInfo>;
/// The live jobs controller-startup recovery interrupts: on a node of one of `pools` (pool rule),
/// not paused, and recorded on a node for which `seen` is false (it never connected).
fn unreconciled_at_startup(jobs: Vec<Job>, pools: &HashSet<String>, seen: impl Fn(&str) -> bool) -> Vec<Job>;
// mongo/mod.rs
/// Every live, non-paused job of the fleet, whatever its node.
pub async fn nonterminal_jobs(&self, fleet_id: &str) -> Result<Vec<Job>>;
```

- [ ] **Step 1: failing tests** (in `mod import_session_tests`)
```rust
    /// Registers `node_name` with an interactive `cap` and a live channel the test answers.
    fn register_pool_node(
        controller: &super::Controller,
        node_name: &str,
        cap: u32,
    ) -> mpsc::Receiver<CtrlMsgResult> {
        let (tx, rx) = mpsc::channel::<CtrlMsgResult>(32);
        controller.nodes.insert(
            node_name.to_string(),
            Arc::new(NodeHandle::new(
                pb::NodeRegistration {
                    node_id: format!("{node_name}-id"),
                    node_name: node_name.to_string(),
                    max_interactive_runs: cap,
                    ..Default::default()
                },
                tx,
            )),
        );
        rx
    }

    /// Accepts every spawn on `rx` under a fresh agent id until the channel closes.
    fn accept_spawns(controller: super::Controller, mut rx: mpsc::Receiver<CtrlMsgResult>) {
        tokio::spawn(async move {
            while let Some(Ok(message)) = rx.recv().await {
                if let Some(CtrlPayload::Spawn(command)) = message.payload {
                    if let Some((_, waiter)) = controller.pending.remove(&command.command_id) {
                        let _ = waiter.send(pb::CommandAck {
                            command_id: command.command_id.clone(),
                            accepted: true,
                            agent_id: format!("agent-{}", command.command_id),
                            error: String::new(),
                        });
                    }
                }
            }
        });
    }

    fn repo_request(n: usize) -> pb::SpawnAgentRequest {
        pb::SpawnAgentRequest {
            request_id: format!("pool-request-{n}"),
            ..repo_spawn_request("repo-runner")
        }
    }

    fn runs_on(controller: &super::Controller, node: &str) -> usize {
        controller.agents.iter().filter(|a| a.node_name == node).count()
    }

    #[tokio::test]
    async fn interactive_runs_spread_over_the_pool_until_it_is_full() {
        let Harness { controller, _tmp, .. } = harness(None);
        controller.nodes.remove("worker");
        accept_spawns(controller.clone(), register_pool_node(&controller, "worker-0", 2));
        accept_spawns(controller.clone(), register_pool_node(&controller, "worker-1", 2));
        for n in 0..4 {
            let response =
                FleetControl::spawn_agent(&controller, tonic::Request::new(repo_request(n)))
                    .await
                    .expect("a pool node has a free slot")
                    .into_inner();
            assert!(response.success, "{}", response.error);
        }
        assert_eq!(runs_on(&controller, "worker-0"), 2);
        assert_eq!(runs_on(&controller, "worker-1"), 2);
        let status = FleetControl::spawn_agent(&controller, tonic::Request::new(repo_request(4)))
            .await
            .expect_err("the pool is full");
        assert_eq!(status.code(), tonic::Code::ResourceExhausted);
        assert_eq!(status.message(), "node at capacity");
    }

    #[test]
    fn the_node_with_more_free_slots_wins_and_ties_go_to_the_lowest_name() {
        assert_eq!(super::most_free(&[("b".into(), 2), ("a".into(), 2)]), Some("a"));
        assert_eq!(super::most_free(&[("a".into(), 1), ("b".into(), 3)]), Some("b"));
        assert_eq!(super::most_free(&[("a".into(), 0)]), None);
        assert_eq!(super::least_loaded(&[("b".into(), 0), ("a".into(), 1)]), Some("b"));
    }

    #[tokio::test]
    async fn a_requested_node_must_be_in_the_pool() {
        let Harness { controller, _tmp, .. } = harness(None);
        accept_spawns(controller.clone(), register_pool_node(&controller, "worker-1", 2));
        let outside = FleetControl::spawn_agent(
            &controller,
            tonic::Request::new(pb::SpawnAgentRequest {
                node_name: "other-node".into(),
                ..repo_request(0)
            }),
        )
        .await
        .expect_err("outside the pool");
        assert_eq!(outside.code(), tonic::Code::PermissionDenied);
        let response = FleetControl::spawn_agent(
            &controller,
            tonic::Request::new(pb::SpawnAgentRequest {
                node_name: "worker-1".into(),
                ..repo_request(1)
            }),
        )
        .await
        .unwrap()
        .into_inner();
        assert!(response.success, "{}", response.error);
        assert_eq!(response.node_name, "worker-1");
    }

    #[tokio::test]
    async fn the_single_node_named_like_the_pool_still_matches() {
        // harness() registers exactly "worker", the agents' `node:` value.
        let Harness { controller, node_rx, _tmp } = harness(None);
        let spawned = acknowledge_spawn(controller.clone(), node_rx);
        let response = FleetControl::spawn_agent(&controller, tonic::Request::new(repo_request(0)))
            .await
            .unwrap()
            .into_inner();
        assert!(response.success, "{}", response.error);
        assert_eq!(response.node_name, "worker");
        spawned.await.unwrap();
    }

    #[test]
    fn startup_recovery_matches_pool_nodes_and_spares_connected_and_paused_ones() {
        let job = |node: &str, state: crate::state::JobState| {
            let mut job = crate::state::Job::new("a", "claude");
            job.node = Some(node.to_string());
            job.state = state;
            job
        };
        use crate::state::JobState::{Paused, Running};
        let jobs = vec![
            job("agent-ctrl-node-0", Running), // pool member that never connected
            job("agent-ctrl-node-1", Running), // pool member that connected
            job("agent-ctrl-node-0", Paused),  // paused: nothing to reconcile
            job("obachan-worker", Running),    // exact-name pool, never connected
            job("someone-else", Running),      // no configured pool
        ];
        let pools: HashSet<String> = ["agent-ctrl-node".to_string(), "obachan-worker".to_string()].into();
        let interrupted = super::unreconciled_at_startup(jobs, &pools, |node| node == "agent-ctrl-node-1");
        let nodes: Vec<_> = interrupted.iter().map(|j| (j.node.clone().unwrap(), j.state)).collect();
        assert_eq!(
            nodes,
            vec![("agent-ctrl-node-0".to_string(), Running), ("obachan-worker".to_string(), Running)]
        );
    }

    #[tokio::test]
    async fn a_repository_every_node_reaches_is_reachable() {
        let Harness { controller, _tmp, .. } = harness(None);
        register_node(&controller, "worker", vec![repository("repo", REPO_URL, "dev", "")]);
        register_node(
            &controller,
            "worker-2",
            vec![repository("repo", "git@git.example.com:team/repo.git", "dev", "")],
        );
        let repositories = FleetControl::list_repositories(
            &controller,
            tonic::Request::new(pb::ListRepositoriesRequest {}),
        )
        .await
        .unwrap()
        .into_inner()
        .repositories;
        assert_eq!(repositories.len(), 1);
        assert!(repositories[0].reachable && repositories[0].error.is_empty());
    }
```
  In `list_repositories_merges_node_registrations`, replace the assertions after the sort with:
```rust
        assert_eq!(repositories.len(), 3, "{repositories:?}");
        let repo = &repositories[1];
        assert_eq!(repo.name, "repo");
        assert_eq!(repo.url, REPO_URL);
        assert!(!repo.reachable, "one node failing makes the repository unreachable");
        assert_eq!(repo.error, "worker: clone failed");
        let docs = &repositories[0];
        assert!(!docs.reachable);
        assert_eq!(docs.error, "worker-2: offline");
        assert!(repositories[2].reachable && repositories[2].error.is_empty());
```
  (The existing `node_at_capacity_becomes_resource_exhausted` :3796 keeps covering the node-side NACK path.)
- [ ] **Step 2:** `cargo test pool`, `cargo test repositor` and `cargo test startup_recovery` → FAIL.
- [ ] **Step 3: implement.**
```rust
const DEFAULT_NODE_INTERACTIVE_CAP: u32 = 4;

fn in_pool(node_name: &str, pool: &str) -> bool {
    node_name.starts_with(pool)
}

fn most_free(candidates: &[(String, u32)]) -> Option<&str> {
    candidates
        .iter()
        .filter(|(_, free)| *free > 0)
        .min_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)))
        .map(|(name, _)| name.as_str())
}

fn least_loaded(candidates: &[(String, u32)]) -> Option<&str> {
    candidates
        .iter()
        .min_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)))
        .map(|(name, _)| name.as_str())
}

struct SlotReservation(Arc<NodeHandle>);

impl Drop for SlotReservation {
    fn drop(&mut self) {
        let _ = self
            .0
            .reserved_interactive
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| Some(n.saturating_sub(1)));
    }
}
```
  `ControllerInner` gains `schedule_lock: Mutex<()>` (`Mutex::new(())` in `new`).
```rust
    /// Free interactive slots on `handle`: its cap less the larger of what it reported and the
    /// interactive agents this controller tracks on it, less reservations.
    fn free_interactive_slots(&self, handle: &NodeHandle) -> u32 {
        let name = &handle.registration.node_name;
        let cap = match handle.registration.max_interactive_runs {
            0 => DEFAULT_NODE_INTERACTIVE_CAP,
            cap => cap,
        };
        let known = self
            .agents
            .iter()
            .filter(|agent| agent.interactive && &agent.node_name == name)
            .count() as u32;
        let used = known.max(handle.running_interactive.load(Ordering::SeqCst))
            + handle.reserved_interactive.load(Ordering::SeqCst);
        cap.saturating_sub(used)
    }

    /// Chooses the node in `pool` for a run; `pinned` restricts it to that node. An interactive
    /// run reserves a slot. An error ending in `node at capacity` becomes RESOURCE_EXHAUSTED in
    /// `finish_launch`.
    async fn schedule(
        &self,
        pool: &str,
        pinned: Option<&str>,
        interactive: bool,
    ) -> std::result::Result<(String, Option<SlotReservation>), String> {
        let _serialized = self.schedule_lock.lock().await;
        let handles: Vec<Arc<NodeHandle>> = self
            .nodes
            .iter()
            .filter(|node| in_pool(node.key(), pool))
            .filter(|node| pinned.map_or(true, |name| node.key() == name))
            .map(|node| node.value().clone())
            .collect();
        if handles.is_empty() {
            return Err(match pinned {
                Some(name) => format!("node '{name}' is not connected"),
                None => format!("no node in pool '{pool}' is connected"),
            });
        }
        if !interactive {
            let loads: Vec<(String, u32)> = handles
                .iter()
                .map(|h| (h.registration.node_name.clone(), h.running_agents.load(Ordering::SeqCst)))
                .collect();
            let chosen = least_loaded(&loads).expect("handles is not empty").to_string();
            return Ok((chosen, None));
        }
        let free: Vec<(String, u32)> = handles
            .iter()
            .map(|h| (h.registration.node_name.clone(), self.free_interactive_slots(h)))
            .collect();
        let Some(chosen) = most_free(&free).map(str::to_string) else {
            return Err(NODE_AT_CAPACITY.to_string());
        };
        let handle = handles
            .into_iter()
            .find(|h| h.registration.node_name == chosen)
            .expect("chosen from handles");
        handle.reserved_interactive.fetch_add(1, Ordering::SeqCst);
        Ok((chosen, Some(SlotReservation(handle))))
    }
```
  `spawn_agent`: rename `configured_node` → `pool`; replace the node check (:1769-1773) with
```rust
        if !req.node_name.is_empty() && !in_pool(&req.node_name, &pool) {
            return Err(Status::permission_denied(
                "requested node is not in the agent's node pool",
            ));
        }
        let pinned = (!req.node_name.is_empty()).then_some(req.node_name.as_str());
```
  The fingerprint and `begin_command` keep `pool` (for today's single node the fingerprint is unchanged). Replace the dispatch tail (:1834-1850):
```rust
        let (target_node, dispatch) = match self.schedule(&pool, pinned, interactive).await {
            Ok((node, _reservation)) => {
                let dispatch = self
                    .dispatch_remote(
                        &node,
                        &agent_name,
                        override_prompt,
                        DispatchOptions {
                            interactive_override: Some(interactive),
                            control: Some(&control),
                            task: task.as_ref(),
                            run_repo: run_repo.as_ref(),
                            ..DispatchOptions::default()
                        },
                    )
                    .await;
                (node, dispatch)
            }
            Err(error) => (pool.clone(), Err(error)),
        };
        self.finish_launch(&control, target_node, dispatch)
            .await
            .map(Response::new)
```
  (`_reservation` stays alive until the dispatch returns.) `import_session`: the same rename and pool check; `self.schedule(&pool, pinned, true)` around its `dispatch_remote`. `trigger_agent` (:2174, :2208-2221): `let pool = …;` fingerprint unchanged; `let dispatch = match self.schedule(&pool, None, false).await { Ok((node, _)) => self.dispatch_remote(&node, …).await, Err(error) => Err(error) };`. `route_trigger` and `route_trigger_with_prompt`: replace the `Some(node) if self.node_connected(node)` and `Some(node)` arms with
```rust
            Some(pool) => match self.schedule(pool, None, false).await {
                Ok((node, _)) => { /* the existing dispatch_remote(&node, …) call and logging */ }
                Err(error) => {
                    tracing::warn!(agent = %agent_name, %pool, %error, "no pool node for trigger; skipping");
                }
            },
```
  (`route_trigger_with_prompt` returns `None` in the `Err` arm.)

  Controller-startup recovery (:2627-2693) today collects the configured `node:` values and, per value, interrupts the live jobs recorded on exactly that name — jobs on `agent-ctrl-node-0` would never be found. It becomes pool-based:
```rust
fn unreconciled_at_startup(
    jobs: Vec<Job>,
    pools: &HashSet<String>,
    seen: impl Fn(&str) -> bool,
) -> Vec<Job> {
    jobs.into_iter()
        .filter(|job| !job.state.is_terminal() && job.state != JobState::Paused)
        .filter(|job| {
            job.node.as_deref().is_some_and(|node| {
                pools.iter().any(|pool| in_pool(node, pool)) && !seen(node)
            })
        })
        .collect()
}
```
```rust
// run_controller, replacing :2628-2691
    let startup_controller = controller.clone();
    let startup_pools: HashSet<String> = fleet
        .config()
        .agents
        .iter()
        .filter_map(|agent| agent.config.node.clone())
        .collect();
    tokio::spawn(async move {
        tokio::time::sleep(RECONCILIATION_GRACE).await;
        let mut jobs = startup_controller.fleet.state().list_jobs().unwrap_or_default();
        if let Some(mongo) = startup_controller.fleet.mongo() {
            if let Ok(remote) = mongo.nonterminal_jobs(&startup_controller.fleet.fleet_id()).await {
                for job in remote {
                    if !jobs.iter().any(|existing| existing.job_id == job.job_id) {
                        jobs.push(job);
                    }
                }
            }
        }
        let seen = |node: &str| {
            startup_controller.node_connected(node)
                || startup_controller.connection_epochs.contains_key(node)
        };
        for job in unreconciled_at_startup(jobs, &startup_pools, &seen) {
            let node_name = job.node.clone().unwrap_or_default();
            if seen(&node_name) {
                continue;
            }
            let Some(mut job) = startup_controller.load_durable_job(&job.job_id).await else {
                continue;
            };
            if job.state.is_terminal() || job.state == JobState::Paused {
                continue;
            }
            let reason = format!(
                "node '{}' did not connect and reconcile within {} seconds of controller startup",
                node_name,
                RECONCILIATION_GRACE.as_secs()
            );
            job.state = JobState::Interrupted;
            job.live_state = Some("unknown".to_string());
            job.interrupted_reason = Some(reason.clone());
            job.error = Some(reason);
            job.completed_at = Some(Utc::now());
            job.last_activity_at = job.completed_at;
            if let Err(error) = startup_controller.fleet.persist_job(&job).await {
                tracing::warn!(job_id = %job.job_id, error = %error, "failed to persist startup-unreconciled job");
            }
        }
    });
```
  (This replaces today's per-configured-name loop; the per-job body is the existing one at :2667-2689 with `node_name` taken from the job.)
```rust
// mongo/mod.rs
    pub async fn nonterminal_jobs(&self, fleet_id: &str) -> Result<Vec<Job>> {
        let cursor = self
            .jobs()
            .find(doc! {
                "fleet_id": fleet_id,
                "state": { "$nin": ["completed", "waiting_input", "failed", "stopped", "interrupted", "paused"] },
            })
            .await?;
        Ok(cursor.try_collect().await?)
    }
```
  Repositories union:
```rust
/// One entry per normalized repository url, with the first node's name and branch. Reachable only
/// when every node that lists it reaches it; `error` names the first node that does not.
fn merge_repositories<'a>(
    reported: impl Iterator<Item = (&'a str, &'a pb::RepositoryInfo)>,
) -> Vec<pb::RepositoryInfo> {
    let mut merged: Vec<(String, pb::RepositoryInfo)> = Vec::new();
    for (node, repo) in reported {
        let key = normalize_repo_url(&repo.url);
        let failure = (!repo.reachable).then(|| format!("{node}: {}", repo.error));
        match merged.iter_mut().find(|(existing, _)| *existing == key) {
            Some((_, entry)) => {
                entry.reachable &= repo.reachable;
                if entry.error.is_empty() {
                    entry.error = failure.unwrap_or_default();
                }
            }
            None => {
                let mut entry = repo.clone();
                entry.error = failure.unwrap_or_default();
                merged.push((key, entry));
            }
        }
    }
    merged.into_iter().map(|(_, repo)| repo).collect()
}
```
  `list_repositories`: `.flat_map(|node| node.registration.repositories.iter().map(move |repo| (node.registration.node_name.as_str(), repo)))`.
- [ ] **Step 4:** `cargo test` (every existing spawn/import/trigger test stays green), clippy, fmt.
- [ ] **Step 5: commit**
```bash
git add src/remote/controller.rs src/mongo/mod.rs
git commit -m "feat(controller): schedule runs on the node pool with the most free interactive slots"
```

### Task A6: Workspace — pause push, paused marker, sweep skip, resume worktree, discard

**Files:** Modify `src/workspace_handoff.rs` (`valid_agent_branch` :109 → `pub`, `push_handoff_work` :215-257, `sweep_runs` :393-424, new fns after `prepare_run_worktree` :140-152 and after `remove_handoff_worktree` :333-357, tests in `mod tests`).

**Interfaces — Produces:**
```rust
/// Commits whatever the agent left uncommitted as `agent-ctrl run <job_id> <label>` and pushes the
/// run branch; `push_handoff_work` is this with label "end".
pub async fn push_run_work(worktree: &Path, branch: &str, job_id: &str, label: &str) -> Result<(), BackendError>;
pub fn valid_agent_branch(branch: &str) -> bool;
/// `<root>/.runs/<job_id>.paused` beside `worktree`.
pub fn paused_marker(worktree: &Path) -> PathBuf;
/// Records that `worktree` holds a paused run of `branch` on `repo_url` (credentials stripped).
pub async fn mark_paused(worktree: &Path, branch: &str, repo_url: &str) -> std::io::Result<()>;
/// The branch and repository a paused marker records; `None` without a marker.
pub async fn read_paused(worktree: &Path) -> Option<(String, String)>;
pub async fn clear_paused(worktree: &Path);
/// Adds `<root>/.runs/<job_id>` at `origin/<branch>` for a paused run resumed without its local
/// worktree; a stale paused worktree of the same job is discarded first. `agent/<job>` gets a
/// local branch; a handoff branch is detached.
pub async fn prepare_resume_worktree(config: &WorkspaceConfig, repo_url: &str, branch: &str, job_id: &str) -> Result<PathBuf, BackendError>;
/// Stops a paused run held here: pushes anything left, removes the worktree, its local agent
/// branch, the marker and the session dir. Without a marker or worktree there is nothing to do.
pub async fn discard_paused_run(config: &WorkspaceConfig, job_id: &str) -> Result<(), BackendError>;
```

- [ ] **Step 1: failing tests**
```rust
    #[tokio::test]
    async fn pause_push_commits_dirty_work_with_a_pause_message() {
        let tmp = tempfile::tempdir().unwrap();
        let (config, url) = test_support::handoff_fixture(tmp.path());
        let worktree = prepare_run_worktree(&config, &url, None, "job-1").await.unwrap();
        std::fs::write(worktree.join("wip.txt"), "half done").unwrap();
        push_run_work(&worktree, &agent_branch("job-1"), "job-1", "pause")
            .await
            .unwrap();
        let origin = std::path::PathBuf::from(&url);
        assert_eq!(
            test_support::stdout(&origin, &["log", "-1", "--format=%s", "agent/job-1"]),
            "agent-ctrl run job-1 pause"
        );
        assert_eq!(
            test_support::stdout(&origin, &["show", "agent/job-1:wip.txt"]),
            "half done"
        );
    }

    #[tokio::test]
    async fn sweep_keeps_paused_worktrees_and_their_session_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let (config, url) = test_support::handoff_fixture(tmp.path());
        let root = config.root.canonicalize().unwrap();
        let runs = root.join(".runs");
        // Pushed and clean: the sweep would remove it, were it not paused.
        let worktree = prepare_run_worktree(&config, &url, None, "job-p").await.unwrap();
        commit_file(&worktree, "p.txt");
        push_handoff_work(&worktree, &agent_branch("job-p"), "job-p").await.unwrap();
        mark_paused(&worktree, &agent_branch("job-p"), &url).await.unwrap();
        std::fs::create_dir_all(runs.join("job-p.session")).unwrap();
        std::fs::create_dir_all(runs.join("job-x.session")).unwrap();

        let report = sweep_runs(&config).await;

        assert!(worktree.is_dir());
        assert!(runs.join("job-p.session").is_dir());
        assert!(!runs.join("job-x.session").exists());
        assert_eq!(report.removed_session_dirs, 1);
        assert_eq!(
            report.kept_worktrees,
            vec![("job-p".to_string(), "paused".to_string())]
        );
        assert_eq!(
            read_paused(&worktree).await,
            Some((agent_branch("job-p"), redacted_repo_url(&url)))
        );
    }

    #[tokio::test]
    async fn resume_worktree_is_built_from_the_pushed_branch() {
        let tmp = tempfile::tempdir().unwrap();
        let (config, url) = test_support::handoff_fixture(tmp.path());
        let first = prepare_run_worktree(&config, &url, None, "job-1").await.unwrap();
        commit_file(&first, "work.txt");
        push_handoff_work(&first, &agent_branch("job-1"), "job-1").await.unwrap();
        // Another node: a fresh checkout of the same origin.
        let other = tempfile::tempdir().unwrap();
        let other_root = other.path().join("root");
        std::fs::create_dir_all(&other_root).unwrap();
        test_support::run(&other_root, &["clone", "--branch", "dev", &url, "repo"]);
        let other_config = WorkspaceConfig {
            root: other_root,
            repositories: config.repositories.clone(),
        };

        let worktree =
            prepare_resume_worktree(&other_config, &url, &agent_branch("job-1"), "job-1")
                .await
                .unwrap();

        assert_eq!(
            test_support::stdout(&worktree, &["branch", "--show-current"]),
            "agent/job-1"
        );
        assert_eq!(std::fs::read_to_string(worktree.join("work.txt")).unwrap(), "work.txt");
        let handoff =
            prepare_resume_worktree(&other_config, &url, test_support::HANDOFF_BRANCH, "job-2")
                .await
                .unwrap();
        assert_eq!(
            std::fs::read_to_string(handoff.join("moved.txt")).unwrap(),
            "local work"
        );
    }

    #[tokio::test]
    async fn resume_replaces_a_stale_paused_worktree_of_the_same_job() {
        let tmp = tempfile::tempdir().unwrap();
        let (config, url) = test_support::handoff_fixture(tmp.path());
        let stale = prepare_run_worktree(&config, &url, None, "job-1").await.unwrap();
        commit_file(&stale, "early.txt");
        push_handoff_work(&stale, &agent_branch("job-1"), "job-1").await.unwrap();
        mark_paused(&stale, &agent_branch("job-1"), &url).await.unwrap();
        // The run moved on elsewhere and pushed more.
        let elsewhere = tempfile::tempdir().unwrap();
        test_support::run(
            elsewhere.path(),
            &["clone", "--branch", "agent/job-1", &url, "w"],
        );
        commit_file(&elsewhere.path().join("w"), "later.txt");
        test_support::run(&elsewhere.path().join("w"), &["push", "origin", "agent/job-1"]);

        let worktree = prepare_resume_worktree(&config, &url, &agent_branch("job-1"), "job-1")
            .await
            .unwrap();

        assert!(worktree.join("later.txt").exists());
        assert_eq!(read_paused(&worktree).await, None);
    }

    #[tokio::test]
    async fn discarding_a_paused_run_removes_its_worktree_marker_and_branch() {
        let tmp = tempfile::tempdir().unwrap();
        let (config, url) = test_support::handoff_fixture(tmp.path());
        let root = config.root.canonicalize().unwrap();
        let worktree = prepare_run_worktree(&config, &url, None, "job-1").await.unwrap();
        commit_file(&worktree, "done.txt");
        push_handoff_work(&worktree, &agent_branch("job-1"), "job-1").await.unwrap();
        mark_paused(&worktree, &agent_branch("job-1"), &url).await.unwrap();

        discard_paused_run(&config, "job-1").await.unwrap();

        assert!(!worktree.exists());
        assert!(!paused_marker(&worktree).exists());
        let branches =
            test_support::stdout(&root.join("repo"), &["branch", "--list", "agent/job-1"]);
        assert!(branches.is_empty(), "{branches}");
        discard_paused_run(&config, "job-1").await.unwrap(); // nothing left: still Ok
    }
```
- [ ] **Step 2:** `cargo test workspace_handoff` → FAIL.
- [ ] **Step 3: implement.** Move the body of `push_handoff_work` into `push_run_work`, with `let message = format!("agent-ctrl run {job_id} {label}");`, then:
```rust
pub async fn push_handoff_work(
    worktree: &Path,
    branch: &str,
    job_id: &str,
) -> Result<(), BackendError> {
    push_run_work(worktree, branch, job_id, "end").await
}

pub fn paused_marker(worktree: &Path) -> PathBuf {
    worktree.with_extension("paused")
}

pub async fn mark_paused(worktree: &Path, branch: &str, repo_url: &str) -> std::io::Result<()> {
    let record = format!("{branch}\n{}\n", redacted_repo_url(repo_url));
    tokio::fs::write(paused_marker(worktree), record).await
}

pub async fn read_paused(worktree: &Path) -> Option<(String, String)> {
    let text = tokio::fs::read_to_string(paused_marker(worktree)).await.ok()?;
    let mut lines = text.lines();
    Some((lines.next()?.to_string(), lines.next()?.to_string()))
}

pub async fn clear_paused(worktree: &Path) {
    remove_file_quietly(&paused_marker(worktree)).await;
}

pub async fn prepare_resume_worktree(
    config: &WorkspaceConfig,
    repo_url: &str,
    branch: &str,
    job_id: &str,
) -> Result<PathBuf, BackendError> {
    let repo = configured_repository(config, repo_url)?;
    if !valid_job_id(job_id) {
        return Err(BackendError::SpawnFailed("invalid job id".into()));
    }
    let worktree = config.root.canonicalize()?.join(".runs").join(job_id);
    if read_paused(&worktree).await.is_some() {
        discard_paused_run(config, job_id).await?;
    }
    if valid_agent_branch(branch) {
        add_run_worktree(config, repo, job_id, branch, Some(branch)).await
    } else if valid_handoff_branch(branch) {
        add_run_worktree(config, repo, job_id, branch, None).await
    } else {
        Err(BackendError::SpawnFailed("invalid run branch".into()))
    }
}

pub async fn discard_paused_run(config: &WorkspaceConfig, job_id: &str) -> Result<(), BackendError> {
    if !valid_job_id(job_id) {
        return Err(BackendError::SpawnFailed("invalid job id".into()));
    }
    let worktree = config.root.canonicalize()?.join(".runs").join(job_id);
    let exists = tokio::fs::symlink_metadata(&worktree).await.is_ok();
    let Some((branch, repo_url)) = read_paused(&worktree).await else {
        if exists {
            return Err(BackendError::SpawnFailed(format!(".runs/{job_id} is not paused")));
        }
        return Ok(());
    };
    if exists {
        push_run_work(&worktree, &branch, job_id, "stop").await?;
        let local = valid_agent_branch(&branch).then_some(branch.as_str());
        remove_handoff_worktree(config, &repo_url, &worktree, local).await?;
    }
    clear_paused(&worktree).await;
    match tokio::fs::remove_dir_all(worktree.with_extension("session")).await {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            tracing::warn!(%error, %job_id, "failed to remove the paused run's session dir");
        }
        _ => {}
    }
    Ok(())
}
```
  `add_run_worktree` with `new_branch == fetch_branch` checks `refs/heads/<branch>` in the checkout; run end, the sweep and `discard_paused_run` delete that local branch, so the check holds for a fresh resume. In `sweep_runs`, at the top of the loop body after `name` is computed:
```rust
        let job = name.strip_suffix(".session").unwrap_or(&name);
        if root.join(".runs").join(format!("{job}.paused")).exists() {
            if !path.extension().is_some_and(|ext| ext == "session") {
                report.kept_worktrees.push((name, "paused".to_string()));
            }
            continue;
        }
```
  The `sweep_runs` doc gains: "A paused run (`<job>.paused` beside it) keeps its worktree and session dir."
- [ ] **Step 4:** green (existing sweep tests unchanged), clippy, fmt.
- [ ] **Step 5: commit**
```bash
git add src/workspace_handoff.rs
git commit -m "feat(workspace): pause push, paused marker the sweep honours, resume worktree, discard"
```

### Task A7: Session ids and session files in the backends

**Files:** Modify `src/agent/backend.rs` (after `PlacedSession` :42-48; trait after `place_session` :141-154), `src/agent/process.rs` (`CliAdapter` after `place_session` :128-140; helper after `write_placed_file` :183-192; `impl AgentBackend for CliBackend` after `place_session` :1398-1408), `src/agent/runner.rs` (after `place_session` :95-107), `src/agent/claude.rs` (`build_args` :49-52, impl after `place_session` :160-179, tests), `src/agent/pi.rs` (after `place_session` :250-265, tests), `src/agent/codex.rs` (after `place_session` :165-192, tests), `src/remote/node.rs` (`FakeBackend` :1804-2008).

**Interfaces — Produces:**
```rust
// backend.rs
/// A session file a CLI wrote on this machine.
#[derive(Debug, Clone, PartialEq)]
pub struct LocatedSession {
    /// Passed to `resume`.
    pub resume_ref: String,
    pub file: PathBuf,
}
// AgentBackend and CliAdapter (default: None)
/// The file this CLI keeps session `session_id` in for a run in `cwd`, if it exists here.
/// `run_dir` is where a placed session was written.
fn locate_session(&self, session_id: &str, cwd: &Path, run_dir: &Path, env: &HashMap<String, String>) -> Option<LocatedSession>;
// process.rs
/// The first file under `dir`, at most `depth` directories down, whose name `matches`; newest
/// (highest-sorting) names first.
pub(crate) fn find_file(dir: &Path, depth: usize, matches: &dyn Fn(&str) -> bool) -> Option<PathBuf>;
// runner.rs
pub fn locate_session(&self, backend: Backend, session_id: &str, cwd: &Path, run_dir: &Path, env: &HashMap<String, String>) -> Option<LocatedSession>;
```

- [ ] **Step 1: failing tests**
```rust
// claude.rs tests
    #[test]
    fn interactive_spawn_pins_the_session_id_and_resume_does_not() {
        let mut c = cfg(true);
        c.session_id = Some("0b5e1c2a-7f4d-4c1e-9a43-2f1d5b6c7e8f".into());
        let args = ClaudeAdapter.build_args(&c, None);
        let at = args.iter().position(|a| a == "--session-id").expect("--session-id");
        assert_eq!(args[at + 1], "0b5e1c2a-7f4d-4c1e-9a43-2f1d5b6c7e8f");
        let resumed = ClaudeAdapter.build_args(&c, Some("0b5e1c2a-7f4d-4c1e-9a43-2f1d5b6c7e8f"));
        assert!(!resumed.contains(&"--session-id".to_string()));
        assert!(resumed.contains(&"--resume".to_string()));
        let mut one_shot = cfg(false);
        one_shot.session_id = Some("x".into());
        assert!(!ClaudeAdapter
            .build_args(&one_shot, None)
            .contains(&"--session-id".to_string()));
    }

    #[test]
    fn a_session_is_located_in_the_project_dir_of_its_cwd() {
        let home = tempfile::tempdir().unwrap();
        let env = HashMap::from([("HOME".to_string(), home.path().display().to_string())]);
        let cwd = Path::new("/workspaces/.runs/job-1");
        assert_eq!(ClaudeAdapter.locate_session("s-1", cwd, home.path(), &env), None);
        let placed = ClaudeAdapter
            .place_session(b"{}\n", "s-1", cwd, home.path(), &env)
            .unwrap();
        let located = ClaudeAdapter
            .locate_session("s-1", cwd, home.path(), &env)
            .unwrap();
        assert_eq!(located.resume_ref, "s-1");
        assert_eq!(located.file, placed.cleanup.join("s-1.jsonl"));
    }

// pi.rs tests
    #[test]
    fn a_session_is_located_in_the_run_dir_or_the_pi_sessions_tree() {
        let home = tempfile::tempdir().unwrap();
        let run_dir = tempfile::tempdir().unwrap();
        let env = HashMap::from([("HOME".to_string(), home.path().display().to_string())]);
        let cwd = Path::new("/workspace/.runs/job-1");
        assert_eq!(PiAdapter.locate_session("abc-123", cwd, run_dir.path(), &env), None);
        let tree = home.path().join(".pi/agent/sessions/--workspace-.runs-job-1--");
        std::fs::create_dir_all(&tree).unwrap();
        let file = tree.join("2026-10-07T10-00-00-000Z_abc-123.jsonl");
        std::fs::write(&file, "{}\n").unwrap();
        let located = PiAdapter
            .locate_session("abc-123", cwd, run_dir.path(), &env)
            .unwrap();
        assert_eq!(located.file, file);
        assert_eq!(located.resume_ref, file.to_string_lossy());
        std::fs::write(run_dir.path().join("session.jsonl"), "{}\n").unwrap();
        let placed = PiAdapter
            .locate_session("abc-123", cwd, run_dir.path(), &env)
            .unwrap();
        assert_eq!(placed.file, run_dir.path().join("session.jsonl"));
    }

// codex.rs tests
    #[test]
    fn a_session_is_located_by_its_rollout_name() {
        let home = tempfile::tempdir().unwrap();
        let env = HashMap::from([("CODEX_HOME".to_string(), home.path().display().to_string())]);
        let cwd = Path::new("/w");
        assert_eq!(CodexAdapter.locate_session("t-1", cwd, home.path(), &env), None);
        let placed = CodexAdapter
            .place_session(b"{}\n", "t-1", cwd, home.path(), &env)
            .unwrap();
        let located = CodexAdapter
            .locate_session("t-1", cwd, home.path(), &env)
            .unwrap();
        assert_eq!(
            located,
            LocatedSession { resume_ref: "t-1".into(), file: placed.cleanup }
        );
    }
```
  (Use the adapter type names declared in `pi.rs` / `codex.rs` (`#[derive(Default)] pub struct …Adapter`); import `LocatedSession` from `crate::agent::backend`.)
- [ ] **Step 2:** run → FAIL.
- [ ] **Step 3: implement.**
  - `backend.rs`: `LocatedSession`; trait default `fn locate_session(&self, _session_id: &str, _cwd: &Path, _run_dir: &Path, _env: &HashMap<String, String>) -> Option<LocatedSession> { None }`.
  - `process.rs`: the same default on `CliAdapter`; `CliBackend` forwards to `self.adapter.locate_session(session_id, cwd, run_dir, env)`;
```rust
pub(crate) fn find_file(dir: &Path, depth: usize, matches: &dyn Fn(&str) -> bool) -> Option<PathBuf> {
    let mut entries: Vec<_> = std::fs::read_dir(dir).ok()?.flatten().collect();
    entries.sort_by_key(|entry| std::cmp::Reverse(entry.file_name()));
    for entry in entries {
        let path = entry.path();
        if path.is_file() && entry.file_name().to_str().is_some_and(matches) {
            return Some(path);
        }
        if depth > 0 && path.is_dir() {
            if let Some(found) = find_file(&path, depth - 1, matches) {
                return Some(found);
            }
        }
    }
    None
}
```
  - `runner.rs`: `self.get_backend_for(backend)?.locate_session(session_id, cwd, run_dir, env)`.
  - `claude.rs` `build_args`:
```rust
        if let Some(session) = resume {
            args.push("--resume".to_string());
            args.push(session.to_string());
        } else if let (true, Some(id)) = (cfg.interactive, cfg.session_id.as_ref()) {
            // The node chooses the id up front, so the run can be paused before its first turn.
            args.push("--session-id".to_string());
            args.push(id.clone());
        }
```
```rust
    fn locate_session(
        &self,
        session_id: &str,
        cwd: &Path,
        _run_dir: &Path,
        env: &HashMap<String, String>,
    ) -> Option<LocatedSession> {
        validate_session_id(session_id).ok()?;
        let file = agent_home(env)
            .ok()?
            .join(".claude/projects")
            .join(claude_project_slug(cwd))
            .join(format!("{session_id}.jsonl"));
        file.is_file().then(|| LocatedSession {
            resume_ref: session_id.to_string(),
            file,
        })
    }
```
  - `pi.rs`:
```rust
    fn locate_session(
        &self,
        session_id: &str,
        _cwd: &Path,
        run_dir: &Path,
        env: &HashMap<String, String>,
    ) -> Option<LocatedSession> {
        validate_session_id(session_id).ok()?;
        let placed = run_dir.join("session.jsonl");
        let file = if placed.is_file() {
            placed
        } else {
            let agent_dir = env_path(env, "PI_CODING_AGENT_DIR")
                .or_else(|| agent_home(env).ok().map(|home| home.join(".pi/agent")))?;
            let suffix = format!("_{session_id}.jsonl");
            find_file(&agent_dir.join("sessions"), 2, &|name| name.ends_with(&suffix))?
        };
        Some(LocatedSession {
            resume_ref: file.to_string_lossy().into_owned(),
            file,
        })
    }
```
  - `codex.rs`:
```rust
    fn locate_session(
        &self,
        session_id: &str,
        _cwd: &Path,
        _run_dir: &Path,
        env: &HashMap<String, String>,
    ) -> Option<LocatedSession> {
        validate_session_id(session_id).ok()?;
        let codex_home = env_path(env, "CODEX_HOME")
            .or_else(|| agent_home(env).ok().map(|home| home.join(".codex")))?;
        let suffix = format!("-{session_id}.jsonl");
        let file = find_file(&codex_home.join("sessions"), 4, &|name| {
            name.starts_with("rollout-") && name.ends_with(&suffix)
        })?;
        Some(LocatedSession {
            resume_ref: session_id.to_string(),
            file,
        })
    }
```
  - `node.rs` `FakeBackend`: field `located: StdMutex<Option<PathBuf>>` (default `None`) and
```rust
        fn locate_session(
            &self,
            session_id: &str,
            _cwd: &Path,
            _run_dir: &Path,
            _env: &HashMap<String, String>,
        ) -> Option<LocatedSession> {
            self.located.lock().unwrap().clone().map(|file| LocatedSession {
                resume_ref: format!("local-{session_id}"),
                file,
            })
        }
```
- [ ] **Step 4:** green, clippy, fmt.
- [ ] **Step 5: commit**
```bash
git add src/agent/backend.rs src/agent/process.rs src/agent/runner.rs src/agent/claude.rs src/agent/pi.rs src/agent/codex.rs src/remote/node.rs
git commit -m "feat(agent): claude --session-id for interactive runs; locate a CLI's session file"
```

### Task A8: Node pauses an isolated run

**Files:** Modify `src/remote/node.rs`: consts :27-38 (`PAUSE_GRACE`), imports :10-15, `run_node` sweep and runtime creation :198-218, `ActiveRun` :41-47 (literals :706, :1071, :3420, :3696, :3978), `Stop` arm :465-489, `Handoff` :740-824 (literals :1140, :1360, :3428, :3985), `start_isolated_run` :1053-1094, `IsolatedRun` :1097-1107, `run_fresh` :1150-1157, `stream_agent` :1586-1684; tests in `mod tests`.

**Interfaces — Consumes:** `push_run_work`, `mark_paused`, `read_paused`, `paused_marker` (A6); `AgentStatusUpdate.push_failed` (A1); `AgentRunner::locate_session` (A7); `StopCommand.pause`, `AgentCompleted.{paused, session_id, session_file, push_failed}`, `AgentState::Paused` (A1).

**Produces:**
```rust
/// How long a pausing agent may take to exit before it is killed.
const PAUSE_GRACE: std::time::Duration = std::time::Duration::from_secs(60);
struct ActiveRun { /* existing */ pausing: Arc<AtomicBool> }
struct Handoff { /* existing */ backend: Backend, env: HashMap<String, String>, pausing: Arc<AtomicBool> }
/// What a pause leaves for the controller.
struct PausedRun { push_failed: bool, session_file: Vec<u8> }
impl Handoff {
    /// Keeps the worktree for a resume: commits and pushes the agent's work as a pause, writes the
    /// paused marker, and reads the session file for the controller. A failed push is retried in
    /// the background (`retry_paused_push`).
    async fn pause(&self, runtime: &Arc<NodeRuntime>, runner: &AgentRunner, agent_id: &str, session_id: Option<&str>) -> PausedRun;
}
/// Retries a paused run's push every `interval` while `.runs/<job>.paused` exists; once the
/// branch holds everything, tells the controller (`paused_push_landed`) and stops.
fn retry_paused_push(runtime: Arc<NodeRuntime>, worktree: PathBuf, branch: String, job_id: String, interval: std::time::Duration);
/// `AgentStatusUpdate { job_id, state: AGENT_STATE_PAUSED, push_failed: false }`.
fn paused_push_landed(job_id: &str) -> pb::NodeMessage;
/// Starts pausing a started isolated run: its agent is asked to exit and killed after `grace`;
/// `stream_agent` then reports it paused.
fn begin_pause(agent_id: &str, runner: &Arc<AgentRunner>, runtime: &Arc<NodeRuntime>, grace: std::time::Duration) -> Result<(), BackendError>;
fn paused_completion(agent_id: &str, job_id: &str, sequence: u64, session_id: String, paused: PausedRun, error_message: String) -> pb::NodeMessage;
```

- [ ] **Step 1: failing tests**
```rust
    fn run_started(runtime: &NodeRuntime, agent_id: &str) -> bool {
        runtime
            .active
            .get(agent_id)
            .and_then(|run| run.isolated.clone())
            .is_some_and(|state| state.phase.load(Ordering::SeqCst) == MovedState::RESUMED)
    }

    /// Starts a fresh run of job-1 and waits until it has started. Returns its agent id and worktree.
    async fn started_fresh_run(
        h: &mut Harness,
        config: &WorkspaceConfig,
        url: &str,
    ) -> (String, PathBuf) {
        h.spawn(fresh_command(url), config).await;
        let ack = command_ack(&mut h.messages).await;
        assert!(ack.accepted, "{}", ack.error);
        let worktree = h.spawned().await.working_directory;
        for _ in 0..200 {
            if run_started(&h.runtime, &ack.agent_id) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        (ack.agent_id, worktree)
    }

    async fn pause_run(h: &mut Harness, agent_id: &str) -> pb::CommandAck {
        let pause = pb::StopCommand {
            command_id: format!("cmd-pause-{agent_id}"),
            agent_id: agent_id.into(),
            pause: true,
            ..Default::default()
        };
        handle_controller_message(controller(CtrlPayload::Stop(pause)), &h.runner, &h.runtime).await;
        command_ack(&mut h.messages).await
    }

    #[tokio::test]
    async fn pause_pushes_a_pause_commit_marks_the_worktree_and_reports_the_session() {
        let tmp = tempfile::tempdir().unwrap();
        let (config, url) = handoff_fixture(tmp.path());
        let mut h = harness().await;
        let transcript = tmp.path().join("transcript.jsonl");
        std::fs::write(&transcript, "conversation").unwrap();
        *h.backend.located.lock().unwrap() = Some(transcript);
        let (agent_id, worktree) = started_fresh_run(&mut h, &config, &url).await;
        std::fs::write(worktree.join("wip.txt"), "half done").unwrap();

        let ack = pause_run(&mut h, &agent_id).await;
        assert!(ack.accepted, "{}", ack.error);
        let (_, done) = run_outcome(&mut h.messages).await;

        assert!(done.paused);
        assert_eq!(done.final_state, pb::AgentState::Paused as i32);
        assert_eq!(done.session_id, "s-1");
        assert_eq!(done.session_file, b"conversation");
        assert!(!done.push_failed);
        assert_eq!(
            git_stdout(&origin(&config), &["log", "-1", "--format=%s", "agent/job-1"]),
            "agent-ctrl run job-1 pause"
        );
        assert!(worktree.join("wip.txt").exists(), "the worktree is kept");
        assert!(crate::workspace::handoff::paused_marker(&worktree).exists());
        let root = config.root.canonicalize().unwrap();
        assert!(
            crate::workspace::WorkspaceGuard::worktree(&root, "job-1").is_ok(),
            "the worktree lock is released"
        );
        assert!(!h.runtime.active.contains_key(&agent_id), "the slot is free");
    }

    #[tokio::test]
    async fn pause_that_cannot_push_keeps_the_worktree_and_says_so() {
        let tmp = tempfile::tempdir().unwrap();
        let (config, url) = handoff_fixture(tmp.path());
        let mut h = harness().await;
        let (agent_id, worktree) = started_fresh_run(&mut h, &config, &url).await;
        std::fs::write(worktree.join("wip.txt"), "half done").unwrap();
        std::fs::remove_dir_all(origin(&config)).unwrap(); // origin gone: the push fails

        assert!(pause_run(&mut h, &agent_id).await.accepted);
        let (chunks, done) = run_outcome(&mut h.messages).await;

        assert!(done.paused && done.push_failed);
        assert!(chunks
            .iter()
            .any(|chunk| chunk.content.starts_with("pause push failed")));
        assert!(worktree.join("wip.txt").exists());
        assert!(crate::workspace::handoff::paused_marker(&worktree).exists());
    }

    #[tokio::test]
    async fn a_failed_pause_push_is_retried_until_it_lands_and_the_controller_is_told() {
        let tmp = tempfile::tempdir().unwrap();
        let (config, url) = handoff_fixture(tmp.path());
        let mut h = harness().await;
        // Retries run every PROGRESS_TICK, the run's progress-push interval.
        let worktree = h.start_progress_run(&config, &url).await;
        let agent_id = h.runtime.active.iter().next().unwrap().key().clone();
        for _ in 0..200 {
            if run_started(&h.runtime, &agent_id) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        std::fs::write(worktree.join("wip.txt"), "half done").unwrap();
        let missing = tmp.path().join("missing-origin");
        git_run(&checkout(&config), &["remote", "set-url", "origin", missing.to_str().unwrap()]);

        assert!(pause_run(&mut h, &agent_id).await.accepted);
        let (_, done) = run_outcome(&mut h.messages).await;
        assert!(done.push_failed);
        git_run(&checkout(&config), &["remote", "set-url", "origin", &url]);

        let landed = loop {
            if let NodePayload::StatusUpdate(update) = next_message(&mut h.messages).await {
                break update;
            }
        };
        assert_eq!(landed.state, pb::AgentState::Paused as i32);
        assert_eq!(landed.job_id, "job-1");
        assert!(!landed.push_failed);
        assert_eq!(git_stdout(&origin(&config), &["show", "agent/job-1:wip.txt"]), "half done");
        assert!(crate::workspace::handoff::paused_marker(&worktree).exists(), "still paused");
    }

    #[tokio::test]
    async fn only_a_started_run_in_its_own_worktree_can_be_paused() {
        let mut h = harness().await;
        let empty = tempfile::tempdir().unwrap();
        let config = WorkspaceConfig {
            root: empty.path().to_path_buf(),
            repositories: vec![],
        };
        h.spawn(plain_command("cmd-1", "job-1", true), &config).await;
        let ack = command_ack(&mut h.messages).await;
        let refused = pause_run(&mut h, &ack.agent_id).await;
        assert!(!refused.accepted);
        assert!(
            refused.error.contains("only a run in its own worktree can be paused"),
            "{}",
            refused.error
        );
    }

    #[tokio::test]
    async fn fresh_claude_runs_get_a_node_chosen_session_id() {
        let tmp = tempfile::tempdir().unwrap();
        let (config, url) = handoff_fixture(tmp.path());
        let (spawned, _, _) = run_fresh(&config, fresh_command(&url), |_| {}).await;
        let id = spawned.session_id.expect("session id");
        assert!(uuid::Uuid::parse_str(&id).is_ok(), "{id}");
    }
```
- [ ] **Step 2:** `cargo test pause` → FAIL.
- [ ] **Step 3: implement.**
  - `ActiveRun.pausing: Arc<AtomicBool>` (literals: `pausing: Arc::default(),`; `use std::sync::atomic::AtomicBool`). `start_isolated_run` creates `let pausing = Arc::new(AtomicBool::new(false));` and puts it in `ActiveRun` and in a new `IsolatedRun.pausing` field.
  - `Handoff` gains `backend: Backend, env: HashMap<String, String>, pausing: Arc<AtomicBool>`; `run_fresh` and `prepare` set `backend: self.backend, env: self.cmd.env_vars.clone(), pausing: self.pausing.clone()`; test literals :3428, :3985 set `backend: Backend::Claude, env: HashMap::new(), pausing: Arc::default()`.
  - `run_fresh`, after `config.agent_id = Some(self.agent_id.clone());` (:1157):
```rust
        if matches!(self.backend, Backend::Claude) && config.session_id.is_none() {
            config.session_id = Some(uuid::Uuid::new_v4().to_string());
        }
```
    (`launch` seeds `LiveStatus.session_id` from `config.session_id`, so the node's first status update carries it to the controller job.)
  - `Stop` arm: insert as the second branch, after the dedup `if let Some(previous) …` (the discard branch joins it in A9):
```rust
            } else if cmd.pause {
                let result = begin_pause(&cmd.agent_id, runner, runtime, PAUSE_GRACE);
                let response = ack(&cmd.command_id, result.is_ok(), &cmd.agent_id, result.err());
                runtime.remember_ack(&cmd.command_id, response.clone());
                runtime.send(response).await;
            } else {
```
```rust
fn begin_pause(
    agent_id: &str,
    runner: &Arc<AgentRunner>,
    runtime: &Arc<NodeRuntime>,
    grace: std::time::Duration,
) -> Result<(), BackendError> {
    let run = runtime
        .active
        .get(agent_id)
        .map(|run| run.clone())
        .ok_or_else(|| BackendError::NotRunning(agent_id.to_string()))?;
    let Some(state) = run.isolated.as_ref() else {
        return Err(BackendError::SpawnFailed(
            "only a run in its own worktree can be paused".into(),
        ));
    };
    if state.phase.load(Ordering::SeqCst) != MovedState::RESUMED {
        return Err(BackendError::NotRunning(format!("{agent_id} has not started yet")));
    }
    if run.pausing.swap(true, Ordering::SeqCst) {
        return Ok(());
    }
    let runner = runner.clone();
    let agent_id = agent_id.to_string();
    tokio::spawn(async move {
        if let Err(error) = runner.stop(&agent_id, false).await {
            tracing::warn!(%agent_id, %error, "graceful stop for pause failed");
        }
        let deadline = tokio::time::Instant::now() + grace;
        loop {
            match runner.status(&agent_id).await {
                Ok(status) if !is_terminal(status.state) => {}
                _ => return,
            }
            if tokio::time::Instant::now() >= deadline {
                let _ = runner.stop(&agent_id, true).await;
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    });
    Ok(())
}
```
  - `Handoff::pause`:
```rust
    async fn pause(
        &self,
        runtime: &Arc<NodeRuntime>,
        runner: &AgentRunner,
        agent_id: &str,
        session_id: Option<&str>,
    ) -> PausedRun {
        let push_failed = match push_run_work(&self.worktree, &self.branch, &self.job_id, "pause").await {
            Ok(()) => false,
            Err(error) => {
                tracing::warn!(job_id = %self.job_id, %error, "pause push failed; keeping the worktree");
                let note = format!("pause push failed; worktree kept at {}", self.worktree.display());
                emit_chunk(runtime, agent_id, note, pb::OutputType::Status).await;
                true
            }
        };
        if let Err(error) = mark_paused(&self.worktree, &self.branch, &self.repo_url).await {
            tracing::warn!(job_id = %self.job_id, %error, "failed to write the paused marker");
        }
        if push_failed {
            retry_paused_push(
                runtime.clone(),
                self.worktree.clone(),
                self.branch.clone(),
                self.job_id.clone(),
                self.progress_push,
            );
        }
        let located = session_id.and_then(|id| {
            runner.locate_session(self.backend, id, &self.worktree, &self.run_dir(), &self.env)
        });
        let session_file = match located {
            Some(located) => match tokio::fs::read(&located.file).await {
                Ok(file) if file.len() <= super::MAX_SESSION_FILE_BYTES => file,
                Ok(_) => {
                    let note = "session file too large for the controller; resume this run on this node";
                    emit_chunk(runtime, agent_id, note.to_string(), pb::OutputType::Status).await;
                    Vec::new()
                }
                Err(error) => {
                    tracing::warn!(job_id = %self.job_id, %error, "failed to read the paused session file");
                    Vec::new()
                }
            },
            None => Vec::new(),
        };
        PausedRun { push_failed, session_file }
    }
```
```rust
fn retry_paused_push(
    runtime: Arc<NodeRuntime>,
    worktree: PathBuf,
    branch: String,
    job_id: String,
    interval: std::time::Duration,
) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(interval).await;
            // A resume or a stop removed the marker; the run's own pushes take over.
            if read_paused(&worktree).await.is_none() {
                return;
            }
            match push_run_work(&worktree, &branch, &job_id, "pause").await {
                Ok(()) => {
                    tracing::info!(%job_id, %branch, "paused run's push landed");
                    runtime.send(paused_push_landed(&job_id)).await;
                    return;
                }
                Err(error) => tracing::warn!(%job_id, %error, "paused run's push still failing"),
            }
        }
    });
}

fn paused_push_landed(job_id: &str) -> pb::NodeMessage {
    pb::NodeMessage {
        payload: Some(NodePayload::StatusUpdate(pb::AgentStatusUpdate {
            job_id: job_id.to_string(),
            state: pb::AgentState::Paused as i32,
            push_failed: false,
            interactive: true,
            last_activity: Some(super::to_pb_timestamp(chrono::Utc::now())),
            ..Default::default()
        })),
    }
}
```
  - Node restart: the sweep keeps paused worktrees (A6), but their retry tasks died with the process. In `run_node` (:198-213) collect `let paused: Vec<String> = swept.kept_worktrees.iter().filter(|(_, reason)| reason == "paused").map(|(job, _)| job.clone()).collect();` and, once `runtime` exists (:215), start a retry for each:
```rust
    if let Ok(Some(workspaces)) = crate::workspace::load_workspace_config_from_env() {
        if let Ok(root) = workspaces.root.canonicalize() {
            for job_id in paused {
                let worktree = local_path(root.join(".runs").join(&job_id));
                if let Some((branch, _)) = read_paused(&worktree).await {
                    retry_paused_push(runtime.clone(), worktree, branch, job_id, progress_push_interval());
                }
            }
        }
    }
```
    (A worktree whose push already landed reports `push_failed: false` after one interval; the controller ignores it unless the job is paused on this node with its push recorded as failed.)
  - Imports: `read_paused` from `crate::workspace::handoff`.
  - `stream_agent`: after `let status = wait_for_terminal_status(…).await;` read `let session_id = status.as_ref().and_then(|s| s.session_id.clone());` (before `runner.forget`). Replace the `if let Some(handoff) = handoff { … }` block (:1630-1641) with
```rust
        if let Some(handoff) = &handoff {
            let ended = status.as_ref().is_some_and(|s| is_terminal(s.state));
            if ended && handoff.pausing.load(Ordering::SeqCst) {
                let paused = handoff
                    .pause(&runtime, &runner, &agent_id, session_id.as_deref())
                    .await;
                let message = paused_completion(
                    &agent_id,
                    &job_id,
                    last_event_sequence.load(Ordering::SeqCst),
                    session_id.unwrap_or_default(),
                    paused,
                    String::new(),
                );
                runtime.send(message).await;
                runtime.active.remove(&agent_id);
                return;
            }
            if ended {
                handoff.finish(&runtime, &agent_id).await;
            } else {
                tracing::warn!(
                    agent_id,
                    worktree = %handoff.worktree.display(),
                    "run may still be running; leaving its worktree in place"
                );
                handoff.remove_session_files().await;
            }
        }
```
    (`runner.forget` stays before it; `locate_session` works by backend, not by agent id.)
```rust
fn paused_completion(
    agent_id: &str,
    job_id: &str,
    sequence: u64,
    session_id: String,
    paused: PausedRun,
    error_message: String,
) -> pb::NodeMessage {
    pb::NodeMessage {
        payload: Some(NodePayload::AgentCompleted(pb::AgentCompleted {
            agent_id: agent_id.to_string(),
            final_state: pb::AgentState::Paused as i32,
            exit_code: 0,
            error_message,
            token_usage: None,
            duration_ms: 0,
            job_id: job_id.to_string(),
            last_event_sequence: sequence,
            paused: true,
            session_id,
            session_file: paused.session_file,
            push_failed: paused.push_failed,
        })),
    }
}
```
- [ ] **Step 4:** green (every existing node test too), clippy, fmt.
- [ ] **Step 5: commit**
```bash
git add src/remote/node.rs
git commit -m "feat(node): pause an isolated run — push, keep the worktree, report its session"
```

### Task A9: Node resumes a paused run and discards one on stop

**Files:** Modify `src/remote/node.rs`: `handle_controller_message` :433-528 (becomes a wrapper), `Stop` arm, `handle_spawn_with` isolated selection :652-665, new `validate_paused_resume` after `validate_fresh_run` :950-980, `RunMode` :1030-1042, `IsolatedRun::run` :1110-1122, new `run_resumed` after `run_fresh`, `prepare` :1349-1403 (placement extracted), `fail`/`end_stopped` :1405-1425; tests.

**Interfaces — Consumes:** `prepare_resume_worktree`, `discard_paused_run`, `read_paused`, `clear_paused`, `valid_agent_branch` (A6); `locate_session` (A7); `Handoff::pause`, `paused_completion`, `PausedRun` (A8).

**Produces:**
```rust
enum RunMode {
    Moved,
    Fresh { repo_url: String, base_branch: Option<String>, subdir: PathBuf },
    /// Resumes a paused run of the same job on `branch`, in its kept worktree when `local`.
    Resume { repo_url: String, branch: String, local: bool },
}
async fn handle_controller_message_with(
    message: pb::ControllerMessage,
    runner: &Arc<AgentRunner>,
    runtime: &Arc<NodeRuntime>,
    load_workspaces: impl FnOnce() -> std::result::Result<Option<WorkspaceConfig>, BackendError>,
);
fn validate_paused_resume(cmd: &pb::SpawnCommand, load_workspaces: impl FnOnce() -> std::result::Result<Option<WorkspaceConfig>, BackendError>) -> std::result::Result<WorkspaceConfig, BackendError>;
impl IsolatedRun {
    async fn run_resumed(self, workspaces: WorkspaceConfig, repo_url: String, branch: String, local: bool);
    /// Places the command's session file beside the worktree; returns the resume reference.
    async fn place_session_file(&mut self, handoff: &mut Handoff) -> std::result::Result<String, BackendError>;
    /// Ends a resume that never started its agent: the job goes back to paused.
    async fn end_paused(self, paused: PausedRun, reason: String);
}
```

- [ ] **Step 1: failing tests**
```rust
    fn resume_command(url: &str, local: bool) -> pb::SpawnCommand {
        pb::SpawnCommand {
            command_id: "cmd-resume".into(),
            agent_name: "staging-claude-interactive-rw".into(),
            backend: "claude".into(),
            interactive: true,
            permission_mode: "acceptEdits".into(),
            job_id: "job-1".into(),
            session_id: "s-1".into(),
            run_repo_url: url.into(),
            resume_paused: true,
            resume_branch: "agent/job-1".into(),
            resume_local: local,
            ..Default::default()
        }
    }

    /// Pauses a fresh run of job-1 on `h` and returns its worktree.
    async fn paused_run(h: &mut Harness, config: &WorkspaceConfig, url: &str) -> PathBuf {
        let (agent_id, worktree) = started_fresh_run(h, config, url).await;
        std::fs::write(worktree.join("wip.txt"), "half done").unwrap();
        assert!(pause_run(h, &agent_id).await.accepted);
        assert!(run_outcome(&mut h.messages).await.1.paused);
        h.backend.stopped.store(false, Ordering::SeqCst);
        h.backend.finished.store(false, Ordering::SeqCst);
        worktree
    }

    #[tokio::test]
    async fn resume_on_the_same_node_continues_in_the_kept_worktree() {
        let tmp = tempfile::tempdir().unwrap();
        let (config, url) = handoff_fixture(tmp.path());
        let mut h = harness().await;
        let worktree = paused_run(&mut h, &config, &url).await;
        let local = tmp.path().join("local.jsonl");
        std::fs::write(&local, "x").unwrap();
        *h.backend.located.lock().unwrap() = Some(local);

        h.spawn(resume_command(&url, true), &config).await;
        let ack = command_ack(&mut h.messages).await;
        assert!(ack.accepted, "{}", ack.error);
        let (resume_ref, resumed) = h.resumed().await;

        assert_eq!(resume_ref, "local-s-1");
        assert_eq!(resumed.working_directory, worktree);
        assert!(worktree.join("wip.txt").exists());
        assert!(!crate::workspace::handoff::paused_marker(&worktree).exists());
        assert!(h.backend.placed.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn resume_on_another_node_builds_the_worktree_and_places_the_session() {
        let tmp = tempfile::tempdir().unwrap();
        let (config, url) = handoff_fixture(tmp.path());
        let mut first = harness().await;
        paused_run(&mut first, &config, &url).await;
        let other_root = tmp.path().join("other");
        std::fs::create_dir_all(&other_root).unwrap();
        git_run(&other_root, &["clone", "--branch", "dev", &url, "repo"]);
        let other = WorkspaceConfig {
            root: other_root,
            repositories: copy_config(&config).repositories,
        };
        let mut h = harness().await;

        let command = pb::SpawnCommand {
            resume_session_file: b"conversation".to_vec(),
            ..resume_command(&url, false)
        };
        h.spawn(command, &other).await;
        assert!(command_ack(&mut h.messages).await.accepted);
        let (resume_ref, resumed) = h.resumed().await;

        assert_eq!(resume_ref, "resume-s-1");
        assert_eq!(
            std::fs::read_to_string(resumed.working_directory.join("wip.txt")).unwrap(),
            "half done"
        );
        let placed = h.backend.placed.lock().unwrap().clone();
        assert_eq!(placed[0].file, b"conversation");
        assert_eq!(placed[0].session_id, "s-1");
    }

    #[tokio::test]
    async fn resume_without_any_transcript_starts_the_session_anew() {
        let tmp = tempfile::tempdir().unwrap();
        let (config, url) = handoff_fixture(tmp.path());
        let mut h = harness().await;
        paused_run(&mut h, &config, &url).await;
        h.backend.spawned.lock().unwrap().clear();

        h.spawn(resume_command(&url, true), &config).await;
        assert!(command_ack(&mut h.messages).await.accepted);
        let spawned = h.spawned().await;

        assert_eq!(spawned.session_id.as_deref(), Some("s-1"));
        assert!(h.backend.resumed.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_resume_whose_worktree_cannot_be_built_stays_paused() {
        let tmp = tempfile::tempdir().unwrap();
        let (config, url) = handoff_fixture(tmp.path());
        let mut h = harness().await;
        // Nothing was ever pushed to agent/job-1, so the worktree cannot be fetched.
        h.spawn(resume_command(&url, false), &config).await;
        assert!(command_ack(&mut h.messages).await.accepted);
        let (_, done) = run_outcome(&mut h.messages).await;
        assert!(done.paused);
        assert!(done.error_message.starts_with("resume failed"), "{}", done.error_message);
        assert_eq!(h.backend.calls(), 0);
    }

    #[tokio::test]
    async fn resume_counts_against_the_interactive_cap() {
        let tmp = tempfile::tempdir().unwrap();
        let (config, url) = handoff_fixture(tmp.path());
        let mut h = harness().await;
        h.spawn_capped(plain_command("cmd-0", "job-0", true), &config, 1).await;
        assert!(command_ack(&mut h.messages).await.accepted);
        h.spawn_capped(resume_command(&url, true), &config, 1).await;
        let refused = command_ack(&mut h.messages).await;
        assert!(refused.error.contains("node at capacity"), "{}", refused.error);
    }

    #[tokio::test]
    async fn stopping_a_paused_run_discards_its_worktree() {
        let tmp = tempfile::tempdir().unwrap();
        let (config, url) = handoff_fixture(tmp.path());
        let mut h = harness().await;
        let worktree = paused_run(&mut h, &config, &url).await;
        let stop = pb::StopCommand {
            command_id: "cmd-discard".into(),
            discard_paused_job_id: "job-1".into(),
            ..Default::default()
        };
        handle_controller_message_with(
            controller(CtrlPayload::Stop(stop)),
            &h.runner,
            &h.runtime,
            || Ok(Some(copy_config(&config))),
        )
        .await;
        let ack = command_ack(&mut h.messages).await;
        assert!(ack.accepted, "{}", ack.error);
        assert!(!worktree.exists());
        assert!(!crate::workspace::handoff::paused_marker(&worktree).exists());
    }
```
- [ ] **Step 2:** run → FAIL.
- [ ] **Step 3: implement.**
  - Rename the body of `handle_controller_message` to `handle_controller_message_with` (extra `load_workspaces` parameter); `handle_controller_message(message, runner, runtime)` calls it with `crate::workspace::load_workspace_config_from_env`. In the `Stop` arm, before the pause branch:
```rust
            } else if !cmd.discard_paused_job_id.is_empty() {
                let result = match load_workspaces() {
                    Ok(Some(workspaces)) => {
                        discard_paused_run(&workspaces, &cmd.discard_paused_job_id).await
                    }
                    Ok(None) => Err(BackendError::SpawnFailed(
                        "node has no workspace configuration".into(),
                    )),
                    Err(error) => Err(error),
                };
                let response = ack(&cmd.command_id, result.is_ok(), "", result.err());
                runtime.remember_ack(&cmd.command_id, response.clone());
                runtime.send(response).await;
```
    (The `Spawn` arm keeps calling `handle_spawn(cmd, runner, runtime)`, which loads its own configuration; only the `Stop` arm uses `load_workspaces`, so pass it through as `impl FnOnce` used in that arm alone.)
  - `handle_spawn_with` (:652) — first branch:
```rust
    let isolated = if cmd.resume_paused {
        Some(validate_paused_resume(&cmd, load_workspaces).map(|ws| {
            let mode = RunMode::Resume {
                repo_url: cmd.run_repo_url.clone(),
                branch: cmd.resume_branch.clone(),
                local: cmd.resume_local,
            };
            (ws, mode)
        }))
    } else if !cmd.resume_session_file.is_empty() {
```
```rust
fn validate_paused_resume(
    cmd: &pb::SpawnCommand,
    load_workspaces: impl FnOnce() -> std::result::Result<Option<WorkspaceConfig>, BackendError>,
) -> std::result::Result<WorkspaceConfig, BackendError> {
    if !cmd.interactive {
        return Err(BackendError::SpawnFailed("a paused run resumes interactively".into()));
    }
    if !valid_job_id(&cmd.job_id) {
        return Err(BackendError::SpawnFailed("invalid job id".into()));
    }
    validate_session_id(&cmd.session_id)?;
    if cmd.resume_session_file.len() > super::MAX_SESSION_FILE_BYTES {
        return Err(BackendError::SpawnFailed("paused session file is too large".into()));
    }
    if !valid_handoff_branch(&cmd.resume_branch) && !valid_agent_branch(&cmd.resume_branch) {
        return Err(BackendError::SpawnFailed("invalid run branch".into()));
    }
    let workspaces = load_workspaces()?
        .ok_or_else(|| BackendError::SpawnFailed("node has no workspace configuration".into()))?;
    if find_repository(&workspaces, &cmd.run_repo_url).is_none() {
        return Err(BackendError::SpawnFailed(format!(
            "{} is not an operator-configured repository",
            redacted_repo_url(&cmd.run_repo_url)
        )));
    }
    Ok(workspaces)
}
```
  - `IsolatedRun::run`: `RunMode::Resume { repo_url, branch, local } => self.run_resumed(workspaces, repo_url, branch, local).await`. `fail` / `end_stopped` outcomes gain `RunMode::Resume { .. } => "paused run did not resume"` / `"paused run stopped before it resumed"`.
  - Extract the placement half of `prepare` (:1371-1401, from `let run_dir = handoff.run_dir();` through setting `handoff.transcript`) into `place_session_file(&mut self, handoff: &mut Handoff) -> Result<String, BackendError>` returning `placed.resume_ref`; `prepare` calls it and keeps removing the worktree on its error.
  - `run_resumed`:
```rust
    async fn run_resumed(
        mut self,
        workspaces: WorkspaceConfig,
        repo_url: String,
        branch: String,
        local: bool,
    ) {
        let job_id = self.cmd.job_id.clone();
        let kept = match workspaces.root.canonicalize() {
            Ok(root) => local_path(root.join(".runs").join(&job_id)),
            Err(error) => {
                let paused = PausedRun { push_failed: false, session_file: Vec::new() };
                return self.end_paused(paused, format!("resume failed: {error}")).await;
            }
        };
        let worktree = if local && read_paused(&kept).await.is_some() {
            Ok(kept)
        } else {
            prepare_resume_worktree(&workspaces, &repo_url, &branch, &job_id)
                .await
                .map(local_path)
        };
        let worktree = match worktree {
            Ok(worktree) => worktree,
            Err(_) if self.stopped() => return self.end_stopped(None).await,
            Err(error) => {
                let session_file = std::mem::take(&mut self.cmd.resume_session_file);
                let paused = PausedRun { push_failed: false, session_file };
                return self.end_paused(paused, format!("resume failed: {error}")).await;
            }
        };
        let mut handoff = Handoff {
            workspaces,
            repo_url,
            branch: branch.clone(),
            job_id,
            worktree,
            transcript: None,
            local_branch: valid_agent_branch(&branch).then(|| branch.clone()),
            progress_push: self.progress_push,
            backend: self.backend,
            env: self.cmd.env_vars.clone(),
            pausing: self.pausing.clone(),
        };
        let session_id = self.cmd.session_id.clone();
        let located = self.runner.locate_session(
            self.backend,
            &session_id,
            &handoff.worktree,
            &handoff.run_dir(),
            &handoff.env,
        );
        let resume_ref = match located {
            Some(located) => {
                handoff.transcript = Some(located.file);
                Ok(Some(located.resume_ref))
            }
            None if self.cmd.resume_session_file.is_empty() => Ok(None),
            None => self.place_session_file(&mut handoff).await.map(Some),
        };
        let resume_ref = match resume_ref {
            Ok(resume_ref) => resume_ref,
            Err(error) => {
                let paused = handoff
                    .pause(&self.runtime, &self.runner, &self.agent_id, Some(&session_id))
                    .await;
                return self.end_paused(paused, format!("resume failed: {error}")).await;
            }
        };
        if self.stopped() {
            handoff.finish(&self.runtime, &self.agent_id).await;
            return self.end_stopped(None).await;
        }
        clear_paused(&handoff.worktree).await;
        let mut config = super::spawn_config_from(&self.cmd);
        config.working_directory = handoff.worktree.clone();
        config.prompt = String::new();
        config.session_id = Some(session_id.clone());
        config.agent_id = Some(self.agent_id.clone());
        let started = match &resume_ref {
            Some(resume_ref) => {
                self.runner
                    .resume(&self.cmd.agent_name, self.backend, resume_ref, config)
                    .await
            }
            None => self.runner.spawn(&self.cmd.agent_name, self.backend, config).await,
        };
        let running = match started {
            Ok(running) => running,
            Err(error) => {
                let paused = handoff
                    .pause(&self.runtime, &self.runner, &self.agent_id, Some(&session_id))
                    .await;
                return self.end_paused(paused, format!("resume failed: {error}")).await;
            }
        };
        monitor_agent_status(
            self.agent_id.clone(),
            self.cmd.agent_name.clone(),
            self.cmd.backend.clone(),
            self.cmd.job_id.clone(),
            self.runner.clone(),
            self.runtime.clone(),
            true,
        );
        if !self.moved.transition(MovedState::PENDING, MovedState::RESUMED) {
            let _ = self.runner.stop(&self.agent_id, true).await;
        }
        let note = if resume_ref.is_some() {
            "resumed"
        } else {
            "resumed without a transcript; the conversation starts anew"
        };
        emit_chunk(&self.runtime, &self.agent_id, note.to_string(), pb::OutputType::Status).await;
        stream_agent(
            running,
            self.cmd.job_id.clone(),
            self.runner.clone(),
            self.runtime.clone(),
            self.sequence.clone(),
            Some(handoff),
        );
    }

    async fn end_paused(self, paused: PausedRun, reason: String) {
        tracing::warn!(agent_id = %self.agent_id, job_id = %self.cmd.job_id, %reason, "paused run did not resume");
        let message = paused_completion(
            &self.agent_id,
            &self.cmd.job_id,
            self.sequence.load(Ordering::SeqCst),
            self.cmd.session_id.clone(),
            paused,
            reason,
        );
        self.runtime.send(message).await;
        self.runtime.active.remove(&self.agent_id);
    }
```
    (A resume that fails after its worktree is ready re-runs `Handoff::pause`, which re-writes the marker and reports `push_failed` accurately, so a pinned run stays pinned.)
- [ ] **Step 4:** green, clippy, fmt.
- [ ] **Step 5: commit**
```bash
git add src/remote/node.rs
git commit -m "feat(node): resume a paused run here or from its branch; discard a paused run on stop"
```

### Task A10: Controller `PauseAgent`, paused completions, `StopAgent` on a paused run

**Files:** Modify `src/remote/controller.rs`: consts :32-35 (`PAUSE_SETTLE`), `finalize_agent` :1146-1204, `stop_agent` :1853-1945 (ack code extracted into `send_stop`), `pause_agent` stub (A1), free fn `pause_response` after `replayed_launch`; tests in `mod import_session_tests`.

**Interfaces — Consumes:** `store_paused_session`, `load_paused_session`, `forget_paused_session` (A3); `free_interactive_slots` (A5); proto from A1.

**Produces:**
```rust
/// How long `PauseAgent` waits for the node to report the run paused (60 s grace plus push).
const PAUSE_SETTLE: Duration = Duration::from_secs(120);
impl ControllerInner {
    /// Records a node's paused completion: the job is paused, its transcript kept for a resume.
    async fn record_pause(&self, job: &mut Job, agent: &RemoteAgent, done: pb::AgentCompleted);
    /// A paused run's retried push landed on `node_name`: the job is no longer pinned there.
    async fn record_paused_push(&self, node_name: &str, update: &pb::AgentStatusUpdate);
    /// Sends `command` to `node_name` and waits for its acknowledgement: (accepted, error).
    async fn send_stop(&self, node_name: &str, command: pb::StopCommand) -> (bool, String);
    /// The paused job a stop names: by `job_id`, else by its last agent id.
    async fn paused_job_for_stop(&self, req: &pb::StopAgentRequest) -> Option<Job>;
    /// Tells the node holding a paused run's worktree to discard it, then stops the job.
    async fn discard_paused(&self, job: &Job) -> std::result::Result<(), String>;
}
fn pause_response(job: &Job, accepted: bool, error: String, request_id: String, replayed: bool) -> pb::PauseAgentResponse;
```

- [ ] **Step 1: failing tests**
```rust
    async fn next_command(rx: &mut mpsc::Receiver<CtrlMsgResult>) -> CtrlPayload {
        tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("controller sent nothing")
            .expect("node channel closed")
            .expect("controller sent a status error")
            .payload
            .expect("empty controller message")
    }

    /// Answers the next spawn or stop on `rx` with `accepted` / `error`, as agent "agent-1".
    async fn answer_next(
        controller: &super::Controller,
        rx: &mut mpsc::Receiver<CtrlMsgResult>,
        accepted: bool,
        error: &str,
    ) {
        let command_id = match next_command(rx).await {
            CtrlPayload::Spawn(command) => command.command_id,
            CtrlPayload::Stop(command) => command.command_id,
            _ => panic!("unexpected command"),
        };
        let (_, waiter) = controller.pending.remove(&command_id).expect("pending ack");
        waiter
            .send(pb::CommandAck {
                command_id,
                accepted,
                agent_id: "agent-1".into(),
                error: error.into(),
            })
            .unwrap();
    }

    async fn status_with_session(
        controller: &super::Controller,
        agent_id: &str,
        job_id: &str,
        session_id: &str,
    ) {
        controller
            .handle_node_message(
                "worker",
                pb::NodeMessage {
                    payload: Some(super::NodePayload::StatusUpdate(pb::AgentStatusUpdate {
                        agent_id: agent_id.into(),
                        job_id: job_id.into(),
                        session_id: session_id.into(),
                        state: pb::AgentState::WaitingInput as i32,
                        interactive: true,
                        ..Default::default()
                    })),
                },
            )
            .await;
    }

    /// A repo run of repo-runner on "worker" that reported session `s-1`.
    /// Returns (job_id, agent_id, node_rx).
    async fn running_repo_job(
        controller: &super::Controller,
        mut node_rx: mpsc::Receiver<CtrlMsgResult>,
    ) -> (String, String, mpsc::Receiver<CtrlMsgResult>) {
        let (response, ()) = tokio::join!(
            FleetControl::spawn_agent(controller, tonic::Request::new(repo_spawn_request("repo-runner"))),
            answer_next(controller, &mut node_rx, true, ""),
        );
        let response = response.unwrap().into_inner();
        status_with_session(controller, &response.agent_id, &response.job_id, "s-1").await;
        (response.job_id, response.agent_id, node_rx)
    }

    fn pause_request(job_id: &str, request_id: &str) -> pb::PauseAgentRequest {
        pb::PauseAgentRequest {
            job_id: job_id.into(),
            request_id: request_id.into(),
            actor: "operator".into(),
            source: "direct_cli".into(),
        }
    }

    /// Plays the node's side of a pause: ack the pause command, then report the run paused.
    async fn node_pauses(
        controller: &super::Controller,
        rx: &mut mpsc::Receiver<CtrlMsgResult>,
        job_id: &str,
        agent_id: &str,
        push_failed: bool,
    ) {
        let CtrlPayload::Stop(stop) = next_command(rx).await else {
            panic!("expected Stop")
        };
        assert!(stop.pause);
        assert_eq!(stop.agent_id, agent_id);
        let (_, waiter) = controller.pending.remove(&stop.command_id).unwrap();
        waiter
            .send(pb::CommandAck {
                command_id: stop.command_id,
                accepted: true,
                agent_id: agent_id.into(),
                error: String::new(),
            })
            .unwrap();
        controller
            .handle_node_message(
                "worker",
                pb::NodeMessage {
                    payload: Some(super::NodePayload::AgentCompleted(pb::AgentCompleted {
                        agent_id: agent_id.into(),
                        job_id: job_id.into(),
                        final_state: pb::AgentState::Paused as i32,
                        paused: true,
                        session_id: "s-1".into(),
                        session_file: b"transcript".to_vec(),
                        push_failed,
                        ..Default::default()
                    })),
                },
            )
            .await;
    }

    #[tokio::test]
    async fn pause_records_a_paused_job_keeps_its_session_and_frees_the_slot() {
        let Harness { controller, node_rx, _tmp } = harness(None);
        let (job_id, agent_id, mut node_rx) = running_repo_job(&controller, node_rx).await;
        let worker = controller.nodes.get("worker").unwrap().clone();
        let before = controller.free_interactive_slots(&worker);

        let (response, ()) = tokio::join!(
            FleetControl::pause_agent(&controller, tonic::Request::new(pause_request(&job_id, "pause-1"))),
            node_pauses(&controller, &mut node_rx, &job_id, &agent_id, false),
        );
        let response = response.unwrap().into_inner();

        assert!(response.accepted, "{}", response.error);
        assert_eq!(response.state, "paused");
        assert_eq!(response.job_id, job_id);
        assert_eq!(response.node_name, "worker");
        let job = controller.fleet.state().load_job(&job_id).unwrap();
        assert_eq!(job.state, crate::state::JobState::Paused);
        assert_eq!(job.session_id.as_deref(), Some("s-1"));
        assert!(job.paused_at.is_some() && job.completed_at.is_none());
        assert_eq!(controller.load_paused_session(&job_id).await, b"transcript");
        assert_eq!(controller.free_interactive_slots(&worker), before + 1);

        let again = FleetControl::pause_agent(&controller, tonic::Request::new(pause_request(&job_id, "pause-2")))
            .await
            .unwrap()
            .into_inner();
        assert!(again.accepted);
        assert_eq!(again.state, "paused");
        assert!(node_rx.try_recv().is_err(), "a no-op pause sends nothing");
    }

    #[tokio::test]
    async fn a_landed_paused_push_lifts_the_node_pin() {
        let Harness { controller, node_rx, _tmp } = harness(None);
        let (job_id, agent_id, mut node_rx) = running_repo_job(&controller, node_rx).await;
        let (paused, ()) = tokio::join!(
            FleetControl::pause_agent(&controller, tonic::Request::new(pause_request(&job_id, "pause-1"))),
            node_pauses(&controller, &mut node_rx, &job_id, &agent_id, true),
        );
        assert!(paused.unwrap().into_inner().accepted);
        assert!(controller.fleet.state().load_job(&job_id).unwrap().pause_push_failed);
        let landed = || pb::NodeMessage {
            payload: Some(super::NodePayload::StatusUpdate(pb::AgentStatusUpdate {
                job_id: job_id.clone(),
                state: pb::AgentState::Paused as i32,
                push_failed: false,
                ..Default::default()
            })),
        };

        controller.handle_node_message("worker-2", landed()).await;
        assert!(
            controller.fleet.state().load_job(&job_id).unwrap().pause_push_failed,
            "only the node holding the worktree can lift the pin"
        );
        controller.handle_node_message("worker", landed()).await;
        let job = controller.fleet.state().load_job(&job_id).unwrap();
        assert!(!job.pause_push_failed);
        assert_eq!(job.state, crate::state::JobState::Paused, "a push report never resumes a run");
        assert!(job.error.is_none());
    }

    #[tokio::test]
    async fn pause_without_a_known_session_id_is_refused() {
        let Harness { controller, mut node_rx, _tmp } = harness(None);
        let (response, ()) = tokio::join!(
            FleetControl::spawn_agent(&controller, tonic::Request::new(repo_spawn_request("repo-runner"))),
            answer_next(&controller, &mut node_rx, true, ""),
        );
        let job_id = response.unwrap().into_inner().job_id;
        let status = FleetControl::pause_agent(&controller, tonic::Request::new(pause_request(&job_id, "pause-1")))
            .await
            .expect_err("session unknown");
        assert_eq!(status.code(), tonic::Code::FailedPrecondition);
        assert_eq!(status.message(), "session id unknown");
    }

    #[tokio::test]
    async fn stopping_a_paused_run_discards_it_on_its_node_and_stops_the_job() {
        let Harness { controller, node_rx, _tmp } = harness(None);
        let (job_id, agent_id, mut node_rx) = running_repo_job(&controller, node_rx).await;
        let (paused, ()) = tokio::join!(
            FleetControl::pause_agent(&controller, tonic::Request::new(pause_request(&job_id, "pause-1"))),
            node_pauses(&controller, &mut node_rx, &job_id, &agent_id, false),
        );
        assert!(paused.unwrap().into_inner().accepted);

        let stop = pb::StopAgentRequest {
            agent_id: agent_id.clone(),
            job_id: job_id.clone(),
            request_id: "stop-1".into(),
            actor: "operator".into(),
            source: "direct_cli".into(),
            ..Default::default()
        };
        let node = async {
            let CtrlPayload::Stop(command) = next_command(&mut node_rx).await else {
                panic!("expected Stop")
            };
            assert_eq!(command.discard_paused_job_id, job_id);
            assert!(command.agent_id.is_empty() && !command.pause);
            let (_, waiter) = controller.pending.remove(&command.command_id).unwrap();
            waiter
                .send(pb::CommandAck {
                    command_id: command.command_id,
                    accepted: true,
                    ..Default::default()
                })
                .unwrap();
        };
        let (response, ()) =
            tokio::join!(FleetControl::stop_agent(&controller, tonic::Request::new(stop)), node);
        assert!(response.unwrap().into_inner().success);
        let job = controller.fleet.state().load_job(&job_id).unwrap();
        assert_eq!(job.state, crate::state::JobState::Stopped);
        assert!(job.completed_at.is_some());
        assert!(controller.load_paused_session(&job_id).await.is_empty());
    }
```
- [ ] **Step 2:** `cargo test pause` → FAIL (stub returns UNIMPLEMENTED).
- [ ] **Step 3: implement.**
  - `finalize_agent`, right after the `let mut job = match self.fleet.state().load_job(..)` block (:1146-1152):
```rust
        if done.paused {
            self.record_pause(&mut job, &agent, done).await;
            return;
        }
```
    and after `job.completed_at = Some(Utc::now());` (:1196):
```rust
        if job.paused_session_stored || job.paused_at.is_some() {
            self.forget_paused_session(&job.job_id).await;
            job.paused_session_stored = false;
        }
```
```rust
    async fn record_pause(&self, job: &mut Job, agent: &RemoteAgent, done: pb::AgentCompleted) {
        job.state = JobState::Paused;
        job.live_state = Some("paused".to_string());
        job.paused_at = Some(Utc::now());
        job.last_activity_at = job.paused_at;
        job.last_event_sequence = job.last_event_sequence.max(done.last_event_sequence);
        if !done.session_id.is_empty() {
            job.session_id = Some(done.session_id.clone());
        } else if agent.session_id.is_some() {
            job.session_id = agent.session_id.clone();
        }
        job.pause_push_failed = done.push_failed;
        job.error = if !done.error_message.is_empty() {
            Some(done.error_message.clone())
        } else if done.push_failed {
            Some(format!("pause push failed; worktree kept on {}", agent.node_name))
        } else {
            None
        };
        if !done.session_file.is_empty() {
            job.paused_session_stored =
                self.store_paused_session(&job.job_id, &done.session_file).await;
        }
        if let Err(error) = self.fleet.persist_job(job).await {
            tracing::warn!(job_id = %job.job_id, %error, "failed to persist paused job");
        }
        tracing::info!(agent = %agent.agent_name, job_id = %job.job_id, node = %agent.node_name, "run paused");
    }
```
    (The `running_agents` decrement and the `log_fanout` / `log_writers` / `event_sequences` removal above :1146 already ran; no hooks fire for a non-terminal state.)
  - `handle_node_message`, first statement of the `StatusUpdate(u)` arm (:841), so a paused run's report never reaches the adoption path that would mark the job Running:
```rust
                if u.state == pb::AgentState::Paused as i32 {
                    self.record_paused_push(node_name, &u).await;
                    return;
                }
```
```rust
    async fn record_paused_push(&self, node_name: &str, update: &pb::AgentStatusUpdate) {
        if update.push_failed || update.job_id.is_empty() {
            return;
        }
        let Some(mut job) = self.load_durable_job(&update.job_id).await else {
            return;
        };
        if job.state != JobState::Paused
            || job.node.as_deref() != Some(node_name)
            || !job.pause_push_failed
        {
            return;
        }
        job.pause_push_failed = false;
        job.error = None;
        if let Err(error) = self.fleet.persist_job(&job).await {
            tracing::warn!(job_id = %job.job_id, %error, "failed to persist the landed pause push");
        }
        tracing::info!(job_id = %job.job_id, node = %node_name, "paused run's push landed; resume no longer pinned");
    }
```
  - `send_stop` (the ack code of `stop_agent` :1891-1923, extracted):
```rust
    async fn send_stop(&self, node_name: &str, command: pb::StopCommand) -> (bool, String) {
        let Some(handle) = self.nodes.get(node_name).map(|h| h.clone()) else {
            return (false, format!("node '{node_name}' not connected"));
        };
        let command_id = command.command_id.clone();
        let (ack_tx, ack_rx) = oneshot::channel();
        self.pending.insert(command_id.clone(), ack_tx);
        let message = pb::ControllerMessage {
            payload: Some(CtrlPayload::Stop(command)),
        };
        if handle.tx.send(Ok(message)).await.is_err() {
            self.pending.remove(&command_id);
            return (false, "node stream closed".to_string());
        }
        match tokio::time::timeout(COMMAND_ACK_TIMEOUT, ack_rx).await {
            Ok(Ok(ack)) => (ack.accepted, ack.error),
            _ => {
                self.pending.remove(&command_id);
                (false, "timed out waiting for node ack".to_string())
            }
        }
    }
```
  - `stop_agent`: the live-agent branch becomes `self.send_stop(&agent.node_name, pb::StopCommand { command_id: format!("cmd-{}", uuid::Uuid::new_v4()), agent_id: req.agent_id.clone(), force: req.force, ..Default::default() }).await`; `audit_job_id` falls back to `(!req.job_id.is_empty()).then(|| req.job_id.clone())`; the `else` (no live agent) branch becomes
```rust
        } else if let Some(paused) = self.paused_job_for_stop(&req).await {
            match self.discard_paused(&paused).await {
                Ok(()) => (true, String::new()),
                Err(error) => (false, error),
            }
        } else {
            (false, format!("unknown agent: {}", req.agent_id))
        };
```
```rust
    async fn paused_job_for_stop(&self, req: &pb::StopAgentRequest) -> Option<Job> {
        let job = if !req.job_id.is_empty() {
            self.load_durable_job(&req.job_id).await?
        } else {
            self.fleet.state().list_jobs().ok()?.into_iter().find(|job| {
                job.agent_id.as_deref() == Some(req.agent_id.as_str())
                    && job.state == JobState::Paused
            })?
        };
        (job.state == JobState::Paused).then_some(job)
    }

    async fn discard_paused(&self, job: &Job) -> std::result::Result<(), String> {
        let node = job.node.clone().unwrap_or_default();
        let command = pb::StopCommand {
            command_id: format!("cmd-{}", uuid::Uuid::new_v4()),
            discard_paused_job_id: job.job_id.clone(),
            ..Default::default()
        };
        let (accepted, error) = self.send_stop(&node, command).await;
        if !accepted {
            return Err(error);
        }
        let mut stopped = job.clone();
        stopped.state = JobState::Stopped;
        stopped.live_state = Some("stopped".to_string());
        stopped.completed_at = Some(Utc::now());
        stopped.paused_session_stored = false;
        self.forget_paused_session(&job.job_id).await;
        self.fleet
            .persist_job(&stopped)
            .await
            .map_err(|error| error.to_string())?;
        self.fleet.fire_hooks_for_job(&stopped.agent_name, &stopped).await;
        Ok(())
    }
```
  - `pause_agent`:
```rust
    async fn pause_agent(
        &self,
        request: Request<pb::PauseAgentRequest>,
    ) -> std::result::Result<Response<pb::PauseAgentResponse>, Status> {
        let req = request.into_inner();
        let control = mutation_context(&req.request_id, &req.actor, &req.source)?;
        if req.job_id.is_empty() || req.job_id.len() > 128 {
            return Err(Status::invalid_argument("invalid job_id"));
        }
        let job = self
            .load_durable_job(&req.job_id)
            .await
            .ok_or_else(|| Status::not_found("job not found"))?;
        if job.state == JobState::Paused {
            return Ok(Response::new(pause_response(&job, true, String::new(), control.request_id, false)));
        }
        let agent = self
            .agents
            .iter()
            .find(|agent| agent.job_id == job.job_id)
            .map(|agent| agent.clone())
            .filter(|_| job.state == JobState::Running && job.interactive && job.branch.is_some())
            .ok_or_else(|| {
                Status::failed_precondition("only a running interactive run on a branch can be paused")
            })?;
        if agent.session_id.is_none() && job.session_id.is_none() {
            return Err(Status::failed_precondition("session id unknown"));
        }
        let fingerprint =
            ControllerInner::command_fingerprint(&["pause", &job.job_id, &agent.agent_id]);
        if let Some(result) = self
            .begin_command(
                &control,
                "pause",
                &fingerprint,
                Some(&agent.agent_name),
                Some(&agent.node_name),
                Some(&agent.agent_id),
                Some(&job.job_id),
                None,
            )
            .await?
        {
            let current = self.load_durable_job(&job.job_id).await.unwrap_or(job);
            return Ok(Response::new(pause_response(
                &current,
                result.get_bool("accepted").unwrap_or(false),
                result.get_str("error").unwrap_or_default().to_string(),
                control.request_id,
                true,
            )));
        }
        let command = pb::StopCommand {
            command_id: format!("cmd-{}", uuid::Uuid::new_v4()),
            agent_id: agent.agent_id.clone(),
            pause: true,
            ..Default::default()
        };
        let (accepted, error) = self.send_stop(&agent.node_name, command).await;
        let mut current = job.clone();
        if accepted {
            let deadline = tokio::time::Instant::now() + PAUSE_SETTLE;
            while tokio::time::Instant::now() < deadline {
                if let Some(latest) = self.load_durable_job(&job.job_id).await {
                    let settled = latest.state == JobState::Paused || latest.state.is_terminal();
                    current = latest;
                    if settled {
                        break;
                    }
                }
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
        }
        self.finish_command(
            &control,
            accepted,
            doc! {
                "accepted": accepted,
                "error": &error,
                "job_id": &job.job_id,
                "state": current.state.as_str(),
            },
            (!accepted).then_some("command_rejected"),
            (!accepted).then_some(error.as_str()),
            Some(&job.job_id),
            Some(&agent.agent_id),
        )
        .await?;
        Ok(Response::new(pause_response(&current, accepted, error, control.request_id, false)))
    }
```
```rust
fn pause_response(
    job: &Job,
    accepted: bool,
    error: String,
    request_id: String,
    replayed: bool,
) -> pb::PauseAgentResponse {
    pb::PauseAgentResponse {
        job_id: job.job_id.clone(),
        state: job.state.as_str().to_string(),
        accepted,
        node_name: job.node.clone().unwrap_or_default(),
        error,
        request_id,
        replayed,
    }
}
```
- [ ] **Step 4:** green, clippy, fmt.
- [ ] **Step 5: commit**
```bash
git add src/remote/controller.rs
git commit -m "feat(controller): PauseAgent; paused completions; StopAgent discards a paused run"
```

### Task A11: Controller `ResumeAgent`

**Files:** Modify `src/remote/controller.rs`: `dispatch_remote` :540-730 (delivery and registration extracted), `resume_agent` stub (A1), free fn `resume_response`; tests.

**Interfaces — Consumes:** `schedule`, `SlotReservation`, `register_pool_node` (A5); `send_stop` (A10); node-side `discard_paused_job_id` handling (A9); `load_paused_session` (A3); `running_repo_job`, `node_pauses`, `pause_request`, `answer_next`, `next_command` (A10 tests); `SpawnCommand.resume_*` (A1).

**Produces:**
```rust
/// What became of a spawn sent to a node.
enum Delivery { Accepted(String), Rejected(String), Undelivered, Unacknowledged }
impl ControllerInner {
    /// Sends `cmd` to `handle` and waits for its acknowledgement.
    async fn deliver(&self, handle: &NodeHandle, cmd: pb::SpawnCommand) -> Delivery;
    /// Marks an acknowledged job running on `handle` and starts tracking its agent.
    async fn register_running(&self, handle: &NodeHandle, job: Job, agent_id: String, interactive: bool) -> std::result::Result<(String, String), String>;
    /// Dispatches a paused job's resume to `node_name`. A rejected resume leaves the job paused
    /// where it was.
    async fn dispatch_resume(&self, node_name: &str, job: Job) -> std::result::Result<(String, String), String>;
    /// Asks `node_name` to drop the paused worktree and marker `job_id` left there.
    async fn discard_stale_worktree(&self, node_name: &str, job_id: &str);
}
fn resume_response(job: &Job, accepted: bool, error: String, request_id: String, replayed: bool) -> pb::ResumeAgentResponse;
```

- [ ] **Step 1: failing tests**
```rust
    fn resume_request(job_id: &str, request_id: &str) -> pb::ResumeAgentRequest {
        pb::ResumeAgentRequest {
            job_id: job_id.into(),
            request_id: request_id.into(),
            actor: "operator".into(),
            source: "direct_cli".into(),
        }
    }

    /// A repo job paused on "worker". Returns (job_id, node_rx).
    async fn paused_repo_job(
        controller: &super::Controller,
        node_rx: mpsc::Receiver<CtrlMsgResult>,
        push_failed: bool,
    ) -> (String, mpsc::Receiver<CtrlMsgResult>) {
        let (job_id, agent_id, mut node_rx) = running_repo_job(controller, node_rx).await;
        let (paused, ()) = tokio::join!(
            FleetControl::pause_agent(controller, tonic::Request::new(pause_request(&job_id, "pause-1"))),
            node_pauses(controller, &mut node_rx, &job_id, &agent_id, push_failed),
        );
        assert_eq!(paused.unwrap().into_inner().state, "paused");
        (job_id, node_rx)
    }

    /// Accepts the next spawn on `rx` as `agent_id` and hands the command back.
    async fn accept_spawn(
        controller: &super::Controller,
        rx: &mut mpsc::Receiver<CtrlMsgResult>,
        agent_id: &str,
    ) -> pb::SpawnCommand {
        let CtrlPayload::Spawn(command) = next_command(rx).await else {
            panic!("expected Spawn")
        };
        let (_, waiter) = controller.pending.remove(&command.command_id).unwrap();
        waiter
            .send(pb::CommandAck {
                command_id: command.command_id.clone(),
                accepted: true,
                agent_id: agent_id.into(),
                error: String::new(),
            })
            .unwrap();
        command
    }

    #[tokio::test]
    async fn resume_on_the_pausing_node_reuses_its_worktree() {
        let Harness { controller, node_rx, _tmp } = harness(None);
        let (job_id, mut node_rx) = paused_repo_job(&controller, node_rx, false).await;
        let (response, command) = tokio::join!(
            FleetControl::resume_agent(&controller, tonic::Request::new(resume_request(&job_id, "resume-1"))),
            accept_spawn(&controller, &mut node_rx, "agent-2"),
        );
        let response = response.unwrap().into_inner();
        assert!(response.accepted, "{}", response.error);
        assert_eq!(response.state, "running");
        assert_eq!(response.node_name, "worker");
        assert_eq!(response.agent_id, "agent-2");
        assert!(command.resume_paused && command.resume_local && command.interactive);
        assert_eq!(command.job_id, job_id);
        assert_eq!(command.session_id, "s-1");
        assert_eq!(command.resume_branch, format!("agent/{job_id}"));
        assert_eq!(command.run_repo_url, REPO_URL);
        assert_eq!(command.resume_session_file, b"transcript");
        let job = controller.fleet.state().load_job(&job_id).unwrap();
        assert_eq!(job.state, crate::state::JobState::Running);
        assert!(job.paused_at.is_none());

        let again = FleetControl::resume_agent(&controller, tonic::Request::new(resume_request(&job_id, "resume-2")))
            .await
            .unwrap()
            .into_inner();
        assert!(again.accepted);
        assert_eq!(again.state, "running");
    }

    #[tokio::test]
    async fn resume_goes_to_another_pool_node_when_the_original_is_gone() {
        let Harness { controller, node_rx, _tmp } = harness(None);
        let (job_id, _old_rx) = paused_repo_job(&controller, node_rx, false).await;
        controller.nodes.remove("worker");
        let mut other_rx = register_pool_node(&controller, "worker-2", 2);
        let (response, command) = tokio::join!(
            FleetControl::resume_agent(&controller, tonic::Request::new(resume_request(&job_id, "resume-1"))),
            accept_spawn(&controller, &mut other_rx, "agent-3"),
        );
        assert_eq!(response.unwrap().into_inner().node_name, "worker-2");
        assert!(!command.resume_local);
        assert_eq!(
            controller.fleet.state().load_job(&job_id).unwrap().node.as_deref(),
            Some("worker-2")
        );
    }

    #[tokio::test]
    async fn a_resume_on_another_node_discards_the_stale_worktree_on_the_original() {
        let Harness { controller, node_rx, _tmp } = harness(None);
        let (job_id, mut worker_rx) = paused_repo_job(&controller, node_rx, false).await;
        // "worker" is full (default cap 4), so the resume goes to "worker-2".
        controller
            .nodes
            .get("worker")
            .unwrap()
            .running_interactive
            .store(4, std::sync::atomic::Ordering::SeqCst);
        let mut other_rx = register_pool_node(&controller, "worker-2", 2);
        let nodes = async {
            accept_spawn(&controller, &mut other_rx, "agent-3").await;
            let CtrlPayload::Stop(discard) = next_command(&mut worker_rx).await else {
                panic!("expected the discard on the original node")
            };
            assert_eq!(discard.discard_paused_job_id, job_id);
            assert!(discard.agent_id.is_empty() && !discard.pause);
            let (_, waiter) = controller.pending.remove(&discard.command_id).unwrap();
            waiter
                .send(pb::CommandAck { command_id: discard.command_id, accepted: true, ..Default::default() })
                .unwrap();
        };
        let (response, ()) = tokio::join!(
            FleetControl::resume_agent(&controller, tonic::Request::new(resume_request(&job_id, "resume-1"))),
            nodes,
        );
        let response = response.unwrap().into_inner();
        assert!(response.accepted, "{}", response.error);
        assert_eq!(response.node_name, "worker-2");
        assert_eq!(response.state, "running");
    }

    #[tokio::test]
    async fn a_pause_that_could_not_push_resumes_only_where_its_worktree_is() {
        let Harness { controller, node_rx, _tmp } = harness(None);
        let (job_id, _old_rx) = paused_repo_job(&controller, node_rx, true).await;
        controller.nodes.remove("worker");
        let _other_rx = register_pool_node(&controller, "worker-2", 2);
        let response = FleetControl::resume_agent(&controller, tonic::Request::new(resume_request(&job_id, "resume-1")))
            .await
            .unwrap()
            .into_inner();
        assert!(!response.accepted);
        assert!(response.error.contains("node 'worker' is not connected"), "{}", response.error);
        assert_eq!(
            controller.fleet.state().load_job(&job_id).unwrap().state,
            crate::state::JobState::Paused
        );
    }

    #[tokio::test]
    async fn a_rejected_resume_keeps_the_job_paused_and_capacity_is_resource_exhausted() {
        let Harness { controller, node_rx, _tmp } = harness(None);
        let (job_id, mut node_rx) = paused_repo_job(&controller, node_rx, false).await;
        let (response, ()) = tokio::join!(
            FleetControl::resume_agent(&controller, tonic::Request::new(resume_request(&job_id, "resume-1"))),
            answer_next(&controller, &mut node_rx, false, "spawn failed: invalid run branch"),
        );
        assert!(!response.unwrap().into_inner().accepted);
        let job = controller.fleet.state().load_job(&job_id).unwrap();
        assert_eq!(job.state, crate::state::JobState::Paused);
        assert_eq!(job.node.as_deref(), Some("worker"));
        let (status, ()) = tokio::join!(
            FleetControl::resume_agent(&controller, tonic::Request::new(resume_request(&job_id, "resume-2"))),
            answer_next(&controller, &mut node_rx, false, "spawn failed: node at capacity"),
        );
        assert_eq!(status.expect_err("capacity").code(), tonic::Code::ResourceExhausted);
    }

    #[tokio::test]
    async fn resuming_a_job_that_is_neither_paused_nor_running_is_refused() {
        let Harness { controller, _tmp, .. } = harness(None);
        let mut job = controller.fleet.new_job("repo-runner", "claude");
        job.state = crate::state::JobState::Completed;
        controller.fleet.persist_job(&job).await.unwrap();
        let status = FleetControl::resume_agent(&controller, tonic::Request::new(resume_request(&job.job_id, "resume-1")))
            .await
            .expect_err("terminal");
        assert_eq!(status.code(), tonic::Code::FailedPrecondition);
    }
```
- [ ] **Step 2:** run → FAIL.
- [ ] **Step 3: implement.**
  - In `dispatch_remote`, keep everything up to building `cmd` (:540-631); replace :633-729 with
```rust
        match self.deliver(&handle, cmd).await {
            Delivery::Accepted(agent_id) => {
                self.register_running(&handle, job, agent_id, prepared.spawn.interactive)
                    .await
            }
            Delivery::Undelivered => {
                job.state = JobState::Failed;
                job.error = Some(format!("node '{node_name}' stream closed"));
                job.completed_at = Some(Utc::now());
                let _ = self.fleet.persist_job(&job).await;
                Err(format!("node '{node_name}' stream closed"))
            }
            Delivery::Unacknowledged => {
                job.live_state = Some("unknown".to_string());
                job.error =
                    Some("timed out waiting for node ack; awaiting reconciliation".to_string());
                let _ = self.fleet.persist_job(&job).await;
                Err("timed out waiting for node ack; inspect the durable job before retrying"
                    .to_string())
            }
            Delivery::Rejected(error) => {
                job.state = JobState::Failed;
                job.error = Some(format!("node rejected spawn: {error}"));
                job.completed_at = Some(Utc::now());
                let _ = self.fleet.persist_job(&job).await;
                Err(job.error.clone().unwrap_or_default())
            }
        }
```
    `deliver` holds the old pending-insert / send / ack-timeout code (:633-674) and returns the enum. `register_running` holds the old :677-729 with these changes: `RemoteAgent.agent_name`/`backend` from `job.agent_name`/`job.backend`, `node_name: handle.registration.node_name.clone()`, `session_id: job.session_id.clone()`, `interactive` from the parameter; `job.started_at.get_or_insert_with(Utc::now)` (keeps the first start of a resumed job); `job.paused_at = None; job.error = None;`. It returns `Ok((agent_id, job.job_id))`.
```rust
    async fn dispatch_resume(
        &self,
        node_name: &str,
        mut job: Job,
    ) -> std::result::Result<(String, String), String> {
        let handle = self
            .nodes
            .get(node_name)
            .map(|h| h.clone())
            .ok_or_else(|| format!("node '{node_name}' is not connected"))?;
        let prepared = self
            .fleet
            .prepare_run(&job.agent_name, Some(""), None)
            .map_err(|e| e.to_string())?;
        let previous_node = job.node.clone();
        let command_id = format!("cmd-{}", uuid::Uuid::new_v4());
        let mut cmd = super::spawn_command(
            &job.agent_name,
            &job.backend,
            &prepared.spawn,
            &command_id,
            &job.job_id,
        );
        cmd.interactive = true;
        cmd.shared_checkout = false;
        cmd.resume_paused = true;
        cmd.session_id = job.session_id.clone().unwrap_or_default();
        cmd.run_repo_url = job.repo_url.clone().unwrap_or_default();
        cmd.resume_branch = job.branch.clone().unwrap_or_default();
        cmd.resume_local = previous_node.as_deref() == Some(node_name);
        cmd.resume_session_file = self.load_paused_session(&job.job_id).await;
        // Recorded before delivery so that this node's first status report is adopted.
        job.node = Some(node_name.to_string());
        self.fleet
            .persist_job(&job)
            .await
            .map_err(|e| format!("failed to persist resuming job: {e}"))?;
        match self.deliver(&handle, cmd).await {
            Delivery::Accepted(agent_id) => {
                let started = self.register_running(&handle, job, agent_id, true).await?;
                // The run now lives here; the node it paused on still holds a stale worktree.
                if let Some(previous) = previous_node.filter(|previous| previous != node_name) {
                    self.discard_stale_worktree(&previous, &started.1).await;
                }
                Ok(started)
            }
            Delivery::Unacknowledged => {
                job.live_state = Some("unknown".to_string());
                let _ = self.fleet.persist_job(&job).await;
                Err("timed out waiting for node ack; inspect the durable job before retrying"
                    .to_string())
            }
            Delivery::Rejected(error) => {
                job.node = previous_node;
                job.error = Some(format!("resume rejected: {error}"));
                let _ = self.fleet.persist_job(&job).await;
                Err(format!("node rejected resume: {error}"))
            }
            Delivery::Undelivered => {
                job.node = previous_node;
                let _ = self.fleet.persist_job(&job).await;
                Err(format!("node '{node_name}' stream closed"))
            }
        }
    }
```
    (`job.repo_url` is the credential-free form; the node's `find_repository` compares normalized urls, as for moved jobs.)
```rust
    /// Asks `node_name` to drop the paused worktree and marker `job_id` left there. A node that is
    /// gone or refuses is only logged: the job runs elsewhere either way.
    async fn discard_stale_worktree(&self, node_name: &str, job_id: &str) {
        let command = pb::StopCommand {
            command_id: format!("cmd-{}", uuid::Uuid::new_v4()),
            discard_paused_job_id: job_id.to_string(),
            ..Default::default()
        };
        let (accepted, error) = self.send_stop(node_name, command).await;
        if !accepted {
            tracing::warn!(%job_id, node = %node_name, %error, "stale paused worktree not discarded");
        }
    }
```
  - `resume_agent`:
```rust
    async fn resume_agent(
        &self,
        request: Request<pb::ResumeAgentRequest>,
    ) -> std::result::Result<Response<pb::ResumeAgentResponse>, Status> {
        let req = request.into_inner();
        let control = mutation_context(&req.request_id, &req.actor, &req.source)?;
        if req.job_id.is_empty() || req.job_id.len() > 128 {
            return Err(Status::invalid_argument("invalid job_id"));
        }
        let job = self
            .load_durable_job(&req.job_id)
            .await
            .ok_or_else(|| Status::not_found("job not found"))?;
        if matches!(job.state, JobState::Running | JobState::Pending) {
            return Ok(Response::new(resume_response(&job, true, String::new(), control.request_id, false)));
        }
        if job.state != JobState::Paused {
            return Err(Status::failed_precondition("job is not paused"));
        }
        let Some(session_id) = job.session_id.clone() else {
            return Err(Status::failed_precondition("session id unknown"));
        };
        let resolved = self
            .fleet
            .agent(&job.agent_name)
            .filter(|agent| agent.config.operator_control.launchable)
            .ok_or_else(|| Status::permission_denied("agent is not operator-launchable"))?;
        let pool = resolved.config.node.clone().ok_or_else(|| {
            Status::failed_precondition("FleetControl launch requires a configured remote agent node")
        })?;
        let paused_at = job.paused_at.map(|at| at.to_rfc3339()).unwrap_or_default();
        let fingerprint = ControllerInner::command_fingerprint(&["resume", &job.job_id, &session_id, &paused_at]);
        if let Some(result) = self
            .begin_command(&control, "resume", &fingerprint, Some(&job.agent_name), Some(&pool), None, Some(&job.job_id), None)
            .await?
        {
            let success = result.get_bool("success").unwrap_or(false);
            let error = result.get_str("error").unwrap_or_default().to_string();
            if !success && error.ends_with(NODE_AT_CAPACITY) {
                return Err(Status::resource_exhausted(NODE_AT_CAPACITY));
            }
            let current = self.load_durable_job(&job.job_id).await.unwrap_or(job);
            return Ok(Response::new(resume_response(&current, success, error, control.request_id, true)));
        }
        let pinned = if job.pause_push_failed { job.node.clone() } else { None };
        let (target_node, dispatch) = match self.schedule(&pool, pinned.as_deref(), true).await {
            Ok((node, _reservation)) => {
                let dispatch = self.dispatch_resume(&node, job.clone()).await;
                (node, dispatch)
            }
            Err(error) => (pool.clone(), Err(error)),
        };
        let launched = self.finish_launch(&control, target_node, dispatch).await?;
        let current = self.load_durable_job(&req.job_id).await.unwrap_or(job);
        Ok(Response::new(pb::ResumeAgentResponse {
            agent_id: launched.agent_id,
            ..resume_response(&current, launched.success, launched.error, control.request_id, false)
        }))
    }
```
    (`finish_launch` audits the outcome and maps a `node at capacity` error to RESOURCE_EXHAUSTED, also when it is the node's NACK.)
```rust
fn resume_response(
    job: &Job,
    accepted: bool,
    error: String,
    request_id: String,
    replayed: bool,
) -> pb::ResumeAgentResponse {
    pb::ResumeAgentResponse {
        job_id: job.job_id.clone(),
        state: job.state.as_str().to_string(),
        accepted,
        node_name: job.node.clone().unwrap_or_default(),
        error,
        request_id,
        replayed,
        agent_id: job.agent_id.clone().unwrap_or_default(),
    }
}
```
- [ ] **Step 4:** green (every existing spawn/import test exercises the refactored delivery), clippy, fmt.
- [ ] **Step 5: commit**
```bash
git add src/remote/controller.rs
git commit -m "feat(controller): ResumeAgent on the pool, pinned when the pause could not push"
```

### Task A12: CLI `pause`, `resume`, `job`

**Files:** Modify `src/remote/client.rs` (after `stop` :136-153), `src/main.rs` (`Commands` after `Stop` :78-83, dispatch :223, after `cmd_stop` :660-669, test module at the end of the file).

**Interfaces — Produces:**
```rust
impl FleetClient {
    pub async fn pause(&mut self, job_id: &str) -> Result<pb::PauseAgentResponse>;
    pub async fn resume(&mut self, job_id: &str) -> Result<pb::ResumeAgentResponse>;
}
// main.rs
Commands::Pause { job_id: String }
Commands::Resume { job_id: String }
Commands::Job { job_id: String }
```

- [ ] **Step 1: failing test** (end of `src/main.rs`; use the clap root type and its subcommand field as declared at the top of `main.rs`):
```rust
#[cfg(test)]
mod pause_cli_tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn pause_resume_and_job_take_a_job_id() {
        for verb in ["pause", "resume", "job"] {
            let cli = Cli::try_parse_from(["agent-ctrl", verb, "job-1"]).unwrap();
            let job_id = match (verb, cli.command) {
                ("pause", Commands::Pause { job_id })
                | ("resume", Commands::Resume { job_id })
                | ("job", Commands::Job { job_id }) => job_id,
                (verb, _) => panic!("{verb} parsed to the wrong command"),
            };
            assert_eq!(job_id, "job-1");
        }
    }
}
```
- [ ] **Step 2:** `cargo test pause_resume_and_job` → FAIL.
- [ ] **Step 3: implement.**
```rust
// client.rs
    pub async fn pause(&mut self, job_id: &str) -> Result<pb::PauseAgentResponse> {
        let (request_id, actor, source) = direct_cli_context();
        let request = self.authed(pb::PauseAgentRequest {
            job_id: job_id.to_string(),
            request_id,
            actor,
            source,
        });
        let resp = self.inner.pause_agent(request).await.map_err(RemoteError::Status)?;
        Ok(resp.into_inner())
    }

    pub async fn resume(&mut self, job_id: &str) -> Result<pb::ResumeAgentResponse> {
        let (request_id, actor, source) = direct_cli_context();
        let request = self.authed(pb::ResumeAgentRequest {
            job_id: job_id.to_string(),
            request_id,
            actor,
            source,
        });
        let resp = self.inner.resume_agent(request).await.map_err(RemoteError::Status)?;
        Ok(resp.into_inner())
    }
```
```rust
// main.rs, Commands
    /// Pause an interactive run: its work is pushed and its session kept for a resume.
    Pause { job_id: String },
    /// Resume a paused run on a free node of its pool.
    Resume { job_id: String },
    /// Print one job as the controller records it, as JSON.
    Job { job_id: String },

// dispatch
        Commands::Pause { job_id } => cmd_pause(job_id).await,
        Commands::Resume { job_id } => cmd_resume(job_id).await,
        Commands::Job { job_id } => cmd_job(job_id).await,

async fn cmd_pause(job_id: String) -> Result<()> {
    let mut client = FleetClient::connect(controller_address()).await?;
    let resp = client.pause(&job_id).await?;
    if !resp.accepted {
        anyhow::bail!("pause failed: {}", resp.error);
    }
    println!("{job_id} {} on {}", resp.state, resp.node_name);
    Ok(())
}

async fn cmd_resume(job_id: String) -> Result<()> {
    let mut client = FleetClient::connect(controller_address()).await?;
    let resp = client.resume(&job_id).await?;
    if !resp.accepted {
        anyhow::bail!("resume failed: {}", resp.error);
    }
    println!("{job_id} {} on {} as {}", resp.state, resp.node_name, resp.agent_id);
    Ok(())
}

async fn cmd_job(job_id: String) -> Result<()> {
    let mut client = FleetClient::connect(controller_address()).await?;
    let job = client
        .get_job(&job_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("job not found"))?;
    let record = serde_json::json!({
        "job_id": job.job_id,
        "state": job.state,
        "node_name": job.node_name,
        "agent_id": job.agent_id,
        "session_id": job.session_id,
        "branch": job.branch,
        "error": job.error,
        "paused_at": job.paused_at.map(|t| t.seconds),
        "pause_push_failed": job.pause_push_failed,
    });
    println!("{record}");
    Ok(())
}
```
- [ ] **Step 4:** green, clippy, fmt.
- [ ] **Step 5: commit**
```bash
git add src/remote/client.rs src/main.rs
git commit -m "feat(cli): agent-ctrl pause, resume and job"
```

### Task A13: Render the node pool as a StatefulSet (opt-in per instance)

**Files:** Modify `deploy/k8s/templates/node-deployment.tmpl.yaml` (:3 kind, :13-15 replicas/strategy, :118 node name, :119 env, :172-174 workspace volume, :194 before tolerations, end of file), `deploy/k8s/render.py` (`_ORDER` :43-59, `_DEFAULTS` :62-120, `load_instance` :322-337, `_subs` :403-483, `_skipped_resources` :485-496, `main` :554-596), `deploy/k8s/ci-deployer-rbac.yaml` (:67-69, :87-89), `.gitea/workflows/deploy.yml` (:147-159), `.gitlab-ci.yml` (:143-151), `deploy/staging/obachan-k8s/workspace-sync.yaml` (regenerated), `deploy/k8s/test_shared_workspaces.py`; create `deploy/k8s/templates/node-service.tmpl.yaml`, `deploy/k8s/templates/workspace-sync-configmap.yaml`, `deploy/k8s/templates/workspace-sync-cronjob.tmpl.yaml`.

**Interfaces — Produces:**
```python
_DEFAULTS["node_workload"] = "deployment"   # or "statefulset"
def _node_workload_subs(cfg: dict) -> dict: ...
def workspace_sync_yaml(cfg: dict) -> str: ...  # ConfigMap, then one CronJob per node volume
# CLI: python deploy/k8s/render.py --instance <name> --workspace-sync-yaml
# CLI: python deploy/k8s/render.py --instance <name> --node-rollout-target   # statefulset/agent-ctrl-node | deploy/agent-ctrl-node
```

- [ ] **Step 1: failing tests** (in `RenderTests`)
```python
    def statefulset_cfg(self, replicas=2):
        cfg = render.load_instance("staging", "test-image")
        cfg["node_workload"] = "statefulset"
        cfg["node_replicas"] = replicas
        return cfg

    def test_statefulset_pool_has_claims_headless_service_and_anti_affinity(self):
        docs = dict(render.render(self.statefulset_cfg()))
        node = docs["node-deployment"]
        self.assertIn("kind: StatefulSet", node)
        self.assertIn("serviceName: agent-ctrl-node", node)
        self.assertIn("replicas: 2", node)
        self.assertIn("volumeClaimTemplates:", node)
        self.assertIn("storage: 50Gi", node)
        self.assertIn("storageClassName: hcloud-volumes", node)
        self.assertIn("podAntiAffinity:", node)
        self.assertIn("requiredDuringSchedulingIgnoredDuringExecution:", node)
        self.assertIn("topologyKey: kubernetes.io/hostname", node)
        self.assertIn('--node-name "$POD_NAME"', node)
        self.assertIn("fieldPath: metadata.name", node)
        self.assertNotIn("claimName: agent-ctrl-workspace", node)
        self.assertNotIn("strategy:", node)
        self.assertIn("memory: 12Gi", node)
        self.assertNotIn("pvc", docs)
        service = docs["node-service"]
        self.assertIn("name: agent-ctrl-node", service)
        self.assertIn("clusterIP: None", service)

    def test_deployment_instances_are_unchanged(self):
        for instance in ("staging", "prod"):
            docs = dict(render.render(render.load_instance(instance, "test-image")))
            self.assertIn("kind: Deployment", docs["node-deployment"])
            self.assertIn("claimName: agent-ctrl-workspace", docs["node-deployment"])
            self.assertIn("--node-name agent-ctrl-node", docs["node-deployment"])
            self.assertIn("pvc", docs)
            self.assertNotIn("node-service", docs)

    def test_workspace_sync_has_one_cronjob_per_node_volume(self):
        out = render.workspace_sync_yaml(self.statefulset_cfg())
        self.assertEqual(out.count("kind: CronJob"), 2)
        for i in (0, 1):
            self.assertIn(f"name: agent-ctrl-workspace-sync-{i}\n", out)
            self.assertIn(f"claimName: shared-checkouts-agent-ctrl-node-{i}", out)
            self.assertIn(f"statefulset.kubernetes.io/pod-name: agent-ctrl-node-{i}", out)
        self.assertIn('schedule: "17 3 * * *"', out)
        self.assertIn('schedule: "47 3 * * *"', out)
        self.assertIn('flock -w 600 "$lock" git -C "$dest" fetch --prune --quiet', out)
        single = render.workspace_sync_yaml(render.load_instance("staging", "test-image"))
        self.assertEqual(single.count("kind: CronJob"), 1)
        self.assertIn("name: agent-ctrl-workspace-sync\n", single)
        self.assertIn("claimName: agent-ctrl-workspace", single)

    def test_committed_workspace_sync_matches_render_output(self):
        import subprocess, sys
        out = subprocess.run(
            [sys.executable, str(Path(__file__).parent / "render.py"), "--instance", "staging", "--workspace-sync-yaml"],
            check=True, capture_output=True,
        ).stdout.decode()
        root = Path(__file__).resolve().parents[2]
        committed = (root / "deploy/staging/obachan-k8s/workspace-sync.yaml").read_bytes().decode()
        self.assertEqual(committed, out)

    def test_ci_can_deploy_statefulsets(self):
        rbac = (Path(__file__).parent / "ci-deployer-rbac.yaml").read_text()
        self.assertIn('resources: ["deployments", "statefulsets"]', rbac)
        self.assertIn('resources: ["replicasets", "controllerrevisions"]', rbac)
        root = Path(__file__).resolve().parents[2]
        for workflow in (".gitea/workflows/deploy.yml", ".gitlab-ci.yml"):
            text = (root / workflow).read_text()
            self.assertIn("can-i patch statefulsets", text)
            self.assertIn("--node-rollout-target", text)
            self.assertNotIn("rollout status deploy/agent-ctrl-node", text)

    def test_node_rollout_target_follows_the_workload(self):
        import subprocess, sys
        def target(instance):
            return subprocess.run(
                [sys.executable, str(Path(__file__).parent / "render.py"), "--instance", instance, "--node-rollout-target"],
                check=True, capture_output=True,
            ).stdout.decode().strip()
        self.assertEqual(target("prod"), "deploy/agent-ctrl-node")
        expected = "statefulset/agent-ctrl-node" if render.load_instance("staging", None)["node_workload"] == "statefulset" else "deploy/agent-ctrl-node"
        self.assertEqual(target("staging"), expected)
```
- [ ] **Step 2:** `python -m unittest discover -s deploy/k8s -p "test_*.py"` → FAIL.
- [ ] **Step 3: implement.**
  - `node-deployment.tmpl.yaml`: line 3 `kind: {node_kind}`; replace :13-15 with
```yaml
  replicas: {replicas}
{node_workload_block}  selector:
```
    (the existing `selector:` line follows); line 118 `--node-name {node_name_arg}`; directly under `env:` (:119) insert a line `{node_pod_name_env}`-only placeholder (no leading spaces, the substitution carries its own indentation); replace the volume at :172-174 with `{node_workspace_volume}` placed immediately after `{gitlab_volume}` on the same line; insert `{node_affinity_block}` on its own line (no indentation) before `      tolerations:` (:194); append `{node_claim_templates}` as the last line.
  - `render.py`:
```python
def _node_workload_subs(cfg: dict) -> dict:
    if cfg["node_workload"] == "statefulset":
        return {
            "node_kind": "StatefulSet",
            "node_workload_block": (
                "  serviceName: agent-ctrl-node\n"
                "  podManagementPolicy: Parallel\n"
                "  updateStrategy:\n    type: RollingUpdate\n"
            ),
            "node_name_arg": '"$POD_NAME"',
            "node_pod_name_env": (
                "            # The pod hostname (agent-ctrl-node-<n>) is the node name; the agents'\n"
                "            # `node:` value is the pool prefix.\n"
                "            - name: POD_NAME\n"
                "              valueFrom:\n"
                "                fieldRef:\n"
                "                  fieldPath: metadata.name\n"
            ),
            "node_workspace_volume": "",
            "node_affinity_block": (
                "      affinity:\n"
                "        podAntiAffinity:\n"
                "          requiredDuringSchedulingIgnoredDuringExecution:\n"
                "            - topologyKey: kubernetes.io/hostname\n"
                "              labelSelector:\n"
                "                matchLabels:\n"
                "                  app.kubernetes.io/name: agent-ctrl\n"
                "                  app.kubernetes.io/component: node\n"
            ),
            "node_claim_templates": (
                "  volumeClaimTemplates:\n"
                "    - metadata:\n"
                "        name: shared-checkouts\n"
                "        labels:\n"
                "          app.kubernetes.io/name: agent-ctrl\n"
                "      spec:\n"
                '        accessModes: ["ReadWriteOnce"]\n'
                f"        storageClassName: {cfg['shared_checkouts_storage_class']}\n"
                "        resources:\n"
                "          requests:\n"
                f"            storage: {cfg['shared_checkouts_storage_size']}\n"
            ),
        }
    return {
        "node_kind": "Deployment",
        "node_workload_block": "  strategy:\n    type: Recreate\n",
        "node_name_arg": cfg["node_name"],
        "node_pod_name_env": "",
        "node_workspace_volume": (
            "        - name: shared-checkouts\n"
            "          persistentVolumeClaim:\n"
            f"            claimName: {cfg['shared_checkouts_claim']}\n"
        ),
        "node_affinity_block": "",
        "node_claim_templates": "",
    }
```
    `_subs` returns `{**cfg, …existing keys…, **_node_workload_subs(cfg)}`. `_DEFAULTS` gains `"node_workload": "deployment"`. `load_instance` (after merging defaults): `if cfg["node_workload"] not in ("deployment", "statefulset"): raise SystemExit(f"node_workload must be deployment or statefulset, not {cfg['node_workload']!r}")`. `_ORDER` gains `("node-service.tmpl.yaml", "node-service")` right after `("service.tmpl.yaml", "service")`. `_skipped_resources`: `skip.add("pvc") if cfg["node_workload"] == "statefulset" else skip.add("node-service")`.
  - `node-service.tmpl.yaml`:
```yaml
# SPDX-License-Identifier: Apache-2.0
# Headless governing Service of the agent-ctrl-node StatefulSet. Nodes dial out to the controller,
# so it exposes no ports.
apiVersion: v1
kind: Service
metadata:
  name: agent-ctrl-node
  namespace: {namespace}
  labels:
    app.kubernetes.io/name: agent-ctrl
    app.kubernetes.io/component: node
spec:
  clusterIP: None
  selector:
    app.kubernetes.io/name: agent-ctrl
    app.kubernetes.io/component: node
```
  - `workspace-sync-configmap.yaml`: lines 1-94 of today's `deploy/staging/obachan-k8s/workspace-sync.yaml` verbatim, with the header's "Apply order" paragraph replaced by "Generated by `render.py --instance staging --workspace-sync-yaml`: this ConfigMap, then one CronJob per node workspace volume." It is copied, never `str.format`ed.
  - `workspace-sync-cronjob.tmpl.yaml`: lines 96-198 of today's file (`apiVersion: batch/v1` … end) with `name: {sync_name}` (:99), `schedule: "{sync_schedule}"` (:105), `claimName: {sync_claim}` (:186), `emptyDir: {{}}` (:194), and a line `{sync_affinity}` (no indentation) immediately before `          tolerations:` (:195).
  - `render.py`:
```python
def workspace_sync_yaml(cfg: dict) -> str:
    """The workspace-sync ConfigMap, then one CronJob per node workspace volume."""
    configmap = (_TEMPLATES / "workspace-sync-configmap.yaml").read_text(encoding="utf-8")
    cronjob = (_TEMPLATES / "workspace-sync-cronjob.tmpl.yaml").read_text(encoding="utf-8")
    if cfg["node_workload"] == "statefulset":
        volumes = []
        for i in range(cfg["node_replicas"]):
            minute = 17 + 30 * i
            # RWO volumes: the sync pod must run on the host its node pod has the volume on.
            affinity = (
                "          affinity:\n"
                "            podAffinity:\n"
                "              requiredDuringSchedulingIgnoredDuringExecution:\n"
                "                - topologyKey: kubernetes.io/hostname\n"
                "                  labelSelector:\n"
                "                    matchLabels:\n"
                f"                      statefulset.kubernetes.io/pod-name: agent-ctrl-node-{i}\n"
            )
            volumes.append((
                f"agent-ctrl-workspace-sync-{i}",
                f"shared-checkouts-agent-ctrl-node-{i}",
                f"{minute % 60} {3 + minute // 60} * * *",
                affinity,
            ))
    else:
        volumes = [("agent-ctrl-workspace-sync", cfg["shared_checkouts_claim"], "17 3 * * *", "")]
    jobs = [
        cronjob.format(sync_name=name, sync_claim=claim, sync_schedule=schedule, sync_affinity=affinity)
        for name, claim, schedule, affinity in volumes
    ]
    return "---\n".join([configmap.rstrip("\n") + "\n", *jobs])
```
    `main`: `ap.add_argument("--workspace-sync-yaml", action="store_true", help="print the workspace-sync ConfigMap and CronJobs and exit")`; handled like `--workspace-repos-txt`: `sys.stdout.buffer.write(workspace_sync_yaml(cfg).encode("utf-8")); return 0`. Regenerate the committed file (staging is still `deployment`, so it keeps one CronJob named `agent-ctrl-workspace-sync` and `test_workspace_sync_uses_scoped_gitea_credentials_for_canonical_stozn_repos` stays green):
```bash
python deploy/k8s/render.py --instance staging --workspace-sync-yaml > deploy/staging/obachan-k8s/workspace-sync.yaml
```
  - `deploy/k8s/ci-deployer-rbac.yaml` (CI must create and watch the StatefulSet):
    - :67-69 `- apiGroups: ["apps"]` / `resources: ["deployments"]` → `resources: ["deployments", "statefulsets"]` (verbs unchanged: get, list, watch, create, patch, update).
    - :84-89: the comment becomes "`kubectl rollout status` reads the ReplicaSets behind a Deployment and the ControllerRevisions behind a StatefulSet, and pods are needed to diagnose a rollout that does not converge." and `resources: ["replicasets"]` → `resources: ["replicasets", "controllerrevisions"]`.
  - `render.py` gains `--node-rollout-target`: prints `statefulset/agent-ctrl-node` when the instance's `node_workload` is `statefulset`, else `deploy/agent-ctrl-node` (prod and, until PR 2, staging), so both workflows check the workload the render produced:
```python
    ap.add_argument("--node-rollout-target", action="store_true", help="print the node workload for `kubectl rollout status` and exit")
    ...
    if args.node_rollout_target:
        print("statefulset/agent-ctrl-node" if cfg["node_workload"] == "statefulset" else "deploy/agent-ctrl-node")
        return 0
```  - `.gitea/workflows/deploy.yml`:
    - after :147 (`kubectl auth can-i patch deployments …`) add `          kubectl auth can-i patch statefulsets -n "$namespace" | grep -Fx yes`
    - replace :151 `kubectl -n "$namespace" rollout status deploy/agent-ctrl-node --timeout=600s` with
```bash
          kubectl -n "$namespace" rollout status "$(python3 deploy/k8s/render.py --instance "$instance" --node-rollout-target)" --timeout=600s
```
      (after PR 2 this is `kubectl -n agent-ctrl rollout status statefulset/agent-ctrl-node --timeout=600s` on staging)
    - replace :157-158 (the `sed … deploy/staging/obachan-k8s/workspace-sync.yaml | kubectl apply -f -`) with
```bash
            python3 deploy/k8s/render.py --instance staging --workspace-sync-yaml \
              | sed "s#pseekoo-stoz3n-agent-ctrl/app:staging#pseekoo-stoz3n-agent-ctrl/app:$DEPLOY_IMAGE_TAG#" \
              | kubectl apply -f -
```
  - `.gitlab-ci.yml`:
    - after :143 add `    - kubectl auth can-i patch statefulsets -n "$NAMESPACE" || { echo "the CI ServiceAccount cannot deploy StatefulSets to $NAMESPACE; re-apply deploy/k8s/ci-deployer-rbac.yaml" >&2; exit 1; }`
    - :151 `if ! kubectl -n "$NAMESPACE" rollout status deploy/agent-ctrl-node --timeout=240s; then` → `if ! kubectl -n "$NAMESPACE" rollout status "$(python3 deploy/k8s/render.py --instance "$INSTANCE" --node-rollout-target)" --timeout=240s; then` (the diagnostics block that follows selects pods by label and works for both kinds).
- [ ] **Step 4:** Python tests green; `python deploy/k8s/render.py --instance staging --print` and `--instance prod --print` succeed; `cargo test rendered_instance_workspaces_pass_node_validation` (workspace.rs :714) green.
- [ ] **Step 5: commit and push PR-1 branch**
```bash
git add deploy/k8s/templates/node-deployment.tmpl.yaml deploy/k8s/templates/node-service.tmpl.yaml deploy/k8s/templates/workspace-sync-configmap.yaml deploy/k8s/templates/workspace-sync-cronjob.tmpl.yaml deploy/k8s/render.py deploy/k8s/ci-deployer-rbac.yaml deploy/k8s/test_shared_workspaces.py deploy/staging/obachan-k8s/workspace-sync.yaml .gitea/workflows/deploy.yml .gitlab-ci.yml
git commit -m "deploy: optional StatefulSet node pool with per-node workspaces and sync jobs"
git push -u origin feat/pause-and-second-node
```

### Task A14: Staging flips to a two-node StatefulSet (PR 2)

**Files:** Modify `deploy/k8s/instances/staging.toml` (after :29), `deploy/staging/obachan-k8s/workspace-sync.yaml` (regenerated), `deploy/staging/obachan-k8s/README.md` (sync section), `deploy/k8s/test_shared_workspaces.py` (:81-83 and a new test).

Branch, after PR 1 is merged: `git -C T:/projects/SOTZ3N/stoz3n-agent-ctrl fetch origin && git -C T:/projects/SOTZ3N/stoz3n-agent-ctrl worktree add -b deploy/staging-node-statefulset T:/projects/SOTZ3N/stoz3n-agent-ctrl-statefulset origin/dev`.

- [ ] **Step 1: failing test**
```python
    def test_staging_runs_two_statefulset_nodes_with_eight_slots_each(self):
        cfg = render.load_instance("staging", "test-image")
        self.assertEqual(
            (cfg["node_workload"], cfg["node_replicas"], cfg["max_interactive_runs"]),
            ("statefulset", 2, 8),
        )
        node = dict(render.render(cfg))["node-deployment"]
        self.assertIn("kind: StatefulSet", node)
        self.assertIn("replicas: 2", node)
        self.assertIn('- name: AGENT_CTRL_MAX_INTERACTIVE_RUNS' + chr(10) + '              value: "8"', node)
        self.assertIn("memory: 12Gi", node)
        self.assertEqual(render.load_instance("prod", "test-image")["node_workload"], "deployment")
```
  In `test_both_instances_have_stable_node_owned_checkouts` (:81-83) assert `"volumeClaimTemplates:"` for staging and `"persistentVolumeClaim:"` for prod.
- [ ] **Step 2:** run → FAIL.
- [ ] **Step 3: implement.** `staging.toml`, after `max_interactive_runs = 8`:
```toml
# Two node pods, each with its own workspace volume (shared-checkouts-agent-ctrl-node-<n>) on its
# own host: 16 interactive slots. Agents keep `node: agent-ctrl-node`, now the pool prefix.
node_workload = "statefulset"
node_replicas = 2
```
  Regenerate: `python deploy/k8s/render.py --instance staging --workspace-sync-yaml > deploy/staging/obachan-k8s/workspace-sync.yaml`. README sync section: "One CronJob per node volume (`agent-ctrl-workspace-sync-<n>`); run one by hand with `kubectl create job -n agent-ctrl --from=cronjob/agent-ctrl-workspace-sync-0 workspace-sync-manual-<n>`."
- [ ] **Step 4:** Python tests green.
- [ ] **Step 5: commit and push (merge only in Task A15 step 6)**
```bash
git add deploy/k8s/instances/staging.toml deploy/staging/obachan-k8s/workspace-sync.yaml deploy/staging/obachan-k8s/README.md deploy/k8s/test_shared_workspaces.py
git commit -m "deploy(staging): two agent-ctrl nodes as a StatefulSet with per-node workspaces"
git push -u origin deploy/staging-node-statefulset
```

### Task A15: Rollout (operator; each merge needs the user's go-ahead — merging to dev deploys staging)

Shell setup (Git Bash):
```bash
KC="kubectl --server https://staging-gateway:6443 --tls-server-name 10.96.0.1"
K="$KC -n agent-ctrl"
CTL="$K exec deploy/agent-ctrl-controller -- agent-ctrl"   # the controller pod carries AGENT_CTRL_CONTROL_AUTH_TOKEN
TOKEN=$(python -c "import json,os;print(json.load(open(os.path.expanduser('~/.stozn.json')))['token'])")
D=https://dash-backend-dev.stozn.ai/api/agent-control
```

- [ ] **1. PR 1** (code + render support; staging still one Deployment):
```bash
cd T:/projects/SOTZ3N/stoz3n-agent-ctrl-pause
cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt -- --check && python -m unittest discover -s deploy/k8s -p "test_*.py"
tea pr create --login obachan-dev-budelius --repo stozn/stoz3n-agent-ctrl --head feat/pause-and-second-node --base dev \
  --title "feat: pause/resume runs and schedule on a node pool" \
  --description "Spec A of 2026-10-07-agentctrl-pause-and-second-node-design: PauseAgent/ResumeAgent, JobState::Paused, pool scheduling by free interactive slots, repositories union, optional StatefulSet render (staging flips in a follow-up PR)."
```
  After the go-ahead and merge, watch the Gitea deploy workflow, then:
```bash
$K rollout status deploy/agent-ctrl-controller --timeout=300s
$K rollout status deploy/agent-ctrl-node --timeout=600s
$CTL nodes                                    # agent-ctrl-node, connected
```
- [ ] **2. Re-apply the CI RBAC** (the ClusterRole gains `statefulsets`, `controllerrevisions`; use the ServiceAccount named in the file's `RoleBinding` subject):
```bash
$KC apply -f deploy/k8s/ci-deployer-rbac.yaml
$KC auth can-i patch statefulsets -n agent-ctrl --as system:serviceaccount:agent-ctrl:agent-ctrl-deployer
```
- [ ] **3. Two sandbox hosts exist** (the pods keep the bwrap nodeSelector and must land on different hosts). This gate runs before any run is paused:
```bash
$KC get nodes -l obachan.dev/agent-sandbox=bwrap-v1
hosts=$($KC get nodes -l obachan.dev/agent-sandbox=bwrap-v1 -o name | wc -l)
if [ "$hosts" -lt 2 ]; then
  echo "STOP: only $hosts host(s) labelled obachan.dev/agent-sandbox=bwrap-v1; the second agent-ctrl-node pod cannot schedule." >&2
  echo "Prepare a second host with the reviewed bwrap-v1 prerequisites (seccomp profile profiles/agent-bwrap-v1.json), label it obachan.dev/agent-sandbox=bwrap-v1, then restart this rollout at step 3." >&2
fi
```
  With fewer than two hosts, stop here and hand the message to the user; preparing and labelling the host is theirs to do. Never set `node_bwrap_sandbox = false` to get past this gate.
- [ ] **4. Pause every live run:**
```bash
curl -fsS -H "Authorization: Bearer $TOKEN" "$D/runs?state=running&limit=100" \
  | python -c "import json,sys;[print(r['job_id']) for r in json.load(sys.stdin)['items']]" > /tmp/live-jobs.txt
for job in $(cat /tmp/live-jobs.txt); do $CTL pause "$job" || echo "PAUSE FAILED $job"; done
for job in $(cat /tmp/live-jobs.txt); do $CTL job "$job"; done
```
  Every job must print `"state":"paused"` and `"pause_push_failed":false`. A run that cannot be paused (`session id unknown`, or a shared-checkout inspector) is stopped with `$CTL stop <agent_id>` after telling the user which. A run with `"pause_push_failed":true` blocks the switch: the node retries its push every 300 s (`AGENT_CTRL_PROGRESS_PUSH_SECS`) and the flag clears once one lands; re-check with `$CTL job <job>`, and if it stays `true` after two intervals, report the job and its `error` to the user and wait for the cause (credentials, a rejected branch) to be fixed.
- [ ] **5. Retire the old Deployment and sync CronJob** (the PVC stays one more day):
```bash
$K delete deployment agent-ctrl-node
$K delete cronjob agent-ctrl-workspace-sync
```
- [ ] **6. PR 2** (Task A14), after the go-ahead:
```bash
tea pr create --login obachan-dev-budelius --repo stozn/stoz3n-agent-ctrl --head deploy/staging-node-statefulset --base dev \
  --title "deploy(staging): two agent-ctrl nodes as a StatefulSet" \
  --description "Every live run is paused; the old Deployment and sync CronJob are deleted. Resume-all follows the deploy."
```
  Merge, then:
```bash
$K rollout status statefulset/agent-ctrl-node --timeout=900s
$K get pods -l app.kubernetes.io/component=node -o wide     # two pods on two different hosts
$K get pvc                                                    # shared-checkouts-agent-ctrl-node-0/1 Bound; agent-ctrl-workspace still present
$CTL nodes                                                    # agent-ctrl-node-0 and agent-ctrl-node-1
curl -fsS -H "Authorization: Bearer $TOKEN" "$D/repos" \
  | python -c "import json,sys;[print(r['name'],r['error']) for r in json.load(sys.stdin)['items'] if not r['reachable']]"
```
  The repos check prints nothing (every repository reachable on both nodes).
- [ ] **7. Resume every paused run:**
```bash
for job in $(cat /tmp/live-jobs.txt); do $CTL resume "$job" || echo "RESUME FAILED $job"; done
```
  Each prints `running on agent-ctrl-node-<n> as <agent>`; open one run per backend in the dashboard viewer and confirm it answers.
- [ ] **8. One day later** (on or after 2026-10-08) delete the old volume: `$K delete pvc agent-ctrl-workspace`.

### Task A16: Staging E2E

Shell setup as in Task A15.

- [ ] **1. Both nodes registered:** `$CTL nodes` lists `agent-ctrl-node-0` and `agent-ctrl-node-1`; note each node's `AGENTS` as the baseline.
- [ ] **2. Start 9 IWAKAN runs:**
```bash
: > /tmp/e2e-jobs.txt
i=0
for repo in be-curious.io kaizen martin-wieser.me me mist-kibl.at pseekoo.ai pseekoo.at pseekoo.com pseekoos-economics-and-philosophies; do
  i=$((i+1))
  curl -fsS -X POST "$D/runs" -H "Authorization: Bearer $TOKEN" -H "content-type: application/json" \
    -H "idempotency-key: e2e-pause-20261007-$i" \
    -d "{\"agent_name\":\"staging-claude-interactive-rw\",\"interactive\":true,\"repo_url\":\"https://gitlab.com/pseekoo/$repo.git\"}" \
    | python -c "import json,sys;print(json.load(sys.stdin)['job_id'])" >> /tmp/e2e-jobs.txt
done
for job in $(cat /tmp/e2e-jobs.txt); do $CTL job "$job"; done
$CTL nodes
```
  All nine are `running`, each with a `session_id` (Claude's node-chosen id), and they spread across both nodes (most free slots first): both `node_name` values occur and, baseline included, the two nodes' interactive counts differ by at most one.
- [ ] **2b. The seventeenth interactive run fails with `node at capacity`.** Fill both nodes (8 + 8 = 16 slots) and expect the next start to be refused:
```bash
: > /tmp/e2e-fill.txt
n=0
while :; do
  n=$((n+1))
  code=$(curl -sS -o /tmp/e2e-start.json -w '%{http_code}' -X POST "$D/runs" -H "Authorization: Bearer $TOKEN" \
    -H "content-type: application/json" -H "idempotency-key: e2e-fill-20261007-$n" \
    -d '{"agent_name":"staging-claude-interactive-rw","interactive":true,"repo_url":"https://gitlab.com/pseekoo/kaizen.git"}')
  if [ "$code" = 409 ]; then cat /tmp/e2e-start.json; echo; break; fi
  python -c "import json;print(json.load(open('/tmp/e2e-start.json'))['job_id'])" >> /tmp/e2e-fill.txt
  [ "$n" -gt 16 ] && { echo "no capacity refusal after 16 starts" >&2; break; }
done
curl -fsS -H "Authorization: Bearer $TOKEN" "$D/runs?state=running&limit=100" \
  | python -c "import json,sys;print(sum(1 for r in json.load(sys.stdin)['items'] if r.get('interactive')))"
$CTL nodes
```
  The refused start returns `{"code":"node_at_capacity",…}`, at that moment the running interactive count is 16 (8 per node), and the refused start was the seventeenth. Free the slots again: for each job in `/tmp/e2e-fill.txt`, `$CTL stop "$($CTL job <job> | python -c "import json,sys;print(json.load(sys.stdin)['agent_id'])")"`.
- [ ] **3. Converse with one run:** `J=$(sed -n 2p /tmp/e2e-jobs.txt)` (kaizen). In the dashboard viewer send: "Remember the word PERSIMMON. Also create notes/pause-e2e.txt containing 'before pause'." Wait for the answer.
- [ ] **4. Pause it; check the slot, the branch and the marker:**
```bash
$CTL pause "$J"                       # "<job> paused on agent-ctrl-node-<n>"
$CTL job "$J"                         # state paused, session_id set, pause_push_failed false, paused_at set
$CTL nodes                            # its node's AGENTS is one lower
git ls-remote https://gitlab.com/pseekoo/kaizen.git "refs/heads/agent/$J"
git -C T:/projects/IWAKAN/kaizen fetch origin "agent/$J"
git -C T:/projects/IWAKAN/kaizen log -3 --format=%s FETCH_HEAD     # top: agent-ctrl run <job> pause (unless the agent committed everything itself)
git -C T:/projects/IWAKAN/kaizen show FETCH_HEAD:notes/pause-e2e.txt
NODE=$($CTL job "$J" | python -c "import json,sys;print(json.load(sys.stdin)['node_name'])")
$K exec "$NODE" -- ls /workspace/.runs | grep "$J"   # <job>, <job>.branch, <job>.paused
```
- [ ] **5. Resume it; the conversation continues:** `$CTL resume "$J"`. In the viewer ask "Which word did I ask you to remember?" — the answer is PERSIMMON.
- [ ] **6. Cross-node resume, and the stale copy goes:** `J2=$(sed -n 3p /tmp/e2e-jobs.txt)`; send it "Remember the number 4711."; `$CTL pause "$J2"`; `A=$($CTL job "$J2" | python -c "import json,sys;print(json.load(sys.stdin)['node_name'])")`. Make the other node the one with more free slots: start extra runs (as in step 2b, idempotency keys `e2e-tilt-20261007-<n>`) until `$CTL nodes` shows `$A` holding more interactive runs than the other node, then `$CTL resume "$J2"`. `$CTL job "$J2"` shows the other `node_name`; asked "Which number did I ask you to remember?" it answers 4711; `.runs/$J2` exists on the new node; and on `$A`, `$K exec "$A" -- ls /workspace/.runs | grep "$J2"` prints nothing (the controller discarded the stale worktree and marker after the resume reported Running). Stop the extra runs again.
- [ ] **7. Pi session layout (verification):** start a Pi run and pause and resume it, showing where its session file lives:
```bash
curl -fsS -X POST "$D/runs" -H "Authorization: Bearer $TOKEN" -H "content-type: application/json" \
  -H "idempotency-key: e2e-pause-pi-20261007" \
  -d '{"agent_name":"staging-pi-interactive-rw","interactive":true,"repo_url":"https://gitlab.com/pseekoo/battery-cafe.git"}' \
  | python -c "import json,sys;print(json.load(sys.stdin)['job_id'])" | tee -a /tmp/e2e-jobs.txt
P=$(tail -1 /tmp/e2e-jobs.txt)
```
  In the viewer send "Remember the colour TEAL." and wait for the answer; `$CTL job "$P"` shows a `session_id` (from Pi's status hook). Then:
```bash
$CTL pause "$P"
PNODE=$($CTL job "$P" | python -c "import json,sys;print(json.load(sys.stdin)['node_name'])")
PSID=$($CTL job "$P" | python -c "import json,sys;print(json.load(sys.stdin)['session_id'])")
$K exec "$PNODE" -- sh -c "ls -l /workspace/.runs/$P.session 2>/dev/null; find /home/agentctrl/.pi/agent/sessions -maxdepth 2 -name \"*_$PSID.jsonl\" -exec ls -l {} +"
$CTL resume "$P"
```
  The `find` prints exactly one file, `/home/agentctrl/.pi/agent/sessions/<dir>/<timestamp>_<session_id>.jsonl` — the layout `PiAdapter::locate_session` searches. If it prints nothing, list the tree (`$K exec "$PNODE" -- sh -c "find /home/agentctrl/.pi/agent/sessions -maxdepth 3 -newer /workspace/.runs/$P.branch"`), record the real path, and fix `locate_session` (A7) before continuing. After the resume, asked "Which colour did I ask you to remember?" it answers TEAL. Record the observed path in the PR description.
- [ ] **8. Stop a paused run:** pause `J` again, then `$CTL stop <agent_id of J>` (from `$CTL job "$J"`). The job becomes `stopped`; on its node `$K exec "$NODE" -- ls /workspace/.runs | grep "$J"` prints nothing.
- [ ] **9. Clean up:** stop every job in `/tmp/e2e-jobs.txt` (`$CTL stop <agent_id>`; paused ones the same way); afterwards neither node's `/workspace/.runs` holds an E2E job or `.paused` marker.

---

## Open questions

Resolved by the coordinator rulings of 2026-10-07 and folded into the tasks: heartbeat source (Global Constraints, A4), the node-registry message limit (A1), the pool rule for every node match including startup recovery (A5), the E2E wording (A16 steps 2/2b), stale worktrees after a cross-node resume (A11), retrying a failed pause push and lifting the pin (A8, A10), the second sandbox host gate (A15 step 3), the Pi session layout check (A16 step 7), CI RBAC and the rollout check (A13).

Still open:

1. **Discard of a stale worktree when the original node is offline.** `discard_stale_worktree` is fire-once; if node A is disconnected when the run resumes on B, A keeps its copy (the sweep never removes paused worktrees). It is replaced when the job next resumes on A, otherwise it stays until removed by hand.
2. **Claude runs paused before their first turn** have no transcript; the resume starts Claude with `--session-id <same id>` and an empty conversation (status "resumed without a transcript").
3. **Node replay buffer.** The node keeps every sent message (up to 8192); paused completions carrying up to 32 MiB session files stay in memory until evicted. Acceptable for staging volumes; a size-aware buffer is a follow-up.
4. **`AGENT_STATE_PAUSED = 7`** is new on the wire; the dashboard (Spec B) must map it or ignore `live_state` for paused runs.
5. **`ResumeAgent` in the spec has no `actor`/`source`;** the coordinator ruling adds them (audited like `StopAgent`). The plan follows the ruling.
