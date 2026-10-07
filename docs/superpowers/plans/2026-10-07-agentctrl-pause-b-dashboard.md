# AgentCtrl Pause and Second Node — Plan B (dashboard)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The operator can pause a running interactive run and resume it later from the dashboard. A paused run is listed as live but counted apart from active runs. Each run row shows the node it runs on.

**Architecture:** `server/agent-control.ts` gains `POST /api/agent-control/runs/:id/pause` and `/resume`, routed through the existing `mutateRun` path: same gates, idempotency key, rate limit and terminal-run check as `stop`. They call the new FleetControl RPCs `PauseAgent` and `ResumeAgent` that Plan A adds. `paused` becomes an allowed, non-terminal job state. The run read model (`publicRun`) gains `paused_at`, `pause_allowed` and `resume_allowed`; the overview gains `paused_runs`; the WarpSpeed session descriptor gains the `pause`/`resume` capabilities. `GET /repos` stays a pass-through, because the controller now returns the union over the node pool. `src/pages/AgentControl.tsx` gains Pause/Resume buttons in run rows and in the detail header, plus a `paused` badge tone and a paused-run count.

**Tech Stack:** TypeScript (node:http server, `@grpc/grpc-js` + `@grpc/proto-loader`) in `server/`; React 19 + Tailwind in `src/`; hermetic Node check script `scripts/check-agent-control.mjs` (in-memory Mongo fake + in-process FleetControl gRPC fake); Playwright component harness `playwright.agent.config.ts` (`tests/agent-interaction/`). Package manager: yarn 4 (`yarn install --immutable`).

Spec: `T:/projects/dev-ops/warp-agentctrl-pause/docs/superpowers/specs/2026-10-07-agentctrl-pause-and-second-node-design.md` (Spec B; it calls the RPCs Spec A defines).

## Global Constraints

Spec B, verbatim:

- `POST /api/agent-control/runs/:id/pause` and `/resume` → `PauseAgent` / `ResumeAgent`; errors map as for stop (409 `node_at_capacity` on resume, 412 `session_unknown` on pause).
- `paused` joins `ALLOWED_JOB_STATES`; it is **not** terminal. Capabilities: `pause` when `state == running` and the run is interactive with a known session id; `resume` when `state == paused`.
- Run row and detail: Pause / Resume buttons next to Stop; a `paused` badge; `active_runs` counts paused runs under a separate `paused_runs` figure.
- `/repos` passes the union through unchanged (the controller does the merge).
- `/runs` items gain `node_name` and `paused_at`.

Dashboard contract from Spec A, verbatim:

- New RPCs `PauseAgent { job_id }` and `ResumeAgent { job_id, request_id }` on FleetControl, both operator mutations (same audit/actor/source handling as `StopAgent`), both idempotent: pausing a paused run and resuming a running run are no-ops that return the current state.
- New `JobState::Paused` (`"paused"`), non-terminal, excluded from every capacity count. Persisted like the other states (controller job record, Mongo read model, `GET /runs`).
- Only when no candidate has a free slot does the call fail with `RESOURCE_EXHAUSTED "node at capacity"` (the dashboard keeps mapping it to 409 `node_at_capacity`).
- A run whose session id is unknown cannot be paused: `PauseAgent` fails with `FAILED_PRECONDITION "session id unknown"` and the dashboard disables Pause for it.
- `ListRepositories` returns the union over connected pool nodes: a repository is `reachable` only if every node that lists it reports it reachable; `error` names the failing node.
- Dashboard run view (`GET /runs`, `/runs/:id`): `state: paused`, `paused_at`, `node_name`.

Plan constraints:

- Error bodies keep the existing shape `{"error": <message>, "code": <code>}`. Pause/resume codes: 404 `not_found` (unknown job, or the controller's `NOT_FOUND`); 409 `terminal_run` (run is terminal); 409 `not_interactive` (pause of a non-interactive run); 409 `invalid_state` (pause of a run that is neither `running` nor `paused`, resume of a run that is neither `paused` nor `running`); 409 `node_at_capacity` (`RESOURCE_EXHAUSTED`); 412 `session_unknown` (`FAILED_PRECONDITION` whose details contain `session id unknown`); 409 `conflict` (any other `FAILED_PRECONDITION`/`ALREADY_EXISTS`/`ABORTED`); 409 `command_rejected` (the controller answered `success: false`; `error` carries its message); 422 `validation_error` (non-empty body).
- Pause/resume request: empty JSON body `{}` (no fields), `Content-Type: application/json`, and the same `Idempotency-Key` handling as `/stop` (required, `^[A-Za-z0-9._:-]{16,128}$`, forwarded as the RPC's `request_id`; a replay sets the `Idempotency-Replayed: true` header).
- Pause/resume success response: **HTTP 200** `{ "job_id": string, "state": string, "accepted": true, "node_name": string | null }`. `state` is `"paused"` after pause and `"running"` after resume (an idempotent no-op returns the controller's current state). `node_name` is the node the run is on afterwards (resume may move it). WarpSpeed relies on `accepted` and `state`: every 200 has `accepted: true`, and every failure is a non-2xx with the error body above, never a 200 with `accepted: false`. This differs on purpose from the 202 `ManagedCommandResult` that `input`/`close`/`stop` return.
- A paused run stays watchable: `GET /runs/:id/session` returns 200 with capabilities `["watch", "stop", "resume"]` (never `input`/`close`), and `GET /runs/:id/events` returns 200 with `terminal: false`, so an attached viewer keeps polling.
- The controller decides whether a session id is known. The dashboard does not refuse a pause on its own when the read model lacks `session_id`, because the read model can lag the spawn. It only uses the read model to set `pause_allowed` for the UI.
- Mongo read-model fields Plan A must write (the dashboard reads them; it writes nothing): `state: "paused"`, `paused_at` (BSON date), `node` (the job's current node, updated on resume), `session_id`.
- Proto: `server/agentctrl/v1.proto` is the dashboard's copy of agent-ctrl's proto. Task B1 adds the Spec A definitions now. Once Plan A lands, Task B6 overwrites the file verbatim from `T:/projects/SOTZ3N/stoz3n-agent-ctrl-pause/proto/agentctrl/v1.proto`.
- Worktree `T:/projects/SOTZ3N/stoz3n-agent-dash-pause`, branch `feat/pause-and-second-node` (off `dev` 36744bb). Remote `origin` = `https://git-dev.obachan.dev/stozn/stoz3n-agent-dash.git`. Stage explicit paths only; never `git add -A`.
- `yarn build:server` must succeed after every task. Every commit message ends with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Merging to `dev` deploys staging. The PR is merged only after Plan A is on staging (spec rollout order step 2) and only with the user's go-ahead.

Setup (once): `cd T:/projects/SOTZ3N/stoz3n-agent-dash-pause && yarn install --immutable && yarn playwright install chromium`.

---

### Task B1: Proto — PauseAgent, ResumeAgent, paused_at

**Files:**
- Modify: `server/agentctrl/v1.proto`. Add the RPCs to `service FleetControl` (:23-38, after `ListRepositories` :37). Add `max_interactive_runs` to `NodeRegistration` (:65-76, next tag 11). Add `running_interactive` to `HeartbeatRequest` (:360-365, next tag 5). Add `paused_at` to `JobInfo` (next tag 33, after `string branch = 32;`). Append the four new messages after `ListRepositoriesResponse`.
- Modify: `scripts/check-agent-control.mjs` (new check after `strict mutation validation rejects unknown fields`, :481-484).

**Interfaces:**
- Consumes: Spec A's RPC names.
- Produces (proto, the definitions Plan A must match; B6 replaces the file with Plan A's verbatim copy):
```proto
// service FleetControl, after ListRepositories
rpc PauseAgent(PauseAgentRequest) returns (PauseAgentResponse);
rpc ResumeAgent(ResumeAgentRequest) returns (ResumeAgentResponse);

// NodeRegistration
uint32 max_interactive_runs = 11;
// HeartbeatRequest
uint32 running_interactive = 5;
// JobInfo
google.protobuf.Timestamp paused_at = 33;

message PauseAgentRequest {
  string job_id = 1;
  string request_id = 2;
  string actor = 3;
  string source = 4;
}

message PauseAgentResponse {
  bool success = 1;
  string error = 2;
  string request_id = 3;
  bool replayed = 4;
  // "paused", or the current state for an idempotent no-op.
  string state = 5;
}

message ResumeAgentRequest {
  string job_id = 1;
  string request_id = 2;
  string actor = 3;
  string source = 4;
}

message ResumeAgentResponse {
  bool success = 1;
  string error = 2;
  string request_id = 3;
  bool replayed = 4;
  // "running", or the current state for an idempotent no-op.
  string state = 5;
  // The node the resumed run landed on.
  string node_name = 6;
  string agent_id = 7;
}
```
The dashboard reads `success`, `error`, `replayed`, `state` and (resume only) `node_name` from the responses (B3 `controlResponse`). It falls back to `"paused"`/`"running"` and to the job's recorded node when `state`/`node_name` are empty, so an older controller still yields a valid 200 body.

- [ ] **Step 1: Write the failing check.** Insert after the `strict mutation validation rejects unknown fields` check:
```js
await check("proto declares PauseAgent, ResumeAgent and the pause fields", async () => {
  const pause = fleetControl.service.PauseAgent;
  const resume = fleetControl.service.ResumeAgent;
  assert.equal(pause.path, "/agentctrl.v1.FleetControl/PauseAgent");
  assert.equal(resume.path, "/agentctrl.v1.FleetControl/ResumeAgent");
  const pauseRequest = { job_id: "job-1", request_id: "pause-request-000001", actor: "a", source: "dashboard" };
  assert.deepEqual(pause.requestDeserialize(pause.requestSerialize(pauseRequest)), pauseRequest);
  const resumeRequest = { job_id: "job-1", request_id: "resume-request-00001", actor: "a", source: "dashboard" };
  assert.deepEqual(resume.requestDeserialize(resume.requestSerialize(resumeRequest)), resumeRequest);
  const paused = { success: true, error: "", request_id: "r", replayed: false, state: "paused" };
  assert.deepEqual(pause.responseDeserialize(pause.responseSerialize(paused)), paused);
  const fieldNames = (message) => definition[`agentctrl.v1.${message}`].type.field.map((field) => field.name);
  assert.ok(fieldNames("JobInfo").includes("paused_at"));
  assert.ok(fieldNames("NodeRegistration").includes("max_interactive_runs"));
  assert.ok(fieldNames("HeartbeatRequest").includes("running_interactive"));
});
```
- [ ] **Step 2: Run it and watch it fail.** `yarn test:agent-control` → `FAIL proto declares PauseAgent, ResumeAgent and the pause fields` (`Cannot read properties of undefined (reading 'path')`).
- [ ] **Step 3: Implement.** Add the proto text above at the listed places.
- [ ] **Step 4: Run it and watch it pass.** `yarn test:agent-control` (all checks `ok`), then `yarn build:server`.
- [ ] **Step 5: Commit.**
```bash
git add server/agentctrl/v1.proto scripts/check-agent-control.mjs
git commit -m "chore(proto): PauseAgent, ResumeAgent and paused_at from the pause spec

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

### Task B2: `paused` in the run read model, overview and session capabilities

**Files:**
- Modify: `server/agent-control.ts`
  - `ALLOWED_JOB_STATES` :26-28
  - session descriptor capabilities :102-104
  - `fleetOverview` :136-180 (job queries :143-146, `active_runs` :175)
  - `publicRun` :508-556 (`session_id` :524; new fields after `task_room_id` :553)
  - `liveStateFor` :610-620
- Modify: `scripts/check-agent-control.mjs` (seed `jobs` :30-79; new checks after `runs expose repo_url and branch` :566-574)

**Interfaces:**
- Consumes: Mongo `jobs` rows with `state: "paused"`, `paused_at: Date`, `node: string`, `session_id: string`, `interactive: boolean`.
- Produces:
  - `GET /api/agent-control/runs[?state=paused]` and `GET /api/agent-control/runs/:id`: each item gains `paused_at: string | null` (ISO), `pause_allowed: boolean`, `resume_allowed: boolean`. `node_name: string | null` stays (already projected from `job.node`). `live_state` is `"paused"` for a paused run.
  - `GET /api/agent-control/overview`: `active_runs` = non-terminal runs that are not paused (from the newest 100 jobs); new `paused_runs: ManagedRun[]` = all paused runs (separate query, newest `paused_at` first, at most 200).
  - `GET /api/agent-control/runs/:id/session`: `capabilities` gains `"pause"` when `pause_allowed` and `"resume"` when `resume_allowed`, appended after `"stop"`. For a paused run it answers 200 with `state: "paused"`, `live_state: "paused"` and capabilities exactly `["watch", "stop", "resume"]`; `input`/`close` never appear, because `input_allowed` requires `state === "running"`.
  - `GET /api/agent-control/runs/:id/events` for a paused run: 200 with `terminal: false` (paused is not in `TERMINAL_STATES`), so an attached WarpSpeed viewer keeps polling. A long poll (`wait_ms > 0`) waits out its window and returns 200 with no items instead of ending early.
  - Rule: `pause_allowed = state === "running" && interactive === true && session_id is a non-empty string`; `resume_allowed = state === "paused"`.

- [ ] **Step 1: Seed three jobs and write the failing checks.** Directly above `const jobs = [` (:30) add:
```js
const pausedAt = new Date(now.getTime() - 500);
```
Append to the `jobs` array, after the `job-repo` entry (the seeds are older than every existing seed, so the pagination check keeps `job-new`, then `job-old`):
```js
  {
    job_id: "job-pausable",
    fleet_id: "fleet-staging",
    environment: "staging",
    agent_name: "staging-pi-inspector",
    backend: "claude",
    node: "agent-ctrl-node-0",
    agent_id: "agent-2",
    session_id: "0b6c1d2e-0000-4000-8000-000000000001",
    source: "dashboard",
    interactive: true,
    state: "running",
    live_state: "running",
    session_protocol_version: 1,
    created_at: new Date(now.getTime() - 4_000),
  },
  {
    job_id: "job-paused",
    fleet_id: "fleet-staging",
    environment: "staging",
    agent_name: "staging-pi-inspector",
    backend: "claude",
    node: "agent-ctrl-node-1",
    agent_id: "agent-4",
    session_id: "0b6c1d2e-0000-4000-8000-000000000002",
    source: "dashboard",
    interactive: true,
    state: "paused",
    // The last live state reported before the pause; it must not re-enable input.
    live_state: "waiting_input",
    paused_at: pausedAt,
    session_protocol_version: 1,
    created_at: new Date(now.getTime() - 5_000),
  },
  {
    job_id: "job-worker",
    fleet_id: "fleet-staging",
    environment: "staging",
    agent_name: "staging-pi-inspector",
    backend: "pi",
    node: "agent-ctrl-node-0",
    agent_id: "agent-3",
    source: "schedule",
    interactive: false,
    state: "running",
    live_state: "running",
    session_protocol_version: 1,
    created_at: new Date(now.getTime() - 6_000),
  },
```
Insert after the `runs expose repo_url and branch` check:
```js
await check("paused is a live, filterable run state with node_name and paused_at", async () => {
  const response = await call("/api/agent-control/runs?state=paused");
  assert.equal(response.status, 200);
  const body = await response.json();
  assert.deepEqual(body.items.map((item) => item.job_id), ["job-paused"]);
  const run = body.items[0];
  assert.equal(run.state, "paused");
  assert.equal(run.live_state, "paused");
  assert.equal(run.node_name, "agent-ctrl-node-1");
  assert.equal(run.paused_at, pausedAt.toISOString());
  assert.equal(run.pause_allowed, false);
  assert.equal(run.resume_allowed, true);
});

await check("pause is allowed only for a running interactive run with a known session id", async () => {
  const body = await (await call("/api/agent-control/runs?limit=200")).json();
  const byId = Object.fromEntries(body.items.map((item) => [item.job_id, item]));
  assert.equal(byId["job-pausable"].pause_allowed, true);
  assert.equal(byId["job-pausable"].resume_allowed, false);
  assert.equal(byId["job-pausable"].paused_at, null);
  assert.equal(byId["job-pausable"].node_name, "agent-ctrl-node-0");
  assert.equal(byId["job-new"].pause_allowed, false, "no session id");
  assert.equal(byId["job-worker"].pause_allowed, false, "not interactive");
  assert.equal(byId["job-old"].pause_allowed, false, "terminal");
});

await check("overview lists paused runs apart from active runs", async () => {
  const body = await (await call("/api/agent-control/overview")).json();
  assert.deepEqual(body.paused_runs.map((run) => run.job_id), ["job-paused"]);
  assert.ok(!body.active_runs.some((run) => run.job_id === "job-paused"));
  assert.ok(body.active_runs.some((run) => run.job_id === "job-pausable"));
});

await check("session descriptor advertises pause and resume by state", async () => {
  const pausable = await (await call("/api/agent-control/runs/job-pausable/session")).json();
  assert.deepEqual(pausable.capabilities, ["watch", "stop", "pause"]);
  const pausedResponse = await call("/api/agent-control/runs/job-paused/session");
  assert.equal(pausedResponse.status, 200);
  const paused = await pausedResponse.json();
  assert.equal(paused.state, "paused");
  assert.equal(paused.live_state, "paused");
  assert.deepEqual(paused.capabilities, ["watch", "stop", "resume"]);
  const noSession = await (await call("/api/agent-control/runs/job-new/session")).json();
  assert.ok(!noSession.capabilities.includes("pause"));
});

await check("a viewer attached to a paused run keeps polling its events", async () => {
  for (const waitMs of [0, 300]) {
    const response = await call(`/api/agent-control/runs/job-paused/events?wait_ms=${waitMs}`);
    assert.equal(response.status, 200, `wait_ms=${waitMs}`);
    const body = await response.json();
    assert.equal(body.terminal, false);
    assert.equal(body.gap, false);
    assert.deepEqual(body.items, []);
  }
});
```
- [ ] **Step 2: Run them and watch them fail.** `yarn test:agent-control` fails four checks. `?state=paused` returns 422, `paused_runs` is undefined, and `capabilities` lacks `pause`/`resume`. The events check already passes, because `paused` is not in `TERMINAL_STATES` today; it pins that a paused run stays pollable.
- [ ] **Step 3: Implement** in `server/agent-control.ts`.

`ALLOWED_JOB_STATES` (:26-28):
```ts
const ALLOWED_JOB_STATES = new Set([
  "pending", "dispatching", "running", "paused", "completed", "waiting_input", "failed", "stopped", "interrupted",
]);
```
Session descriptor (:102-104):
```ts
    const capabilities = ["watch"];
    if (run.input_allowed === true) capabilities.push("input", "close");
    if (!TERMINAL_STATES.has(String(run.state))) capabilities.push("stop");
    if (run.pause_allowed === true) capabilities.push("pause");
    if (run.resume_allowed === true) capabilities.push("resume");
```
`fleetOverview`: replace the job query block (:143-147) and `active_runs` (:175):
```ts
  const [nodes, jobs, pausedJobs] = await Promise.all([
    db.collection("nodes").find({ fleet_id: fleetId }).sort({ node_name: 1 }).toArray(),
    db.collection("jobs").find({ fleet_id: fleetId, environment: config.environment }).sort({ created_at: -1 }).limit(100).toArray(),
    // Paused runs stay live for days; list them all, not only those among the newest 100 jobs.
    db.collection("jobs").find({ fleet_id: fleetId, environment: config.environment, state: "paused" }).sort({ paused_at: -1 }).limit(MAX_LIMIT).toArray(),
  ]);
  const projected = jobs.map((job) => publicRun(job, snapshot));
```
```ts
    active_runs: projected.filter((job) => !TERMINAL_STATES.has(String(job.state)) && job.state !== "paused"),
    paused_runs: pausedJobs.map((job) => publicRun(job, snapshot)),
```
`publicRun`: after `const inputAllowed = …` add
```ts
  const sessionId = nullableString(job.session_id);
```
and change `session_id: nullableString(job.session_id),` (:524) to `session_id: sessionId,`. After `task_room_id: nullableString(job.task_room_id),` (:553) add:
```ts
    paused_at: iso(job.paused_at),
    pause_allowed: state === "running" && job.interactive === true && sessionId !== null,
    resume_allowed: state === "paused",
```
`liveStateFor` (before `if (state === "pending" || state === "dispatching")`, :618):
```ts
  if (state === "paused") return "paused";
```
- [ ] **Step 4: Run them and watch them pass.** `yarn test:agent-control` (all `ok`); `yarn build:server`.
- [ ] **Step 5: Commit.**
```bash
git add server/agent-control.ts scripts/check-agent-control.mjs
git commit -m "feat(agent-control): paused run state, paused_runs, pause/resume capabilities

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

### Task B3: Pause and resume routes with error mapping

**Files:**
- Modify: `server/agent-control.ts`
  - run-route regex :84
  - POST dispatch :130
  - `mutateRun` :356-405 (new branches before the stop `else` :395)
  - `mapGrpcError` :717-729
  - new `controlResponse` after `commandResponse` :731-752
- Modify: `scripts/check-agent-control.mjs`
  - fake `mutation` :190-211 (new `sessionunknown` prefix; new `controlRpc` wrapper after it)
  - fake service :214-225 (register `PauseAgent`, `ResumeAgent`)
  - new checks after the B2 checks

**Interfaces:**
- Consumes: `FleetControl.PauseAgent(PauseAgentRequest)` and `FleetControl.ResumeAgent(ResumeAgentRequest)` (B1).
- Produces:
  - `POST /api/agent-control/runs/{jobId}/pause`
    - Headers: `Content-Type: application/json`, `Idempotency-Key`. Optional: `X-Stozn-Client: warpspeed`.
    - Body: `{}`.
    - gRPC request: `{ job_id, request_id: <Idempotency-Key>, actor: <claims.sub[:128]>, source: "dashboard"|"warpspeed" }`.
    - Success: **200** `{ "job_id": "<jobId>", "state": "paused", "accepted": true, "node_name": "<node>" | null }` with headers `Cache-Control: private, no-store` and `Idempotency-Replayed: true|false`.
  - `POST /api/agent-control/runs/{jobId}/resume`: same request; success **200** `{ "job_id", "state": "running", "accepted": true, "node_name": <node the run landed on> }`.
  - `state` = the RPC response's `state`, else `"paused"`/`"running"`. `node_name` = the RPC response's `node_name`, else the job's recorded `node`, else `null`.
  - A controller answer with `success: false` is 409 `command_rejected` (never a 200 with `accepted: false`). Other errors are listed in Global Constraints.
  - TS type (server-side shape; the SPA mirrors it in B5):
```ts
interface ManagedRunControlResult { job_id: string; state: string; accepted: true; node_name: string | null }
```

- [ ] **Step 1: Extend the fake and write the failing checks.** In `mutation` (:190), after the `atcapacity` branch:
```js
  if (id.startsWith("sessionunknown")) {
    return callback({ code: grpc.status.FAILED_PRECONDITION, message: "session id unknown" });
  }
```
After `function mutation(…) { … }` add a wrapper that answers like Plan A's controller (state, and the landing node on resume):
```js
function controlRpc(state, nodeName) {
  return (call, callback) => mutation(call, (error, value) => callback(error, value && { ...value, state, node_name: nodeName }));
}
```
In `server.addService(fleetControl.service, { … })` add:
```js
  PauseAgent: controlRpc("paused", ""),
  ResumeAgent: controlRpc("running", "agent-ctrl-node-1"),
```
After the B2 checks:
```js
// A separate actor keeps these checks clear of the per-actor mutation rate limit.
const pauseActor = { sub: "pause@example.test", role: "admin" };

await check("pause forwards PauseAgent for the job", async () => {
  const response = await call("/api/agent-control/runs/job-pausable/pause", jsonMutation({}, "pause-request-000001", { "x-stozn-client": "warpspeed" }), pauseActor);
  assert.equal(response.status, 200);
  assert.equal(response.headers.get("idempotency-replayed"), "false");
  // node_name falls back to the job's recorded node when the controller leaves it empty.
  assert.deepEqual(await response.json(), { job_id: "job-pausable", state: "paused", accepted: true, node_name: "agent-ctrl-node-0" });
  assert.deepEqual(requests.at(-1), { job_id: "job-pausable", request_id: "pause-request-000001", actor: "pause@example.test", source: "warpspeed" });
});

await check("resume forwards ResumeAgent for the paused job", async () => {
  const response = await call("/api/agent-control/runs/job-paused/resume", jsonMutation({}, "resume-request-00001"), pauseActor);
  assert.equal(response.status, 200);
  assert.deepEqual(await response.json(), { job_id: "job-paused", state: "running", accepted: true, node_name: "agent-ctrl-node-1" });
  assert.deepEqual(requests.at(-1), { job_id: "job-paused", request_id: "resume-request-00001", actor: "pause@example.test", source: "dashboard" });
});

await check("a replayed pause is a 200 with Idempotency-Replayed", async () => {
  const response = await call("/api/agent-control/runs/job-pausable/pause", jsonMutation({}, "replay-pause-000001"), pauseActor);
  assert.equal(response.status, 200);
  assert.equal(response.headers.get("idempotency-replayed"), "true");
  assert.equal((await response.json()).accepted, true);
});

await check("a controller rejection is a 409 command_rejected, never accepted: false", async () => {
  const response = await call("/api/agent-control/runs/job-paused/resume", jsonMutation({}, "rejected-resume-0001"), pauseActor);
  assert.equal(response.status, 409);
  assert.deepEqual(await response.json(), { error: "policy denied", code: "command_rejected" });
});

await check("resume with every node at capacity is a 409 node_at_capacity", async () => {
  const response = await call("/api/agent-control/runs/job-paused/resume", jsonMutation({}, "atcapacity-resume-0001"), pauseActor);
  assert.equal(response.status, 409);
  assert.deepEqual(await response.json(), { error: "node at capacity", code: "node_at_capacity" });
});

await check("pause with an unknown session id is a 412 session_unknown", async () => {
  const response = await call("/api/agent-control/runs/job-new/pause", jsonMutation({}, "sessionunknown-pause-01"), pauseActor);
  assert.equal(response.status, 412);
  assert.equal((await response.json()).code, "session_unknown");
});

await check("pause and resume of an unknown job are a 404 without dispatch", async () => {
  const before = requests.length;
  for (const action of ["pause", "resume"]) {
    const response = await call(`/api/agent-control/runs/job-missing/${action}`, jsonMutation({}, `${action}-missing-0000001`), pauseActor);
    assert.equal(response.status, 404, action);
    assert.equal((await response.json()).code, "not_found");
  }
  assert.equal(requests.length, before);
});

await check("pause and resume of a terminal run are a 409 terminal_run", async () => {
  const before = requests.length;
  for (const action of ["pause", "resume"]) {
    const response = await call(`/api/agent-control/runs/job-old/${action}`, jsonMutation({}, `${action}-terminal-000001`), pauseActor);
    assert.equal(response.status, 409, action);
    assert.equal((await response.json()).code, "terminal_run");
  }
  assert.equal(requests.length, before);
});

await check("pause refuses a non-interactive run and resume a run that is not paused or running", async () => {
  const before = requests.length;
  const worker = await call("/api/agent-control/runs/job-worker/pause", jsonMutation({}, "pause-worker-0000001"), pauseActor);
  assert.equal(worker.status, 409);
  assert.equal((await worker.json()).code, "not_interactive");
  const seed = jobs.find((job) => job.job_id === "job-pausable");
  seed.state = "dispatching";
  try {
    const dispatching = await call("/api/agent-control/runs/job-pausable/resume", jsonMutation({}, "resume-dispatch-0001"), pauseActor);
    assert.equal(dispatching.status, 409);
    assert.equal((await dispatching.json()).code, "invalid_state");
  } finally {
    seed.state = "running";
  }
  assert.equal(requests.length, before);
});

await check("pause and resume take an empty body", async () => {
  const response = await call("/api/agent-control/runs/job-pausable/pause", jsonMutation({ force: true }, "pause-body-000000001"), pauseActor);
  assert.equal(response.status, 422);
  assert.equal((await response.json()).code, "validation_error");
});
```
- [ ] **Step 2: Run them and watch them fail.** `yarn test:agent-control`. The forward, replay, rejection, capacity, session, terminal, state and body checks fail, because the routes answer 404 `not_found`. The 404 check already passes, because unmatched suffixes return 404 today.
- [ ] **Step 3: Implement** in `server/agent-control.ts`.

Route regex (:84):
```ts
  const match = url.pathname.match(/^\/api\/agent-control\/runs\/([^/]+)(?:\/(session|events|input|close|stop|pause|resume))?$/);
```
POST dispatch (:130):
```ts
  if (request.method === "POST" && ["input", "close", "stop", "pause", "resume"].includes(suffix)) {
```
`mutateRun`: replace the final `} else {` (:395) with:
```ts
  } else if (suffix === "pause") {
    exactKeys(body, []);
    if (job.interactive !== true) throw serviceError(409, "Only interactive runs can be paused", "not_interactive");
    if (run.state !== "running" && run.state !== "paused") {
      throw serviceError(409, "Only a running run can be paused", "invalid_state");
    }
    result = await unary(config, "PauseAgent", { job_id: String(job.job_id), request_id: requestId, actor, source });
    return controlResponse(job, "paused", result, headers);
  } else if (suffix === "resume") {
    exactKeys(body, []);
    if (run.state !== "paused" && run.state !== "running") {
      throw serviceError(409, "Only a paused run can be resumed", "invalid_state");
    }
    result = await unary(config, "ResumeAgent", { job_id: String(job.job_id), request_id: requestId, actor, source });
    return controlResponse(job, "running", result, headers);
  } else {
```
After `commandResponse` add:
```ts
/** Pause/resume answer: 200 with the run's state, or a thrown 409 when the controller rejects. */
function controlResponse(
  job: MongoDocument,
  fallbackState: "paused" | "running",
  result: Record<string, unknown>,
  headers: Headers,
): Response {
  if (result.success !== true) {
    throw serviceError(409, safeText(result.error, 1000) ?? "Command rejected", "command_rejected");
  }
  const outHeaders = noStore(headers);
  outHeaders.set("Idempotency-Replayed", String(result.replayed === true));
  return responseJson({
    job_id: String(job.job_id),
    state: nullableString(result.state) ?? fallbackState,
    accepted: true,
    node_name: nullableString(result.node_name) ?? nullableString(job.node),
  }, 200, outHeaders);
}
```
(The `running` state is accepted on resume and the `paused` state on pause because Spec A makes both RPCs idempotent no-ops.)

`mapGrpcError`: insert before the `ALREADY_EXISTS || ABORTED || FAILED_PRECONDITION` branch:
```ts
  if (code === grpc.status.FAILED_PRECONDITION && /session id unknown/i.test(error?.details ?? "")) {
    return serviceError(412, "Session id unknown; the run cannot be paused", "session_unknown");
  }
```
- [ ] **Step 4: Run them and watch them pass.** `yarn test:agent-control` (all `ok`); `yarn build:server`.
- [ ] **Step 5: Commit.**
```bash
git add server/agent-control.ts scripts/check-agent-control.mjs
git commit -m "feat(agent-control): pause and resume routes for managed runs

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

### Task B4: OpenAPI for pause/resume, and the `/repos` union contract

**Files:**
- Modify: `server/openapi.ts`
  - new paths after `/api/agent-control/runs/{jobId}/stop` :236-240
  - `FleetOverview` schema :623
  - `ManagedRun` schema :624-631
- Modify: `scripts/check-agent-control.mjs`
  - `let repositoriesUnavailable` :213 (add `repositoriesUnion`)
  - fake `ListRepositories` :220-224
  - new checks after the B3 checks

**Interfaces:**
- Consumes: the routes from B3 and the run fields from B2. `ListRepositories` returns the union (Plan A): an entry with `reachable: false` whose `error` names the failing node.
- Produces:
  - OpenAPI paths `/api/agent-control/runs/{jobId}/pause` and `/resume`, each with a 200 `ManagedRunControlResult` response (no 202).
  - `ManagedRun.properties` gains `state` (enum including `paused`), `node_name`, `paused_at`, `pause_allowed`, `resume_allowed`.
  - `FleetOverview.properties` gains `active_runs` and `paused_runs`.
  - `GET /api/agent-control/repos` item shape stays `{ name, url, branch, reachable, error }`, with `error` passed through unchanged (bounded to 1000 characters by `safeText`).

- [ ] **Step 1: Write the checks.** Replace `let repositoriesUnavailable = false;` (:213) with:
```js
let repositoriesUnavailable = false;
let repositoriesUnion = false;
```
Replace the `ListRepositories` entry (:220-224) with:
```js
  ListRepositories: (_call, cb) => repositoriesUnavailable
    ? cb({ code: grpc.status.UNAVAILABLE, message: "down" })
    : cb(null, {
      repositories: [
        { name: "kaizen", url: "https://gitlab.com/pseekoo/kaizen.git", branch: "main", reachable: true, error: "" },
        ...(repositoriesUnion
          ? [{ name: "battery-cafe", url: "https://gitlab.com/pseekoo/battery-cafe.git", branch: "main", reachable: false, error: "agent-ctrl-node-1: could not read from remote repository" }]
          : []),
      ],
    }),
```
After the B3 checks:
```js
await check("repos passes the controller's pool union through unchanged", async () => {
  repositoriesUnion = true;
  try {
    const body = await (await call("/api/agent-control/repos")).json();
    assert.deepEqual(body.items, [
      { name: "kaizen", url: "https://gitlab.com/pseekoo/kaizen.git", branch: "main", reachable: true, error: null },
      { name: "battery-cafe", url: "https://gitlab.com/pseekoo/battery-cafe.git", branch: "main", reachable: false, error: "agent-ctrl-node-1: could not read from remote repository" },
    ]);
  } finally {
    repositoriesUnion = false;
  }
});

const { dashboardOpenApiSpec } = await import("../dist/server/openapi.js");

await check("OpenAPI documents pause, resume and the paused run fields", async () => {
  const spec = dashboardOpenApiSpec("https://dash.example");
  const pause = spec.paths["/api/agent-control/runs/{jobId}/pause"].post;
  for (const status of ["200", "404", "409", "412", "422", "503"]) assert.ok(pause.responses[status], `pause ${status}`);
  assert.equal(pause.responses["202"], undefined);
  assert.deepEqual(pause.responses["200"].content["application/json"].schema, { $ref: "#/components/schemas/ManagedRunControlResult" });
  assert.match(pause.responses["412"].description, /session_unknown/);
  const resume = spec.paths["/api/agent-control/runs/{jobId}/resume"].post;
  for (const status of ["200", "404", "409", "422", "503"]) assert.ok(resume.responses[status], `resume ${status}`);
  const control = spec.components.schemas.ManagedRunControlResult;
  assert.deepEqual(control.required, ["job_id", "state", "accepted", "node_name"]);
  assert.deepEqual(control.properties.accepted, { const: true });
  assert.match(resume.responses["409"].description, /node_at_capacity/);
  const run = spec.components.schemas.ManagedRun.properties;
  for (const field of ["state", "node_name", "paused_at", "pause_allowed", "resume_allowed"]) assert.ok(run[field], field);
  assert.ok(run.state.enum.includes("paused"));
  assert.ok(spec.components.schemas.FleetOverview.properties.paused_runs);
});
```
- [ ] **Step 2: Run them.** `yarn test:agent-control`. The repos check PASSES already; it pins the pass-through contract. The OpenAPI check FAILS (`Cannot read properties of undefined (reading 'post')`).
- [ ] **Step 3: Implement** in `server/openapi.ts`. After the `/api/agent-control/runs/{jobId}/stop` entry:
```ts
      "/api/agent-control/runs/{jobId}/pause": {
        post: operation("Pause an interactive managed run; frees its node slot and keeps its conversation", ["Managed runs"], {
          "200": response("Run paused (or already paused)", ref("ManagedRunControlResult"), { job_id: "job-0b6c", state: "paused", accepted: true, node_name: "agent-ctrl-node-0" }),
          "404": response("Run not found", ref("Error")),
          "409": response(
            "Run is terminal (`terminal_run`), not interactive (`not_interactive`), not running (`invalid_state`), rejected by the controller (`command_rejected`), or an idempotency conflict (`conflict`)",
            ref("Error"),
          ),
          "412": response("The run's session id is unknown, so it could not be resumed (`session_unknown`)", ref("Error")),
          "422": response("Validation failed", ref("Error")),
          "503": response("Controller unavailable", ref("Error")),
        }, { parameters: [jobId, idempotencyKey], requestBody: body({ type: "object", additionalProperties: false }, {}) }),
      },
      "/api/agent-control/runs/{jobId}/resume": {
        post: operation("Resume a paused managed run on the node pool", ["Managed runs"], {
          "200": response("Run resumed (or already running)", ref("ManagedRunControlResult"), { job_id: "job-0b6c", state: "running", accepted: true, node_name: "agent-ctrl-node-1" }),
          "404": response("Run not found", ref("Error")),
          "409": response(
            "Run is terminal (`terminal_run`), not paused (`invalid_state`), every pool node is at its interactive-run cap (`node_at_capacity`), rejected by the controller (`command_rejected`), or an idempotency conflict (`conflict`)",
            ref("Error"),
          ),
          "422": response("Validation failed", ref("Error")),
          "503": response("Controller unavailable", ref("Error")),
        }, { parameters: [jobId, idempotencyKey], requestBody: body({ type: "object", additionalProperties: false }, {}) }),
      },
```
Before `ManagedRepository` add:
```ts
        ManagedRunControlResult: {
          type: "object",
          additionalProperties: false,
          required: ["job_id", "state", "accepted", "node_name"],
          properties: {
            job_id: { type: "string" },
            state: { type: "string", description: "`paused` after pause, `running` after resume; the current state for an idempotent no-op." },
            accepted: { const: true },
            node_name: { type: ["string", "null"], description: "Node the run is on afterwards." },
          },
        },
```
Replace `FleetOverview: { type: "object", additionalProperties: true },` (:623) with:
```ts
        FleetOverview: {
          type: "object",
          additionalProperties: true,
          properties: {
            active_runs: { ...array(ref("ManagedRun")), description: "Live runs that are not paused." },
            paused_runs: { ...array(ref("ManagedRun")), description: "Paused runs; they hold no node slot." },
          },
        },
```
Replace the `ManagedRun.properties` object (:627-630) with:
```ts
          properties: {
            state: {
              type: "string",
              enum: ["pending", "dispatching", "running", "paused", "completed", "waiting_input", "failed", "stopped", "interrupted"],
            },
            node_name: { type: ["string", "null"], description: "Node the run is on; changes when a resume lands elsewhere." },
            paused_at: { type: ["string", "null"], format: "date-time" },
            pause_allowed: { type: "boolean", description: "Running, interactive, and the session id is known." },
            resume_allowed: { type: "boolean", description: "The run is paused." },
            repo_url: { type: ["string", "null"], description: "Repository the run works on." },
            branch: { type: ["string", "null"], description: "Branch the run pushes to." },
          },
```
- [ ] **Step 4: Run them and watch them pass.** `yarn test:agent-control` (all `ok`); `yarn build:server`.
- [ ] **Step 5: Commit.**
```bash
git add server/openapi.ts scripts/check-agent-control.mjs
git commit -m "docs(openapi): pause and resume routes, control result, paused run fields, repos union check

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

### Task B5: Dashboard UI — Pause/Resume buttons, paused badge, paused count

**Files:**
- Modify: `src/lib/types.ts` (`ManagedRun` :377-403, `AgentControlOverview` :424-437)
- Modify: `src/lib/api.ts` (after `closeManagedRunInput` :551-552)
- Modify: `src/pages/AgentControl.tsx`
  - `Badge`/`stateTone` :12-26
  - new `PauseResume` after `date()` :28-32
  - `runs` memo :118-122
  - new `control()` after `stop()` :171-183
  - banner :228-231
  - run rows :267-274
  - detail buttons :283-290
  - detail `<dl>` :292-298
- Create: `tests/agent-interaction/agent-control.html`, `tests/agent-interaction/agent-control.tsx`, `tests/agent-interaction/agent-control.spec.ts`

**Interfaces:**
- Consumes: `GET /api/agent-control/overview` (`active_runs`, `paused_runs`, `recent_runs`), `POST /runs/{jobId}/pause|resume` (B3).
- Produces (TS):
```ts
// ManagedRun additions
paused_at: string | null;
pause_allowed: boolean;
resume_allowed: boolean;
// AgentControlOverview addition
paused_runs: ManagedRun[];
// api additions
// new type
export interface ManagedRunControlResult { job_id: string; state: string; accepted: true; node_name: string | null }
pauseManagedRun: (jobId: string, requestId?: string) => Promise<ManagedRunControlResult>;
resumeManagedRun: (jobId: string, requestId?: string) => Promise<ManagedRunControlResult>;
```
  - UI:
    - Running interactive runs show a Pause button. It is disabled with the title `Session id unknown; this run cannot be paused` when `pause_allowed` is false.
    - Paused runs show a Resume button.
    - In run rows the buttons are labelled `Pause <job_id>` / `Resume <job_id>`; in the detail header they are labelled `Pause` / `Resume`, next to Stop.
    - Both appear only when `overview.mutation_available`.
    - The badge text is the state, with an indigo tone for `paused`.
    - The banner reads `· <n> active · <m> paused`.

- [ ] **Step 1: Write the failing Playwright test.** `tests/agent-interaction/agent-control.html`:
```html
<!doctype html><html><head><meta charset="utf-8"><title>Agent control test</title></head><body><div id="root"></div><script type="module" src="/tests/agent-interaction/agent-control.tsx"></script></body></html>
```
`tests/agent-interaction/agent-control.tsx`:
```tsx
import { createRoot } from 'react-dom/client';
import { AgentControl } from '../../src/pages/AgentControl';
import '../../src/index.css';

createRoot(document.getElementById('root')!).render(<main className="mx-auto max-w-6xl p-8"><AgentControl /></main>);
```
`tests/agent-interaction/agent-control.spec.ts`:
```ts
import { test, expect, type Page } from '@playwright/test';

const base = {
  fleet_id: 'fleet-staging', environment: 'staging', agent_name: 'staging-claude-interactive-rw', backend: 'claude',
  agent_id: 'agent-1', live_state: 'running', source: 'dashboard', interactive: true, input_allowed: false,
  project_path: null, created_at: '2026-10-07T10:00:00Z', started_at: '2026-10-07T10:00:01Z', completed_at: null,
  last_activity_at: '2026-10-07T10:05:00Z', error: null, interrupted_reason: null, result_tail: null,
  last_event_sequence: 0, task_id: null, task_ref: null, task_title: null, task_room_id: null,
  paused_at: null, pause_allowed: false, resume_allowed: false,
};
const running = { ...base, job_id: 'job-running', state: 'running', node_name: 'agent-ctrl-node-0', pause_allowed: true };
const noSession = { ...base, job_id: 'job-nosession', state: 'running', node_name: 'agent-ctrl-node-0' };
const paused = {
  ...base, job_id: 'job-paused', state: 'paused', live_state: 'paused', node_name: 'agent-ctrl-node-1',
  paused_at: '2026-10-07T11:00:00Z', resume_allowed: true,
};
const overview = {
  fleet_id: 'fleet-staging', environment: 'staging', controller_version: 'test', control_api_version: 2,
  controller_last_seen: '2026-10-07T11:59:59Z', controller_stale: false, mutation_available: true,
  mutation_unavailable_reason: null, agents: [], nodes: [], active_runs: [running, noSession], paused_runs: [paused],
  recent_runs: [running, noSession, paused], refreshed_at: '2026-10-07T12:00:00Z',
};
const emptyEvents = { items: [], next_cursor: null, terminal: false, gap: false, earliest_cursor: null, refreshed_at: '2026-10-07T12:00:00Z' };

async function mockApi(page: Page, posts: { path: string; body: unknown }[], resumeAtCapacity = false) {
  await page.route('**/*', route => new URL(route.request().url()).origin === 'http://127.0.0.1:4189' ? route.continue() : route.abort());
  await page.route(/\/api\/agent-control\/overview$/, route => route.fulfill({ json: overview }));
  await page.route(/\/api\/agent-control\/commands\?/, route => route.fulfill({ json: { items: [], next_cursor: null } }));
  await page.route(/\/api\/work-items\//, route => route.fulfill({ json: { items: [], total: 0, skip: 0, limit: 200 } }));
  await page.route(/\/dashboard\/rooms$/, route => route.fulfill({ json: [] }));
  await page.route(/\/api\/agent-control\/runs\/[^/]+\/events/, route => route.fulfill({ json: emptyEvents }));
  await page.route(/\/api\/agent-control\/runs\/[^/]+\/(pause|resume)$/, route => {
    const path = new URL(route.request().url()).pathname;
    posts.push({ path, body: route.request().postDataJSON() });
    const action = path.endsWith('/pause') ? 'pause' : 'resume';
    if (action === 'resume' && resumeAtCapacity) {
      return route.fulfill({ status: 409, json: { error: 'node at capacity', code: 'node_at_capacity' } });
    }
    const jobId = path.split('/')[4];
    return route.fulfill({ status: 200, json: { job_id: jobId, state: action === 'pause' ? 'paused' : 'running', accepted: true, node_name: 'agent-ctrl-node-0' } });
  });
}

test('rows and detail offer pause/resume, show the paused badge, node and count', async ({ page }) => {
  const posts: { path: string; body: unknown }[] = [];
  await mockApi(page, posts);
  await page.goto('/tests/agent-interaction/agent-control.html');
  await expect(page.getByText('2 active · 1 paused')).toBeVisible();
  await expect(page.getByRole('button', { name: 'Pause job-running' })).toBeEnabled();
  await expect(page.getByRole('button', { name: 'Pause job-nosession' })).toBeDisabled();
  await expect(page.getByRole('button', { name: 'Pause job-nosession' })).toHaveAttribute('title', 'Session id unknown; this run cannot be paused');
  await expect(page.getByRole('button', { name: 'Resume job-paused' })).toBeEnabled();
  await expect(page.getByRole('button', { name: 'Pause job-paused' })).toHaveCount(0);
  await expect(page.getByText('paused', { exact: true }).first()).toBeVisible();
  await expect(page.getByText(/agent-ctrl-node-1/).first()).toBeVisible();

  await page.getByRole('button', { name: 'Pause job-running' }).click();
  await expect.poll(() => posts).toEqual([{ path: '/api/agent-control/runs/job-running/pause', body: {} }]);

  await page.getByText('job-paused', { exact: true }).click();
  await expect(page.getByRole('button', { name: 'Stop', exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Pause', exact: true })).toHaveCount(0);
  await page.getByRole('button', { name: 'Resume', exact: true }).click();
  await expect.poll(() => posts.at(-1)).toEqual({ path: '/api/agent-control/runs/job-paused/resume', body: {} });
});

test('resume with every node at capacity shows the controller message', async ({ page }) => {
  await mockApi(page, [], true);
  await page.goto('/tests/agent-interaction/agent-control.html');
  await page.getByRole('button', { name: 'Resume job-paused' }).click();
  await expect(page.getByRole('alert')).toContainText('node at capacity');
});
```
- [ ] **Step 2: Run it and watch it fail.** `yarn playwright test --config playwright.agent.config.ts tests/agent-interaction/agent-control.spec.ts` → both tests fail (no `Pause job-running` button; banner lacks `paused`).
- [ ] **Step 3: Implement.**

`src/lib/types.ts`. In `ManagedRun`, after `task_room_id: string | null;`:
```ts
  paused_at: string | null;
  pause_allowed: boolean;
  resume_allowed: boolean;
```
In `AgentControlOverview`, after `active_runs: ManagedRun[];`:
```ts
  paused_runs: ManagedRun[];
```
After `ManagedCommandResult`:
```ts
/** 200 answer of POST /runs/:id/pause and /resume; failures are non-2xx `{ error, code }`. */
export interface ManagedRunControlResult {
  job_id: string;
  state: string;
  accepted: true;
  node_name: string | null;
}
```
Add `ManagedRunControlResult` to the `../lib/types` import list in `src/lib/api.ts` (:44-49).
`src/lib/api.ts`, after `closeManagedRunInput`:
```ts
  pauseManagedRun: (jobId: string, requestId?: string) =>
    managedMutation<ManagedRunControlResult>(`/api/agent-control/runs/${encodeURIComponent(jobId)}/pause`, {}, requestId),
  resumeManagedRun: (jobId: string, requestId?: string) =>
    managedMutation<ManagedRunControlResult>(`/api/agent-control/runs/${encodeURIComponent(jobId)}/resume`, {}, requestId),
```
`src/pages/AgentControl.tsx`. Replace `Badge` and `stateTone` (:12-26):
```tsx
type Tone = "gray" | "green" | "blue" | "red" | "amber" | "indigo";

function Badge({ children, tone = "gray" }: { children: React.ReactNode; tone?: Tone }) {
  const colors = {
    gray: "bg-gray-100 text-gray-700", green: "bg-green-100 text-green-800", blue: "bg-blue-100 text-blue-800",
    red: "bg-red-100 text-red-800", amber: "bg-amber-100 text-amber-800", indigo: "bg-indigo-100 text-indigo-800",
  };
  return <span className={`rounded-full px-2.5 py-0.5 text-xs font-medium ${colors[tone]}`}>{children}</span>;
}

function stateTone(state: string): Tone {
  if (state === "running" || state === "dispatching") return "blue";
  if (state === "paused") return "indigo";
  if (state === "completed") return "green";
  if (state === "failed" || state === "interrupted") return "red";
  if (state === "waiting_input") return "amber";
  return "gray";
}
```
After `date()`:
```tsx
function PauseResume({ run, busy, inRow, onControl }: {
  run: ManagedRun;
  busy: boolean;
  inRow: boolean;
  onControl: (run: ManagedRun, action: "pause" | "resume") => void;
}) {
  if (run.state === "paused") {
    return <button type="button" disabled={busy || !run.resume_allowed} onClick={() => onControl(run, "resume")} aria-label={inRow ? `Resume ${run.job_id}` : undefined} className="rounded border border-indigo-300 px-2 py-1 text-xs text-indigo-700 disabled:opacity-40">Resume</button>;
  }
  if (run.state === "running" && run.interactive) {
    return <button type="button" disabled={busy || !run.pause_allowed} title={run.pause_allowed ? undefined : "Session id unknown; this run cannot be paused"} onClick={() => onControl(run, "pause")} aria-label={inRow ? `Pause ${run.job_id}` : undefined} className="rounded border border-gray-300 px-2 py-1 text-xs disabled:opacity-40">Pause</button>;
  }
  return null;
}
```
`runs` memo (:121):
```tsx
    return [...overview.active_runs, ...overview.paused_runs, ...overview.recent_runs].filter((run) => !seen.has(run.job_id) && Boolean(seen.add(run.job_id)));
```
After `stop()`:
```tsx
  async function control(run: ManagedRun, action: "pause" | "resume") {
    setBusy(true);
    setError(null);
    try {
      const result = action === "pause" ? await api.pauseManagedRun(run.job_id) : await api.resumeManagedRun(run.job_id);
      if (result.accepted !== true) throw new Error(`${action === "pause" ? "Pause" : "Resume"} was not accepted`);
      await load(true);
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : action === "pause" ? "Pause failed" : "Resume failed");
    } finally {
      setBusy(false);
    }
  }
```
Banner line (:229):
```tsx
          <strong>{overview.fleet_id}</strong> · {overview.environment} · {overview.controller_stale ? "controller stale" : "controller current"} · {overview.active_runs.length} active · {overview.paused_runs.length} paused
```
Run rows (:267-274). The row stops being a single `<button>` so that it can hold action buttons:
```tsx
          {runs.map((run) => (
            <div key={run.job_id} className={`rounded-lg border shadow-sm ${selectedJob === run.job_id ? "border-blue-400 bg-blue-50" : "border-gray-200 bg-white hover:border-gray-300"}`}>
              <button type="button" onClick={() => setSelectedJob(run.job_id)} className="w-full p-3 text-left">
                <div className="flex items-center justify-between gap-2"><strong className="truncate text-sm text-gray-900">{run.agent_name}</strong><Badge tone={stateTone(run.state)}>{run.state}</Badge></div>
                <p className="mt-1 font-mono text-xs text-gray-500">{run.job_id}</p>
                <p className="mt-1 truncate text-xs font-medium text-indigo-700">{run.task_ref ?? run.task_id ?? "Legacy run without task"}{run.task_title ? ` · ${run.task_title}` : ""}</p>
                <p className="mt-1 text-xs text-gray-500">{run.backend} · {run.node_name ?? "unknown node"} · {run.paused_at ? `paused ${date(run.paused_at)}` : date(run.last_activity_at)}</p>
              </button>
              {overview?.mutation_available && (run.state === "paused" || (run.state === "running" && run.interactive)) && (
                <div className="flex justify-end px-3 pb-3">
                  <PauseResume run={run} busy={busy} inRow onControl={(target, action) => void control(target, action)} />
                </div>
              )}
            </div>
          ))}
```
Detail buttons (:286-289):
```tsx
                    {!TERMINAL.has(selected.state) && overview?.mutation_available && <>
                      <PauseResume run={selected} busy={busy} inRow={false} onControl={(target, action) => void control(target, action)} />
                      <button type="button" disabled={busy} onClick={() => void stop(selected, false)} className="rounded border border-gray-300 px-2 py-1 text-xs">Stop</button>
                      <button type="button" disabled={busy} onClick={() => void stop(selected, true)} className="rounded border border-red-300 px-2 py-1 text-xs text-red-700">Force stop</button>
                    </>}
```
Detail `<dl>`, after the `Started` entry (:294):
```tsx
                  {selected.paused_at && <div><dt className="font-medium text-gray-400">Paused</dt><dd>{date(selected.paused_at)}</dd></div>}
```
- [ ] **Step 4: Run it and watch it pass.**
  - `yarn playwright test --config playwright.agent.config.ts tests/agent-interaction/agent-control.spec.ts` → 2 passed.
  - `yarn test:agent-ui` → the existing `progress.spec.ts` stays green.
  - `yarn check && yarn lint && yarn build:server`.
- [ ] **Step 5: Commit.**
```bash
git add src/lib/types.ts src/lib/api.ts src/pages/AgentControl.tsx tests/agent-interaction/agent-control.html tests/agent-interaction/agent-control.tsx tests/agent-interaction/agent-control.spec.ts
git commit -m "feat(spa): pause and resume managed runs, paused badge and count

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

### Task B6: Proto re-sync, full verification, push, PR

**Files:**
- Modify (only if Plan A's proto differs): `server/agentctrl/v1.proto`, plus the B1 check if Plan A renamed a field.

**Interfaces:**
- Consumes: Plan A merged to agent-ctrl `dev`, with `proto/agentctrl/v1.proto` at `T:/projects/SOTZ3N/stoz3n-agent-ctrl-pause/proto/agentctrl/v1.proto`.
- Produces: branch `feat/pause-and-second-node` pushed to `origin`, and a PR into `dev`.

- [ ] **Step 1: Copy the proto verbatim from Plan A.**
```bash
cd T:/projects/SOTZ3N/stoz3n-agent-dash-pause
cp T:/projects/SOTZ3N/stoz3n-agent-ctrl-pause/proto/agentctrl/v1.proto server/agentctrl/v1.proto
git diff --stat server/agentctrl/v1.proto
```
If the diff changes a request field the dashboard sends (`job_id`, `request_id`, `actor`, `source` on `PauseAgentRequest`/`ResumeAgentRequest`), renames the response fields `state`/`node_name` that `controlResponse` reads, or renames `paused_at`, `max_interactive_runs` or `running_interactive`, update the B1 check and the B3 `unary(...)` calls to match before continuing.
- [ ] **Step 2: Run the full gate** (the CI job in `.gitea/workflows/deploy.yml` runs install, lint, check, build:server and build):
```bash
yarn install --immutable
yarn lint
yarn check
npx tsc -p tsconfig.server.json --noEmit
yarn build:server
yarn test:agent-control
yarn test:agent-ui
yarn build
```
Every command must exit 0. `yarn test:agent-control` ends with `agent-control checks passed (<n>)`.
- [ ] **Step 3: Commit the re-synced proto (only if Step 1 changed it).**
```bash
git add server/agentctrl/v1.proto scripts/check-agent-control.mjs server/agent-control.ts
git commit -m "chore(proto): sync PauseAgent/ResumeAgent from agent-ctrl

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```
- [ ] **Step 4: Push.** `git push -u origin feat/pause-and-second-node`
- [ ] **Step 5: Open the PR** (merge needs the user's go-ahead; merging to `dev` deploys staging; merge only after Plan A is live on staging):
```bash
tea pr create --login obachan-dev-budelius --repo stozn/stoz3n-agent-dash \
  --head feat/pause-and-second-node --base dev \
  --title "feat(agent-control): pause and resume managed runs" \
  --description "$(cat <<'EOF'
Pause/resume for managed runs (Spec B of 2026-10-07-agentctrl-pause-and-second-node-design).

- POST /api/agent-control/runs/:id/pause and /resume -> FleetControl PauseAgent / ResumeAgent; body {}, Idempotency-Key as /stop; 200 { job_id, state, accepted: true, node_name }
- Errors: 404 not_found, 409 terminal_run / not_interactive / invalid_state / node_at_capacity / command_rejected, 412 session_unknown
- A paused run's /session (capabilities watch, stop, resume) and /events (terminal: false) keep answering 200, so attached viewers keep polling
- `paused` is an allowed, non-terminal state; runs gain paused_at, pause_allowed, resume_allowed; overview gains paused_runs
- Session descriptor capabilities: pause (running, interactive, session id known), resume (paused)
- UI: Pause/Resume in run rows and next to Stop, indigo `paused` badge, active/paused counts, node per row
- /repos passes the controller's pool union through unchanged
- Proto synced from agent-ctrl (PauseAgent, ResumeAgent, JobInfo.paused_at)

Requires the agent-ctrl pause/second-node change on staging first.

Tests: yarn test:agent-control, yarn test:agent-ui, yarn lint, yarn check, yarn build.

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```
- [ ] **Step 6: After the merge (user go-ahead), run a staging smoke test on `dash-backend-dev.stozn.ai`.**
  - Pause a running IWAKAN run from the run row. It shows `paused` with `paused_at`, and the paused count goes up.
  - Resume it. It returns to `running`, possibly with a different `node_name`.
  - A Pause on a run without a known session id is disabled.
