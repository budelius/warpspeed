# P1 — AgentCtrl Public Run API Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** WarpSpeed reaches AgentCtrl through obachan's APISIX at `agents-dev.obachan.dev` with its Zitadel token; every run has an owner who alone (with admins) sees and controls it, whose name is on the run's commits and on its model spend.

**Architecture:** A new `agent-ctrl api` subcommand (axum, same crate and image) serves the dashboard's `/api/agent-control/*` contract one-to-one plus an SSE event stream. It authenticates the Zitadel bearer itself (port of the dashboard's `zitadel-token.ts`), reads Mongo (`jobs`, `job_events`, `job_history`, `control_commands`, `nodes`, `fleet_snapshots`) through a `ReadModel` trait, and mutates through the existing FleetControl gRPC behind a `ControlPlane` trait. The controller gains run-owner fields, puts the owner into the run's spawn environment (git author, model attribution) and the node commits leftovers as the owner. APISIX gets a `bearer_only` app-route mode (JWKS validation, no login) and the route for the new host.

**Tech Stack:** Rust 1.94 (tokio, tonic 0.12, axum 0.7, jsonwebtoken 9, mongodb 3.5), Python 3.13 (render.py, obachan, pytest/unittest), Node 24 (dashboard golden export), Kubernetes, APISIX 3.10, Zitadel, LiteLLM.

**Spec:** `docs/superpowers/specs/2026-10-10-p1-agentctrl-public-api-design.md` (this branch).

## Global Constraints

- Staging only: namespace `agent-ctrl`, host `agents-dev.obachan.dev`. The `prod` instance renders no API resources.
- Zitadel issuer `https://id-dev.obachan.dev`, project `394361582064240430`, WarpSpeed client `394361583406417710`. Roles only from claim `urn:zitadel:iam:org:project:394361582064240430:roles`: `agent-admin` → admin, `agent-user` → member, neither → 401.
- Contract parity: paths, JSON shapes, status codes, error codes and messages identical to the dashboard on stoz3n-agent-dash `origin/dev` 9c33330 (`server/agent-control.ts`). The only API-side additions are: `owner_sub`/`owner_email` on run objects, `GET …/runs/{job}/events/stream`, members (not only admins) being served, and 422 `task_launch_unsupported` (Task 10).
- Idempotency key `^[A-Za-z0-9._:-]{16,128}$` → gRPC `request_id`; `Idempotency-Replayed` response header; 64 KiB JSON body cap; 32 MiB session file cap; 30 mutations / 60 s / actor → 429 `rate_limited` with `Retry-After: 60`.
- Never a 500 for dependency failures: controller → 503 `controller_unavailable`, Mongo → 503 `storage_unavailable`, JWKS unreachable → 401 (fail closed).
- Never log a bearer token, an `X-Userinfo` payload, a userinfo response or the control token. Tests use only the test key committed in Task 5.
- Every commit message ends with the line `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Merge to `dev` before any build or install. WarpSpeed is built and installed only from `T:/projects/dev-ops/warp-dev-release` on `dev`. Never restart, kill or relaunch a running WarpSpeed instance.
- Stage explicit paths only (`git add <path> …`); never `git add -A`, `git add .` or `git commit -a`. Other sessions edit these trees.
- Push targets: agent-ctrl, dashboard, obachan, obachan-config, passiv → `origin` (Gitea). WarpSpeed → `gitea` and `gitlab`, never `origin` (upstream warpdotdev).
- Never run `find /` in Git Bash on this machine; bound every search to a named directory or use Grep/Glob.
- Staging cluster access: `kubectl --context admin@obachan-staging --server https://10.10.1.5:6443 --tls-server-name 10.96.0.1 …` (the kubeconfig's own server address and `staging-gateway` are dead; never edit the shared kubeconfig).

---

## Repos, worktrees, branches

| Repo | Local checkout | New worktree | Branch | Base | PR target |
|---|---|---|---|---|---|
| agent-ctrl (Gitea `stozn/stoz3n-agent-ctrl`) | `T:/projects/SOTZ3N/stoz3n-agent-ctrl` | `T:/projects/SOTZ3N/stoz3n-agent-ctrl-p1-api` | `feat/p1-agentctrl-api` | `origin/dev` (a3cde04) | `dev` |
| dashboard (Gitea `stozn/stoz3n-agent-dash`) | `T:/projects/SOTZ3N/stoz3n-agent-dash` | `T:/projects/SOTZ3N/stoz3n-agent-dash-p1-golden` | `test/agent-control-golden-export` | `origin/dev` (9c33330) | `dev` |
| obachan (Gitea `pseekoo/obachan`) | `T:/projects/dev-ops/obachan` | `T:/projects/.worktrees/obachan-apisix-bearer-only` | `feat/apisix-bearer-only` | `origin/dev` (b8b7ea8f) | `dev` |
| obachan-config (Gitea `pseekoo/obachan-config`) | `T:/projects/dev-ops/obachan-config` (on `ops/agent-sandbox-w4`, leave it alone) | `T:/projects/.worktrees/obachan-config-agentctrl-api` | `feat/agentctrl-api-route` | `origin/main` (944cee3) | `main` |
| passiv (Gitea `pseekoo/passiv`) — only if Task 0 picks the end-user header | `T:/projects/dev-ops/passiv` | `T:/projects/.worktrees/passiv-owner-header` | `feat/litellm-owner-header` | `origin/main` | `main` |
| WarpSpeed (Gitea `stozn/warpspeed`) | `T:/projects/dev-ops/warp-dev-release` (on `dev`) | `T:/projects/dev-ops/warp-p1-base-url-override` | `feat/managed-base-url-override` | `gitea/dev` | `dev` |

obachan-config has no `dev` branch: `main` is the only integration branch and changes land by Gitea PR (#227, #228 are the pattern). Its local checkout is parked on another operator branch, so all work happens in the new worktree.

Create each worktree right before its first task:

```bash
git -C T:/projects/SOTZ3N/stoz3n-agent-ctrl fetch origin
git -C T:/projects/SOTZ3N/stoz3n-agent-ctrl worktree add -b feat/p1-agentctrl-api T:/projects/SOTZ3N/stoz3n-agent-ctrl-p1-api origin/dev
git -C T:/projects/SOTZ3N/stoz3n-agent-dash fetch origin
git -C T:/projects/SOTZ3N/stoz3n-agent-dash worktree add -b test/agent-control-golden-export T:/projects/SOTZ3N/stoz3n-agent-dash-p1-golden origin/dev
git -C T:/projects/dev-ops/obachan fetch origin
git -C T:/projects/dev-ops/obachan worktree add -b feat/apisix-bearer-only T:/projects/.worktrees/obachan-apisix-bearer-only origin/dev
git -C T:/projects/dev-ops/obachan-config fetch origin
git -C T:/projects/dev-ops/obachan-config worktree add -b feat/agentctrl-api-route T:/projects/.worktrees/obachan-config-agentctrl-api origin/main
git -C T:/projects/dev-ops/warp-dev-release fetch gitea
git -C T:/projects/dev-ops/warp-dev-release worktree add -b feat/managed-base-url-override T:/projects/dev-ops/warp-p1-base-url-override gitea/dev
```

If `origin/dev` of agent-ctrl moved past a3cde04, the `file:line` references below shift; locate each insertion by the quoted code instead of the number.

## Rulings (2026-10-10)

- Task-linked launches (`task_id`) are rejected by the API with 422 `task_launch_unsupported`; they stay on the dashboard.
- The dashboard allows live input for `agentctrl_api` runs (Task 12a).
- Rate limiting: the API's per-user limit is the real limit; APISIX's bearer route only carries a coarse flood guard
  (3000 requests / 60 s on one shared bucket), because APISIX sees Caddy's address as the client.

## Order and dependencies

```
Task 0 (spike) ─────────────────────────────────────────────┐
Tasks 1→2→3 (controller/node owner) ─┐                       │
Tasks 4→5→6→7→8→9→10→11 (api) ────────┼→ 12 (contract) → 13 (deploy manifests) → 14 (attribution, needs 0)
Task 12 also needs the dashboard export (12a)                │
Task 15 (obachan) → Task 16 (obachan-config)                 │
Task 17 (WarpSpeed override)                                 │
All → Task 18 (staging rollout) → Task 19 (E2E) ─────────────┘
```

Tasks 15–17 are independent of the agent-ctrl tasks and can run in parallel with them.

## File structure (agent-ctrl)

| File | Responsibility |
|---|---|
| `proto/agentctrl/v1.proto` | `owner_sub/owner_email/owner_name` on `SpawnAgentRequest`, `ImportSessionRequest`, `JobInfo` |
| `src/state/mod.rs` | `Job.owner_*` fields (mirrored to Mongo `jobs` by `upsert_job`) |
| `src/mongo/mod.rs` | owner index; `MongoStore::client_options`, `read_only_database` |
| `src/remote/controller.rs` | `RunOwner`, `run_owner()`, `apply_owner_env()`, owner plumbing through spawn/import/resume, `agentctrl_api` source |
| `src/remote/tls.rs` | `lazy_channel()` |
| `src/workspace_handoff.rs` | `RunAuthor`, `push_run_work_as`, `push_handoff_work_as`, owner env names |
| `src/remote/node.rs` | handoff finish and pause commit as the run author |
| `src/api/mod.rs` | `ApiConfig`, `AppState`, `state_from_env`, `run_api` |
| `src/api/error.rs` | `ApiError` JSON shape, gRPC status mapping |
| `src/api/cursor.rs` | event and page cursors (dashboard-compatible) |
| `src/api/auth.rs` | `Verifier` (JWKS cache, RS256, roles, profile), `Caller` extractor |
| `src/api/project.rs` | `public_run`/`public_event`/… projections, text sanitizers |
| `src/api/store.rs` | `ReadModel` trait, `JobQuery`, `MongoReadModel` |
| `src/api/ownership.rs` | owner filters and per-run visibility |
| `src/api/control.rs` | `ControlPlane` trait, `GrpcControlPlane` |
| `src/api/limits.rs` | idempotency key, media type, body, rate limiter, field validation |
| `src/api/handlers/mod.rs` | router, shared response helpers, per-run dispatch |
| `src/api/handlers/reads.rs` | overview, runs, run, session, events, history, repos, commands |
| `src/api/handlers/mutations.rs` | launch, import, history import, input/close/stop/pause/resume |
| `src/api/handlers/stream.rs` | SSE `events/stream` |
| `src/api/handlers/tests.rs` | handler, ownership, mutation and SSE tests |
| `src/api/fakes.rs` | test-only fakes: JWKS, userinfo, read model, control plane, token signing |
| `src/api/contract_tests.rs` | dashboard golden replay |
| `tests/fixtures/api/test-rsa-key.pem` | test-only RS256 key |
| `tests/fixtures/agent_control/{records,cases,golden}.json` | shared golden fixtures (copied from the dashboard) |
| `deploy/k8s/templates/api-{deployment,service,networkpolicy}.tmpl.yaml` | API workload |
| `deploy/k8s/render.py`, `instances/staging.toml`, `test_shared_workspaces.py` | render + tests |
| `deploy/k8s/agentctrl-api.obachan.app.toml` | Caddy route registration spec (operator-run) |
| `.gitea/workflows/deploy.yml` | API rollout status |
| `docs/p1-litellm-attribution-spike.md` | Task 0 evidence |

---

### Task 0: LiteLLM attribution spike

Decides which request attribute carries the run owner to LiteLLM's spend log, per CLI. No product code. Its result selects the variant in Task 14.

**Files:**
- Create: `T:/projects/SOTZ3N/stoz3n-agent-ctrl-p1-api/docs/p1-litellm-attribution-spike.md`

**Interfaces:**
- Produces: `MECHANISM` ∈ {`tags-header`, `end-user-header`} and a PASS/FAIL row per CLI (claude, codex, pi, kimi, dsh). Task 14 reads exactly these.

**Background (verified):** staging Claude workers call `https://gateway-dev.mezzanin.eu` with `ANTHROPIC_AUTH_TOKEN` = the staging LiteLLM key; codex, kimi and dsh call passiv (`http://passiv-litellm.passiv-litellm.svc.cluster.local:4000/v1`, `OPENAI_API_KEY` = the same key); pi uses passiv via `OPENAI_API_KEY`. LiteLLM's config is baked into the passiv image from `T:/projects/dev-ops/passiv/deploy/obachan/passiv-litellm/config.yaml` (`general_settings` at line 85 has no `user_header_name`; `litellm_settings.turn_off_message_logging: true` does not drop tags or end user).

Two candidate mechanisms:
- **tags-header** (no server change): request header `x-litellm-tags: owner:<email>` → spend-log column `request_tags` contains `owner:<email>`.
- **end-user-header** (server change): LiteLLM `general_settings.user_header_name: x-stozn-owner` → header `x-stozn-owner: <email>` → spend-log column `end_user` = `<email>`.

- [ ] **Step 1: Start a throwaway spike pod with the worker image and runtime secret (never exec into the live node pods)**

```bash
export KUBECONFIG=~/.kube/obachan-staging
K="kubectl --server https://staging-gateway:6443 --tls-server-name 10.96.0.1 -n agent-ctrl"
TAG=$($K get statefulset agent-ctrl-node -o jsonpath='{.spec.template.spec.containers[0].image}' | sed 's/.*://')
$K run p1-litellm-spike --restart=Never \
  --image "harbor.pseekoo.io/pseekoo-stoz3n-agent-ctrl/app:$TAG" \
  --overrides='{"spec":{"imagePullSecrets":[{"name":"harbor-pull"}],"containers":[{"name":"p1-litellm-spike","image":"harbor.pseekoo.io/pseekoo-stoz3n-agent-ctrl/app:'"$TAG"'","command":["sleep","3600"],"envFrom":[{"secretRef":{"name":"agent-ctrl-runtime"}}]}]}}'
$K wait --for=condition=Ready pod/p1-litellm-spike --timeout=180s
STAMP=$(date -u +%Y%m%d%H%M)
```

Expected: `pod/p1-litellm-spike condition met`.

- [ ] **Step 2: Phase A — tags header, one call per CLI**

Each call uses a unique marker `owner:spike-<cli>-$STAMP@stozn.test`. Run each command; record the exit code and whether the model replied.

```bash
# claude: ANTHROPIC_CUSTOM_HEADERS is newline-separated "Name: value"
$K exec p1-litellm-spike -- env ANTHROPIC_BASE_URL=https://gateway-dev.mezzanin.eu ANTHROPIC_MODEL=claude-sonnet \
  ANTHROPIC_CUSTOM_HEADERS="x-litellm-tags: owner:spike-claude-$STAMP@stozn.test" \
  claude -p "Reply with OK" --max-turns 1
# codex: provider http_headers in a scratch CODEX_HOME
$K exec p1-litellm-spike -- sh -c 'mkdir -p /tmp/codex && cat > /tmp/codex/config.toml <<TOML
model_provider = "passiv"
model = "gpt-5.4"
preferred_auth_method = "apikey"
[model_providers.passiv]
name = "passiv"
base_url = "http://passiv-litellm.passiv-litellm.svc.cluster.local:4000/v1"
env_key = "OPENAI_API_KEY"
wire_api = "responses"
http_headers = { "x-litellm-tags" = "owner:spike-codex-'"$STAMP"'@stozn.test" }
TOML
cd /tmp && CODEX_HOME=/tmp/codex codex exec --skip-git-repo-check "Reply with OK"'
```

For **pi** (0.52.6), **kimi** (2.1.1) and **dsh** (0.2.1-alpha.1): run `pi --help`, `kimi --help`, `dsh --help` in the pod and read the packaged docs (`$K exec p1-litellm-spike -- sh -c 'ls $(npm root -g)/@mariozechner/pi-coding-agent $(npm root -g)/@moonshot-ai/kimi-code $(npm root -g)/@deepseek-ai/dsh'`) for a per-provider custom-header setting (pi: `~/.pi/agent/models.json` provider `headers`; kimi: a `KIMI_MODEL_*`/config `headers` key; dsh: the provider block of the `/agent-config/dsh-passiv.patch.yml` overlay). Where one exists, make one call with `x-litellm-tags: owner:spike-<cli>-$STAMP@stozn.test` set through it against passiv. Where none exists, the CLI is FAIL without a call.

- [ ] **Step 3: Read the spend log for each marker**

```bash
# LiteLLM master key: OpenBao obachan/staging/passiv-litellm, property litellm_master_key
LITELLM_MASTER_KEY=$(bao kv get -field=litellm_master_key obachan/staging/passiv-litellm)
DAY=$(date -u +%F)
$K exec p1-litellm-spike -- curl -s -H "Authorization: Bearer $LITELLM_MASTER_KEY" \
  "http://passiv-litellm.passiv-litellm.svc.cluster.local:4000/spend/logs?start_date=$DAY&end_date=$(date -u -d tomorrow +%F)" \
  | jq --arg stamp "$STAMP" '[.[] | select(((.request_tags // []) | tostring | contains($stamp)) or ((.end_user // "") | contains($stamp))) | {request_id, model, request_tags, end_user, startTime}]'
unset LITELLM_MASTER_KEY
```

PASS for a CLI: at least one row whose `request_tags` contains `owner:spike-<cli>-$STAMP@stozn.test`. FAIL: the call succeeded (a spend row exists for the call's time window and model) but no row carries the marker, or the CLI has no header setting.

- [ ] **Step 4: Phase B — only if claude FAILED Phase A**

In a passiv worktree (`git -C T:/projects/dev-ops/passiv worktree add -b feat/litellm-owner-header T:/projects/.worktrees/passiv-owner-header origin/main`), add to `deploy/obachan/passiv-litellm/config.yaml` under `general_settings:` (after `master_key`):

```yaml
  # AgentCtrl attributes each run's model spend to its owner (P1): the owner's email arrives in
  # this header and lands in the spend log's end_user column.
  user_header_name: x-stozn-owner
```

Open a Gitea PR to `main`, merge, let passiv CI deploy staging, then repeat Steps 2–3 with header `x-stozn-owner: spike-<cli>-$STAMP@stozn.test` and the `end_user` column. If claude still FAILS, record `MECHANISM=none` and stop: Task 14 is then skipped and the spec gap is reported.

- [ ] **Step 5: Delete the spike pod**

```bash
$K delete pod p1-litellm-spike --wait=false
```

- [ ] **Step 6: Write the evidence file**

`docs/p1-litellm-attribution-spike.md` (fill every cell from Steps 2–4; no prompts, keys or full spend rows):

```markdown
# P1 LiteLLM attribution spike (2026-10-xx)

MECHANISM: tags-header | end-user-header | none
LiteLLM image digest: <from passiv deploy/obachan/passiv-litellm/Dockerfile FROM line>

| CLI | Version | Setting used | Marker | Result | Spend-log request_id |
|---|---|---|---|---|---|
| claude | 2.1.201 | ANTHROPIC_CUSTOM_HEADERS | owner:spike-claude-<stamp>@stozn.test | PASS/FAIL | <id> |
| codex | 0.142.5 | model_providers.passiv.http_headers | … | … | … |
| pi | 0.52.6 | <setting or "none found"> | … | … | … |
| kimi | 2.1.1 | <setting or "none found"> | … | … | … |
| dsh | 0.2.1-alpha.1 | <setting or "none found"> | … | … | … |

Untagged CLIs (spend not attributable in P1): <list>
```

- [ ] **Step 7: Commit**

```bash
cd T:/projects/SOTZ3N/stoz3n-agent-ctrl-p1-api
git add docs/p1-litellm-attribution-spike.md
git commit -m "docs(p1): LiteLLM owner attribution spike results

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 1: Run owner on jobs, requests and JobInfo; accept the API's mutation source

**Files:**
- Modify: `proto/agentctrl/v1.proto:128-139` (ImportSessionRequest), `:286-306` (SpawnAgentRequest), `:456-491` (JobInfo)
- Modify: `src/state/mod.rs:180-184` (Job struct end), `:228-235` (`Job::new` literal end)
- Modify: `src/mongo/mod.rs:584-587` (jobs index list)
- Modify: `src/remote/controller.rs` — `ControlContext`/`DispatchOptions` (50-90), `mutation_context` (225), `source_allows_live_input` (401-403), `dispatch_remote` (873-930), `spawn_agent` (2893-3053), `import_session` (3535-3634), `job_info` (4157-4224)
- Modify: `src/remote/client.rs:114-127`, `tests/remote_import.rs:33-44` (struct literals gain the new fields)
- Test: `src/remote/controller.rs` modules `auth_tests` (4445) and `import_session_tests` (4797)

**Interfaces:**
- Produces: proto fields `owner_sub`, `owner_email`, `owner_name` (string) on `SpawnAgentRequest` (13–15), `ImportSessionRequest` (11–13), `JobInfo` (35–37); `Job { owner_sub: String, owner_email: String, owner_name: String }` (all `#[serde(default)]`, empty = no owner); `pub(crate) struct RunOwner { sub, email, name: String }`; `fn run_owner(sub: &str, email: &str, name: &str) -> Result<Option<RunOwner>, Status>`; `DispatchOptions.owner: Option<&RunOwner>`; mutation source `"agentctrl_api"` accepted and allowed live input.

- [ ] **Step 1: Write the failing tests**

In `src/remote/controller.rs`, module `auth_tests`, next to `mutation_context_is_bounded_and_accepts_managed_sources` (≈4544), add:

```rust
    #[test]
    fn the_public_api_is_a_managed_mutation_source() {
        assert!(mutation_context("request-api-1", "@ada:example.test", "agentctrl_api").is_ok());
        assert!(source_allows_live_input("agentctrl_api"));
    }

    #[test]
    fn run_owners_are_bounded_and_optional() {
        assert_eq!(super::run_owner("", "", "").unwrap(), None);
        let owner = super::run_owner("394000000000000001", "ada@example.test", "Ada Owner")
            .unwrap()
            .expect("owner");
        assert_eq!(owner.sub, "394000000000000001");
        assert_eq!(owner.email, "ada@example.test");
        assert_eq!(owner.name, "Ada Owner");
        assert!(super::run_owner("394000000000000001", "", "").unwrap().is_some());
        assert!(super::run_owner("", "ada@example.test", "").is_err(), "email needs a subject");
        assert!(super::run_owner("bad\nsub", "", "").is_err());
        assert!(super::run_owner("s", "not-an-email", "").is_err());
        assert!(super::run_owner("s", "a <b>@example.test", "").is_err());
        assert!(super::run_owner("s", "a@example.test", "Ada <x>").is_err());
        assert!(super::run_owner(&"s".repeat(129), "", "").is_err());
    }
```

In module `import_session_tests`, add after `spawn_with_repo_url_dispatches_a_fresh_repo_run` (≈5353):

```rust
    fn owned(request: pb::SpawnAgentRequest) -> pb::SpawnAgentRequest {
        pb::SpawnAgentRequest {
            owner_sub: "394000000000000001".to_string(),
            owner_email: "ada@example.test".to_string(),
            owner_name: "Ada Owner".to_string(),
            ..request
        }
    }

    #[tokio::test]
    async fn an_owned_repo_run_records_its_owner() {
        let Harness {
            controller,
            node_rx,
            _tmp,
        } = harness(None);
        let spawned = acknowledge_spawn(controller.clone(), node_rx);
        let response = FleetControl::spawn_agent(
            &controller,
            tonic::Request::new(owned(repo_spawn_request("repo-runner"))),
        )
        .await
        .expect("owned repo run accepted")
        .into_inner();
        assert!(response.success, "{}", response.error);
        spawned.await.expect("spawn command");

        let job = load(&controller, &response.job_id);
        assert_eq!(job.owner_sub, "394000000000000001");
        assert_eq!(job.owner_email, "ada@example.test");
        assert_eq!(job.owner_name, "Ada Owner");
        let info = get_job(&controller, &response.job_id).await;
        assert_eq!(info.owner_sub, "394000000000000001");
        assert_eq!(info.owner_email, "ada@example.test");
        assert_eq!(info.owner_name, "Ada Owner");
    }

    #[tokio::test]
    async fn an_unowned_run_has_no_owner() {
        let Harness {
            controller,
            node_rx,
            _tmp,
        } = harness(None);
        let spawned = acknowledge_spawn(controller.clone(), node_rx);
        let response = FleetControl::spawn_agent(
            &controller,
            tonic::Request::new(repo_spawn_request("repo-runner")),
        )
        .await
        .unwrap()
        .into_inner();
        spawned.await.unwrap();
        assert!(load(&controller, &response.job_id).owner_sub.is_empty());
    }

    #[tokio::test]
    async fn an_invalid_owner_is_rejected_before_dispatch() {
        let Harness {
            controller, _tmp, ..
        } = harness(None);
        let status = FleetControl::spawn_agent(
            &controller,
            tonic::Request::new(pb::SpawnAgentRequest {
                owner_email: "ada@example.test".to_string(),
                ..repo_spawn_request("repo-runner")
            }),
        )
        .await
        .expect_err("an email without a subject is invalid");
        assert_eq!(status.code(), tonic::Code::InvalidArgument);
    }
```

The owner is also folded into the launch/import idempotency fingerprint (Step 7), so an `Idempotency-Key` reused by another person conflicts in `control_commands` instead of replaying the first person's launch. Replays are only stored with Mongo, which the unit harness does not have, so no unit test covers it; the Task 19 E2E does not rely on it either.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd T:/projects/SOTZ3N/stoz3n-agent-ctrl-p1-api && cargo test --lib owner`
Expected: compile errors — `no field owner_sub on type SpawnAgentRequest`, `cannot find function run_owner`.

- [ ] **Step 3: Proto fields**

In `proto/agentctrl/v1.proto`, append to `message ImportSessionRequest` (after `string source = 10;`):

```proto
  // The person the run belongs to, as the public API authenticated them. Empty for runs started
  // by the CLI, schedules or work sources.
  string owner_sub = 11;
  string owner_email = 12;
  string owner_name = 13;
```

Append to `message SpawnAgentRequest` (after `string base_branch = 12;`):

```proto
  // The person the run belongs to; see ImportSessionRequest.owner_sub.
  string owner_sub = 13;
  string owner_email = 14;
  string owner_name = 15;
```

Append to `message JobInfo` (after `bool pause_push_failed = 34;`):

```proto
  string owner_sub = 35;
  string owner_email = 36;
  string owner_name = 37;
```

- [ ] **Step 4: Job fields and Mongo index**

In `src/state/mod.rs`, after `pub event_base: u64,` (line 183) add:

```rust
    /// Zitadel subject of the person the run belongs to; empty for runs without an owner.
    #[serde(default)]
    pub owner_sub: String,
    #[serde(default)]
    pub owner_email: String,
    /// The owner's display name, used as the git author name of the run's commits.
    #[serde(default)]
    pub owner_name: String,
```

In `Job::new`, after `event_base: 0,` (line 234) add:

```rust
            owner_sub: String::new(),
            owner_email: String::new(),
            owner_name: String::new(),
```

In `src/mongo/mod.rs`, in the `jobs` index list after the `"fleet_task_created_at"` entry (line 584-587) add:

```rust
            (
                "fleet_owner_created_at",
                doc! { "fleet_id": 1, "owner_sub": 1, "created_at": -1 },
            ),
```

`upsert_job` serializes the whole `Job`, so the fields reach Mongo `jobs` without further changes.

- [ ] **Step 5: Controller — owner type, validation, source**

In `src/remote/controller.rs`, after `struct TaskAttachment { … }` (line 64) add:

```rust
/// The person a run belongs to, as the public API authenticated them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct RunOwner {
    pub sub: String,
    pub email: String,
    pub name: String,
}

/// The run owner a FleetControl request names, if any. Owners are only ever set by the public
/// API; an email or name without a subject is a caller bug.
#[allow(clippy::result_large_err)]
fn run_owner(
    sub: &str,
    email: &str,
    name: &str,
) -> std::result::Result<Option<RunOwner>, Status> {
    if sub.is_empty() {
        if email.is_empty() && name.is_empty() {
            return Ok(None);
        }
        return Err(Status::invalid_argument("owner_email and owner_name require owner_sub"));
    }
    let unsafe_char = |c: char| c.is_control() || c == '<' || c == '>';
    if sub.len() > 128 || sub.chars().any(unsafe_char) {
        return Err(Status::invalid_argument("invalid owner_sub"));
    }
    if !email.is_empty()
        && (email.len() > 254
            || !email.contains('@')
            || email.chars().any(|c| unsafe_char(c) || c.is_whitespace()))
    {
        return Err(Status::invalid_argument("invalid owner_email"));
    }
    if name.chars().count() > 128 || name.chars().any(unsafe_char) {
        return Err(Status::invalid_argument("invalid owner_name"));
    }
    Ok(Some(RunOwner {
        sub: sub.to_string(),
        email: email.to_string(),
        name: name.to_string(),
    }))
}
```

Add the field to `DispatchOptions` (line 83-90):

```rust
    run_repo: Option<&'a RunRepo>,
    owner: Option<&'a RunOwner>,
}
```

In `mutation_context` (line 225) and `source_allows_live_input` (line 402) replace the source pattern `"dashboard" | "warp" | "warpspeed" | "direct_cli"` with:

```rust
"dashboard" | "warp" | "warpspeed" | "agentctrl_api" | "direct_cli"
```

- [ ] **Step 6: Controller — record the owner on the job**

In `dispatch_remote`, extend the destructuring (line 873-880):

```rust
        let DispatchOptions {
            schedule_name,
            interactive_override,
            control,
            task,
            moved,
            run_repo,
            owner,
        } = options;
```

After the `if let Some(control) = control { … } else { … }` block that sets `job.source` (ends ≈line 932), add:

```rust
        if let Some(owner) = owner {
            job.owner_sub = owner.sub.clone();
            job.owner_email = owner.email.clone();
            job.owner_name = owner.name.clone();
        }
```

- [ ] **Step 7: Controller — parse the owner in SpawnAgent and ImportSession, and fingerprint it**

In `spawn_agent`, directly after `let run_repo = run_repo(&req)?;` (≈2900) add:

```rust
        let owner = run_owner(&req.owner_sub, &req.owner_email, &req.owner_name)?;
```

Before `let fingerprint = ControllerInner::command_fingerprint(&fingerprint_parts);` (line 3012) add:

```rust
        // A request id reused by another owner must conflict, never replay their launch.
        if let Some(owner) = &owner {
            fingerprint_parts.extend(["owner", owner.sub.as_str()]);
        }
```

In the `DispatchOptions { … }` literal of `spawn_agent` (line 3036-3042) add `owner: owner.as_ref(),` before `..DispatchOptions::default()`.

In `import_session`, directly after `let mut moved = moved_session(&mut req)?;` (≈3542) add:

```rust
        let owner = run_owner(&req.owner_sub, &req.owner_email, &req.owner_name)?;
```

Replace the fingerprint statement (line 3580-3589):

```rust
        let session_sha256 = format!("{:x}", Sha256::digest(&moved.file));
        let mut fingerprint_parts = vec![
            "import_session",
            &agent_name,
            &pool,
            &moved.session_id,
            &session_sha256,
            &moved.repo_url,
            &moved.branch,
            &moved.source_cwd,
        ];
        if let Some(owner) = &owner {
            fingerprint_parts.extend(["owner", owner.sub.as_str()]);
        }
        let fingerprint = ControllerInner::command_fingerprint(&fingerprint_parts);
```

In its `DispatchOptions { … }` literal (line 3618-3625) add `owner: owner.as_ref(),` after `run_repo: None,`.

- [ ] **Step 8: Controller — expose the owner in JobInfo**

In `job_info`, after `pause_push_failed: j.pause_push_failed,` (line 4223) add:

```rust
        owner_sub: j.owner_sub.clone(),
        owner_email: j.owner_email.clone(),
        owner_name: j.owner_name.clone(),
```

- [ ] **Step 9: Fix exhaustive struct literals**

Run: `cargo test --locked --no-run 2>&1 | grep -B2 -A6 "E0063"` (or without `--locked` the first time; see Step 10). For every reported `missing fields owner_email, owner_name and owner_sub` add

```rust
            owner_sub: String::new(),
            owner_email: String::new(),
            owner_name: String::new(),
```

Known sites on a3cde04: `src/remote/client.rs:114` (`pb::SpawnAgentRequest`), `tests/remote_import.rs:33` (`pb::ImportSessionRequest`), `src/remote/controller.rs:4946` (`valid_request`), `:5041`, `:5687`. Literals ending in `..Default::default()` need nothing.

- [ ] **Step 10: Run the tests to verify they pass**

Run: `cargo test --lib owner && cargo test --lib managed_mutation_source && cargo test --locked`
Expected: the five new tests PASS; the full suite passes (Mongo-backed tests print `skipping … AGENT_CTRL_TEST_MONGO_URI is unset`).

- [ ] **Step 11: Commit**

```bash
git add proto/agentctrl/v1.proto src/state/mod.rs src/mongo/mod.rs src/remote/controller.rs src/remote/client.rs tests/remote_import.rs
git commit -m "feat(controller): record the run owner and accept the public API source

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: Owner as git author on start, import and resume

**Files:**
- Modify: `src/workspace_handoff.rs` (owner env names, top of file after the `use` lines)
- Modify: `src/remote/controller.rs` — new `apply_owner_env` (after `run_owner`), `dispatch_remote` after `let mut cmd = super::spawn_command(…)` (line 967-973), `dispatch_resume` after `let mut cmd = super::spawn_command(…)` (line 1142-1148)
- Test: `src/remote/controller.rs` module `import_session_tests`

**Interfaces:**
- Consumes: `Job.owner_*` (Task 1).
- Produces: `pub const OWNER_NAME_ENV: &str = "AGENT_CTRL_OWNER_NAME"`, `pub const OWNER_EMAIL_ENV: &str = "AGENT_CTRL_OWNER_EMAIL"` in `crate::workspace::handoff`; `pub(crate) fn apply_owner_env(env: &mut std::collections::HashMap<String, String>, job: &Job)` — when `job.owner_email` is non-empty sets `GIT_AUTHOR_NAME` (owner name, else email), `GIT_AUTHOR_EMAIL`, `AGENT_CTRL_OWNER_NAME`, `AGENT_CTRL_OWNER_EMAIL`; never touches `GIT_COMMITTER_*`. Task 14 extends this function.

- [ ] **Step 1: Write the failing tests**

In module `import_session_tests` add:

```rust
    fn owner_env(command: &pb::SpawnCommand, key: &str) -> Option<String> {
        command.env_vars.get(key).cloned()
    }

    #[tokio::test]
    async fn an_owned_run_names_its_owner_as_git_author() {
        let Harness {
            controller,
            node_rx,
            _tmp,
        } = harness(None);
        let spawned = acknowledge_spawn(controller.clone(), node_rx);
        let response = FleetControl::spawn_agent(
            &controller,
            tonic::Request::new(owned(repo_spawn_request("repo-runner"))),
        )
        .await
        .unwrap()
        .into_inner();
        assert!(response.success, "{}", response.error);
        let command = spawned.await.unwrap();
        assert_eq!(owner_env(&command, "GIT_AUTHOR_NAME").as_deref(), Some("Ada Owner"));
        assert_eq!(owner_env(&command, "GIT_AUTHOR_EMAIL").as_deref(), Some("ada@example.test"));
        assert_eq!(owner_env(&command, "AGENT_CTRL_OWNER_EMAIL").as_deref(), Some("ada@example.test"));
        assert_eq!(owner_env(&command, "GIT_COMMITTER_NAME"), None, "the node stays committer");
    }

    #[tokio::test]
    async fn an_unowned_run_keeps_the_profile_git_identity() {
        let Harness {
            controller,
            node_rx,
            _tmp,
        } = harness(None);
        let spawned = acknowledge_spawn(controller.clone(), node_rx);
        FleetControl::spawn_agent(
            &controller,
            tonic::Request::new(repo_spawn_request("repo-runner")),
        )
        .await
        .unwrap();
        let command = spawned.await.unwrap();
        assert_eq!(owner_env(&command, "GIT_AUTHOR_NAME"), None);
        assert_eq!(owner_env(&command, "AGENT_CTRL_OWNER_EMAIL"), None);
    }

    #[tokio::test]
    async fn an_owned_import_names_its_owner_as_git_author() {
        let Harness {
            controller,
            node_rx,
            _tmp,
        } = harness(None);
        let spawned = acknowledge_spawn(controller.clone(), node_rx);
        let request = pb::ImportSessionRequest {
            owner_sub: "394000000000000001".to_string(),
            owner_email: "ada@example.test".to_string(),
            owner_name: "Ada Owner".to_string(),
            ..valid_request("direct_cli")
        };
        let response = FleetControl::import_session(&controller, tonic::Request::new(request))
            .await
            .expect("import accepted")
            .into_inner();
        assert!(response.success, "{}", response.error);
        let command = spawned.await.unwrap();
        assert_eq!(owner_env(&command, "GIT_AUTHOR_EMAIL").as_deref(), Some("ada@example.test"));
        assert_eq!(load(&controller, &response.job_id).owner_sub, "394000000000000001");
    }

    #[tokio::test]
    async fn a_resumed_owned_run_keeps_its_owner_as_git_author() {
        let Harness {
            controller,
            node_rx,
            _tmp,
        } = harness(None);
        let (job_id, _agent_id, mut node_rx) =
            paused_repo_job(&controller, node_rx, b"transcript", "g-1", false).await;
        let mut job = load(&controller, &job_id);
        job.owner_sub = "394000000000000001".into();
        job.owner_email = "ada@example.test".into();
        job.owner_name = "Ada Owner".into();
        controller.fleet.state().save_job(&job).unwrap();
        let (response, command) = tokio::join!(
            FleetControl::resume_agent(
                &controller,
                tonic::Request::new(resume_request(&job_id, "resume-owner-1"))
            ),
            accept_spawn(&controller, &mut node_rx, "agent-2"),
        );
        assert!(response.unwrap().into_inner().accepted);
        assert_eq!(owner_env(&command, "GIT_AUTHOR_NAME").as_deref(), Some("Ada Owner"));
        assert_eq!(owner_env(&command, "GIT_AUTHOR_EMAIL").as_deref(), Some("ada@example.test"));
    }

    #[test]
    fn an_owner_without_email_sets_no_git_author() {
        let mut job = crate::state::Job::new("repo-runner", "claude");
        job.owner_sub = "394000000000000001".into();
        job.owner_name = "Ada Owner".into();
        let mut env = std::collections::HashMap::new();
        super::apply_owner_env(&mut env, &job);
        assert!(env.is_empty());
        job.owner_email = "ada@example.test".into();
        job.owner_name.clear();
        super::apply_owner_env(&mut env, &job);
        assert_eq!(env.get("GIT_AUTHOR_NAME").map(String::as_str), Some("ada@example.test"));
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --lib git_author`
Expected: FAIL to compile — `cannot find function apply_owner_env`.

- [ ] **Step 3: Owner env names**

In `src/workspace_handoff.rs`, after the `use` lines (line 6) add:

```rust
/// The run owner's display name and email in a run's spawn environment, set by the controller
/// only for runs that have an owner. The node commits a run's leftovers as this author.
pub const OWNER_NAME_ENV: &str = "AGENT_CTRL_OWNER_NAME";
pub const OWNER_EMAIL_ENV: &str = "AGENT_CTRL_OWNER_EMAIL";
```

- [ ] **Step 4: `apply_owner_env` and its call sites**

In `src/remote/controller.rs`, after `fn run_owner` add:

```rust
/// Puts the run owner into a spawn environment: the agent's own commits name the owner as
/// author (the committer stays the node's identity), and the node commits leftovers as the
/// same author. Runs without an owner email are left as configured.
pub(crate) fn apply_owner_env(env: &mut std::collections::HashMap<String, String>, job: &Job) {
    use crate::workspace::handoff::{OWNER_EMAIL_ENV, OWNER_NAME_ENV};
    if job.owner_email.is_empty() {
        return;
    }
    let name = if job.owner_name.is_empty() {
        job.owner_email.clone()
    } else {
        job.owner_name.clone()
    };
    env.insert("GIT_AUTHOR_NAME".to_string(), name.clone());
    env.insert("GIT_AUTHOR_EMAIL".to_string(), job.owner_email.clone());
    env.insert(OWNER_NAME_ENV.to_string(), name);
    env.insert(OWNER_EMAIL_ENV.to_string(), job.owner_email.clone());
}
```

In `dispatch_remote`, directly after the `let mut cmd = super::spawn_command(…);` statement (line 967-973) add:

```rust
        apply_owner_env(&mut cmd.env_vars, &job);
```

In `dispatch_resume`, directly after its `let mut cmd = super::spawn_command(…);` statement (line 1142-1148) add the same line. The import path dispatches through `dispatch_remote`, so start, import and resume are all covered.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test --lib git_author && cargo test --lib owner && cargo test --locked`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add src/workspace_handoff.rs src/remote/controller.rs
git commit -m "feat(controller): name the run owner as git author on start, import and resume

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: Node commits a run's leftovers as its owner

**Files:**
- Modify: `src/workspace_handoff.rs:210-270` (`push_handoff_work`, `push_run_work`)
- Modify: `src/remote/node.rs:10-16` (imports), `Handoff` impl (`finish` at 907-908, `pause` at 957)
- Test: `src/workspace_handoff.rs` module `tests` (after `a_pause_without_commits_still_creates_the_run_branch_on_origin`, ≈1131)

**Interfaces:**
- Consumes: `OWNER_NAME_ENV`, `OWNER_EMAIL_ENV` (Task 2).
- Produces: `pub struct RunAuthor { pub name: String, pub email: String }` with `pub fn from_env(env: &HashMap<String, String>) -> Option<RunAuthor>`; `pub async fn push_run_work_as(worktree, branch, job_id, label, author: Option<&RunAuthor>)`; `pub async fn push_handoff_work_as(worktree, branch, job_id, author: Option<&RunAuthor>)`. `push_run_work` / `push_handoff_work` keep their signatures (author `None`).

The paused-push retry (`retry_paused_push`) and the paused-run discard (`discard_paused_run`) keep `push_run_work` without an author: by then the pause has already committed the run's work as the owner, and they only push.

- [ ] **Step 1: Write the failing tests**

```rust
    #[tokio::test]
    async fn an_owned_run_commits_its_leftovers_as_the_owner() {
        let tmp = tempfile::tempdir().unwrap();
        let (config, url) = test_support::handoff_fixture(tmp.path());
        let worktree = prepare_run_worktree(&config, &url, None, "job-1")
            .await
            .unwrap();
        std::fs::write(worktree.join("left.txt"), "uncommitted").unwrap();
        let author = RunAuthor {
            name: "Ada Owner".into(),
            email: "ada@example.test".into(),
        };

        push_run_work_as(&worktree, &agent_branch("job-1"), "job-1", "pause", Some(&author))
            .await
            .unwrap();

        assert_eq!(
            test_support::stdout(&worktree, &["log", "-1", "--format=%an <%ae>"]),
            "Ada Owner <ada@example.test>"
        );
        assert_eq!(
            test_support::stdout(Path::new(&url), &["rev-parse", "agent/job-1"]),
            test_support::stdout(&worktree, &["rev-parse", "HEAD"])
        );
    }

    #[test]
    fn the_run_author_comes_only_from_the_owner_env() {
        let mut env = std::collections::HashMap::new();
        env.insert("GIT_AUTHOR_NAME".to_string(), "stoz3n-agent-staging".to_string());
        env.insert("GIT_AUTHOR_EMAIL".to_string(), "agents@staging.pseekoo.io".to_string());
        assert_eq!(RunAuthor::from_env(&env), None, "profile identities are not owners");
        env.insert(OWNER_NAME_ENV.to_string(), "Ada Owner".to_string());
        env.insert(OWNER_EMAIL_ENV.to_string(), "ada@example.test".to_string());
        assert_eq!(
            RunAuthor::from_env(&env),
            Some(RunAuthor {
                name: "Ada Owner".into(),
                email: "ada@example.test".into()
            })
        );
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --lib run_author`
Expected: compile error `cannot find struct RunAuthor`.

- [ ] **Step 3: Implement**

In `src/workspace_handoff.rs`, after the `OWNER_*_ENV` constants add:

```rust
/// The author a run's own commits name: its owner. The committer stays the node's identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunAuthor {
    pub name: String,
    pub email: String,
}

impl RunAuthor {
    /// The owner the controller put into the run's spawn environment, if the run has one.
    pub fn from_env(env: &std::collections::HashMap<String, String>) -> Option<Self> {
        let name = env.get(OWNER_NAME_ENV).filter(|value| !value.is_empty())?;
        let email = env.get(OWNER_EMAIL_ENV).filter(|value| !value.is_empty())?;
        Some(Self {
            name: name.clone(),
            email: email.clone(),
        })
    }
}
```

Replace `push_handoff_work` (line 210-219) with:

```rust
/// Commits whatever the agent left uncommitted as `agent-ctrl run <job_id> end` and pushes the
/// run branch, so removing the worktree loses nothing; see [`push_run_work`].
pub async fn push_handoff_work(
    worktree: &Path,
    branch: &str,
    job_id: &str,
) -> Result<(), BackendError> {
    push_handoff_work_as(worktree, branch, job_id, None).await
}

/// [`push_handoff_work`], committing as `author` when given.
pub async fn push_handoff_work_as(
    worktree: &Path,
    branch: &str,
    job_id: &str,
    author: Option<&RunAuthor>,
) -> Result<(), BackendError> {
    push_run_work_as(worktree, branch, job_id, "end", author).await
}
```

Rename the existing `pub async fn push_run_work(worktree, branch, job_id, label)` to `push_run_work_as` with the extra parameter, and add the old name as a wrapper above it:

```rust
/// [`push_run_work_as`] with no run author.
pub async fn push_run_work(
    worktree: &Path,
    branch: &str,
    job_id: &str,
    label: &str,
) -> Result<(), BackendError> {
    push_run_work_as(worktree, branch, job_id, label, None).await
}

/// Commits whatever the agent left uncommitted as `agent-ctrl run <job_id> <label>` (authored by
/// `author` when given) and pushes the run's local branch (HEAD when there is none) to the run
/// branch. A no-op when there is nothing new, except that a pause still creates the run branch on
/// origin; a branch moved by someone else makes the push fail rather than overwrite it, and
/// commits the push cannot reach (on a detached HEAD) make it an error.
pub async fn push_run_work_as(
    worktree: &Path,
    branch: &str,
    job_id: &str,
    label: &str,
    author: Option<&RunAuthor>,
) -> Result<(), BackendError> {
```

Inside the commit block, replace `args.extend(["commit", "--no-verify", "-m", &message]);` with:

```rust
        let author_arg = author.map(|author| format!("--author={} <{}>", author.name, author.email));
        args.extend(["commit", "--no-verify", "-m", &message]);
        if let Some(author_arg) = &author_arg {
            args.push(author_arg);
        }
```

The rest of the body is unchanged.

In `src/remote/node.rs`, change the import list (line 10-16): replace `push_handoff_work,` with `push_handoff_work_as,` and `push_run_progress, push_run_work,` with `push_run_progress, push_run_work, push_run_work_as,`, and add `RunAuthor,` after `PauseDetails,`.

In `impl Handoff`, after `fn run_dir` add:

```rust
    /// The run owner its leftover commits are authored as, when the run has one.
    fn author(&self) -> Option<RunAuthor> {
        RunAuthor::from_env(&self.env)
    }
```

In `Handoff::finish` (line 908) replace `push_handoff_work(&self.worktree, &self.branch, &self.job_id)` with `push_handoff_work_as(&self.worktree, &self.branch, &self.job_id, self.author().as_ref())`.

In `Handoff::pause` (line 957) replace `push_run_work(&self.worktree, &self.branch, &self.job_id, "pause")` with `push_run_work_as(&self.worktree, &self.branch, &self.job_id, "pause", self.author().as_ref())`.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --lib run_author && cargo test --lib leftovers_as_the_owner && cargo test --locked`
Expected: PASS (git must be on PATH; the handoff tests already require it).

- [ ] **Step 5: Commit**

```bash
git add src/workspace_handoff.rs src/remote/node.rs
git commit -m "feat(node): commit a run's leftovers and pause work as its owner

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: API scaffold — dependencies, module, errors, cursors, lazy clients

**Files:**
- Modify: `Cargo.toml` (`[dependencies]` after the `reqwest` line; `[dev-dependencies]`), `Cargo.lock` (regenerated)
- Modify: `src/lib.rs:6` (module list)
- Create: `src/api/mod.rs`, `src/api/error.rs`, `src/api/cursor.rs`
- Modify: `src/remote/tls.rs:131-146` (`connect_channel`)
- Modify: `src/mongo/mod.rs:432-456` (`MongoStore::connect`)

**Interfaces:**
- Produces: `crate::api::ApiConfig { environment: String, fleet_id: String, mutations_enabled: bool, actor_domain: String }` with `fn fleet_filter(&self) -> Option<&str>`; `crate::api::error::ApiError { status: StatusCode, code: &'static str, message: String }` with `new(status: u16, message, code)`, `not_found()`, `run_not_found()`, `unauthenticated()`, `storage()`, `from_status(&tonic::Status, state_refusals: bool)`, `IntoResponse`; `crate::api::cursor::{BASE64URL, encode_event_cursor(u64) -> Option<String>, decode_event_cursor(Option<&str>) -> Result<u64, ApiError>, encode_page_cursor(i64, &str) -> String, decode_page_cursor(&str) -> Result<(i64, String), ApiError>}`; `crate::remote::tls::lazy_channel(String) -> Result<Channel>`; `MongoStore::read_only_database(uri, database, tls) -> Result<mongodb::Database>`.

- [ ] **Step 1: Dependencies**

In `Cargo.toml`, after `reqwest = …` add:

```toml
# Public run API (agent-ctrl api)
axum = { version = "0.7", default-features = false, features = ["http1", "json", "query", "multipart", "tokio"] }
jsonwebtoken = "9"
base64 = "0.22"
```

In `[dev-dependencies]` add:

```toml
tower = { version = "0.5", features = ["util"] }
http-body-util = "0.1"
tokio = { version = "1", features = ["full", "test-util"] }
```

In `src/lib.rs`, add `pub mod api;` as the first `pub mod` line (before `pub mod agent;`).

- [ ] **Step 2: Write the failing tests**

Create `src/api/mod.rs`:

```rust
//! The public run API (`agent-ctrl api`): the STOZN dashboard's `/api/agent-control/*` contract
//! served by AgentCtrl itself to bearer-token clients such as WarpSpeed, where members see and
//! control only the runs they own.

pub mod cursor;
pub mod error;

/// Deployment settings the routes depend on.
#[derive(Clone, Debug)]
pub struct ApiConfig {
    /// The `environment` of the jobs and fleet snapshot this API serves, e.g. `staging`.
    pub environment: String,
    /// Fleet id the API is pinned to; empty serves whichever fleet reports `environment`.
    pub fleet_id: String,
    /// Rollout gate for every mutation.
    pub mutations_enabled: bool,
    /// Domain of the audit actor `@<localpart>:<domain>` recorded with each mutation.
    pub actor_domain: String,
}

impl ApiConfig {
    pub fn fleet_filter(&self) -> Option<&str> {
        (!self.fleet_id.is_empty()).then_some(self.fleet_id.as_str())
    }
}
```

Create `src/api/error.rs` with only its test module first:

```rust
//! The API's JSON error body and the FleetControl status mapping shared with the dashboard.

#[cfg(test)]
mod tests {
    use super::*;
    use http_body_util::BodyExt;
    use tonic::Status;

    fn mapped(status: Status, state_refusals: bool) -> (u16, &'static str, String) {
        let error = ApiError::from_status(&status, state_refusals);
        (error.status.as_u16(), error.code, error.message)
    }

    #[test]
    fn grpc_statuses_map_like_the_dashboard() {
        assert_eq!(
            mapped(Status::unauthenticated("bad token"), false),
            (503, "controller_unavailable", "Controller authentication failed".into())
        );
        assert_eq!(mapped(Status::permission_denied("no"), false).1, "forbidden");
        assert_eq!(
            mapped(Status::not_found("x"), false),
            (404, "not_found", "Run not found".into())
        );
        assert_eq!(
            mapped(Status::failed_precondition("session id unknown"), true),
            (412, "session_unknown", "Session id unknown".into())
        );
        assert_eq!(
            mapped(Status::failed_precondition("agent does not permit operator interaction"), true).1,
            "not_interactive"
        );
        assert_eq!(
            mapped(Status::failed_precondition("job is not paused"), true),
            (409, "invalid_state", "job is not paused".into())
        );
        assert_eq!(mapped(Status::failed_precondition("job is not paused"), false).1, "conflict");
        assert_eq!(
            mapped(Status::already_exists("request fingerprint mismatch"), false),
            (409, "conflict", "request fingerprint mismatch".into())
        );
        assert_eq!(
            mapped(Status::resource_exhausted("node at capacity"), false),
            (409, "node_at_capacity", "node at capacity".into())
        );
        assert_eq!(
            mapped(Status::invalid_argument("bad"), false),
            (422, "validation_error", "bad".into())
        );
        assert_eq!(
            mapped(Status::unavailable("down"), false),
            (503, "controller_unavailable", "Controller is unavailable".into())
        );
    }

    #[tokio::test]
    async fn server_errors_hide_their_message_and_rate_limits_say_when_to_retry() {
        let response =
            ApiError::new(503, "mongo exploded at host x", "storage_unavailable").into_response();
        assert_eq!(response.headers()["cache-control"], "no-store");
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body).unwrap(),
            serde_json::json!({"error": "Service unavailable", "code": "storage_unavailable"})
        );
        let limited = ApiError::new(429, "Mutation rate limit exceeded", "rate_limited").into_response();
        assert_eq!(limited.headers()["retry-after"], "60");
    }
}
```

Create `src/api/cursor.rs` with only its test module first:

```rust
//! Opaque cursors, byte-compatible with the dashboard's so clients can switch hosts mid-session.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_cursors_match_the_dashboard_encoding() {
        assert_eq!(encode_event_cursor(0), None);
        assert_eq!(encode_event_cursor(1).as_deref(), Some("djE6MQ"));
        assert_eq!(encode_event_cursor(4).as_deref(), Some("djE6NA"));
        assert_eq!(decode_event_cursor(None).unwrap(), 0);
        assert_eq!(decode_event_cursor(Some("")).unwrap(), 0);
        assert_eq!(decode_event_cursor(Some("djE6MQ")).unwrap(), 1);
        assert_eq!(decode_event_cursor(Some("djE6MQ==")).unwrap(), 1);
        for bad in ["nope", "djI6MQ", "djE6", "djE6LTE"] {
            assert_eq!(decode_event_cursor(Some(bad)).unwrap_err().code, "invalid_cursor", "{bad}");
        }
    }

    #[test]
    fn page_cursors_are_the_dashboards_json_in_base64url() {
        let cursor = encode_page_cursor(1_760_000_000_123, "job-1");
        assert_eq!(
            BASE64URL.decode(&cursor).unwrap(),
            br#"{"v":1,"time":1760000000123,"id":"job-1"}"#
        );
        assert!(!cursor.contains('='));
        assert_eq!(decode_page_cursor(&cursor).unwrap(), (1_760_000_000_123, "job-1".to_string()));
        let wrong_version = BASE64URL.encode(br#"{"v":2,"time":1,"id":"x"}"#);
        assert_eq!(decode_page_cursor(&wrong_version).unwrap_err().code, "invalid_cursor");
        assert_eq!(decode_page_cursor("%%%").unwrap_err().code, "invalid_cursor");
    }
}
```

Add to the `#[cfg(test)] mod tests` of `src/remote/tls.rs`:

```rust
    #[tokio::test]
    async fn a_lazy_channel_does_not_dial_until_used() {
        // Nothing listens on the discard port; an eager connect would fail here.
        assert!(lazy_channel("http://127.0.0.1:9".to_string()).is_ok());
        assert!(lazy_channel("not a uri".to_string()).is_err());
    }
```

Add to the `#[cfg(test)] mod tests` of `src/mongo/mod.rs`:

```rust
    #[tokio::test]
    async fn a_read_only_database_starts_without_a_server() {
        let db = MongoStore::read_only_database(
            "mongodb://127.0.0.1:9/?serverSelectionTimeoutMS=100",
            "agent_ctrl_api_test",
            &MongoTlsSettings::default(),
        )
        .await
        .expect("no connection is attempted");
        assert_eq!(db.name(), "agent_ctrl_api_test");
    }
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test --lib api::` then `cargo test --lib lazy_channel` then `cargo test --lib read_only_database`
Expected: compile errors (`ApiError`, `encode_event_cursor`, `lazy_channel`, `read_only_database` not found). This first `cargo` run (without `--locked`) also updates `Cargo.lock` with axum's optional features, jsonwebtoken and their dependencies.

- [ ] **Step 4: Implement `error.rs`**

Above the test module in `src/api/error.rs`:

```rust
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;
use tonic::Code;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiError {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: String,
}

impl ApiError {
    pub fn new(status: u16, message: impl Into<String>, code: &'static str) -> Self {
        Self {
            status: StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            code,
            message: message.into(),
        }
    }

    pub fn not_found() -> Self {
        Self::new(404, "Not found", "not_found")
    }

    pub fn run_not_found() -> Self {
        Self::new(404, "Run not found", "not_found")
    }

    pub fn unauthenticated() -> Self {
        Self::new(401, "Unauthenticated", "unauthenticated")
    }

    pub fn storage() -> Self {
        Self::new(503, "Fleet read model is unavailable", "storage_unavailable")
    }

    /// Maps a FleetControl status like the dashboard's `mapGrpcError`. `state_refusals` is set for
    /// pause and resume, whose precondition failures are state conflicts rather than id conflicts.
    pub fn from_status(status: &tonic::Status, state_refusals: bool) -> Self {
        let details = status.message();
        let lower = details.to_ascii_lowercase();
        let or = |fallback: &str| {
            if details.is_empty() {
                fallback.to_string()
            } else {
                details.to_string()
            }
        };
        match status.code() {
            Code::Unauthenticated => {
                Self::new(503, "Controller authentication failed", "controller_unavailable")
            }
            Code::PermissionDenied => Self::new(403, or("Forbidden"), "forbidden"),
            Code::NotFound => Self::run_not_found(),
            Code::FailedPrecondition if lower.contains("session id unknown") => {
                Self::new(412, "Session id unknown", "session_unknown")
            }
            Code::FailedPrecondition
                if state_refusals && lower.contains("does not permit operator interaction") =>
            {
                Self::new(409, or("Run does not permit operator interaction"), "not_interactive")
            }
            Code::FailedPrecondition if state_refusals => {
                Self::new(409, or("Run is not in a state that allows this"), "invalid_state")
            }
            Code::AlreadyExists | Code::Aborted | Code::FailedPrecondition => {
                Self::new(409, or("Mutation conflict"), "conflict")
            }
            Code::ResourceExhausted => Self::new(409, or("node at capacity"), "node_at_capacity"),
            Code::InvalidArgument => Self::new(422, or("Validation failed"), "validation_error"),
            _ => Self::new(503, "Controller is unavailable", "controller_unavailable"),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let message = if self.status.as_u16() < 500 {
            self.message
        } else if self.status == StatusCode::SERVICE_UNAVAILABLE {
            "Service unavailable".to_string()
        } else {
            "Internal server error".to_string()
        };
        let mut response =
            (self.status, Json(json!({ "error": message, "code": self.code }))).into_response();
        let headers = response.headers_mut();
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        if self.status == StatusCode::TOO_MANY_REQUESTS {
            headers.insert(header::RETRY_AFTER, HeaderValue::from_static("60"));
        }
        response
    }
}
```

- [ ] **Step 5: Implement `cursor.rs`**

Above the test module in `src/api/cursor.rs`:

```rust
use super::error::ApiError;
use base64::alphabet::URL_SAFE;
use base64::engine::general_purpose::{GeneralPurpose, GeneralPurposeConfig};
use base64::engine::DecodePaddingMode;
use base64::Engine as _;

const MAX_SAFE_INTEGER: i64 = 9_007_199_254_740_991;

/// Unpadded on encode and padding-indifferent on decode, like Node's `base64url`.
pub const BASE64URL: GeneralPurpose = GeneralPurpose::new(
    &URL_SAFE,
    GeneralPurposeConfig::new()
        .with_encode_padding(false)
        .with_decode_padding_mode(DecodePaddingMode::Indifferent)
        .with_decode_allow_trailing_bits(true),
);

/// The event cursor after `sequence`; `None` before the first event.
pub fn encode_event_cursor(sequence: u64) -> Option<String> {
    (sequence > 0).then(|| BASE64URL.encode(format!("v1:{sequence}")))
}

/// The sequence an event cursor points after; 0 when absent.
pub fn decode_event_cursor(value: Option<&str>) -> Result<u64, ApiError> {
    let Some(value) = value.filter(|value| !value.is_empty()) else {
        return Ok(0);
    };
    let invalid = || ApiError::new(400, "Invalid event cursor", "invalid_cursor");
    let bytes = BASE64URL.decode(value).map_err(|_| invalid())?;
    let text = String::from_utf8(bytes).map_err(|_| invalid())?;
    let digits = text
        .strip_prefix("v1:")
        .filter(|digits| !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit()))
        .ok_or_else(invalid)?;
    digits.parse().map_err(|_| invalid())
}

/// A list page cursor: `{"v":1,"time":<created_at ms>,"id":<id>}` in base64url.
pub fn encode_page_cursor(time_ms: i64, id: &str) -> String {
    let id = serde_json::to_string(id).expect("a string always serializes");
    BASE64URL.encode(format!("{{\"v\":1,\"time\":{time_ms},\"id\":{id}}}"))
}

pub fn decode_page_cursor(value: &str) -> Result<(i64, String), ApiError> {
    let invalid = || ApiError::new(400, "Invalid page cursor", "invalid_cursor");
    let bytes = BASE64URL.decode(value).map_err(|_| invalid())?;
    let parsed: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
    if parsed.get("v").and_then(serde_json::Value::as_i64) != Some(1) {
        return Err(invalid());
    }
    let time = parsed
        .get("time")
        .and_then(serde_json::Value::as_i64)
        .filter(|time| time.abs() <= MAX_SAFE_INTEGER)
        .ok_or_else(invalid)?;
    let id = parsed.get("id").and_then(serde_json::Value::as_str).ok_or_else(invalid)?;
    Ok((time, id.to_string()))
}
```

The test module uses `BASE64URL.decode`/`.encode`, which need `base64::Engine` in scope; add `use base64::Engine as _;` inside `mod tests` as well.

- [ ] **Step 6: Implement `lazy_channel` and `read_only_database`**

In `src/remote/tls.rs`, replace `connect_channel` (line 131-146) with:

```rust
fn configured_endpoint(addr: String) -> Result<Endpoint> {
    let mut endpoint = Endpoint::from_shared(addr).map_err(RemoteError::Transport)?;
    if let Some(tls) = client_tls_from_env()? {
        endpoint = endpoint.tls_config(tls).map_err(RemoteError::Transport)?;
    }
    Ok(endpoint
        .http2_keep_alive_interval(KEEPALIVE_INTERVAL)
        .keep_alive_timeout(KEEPALIVE_TIMEOUT)
        // Without this, pings are only sent while a request is in flight ---
        // which is exactly never for an idle registered node, so the dead
        // stream would still go unnoticed.
        .keep_alive_while_idle(true)
        .tcp_keepalive(Some(KEEPALIVE_INTERVAL)))
}

pub async fn connect_channel(addr: String) -> Result<Channel> {
    Ok(configured_endpoint(addr)?
        .connect()
        .await
        .map_err(RemoteError::Transport)?)
}

/// [`connect_channel`] without dialing: the channel connects on first use and again after a
/// failure, so a client can start while the controller is down.
pub fn lazy_channel(addr: String) -> Result<Channel> {
    Ok(configured_endpoint(addr)?.connect_lazy())
}
```

In `src/mongo/mod.rs`, split `MongoStore::connect` (line 432-456):

```rust
    async fn client_options(uri: &str, tls: &MongoTlsSettings) -> Result<ClientOptions> {
        let mut opts = ClientOptions::parse(uri).await?;
        opts.server_selection_timeout = Some(CONNECT_TIMEOUT);
        opts.connect_timeout = Some(CONNECT_TIMEOUT);
        opts.app_name = Some("agent-ctrl".to_string());
        if tls.is_configured() {
            let mut tls_opts = TlsOptions::default();
            tls_opts.ca_file_path = tls.ca_file.clone();
            tls_opts.cert_key_file_path = tls.cert_key_file.clone();
            if tls.insecure {
                tls_opts.allow_invalid_certificates = Some(true);
            }
            opts.tls = Some(Tls::Enabled(tls_opts));
        }
        Ok(opts)
    }

    /// Connect and verify reachability with a short timeout.
    pub async fn connect(uri: &str, database: &str, tls: &MongoTlsSettings) -> Result<Self> {
        let client = Client::with_options(Self::client_options(uri, tls).await?)?;
        let db = client.database(database);
        // Ping to confirm the server is actually reachable.
        db.run_command(doc! { "ping": 1 }).await?;
        let store = Self { db };
        store.ensure_indexes().await?;
        Ok(store)
    }

    /// A handle on `database` that connects on first use, without the reachability ping or index
    /// setup, for read-only clients that must start while MongoDB is unreachable.
    pub async fn read_only_database(
        uri: &str,
        database: &str,
        tls: &MongoTlsSettings,
    ) -> Result<Database> {
        let client = Client::with_options(Self::client_options(uri, tls).await?)?;
        Ok(client.database(database))
    }
```

- [ ] **Step 7: Run the tests to verify they pass**

Run: `cargo test --lib api:: && cargo test --lib lazy_channel && cargo test --lib read_only_database && cargo test --locked`
Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add Cargo.toml Cargo.lock src/lib.rs src/api/mod.rs src/api/error.rs src/api/cursor.rs src/remote/tls.rs src/mongo/mod.rs
git commit -m "feat(api): scaffold the public run API with errors, cursors and lazy clients

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: Zitadel bearer verification

Port of stoz3n-agent-dash `server/zitadel-token.ts` (origin/dev 9c33330).

**Files:**
- Create: `src/api/auth.rs`, `src/api/fakes.rs`, `tests/fixtures/api/test-rsa-key.pem`
- Modify: `src/api/mod.rs` (module list)

**Interfaces:**
- Consumes: `ApiError::unauthenticated()` (Task 4).
- Produces: `ZitadelSettings { issuer, project_id }`; `Jwk { kid: Option<String>, kty: String, n: Option<String>, e: Option<String> }`; traits `JwksSource { async fn fetch(&self, issuer: &str) -> Option<Vec<Jwk>> }` and `ProfileSource { async fn userinfo(&self, issuer: &str, token: &str) -> Option<Profile> }`; `HttpZitadel` (implements both); `Profile { email, name, preferred_username: Option<String> }`; `Role { Admin, Member }`; `Caller { sub: String, role: Role, email: Option<String>, name: Option<String>, actor: String }` with `is_admin()`; `Verifier::new(ZitadelSettings, Arc<dyn JwksSource>, Arc<dyn ProfileSource>, actor_domain: String)`, `async fn verify(&self, token) -> Option<VerifiedToken>`, `async fn authenticate(&self, &HeaderMap) -> Result<Caller, ApiError>`. Test-only `crate::api::fakes::{ISSUER, PROJECT, WARPSPEED_CLIENT, KID, TEST_MODULUS, test_jwk, encoding_key, claims, sign, token, FakeJwks, FakeProfiles, verifier}`.

- [ ] **Step 1: Commit the test-only signing key**

Create `tests/fixtures/api/test-rsa-key.pem` (a 2048-bit key generated for this plan; it signs test tokens only and is trusted nowhere):

```
-----BEGIN RSA PRIVATE KEY-----
MIIEowIBAAKCAQEA1ly20klcvJLwesQR+AJf545WWN/6BYadsPMMi3wu+u98zy54
WCk+3pyoW+yxTGpzBGfgO7BQKHNj20lD72y8LWUnsfJsF5awqf2nR0Wo+dXiCyGg
3RYKyNndK4Seqzpl/50o3zlUijxGCVS/UNW6Qckupdl6lsFvlSWbphITsOqiOvda
CyNnv+3rHwFindF6rfsX8nBCur/MLkuachvzROUZ2qrpYGzxRAttqBRTVLqcG4aY
vTY0aHv3tKSJU95yjyJvQNijzDEwm9aTZb32ot8OkOIJmsC4H+pxk1Npx+aMdqQx
5SWtb21aasGREzM/s9K8DZbvcv1KlOeCaDFQIQIDAQABAoIBAFrF2AUkW8je0Eas
UI+dMc90TsxDzDQa1RvhUCag56QLVebF3uA2VxEddY23pTyetyXedu3Ps6HtexOY
6tOwT99JPo30AiHbBeR2LSfZL7zb/WSJUP1OIDL9TtLmzenkSNz9SBl+xXXkICRD
ZuBsIPtKnh5NAcRlFUMrMcYZUY1QiAwsFwHdvsQsviOZfbZVAP7fJIkRafl9XirL
hlfGEbByCoeXwta95WFa3HSutoUUPquKxC+UZKHOMkLs7g2pQUuP4y6WLLQxQGn8
h002nivADXi7Zqr2iBfROyBhbnlCLi1IQQ3PkQ//5UZB4H0Oawx4945iNi1RbFAf
byfSOCECgYEA6p7X9OXa0lWqc91n98LzA9MfU/qBZaGsP/MKwaaImGr3+G17JmGD
5aanB51CoC+Lmh+NBO+bwAhWVaa+Uu8dFPEmSkf/pcieNdGEVnL+tB0/CAtgCJ69
fImpjfncsoA5bFL1NnQo/aTt09oOwbU60LwYY3U+QvevwX578MVNEQMCgYEA6eVK
Y51hX5mI4xHbO1TnQJQo/73fx/ci06o6RLqisOljnsqqY5c4MOOSBmf+r6RPOOQN
NbDkI2/8tlaFwF59L/dqVYtSNaJ6EVeYjeMMMsgO7ilT9dsy823f5LEJXDC4M0H0
XcGRLPQvL7lLW+KqpqhJLcyT9syOdhuNJyejhwsCgYBv6sGdQp/1QYQDrRPRLh96
LZV+G7rp3gsqnib9ZRE3k58h1CewkUe3np2CllOX0dZH+NDuD9Y97jwws0+KhCFd
QgM/38RlMqO1jHISQtyY0nKkxbhTpFVDOO+2l//c7il2MMRj61k6VRhVvGjZojei
JneDygOfMkOWi11X5uXyPQKBgQCN+KOiu5CzWpqAWgJcjTihNk18jmAbHKODHzyn
IzJFeCUD6bhrIwr0ge3M/arWKkH8IlDyWiDgccVPWE1vautKBkABFMQd0jWN+UGc
PkjVOiH15hgUj6zpnbPdOKiZ0Oi+FDakAi6Avuc1bJ+VcoxrBJMGvbrCf3+pqSeQ
sLrN6QKBgBTTZ8iHxC00B+uAu92tsFbyVx/V9wg2/ApYxIUO2SyX3Q00vUxbO9ui
PDr3Dr4Z/CwZRjxwO3ZG/PkKYFEx3+eWU7GpUXDMRKQUxIQhGJx7eQrOnR4ifzUA
HBExFRNP3hlWDvqV+5fSXk6QGPtMawsbggit+ED/v6LzTpGZGhLO
-----END RSA PRIVATE KEY-----
```

Its JWK modulus (base64url) is

```
1ly20klcvJLwesQR-AJf545WWN_6BYadsPMMi3wu-u98zy54WCk-3pyoW-yxTGpzBGfgO7BQKHNj20lD72y8LWUnsfJsF5awqf2nR0Wo-dXiCyGg3RYKyNndK4Seqzpl_50o3zlUijxGCVS_UNW6Qckupdl6lsFvlSWbphITsOqiOvdaCyNnv-3rHwFindF6rfsX8nBCur_MLkuachvzROUZ2qrpYGzxRAttqBRTVLqcG4aYvTY0aHv3tKSJU95yjyJvQNijzDEwm9aTZb32ot8OkOIJmsC4H-pxk1Npx-aMdqQx5SWtb21aasGREzM_s9K8DZbvcv1KlOeCaDFQIQ
```

and its exponent `AQAB`. Check before relying on it: `openssl rsa -in tests/fixtures/api/test-rsa-key.pem -noout -modulus` prints `Modulus=D65CB6D2…` (the same number in hex).

- [ ] **Step 2: Fakes**

Add to `src/api/mod.rs` after `pub mod error;`:

```rust
pub mod auth;
#[cfg(test)]
pub(crate) mod fakes;
```

Create `src/api/fakes.rs`:

```rust
//! Test doubles for the API: Zitadel (JWKS, userinfo) and tokens signed with the committed
//! test-only key. Later tasks add the read model and FleetControl fakes.

use super::auth::{Jwk, JwksSource, Profile, ProfileSource, Verifier, ZitadelSettings};
use async_trait::async_trait;
use base64::Engine as _;
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use serde_json::{json, Map, Value};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

pub const ISSUER: &str = "https://id-dev.obachan.dev";
pub const PROJECT: &str = "394361582064240430";
pub const WARPSPEED_CLIENT: &str = "394361583406417710";
pub const KID: &str = "test-key-1";
/// Base64url RSA modulus of `tests/fixtures/api/test-rsa-key.pem`; the exponent is 65537.
pub const TEST_MODULUS: &str = "1ly20klcvJLwesQR-AJf545WWN_6BYadsPMMi3wu-u98zy54WCk-3pyoW-yxTGpzBGfgO7BQKHNj20lD72y8LWUnsfJsF5awqf2nR0Wo-dXiCyGg3RYKyNndK4Seqzpl_50o3zlUijxGCVS_UNW6Qckupdl6lsFvlSWbphITsOqiOvdaCyNnv-3rHwFindF6rfsX8nBCur_MLkuachvzROUZ2qrpYGzxRAttqBRTVLqcG4aYvTY0aHv3tKSJU95yjyJvQNijzDEwm9aTZb32ot8OkOIJmsC4H-pxk1Npx-aMdqQx5SWtb21aasGREzM_s9K8DZbvcv1KlOeCaDFQIQ";
const TEST_KEY_PEM: &[u8] = include_bytes!("../../tests/fixtures/api/test-rsa-key.pem");

pub fn test_jwk() -> Jwk {
    Jwk {
        kid: Some(KID.to_string()),
        kty: "RSA".to_string(),
        n: Some(TEST_MODULUS.to_string()),
        e: Some("AQAB".to_string()),
    }
}

pub fn encoding_key() -> EncodingKey {
    EncodingKey::from_rsa_pem(TEST_KEY_PEM).expect("valid test key")
}

/// Zitadel-shaped access-token claims for `sub` holding `roles` in the agent-platform project.
pub fn claims(sub: &str, roles: &[&str]) -> Map<String, Value> {
    let now = chrono::Utc::now().timestamp();
    let mut granted = Map::new();
    for role in roles {
        granted.insert((*role).to_string(), json!({ "394361582064240429": "pseekoo" }));
    }
    let mut claims = Map::new();
    claims.insert("iss".into(), json!(ISSUER));
    claims.insert("sub".into(), json!(sub));
    claims.insert("aud".into(), json!([WARPSPEED_CLIENT, PROJECT]));
    claims.insert("iat".into(), json!(now));
    claims.insert("nbf".into(), json!(now));
    claims.insert("exp".into(), json!(now + 3600));
    claims.insert(
        format!("urn:zitadel:iam:org:project:{PROJECT}:roles"),
        Value::Object(granted),
    );
    claims
}

pub fn sign(claims: &Map<String, Value>) -> String {
    let header = Header {
        kid: Some(KID.to_string()),
        ..Header::new(Algorithm::RS256)
    };
    encode(&header, claims, &encoding_key()).expect("signed test token")
}

pub fn token(sub: &str, roles: &[&str]) -> String {
    sign(&claims(sub, roles))
}

#[derive(Default)]
pub struct FakeJwks {
    keys: Mutex<Option<Vec<Jwk>>>,
    pub fetches: AtomicUsize,
}

impl FakeJwks {
    pub fn serving(keys: Vec<Jwk>) -> Arc<Self> {
        let fake = Arc::new(Self::default());
        fake.serve(keys);
        fake
    }

    pub fn serve(&self, keys: Vec<Jwk>) {
        *self.keys.lock().unwrap() = Some(keys);
    }
}

#[async_trait]
impl JwksSource for FakeJwks {
    async fn fetch(&self, issuer: &str) -> Option<Vec<Jwk>> {
        assert_eq!(issuer, ISSUER);
        self.fetches.fetch_add(1, Ordering::SeqCst);
        self.keys.lock().unwrap().clone()
    }
}

/// Answers userinfo as email `<sub>@example.test`, name `User <sub>`, preferred username `<sub>`.
#[derive(Default)]
pub struct FakeProfiles {
    pub calls: AtomicUsize,
}

#[async_trait]
impl ProfileSource for FakeProfiles {
    async fn userinfo(&self, _issuer: &str, token: &str) -> Option<Profile> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let payload = token.split('.').nth(1)?;
        let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload).ok()?;
        let claims: Value = serde_json::from_slice(&bytes).ok()?;
        let sub = claims.get("sub")?.as_str()?;
        Some(Profile {
            email: Some(format!("{sub}@example.test")),
            name: Some(format!("User {sub}")),
            preferred_username: Some(sub.to_string()),
        })
    }
}

pub fn verifier(jwks: Arc<FakeJwks>, profiles: Arc<FakeProfiles>) -> Verifier {
    Verifier::new(
        ZitadelSettings {
            issuer: ISSUER.into(),
            project_id: PROJECT.into(),
        },
        jwks,
        profiles,
        "example.test".into(),
    )
}
```

- [ ] **Step 3: Write the failing tests**

Create `src/api/auth.rs` with only the test module:

```rust
//! Zitadel bearer verification, ported from the dashboard's `zitadel-token.ts`: RS256 with a
//! string `kid` only, JWKS cached for 10 minutes with forced refetches spaced by 60 s, exact
//! issuer, project in `aud`, `exp`/`nbf` with 60 s leeway, roles only from the project-scoped
//! claim. The bearer is authoritative; APISIX's `X-Userinfo` only supplies display details.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::fakes::{self, FakeJwks, FakeProfiles, PROJECT};
    use axum::http::HeaderValue;
    use jsonwebtoken::{encode, EncodingKey, Header};
    use serde_json::json;
    use std::sync::atomic::Ordering;

    fn headers(token: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            "authorization",
            HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
        );
        headers
    }

    fn verifier() -> (Verifier, Arc<FakeJwks>, Arc<FakeProfiles>) {
        let jwks = FakeJwks::serving(vec![fakes::test_jwk()]);
        let profiles = Arc::new(FakeProfiles::default());
        (fakes::verifier(jwks.clone(), profiles.clone()), jwks, profiles)
    }

    async fn rejected(verifier: &Verifier, token: &str) -> bool {
        verifier.authenticate(&headers(token)).await == Err(ApiError::unauthenticated())
    }

    fn rs256(kid: Option<&str>, claims: &serde_json::Map<String, serde_json::Value>) -> String {
        let header = Header {
            kid: kid.map(str::to_string),
            ..Header::new(Algorithm::RS256)
        };
        encode(&header, claims, &fakes::encoding_key()).unwrap()
    }

    #[tokio::test]
    async fn an_agent_admin_token_authenticates_as_admin_with_its_profile() {
        let (verifier, _, _) = verifier();
        let token = fakes::token("394000000000000001", &["agent-admin", "agent-user"]);
        let caller = verifier.authenticate(&headers(&token)).await.unwrap();
        assert_eq!(caller.role, Role::Admin);
        assert_eq!(caller.sub, "394000000000000001");
        assert_eq!(caller.email.as_deref(), Some("394000000000000001@example.test"));
        assert_eq!(caller.name.as_deref(), Some("User 394000000000000001"));
        assert_eq!(caller.actor, "@394000000000000001:example.test");
    }

    #[tokio::test]
    async fn an_agent_user_token_is_a_member() {
        let (verifier, _, _) = verifier();
        let caller = verifier
            .authenticate(&headers(&fakes::token("s-1", &["agent-user"])))
            .await
            .unwrap();
        assert_eq!(caller.role, Role::Member);
        assert!(!caller.is_admin());
    }

    #[tokio::test]
    async fn tokens_without_an_agent_role_are_rejected() {
        let (verifier, _, _) = verifier();
        assert!(rejected(&verifier, &fakes::token("s", &[])).await);
        assert!(rejected(&verifier, &fakes::token("s", &["viewer"])).await);
        let mut generic = fakes::claims("s", &[]);
        generic.insert(
            "urn:zitadel:iam:org:project:roles".into(),
            json!({ "agent-admin": { "1": "pseekoo" } }),
        );
        assert!(
            rejected(&verifier, &fakes::sign(&generic)).await,
            "only the project-scoped roles claim counts"
        );
    }

    #[tokio::test]
    async fn issuer_audience_and_lifetime_are_enforced_with_leeway() {
        let (verifier, _, _) = verifier();
        let now = chrono::Utc::now().timestamp();
        let with = |key: &str, value: serde_json::Value| {
            let mut claims = fakes::claims("s", &["agent-admin"]);
            claims.insert(key.into(), value);
            fakes::sign(&claims)
        };
        assert!(rejected(&verifier, &with("iss", json!("https://id.obachan.dev"))).await);
        assert!(rejected(&verifier, &with("aud", json!(["394361583406417710"]))).await);
        assert!(!rejected(&verifier, &with("aud", json!(PROJECT))).await, "a string aud");
        assert!(rejected(&verifier, &with("exp", json!(now - 120))).await);
        assert!(!rejected(&verifier, &with("exp", json!(now - 30))).await, "60 s leeway");
        assert!(rejected(&verifier, &with("nbf", json!(now + 120))).await);
        assert!(rejected(&verifier, &with("sub", json!(""))).await);
    }

    #[tokio::test]
    async fn only_rs256_with_a_known_string_kid_is_accepted() {
        let (verifier, _, _) = verifier();
        let claims = fakes::claims("s", &["agent-admin"]);
        let hs256 = encode(
            &Header::new(Algorithm::HS256),
            &claims,
            &EncodingKey::from_secret(b"secret"),
        )
        .unwrap();
        assert!(rejected(&verifier, &hs256).await);
        assert!(rejected(&verifier, &rs256(None, &claims)).await);
        assert!(rejected(&verifier, &rs256(Some("other"), &claims)).await);
        assert!(rejected(&verifier, "not.a.jwt").await);
        assert!(rejected(&verifier, "garbage").await);
    }

    #[tokio::test(start_paused = true)]
    async fn unknown_kids_refetch_the_jwks_at_most_once_a_minute() {
        let (verifier, jwks, _) = verifier();
        let rotated = rs256(Some("rotated"), &fakes::claims("s", &["agent-admin"]));
        assert!(rejected(&verifier, &rotated).await);
        assert!(rejected(&verifier, &rotated).await);
        assert_eq!(jwks.fetches.load(Ordering::SeqCst), 1, "the forced refetch is throttled");
        tokio::time::advance(Duration::from_secs(61)).await;
        jwks.serve(vec![
            fakes::test_jwk(),
            Jwk {
                kid: Some("rotated".into()),
                ..fakes::test_jwk()
            },
        ]);
        assert!(!rejected(&verifier, &rotated).await, "a rotated key is picked up");
        assert_eq!(jwks.fetches.load(Ordering::SeqCst), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn the_jwks_is_cached_for_ten_minutes() {
        let (verifier, jwks, _) = verifier();
        let token = fakes::token("s", &["agent-admin"]);
        assert!(!rejected(&verifier, &token).await);
        assert!(!rejected(&verifier, &token).await);
        assert_eq!(jwks.fetches.load(Ordering::SeqCst), 1);
        tokio::time::advance(Duration::from_secs(601)).await;
        assert!(!rejected(&verifier, &token).await);
        assert_eq!(jwks.fetches.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn an_unreachable_jwks_fails_closed() {
        let verifier =
            fakes::verifier(Arc::new(FakeJwks::default()), Arc::new(FakeProfiles::default()));
        assert!(rejected(&verifier, &fakes::token("s", &["agent-admin"])).await);
    }

    #[tokio::test]
    async fn a_missing_or_non_bearer_authorization_is_rejected() {
        let (verifier, _, _) = verifier();
        assert_eq!(
            verifier.authenticate(&HeaderMap::new()).await,
            Err(ApiError::unauthenticated())
        );
        let mut basic = HeaderMap::new();
        basic.insert("authorization", HeaderValue::from_static("Basic dXNlcjpwYXNz"));
        assert_eq!(verifier.authenticate(&basic).await, Err(ApiError::unauthenticated()));
    }

    #[tokio::test]
    async fn the_apisix_userinfo_header_supplies_the_profile_only_for_the_token_subject() {
        let (verifier, _, profiles) = verifier();
        let token = fakes::token("394000000000000001", &["agent-user"]);
        let encode_info = |value: serde_json::Value| {
            base64::engine::general_purpose::STANDARD.encode(value.to_string())
        };
        let mut request = headers(&token);
        request.insert(
            "x-userinfo",
            HeaderValue::from_str(&encode_info(json!({
                "sub": "394000000000000001",
                "email": "ada@example.test",
                "name": "Ada Owner",
                "preferred_username": "ada"
            })))
            .unwrap(),
        );
        let caller = verifier.authenticate(&request).await.unwrap();
        assert_eq!(caller.email.as_deref(), Some("ada@example.test"));
        assert_eq!(caller.name.as_deref(), Some("Ada Owner"));
        assert_eq!(caller.actor, "@ada:example.test");
        assert_eq!(profiles.calls.load(Ordering::SeqCst), 0);

        request.insert(
            "x-userinfo",
            HeaderValue::from_str(&encode_info(json!({
                "sub": "someone-else",
                "email": "eve@example.test"
            })))
            .unwrap(),
        );
        let caller = verifier.authenticate(&request).await.unwrap();
        assert_eq!(
            caller.email.as_deref(),
            Some("394000000000000001@example.test"),
            "a header about another subject is ignored"
        );
    }

    #[tokio::test]
    async fn userinfo_lookups_are_cached_per_token() {
        let (verifier, _, profiles) = verifier();
        let token = fakes::token("s", &["agent-user"]);
        verifier.authenticate(&headers(&token)).await.unwrap();
        verifier.authenticate(&headers(&token)).await.unwrap();
        assert_eq!(profiles.calls.load(Ordering::SeqCst), 1);
    }
}
```

- [ ] **Step 4: Run the tests to verify they fail**

Run: `cargo test --lib api::auth`
Expected: compile errors (`Verifier`, `Role`, `Jwk` not found).

- [ ] **Step 5: Implement `auth.rs`**

Above the test module:

```rust
use super::error::ApiError;
use async_trait::async_trait;
use axum::http::{header, HeaderMap};
use base64::Engine as _;
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use serde::Deserialize;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::time::Instant;

const LEEWAY_SECONDS: u64 = 60;
const JWKS_TTL: Duration = Duration::from_secs(10 * 60);
/// A failed or forced JWKS fetch is not retried within this window, so verification fails
/// closed for up to 60 s and unauthenticated callers cannot hammer the issuer.
const JWKS_MIN_REFETCH: Duration = Duration::from_secs(60);
const USERINFO_TTL: Duration = Duration::from_secs(5 * 60);
const USERINFO_CACHE_LIMIT: usize = 1000;
const MAX_BEARER_BYTES: usize = 8192;

#[derive(Debug, Clone)]
pub struct ZitadelSettings {
    /// Exact `iss`, without a trailing slash, e.g. `https://id-dev.obachan.dev`.
    pub issuer: String,
    /// Must appear in `aud`; also scopes the roles claim.
    pub project_id: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Jwk {
    #[serde(default)]
    pub kid: Option<String>,
    #[serde(default)]
    pub kty: String,
    #[serde(default)]
    pub n: Option<String>,
    #[serde(default)]
    pub e: Option<String>,
}

#[async_trait]
pub trait JwksSource: Send + Sync {
    /// The issuer's current signing keys; `None` when they could not be fetched.
    async fn fetch(&self, issuer: &str) -> Option<Vec<Jwk>>;
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Profile {
    pub email: Option<String>,
    pub name: Option<String>,
    pub preferred_username: Option<String>,
}

#[async_trait]
pub trait ProfileSource: Send + Sync {
    /// The token holder's OIDC userinfo; `None` when unavailable.
    async fn userinfo(&self, issuer: &str, token: &str) -> Option<Profile>;
}

/// Zitadel over HTTPS: `<issuer>/oauth/v2/keys` and `<issuer>/oidc/v1/userinfo`.
pub struct HttpZitadel {
    client: reqwest::Client,
}

impl HttpZitadel {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(5))
                .build()
                .expect("a default reqwest client builds"),
        }
    }
}

impl Default for HttpZitadel {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Deserialize)]
struct JwksBody {
    keys: Vec<Jwk>,
}

#[async_trait]
impl JwksSource for HttpZitadel {
    async fn fetch(&self, issuer: &str) -> Option<Vec<Jwk>> {
        let response = self
            .client
            .get(format!("{issuer}/oauth/v2/keys"))
            .send()
            .await
            .ok()?;
        if !response.status().is_success() {
            return None;
        }
        response.json::<JwksBody>().await.ok().map(|body| body.keys)
    }
}

#[async_trait]
impl ProfileSource for HttpZitadel {
    async fn userinfo(&self, issuer: &str, token: &str) -> Option<Profile> {
        let response = self
            .client
            .get(format!("{issuer}/oidc/v1/userinfo"))
            .bearer_auth(token)
            .send()
            .await
            .ok()?;
        if !response.status().is_success() {
            return None;
        }
        let value: Value = response.json().await.ok()?;
        Some(profile_from(&value))
    }
}

fn profile_from(value: &Value) -> Profile {
    let field = |key: &str| {
        value
            .get(key)
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
            .map(str::to_string)
    };
    Profile {
        email: field("email"),
        name: field("name"),
        preferred_username: field("preferred_username"),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Admin,
    Member,
}

/// The authenticated person behind a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Caller {
    pub sub: String,
    pub role: Role,
    pub email: Option<String>,
    pub name: Option<String>,
    /// Audit actor `@<preferred_username or sub>:<actor domain>`, at most 128 characters.
    pub actor: String,
}

impl Caller {
    pub fn is_admin(&self) -> bool {
        self.role == Role::Admin
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedToken {
    pub sub: String,
    pub roles: Vec<String>,
}

/// Admin wins when both agent-platform roles are granted.
pub fn agent_platform_role(roles: &[String]) -> Option<Role> {
    if roles.iter().any(|role| role == "agent-admin") {
        Some(Role::Admin)
    } else if roles.iter().any(|role| role == "agent-user") {
        Some(Role::Member)
    } else {
        None
    }
}

#[derive(Default)]
struct KeyCache {
    keys: Vec<Jwk>,
    fetched_at: Option<Instant>,
    last_attempt: Option<Instant>,
}

pub struct Verifier {
    settings: ZitadelSettings,
    jwks: Arc<dyn JwksSource>,
    profiles: Arc<dyn ProfileSource>,
    actor_domain: String,
    keys: Mutex<KeyCache>,
    userinfo: Mutex<HashMap<[u8; 32], (Instant, Profile)>>,
}

impl Verifier {
    pub fn new(
        settings: ZitadelSettings,
        jwks: Arc<dyn JwksSource>,
        profiles: Arc<dyn ProfileSource>,
        actor_domain: String,
    ) -> Self {
        Self {
            settings,
            jwks,
            profiles,
            actor_domain,
            keys: Mutex::new(KeyCache::default()),
            userinfo: Mutex::new(HashMap::new()),
        }
    }

    async fn signing_keys(&self, force: bool) -> Vec<Jwk> {
        let mut cache = self.keys.lock().await;
        let now = Instant::now();
        let fresh = cache
            .fetched_at
            .is_some_and(|at| now.duration_since(at) < JWKS_TTL);
        if !force && fresh {
            return cache.keys.clone();
        }
        if cache
            .last_attempt
            .is_some_and(|at| now.duration_since(at) < JWKS_MIN_REFETCH)
        {
            return cache.keys.clone();
        }
        cache.last_attempt = Some(now);
        if let Some(keys) = self.jwks.fetch(&self.settings.issuer).await {
            cache.keys = keys;
            cache.fetched_at = Some(Instant::now());
        }
        cache.keys.clone()
    }

    /// The token's subject and project roles when it is a valid agent-platform access token.
    pub async fn verify(&self, token: &str) -> Option<VerifiedToken> {
        if token.split('.').count() != 3 {
            return None;
        }
        let header = decode_header(token).ok()?;
        if header.alg != Algorithm::RS256 {
            return None;
        }
        let kid = header.kid?;
        let has_kid =
            |keys: &[Jwk]| keys.iter().any(|key| key.kid.as_deref() == Some(kid.as_str()));
        let mut keys = self.signing_keys(false).await;
        if !has_kid(&keys) {
            keys = self.signing_keys(true).await;
        }
        let jwk = keys
            .into_iter()
            .find(|key| key.kid.as_deref() == Some(kid.as_str()))?;
        if jwk.kty != "RSA" {
            return None;
        }
        let key = DecodingKey::from_rsa_components(jwk.n.as_deref()?, jwk.e.as_deref()?).ok()?;
        let mut validation = Validation::new(Algorithm::RS256);
        validation.leeway = LEEWAY_SECONDS;
        validation.validate_nbf = true;
        validation.set_issuer(&[self.settings.issuer.as_str()]);
        validation.set_audience(&[self.settings.project_id.as_str()]);
        validation.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);
        let data = decode::<Map<String, Value>>(token, &key, &validation).ok()?;
        let sub = data
            .claims
            .get("sub")
            .and_then(Value::as_str)
            .filter(|sub| !sub.is_empty())?
            .to_string();
        let roles_claim = format!(
            "urn:zitadel:iam:org:project:{}:roles",
            self.settings.project_id
        );
        let roles = match data.claims.get(&roles_claim) {
            Some(Value::Object(granted)) => granted.keys().cloned().collect(),
            _ => Vec::new(),
        };
        Some(VerifiedToken { sub, roles })
    }

    /// The caller behind `Authorization: Bearer <token>`; 401 unless it carries an agent role.
    pub async fn authenticate(&self, headers: &HeaderMap) -> Result<Caller, ApiError> {
        let token = bearer(headers).ok_or_else(ApiError::unauthenticated)?;
        let verified = self.verify(token).await.ok_or_else(ApiError::unauthenticated)?;
        let role = agent_platform_role(&verified.roles).ok_or_else(ApiError::unauthenticated)?;
        let profile = match header_profile(headers, &verified.sub) {
            Some(profile) => profile,
            None => self.cached_profile(token).await,
        };
        let localpart = profile
            .preferred_username
            .clone()
            .unwrap_or_else(|| verified.sub.clone());
        Ok(Caller {
            actor: actor_for(&localpart, &self.actor_domain),
            sub: verified.sub,
            role,
            email: profile.email,
            name: profile.name.or(profile.preferred_username),
        })
    }

    async fn cached_profile(&self, token: &str) -> Profile {
        let key: [u8; 32] = Sha256::digest(token.as_bytes()).into();
        if let Some((at, profile)) = self.userinfo.lock().await.get(&key) {
            if at.elapsed() < USERINFO_TTL {
                return profile.clone();
            }
        }
        let Some(profile) = self.profiles.userinfo(&self.settings.issuer, token).await else {
            return Profile::default();
        };
        let mut cache = self.userinfo.lock().await;
        if cache.len() > USERINFO_CACHE_LIMIT {
            cache.clear();
        }
        cache.insert(key, (Instant::now(), profile.clone()));
        profile
    }
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = value.split_once(' ')?;
    let token = token.trim();
    let plausible = !token.is_empty()
        && token.len() <= MAX_BEARER_BYTES
        && token.bytes().all(|byte| (0x21..=0x7e).contains(&byte));
    (scheme.eq_ignore_ascii_case("bearer") && plausible).then_some(token)
}

/// APISIX's `X-Userinfo` (base64 JSON), used only when it describes the token's own subject and
/// carries an email; otherwise the userinfo endpoint is asked.
fn header_profile(headers: &HeaderMap, sub: &str) -> Option<Profile> {
    let raw = headers.get("x-userinfo")?.to_str().ok()?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(raw)
        .or_else(|_| base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(raw))
        .ok()?;
    let value: Value = serde_json::from_slice(&bytes).ok()?;
    if value.get("sub").and_then(Value::as_str) != Some(sub) {
        return None;
    }
    Some(profile_from(&value)).filter(|profile| profile.email.is_some())
}

fn actor_for(localpart: &str, domain: &str) -> String {
    format!("@{localpart}:{domain}")
        .chars()
        .filter(|character| !character.is_control())
        .take(128)
        .collect()
}
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test --lib api::auth`
Expected: 11 tests PASS.

- [ ] **Step 7: Commit**

```bash
git add src/api/mod.rs src/api/auth.rs src/api/fakes.rs tests/fixtures/api/test-rsa-key.pem
git commit -m "feat(api): verify Zitadel bearers like the dashboard

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 6: Read-model projections

Port of `publicRun`, `publicEvent`, `publicHistoryEvent`, `publicNode`, `safeAgents`, `publicHealth`, `publicTokenUsage`, `liveStateFor`, `managedRunDisplayName`, `repositorySlug`, `backendLabel`, `safeText`, `safeMetadataText`, `safeHttpUrl`, `iso`, `integer` from `server/agent-control.ts:607-785, 1045-1119`.

**Files:**
- Create: `src/api/project.rs`
- Modify: `src/api/mod.rs` (add `pub mod project;`)

**Interfaces:**
- Produces (all `pub`): consts `TERMINAL_STATES`, `ALLOWED_JOB_STATES`, `CONTROLLER_STALE_MS = 45_000`, `NODE_STALE_MS = 45_000`; `is_terminal(&str) -> bool`; `bson_string(Option<&Bson>, fallback: &str) -> String`; `string_or(&Document, key, fallback) -> String`; `text(&Document, key) -> Option<&str>`; `nullable(&Document, key) -> Value`; `integer(Option<&Bson>, i64) -> i64`; `millis(Option<&Bson>) -> Option<i64>`; `iso(Option<&Bson>) -> Value`; `iso_now() -> String`; `is_stale(Option<&Bson>, max_ms: i64, now_ms: i64) -> bool`; `safe_text_str(&str, usize) -> Option<String>`; `safe_text(Option<&Bson>, usize) -> Option<String>`; `safe_metadata_text(Option<&Bson>, usize) -> Option<String>`; `display_name(&Document) -> String`; `public_run(&Document, Option<&Document>) -> Value`; `public_event(&Document) -> Value`; `public_history_event(row: &Document, job: &Document) -> Value`; `public_node(&Document, now_ms: i64) -> Value`; `safe_agents(Option<&Bson>) -> Vec<Value>`.

- [ ] **Step 1: Write the failing tests**

Create `src/api/project.rs` with only its test module:

```rust
//! Read-model projections, ported from the dashboard's `agent-control.ts` so both surfaces return
//! the same JSON for the same Mongo documents. Only the public API adds `owner_sub` and
//! `owner_email` to runs.

#[cfg(test)]
mod tests {
    use super::*;
    use mongodb::bson::{doc, DateTime as BsonDateTime};

    fn snapshot(allow_interactive: bool) -> Document {
        doc! { "agents": [ { "name": "staging-pi-inspector", "allow_interactive": allow_interactive } ] }
    }

    fn running_job() -> Document {
        doc! {
            "job_id": "job-new", "fleet_id": "fleet-staging", "environment": "staging",
            "agent_name": "staging-pi-inspector", "backend": "pi", "node": "worker",
            "agent_id": "agent-1", "source": "dashboard", "interactive": true,
            "state": "running", "live_state": "waiting_input", "last_event_sequence": 4_i64,
            "task_id": "task-1", "task_ref": "#42", "task_title": "Fix login",
            "created_at": BsonDateTime::from_millis(1_760_000_000_000),
            "error": "authorization: Bearer should-not-leak",
            "result_tail": "api_key=should-not-leak",
            "owner_sub": "394000000000000001", "owner_email": "ada@example.test",
        }
    }

    #[test]
    fn public_runs_redact_secrets_and_expose_the_owner() {
        let run = public_run(&running_job(), Some(&snapshot(true)));
        assert_eq!(run["error"], "authorization: Bearer [REDACTED]");
        assert_eq!(run["result_tail"], "api_key=[REDACTED]");
        assert_eq!(run["prompt_preview"], Value::Null);
        assert_eq!(run["owner_sub"], "394000000000000001");
        assert_eq!(run["owner_email"], "ada@example.test");
        assert_eq!(run["created_at"], "2025-10-09T08:53:20.000Z");
        assert_eq!(run["display_name"], "#42 · Fix login · Pi");
        assert_eq!(run["input_allowed"], true);
        assert_eq!(run["pause_allowed"], false, "no session id");
        assert_eq!(run["last_event_sequence"], 4);
    }

    #[test]
    fn input_needs_a_managed_source_and_an_interactive_profile() {
        assert_eq!(public_run(&running_job(), Some(&snapshot(false)))["input_allowed"], false);
        let mut scheduled = running_job();
        scheduled.insert("source", "schedule");
        assert_eq!(public_run(&scheduled, Some(&snapshot(true)))["input_allowed"], false);
        let mut api = running_job();
        api.insert("source", "agentctrl_api");
        assert_eq!(public_run(&api, Some(&snapshot(true)))["input_allowed"], true);
        let mut ownerless = running_job();
        ownerless.remove("owner_sub");
        ownerless.remove("owner_email");
        assert_eq!(public_run(&ownerless, None)["owner_sub"], Value::Null);
    }

    #[test]
    fn paused_runs_never_accept_input_and_can_resume() {
        let mut paused = running_job();
        paused.insert("state", "paused");
        let run = public_run(&paused, Some(&snapshot(true)));
        assert_eq!(run["live_state"], "paused");
        assert_eq!(run["input_allowed"], false);
        assert_eq!(run["resume_allowed"], true);
    }

    #[test]
    fn display_names_fall_back_to_repository_and_short_id() {
        let repo = doc! {
            "job_id": "job-repo", "backend": "claude",
            "repo_url": "https://gitlab.com/pseekoo/kaizen.git",
            "session_id": "0b6c1234-5678-90ab-cdef-1234567890ab",
        };
        assert_eq!(display_name(&repo), "pseekoo/kaizen · Claude · 0b6c1234");
        let scp = doc! { "job_id": "job-x", "backend": "codex", "repo_url": "git@gitlab.com:pseekoo/kaizen.git" };
        assert_eq!(display_name(&scp), "pseekoo/kaizen · Codex · x");
        let bare = doc! { "job_id": "job-abcdef0123", "backend": "weird" };
        assert_eq!(display_name(&bare), "Agent · abcdef01");
    }

    #[test]
    fn events_drop_metadata_and_flag_redaction() {
        let row = doc! {
            "_id": "event-3", "job_id": "job-new", "agent_id": "agent-1", "sequence": 3_i64,
            "timestamp": BsonDateTime::from_millis(0), "type": "assistant",
            "content": "password=hunter2", "metadata": { "rawCommand": "cat /run/secrets/token" },
        };
        let event = public_event(&row);
        assert_eq!(event["content"], "password=[REDACTED]");
        assert_eq!(event["redacted"], true);
        assert_eq!(event["truncated"], false);
        assert_eq!(event["metadata"], serde_json::json!({}));
        assert_eq!(event["timestamp"], "1970-01-01T00:00:00.000Z");
    }

    #[test]
    fn timestamps_accept_mongo_dates_and_rfc3339_strings() {
        assert_eq!(
            iso(Some(&Bson::String("2026-10-10T12:00:00.123456789Z".into()))),
            "2026-10-10T12:00:00.123Z"
        );
        assert_eq!(millis(Some(&Bson::DateTime(BsonDateTime::from_millis(5)))), Some(5));
        assert_eq!(iso(None), Value::Null);
        assert_eq!(iso(Some(&Bson::String("yesterday".into()))), Value::Null);
    }

    #[test]
    fn metadata_text_is_trimmed_and_byte_bounded() {
        assert_eq!(
            safe_metadata_text(Some(&Bson::String(" a\u{7}b ".into())), 64).as_deref(),
            Some("a b")
        );
        assert_eq!(safe_metadata_text(Some(&Bson::String("ééé".into())), 5).as_deref(), Some("éé"));
        assert_eq!(safe_metadata_text(Some(&Bson::String("   ".into())), 5), None);
        assert_eq!(safe_text_str("a\u{1}b", 10).as_deref(), Some("a\u{FFFD}b"));
    }

    #[test]
    fn merge_request_links_must_be_http() {
        let job = doc! { "job_id": "job-1", "mr_url": "javascript:alert(1)" };
        assert_eq!(public_run(&job, None)["mr_url"], Value::Null);
        let job = doc! { "job_id": "job-1", "mr_url": "https://git.example.com/mr/1" };
        assert_eq!(public_run(&job, None)["mr_url"], "https://git.example.com/mr/1");
    }

    #[test]
    fn nodes_report_staleness() {
        let now = 1_760_000_000_000;
        let node = doc! {
            "node_id": "n-1", "node_name": "worker", "connected": true,
            "last_seen": BsonDateTime::from_millis(now - 60_000), "backends": ["claude"],
        };
        let projected = public_node(&node, now);
        assert_eq!(projected["stale"], true);
        assert_eq!(projected["backends"], serde_json::json!(["claude"]));
        assert_eq!(projected["health"], Value::Null);
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Add `pub mod project;` to `src/api/mod.rs`. Run: `cargo test --lib api::project`
Expected: compile errors (`public_run` not found).

- [ ] **Step 3: Implement**

Above the test module in `src/api/project.rs`:

```rust
use chrono::{DateTime, SecondsFormat, TimeZone, Utc};
use mongodb::bson::{Bson, Document};
use regex::Regex;
use serde_json::{json, Value};
use std::sync::LazyLock;

pub const TERMINAL_STATES: [&str; 5] =
    ["completed", "failed", "stopped", "interrupted", "waiting_input"];
pub const ALLOWED_JOB_STATES: [&str; 9] = [
    "pending", "dispatching", "running", "paused", "completed", "waiting_input", "failed",
    "stopped", "interrupted",
];
pub const CONTROLLER_STALE_MS: i64 = 45_000;
pub const NODE_STALE_MS: i64 = 45_000;
const MAX_SAFE_INTEGER: i64 = 9_007_199_254_740_991;
const MANAGED_OPERATOR_SOURCES: [&str; 3] = ["dashboard", "warpspeed", "agentctrl_api"];

static BEARER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(authorization\s*[:=]\s*bearer\s+|bearer\s+)[A-Za-z0-9._~+/=-]+")
        .expect("valid bearer redaction pattern")
});
static SECRET: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(api[_-]?key|auth[_-]?token|password|secret)(\s*[:=]\s*)[^\s,;]+")
        .expect("valid secret redaction pattern")
});
static SCP_URL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[^@\s]+@[^:\s]+:([^?#]+)$").expect("valid scp pattern"));

pub fn is_terminal(state: &str) -> bool {
    TERMINAL_STATES.contains(&state)
}

/// JS `String(value ?? fallback)` for the values these documents hold.
pub fn bson_string(value: Option<&Bson>, fallback: &str) -> String {
    match value {
        None | Some(Bson::Null) => fallback.to_string(),
        Some(Bson::String(text)) => text.clone(),
        Some(Bson::ObjectId(id)) => id.to_hex(),
        Some(Bson::Int32(n)) => n.to_string(),
        Some(Bson::Int64(n)) => n.to_string(),
        Some(Bson::Boolean(b)) => b.to_string(),
        Some(other) => other.to_string(),
    }
}

pub fn string_or(doc: &Document, key: &str, fallback: &str) -> String {
    bson_string(doc.get(key), fallback)
}

/// A non-empty string field.
pub fn text<'a>(doc: &'a Document, key: &str) -> Option<&'a str> {
    match doc.get(key) {
        Some(Bson::String(value)) if !value.is_empty() => Some(value),
        _ => None,
    }
}

/// JS `nullableString`: the string, or null when absent or empty.
pub fn nullable(doc: &Document, key: &str) -> Value {
    text(doc, key).map_or(Value::Null, |value| Value::String(value.to_string()))
}

fn is_true(doc: &Document, key: &str) -> bool {
    doc.get_bool(key) == Ok(true)
}

pub fn integer(value: Option<&Bson>, fallback: i64) -> i64 {
    match value {
        Some(Bson::Int32(n)) => i64::from(*n),
        Some(Bson::Int64(n)) if n.abs() <= MAX_SAFE_INTEGER => *n,
        Some(Bson::Double(f)) if f.fract() == 0.0 && f.abs() <= MAX_SAFE_INTEGER as f64 => *f as i64,
        Some(Bson::String(s)) if !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) => {
            s.parse().unwrap_or(fallback)
        }
        _ => fallback,
    }
}

/// Milliseconds since the epoch for a Mongo date, an RFC 3339 string (how `Job` timestamps are
/// stored by `upsert_job`) or a number.
pub fn millis(value: Option<&Bson>) -> Option<i64> {
    match value? {
        Bson::DateTime(at) => Some(at.timestamp_millis()),
        Bson::String(text) => DateTime::parse_from_rfc3339(text)
            .ok()
            .map(|at| at.timestamp_millis()),
        Bson::Int32(n) => Some(i64::from(*n)),
        Bson::Int64(n) => Some(*n),
        Bson::Double(f) if f.is_finite() => Some(*f as i64),
        _ => None,
    }
}

fn iso_millis(ms: i64) -> Option<String> {
    Utc.timestamp_millis_opt(ms)
        .single()
        .map(|at| at.to_rfc3339_opts(SecondsFormat::Millis, true))
}

/// JS `date.toISOString()`, or null.
pub fn iso(value: Option<&Bson>) -> Value {
    millis(value)
        .and_then(iso_millis)
        .map_or(Value::Null, Value::String)
}

pub fn iso_now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

pub fn is_stale(value: Option<&Bson>, max_age_ms: i64, now_ms: i64) -> bool {
    millis(value).is_none_or(|at| now_ms - at > max_age_ms)
}

pub fn safe_text_str(value: &str, max: usize) -> Option<String> {
    if value.is_empty() {
        return None;
    }
    let redacted = BEARER.replace_all(value, "${1}[REDACTED]");
    let redacted = SECRET.replace_all(&redacted, "${1}${2}[REDACTED]");
    Some(
        redacted
            .chars()
            .map(|character| {
                let code = character as u32;
                if code <= 8 || code == 11 || code == 12 || (14..=31).contains(&code) || code == 127 {
                    '\u{FFFD}'
                } else {
                    character
                }
            })
            .take(max)
            .collect(),
    )
}

pub fn safe_text(value: Option<&Bson>, max: usize) -> Option<String> {
    match value {
        Some(Bson::String(text)) => safe_text_str(text, max),
        _ => None,
    }
}

/// Control characters become spaces, the result is trimmed and cut to `max_bytes` UTF-8 bytes.
pub fn safe_metadata_text(value: Option<&Bson>, max_bytes: usize) -> Option<String> {
    let Some(Bson::String(raw)) = value else {
        return None;
    };
    let normalized: String = raw
        .chars()
        .map(|character| {
            let code = character as u32;
            if code <= 31 || (127..=159).contains(&code) {
                ' '
            } else {
                character
            }
        })
        .collect();
    let mut bounded = String::new();
    for character in normalized.trim().chars() {
        if bounded.len() + character.len_utf8() > max_bytes {
            break;
        }
        bounded.push(character);
    }
    (!bounded.is_empty()).then_some(bounded)
}

fn safe_http_url(value: Option<&Bson>) -> Value {
    let Some(Bson::String(raw)) = value else {
        return Value::Null;
    };
    match reqwest::Url::parse(raw) {
        Ok(url) if matches!(url.scheme(), "http" | "https") => Value::String(url.to_string()),
        _ => Value::Null,
    }
}

fn repository_slug(value: Option<&Bson>) -> Option<String> {
    let Some(Bson::String(raw)) = value else {
        return None;
    };
    if raw.is_empty() {
        return None;
    }
    let path = match reqwest::Url::parse(raw) {
        Ok(url) => url.path().to_string(),
        Err(_) => SCP_URL.captures(raw)?.get(1)?.as_str().to_string(),
    };
    let mut parts: Vec<String> = path
        .split('/')
        .filter(|part| !part.is_empty())
        .map(str::to_string)
        .collect();
    let last = parts.last_mut()?;
    if last.to_ascii_lowercase().ends_with(".git") {
        last.truncate(last.len() - 4);
    }
    let slug = parts[parts.len().saturating_sub(2)..].join("/");
    (!slug.is_empty()).then_some(slug)
}

pub fn backend_label(value: Option<&Bson>) -> &'static str {
    match bson_string(value, "").trim().to_lowercase().as_str() {
        "claude" => "Claude",
        "codex" => "Codex",
        "dsh" => "DeepSeek",
        "gemini" => "Gemini",
        "kimi" => "Kimi",
        "openclaw" => "OpenClaw",
        "opencode" => "OpenCode",
        "pi" => "Pi",
        _ => "Agent",
    }
}

fn joined(parts: &[Option<String>]) -> String {
    parts
        .iter()
        .flatten()
        .filter(|part| !part.is_empty())
        .cloned()
        .collect::<Vec<_>>()
        .join(" · ")
        .chars()
        .take(256)
        .collect()
}

pub fn display_name(job: &Document) -> String {
    let backend = Some(backend_label(job.get("backend")).to_string());
    let task_ref = safe_metadata_text(job.get("task_ref"), 64);
    let task_title = safe_metadata_text(job.get("task_title"), 512);
    if task_ref.is_some() || task_title.is_some() {
        return joined(&[task_ref, task_title, backend]);
    }
    let repository = repository_slug(job.get("repo_url"));
    let session_id = safe_metadata_text(job.get("session_id"), 128);
    let job_id = safe_metadata_text(job.get("job_id"), 128)
        .map(|id| id.strip_prefix("job-").map(str::to_string).unwrap_or(id));
    let short_id = session_id
        .or(job_id)
        .map(|id| id.chars().take(8).collect::<String>());
    if repository.is_some() {
        return joined(&[repository, backend, short_id]);
    }
    joined(&[backend, short_id])
}

fn live_state_for(job: &Document, state: &str) -> &'static str {
    match state {
        "running" => match job.get("live_state") {
            Some(Bson::String(live)) => match live.as_str() {
                "starting" => "starting",
                "running" => "running",
                "waiting_input" => "waiting_input",
                _ => "unknown",
            },
            _ => "running",
        },
        "completed" => "completed",
        "failed" => "errored",
        "stopped" => "stopped",
        "paused" => "paused",
        "pending" | "dispatching" => "starting",
        _ => "unknown",
    }
}

fn number(value: Option<&Bson>) -> Value {
    match value {
        Some(Bson::Int32(n)) => json!(n),
        Some(Bson::Int64(n)) => json!(n),
        Some(Bson::Double(f)) => json!(f),
        _ => Value::Null,
    }
}

fn public_token_usage(value: Option<&Bson>) -> Value {
    let Some(Bson::Document(usage)) = value else {
        return Value::Null;
    };
    json!({
        "input_tokens": integer(usage.get("input_tokens"), 0),
        "output_tokens": integer(usage.get("output_tokens"), 0),
        "cache_creation_tokens": integer(usage.get("cache_creation_tokens"), 0),
        "cache_read_tokens": integer(usage.get("cache_read_tokens"), 0),
    })
}

fn first_present<'a>(doc: &'a Document, keys: &[&str]) -> Option<&'a Bson> {
    keys.iter()
        .filter_map(|key| doc.get(key))
        .find(|value| !matches!(value, Bson::Null))
}

pub fn public_run(job: &Document, snapshot: Option<&Document>) -> Value {
    let agent_name = job.get("agent_name");
    let agent = snapshot
        .and_then(|snapshot| snapshot.get_array("agents").ok())
        .and_then(|agents| {
            agents
                .iter()
                .filter_map(Bson::as_document)
                .find(|entry| entry.get("name") == agent_name)
        });
    let state = string_or(job, "state", "pending");
    let live_state = live_state_for(job, &state);
    let source = string_or(job, "source", "");
    let session_id = nullable(job, "session_id");
    let interactive = is_true(job, "interactive");
    let input_allowed = state == "running"
        && live_state == "waiting_input"
        && interactive
        && MANAGED_OPERATOR_SOURCES.contains(&source.as_str())
        && agent.is_some_and(|agent| is_true(agent, "allow_interactive"));
    let pause_allowed = state == "running"
        && interactive
        && !session_id.is_null()
        && text(job, "branch").is_some();
    let resume_allowed = state == "paused";
    json!({
        "job_id": string_or(job, "job_id", ""),
        "fleet_id": string_or(job, "fleet_id", ""),
        "environment": string_or(job, "environment", ""),
        "agent_name": string_or(job, "agent_name", ""),
        "display_name": display_name(job),
        "backend": string_or(job, "backend", ""),
        "node_name": nullable(job, "node"),
        "agent_id": nullable(job, "agent_id"),
        "session_id": session_id,
        "request_id": nullable(job, "request_id"),
        "requested_by": nullable(job, "requested_by"),
        "source": string_or(job, "source", "local"),
        "interactive": interactive,
        "input_allowed": input_allowed,
        "state": state,
        "live_state": live_state,
        "project_path": nullable(job, "project_path"),
        "repo_url": nullable(job, "repo_url"),
        "branch": nullable(job, "branch"),
        // Prompt content is deliberately omitted: it can contain pasted credentials.
        "prompt_preview": Value::Null,
        "created_at": iso(job.get("created_at")),
        "started_at": iso(job.get("started_at")),
        "completed_at": iso(job.get("completed_at")),
        "last_activity_at": iso(first_present(job, &["last_activity_at", "completed_at", "started_at", "created_at"])),
        "exit_code": number(job.get("exit_code")),
        "error": safe_text(job.get("error"), 1000),
        "interrupted_reason": safe_text(job.get("interrupted_reason"), 1000),
        "token_usage": public_token_usage(job.get("token_usage")),
        "result_tail": safe_text(job.get("result_tail"), 4000),
        "mr_url": safe_http_url(job.get("mr_url")),
        "last_event_sequence": integer(job.get("last_event_sequence"), 0),
        "history_event_count": integer(job.get("history_event_count"), 0),
        "session_protocol_version": integer(job.get("session_protocol_version"), 1),
        "task_id": nullable(job, "task_id"),
        "task_ref": nullable(job, "task_ref"),
        "task_title": safe_metadata_text(job.get("task_title"), 512),
        "task_room_id": nullable(job, "task_room_id"),
        "paused_at": iso(job.get("paused_at")),
        "pause_allowed": pause_allowed,
        "resume_allowed": resume_allowed,
        "owner_sub": nullable(job, "owner_sub"),
        "owner_email": nullable(job, "owner_email"),
    })
}

fn utf16_len(value: &str) -> usize {
    value.encode_utf16().count()
}

fn event_body(
    event_id: String,
    job_id: String,
    agent_id: String,
    sequence: i64,
    row: &Document,
    default_type: &str,
) -> Value {
    let raw = string_or(row, "content", "");
    let content = safe_text_str(&raw, 8192).unwrap_or_default();
    let redacted = is_true(row, "redacted") || content != raw;
    let truncated = is_true(row, "truncated") || utf16_len(&content) < utf16_len(&raw);
    json!({
        "event_id": event_id,
        "job_id": job_id,
        "agent_id": agent_id,
        "sequence": sequence,
        "timestamp": iso(row.get("timestamp")),
        "type": string_or(row, "type", default_type),
        "content": content,
        "redacted": redacted,
        "truncated": truncated,
        // Never proxy stored metadata: it can hold raw tool input or credentials.
        "metadata": {},
    })
}

pub fn public_event(row: &Document) -> Value {
    event_body(
        bson_string(first_present(row, &["_id", "event_id"]), ""),
        string_or(row, "job_id", ""),
        string_or(row, "agent_id", ""),
        integer(row.get("sequence"), 0),
        row,
        "stdout",
    )
}

pub fn public_history_event(row: &Document, job: &Document) -> Value {
    event_body(
        string_or(row, "_id", ""),
        string_or(job, "job_id", ""),
        string_or(job, "agent_id", ""),
        integer(row.get("position"), 0),
        row,
        "system",
    )
}

fn strings(value: Option<&Bson>) -> Vec<String> {
    match value {
        Some(Bson::Array(items)) => items.iter().map(|item| bson_string(Some(item), "")).collect(),
        _ => Vec::new(),
    }
}

fn float(value: Option<&Bson>) -> f64 {
    match value {
        Some(Bson::Double(f)) => *f,
        Some(Bson::Int32(n)) => f64::from(*n),
        Some(Bson::Int64(n)) => *n as f64,
        _ => 0.0,
    }
}

fn public_health(value: Option<&Bson>) -> Value {
    let Some(Bson::Document(health)) = value else {
        return Value::Null;
    };
    json!({
        "cpu_percent": float(health.get("cpu_percent")),
        "memory_used_bytes": integer(health.get("memory_used_bytes"), 0),
        "memory_total_bytes": integer(health.get("memory_total_bytes"), 0),
        "disk_free_bytes": integer(health.get("disk_free_bytes"), 0),
    })
}

pub fn public_node(row: &Document, now_ms: i64) -> Value {
    json!({
        "node_id": string_or(row, "node_id", ""),
        "node_name": string_or(row, "node_name", ""),
        "platform": string_or(row, "platform", ""),
        "arch": string_or(row, "arch", ""),
        "version": string_or(row, "agent_ctrl_version", ""),
        "backends": strings(row.get("backends")),
        "connected": is_true(row, "connected"),
        "reconciling": is_true(row, "reconciling"),
        "stale": is_stale(row.get("last_seen"), NODE_STALE_MS, now_ms),
        "last_seen": iso(row.get("last_seen")),
        "running_agents": integer(row.get("running_agents"), 0),
        "health": public_health(row.get("health")),
    })
}

pub fn safe_agents(value: Option<&Bson>) -> Vec<Value> {
    let Some(Bson::Array(agents)) = value else {
        return Vec::new();
    };
    agents
        .iter()
        .filter_map(Bson::as_document)
        .filter(|agent| agent.get_bool("visible") != Ok(false))
        .map(|agent| {
            json!({
                "name": string_or(agent, "name", ""),
                "description": nullable(agent, "description"),
                "backend": string_or(agent, "backend", ""),
                "node_name": nullable(agent, "node_name"),
                "model_label": nullable(agent, "model_label"),
                "interactive_capable": is_true(agent, "interactive_capable"),
                "max_concurrent": integer(agent.get("max_concurrent"), 1),
                "schedule_names": strings(agent.get("schedule_names")),
                "launchable": is_true(agent, "launchable"),
                "allow_interactive": is_true(agent, "allow_interactive"),
            })
        })
        .collect()
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --lib api::project`
Expected: 9 tests PASS.

- [ ] **Step 5: Commit**

```bash
cargo fmt
git add src/api/mod.rs src/api/project.rs
git commit -m "feat(api): port the dashboard's run, event and node projections

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 7: Read model and ownership

**Files:**
- Create: `src/api/store.rs`, `src/api/ownership.rs`
- Modify: `src/api/mod.rs` (modules), `src/api/fakes.rs` (`MemoryReadModel`, `job_doc`)

**Interfaces:**
- Consumes: `project::{millis, integer}` (Task 6), `auth::Caller` (Task 5).
- Produces:
  - `store::JobOrder { Created, Paused }`; `store::JobQuery { fleet_id: Option<String>, environment: String, owner_sub: Option<String>, state: Option<String>, agent_name: Option<String>, node: Option<String>, task_id: Option<String>, before: Option<(i64, String)>, order: JobOrder, limit: i64 }` (`Default`, `Clone`); `store::CommandQuery { fleet_id: String, actor: Option<String>, before: Option<(i64, String)>, limit: i64 }`; `store::StoreError(pub String)`.
  - `#[async_trait] pub trait ReadModel: Send + Sync` with `fleet_snapshot(&self, environment: &str, fleet_id: Option<&str>) -> Result<Option<Document>, StoreError>`, `nodes(&self, fleet_id: &str) -> Result<Vec<Document>, StoreError>`, `jobs(&self, &JobQuery) -> Result<Vec<Document>, StoreError>`, `job(&self, environment: &str, fleet_id: Option<&str>, job_id: &str) -> Result<Option<Document>, StoreError>`, `earliest_event_sequence(&self, fleet_id: &str, job_id: &str) -> Result<Option<u64>, StoreError>`, `events_after(&self, fleet_id, job_id, after: u64, limit: i64) -> Result<Vec<Document>, StoreError>`, `history_after(&self, fleet_id, job_id, after: u64, limit: i64) -> Result<Vec<Document>, StoreError>`, `commands(&self, &CommandQuery) -> Result<Vec<Document>, StoreError>`.
  - `store::job_filter(&JobQuery) -> Document`, `store::after_page_cursor(&Document, &(i64, String), id_key: &str) -> bool`, `store::MongoReadModel::new(mongodb::Database)`.
  - `ownership::{owner_filter(&Caller) -> Option<String>, can_see(&Caller, &Document) -> bool, command_actor_filter(&Caller) -> Option<String>}`.
  - Test-only `fakes::MemoryReadModel` (`insert(&self, collection, Document)`, `load_records(&self, &Value, now_ms)`, `fail(&self, collection: &str)` (`"*"` = every collection)), `fakes::bson_from_json(&Value, now_ms) -> Bson`, `fakes::job_doc(job_id, owner: Option<&str>, created_offset_ms: i64) -> Document`.

Paging note: `upsert_job` stores `Job.created_at` as an RFC 3339 string (the field has no `mongo_datetime`), while the dashboard compares it with a Mongo date, which never matches a string. The API therefore sorts in Mongo and applies the `(created_at, id)` page cursor in Rust at millisecond precision, accepting either stored type.

- [ ] **Step 1: Write the failing tests**

Create `src/api/ownership.rs`:

```rust
//! Who sees which run: admins every run, members only runs they own. Runs without an owner
//! (started before owners existed, or by schedules and work sources) are admin-only.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::auth::{Caller, Role};
    use mongodb::bson::doc;

    fn caller(sub: &str, role: Role) -> Caller {
        Caller {
            sub: sub.into(),
            role,
            email: None,
            name: None,
            actor: format!("@{sub}:example.test"),
        }
    }

    #[test]
    fn members_see_only_their_own_runs_and_admins_see_all() {
        let admin = caller("admin", Role::Admin);
        let ada = caller("ada", Role::Member);
        let own = doc! { "job_id": "job-1", "owner_sub": "ada" };
        let other = doc! { "job_id": "job-2", "owner_sub": "bob" };
        let ownerless = doc! { "job_id": "job-3" };
        let empty_owner = doc! { "job_id": "job-4", "owner_sub": "" };
        assert!(can_see(&ada, &own));
        assert!(!can_see(&ada, &other));
        assert!(!can_see(&ada, &ownerless));
        assert!(!can_see(&ada, &empty_owner));
        for job in [&own, &other, &ownerless, &empty_owner] {
            assert!(can_see(&admin, job));
        }
        assert_eq!(owner_filter(&admin), None);
        assert_eq!(owner_filter(&ada).as_deref(), Some("ada"));
        assert_eq!(command_actor_filter(&admin), None);
        assert_eq!(command_actor_filter(&ada).as_deref(), Some("@ada:example.test"));
    }
}
```

Create `src/api/store.rs` with only its tests:

```rust
//! The Mongo read model the API lists and pages from, behind a trait so handlers are testable
//! without a database.

#[cfg(test)]
mod tests {
    use super::*;
    use mongodb::bson::{doc, DateTime as BsonDateTime};

    #[test]
    fn job_filters_scope_by_environment_fleet_owner_and_query() {
        let query = JobQuery {
            fleet_id: Some("fleet-staging".into()),
            environment: "staging".into(),
            owner_sub: Some("ada".into()),
            state: Some("running".into()),
            agent_name: Some("claude".into()),
            node: Some("agent-ctrl-node-0".into()),
            task_id: None,
            ..JobQuery::default()
        };
        assert_eq!(
            job_filter(&query),
            doc! {
                "environment": "staging", "fleet_id": "fleet-staging", "owner_sub": "ada",
                "state": "running", "agent_name": "claude", "node": "agent-ctrl-node-0",
            }
        );
        let admin = JobQuery {
            environment: "staging".into(),
            ..JobQuery::default()
        };
        assert_eq!(job_filter(&admin), doc! { "environment": "staging" });
    }

    #[test]
    fn page_cursors_compare_strings_and_dates_at_millisecond_precision() {
        let cursor = (1_760_000_000_123, "job-b".to_string());
        let string_date = doc! { "job_id": "job-a", "created_at": "2025-10-09T08:53:20.123456789Z" };
        let mongo_date = doc! { "job_id": "job-c", "created_at": BsonDateTime::from_millis(1_760_000_000_123) };
        let older = doc! { "job_id": "job-z", "created_at": BsonDateTime::from_millis(1_760_000_000_000) };
        assert!(after_page_cursor(&string_date, &cursor, "job_id"), "same ms, lower id");
        assert!(!after_page_cursor(&mongo_date, &cursor, "job_id"), "same ms, higher id");
        assert!(after_page_cursor(&older, &cursor, "job_id"));
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Add `pub mod ownership;` and `pub mod store;` to `src/api/mod.rs`. Run: `cargo test --lib api::ownership` and `cargo test --lib api::store`
Expected: compile errors.

- [ ] **Step 3: Implement `ownership.rs`**

Above its test module:

```rust
use super::auth::Caller;
use mongodb::bson::Document;

/// The `owner_sub` a member's run queries are limited to; none for admins.
pub fn owner_filter(caller: &Caller) -> Option<String> {
    (!caller.is_admin()).then(|| caller.sub.clone())
}

pub fn can_see(caller: &Caller, job: &Document) -> bool {
    caller.is_admin()
        || job
            .get_str("owner_sub")
            .is_ok_and(|owner| !owner.is_empty() && owner == caller.sub)
}

/// The audit actor a member's command listing is limited to; none for admins.
pub fn command_actor_filter(caller: &Caller) -> Option<String> {
    (!caller.is_admin()).then(|| caller.actor.clone())
}
```

- [ ] **Step 4: Implement `store.rs`**

Above its test module:

```rust
use super::project::millis;
use async_trait::async_trait;
use futures::TryStreamExt;
use mongodb::bson::{doc, Bson, Document};
use mongodb::{Collection, Database};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum JobOrder {
    /// `created_at` then `job_id`, newest first.
    #[default]
    Created,
    /// `paused_at`, newest first.
    Paused,
}

#[derive(Debug, Clone, Default)]
pub struct JobQuery {
    pub fleet_id: Option<String>,
    pub environment: String,
    pub owner_sub: Option<String>,
    pub state: Option<String>,
    pub agent_name: Option<String>,
    pub node: Option<String>,
    pub task_id: Option<String>,
    /// Page cursor `(created_at ms, job_id)`: only jobs strictly after it.
    pub before: Option<(i64, String)>,
    pub order: JobOrder,
    pub limit: i64,
}

#[derive(Debug, Clone, Default)]
pub struct CommandQuery {
    pub fleet_id: String,
    pub actor: Option<String>,
    /// Page cursor `(created_at ms, request_id)`.
    pub before: Option<(i64, String)>,
    pub limit: i64,
}

#[derive(Debug, thiserror::Error)]
#[error("read model unavailable: {0}")]
pub struct StoreError(pub String);

impl From<mongodb::error::Error> for StoreError {
    fn from(error: mongodb::error::Error) -> Self {
        Self(error.to_string())
    }
}

#[async_trait]
pub trait ReadModel: Send + Sync {
    async fn fleet_snapshot(
        &self,
        environment: &str,
        fleet_id: Option<&str>,
    ) -> Result<Option<Document>, StoreError>;
    async fn nodes(&self, fleet_id: &str) -> Result<Vec<Document>, StoreError>;
    async fn jobs(&self, query: &JobQuery) -> Result<Vec<Document>, StoreError>;
    async fn job(
        &self,
        environment: &str,
        fleet_id: Option<&str>,
        job_id: &str,
    ) -> Result<Option<Document>, StoreError>;
    async fn earliest_event_sequence(
        &self,
        fleet_id: &str,
        job_id: &str,
    ) -> Result<Option<u64>, StoreError>;
    async fn events_after(
        &self,
        fleet_id: &str,
        job_id: &str,
        after: u64,
        limit: i64,
    ) -> Result<Vec<Document>, StoreError>;
    async fn history_after(
        &self,
        fleet_id: &str,
        job_id: &str,
        after: u64,
        limit: i64,
    ) -> Result<Vec<Document>, StoreError>;
    async fn commands(&self, query: &CommandQuery) -> Result<Vec<Document>, StoreError>;
}

pub fn job_filter(query: &JobQuery) -> Document {
    let mut filter = doc! { "environment": &query.environment };
    for (key, value) in [
        ("fleet_id", &query.fleet_id),
        ("owner_sub", &query.owner_sub),
        ("state", &query.state),
        ("agent_name", &query.agent_name),
        ("node", &query.node),
        ("task_id", &query.task_id),
    ] {
        if let Some(value) = value {
            filter.insert(key, value.as_str());
        }
    }
    filter
}

/// Whether `row` sorts strictly after the page cursor in `(created_at desc, <id_key> desc)` order.
pub fn after_page_cursor(row: &Document, cursor: &(i64, String), id_key: &str) -> bool {
    let time = millis(row.get("created_at")).unwrap_or(0);
    let id = row.get_str(id_key).unwrap_or_default();
    time < cursor.0 || (time == cursor.0 && id < cursor.1.as_str())
}

fn sequence(row: &Document, key: &str) -> Option<u64> {
    match row.get(key)? {
        Bson::Int32(n) => u64::try_from(*n).ok(),
        Bson::Int64(n) => u64::try_from(*n).ok(),
        Bson::Double(f) if *f >= 0.0 && f.fract() == 0.0 => Some(*f as u64),
        _ => None,
    }
}

pub struct MongoReadModel {
    db: Database,
}

impl MongoReadModel {
    pub fn new(db: Database) -> Self {
        Self { db }
    }

    fn collection(&self, name: &str) -> Collection<Document> {
        self.db.collection(name)
    }

    async fn page(
        &self,
        collection: &str,
        filter: Document,
        sort: Document,
        cursor: Option<&(i64, String)>,
        id_key: &str,
        limit: i64,
    ) -> Result<Vec<Document>, StoreError> {
        let mut rows = self.collection(collection).find(filter).sort(sort).await?;
        let mut page = Vec::new();
        while page.len() < limit.max(0) as usize {
            let Some(row) = rows.try_next().await? else {
                break;
            };
            if cursor.is_none_or(|cursor| after_page_cursor(&row, cursor, id_key)) {
                page.push(row);
            }
        }
        Ok(page)
    }
}

#[async_trait]
impl ReadModel for MongoReadModel {
    async fn fleet_snapshot(
        &self,
        environment: &str,
        fleet_id: Option<&str>,
    ) -> Result<Option<Document>, StoreError> {
        let filter = match fleet_id {
            Some(fleet_id) => doc! { "_id": fleet_id, "environment": environment },
            None => doc! { "environment": environment },
        };
        let mut rows = self
            .collection("fleet_snapshots")
            .find(filter)
            .sort(doc! { "last_seen": -1 })
            .limit(1)
            .await?;
        Ok(rows.try_next().await?)
    }

    async fn nodes(&self, fleet_id: &str) -> Result<Vec<Document>, StoreError> {
        let rows = self
            .collection("nodes")
            .find(doc! { "fleet_id": fleet_id })
            .sort(doc! { "node_name": 1 })
            .await?;
        Ok(rows.try_collect().await?)
    }

    async fn jobs(&self, query: &JobQuery) -> Result<Vec<Document>, StoreError> {
        let sort = match query.order {
            JobOrder::Created => doc! { "created_at": -1, "job_id": -1 },
            JobOrder::Paused => doc! { "paused_at": -1 },
        };
        self.page("jobs", job_filter(query), sort, query.before.as_ref(), "job_id", query.limit)
            .await
    }

    async fn job(
        &self,
        environment: &str,
        fleet_id: Option<&str>,
        job_id: &str,
    ) -> Result<Option<Document>, StoreError> {
        let mut filter = doc! { "environment": environment, "job_id": job_id };
        if let Some(fleet_id) = fleet_id {
            filter.insert("fleet_id", fleet_id);
        }
        Ok(self.collection("jobs").find_one(filter).await?)
    }

    async fn earliest_event_sequence(
        &self,
        fleet_id: &str,
        job_id: &str,
    ) -> Result<Option<u64>, StoreError> {
        let mut rows = self
            .collection("job_events")
            .find(doc! { "fleet_id": fleet_id, "job_id": job_id })
            .sort(doc! { "sequence": 1 })
            .limit(1)
            .await?;
        Ok(rows.try_next().await?.and_then(|row| sequence(&row, "sequence")))
    }

    async fn events_after(
        &self,
        fleet_id: &str,
        job_id: &str,
        after: u64,
        limit: i64,
    ) -> Result<Vec<Document>, StoreError> {
        let rows = self
            .collection("job_events")
            .find(doc! { "fleet_id": fleet_id, "job_id": job_id, "sequence": { "$gt": after as i64 } })
            .sort(doc! { "sequence": 1 })
            .limit(limit)
            .await?;
        Ok(rows.try_collect().await?)
    }

    async fn history_after(
        &self,
        fleet_id: &str,
        job_id: &str,
        after: u64,
        limit: i64,
    ) -> Result<Vec<Document>, StoreError> {
        let rows = self
            .collection("job_history")
            .find(doc! { "fleet_id": fleet_id, "job_id": job_id, "position": { "$gt": after as i64 } })
            .sort(doc! { "position": 1 })
            .limit(limit)
            .await?;
        Ok(rows.try_collect().await?)
    }

    async fn commands(&self, query: &CommandQuery) -> Result<Vec<Document>, StoreError> {
        let mut filter = doc! { "fleet_id": &query.fleet_id };
        if let Some(actor) = &query.actor {
            filter.insert("actor", actor.as_str());
        }
        self.page(
            "control_commands",
            filter,
            doc! { "created_at": -1, "request_id": -1 },
            query.before.as_ref(),
            "request_id",
            query.limit,
        )
        .await
    }
}
```

- [ ] **Step 5: `MemoryReadModel`**

Add to the `use` block at the top of `src/api/fakes.rs`:

```rust
use super::project::{integer, millis};
use super::store::{after_page_cursor, CommandQuery, JobOrder, JobQuery, ReadModel, StoreError};
use mongodb::bson::{Bson, DateTime as BsonDateTime, Document};
use std::collections::{HashMap, HashSet};
```

Append to `src/api/fakes.rs`:

```rust
/// An in-memory `ReadModel` with the Mongo implementation's filter and sort semantics.
#[derive(Default)]
pub struct MemoryReadModel {
    collections: Mutex<HashMap<String, Vec<Document>>>,
    failing: Mutex<HashSet<String>>,
}

impl MemoryReadModel {
    pub fn insert(&self, collection: &str, doc: Document) {
        self.collections
            .lock()
            .unwrap()
            .entry(collection.to_string())
            .or_default()
            .push(doc);
    }

    /// Loads `{collection: [documents]}` JSON; `{"$offset_ms": n}` becomes the date `now_ms + n`.
    pub fn load_records(&self, records: &Value, now_ms: i64) {
        for (collection, rows) in records.as_object().expect("records object") {
            for row in rows.as_array().expect("collection array") {
                match bson_from_json(row, now_ms) {
                    Bson::Document(doc) => self.insert(collection, doc),
                    other => panic!("{collection} row is not an object: {other:?}"),
                }
            }
        }
    }

    /// Makes reads of `collection` fail from now on; `*` fails every collection.
    pub fn fail(&self, collection: &str) {
        self.failing.lock().unwrap().insert(collection.to_string());
    }

    fn rows(&self, collection: &str) -> Result<Vec<Document>, StoreError> {
        {
            let failing = self.failing.lock().unwrap();
            if failing.contains("*") || failing.contains(collection) {
                return Err(StoreError("injected failure".into()));
            }
        }
        Ok(self
            .collections
            .lock()
            .unwrap()
            .get(collection)
            .cloned()
            .unwrap_or_default())
    }

    fn events(&self, fleet_id: &str, job_id: &str) -> Result<Vec<Document>, StoreError> {
        let mut rows: Vec<Document> = self
            .rows("job_events")?
            .into_iter()
            .filter(|row| {
                row.get_str("fleet_id").ok() == Some(fleet_id)
                    && row.get_str("job_id").ok() == Some(job_id)
            })
            .collect();
        rows.sort_by_key(|row| integer(row.get("sequence"), 0));
        Ok(rows)
    }
}

pub fn bson_from_json(value: &Value, now_ms: i64) -> Bson {
    match value {
        Value::Null => Bson::Null,
        Value::Bool(flag) => Bson::Boolean(*flag),
        Value::Number(number) => number
            .as_i64()
            .map(Bson::Int64)
            .unwrap_or_else(|| Bson::Double(number.as_f64().unwrap_or(0.0))),
        Value::String(text) => Bson::String(text.clone()),
        Value::Array(items) => {
            Bson::Array(items.iter().map(|item| bson_from_json(item, now_ms)).collect())
        }
        Value::Object(map) => match (map.len(), map.get("$offset_ms").and_then(Value::as_i64)) {
            (1, Some(offset)) => Bson::DateTime(BsonDateTime::from_millis(now_ms + offset)),
            _ => Bson::Document(
                map.iter()
                    .map(|(key, value)| (key.clone(), bson_from_json(value, now_ms)))
                    .collect(),
            ),
        },
    }
}

fn matches_field(row: &Document, key: &str, expected: &Option<String>) -> bool {
    expected
        .as_ref()
        .is_none_or(|expected| row.get_str(key).ok() == Some(expected.as_str()))
}

fn sort_newest(rows: &mut [Document], id_key: &str) {
    let key = |row: &Document| {
        (
            millis(row.get("created_at")).unwrap_or(0),
            row.get_str(id_key).unwrap_or_default().to_string(),
        )
    };
    rows.sort_by(|a, b| key(b).cmp(&key(a)));
}

#[async_trait]
impl ReadModel for MemoryReadModel {
    async fn fleet_snapshot(
        &self,
        environment: &str,
        fleet_id: Option<&str>,
    ) -> Result<Option<Document>, StoreError> {
        let mut rows: Vec<Document> = self
            .rows("fleet_snapshots")?
            .into_iter()
            .filter(|row| row.get_str("environment").ok() == Some(environment))
            .filter(|row| fleet_id.is_none_or(|id| row.get_str("_id").ok() == Some(id)))
            .collect();
        rows.sort_by_key(|row| std::cmp::Reverse(millis(row.get("last_seen")).unwrap_or(0)));
        Ok(rows.into_iter().next())
    }

    async fn nodes(&self, fleet_id: &str) -> Result<Vec<Document>, StoreError> {
        let mut rows: Vec<Document> = self
            .rows("nodes")?
            .into_iter()
            .filter(|row| row.get_str("fleet_id").ok() == Some(fleet_id))
            .collect();
        rows.sort_by(|a, b| {
            a.get_str("node_name")
                .unwrap_or_default()
                .cmp(b.get_str("node_name").unwrap_or_default())
        });
        Ok(rows)
    }

    async fn jobs(&self, query: &JobQuery) -> Result<Vec<Document>, StoreError> {
        let mut rows: Vec<Document> = self
            .rows("jobs")?
            .into_iter()
            .filter(|row| row.get_str("environment").ok() == Some(query.environment.as_str()))
            .filter(|row| {
                matches_field(row, "fleet_id", &query.fleet_id)
                    && matches_field(row, "owner_sub", &query.owner_sub)
                    && matches_field(row, "state", &query.state)
                    && matches_field(row, "agent_name", &query.agent_name)
                    && matches_field(row, "node", &query.node)
                    && matches_field(row, "task_id", &query.task_id)
            })
            .collect();
        match query.order {
            JobOrder::Created => sort_newest(&mut rows, "job_id"),
            JobOrder::Paused => rows.sort_by_key(|row| {
                std::cmp::Reverse(millis(row.get("paused_at")).unwrap_or(0))
            }),
        }
        Ok(rows
            .into_iter()
            .filter(|row| {
                query
                    .before
                    .as_ref()
                    .is_none_or(|cursor| after_page_cursor(row, cursor, "job_id"))
            })
            .take(query.limit.max(0) as usize)
            .collect())
    }

    async fn job(
        &self,
        environment: &str,
        fleet_id: Option<&str>,
        job_id: &str,
    ) -> Result<Option<Document>, StoreError> {
        Ok(self.rows("jobs")?.into_iter().find(|row| {
            row.get_str("environment").ok() == Some(environment)
                && row.get_str("job_id").ok() == Some(job_id)
                && fleet_id.is_none_or(|id| row.get_str("fleet_id").ok() == Some(id))
        }))
    }

    async fn earliest_event_sequence(
        &self,
        fleet_id: &str,
        job_id: &str,
    ) -> Result<Option<u64>, StoreError> {
        Ok(self
            .events(fleet_id, job_id)?
            .first()
            .map(|row| integer(row.get("sequence"), 0).max(0) as u64))
    }

    async fn events_after(
        &self,
        fleet_id: &str,
        job_id: &str,
        after: u64,
        limit: i64,
    ) -> Result<Vec<Document>, StoreError> {
        Ok(self
            .events(fleet_id, job_id)?
            .into_iter()
            .filter(|row| integer(row.get("sequence"), 0) > after as i64)
            .take(limit.max(0) as usize)
            .collect())
    }

    async fn history_after(
        &self,
        fleet_id: &str,
        job_id: &str,
        after: u64,
        limit: i64,
    ) -> Result<Vec<Document>, StoreError> {
        let mut rows: Vec<Document> = self
            .rows("job_history")?
            .into_iter()
            .filter(|row| {
                row.get_str("fleet_id").ok() == Some(fleet_id)
                    && row.get_str("job_id").ok() == Some(job_id)
            })
            .filter(|row| integer(row.get("position"), 0) > after as i64)
            .collect();
        rows.sort_by_key(|row| integer(row.get("position"), 0));
        Ok(rows.into_iter().take(limit.max(0) as usize).collect())
    }

    async fn commands(&self, query: &CommandQuery) -> Result<Vec<Document>, StoreError> {
        let mut rows: Vec<Document> = self
            .rows("control_commands")?
            .into_iter()
            .filter(|row| row.get_str("fleet_id").ok() == Some(query.fleet_id.as_str()))
            .filter(|row| matches_field(row, "actor", &query.actor))
            .collect();
        sort_newest(&mut rows, "request_id");
        Ok(rows
            .into_iter()
            .filter(|row| {
                query
                    .before
                    .as_ref()
                    .is_none_or(|cursor| after_page_cursor(row, cursor, "request_id"))
            })
            .take(query.limit.max(0) as usize)
            .collect())
    }
}

/// A staging job document created `created_offset_ms` from now, owned by `owner` when given.
pub fn job_doc(job_id: &str, owner: Option<&str>, created_offset_ms: i64) -> Document {
    let created = chrono::Utc::now().timestamp_millis() + created_offset_ms;
    let mut job = mongodb::bson::doc! {
        "job_id": job_id, "fleet_id": "fleet-staging", "environment": "staging",
        "agent_name": "staging-claude-interactive-rw", "backend": "claude",
        "node": "agent-ctrl-node-0", "agent_id": format!("agent-{job_id}"),
        "session_id": format!("session-{job_id}"), "branch": format!("agent/{job_id}"),
        "source": "agentctrl_api", "interactive": true, "state": "running",
        "live_state": "waiting_input", "session_protocol_version": 1_i64,
        "created_at": BsonDateTime::from_millis(created),
    };
    if let Some(owner) = owner {
        job.insert("owner_sub", owner);
        job.insert("owner_email", format!("{owner}@example.test"));
    }
    job
}
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test --lib api::ownership && cargo test --lib api::store && cargo test --lib api::`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
cargo fmt
git add src/api/mod.rs src/api/store.rs src/api/ownership.rs src/api/fakes.rs
git commit -m "feat(api): add the Mongo read model and per-owner visibility

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 8: FleetControl seam and request guards

**Files:**
- Create: `src/api/control.rs`, `src/api/limits.rs`
- Modify: `src/api/mod.rs` (modules), `src/api/fakes.rs` (`FakeControl`)

**Interfaces:**
- Produces:
  - `#[async_trait] pub trait ControlPlane: Send + Sync` with `async fn spawn_agent(&self, pb::SpawnAgentRequest) -> Result<pb::SpawnAgentResponse, Status>`, `import_session(pb::ImportSessionRequest) -> Result<pb::SpawnAgentResponse, Status>`, `import_session_history(pb::ImportSessionHistoryRequest) -> Result<pb::ImportSessionHistoryResponse, Status>`, `send_input(pb::SendInputRequest) -> Result<pb::SendInputResponse, Status>`, `stop_agent(pb::StopAgentRequest) -> Result<pb::StopAgentResponse, Status>`, `pause_agent(pb::PauseAgentRequest) -> Result<pb::PauseAgentResponse, Status>`, `resume_agent(pb::ResumeAgentRequest) -> Result<pb::ResumeAgentResponse, Status>`, `list_repositories(&self) -> Result<pb::ListRepositoriesResponse, Status>`.
  - `GrpcControlPlane::connect_lazy(addr: String) -> crate::Result<Self>` (control credential from `AGENT_CTRL_CONTROL_AUTH_TOKEN`).
  - `limits::{MAX_MUTATION_BODY_BYTES, MAX_SESSION_FILE_BYTES, media_type, require_json, idempotency_key, request_source, json_body, exact_keys, required_string, optional_string, bounded_integer, RateLimiter}` (signatures below).
  - Test-only `fakes::FakeControl { spawns: Mutex<Vec<pb::SpawnAgentRequest>>, imports: Mutex<Vec<pb::ImportSessionRequest>>, calls: AtomicUsize, repositories_unavailable: AtomicBool }`, scripted by request-id prefix like the dashboard's fake.

- [ ] **Step 1: Write the failing tests**

Create `src/api/limits.rs` with only its tests:

```rust
//! Request guards shared by the mutation routes: media type, idempotency key, body size, field
//! validation and the per-actor rate limit. Messages and codes match the dashboard.

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;
    use serde_json::json;

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for (name, value) in pairs {
            headers.insert(*name, HeaderValue::from_str(value).unwrap());
        }
        headers
    }

    #[test]
    fn idempotency_keys_are_16_to_128_safe_characters() {
        assert_eq!(
            idempotency_key(&headers(&[("idempotency-key", "input-request-0001")])).unwrap(),
            "input-request-0001"
        );
        let long = "x".repeat(129);
        for bad in ["short", "has space in it here", long.as_str()] {
            let error = idempotency_key(&headers(&[("idempotency-key", bad)])).unwrap_err();
            assert_eq!((error.status.as_u16(), error.code), (400, "invalid_idempotency_key"));
        }
        assert_eq!(
            idempotency_key(&HeaderMap::new()).unwrap_err().code,
            "invalid_idempotency_key"
        );
    }

    #[test]
    fn mutations_require_json_and_name_their_source() {
        assert!(require_json(&headers(&[("content-type", "application/json; charset=utf-8")])).is_ok());
        let error = require_json(&headers(&[("content-type", "text/plain")])).unwrap_err();
        assert_eq!((error.status.as_u16(), error.code), (415, "unsupported_media_type"));
        assert_eq!(request_source(&headers(&[("x-stozn-client", "WarpSpeed")])), "warpspeed");
        assert_eq!(request_source(&headers(&[("x-stozn-client", "warp")])), "warpspeed");
        assert_eq!(request_source(&HeaderMap::new()), "agentctrl_api");
    }

    #[tokio::test]
    async fn json_bodies_are_capped_and_must_be_objects() {
        let ok = json_body(&HeaderMap::new(), Body::from(r#"{"text":"hi"}"#)).await.unwrap();
        assert_eq!(ok["text"], "hi");
        let big = "x".repeat(MAX_MUTATION_BODY_BYTES + 1);
        let error = json_body(&HeaderMap::new(), Body::from(format!(r#"{{"text":"{big}"}}"#)))
            .await
            .unwrap_err();
        assert_eq!((error.status.as_u16(), error.code), (413, "body_too_large"));
        let declared = headers(&[("content-length", "70000")]);
        assert_eq!(json_body(&declared, Body::empty()).await.unwrap_err().code, "body_too_large");
        assert_eq!(
            json_body(&HeaderMap::new(), Body::from("[]")).await.unwrap_err().message,
            "Request body must be a JSON object"
        );
        assert_eq!(
            json_body(&HeaderMap::new(), Body::from("{")).await.unwrap_err().message,
            "Request body must be valid UTF-8 JSON"
        );
    }

    #[test]
    fn fields_are_validated_like_the_dashboard() {
        let body = json!({"text": "hi", "extra": 1}).as_object().unwrap().clone();
        let error = exact_keys(&body, &["text"]).unwrap_err();
        assert_eq!(
            (error.status.as_u16(), error.message.as_str()),
            (422, "Unsupported fields: extra")
        );
        assert_eq!(required_string(Some(&json!("ab")), "text", 2).unwrap(), "ab");
        assert_eq!(
            required_string(Some(&json!("abc")), "text", 2).unwrap_err().message,
            "text must contain 1 to 2 characters without NUL bytes"
        );
        assert!(required_string(Some(&json!("a\u{0}")), "text", 8).is_err());
        assert!(required_string(None, "text", 8).is_err());
        assert_eq!(optional_string(Some(&serde_json::Value::Null), "x", 8).unwrap(), None);
        assert_eq!(bounded_integer(None, 50, 1, 200).unwrap(), 50);
        assert_eq!(bounded_integer(Some("200"), 50, 1, 200).unwrap(), 200);
        assert_eq!(
            bounded_integer(Some("0"), 50, 1, 200).unwrap_err().message,
            "Numeric query parameter is out of range"
        );
        assert_eq!(
            bounded_integer(Some("-1"), 50, 1, 200).unwrap_err().message,
            "Invalid numeric query parameter"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn thirty_mutations_per_minute_per_actor() {
        let limiter = RateLimiter::default();
        for _ in 0..30 {
            limiter.check("@ada:example.test").unwrap();
        }
        let error = limiter.check("@ada:example.test").unwrap_err();
        assert_eq!((error.status.as_u16(), error.code), (429, "rate_limited"));
        assert!(limiter.check("@bob:example.test").is_ok(), "limits are per actor");
        tokio::time::advance(std::time::Duration::from_secs(61)).await;
        assert!(limiter.check("@ada:example.test").is_ok());
    }
}
```

Create `src/api/control.rs` with only its tests:

```rust
//! FleetControl behind a trait, so handlers can be tested with a scripted fake.

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn the_grpc_control_plane_starts_without_a_controller() {
        assert!(GrpcControlPlane::connect_lazy("http://127.0.0.1:9".into()).is_ok());
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Add `pub mod control;` and `pub mod limits;` to `src/api/mod.rs`. Run: `cargo test --lib api::limits` and `cargo test --lib api::control`
Expected: compile errors.

- [ ] **Step 3: Implement `limits.rs`**

Above its test module:

```rust
use super::error::ApiError;
use axum::body::Body;
use axum::http::{header, HeaderMap};
use regex::Regex;
use serde_json::{Map, Value};
use std::collections::{HashMap, VecDeque};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;
use tokio::time::Instant;

pub const MAX_MUTATION_BODY_BYTES: usize = 65_536;
pub const MAX_SESSION_FILE_BYTES: usize = crate::remote::MAX_SESSION_FILE_BYTES;
const MUTATIONS_PER_WINDOW: usize = 30;
const RATE_WINDOW: Duration = Duration::from_secs(60);

static IDEMPOTENCY_KEY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[A-Za-z0-9._:-]{16,128}$").expect("valid idempotency pattern")
});

/// The lowercased media type before any parameters.
pub fn media_type(headers: &HeaderMap) -> String {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
}

pub fn require_json(headers: &HeaderMap) -> Result<(), ApiError> {
    if media_type(headers) == "application/json" {
        Ok(())
    } else {
        Err(ApiError::new(415, "Mutation requires application/json", "unsupported_media_type"))
    }
}

pub fn idempotency_key(headers: &HeaderMap) -> Result<String, ApiError> {
    headers
        .get("idempotency-key")
        .and_then(|value| value.to_str().ok())
        .filter(|value| IDEMPOTENCY_KEY.is_match(value))
        .map(str::to_string)
        .ok_or_else(|| {
            ApiError::new(400, "A valid Idempotency-Key is required", "invalid_idempotency_key")
        })
}

/// The FleetControl mutation source: WarpSpeed identifies itself with `X-Stozn-Client`.
pub fn request_source(headers: &HeaderMap) -> &'static str {
    let client = headers
        .get("x-stozn-client")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if client == "warpspeed" || client == "warp" {
        "warpspeed"
    } else {
        "agentctrl_api"
    }
}

fn too_large() -> ApiError {
    ApiError::new(413, "Mutation body is too large", "body_too_large")
}

pub async fn json_body(headers: &HeaderMap, body: Body) -> Result<Map<String, Value>, ApiError> {
    let declared = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<usize>().ok());
    if declared.is_some_and(|length| length > MAX_MUTATION_BODY_BYTES) {
        return Err(too_large());
    }
    let bytes = axum::body::to_bytes(body, MAX_MUTATION_BODY_BYTES)
        .await
        .map_err(|_| too_large())?;
    let invalid = || ApiError::new(400, "Request body must be valid UTF-8 JSON", "bad_request");
    let text = std::str::from_utf8(&bytes).map_err(|_| invalid())?;
    match serde_json::from_str::<Value>(text).map_err(|_| invalid())? {
        Value::Object(map) => Ok(map),
        _ => Err(ApiError::new(400, "Request body must be a JSON object", "bad_request")),
    }
}

pub fn exact_keys(body: &Map<String, Value>, allowed: &[&str]) -> Result<(), ApiError> {
    let unknown: Vec<&str> = body
        .keys()
        .map(String::as_str)
        .filter(|key| !allowed.contains(key))
        .collect();
    if unknown.is_empty() {
        Ok(())
    } else {
        Err(ApiError::new(
            422,
            format!("Unsupported fields: {}", unknown.join(", ")),
            "validation_error",
        ))
    }
}

pub fn required_string(value: Option<&Value>, field: &str, max: usize) -> Result<String, ApiError> {
    match value {
        Some(Value::String(text))
            if !text.is_empty() && text.chars().count() <= max && !text.contains('\0') =>
        {
            Ok(text.clone())
        }
        _ => Err(ApiError::new(
            422,
            format!("{field} must contain 1 to {max} characters without NUL bytes"),
            "validation_error",
        )),
    }
}

pub fn optional_string(
    value: Option<&Value>,
    field: &str,
    max: usize,
) -> Result<Option<String>, ApiError> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(_) => required_string(value, field, max).map(Some),
    }
}

pub fn bounded_integer(value: Option<&str>, fallback: i64, min: i64, max: i64) -> Result<i64, ApiError> {
    let Some(value) = value.filter(|value| !value.is_empty()) else {
        return Ok(fallback);
    };
    if !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(ApiError::new(422, "Invalid numeric query parameter", "validation_error"));
    }
    value
        .parse::<i64>()
        .ok()
        .filter(|parsed| (min..=max).contains(parsed))
        .ok_or_else(|| {
            ApiError::new(422, "Numeric query parameter is out of range", "validation_error")
        })
}

/// 30 mutations per rolling 60 s per actor, per API replica, like the dashboard per instance.
#[derive(Default)]
pub struct RateLimiter {
    windows: Mutex<HashMap<String, VecDeque<Instant>>>,
}

impl RateLimiter {
    pub fn check(&self, actor: &str) -> Result<(), ApiError> {
        let now = Instant::now();
        let mut windows = self.windows.lock().expect("rate limiter lock");
        let recent = windows.entry(actor.to_string()).or_default();
        while recent
            .front()
            .is_some_and(|at| now.duration_since(*at) >= RATE_WINDOW)
        {
            recent.pop_front();
        }
        if recent.len() >= MUTATIONS_PER_WINDOW {
            return Err(ApiError::new(429, "Mutation rate limit exceeded", "rate_limited"));
        }
        recent.push_back(now);
        Ok(())
    }
}
```

- [ ] **Step 4: Implement `control.rs`**

Above its test module:

```rust
use crate::pb;
use crate::pb::fleet_control_client::FleetControlClient;
use async_trait::async_trait;
use std::time::Duration;
use tonic::transport::Channel;
use tonic::Status;

/// Matches the dashboard's 20 s FleetControl deadline.
const CALL_TIMEOUT: Duration = Duration::from_secs(20);

#[async_trait]
pub trait ControlPlane: Send + Sync {
    async fn spawn_agent(&self, request: pb::SpawnAgentRequest) -> Result<pb::SpawnAgentResponse, Status>;
    async fn import_session(&self, request: pb::ImportSessionRequest) -> Result<pb::SpawnAgentResponse, Status>;
    async fn import_session_history(
        &self,
        request: pb::ImportSessionHistoryRequest,
    ) -> Result<pb::ImportSessionHistoryResponse, Status>;
    async fn send_input(&self, request: pb::SendInputRequest) -> Result<pb::SendInputResponse, Status>;
    async fn stop_agent(&self, request: pb::StopAgentRequest) -> Result<pb::StopAgentResponse, Status>;
    async fn pause_agent(&self, request: pb::PauseAgentRequest) -> Result<pb::PauseAgentResponse, Status>;
    async fn resume_agent(&self, request: pb::ResumeAgentRequest) -> Result<pb::ResumeAgentResponse, Status>;
    async fn list_repositories(&self) -> Result<pb::ListRepositoriesResponse, Status>;
}

/// FleetControl over gRPC with the control credential.
pub struct GrpcControlPlane {
    client: FleetControlClient<Channel>,
    token: Option<String>,
}

impl GrpcControlPlane {
    /// A client that dials `addr` on first use, authenticating with `AGENT_CTRL_CONTROL_AUTH_TOKEN`.
    pub fn connect_lazy(addr: String) -> crate::Result<Self> {
        let token = crate::remote::control_client_token()?;
        let client = FleetControlClient::new(crate::remote::tls::lazy_channel(addr)?)
            .max_encoding_message_size(crate::remote::MAX_GRPC_MESSAGE_BYTES)
            .max_decoding_message_size(crate::remote::MAX_GRPC_MESSAGE_BYTES);
        Ok(Self { client, token })
    }

    fn authed<T>(&self, message: T) -> tonic::Request<T> {
        let mut request = tonic::Request::new(message);
        request.set_timeout(CALL_TIMEOUT);
        if let Some(token) = &self.token {
            let value = format!("Bearer {token}")
                .parse()
                .expect("control_client_token validated the credential");
            request.metadata_mut().insert("authorization", value);
        }
        request
    }
}

#[async_trait]
impl ControlPlane for GrpcControlPlane {
    async fn spawn_agent(&self, request: pb::SpawnAgentRequest) -> Result<pb::SpawnAgentResponse, Status> {
        Ok(self.client.clone().spawn_agent(self.authed(request)).await?.into_inner())
    }

    async fn import_session(&self, request: pb::ImportSessionRequest) -> Result<pb::SpawnAgentResponse, Status> {
        Ok(self.client.clone().import_session(self.authed(request)).await?.into_inner())
    }

    async fn import_session_history(
        &self,
        request: pb::ImportSessionHistoryRequest,
    ) -> Result<pb::ImportSessionHistoryResponse, Status> {
        Ok(self
            .client
            .clone()
            .import_session_history(self.authed(request))
            .await?
            .into_inner())
    }

    async fn send_input(&self, request: pb::SendInputRequest) -> Result<pb::SendInputResponse, Status> {
        Ok(self.client.clone().send_input(self.authed(request)).await?.into_inner())
    }

    async fn stop_agent(&self, request: pb::StopAgentRequest) -> Result<pb::StopAgentResponse, Status> {
        Ok(self.client.clone().stop_agent(self.authed(request)).await?.into_inner())
    }

    async fn pause_agent(&self, request: pb::PauseAgentRequest) -> Result<pb::PauseAgentResponse, Status> {
        Ok(self.client.clone().pause_agent(self.authed(request)).await?.into_inner())
    }

    async fn resume_agent(&self, request: pb::ResumeAgentRequest) -> Result<pb::ResumeAgentResponse, Status> {
        Ok(self.client.clone().resume_agent(self.authed(request)).await?.into_inner())
    }

    async fn list_repositories(&self) -> Result<pb::ListRepositoriesResponse, Status> {
        Ok(self
            .client
            .clone()
            .list_repositories(self.authed(pb::ListRepositoriesRequest {}))
            .await?
            .into_inner())
    }
}
```

- [ ] **Step 5: `FakeControl`**

Add to the `use` block of `src/api/fakes.rs`:

```rust
use super::control::ControlPlane;
use std::sync::atomic::AtomicBool;
use crate::pb;
use tonic::Status;
```

Append:

```rust
/// FleetControl scripted by request-id prefix, exactly like the dashboard's
/// `scripts/check-agent-control.mjs` fake.
#[derive(Default)]
pub struct FakeControl {
    pub spawns: Mutex<Vec<pb::SpawnAgentRequest>>,
    pub imports: Mutex<Vec<pb::ImportSessionRequest>>,
    pub calls: AtomicUsize,
    pub repositories_unavailable: AtomicBool,
}

struct Scripted {
    success: bool,
    error: String,
    replayed: bool,
}

impl FakeControl {
    fn script(&self, request_id: &str) -> Result<Scripted, Status> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let starts = |prefix: &str| request_id.starts_with(prefix);
        if starts("conflict") {
            return Err(Status::already_exists("request fingerprint mismatch"));
        }
        if starts("unavailable") {
            return Err(Status::unavailable("down"));
        }
        if starts("atcapacity") {
            return Err(Status::resource_exhausted("node at capacity"));
        }
        if starts("sessionunknown") {
            return Err(Status::failed_precondition("session id unknown"));
        }
        if starts("notstate") {
            return Err(Status::failed_precondition("job is not paused"));
        }
        if starts("notinteractive") {
            return Err(Status::failed_precondition(
                "agent does not permit operator interaction",
            ));
        }
        let rejected = starts("rejected");
        Ok(Scripted {
            success: !rejected,
            error: if rejected { "policy denied".into() } else { String::new() },
            replayed: starts("replay"),
        })
    }

    fn launched(scripted: Scripted, request_id: String) -> pb::SpawnAgentResponse {
        pb::SpawnAgentResponse {
            agent_id: if scripted.success { "agent-launched".into() } else { String::new() },
            node_name: String::new(),
            success: scripted.success,
            error: scripted.error,
            job_id: if scripted.success { "job-new".into() } else { String::new() },
            request_id,
            replayed: scripted.replayed,
        }
    }
}

#[async_trait]
impl ControlPlane for FakeControl {
    async fn spawn_agent(&self, request: pb::SpawnAgentRequest) -> Result<pb::SpawnAgentResponse, Status> {
        self.spawns.lock().unwrap().push(request.clone());
        let scripted = self.script(&request.request_id)?;
        Ok(Self::launched(scripted, request.request_id))
    }

    async fn import_session(&self, request: pb::ImportSessionRequest) -> Result<pb::SpawnAgentResponse, Status> {
        self.imports.lock().unwrap().push(request.clone());
        let scripted = self.script(&request.request_id)?;
        Ok(Self::launched(scripted, request.request_id))
    }

    async fn import_session_history(
        &self,
        request: pb::ImportSessionHistoryRequest,
    ) -> Result<pb::ImportSessionHistoryResponse, Status> {
        let scripted = self.script(&request.request_id)?;
        Ok(pb::ImportSessionHistoryResponse {
            job_id: request.job_id,
            history_event_count: if request.session_file.is_empty() { 0 } else { 2 },
            success: scripted.success,
            error: scripted.error,
            request_id: request.request_id,
            replayed: scripted.replayed,
        })
    }

    async fn send_input(&self, request: pb::SendInputRequest) -> Result<pb::SendInputResponse, Status> {
        let scripted = self.script(&request.request_id)?;
        Ok(pb::SendInputResponse {
            success: scripted.success,
            error: scripted.error,
            request_id: request.request_id,
            replayed: scripted.replayed,
        })
    }

    async fn stop_agent(&self, request: pb::StopAgentRequest) -> Result<pb::StopAgentResponse, Status> {
        let scripted = self.script(&request.request_id)?;
        Ok(pb::StopAgentResponse {
            success: scripted.success,
            error: scripted.error,
            request_id: request.request_id,
            replayed: scripted.replayed,
        })
    }

    async fn pause_agent(&self, request: pb::PauseAgentRequest) -> Result<pb::PauseAgentResponse, Status> {
        let scripted = self.script(&request.request_id)?;
        Ok(pb::PauseAgentResponse {
            job_id: "job-new".into(),
            state: "paused".into(),
            accepted: scripted.success,
            node_name: String::new(),
            error: scripted.error,
            request_id: request.request_id,
            replayed: scripted.replayed,
        })
    }

    async fn resume_agent(&self, request: pb::ResumeAgentRequest) -> Result<pb::ResumeAgentResponse, Status> {
        let scripted = self.script(&request.request_id)?;
        Ok(pb::ResumeAgentResponse {
            job_id: "job-new".into(),
            state: "running".into(),
            accepted: scripted.success,
            node_name: "agent-ctrl-node-1".into(),
            error: scripted.error,
            request_id: request.request_id,
            replayed: scripted.replayed,
            agent_id: "agent-launched".into(),
        })
    }

    async fn list_repositories(&self) -> Result<pb::ListRepositoriesResponse, Status> {
        if self.repositories_unavailable.load(Ordering::SeqCst) {
            return Err(Status::unavailable("down"));
        }
        Ok(pb::ListRepositoriesResponse {
            repositories: vec![pb::RepositoryInfo {
                name: "kaizen".into(),
                url: "https://gitlab.com/pseekoo/kaizen.git".into(),
                branch: "main".into(),
                reachable: true,
                error: String::new(),
            }],
        })
    }
}
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test --lib api::limits && cargo test --lib api::control && cargo test --lib api::`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
cargo fmt
git add src/api/mod.rs src/api/control.rs src/api/limits.rs src/api/fakes.rs
git commit -m "feat(api): put FleetControl behind a trait and add the mutation guards

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 9: Router, read routes, `agent-ctrl api` subcommand

**Files:**
- Create: `src/api/handlers/mod.rs`, `src/api/handlers/reads.rs`, `src/api/handlers/tests.rs`
- Modify: `src/api/mod.rs` (`AppState`, settings, `state_from_env`, `run_api`), `src/api/fakes.rs` (harness), `src/main.rs:27-33` (`Commands`), `:198` (dispatch), after `cmd_controller` (`:611`)

**Interfaces:**
- Consumes: everything from Tasks 4–8.
- Produces: `pub struct AppState { config: Arc<ApiConfig>, verifier: Arc<Verifier>, control: Arc<dyn ControlPlane>, store: Arc<dyn ReadModel>, limiter: Arc<RateLimiter> }` (`Clone`); `pub fn router(AppState) -> axum::Router` (re-exported as `crate::api::router`); `pub async fn state_from_env() -> anyhow::Result<AppState>`; `pub async fn run_api(SocketAddr, AppState, shutdown) -> anyhow::Result<()>`; `impl FromRequestParts<AppState> for Caller`; in `handlers`: `pub(super) enum Cache { NoStore, PrivateNoStore }`, `json_response(StatusCode, Value, Cache) -> Response`, `param(&[(String, String)], &str) -> Option<&str>`, `storage_error(StoreError) -> ApiError`, `load_run(&AppState, &Caller, job_id) -> Result<Document, ApiError>`, `method_not_allowed() -> ApiError`; in `handlers::reads`: `snapshot`, `snapshot_fleet_id`, `node_ready`, `MAX_LIMIT`, `REQUIRED_CONTROL_API_VERSION`, `overview`, `list_runs`, `session`, `events`, `history`, `commands`, `repos`. Test-only `fakes::{Harness { app, store, control }, harness(), harness_with(ApiConfig), api_config(), seed_fleet(&MemoryReadModel), get, post_json, send, send_text, ADMIN, ADA, BOB, admin_token(), member_token(sub)}`.

Routes (axum 0.7 path syntax). Per-run actions use a wildcard so the static `events/stream` suffix never competes with a parameter segment, and `POST /runs/import` is handled inside the `/runs/:job` route exactly as the dashboard matches it first:

| Route | Methods |
|---|---|
| `/healthz` | GET (no auth; k8s probes) |
| `/api/agent-control/overview` | GET |
| `/api/agent-control/runs` | GET list, POST launch (Task 10) |
| `/api/agent-control/repos`, `/api/agent-control/commands` | GET |
| `/api/agent-control/runs/:job` | GET run; POST when `:job == "import"` (Task 10) |
| `/api/agent-control/runs/:job/*rest` | `session`, `events`, `history` (GET/POST), `input`/`close`/`stop`/`pause`/`resume` (POST, Task 10), `events/stream` (Task 11) |
| anything else under `/api/agent-control` | 401 or 404 `not_found` |

- [ ] **Step 1: Test harness**

Add to the `use` block of `src/api/fakes.rs`:

```rust
use super::limits::RateLimiter;
use super::{ApiConfig, AppState};
use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use axum::Router;
```

Append:

```rust
pub const ADMIN: &str = "394000000000000000";
pub const ADA: &str = "394000000000000001";
pub const BOB: &str = "394000000000000002";

pub fn admin_token() -> String {
    token(ADMIN, &["agent-admin"])
}

pub fn member_token(sub: &str) -> String {
    token(sub, &["agent-user"])
}

pub fn api_config() -> ApiConfig {
    ApiConfig {
        environment: "staging".into(),
        fleet_id: "fleet-staging".into(),
        mutations_enabled: true,
        actor_domain: "example.test".into(),
    }
}

pub struct Harness {
    pub app: Router,
    pub store: Arc<MemoryReadModel>,
    pub control: Arc<FakeControl>,
}

pub fn harness_with(config: ApiConfig) -> Harness {
    let store = Arc::new(MemoryReadModel::default());
    let control = Arc::new(FakeControl::default());
    let state = AppState {
        config: Arc::new(config),
        verifier: Arc::new(verifier(
            FakeJwks::serving(vec![test_jwk()]),
            Arc::new(FakeProfiles::default()),
        )),
        control: control.clone(),
        store: store.clone(),
        limiter: Arc::new(RateLimiter::default()),
    };
    Harness {
        app: super::router(state),
        store,
        control,
    }
}

pub fn harness() -> Harness {
    harness_with(api_config())
}

/// A fresh controller heartbeat, one reconciled node and the interactive repository profile.
pub fn seed_fleet(store: &MemoryReadModel) {
    let now = BsonDateTime::now();
    store.insert(
        "fleet_snapshots",
        mongodb::bson::doc! {
            "_id": "fleet-staging", "environment": "staging", "control_api_version": 2_i64,
            "last_seen": now,
            "agents": [{
                "name": "staging-claude-interactive-rw", "backend": "claude", "visible": true,
                "launchable": true, "allow_interactive": true, "interactive_capable": true,
            }],
        },
    );
    store.insert(
        "nodes",
        mongodb::bson::doc! {
            "fleet_id": "fleet-staging", "node_id": "node-0", "node_name": "agent-ctrl-node-0",
            "connected": true, "reconciling": false, "last_seen": now,
        },
    );
}

pub fn get(path: &str, token: &str) -> Request<Body> {
    Request::builder()
        .uri(path)
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap()
}

pub fn post_json(path: &str, token: &str, key: Option<&str>, body: &Value) -> Request<Body> {
    let mut builder = Request::builder()
        .method("POST")
        .uri(path)
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json");
    if let Some(key) = key {
        builder = builder.header("idempotency-key", key);
    }
    builder.body(Body::from(body.to_string())).unwrap()
}

pub async fn send_text(app: &Router, request: Request<Body>) -> (StatusCode, HeaderMap, String) {
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    let response = app.clone().oneshot(request).await.expect("the router is infallible");
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response.into_body().collect().await.expect("body").to_bytes();
    (status, headers, String::from_utf8_lossy(&bytes).into_owned())
}

/// The response with its body parsed as JSON (a JSON string when it is not JSON).
pub async fn send(app: &Router, request: Request<Body>) -> (StatusCode, HeaderMap, Value) {
    let (status, headers, text) = send_text(app, request).await;
    let body = serde_json::from_str(&text).unwrap_or(Value::String(text));
    (status, headers, body)
}
```

- [ ] **Step 2: Write the failing tests**

Create `src/api/handlers/tests.rs`:

```rust
use crate::api::fakes::{self, admin_token, member_token, send, ADA, BOB};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use std::sync::atomic::Ordering;

fn seeded() -> fakes::Harness {
    let harness = fakes::harness();
    fakes::seed_fleet(&harness.store);
    harness.store.insert("jobs", fakes::job_doc("job-ada", Some(ADA), -1_000));
    harness.store.insert("jobs", fakes::job_doc("job-bob", Some(BOB), -2_000));
    harness.store.insert("jobs", fakes::job_doc("job-legacy", None, -3_000));
    harness
}

fn ids(body: &Value, key: &str) -> Vec<String> {
    body[key]
        .as_array()
        .unwrap_or_else(|| panic!("{key} in {body}"))
        .iter()
        .map(|run| run["job_id"].as_str().unwrap().to_string())
        .collect()
}

fn code(body: &Value) -> Option<&str> {
    body["code"].as_str()
}

#[tokio::test]
async fn unauthenticated_requests_get_401_everywhere() {
    let harness = seeded();
    for path in [
        "/api/agent-control/overview",
        "/api/agent-control/runs",
        "/api/agent-control/runs/job-ada",
        "/api/agent-control/runs/job-ada/events",
        "/api/agent-control/nope",
    ] {
        let request = Request::builder().uri(path).body(Body::empty()).unwrap();
        let (status, _, body) = send(&harness.app, request).await;
        assert_eq!((status, code(&body)), (StatusCode::UNAUTHORIZED, Some("unauthenticated")), "{path}");
    }
}

#[tokio::test]
async fn health_needs_no_token() {
    let harness = fakes::harness();
    let request = Request::builder().uri("/healthz").body(Body::empty()).unwrap();
    let (status, _, body) = send(&harness.app, request).await;
    assert_eq!((status, body), (StatusCode::OK, json!("ok")));
}

#[tokio::test]
async fn members_list_only_their_own_runs_and_admins_list_all() {
    let harness = seeded();
    let (status, headers, body) =
        send(&harness.app, fakes::get("/api/agent-control/runs", &member_token(ADA))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers["cache-control"], "no-store");
    assert_eq!(ids(&body, "items"), ["job-ada"]);
    assert_eq!(body["items"][0]["owner_email"], format!("{ADA}@example.test"));
    let (_, _, body) = send(&harness.app, fakes::get("/api/agent-control/runs", &admin_token())).await;
    assert_eq!(ids(&body, "items"), ["job-ada", "job-bob", "job-legacy"]);
}

#[tokio::test]
async fn run_pages_follow_the_cursor() {
    let harness = seeded();
    let (_, _, first) =
        send(&harness.app, fakes::get("/api/agent-control/runs?limit=2", &admin_token())).await;
    assert_eq!(ids(&first, "items"), ["job-ada", "job-bob"]);
    let cursor = first["next_cursor"].as_str().expect("a second page").to_string();
    let path = format!("/api/agent-control/runs?limit=2&cursor={cursor}");
    let (_, _, second) = send(&harness.app, fakes::get(&path, &admin_token())).await;
    assert_eq!(ids(&second, "items"), ["job-legacy"]);
    assert_eq!(second["next_cursor"], Value::Null);
}

#[tokio::test]
async fn invalid_list_queries_are_rejected_like_the_dashboard() {
    let harness = seeded();
    let admin = admin_token();
    let (status, _, body) =
        send(&harness.app, fakes::get("/api/agent-control/runs?state=bogus", &admin)).await;
    assert_eq!((status, body["error"].as_str()), (StatusCode::UNPROCESSABLE_ENTITY, Some("Invalid run state")));
    let (status, _, body) =
        send(&harness.app, fakes::get("/api/agent-control/runs?limit=0", &admin)).await;
    assert_eq!((status, code(&body)), (StatusCode::UNPROCESSABLE_ENTITY, Some("validation_error")));
    let (status, _, body) =
        send(&harness.app, fakes::get("/api/agent-control/runs?cursor=nope", &admin)).await;
    assert_eq!((status, code(&body)), (StatusCode::BAD_REQUEST, Some("invalid_cursor")));
}

#[tokio::test]
async fn members_get_404_for_runs_they_do_not_own() {
    let harness = seeded();
    let ada = member_token(ADA);
    for path in [
        "/api/agent-control/runs/job-bob",
        "/api/agent-control/runs/job-legacy",
        "/api/agent-control/runs/job-bob/session",
        "/api/agent-control/runs/job-bob/events",
        "/api/agent-control/runs/job-legacy/history",
    ] {
        let (status, _, body) = send(&harness.app, fakes::get(path, &ada)).await;
        assert_eq!((status, body["error"].as_str()), (StatusCode::NOT_FOUND, Some("Run not found")), "{path}");
    }
    let (status, _, body) = send(&harness.app, fakes::get("/api/agent-control/runs/job-ada", &ada)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["job_id"], "job-ada");
    let (status, _, _) =
        send(&harness.app, fakes::get("/api/agent-control/runs/job-legacy", &admin_token())).await;
    assert_eq!(status, StatusCode::OK, "ownerless runs stay visible to admins");
}

#[tokio::test]
async fn malformed_ids_unknown_actions_and_methods_match_the_dashboard() {
    let harness = seeded();
    let admin = admin_token();
    let (status, _, body) =
        send(&harness.app, fakes::get("/api/agent-control/runs/not-a-job", &admin)).await;
    assert_eq!((status, code(&body)), (StatusCode::BAD_REQUEST, Some("invalid_job_id")));
    let (status, _, body) =
        send(&harness.app, fakes::get("/api/agent-control/runs/job-ada/bogus", &admin)).await;
    assert_eq!((status, code(&body)), (StatusCode::NOT_FOUND, Some("not_found")));
    let delete = Request::builder()
        .method("DELETE")
        .uri("/api/agent-control/runs/job-ada")
        .header("authorization", format!("Bearer {admin}"))
        .body(Body::empty())
        .unwrap();
    let (status, _, body) = send(&harness.app, delete).await;
    assert_eq!((status, code(&body)), (StatusCode::METHOD_NOT_ALLOWED, Some("method_not_allowed")));
}

#[tokio::test]
async fn the_session_descriptor_lists_the_run_capabilities() {
    let harness = seeded();
    let (status, headers, body) = send(
        &harness.app,
        fakes::get("/api/agent-control/runs/job-ada/session", &member_token(ADA)),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers["cache-control"], "private, no-store");
    assert_eq!(body["fleet_id"], "fleet-staging");
    assert_eq!(body["environment"], "staging");
    assert_eq!(body["capabilities"], json!(["watch", "input", "close", "stop", "pause"]));
}

#[tokio::test]
async fn events_page_by_cursor_and_report_retention_gaps() {
    let harness = seeded();
    for sequence in [3_i64, 4] {
        harness.store.insert(
            "job_events",
            mongodb::bson::doc! {
                "_id": format!("event-{sequence}"), "fleet_id": "fleet-staging", "job_id": "job-ada",
                "agent_id": "agent-job-ada", "sequence": sequence, "type": "assistant",
                "content": format!("line {sequence}"),
            },
        );
    }
    let ada = member_token(ADA);
    let path = "/api/agent-control/runs/job-ada/events?after=djE6Mg";
    let (status, _, body) = send(&harness.app, fakes::get(path, &ada)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["items"].as_array().unwrap().len(), 2);
    assert_eq!(body["next_cursor"], "djE6NA");
    assert_eq!(body["gap"], false);
    let path = "/api/agent-control/runs/job-ada/events?after=djE6MQ";
    let (status, _, body) = send(&harness.app, fakes::get(path, &ada)).await;
    assert_eq!(status, StatusCode::GONE);
    assert_eq!(body["earliest_cursor"], "djE6Mw");
    assert_eq!(body["items"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn the_overview_shows_members_only_their_runs() {
    let harness = seeded();
    let (status, _, body) =
        send(&harness.app, fakes::get("/api/agent-control/overview", &member_token(ADA))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["controller_stale"], false);
    assert_eq!(body["mutation_available"], true);
    assert_eq!(ids(&body, "recent_runs"), ["job-ada"]);
    assert_eq!(ids(&body, "active_runs"), ["job-ada"]);
}

#[tokio::test]
async fn members_see_only_their_own_commands() {
    let harness = seeded();
    let now = mongodb::bson::DateTime::now();
    for (request_id, sub) in [("req-ada-000000001", ADA), ("req-bob-000000001", BOB)] {
        harness.store.insert(
            "control_commands",
            mongodb::bson::doc! {
                "fleet_id": "fleet-staging", "request_id": request_id,
                "actor": format!("@{sub}:example.test"), "source": "agentctrl_api",
                "action": "launch", "status": "accepted", "created_at": now,
            },
        );
    }
    let (_, _, body) =
        send(&harness.app, fakes::get("/api/agent-control/commands", &member_token(ADA))).await;
    let actors: Vec<&str> = body["items"].as_array().unwrap().iter().map(|c| c["actor"].as_str().unwrap()).collect();
    assert_eq!(actors, [format!("@{ADA}:example.test")]);
    let (_, _, body) = send(&harness.app, fakes::get("/api/agent-control/commands", &admin_token())).await;
    assert_eq!(body["items"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn read_model_failures_are_503_storage_unavailable() {
    let harness = seeded();
    harness.store.fail("*");
    let (status, _, body) = send(&harness.app, fakes::get("/api/agent-control/runs", &admin_token())).await;
    assert_eq!(
        (status, code(&body), body["error"].as_str()),
        (StatusCode::SERVICE_UNAVAILABLE, Some("storage_unavailable"), Some("Service unavailable"))
    );
}

#[tokio::test]
async fn repositories_come_from_fleet_control() {
    let harness = seeded();
    let (status, _, body) =
        send(&harness.app, fakes::get("/api/agent-control/repos", &member_token(ADA))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["items"][0]["name"], "kaizen");
    harness.control.repositories_unavailable.store(true, Ordering::SeqCst);
    let (status, _, body) =
        send(&harness.app, fakes::get("/api/agent-control/repos", &member_token(ADA))).await;
    assert_eq!((status, code(&body)), (StatusCode::SERVICE_UNAVAILABLE, Some("controller_unavailable")));
}
```

Add to the bottom of `src/api/mod.rs` a settings test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    const REQUIRED: [(&str, &str); 4] = [
        ("AGENT_PLATFORM_ISSUER", "https://id-dev.obachan.dev/"),
        ("AGENT_PLATFORM_PROJECT_ID", "394361582064240430"),
        ("AGENT_CTRL_ENVIRONMENT", "staging"),
        ("AGENT_CTRL_MONGO_URI", "mongodb://mongo:27017"),
    ];

    fn settings(pairs: &[(&str, &str)]) -> anyhow::Result<Settings> {
        let vars: HashMap<String, String> = pairs
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect();
        settings_from(&|key| vars.get(key).cloned())
    }

    #[test]
    fn settings_come_from_the_environment_with_safe_defaults() {
        let defaults = settings(&REQUIRED).unwrap();
        assert_eq!(defaults.zitadel.issuer, "https://id-dev.obachan.dev");
        assert_eq!(defaults.config.actor_domain, "id-dev.obachan.dev");
        assert!(!defaults.config.mutations_enabled);
        assert_eq!(defaults.config.fleet_filter(), None);
        assert_eq!(defaults.mongo_db, "claude_hooks");
        assert_eq!(defaults.controller, "http://127.0.0.1:50051");

        let mut pairs = REQUIRED.to_vec();
        pairs.extend([
            ("AGENT_CTRL_MANAGED_MUTATIONS_ENABLED", "true"),
            ("AGENT_CTRL_FLEET_ID", "stoz3n-agent-ctrl-staging"),
            ("AGENTCTRL_API_ACTOR_DOMAIN", "stozn.ai"),
        ]);
        let configured = settings(&pairs).unwrap();
        assert!(configured.config.mutations_enabled);
        assert_eq!(configured.config.fleet_filter(), Some("stoz3n-agent-ctrl-staging"));
        assert_eq!(configured.config.actor_domain, "stozn.ai");
    }

    #[test]
    fn missing_identity_or_storage_settings_fail_startup() {
        for missing in [
            "AGENT_PLATFORM_ISSUER",
            "AGENT_PLATFORM_PROJECT_ID",
            "AGENT_CTRL_ENVIRONMENT",
            "AGENT_CTRL_MONGO_URI",
        ] {
            let pairs: Vec<(&str, &str)> =
                REQUIRED.iter().copied().filter(|(key, _)| *key != missing).collect();
            let error = settings(&pairs).err().expect("startup refused").to_string();
            assert!(error.contains(missing), "{error}");
        }
    }
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test --lib api::handlers` and `cargo test --lib api::tests`
Expected: compile errors (`handlers`, `AppState`, `settings_from` missing).

- [ ] **Step 4: `AppState`, settings and server in `src/api/mod.rs`**

Add after the module declarations (`pub mod handlers;` with them):

```rust
pub mod handlers;

pub use handlers::router;

use auth::{HttpZitadel, Verifier, ZitadelSettings};
use control::{ControlPlane, GrpcControlPlane};
use limits::RateLimiter;
use std::sync::Arc;
use store::{MongoReadModel, ReadModel};

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<ApiConfig>,
    pub verifier: Arc<Verifier>,
    pub control: Arc<dyn ControlPlane>,
    pub store: Arc<dyn ReadModel>,
    pub limiter: Arc<RateLimiter>,
}

struct Settings {
    config: ApiConfig,
    zitadel: ZitadelSettings,
    mongo_uri: String,
    mongo_db: String,
    mongo_tls: crate::mongo::MongoTlsSettings,
    controller: String,
}

fn settings_from(var: &dyn Fn(&str) -> Option<String>) -> anyhow::Result<Settings> {
    use anyhow::Context as _;
    let get = |key: &str| var(key).map(|value| value.trim().to_string()).filter(|value| !value.is_empty());
    let flag = |key: &str| get(key).is_some_and(|value| matches!(value.to_ascii_lowercase().as_str(), "1" | "true" | "yes"));
    let issuer = get("AGENT_PLATFORM_ISSUER")
        .context("AGENT_PLATFORM_ISSUER is required")?
        .trim_end_matches('/')
        .to_string();
    let project_id = get("AGENT_PLATFORM_PROJECT_ID").context("AGENT_PLATFORM_PROJECT_ID is required")?;
    let environment = get("AGENT_CTRL_ENVIRONMENT").context("AGENT_CTRL_ENVIRONMENT is required")?;
    let mongo_uri = get("AGENT_CTRL_MONGO_URI")
        .or_else(|| get("MONGODB_URI"))
        .context("AGENT_CTRL_MONGO_URI is required")?;
    let actor_domain = match get("AGENTCTRL_API_ACTOR_DOMAIN") {
        Some(domain) => domain,
        None => reqwest::Url::parse(&issuer)
            .ok()
            .and_then(|url| url.host_str().map(str::to_string))
            .context("AGENT_PLATFORM_ISSUER must be an absolute URL")?,
    };
    Ok(Settings {
        config: ApiConfig {
            environment,
            fleet_id: get("AGENT_CTRL_FLEET_ID").unwrap_or_default(),
            mutations_enabled: flag("AGENT_CTRL_MANAGED_MUTATIONS_ENABLED"),
            actor_domain,
        },
        zitadel: ZitadelSettings { issuer, project_id },
        mongo_uri,
        mongo_db: get("AGENT_CTRL_MONGO_DB").unwrap_or_else(|| "claude_hooks".to_string()),
        mongo_tls: crate::mongo::MongoTlsSettings {
            ca_file: get("AGENT_CTRL_MONGO_TLS_CA_FILE").map(std::path::PathBuf::from),
            cert_key_file: get("AGENT_CTRL_MONGO_TLS_CERT_KEY_FILE").map(std::path::PathBuf::from),
            insecure: flag("AGENT_CTRL_MONGO_TLS_INSECURE"),
        },
        controller: get("AGENT_CTRL_CONTROLLER")
            .unwrap_or_else(|| crate::remote::DEFAULT_CONTROLLER.to_string()),
    })
}

/// Builds the API from its environment. Required: `AGENT_PLATFORM_ISSUER`,
/// `AGENT_PLATFORM_PROJECT_ID`, `AGENT_CTRL_ENVIRONMENT`, `AGENT_CTRL_MONGO_URI` (or
/// `MONGODB_URI`). Optional: `AGENT_CTRL_FLEET_ID`, `AGENT_CTRL_MANAGED_MUTATIONS_ENABLED`,
/// `AGENT_CTRL_CONTROLLER`, `AGENTCTRL_API_ACTOR_DOMAIN` (default: the issuer host),
/// `AGENT_CTRL_MONGO_DB`, `AGENT_CTRL_MONGO_TLS_*`, and the FleetControl credential
/// `AGENT_CTRL_CONTROL_AUTH_TOKEN`. Neither Mongo nor the controller has to be up at startup.
pub async fn state_from_env() -> anyhow::Result<AppState> {
    let settings = settings_from(&|key| std::env::var(key).ok())?;
    let database = crate::mongo::MongoStore::read_only_database(
        &settings.mongo_uri,
        &settings.mongo_db,
        &settings.mongo_tls,
    )
    .await?;
    let control = GrpcControlPlane::connect_lazy(settings.controller)?;
    let zitadel = Arc::new(HttpZitadel::new());
    let actor_domain = settings.config.actor_domain.clone();
    Ok(AppState {
        config: Arc::new(settings.config),
        verifier: Arc::new(Verifier::new(
            settings.zitadel,
            zitadel.clone(),
            zitadel,
            actor_domain,
        )),
        control: Arc::new(control),
        store: Arc::new(MongoReadModel::new(database)),
        limiter: Arc::new(RateLimiter::default()),
    })
}

pub async fn run_api(
    addr: std::net::SocketAddr,
    state: AppState,
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
) -> anyhow::Result<()> {
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, "agent-ctrl api listening");
    axum::serve(listener, router(state))
        .with_graceful_shutdown(shutdown)
        .await?;
    Ok(())
}
```

- [ ] **Step 5: `src/api/handlers/mod.rs`**

```rust
//! HTTP routes. Every `/api/agent-control/*` route authenticates the bearer first, then applies
//! the dashboard's checks in the dashboard's order.

mod reads;
#[cfg(test)]
mod tests;

use super::auth::Caller;
use super::error::ApiError;
use super::limits::MAX_SESSION_FILE_BYTES;
use super::ownership::can_see;
use super::project;
use super::store::StoreError;
use super::AppState;
use axum::extract::{DefaultBodyLimit, FromRequestParts, Path, Query, Request, State};
use axum::http::request::Parts;
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get};
use axum::{Json, Router};
use mongodb::bson::Document;
use regex::Regex;
use serde_json::Value;
use std::sync::LazyLock;

/// Multipart framing allowance on top of a maximal session file.
const UPLOAD_OVERHEAD_BYTES: usize = 1024 * 1024;
const RUN_ACTIONS: [&str; 8] =
    ["session", "events", "history", "input", "close", "stop", "pause", "resume"];
static JOB_ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^job-[A-Za-z0-9-]{1,124}$").expect("valid job id pattern"));

type Params = Query<Vec<(String, String)>>;

#[axum::async_trait]
impl FromRequestParts<AppState> for Caller {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Self::Rejection> {
        state.verifier.authenticate(&parts.headers).await
    }
}

pub fn router(state: AppState) -> Router {
    let uploads = DefaultBodyLimit::max(MAX_SESSION_FILE_BYTES + UPLOAD_OVERHEAD_BYTES);
    Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/api/agent-control/overview", any(overview))
        .route("/api/agent-control/runs", any(runs))
        .route("/api/agent-control/repos", any(repos))
        .route("/api/agent-control/commands", any(commands))
        .route("/api/agent-control/runs/:job", any(run).layer(uploads))
        .route("/api/agent-control/runs/:job/*rest", any(run_action).layer(uploads))
        .fallback(fallback)
        .with_state(state)
}

#[derive(Clone, Copy)]
pub(super) enum Cache {
    NoStore,
    PrivateNoStore,
}

pub(super) fn json_response(status: StatusCode, body: Value, cache: Cache) -> Response {
    let mut response = (status, Json(body)).into_response();
    let value = match cache {
        Cache::NoStore => "no-store",
        Cache::PrivateNoStore => "private, no-store",
    };
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static(value));
    response
}

/// The first value of query parameter `key`, like `URLSearchParams.get`.
pub(super) fn param<'a>(params: &'a [(String, String)], key: &str) -> Option<&'a str> {
    params
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.as_str())
}

pub(super) fn storage_error(error: StoreError) -> ApiError {
    tracing::warn!(%error, "read model request failed");
    ApiError::storage()
}

pub(super) fn method_not_allowed() -> ApiError {
    ApiError::new(405, "Method not allowed", "method_not_allowed")
}

/// The run `job_id` names if the caller may see it: 400 for a malformed id, 404 otherwise, so a
/// member cannot tell another person's run from a missing one.
pub(super) async fn load_run(state: &AppState, caller: &Caller, job_id: &str) -> Result<Document, ApiError> {
    if !JOB_ID.is_match(job_id) {
        return Err(ApiError::new(400, "Invalid job id", "invalid_job_id"));
    }
    let job = state
        .store
        .job(&state.config.environment, state.config.fleet_filter(), job_id)
        .await
        .map_err(storage_error)?;
    job.filter(|job| can_see(caller, job))
        .ok_or_else(ApiError::run_not_found)
}

async fn overview(State(state): State<AppState>, caller: Caller, method: Method) -> Result<Response, ApiError> {
    if method != Method::GET {
        return Err(ApiError::not_found());
    }
    reads::overview(&state, &caller).await
}

async fn runs(
    State(state): State<AppState>,
    caller: Caller,
    Query(params): Params,
    request: Request,
) -> Result<Response, ApiError> {
    let method = request.method().clone();
    if method == Method::GET {
        reads::list_runs(&state, &caller, &params).await
    } else if method == Method::POST {
        Err(method_not_allowed())
    } else {
        Err(ApiError::not_found())
    }
}

async fn repos(State(state): State<AppState>, _caller: Caller, method: Method) -> Result<Response, ApiError> {
    // Extracting `_caller` authenticates the request; repositories are not per-owner.
    if method != Method::GET {
        return Err(ApiError::not_found());
    }
    reads::repos(&state).await
}

async fn commands(
    State(state): State<AppState>,
    caller: Caller,
    method: Method,
    Query(params): Params,
) -> Result<Response, ApiError> {
    if method != Method::GET {
        return Err(ApiError::not_found());
    }
    reads::commands(&state, &caller, &params).await
}

async fn run(
    State(state): State<AppState>,
    caller: Caller,
    Path(job_id): Path<String>,
    request: Request,
) -> Result<Response, ApiError> {
    let method = request.method().clone();
    let job = load_run(&state, &caller, &job_id).await?;
    let snapshot = reads::snapshot(&state).await?;
    if method == Method::GET {
        return Ok(json_response(
            StatusCode::OK,
            project::public_run(&job, snapshot.as_ref()),
            Cache::NoStore,
        ));
    }
    Err(method_not_allowed())
}

async fn run_action(
    State(state): State<AppState>,
    caller: Caller,
    Path((job_id, rest)): Path<(String, String)>,
    Query(params): Params,
    request: Request,
) -> Result<Response, ApiError> {
    let action = rest.trim_start_matches('/');
    if !RUN_ACTIONS.contains(&action) {
        return Err(ApiError::not_found());
    }
    let job = load_run(&state, &caller, &job_id).await?;
    let snapshot = reads::snapshot(&state).await?;
    match (request.method(), action) {
        (&Method::GET, "session") => reads::session(&state, &job, snapshot.as_ref()),
        (&Method::GET, "events") => reads::events(&state, &job, &params).await,
        (&Method::GET, "history") => reads::history(&state, &job, &params).await,
        _ => Err(method_not_allowed()),
    }
}

async fn fallback(State(state): State<AppState>, uri: Uri, headers: HeaderMap) -> Response {
    let path = uri.path();
    if path == "/api/agent-control" || path.starts_with("/api/agent-control/") {
        return match state.verifier.authenticate(&headers).await {
            Ok(_) => ApiError::not_found().into_response(),
            Err(error) => error.into_response(),
        };
    }
    (StatusCode::NOT_FOUND, "Not found").into_response()
}
```

- [ ] **Step 6: `src/api/handlers/reads.rs`**

```rust
//! Read-only routes, ported from the dashboard's `fleetOverview`, `listRuns`, the session
//! descriptor, `listEvents`, `listHistory`, `listCommands` and `listRepositories`, with member
//! scoping.

use super::{json_response, param, storage_error, Cache};
use crate::api::auth::Caller;
use crate::api::cursor::{decode_event_cursor, decode_page_cursor, encode_event_cursor, encode_page_cursor};
use crate::api::error::ApiError;
use crate::api::limits::bounded_integer;
use crate::api::ownership::{command_actor_filter, owner_filter};
use crate::api::project::{
    self, integer, is_terminal, millis, nullable, string_or, ALLOWED_JOB_STATES,
    CONTROLLER_STALE_MS, NODE_STALE_MS,
};
use crate::api::store::{CommandQuery, JobOrder, JobQuery};
use crate::api::AppState;
use axum::http::StatusCode;
use axum::response::Response;
use chrono::Utc;
use mongodb::bson::{Bson, Document};
use serde_json::{json, Value};
use std::time::Duration;
use tokio::time::Instant;

pub(super) const MAX_LIMIT: i64 = 200;
pub(super) const REQUIRED_CONTROL_API_VERSION: i64 = 2;
const SESSION_PROTOCOL_VERSION: i64 = 1;
const EVENT_POLL: Duration = Duration::from_millis(250);

pub(super) async fn snapshot(state: &AppState) -> Result<Option<Document>, ApiError> {
    state
        .store
        .fleet_snapshot(&state.config.environment, state.config.fleet_filter())
        .await
        .map_err(storage_error)
}

/// JS `String(snapshot._id ?? snapshot.fleet_id ?? "")`.
pub(super) fn snapshot_fleet_id(snapshot: &Document) -> String {
    match snapshot.get("_id") {
        Some(id) if !matches!(id, Bson::Null) => project::bson_string(Some(id), ""),
        _ => string_or(snapshot, "fleet_id", ""),
    }
}

pub(super) fn node_ready(node: &Document, now_ms: i64) -> bool {
    node.get_bool("connected") == Ok(true)
        && node.get_bool("reconciling") != Ok(true)
        && !project::is_stale(node.get("last_seen"), NODE_STALE_MS, now_ms)
}

fn page_of(rows: &[Document], limit: i64) -> &[Document] {
    &rows[..rows.len().min(limit.max(0) as usize)]
}

pub(super) async fn overview(state: &AppState, caller: &Caller) -> Result<Response, ApiError> {
    let snapshot = snapshot(state).await?.ok_or_else(ApiError::storage)?;
    let fleet_id = snapshot_fleet_id(&snapshot);
    let now = Utc::now().timestamp_millis();
    let controller_stale =
        millis(snapshot.get("last_seen")).is_none_or(|seen| now - seen > CONTROLLER_STALE_MS);
    let base = JobQuery {
        fleet_id: Some(fleet_id.clone()),
        environment: state.config.environment.clone(),
        owner_sub: owner_filter(caller),
        ..JobQuery::default()
    };
    let recent_query = JobQuery { limit: 100, ..base.clone() };
    let paused_query = JobQuery {
        state: Some("paused".into()),
        order: JobOrder::Paused,
        limit: MAX_LIMIT,
        ..base
    };
    let (nodes, jobs, paused) = tokio::try_join!(
        state.store.nodes(&fleet_id),
        state.store.jobs(&recent_query),
        state.store.jobs(&paused_query),
    )
    .map_err(storage_error)?;
    let recent: Vec<Value> = jobs.iter().map(|job| project::public_run(job, Some(&snapshot))).collect();
    let connected = nodes.iter().any(|node| node_ready(node, now));
    let version = integer(snapshot.get("control_api_version"), 0);
    let compatible = version >= REQUIRED_CONTROL_API_VERSION;
    let available = state.config.mutations_enabled && !controller_stale && connected && compatible;
    let reason = if available {
        Value::Null
    } else if !state.config.mutations_enabled {
        json!("Managed mutations are disabled by rollout policy")
    } else if controller_stale {
        json!("Controller heartbeat is stale")
    } else if !connected {
        json!("No execution node is connected")
    } else {
        json!("Controller does not support task-bound managed sessions")
    };
    let active: Vec<Value> = recent
        .iter()
        .filter(|run| {
            let state = run["state"].as_str().unwrap_or_default();
            !is_terminal(state) && state != "paused"
        })
        .cloned()
        .collect();
    let environment = match snapshot.get("environment") {
        Some(Bson::String(environment)) => environment.clone(),
        _ => state.config.environment.clone(),
    };
    Ok(json_response(
        StatusCode::OK,
        json!({
            "fleet_id": fleet_id,
            "environment": environment,
            "controller_version": nullable(&snapshot, "controller_version"),
            "control_api_version": version,
            "controller_last_seen": project::iso(snapshot.get("last_seen")),
            "controller_stale": controller_stale,
            "mutation_available": available,
            "mutation_unavailable_reason": reason,
            "agents": project::safe_agents(snapshot.get("agents")),
            "nodes": nodes.iter().map(|node| project::public_node(node, now)).collect::<Vec<_>>(),
            "active_runs": active,
            "paused_runs": paused.iter().map(|job| project::public_run(job, Some(&snapshot))).collect::<Vec<_>>(),
            "recent_runs": recent,
            "refreshed_at": project::iso_now(),
        }),
        Cache::NoStore,
    ))
}

pub(super) async fn list_runs(
    state: &AppState,
    caller: &Caller,
    params: &[(String, String)],
) -> Result<Response, ApiError> {
    let snapshot = snapshot(state).await?;
    let limit = bounded_integer(param(params, "limit"), 50, 1, MAX_LIMIT)?;
    let mut query = JobQuery {
        fleet_id: state.config.fleet_filter().map(str::to_string),
        environment: state.config.environment.clone(),
        owner_sub: owner_filter(caller),
        limit: limit + 1,
        ..JobQuery::default()
    };
    if let Some(job_state) = param(params, "state").filter(|value| !value.is_empty()) {
        if !ALLOWED_JOB_STATES.contains(&job_state) {
            return Err(ApiError::new(422, "Invalid run state", "validation_error"));
        }
        query.state = Some(job_state.to_string());
    }
    for (name, slot) in [
        ("agent_name", &mut query.agent_name),
        ("node_name", &mut query.node),
        ("task_id", &mut query.task_id),
    ] {
        if let Some(value) = param(params, name).filter(|value| !value.is_empty()) {
            if value.encode_utf16().count() > 128 {
                return Err(ApiError::new(422, format!("{name} is too long"), "validation_error"));
            }
            *slot = Some(value.to_string());
        }
    }
    if let Some(cursor) = param(params, "cursor").filter(|value| !value.is_empty()) {
        query.before = Some(decode_page_cursor(cursor)?);
    }
    let rows = state.store.jobs(&query).await.map_err(storage_error)?;
    let page = page_of(&rows, limit);
    let next_cursor = if rows.len() as i64 > limit {
        page.last().map(|last| {
            encode_page_cursor(
                millis(last.get("created_at")).unwrap_or(0),
                &string_or(last, "job_id", ""),
            )
        })
    } else {
        None
    };
    Ok(json_response(
        StatusCode::OK,
        json!({
            "items": page.iter().map(|job| project::public_run(job, snapshot.as_ref())).collect::<Vec<_>>(),
            "next_cursor": next_cursor,
        }),
        Cache::NoStore,
    ))
}

pub(super) fn session(
    state: &AppState,
    job: &Document,
    snapshot: Option<&Document>,
) -> Result<Response, ApiError> {
    let protocol = integer(job.get("session_protocol_version"), 1);
    let environment = match job.get("environment") {
        None | Some(Bson::Null) => state.config.environment.clone(),
        value => project::bson_string(value, ""),
    };
    if protocol != SESSION_PROTOCOL_VERSION || environment != state.config.environment {
        return Err(ApiError::new(
            409,
            "Managed-session protocol or environment is incompatible",
            "incompatible_session",
        ));
    }
    let run = project::public_run(job, snapshot);
    let mut capabilities = vec!["watch"];
    if run["input_allowed"] == true {
        capabilities.extend(["input", "close"]);
    }
    if !is_terminal(run["state"].as_str().unwrap_or_default()) {
        capabilities.push("stop");
    }
    if run["pause_allowed"] == true {
        capabilities.push("pause");
    }
    if run["resume_allowed"] == true {
        capabilities.push("resume");
    }
    let last_sequence = integer(job.get("last_event_sequence"), 0).max(0) as u64;
    Ok(json_response(
        StatusCode::OK,
        json!({
            "protocol_version": protocol,
            "environment": state.config.environment,
            "fleet_id": string_or(job, "fleet_id", "undefined"),
            "job_id": string_or(job, "job_id", ""),
            "display_name": run["display_name"],
            "agent_name": string_or(job, "agent_name", ""),
            "state": run["state"],
            "live_state": run["live_state"],
            "capabilities": capabilities,
            "next_cursor": encode_event_cursor(last_sequence),
            "task": {
                "id": nullable(job, "task_id"),
                "ref": nullable(job, "task_ref"),
                "title": nullable(job, "task_title"),
                "room_id": nullable(job, "task_room_id"),
            },
            "poll_after_ms": 1000,
            "max_wait_ms": 15000,
            "refreshed_at": project::iso_now(),
        }),
        Cache::PrivateNoStore,
    ))
}

pub(super) async fn events(
    state: &AppState,
    job: &Document,
    params: &[(String, String)],
) -> Result<Response, ApiError> {
    let limit = bounded_integer(param(params, "limit"), 100, 1, MAX_LIMIT)?;
    let wait_ms = bounded_integer(param(params, "wait_ms"), 0, 0, 15_000)?;
    let requested = decode_event_cursor(param(params, "after"))?;
    let fleet_id = string_or(job, "fleet_id", "undefined");
    let job_id = string_or(job, "job_id", "");
    let earliest = state
        .store
        .earliest_event_sequence(&fleet_id, &job_id)
        .await
        .map_err(storage_error)?
        .unwrap_or(0);
    let retention_gap = earliest > requested + 1;
    let after = if retention_gap { earliest - 1 } else { requested };
    let terminal = is_terminal(&string_or(job, "state", ""));
    let deadline = Instant::now() + Duration::from_millis(wait_ms as u64);
    let rows = loop {
        let rows = state
            .store
            .events_after(&fleet_id, &job_id, after, limit)
            .await
            .map_err(storage_error)?;
        let now = Instant::now();
        if !rows.is_empty() || now >= deadline || terminal {
            break rows;
        }
        tokio::time::sleep((deadline - now).min(EVENT_POLL)).await;
    };
    let mut expected = after + 1;
    let mut first_missing = 0;
    for row in &rows {
        let sequence = integer(row.get("sequence"), 0).max(0) as u64;
        if first_missing == 0 && sequence > expected {
            first_missing = sequence;
        }
        expected = expected.max(sequence + 1);
    }
    let gap = retention_gap || first_missing > 0;
    let gap_start = if retention_gap { earliest } else { first_missing };
    let last = rows
        .last()
        .map(|row| integer(row.get("sequence"), after as i64).max(0) as u64)
        .unwrap_or(after);
    let earliest_cursor = if gap {
        json!(encode_event_cursor(gap_start))
    } else {
        Value::Null
    };
    Ok(json_response(
        if gap { StatusCode::GONE } else { StatusCode::OK },
        json!({
            "items": rows.iter().map(project::public_event).collect::<Vec<_>>(),
            "next_cursor": encode_event_cursor(last),
            "terminal": terminal,
            "gap": gap,
            "earliest_cursor": earliest_cursor,
            "refreshed_at": project::iso_now(),
        }),
        Cache::PrivateNoStore,
    ))
}

pub(super) async fn history(
    state: &AppState,
    job: &Document,
    params: &[(String, String)],
) -> Result<Response, ApiError> {
    let limit = bounded_integer(param(params, "limit"), MAX_LIMIT, 1, MAX_LIMIT)?;
    let after = decode_event_cursor(param(params, "after"))?;
    let rows = state
        .store
        .history_after(
            &string_or(job, "fleet_id", "undefined"),
            &string_or(job, "job_id", ""),
            after,
            limit + 1,
        )
        .await
        .map_err(storage_error)?;
    let page = page_of(&rows, limit);
    let next_cursor = if rows.len() as i64 > limit {
        page.last().and_then(|last| {
            encode_event_cursor(integer(last.get("position"), after as i64).max(0) as u64)
        })
    } else {
        None
    };
    Ok(json_response(
        StatusCode::OK,
        json!({
            "items": page.iter().map(|row| project::public_history_event(row, job)).collect::<Vec<_>>(),
            "next_cursor": next_cursor,
            "terminal": true,
            "gap": false,
            "earliest_cursor": Value::Null,
            "refreshed_at": project::iso_now(),
        }),
        Cache::PrivateNoStore,
    ))
}

pub(super) async fn commands(
    state: &AppState,
    caller: &Caller,
    params: &[(String, String)],
) -> Result<Response, ApiError> {
    let Some(snapshot) = snapshot(state).await? else {
        return Ok(json_response(StatusCode::OK, json!({ "items": [], "next_cursor": null }), Cache::NoStore));
    };
    let limit = bounded_integer(param(params, "limit"), 50, 1, MAX_LIMIT)?;
    let before = match param(params, "cursor").filter(|value| !value.is_empty()) {
        Some(cursor) => Some(decode_page_cursor(cursor)?),
        None => None,
    };
    let rows = state
        .store
        .commands(&CommandQuery {
            fleet_id: snapshot_fleet_id(&snapshot),
            actor: command_actor_filter(caller),
            before,
            limit: limit + 1,
        })
        .await
        .map_err(storage_error)?;
    let page = page_of(&rows, limit);
    let next_cursor = if rows.len() as i64 > limit {
        page.last().map(|last| {
            encode_page_cursor(
                millis(last.get("created_at")).unwrap_or(0),
                &string_or(last, "request_id", ""),
            )
        })
    } else {
        None
    };
    let items: Vec<Value> = page
        .iter()
        .map(|row| {
            json!({
                "request_id": string_or(row, "request_id", "undefined"),
                "actor": string_or(row, "actor", "undefined"),
                "source": string_or(row, "source", "undefined"),
                "action": string_or(row, "action", "undefined"),
                "status": string_or(row, "status", "undefined"),
                "job_id": nullable(row, "job_id"),
                "agent_id": nullable(row, "agent_id"),
                "agent_name": nullable(row, "agent_name"),
                "node_name": nullable(row, "node_name"),
                "created_at": project::iso(row.get("created_at")),
                "updated_at": project::iso(row.get("updated_at")),
                "completed_at": project::iso(row.get("completed_at")),
                "error_code": nullable(row, "error_code"),
                "error_message": nullable(row, "error_message"),
            })
        })
        .collect();
    Ok(json_response(
        StatusCode::OK,
        json!({ "items": items, "next_cursor": next_cursor }),
        Cache::NoStore,
    ))
}

pub(super) async fn repos(state: &AppState) -> Result<Response, ApiError> {
    let result = state
        .control
        .list_repositories()
        .await
        .map_err(|status| ApiError::from_status(&status, false))?;
    let items: Vec<Value> = result
        .repositories
        .iter()
        .map(|repository| {
            json!({
                "name": repository.name,
                "url": repository.url,
                "branch": repository.branch,
                "reachable": repository.reachable,
                "error": project::safe_text_str(&repository.error, 1000),
            })
        })
        .collect();
    Ok(json_response(StatusCode::OK, json!({ "items": items }), Cache::NoStore))
}
```

- [ ] **Step 7: The `api` subcommand**

In `src/main.rs`, add a variant after `Controller { … }` (line 29-32):

```rust
    /// Serve the public run API (HTTP) for bearer-token clients such as WarpSpeed.
    Api {
        #[arg(long, default_value = "0.0.0.0:8080")]
        bind: String,
    },
```

Dispatch it after `Commands::Controller { bind } => cmd_controller(config_path, bind).await,` (line 198):

```rust
        Commands::Api { bind } => cmd_api(bind).await,
```

Add after `cmd_controller` (ends line 611):

```rust
/// Serve the public run API; it is configured from the environment (see `agent_ctrl::api`).
async fn cmd_api(bind: String) -> Result<()> {
    let addr: std::net::SocketAddr = bind
        .parse()
        .map_err(|e| anyhow::anyhow!("invalid --bind '{bind}': {e}"))?;
    let state = agent_ctrl::api::state_from_env().await?;
    tracing::info!("starting the public run API (Ctrl-C to stop)");
    agent_ctrl::api::run_api(addr, state, ctrl_c()).await
}
```

- [ ] **Step 8: Run the tests to verify they pass**

Run: `cargo test --lib api:: && cargo run --quiet -- api --help`
Expected: all `api::` tests PASS; the help lists `--bind <BIND>  [default: 0.0.0.0:8080]`.

Then `AGENT_CTRL_ENVIRONMENT=staging cargo run --quiet -- api` (no issuer set)
Expected: exits non-zero with `AGENT_PLATFORM_ISSUER is required`.

- [ ] **Step 9: Commit**

```bash
cargo fmt
git add src/main.rs src/api/mod.rs src/api/fakes.rs src/api/handlers/mod.rs src/api/handlers/reads.rs src/api/handlers/tests.rs
git commit -m "feat(api): serve the agent-control read routes with owner scoping

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 10: Mutation routes

**Files:**
- Create: `src/api/handlers/mutations.rs`
- Modify: `src/api/handlers/mod.rs` (`mod mutations;`, `runs`, `run`, `run_action`), `src/api/handlers/tests.rs`

**Interfaces:**
- Consumes: `reads::{snapshot, snapshot_fleet_id, node_ready, REQUIRED_CONTROL_API_VERSION}`, `limits::*`, `ControlPlane`, `Caller` (Tasks 5–9).
- Produces: `mutations::{launch(&AppState, &Caller, Request), import_run(&AppState, &Caller, Request), import_history(&AppState, &Caller, &Document, Request), mutate_run(&AppState, &Caller, &Document, Option<&Document>, action: &str, Request)}`, each `-> Result<Response, ApiError>`.

Owner propagation: launches and imports send `owner_sub = caller.sub`, `owner_email` = the caller's email when it is a plausible address (else empty, which leaves the git author unset), `owner_name` = the display name without `<`, `>` or control characters (≤128 characters), so the controller's `run_owner` validation never rejects a real user.

- [ ] **Step 1: Write the failing tests**

Append to `src/api/handlers/tests.rs`:

```rust
const BOUNDARY: &str = "p1-agentctrl-boundary";

fn launch_body() -> Value {
    json!({
        "agent_name": "staging-claude-interactive-rw",
        "interactive": true,
        "repo_url": "https://git-dev.obachan.dev/stozn/warpspeed.git",
        "base_branch": "dev",
    })
}

/// A multipart body of `(name, file name, content)` parts.
fn multipart(parts: &[(&str, Option<&str>, &[u8])]) -> Vec<u8> {
    let mut body = Vec::new();
    for (name, file, content) in parts {
        body.extend_from_slice(format!("--{BOUNDARY}\r\n").as_bytes());
        let disposition = match file {
            Some(file) => format!(
                "Content-Disposition: form-data; name=\"{name}\"; filename=\"{file}\"\r\nContent-Type: application/octet-stream\r\n\r\n"
            ),
            None => format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n"),
        };
        body.extend_from_slice(disposition.as_bytes());
        body.extend_from_slice(content);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{BOUNDARY}--\r\n").as_bytes());
    body
}

fn post_multipart(path: &str, token: &str, key: &str, body: Vec<u8>) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(path)
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", format!("multipart/form-data; boundary={BOUNDARY}"))
        .header("idempotency-key", key)
        .body(Body::from(body))
        .unwrap()
}

fn import_parts<'a>(handoff_branch: &'a str, extra: Option<(&'a str, &'a [u8])>) -> Vec<(&'a str, Option<&'a str>, &'a [u8])> {
    let mut parts: Vec<(&str, Option<&str>, &[u8])> = vec![
        ("agent_name", None, b"staging-claude-moved".as_slice()),
        ("session_id", None, b"0b5e1c2a-7f4d-4c1e-9a43-2f1d5b6c7e8f".as_slice()),
        ("repo_url", None, b"https://git-dev.obachan.dev/stozn/warpspeed.git".as_slice()),
        ("handoff_branch", None, handoff_branch.as_bytes()),
        ("source_cwd", None, b"C:\\Users\\dev\\warpspeed".as_slice()),
        ("session_file", Some("session.jsonl"), b"{\"type\":\"user\"}\n".as_slice()),
    ];
    if let Some((name, value)) = extra {
        parts.push((name, None, value));
    }
    parts
}

#[tokio::test]
async fn a_launch_names_the_caller_as_owner_and_audit_actor() {
    let harness = seeded();
    let mut request = fakes::post_json(
        "/api/agent-control/runs",
        &member_token(ADA),
        Some("launch-request-0001"),
        &launch_body(),
    );
    request.headers_mut().insert("x-stozn-client", "warpspeed".parse().unwrap());
    let (status, headers, body) = send(&harness.app, request).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(headers["idempotency-replayed"], "false");
    assert_eq!(headers["cache-control"], "private, no-store");
    assert_eq!(body["status"], "accepted");
    assert_eq!(body["action"], "launch");
    assert_eq!(body["job_id"], "job-new");
    let spawn = harness.control.spawns.lock().unwrap().pop().expect("SpawnAgent was called");
    assert_eq!(spawn.owner_sub, ADA);
    assert_eq!(spawn.owner_email, format!("{ADA}@example.test"));
    assert_eq!(spawn.owner_name, format!("User {ADA}"));
    assert_eq!(spawn.actor, format!("@{ADA}:example.test"));
    assert_eq!(spawn.source, "warpspeed");
    assert_eq!(spawn.request_id, "launch-request-0001");
    assert_eq!(spawn.interactive_override, Some(true));
    assert_eq!(spawn.base_branch, "dev");
    assert_eq!(spawn.spec.unwrap().agent_name, "staging-claude-interactive-rw");
}

#[tokio::test]
async fn task_bound_launches_stay_on_the_dashboard() {
    let harness = seeded();
    let body = json!({ "agent_name": "staging-claude-interactive-rw", "interactive": true, "task_id": "task-1" });
    let (status, _, body) = send(
        &harness.app,
        fakes::post_json("/api/agent-control/runs", &member_token(ADA), Some("launch-request-0002"), &body),
    )
    .await;
    assert_eq!((status, code(&body)), (StatusCode::UNPROCESSABLE_ENTITY, Some("task_launch_unsupported")));
    assert_eq!(harness.control.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn members_cannot_mutate_runs_they_do_not_own() {
    let harness = seeded();
    for action in ["input", "close", "stop", "pause", "resume"] {
        let body = match action {
            "input" => json!({ "text": "hi" }),
            "stop" => json!({ "force": false }),
            _ => json!({}),
        };
        let path = format!("/api/agent-control/runs/job-bob/{action}");
        let (status, _, _) = send(
            &harness.app,
            fakes::post_json(&path, &member_token(ADA), Some("member-request-0001"), &body),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{action}");
    }
    assert_eq!(harness.control.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn owners_send_input_and_replays_are_flagged() {
    let harness = seeded();
    let (status, headers, body) = send(
        &harness.app,
        fakes::post_json(
            "/api/agent-control/runs/job-ada/input",
            &member_token(ADA),
            Some("replay-request-0001"),
            &json!({ "text": "continue" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(headers["idempotency-replayed"], "true");
    assert_eq!(body["action"], "input");
    assert_eq!(body["job_id"], "job-ada");
    assert_eq!(body["agent_id"], "agent-job-ada");
}

#[tokio::test]
async fn mutation_guards_match_the_dashboard() {
    let harness = seeded();
    let ada = member_token(ADA);
    let input = "/api/agent-control/runs/job-ada/input";
    let (status, _, body) = send(&harness.app, fakes::post_json(input, &ada, None, &json!({ "text": "x" }))).await;
    assert_eq!((status, code(&body)), (StatusCode::BAD_REQUEST, Some("invalid_idempotency_key")));
    let mut wrong_media = fakes::post_json(input, &ada, Some("input-request-0009"), &json!({ "text": "x" }));
    wrong_media.headers_mut().insert("content-type", "text/plain".parse().unwrap());
    let (status, _, body) = send(&harness.app, wrong_media).await;
    assert_eq!((status, code(&body)), (StatusCode::UNSUPPORTED_MEDIA_TYPE, Some("unsupported_media_type")));
    let huge = json!({ "text": "x".repeat(70_000) });
    let (status, _, body) = send(&harness.app, fakes::post_json(input, &ada, Some("input-request-0010"), &huge)).await;
    assert_eq!((status, code(&body)), (StatusCode::PAYLOAD_TOO_LARGE, Some("body_too_large")));
    let extra = json!({ "text": "x", "extra": true });
    let (status, _, body) = send(&harness.app, fakes::post_json(input, &ada, Some("input-request-0011"), &extra)).await;
    assert_eq!((status, body["error"].as_str()), (StatusCode::UNPROCESSABLE_ENTITY, Some("Unsupported fields: extra")));
}

#[tokio::test]
async fn mutations_need_the_rollout_gate_and_a_fresh_controller() {
    let disabled = fakes::harness_with(crate::api::ApiConfig {
        mutations_enabled: false,
        ..fakes::api_config()
    });
    fakes::seed_fleet(&disabled.store);
    disabled.store.insert("jobs", fakes::job_doc("job-ada", Some(ADA), -1_000));
    let (status, _, body) = send(
        &disabled.app,
        fakes::post_json("/api/agent-control/runs/job-ada/input", &member_token(ADA), Some("input-request-0012"), &json!({ "text": "x" })),
    )
    .await;
    assert_eq!((status, code(&body)), (StatusCode::SERVICE_UNAVAILABLE, Some("mutations_disabled")));

    let stale = fakes::harness();
    let old = mongodb::bson::DateTime::from_millis(chrono::Utc::now().timestamp_millis() - 60_000);
    stale.store.insert("fleet_snapshots", mongodb::bson::doc! { "_id": "fleet-staging", "environment": "staging", "control_api_version": 2_i64, "last_seen": old });
    stale.store.insert("jobs", fakes::job_doc("job-ada", Some(ADA), -1_000));
    let (status, _, body) = send(
        &stale.app,
        fakes::post_json("/api/agent-control/runs", &member_token(ADA), Some("launch-request-0003"), &launch_body()),
    )
    .await;
    assert_eq!((status, code(&body)), (StatusCode::SERVICE_UNAVAILABLE, Some("controller_unavailable")));
    assert_eq!(stale.control.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn thirty_mutations_a_minute_then_429() {
    let harness = seeded();
    let ada = member_token(ADA);
    for n in 0..30 {
        let key = format!("input-request-{n:04}");
        let (status, _, _) = send(
            &harness.app,
            fakes::post_json("/api/agent-control/runs/job-ada/input", &ada, Some(&key), &json!({ "text": "x" })),
        )
        .await;
        assert_eq!(status, StatusCode::ACCEPTED, "{n}");
    }
    let (status, headers, body) = send(
        &harness.app,
        fakes::post_json("/api/agent-control/runs/job-ada/input", &ada, Some("input-request-0030"), &json!({ "text": "x" })),
    )
    .await;
    assert_eq!((status, code(&body)), (StatusCode::TOO_MANY_REQUESTS, Some("rate_limited")));
    assert_eq!(headers["retry-after"], "60");
}

#[tokio::test]
async fn controller_errors_map_to_dashboard_codes() {
    let harness = seeded();
    let ada = member_token(ADA);
    let input = "/api/agent-control/runs/job-ada/input";
    let cases = [
        (input, "conflict-request-01", json!({ "text": "x" }), StatusCode::CONFLICT, "conflict"),
        ("/api/agent-control/runs/job-ada/stop", "unavailable-request-1", json!({ "force": false }), StatusCode::SERVICE_UNAVAILABLE, "controller_unavailable"),
        ("/api/agent-control/runs", "atcapacity-request-1", launch_body(), StatusCode::CONFLICT, "node_at_capacity"),
        ("/api/agent-control/runs/job-ada/pause", "notstate-request-01", json!({}), StatusCode::CONFLICT, "invalid_state"),
        ("/api/agent-control/runs/job-ada/pause", "sessionunknown-req-1", json!({}), StatusCode::PRECONDITION_FAILED, "session_unknown"),
    ];
    for (path, key, body, expected_status, expected_code) in cases {
        let (status, _, body) = send(&harness.app, fakes::post_json(path, &ada, Some(key), &body)).await;
        assert_eq!((status, code(&body)), (expected_status, Some(expected_code)), "{key}");
    }
    let (status, _, body) = send(&harness.app, fakes::post_json(input, &ada, Some("rejected-request-001"), &json!({ "text": "x" }))).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(body["status"], "rejected");
    assert_eq!(body["error"], json!({ "error": "policy denied", "code": "command_rejected", "request_id": "rejected-request-001" }));
}

#[tokio::test]
async fn pause_and_resume_answer_with_the_run_state() {
    let harness = seeded();
    let mut paused = fakes::job_doc("job-ada-paused", Some(ADA), -500);
    paused.insert("state", "paused");
    harness.store.insert("jobs", paused);
    let ada = member_token(ADA);
    let (status, headers, body) = send(&harness.app, fakes::post_json("/api/agent-control/runs/job-ada/pause", &ada, Some("pause-request-00001"), &json!({}))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers["idempotency-replayed"], "false");
    assert_eq!(body, json!({ "job_id": "job-ada", "state": "paused", "accepted": true, "node_name": "agent-ctrl-node-0" }));
    let (status, _, body) = send(&harness.app, fakes::post_json("/api/agent-control/runs/job-ada-paused/resume", &ada, Some("resume-request-0001"), &json!({}))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["node_name"], "agent-ctrl-node-1");
    let (status, _, body) = send(&harness.app, fakes::post_json("/api/agent-control/runs/job-ada/pause", &ada, Some("rejected-request-002"), &json!({}))).await;
    assert_eq!((status, code(&body), body["error"].as_str()), (StatusCode::CONFLICT, Some("command_rejected"), Some("policy denied")));
}

#[tokio::test]
async fn imports_carry_the_owner_and_validate_the_form() {
    let harness = seeded();
    let ada = member_token(ADA);
    let body = multipart(&import_parts("handoff/move-1234", None));
    let (status, _, response) = send(&harness.app, post_multipart("/api/agent-control/runs/import", &ada, "import-request-0001", body)).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{response}");
    assert_eq!(response["action"], "import");
    let import = harness.control.imports.lock().unwrap().pop().expect("ImportSession was called");
    assert_eq!(import.owner_sub, ADA);
    assert_eq!(import.session_file, b"{\"type\":\"user\"}\n".to_vec());
    assert_eq!(import.source, "agentctrl_api");

    let body = multipart(&import_parts("handoff/../x", None));
    let (status, _, response) = send(&harness.app, post_multipart("/api/agent-control/runs/import", &ada, "import-request-0002", body)).await;
    assert_eq!((status, response["error"].as_str()), (StatusCode::UNPROCESSABLE_ENTITY, Some("handoff_branch is invalid")));

    let body = multipart(&import_parts("handoff/move-1234", Some(("extra", b"1".as_slice()))));
    let (status, _, response) = send(&harness.app, post_multipart("/api/agent-control/runs/import", &ada, "import-request-0003", body)).await;
    assert_eq!((status, response["error"].as_str()), (StatusCode::UNPROCESSABLE_ENTITY, Some("Unsupported field: extra")));

    let json_import = fakes::post_json("/api/agent-control/runs/import", &ada, Some("import-request-0004"), &json!({}));
    let (status, _, response) = send(&harness.app, json_import).await;
    assert_eq!((status, code(&response)), (StatusCode::UNSUPPORTED_MEDIA_TYPE, Some("unsupported_media_type")));
}

#[tokio::test]
async fn history_imports_need_a_moved_session() {
    let harness = seeded();
    let ada = member_token(ADA);
    let body = multipart(&[("session_file", Some("session.jsonl"), b"{}\n".as_slice())]);
    let (status, _, response) = send(&harness.app, post_multipart("/api/agent-control/runs/job-ada/history", &ada, "history-request-001", body)).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{response}");
    assert_eq!(response["action"], "import_session_history");
    assert_eq!(response["history_event_count"], 2);
    let body = multipart(&[("other", None, b"x".as_slice())]);
    let (status, _, response) = send(&harness.app, post_multipart("/api/agent-control/runs/job-ada/history", &ada, "history-request-002", body)).await;
    assert_eq!((status, response["error"].as_str()), (StatusCode::UNPROCESSABLE_ENTITY, Some("session_file is the only supported field")));
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --lib api::handlers`
Expected: the new tests FAIL (405 `method_not_allowed` for every POST).

- [ ] **Step 3: Implement `src/api/handlers/mutations.rs`**

```rust
//! Mutating routes, checked in the dashboard's order: rollout gate, controller freshness, media
//! type, rate limit, idempotency key, body, fields, then FleetControl. Launches and imports name
//! the caller as the run owner.

use super::reads::{node_ready, snapshot, snapshot_fleet_id, REQUIRED_CONTROL_API_VERSION};
use super::{json_response, storage_error, Cache};
use crate::api::auth::Caller;
use crate::api::error::ApiError;
use crate::api::limits::{
    exact_keys, idempotency_key, json_body, media_type, optional_string, request_source,
    require_json, required_string, MAX_SESSION_FILE_BYTES,
};
use crate::api::project::{self, integer, is_terminal, millis, nullable, string_or, CONTROLLER_STALE_MS};
use crate::api::AppState;
use crate::pb;
use axum::body::Bytes;
use axum::extract::multipart::MultipartError;
use axum::extract::{FromRequest, Multipart, Request};
use axum::http::{HeaderValue, StatusCode};
use axum::response::Response;
use chrono::Utc;
use mongodb::bson::Document;
use regex::Regex;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::sync::LazyLock;

const LAUNCH_FIELDS: [&str; 7] =
    ["agent_name", "prompt", "node_name", "interactive", "task_id", "repo_url", "base_branch"];
const IMPORT_FIELDS: [&str; 7] = [
    "agent_name", "session_id", "repo_url", "handoff_branch", "source_cwd", "session_file",
    "node_name",
];
static SESSION_ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z0-9_-]{1,128}$").expect("valid session id pattern"));

/// The dashboard's `BASE_BRANCH` pattern without look-arounds.
fn valid_base_branch(branch: &str) -> bool {
    (1..=100).contains(&branch.len())
        && branch
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._/-".contains(&byte))
        && !branch.starts_with(['-', '/', '.'])
        && !branch.contains("..")
        && !branch.contains("/.")
}

/// The dashboard's `HANDOFF_BRANCH` pattern without look-arounds.
fn valid_handoff_branch(branch: &str) -> bool {
    branch.strip_prefix("handoff/").is_some_and(|name| {
        (1..=100).contains(&name.len())
            && name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
            && !name.contains("..")
    })
}

fn owner_email(caller: &Caller) -> String {
    caller
        .email
        .clone()
        .filter(|email| {
            email.len() <= 254
                && email.contains('@')
                && !email
                    .chars()
                    .any(|c| c.is_control() || c.is_whitespace() || c == '<' || c == '>')
        })
        .unwrap_or_default()
}

fn owner_name(caller: &Caller) -> String {
    caller
        .name
        .as_deref()
        .unwrap_or_default()
        .chars()
        .filter(|c| !c.is_control() && *c != '<' && *c != '>')
        .take(128)
        .collect::<String>()
        .trim()
        .to_string()
}

fn require_ready(state: &AppState) -> Result<(), ApiError> {
    if state.config.mutations_enabled {
        Ok(())
    } else {
        Err(ApiError::new(503, "Managed mutations are disabled by rollout policy", "mutations_disabled"))
    }
}

async fn require_fresh_target(state: &AppState) -> Result<(), ApiError> {
    let now = Utc::now().timestamp_millis();
    let stale = || ApiError::new(503, "Controller heartbeat is stale", "controller_unavailable");
    let snapshot = snapshot(state).await?.ok_or_else(stale)?;
    if millis(snapshot.get("last_seen")).is_none_or(|seen| now - seen > CONTROLLER_STALE_MS) {
        return Err(stale());
    }
    if integer(snapshot.get("control_api_version"), 0) < REQUIRED_CONTROL_API_VERSION {
        return Err(ApiError::new(
            503,
            "Controller does not support task-bound managed sessions",
            "controller_incompatible",
        ));
    }
    let nodes = state
        .store
        .nodes(&snapshot_fleet_id(&snapshot))
        .await
        .map_err(storage_error)?;
    if !nodes.iter().any(|node| node_ready(node, now)) {
        return Err(ApiError::new(503, "No reconciled execution node is connected", "controller_unavailable"));
    }
    Ok(())
}

fn nonempty(value: &str) -> Value {
    if value.is_empty() {
        Value::Null
    } else {
        Value::String(value.to_string())
    }
}

fn with_replayed(mut response: Response, replayed: bool) -> Response {
    response.headers_mut().insert(
        "idempotency-replayed",
        HeaderValue::from_static(if replayed { "true" } else { "false" }),
    );
    response
}

struct Outcome {
    success: bool,
    replayed: bool,
    error: String,
    job_id: Value,
    agent_id: Value,
    history_event_count: Option<u32>,
}

/// The dashboard's `commandResponse`: 202 with the command's acceptance.
fn command_response(request_id: &str, action: &str, outcome: Outcome) -> Response {
    let status = if outcome.success { "accepted" } else { "rejected" };
    let error = if outcome.success {
        Value::Null
    } else {
        json!({
            "error": project::safe_text_str(&outcome.error, 1000).unwrap_or_else(|| "Command rejected".into()),
            "code": "command_rejected",
            "request_id": request_id,
        })
    };
    let mut body = json!({
        "request_id": request_id,
        "action": action,
        "status": status,
        "job_id": outcome.job_id,
        "agent_id": outcome.agent_id,
        "replayed": outcome.replayed,
        "error": error,
    });
    if let Some(count) = outcome.history_event_count {
        body["history_event_count"] = json!(count);
    }
    with_replayed(
        json_response(StatusCode::ACCEPTED, body, Cache::PrivateNoStore),
        outcome.replayed,
    )
}

/// The dashboard's `controlResponse` for pause and resume: 200 with the run state, or 409.
fn control_response(
    job: &Document,
    fallback_state: &str,
    accepted: bool,
    replayed: bool,
    error: &str,
    state: &str,
    node_name: &str,
) -> Result<Response, ApiError> {
    if !accepted {
        return Err(ApiError::new(
            409,
            project::safe_text_str(error, 1000).unwrap_or_else(|| "Command rejected".into()),
            "command_rejected",
        ));
    }
    let node = if node_name.is_empty() {
        nullable(job, "node")
    } else {
        Value::String(node_name.to_string())
    };
    let body = json!({
        "job_id": string_or(job, "job_id", ""),
        "state": if state.is_empty() { fallback_state } else { state },
        "accepted": true,
        "node_name": node,
    });
    Ok(with_replayed(
        json_response(StatusCode::OK, body, Cache::PrivateNoStore),
        replayed,
    ))
}

fn grpc(status: tonic::Status) -> ApiError {
    ApiError::from_status(&status, false)
}

pub(super) async fn launch(state: &AppState, caller: &Caller, request: Request) -> Result<Response, ApiError> {
    require_ready(state)?;
    require_fresh_target(state).await?;
    let (parts, body) = request.into_parts();
    require_json(&parts.headers)?;
    state.limiter.check(&caller.actor)?;
    let request_id = idempotency_key(&parts.headers)?;
    let body = json_body(&parts.headers, body).await?;
    exact_keys(&body, &LAUNCH_FIELDS)?;
    let agent_name = required_string(body.get("agent_name"), "agent_name", 128)?;
    let repo_url = optional_string(body.get("repo_url"), "repo_url", 512)?;
    let base_branch = optional_string(body.get("base_branch"), "base_branch", 100)?;
    if base_branch.as_deref().is_some_and(|branch| !valid_base_branch(branch)) {
        return Err(ApiError::new(422, "base_branch is invalid", "validation_error"));
    }
    // Task-bound launches need the STOZN work item and its room visibility, which only the
    // dashboard can resolve.
    let Some(repo_url) = repo_url else {
        return Err(ApiError::new(
            422,
            "Task-bound launches are served by the STOZN dashboard",
            "task_launch_unsupported",
        ));
    };
    let prompt = optional_string(body.get("prompt"), "prompt", 32_000)?;
    let node_name = optional_string(body.get("node_name"), "node_name", 128)?;
    let Some(Value::Bool(interactive)) = body.get("interactive") else {
        return Err(ApiError::new(422, "interactive must be a boolean", "validation_error"));
    };
    let result = state
        .control
        .spawn_agent(pb::SpawnAgentRequest {
            node_name: node_name.unwrap_or_default(),
            spec: Some(pb::SpawnCommand {
                agent_name,
                prompt: prompt.unwrap_or_default(),
                ..Default::default()
            }),
            request_id: request_id.clone(),
            actor: caller.actor.clone(),
            source: request_source(&parts.headers).to_string(),
            interactive_override: Some(*interactive),
            repo_url,
            base_branch: base_branch.unwrap_or_default(),
            owner_sub: caller.sub.clone(),
            owner_email: owner_email(caller),
            owner_name: owner_name(caller),
            ..Default::default()
        })
        .await
        .map_err(grpc)?;
    Ok(command_response(
        &request_id,
        "launch",
        Outcome {
            success: result.success,
            replayed: result.replayed,
            error: result.error,
            job_id: nonempty(&result.job_id),
            agent_id: nonempty(&result.agent_id),
            history_event_count: None,
        },
    ))
}

enum Field {
    Text(String),
    File(Bytes),
}

fn bad_multipart() -> ApiError {
    ApiError::new(400, "Request body must be valid multipart/form-data", "bad_request")
}

fn file_too_large() -> ApiError {
    ApiError::new(413, "Session file is too large", "payload_too_large")
}

fn multipart_error(error: MultipartError) -> ApiError {
    if error.status() == StatusCode::PAYLOAD_TOO_LARGE {
        file_too_large()
    } else {
        bad_multipart()
    }
}

/// Every form field in order; a part with a file name is a file, like a browser `FormData` Blob.
async fn read_form(state: &AppState, request: Request) -> Result<Vec<(String, Field)>, ApiError> {
    let mut multipart = Multipart::from_request(request, state)
        .await
        .map_err(|_| bad_multipart())?;
    let mut fields = Vec::new();
    while let Some(field) = multipart.next_field().await.map_err(multipart_error)? {
        let name = field.name().unwrap_or_default().to_string();
        let is_file = field.file_name().is_some();
        let bytes = field.bytes().await.map_err(multipart_error)?;
        let value = if is_file {
            Field::File(bytes)
        } else {
            Field::Text(String::from_utf8_lossy(&bytes).into_owned())
        };
        fields.push((name, value));
    }
    Ok(fields)
}

fn field<'a>(fields: &'a [(String, Field)], name: &str) -> Option<&'a Field> {
    fields.iter().find(|(key, _)| key == name).map(|(_, value)| value)
}

fn text_field(fields: &[(String, Field)], name: &str, max: usize) -> Result<Option<String>, ApiError> {
    match field(fields, name) {
        None => Ok(None),
        Some(Field::File(_)) => Err(ApiError::new(422, format!("{name} must be a text field"), "validation_error")),
        Some(Field::Text(value)) => required_string(Some(&Value::String(value.clone())), name, max).map(Some),
    }
}

fn required_field(fields: &[(String, Field)], name: &str, max: usize) -> Result<String, ApiError> {
    text_field(fields, name, max)?
        .ok_or_else(|| ApiError::new(422, format!("{name} is required"), "validation_error"))
}

fn session_file(fields: &[(String, Field)]) -> Result<Bytes, ApiError> {
    match field(fields, "session_file") {
        Some(Field::File(bytes)) if !bytes.is_empty() => {
            if bytes.len() > MAX_SESSION_FILE_BYTES {
                Err(file_too_large())
            } else {
                Ok(bytes.clone())
            }
        }
        _ => Err(ApiError::new(422, "session_file is required", "validation_error")),
    }
}

fn require_multipart(request: &Request, message: &str) -> Result<(), ApiError> {
    if media_type(request.headers()) == "multipart/form-data" {
        Ok(())
    } else {
        Err(ApiError::new(415, message, "unsupported_media_type"))
    }
}

pub(super) async fn import_run(state: &AppState, caller: &Caller, request: Request) -> Result<Response, ApiError> {
    require_ready(state)?;
    require_fresh_target(state).await?;
    require_multipart(&request, "Import requires multipart/form-data")?;
    state.limiter.check(&caller.actor)?;
    let request_id = idempotency_key(request.headers())?;
    let source = request_source(request.headers());
    let fields = read_form(state, request).await?;
    let mut seen = HashSet::new();
    for (name, _) in &fields {
        if !IMPORT_FIELDS.contains(&name.as_str()) {
            let shown: String = name.chars().take(64).collect();
            return Err(ApiError::new(422, format!("Unsupported field: {shown}"), "validation_error"));
        }
        if !seen.insert(name.as_str()) {
            return Err(ApiError::new(422, format!("{name} must be sent once"), "validation_error"));
        }
    }
    let agent_name = required_field(&fields, "agent_name", 128)?;
    let session_id = required_field(&fields, "session_id", 128)?;
    if !SESSION_ID.is_match(&session_id) {
        return Err(ApiError::new(422, "session_id is invalid", "validation_error"));
    }
    let repo_url = required_field(&fields, "repo_url", 512)?;
    let handoff_branch = required_field(&fields, "handoff_branch", 108)?;
    if !valid_handoff_branch(&handoff_branch) {
        return Err(ApiError::new(422, "handoff_branch is invalid", "validation_error"));
    }
    let source_cwd = required_field(&fields, "source_cwd", 1024)?;
    let node_name = text_field(&fields, "node_name", 128)?;
    let file = session_file(&fields)?;
    let result = state
        .control
        .import_session(pb::ImportSessionRequest {
            node_name: node_name.unwrap_or_default(),
            agent_name,
            session_id,
            session_file: file.to_vec(),
            repo_url,
            handoff_branch,
            source_cwd,
            request_id: request_id.clone(),
            actor: caller.actor.clone(),
            source: source.to_string(),
            owner_sub: caller.sub.clone(),
            owner_email: owner_email(caller),
            owner_name: owner_name(caller),
        })
        .await
        .map_err(grpc)?;
    Ok(command_response(
        &request_id,
        "import",
        Outcome {
            success: result.success,
            replayed: result.replayed,
            error: result.error,
            job_id: nonempty(&result.job_id),
            agent_id: nonempty(&result.agent_id),
            history_event_count: None,
        },
    ))
}

pub(super) async fn import_history(
    state: &AppState,
    caller: &Caller,
    job: &Document,
    request: Request,
) -> Result<Response, ApiError> {
    require_ready(state)?;
    require_multipart(&request, "History import requires multipart/form-data")?;
    state.limiter.check(&caller.actor)?;
    let request_id = idempotency_key(request.headers())?;
    let source = request_source(request.headers());
    let fields = read_form(state, request).await?;
    if fields.len() != 1 || fields[0].0 != "session_file" {
        return Err(ApiError::new(422, "session_file is the only supported field", "validation_error"));
    }
    let file = session_file(&fields)?;
    let Some(session_id) = project::text(job, "session_id") else {
        return Err(ApiError::new(409, "Run has no moved-session identity", "not_moved_session"));
    };
    let job_id = string_or(job, "job_id", "");
    let result = state
        .control
        .import_session_history(pb::ImportSessionHistoryRequest {
            job_id: job_id.clone(),
            session_id: session_id.to_string(),
            session_file: file.to_vec(),
            request_id: request_id.clone(),
            actor: caller.actor.clone(),
            source: source.to_string(),
        })
        .await
        .map_err(grpc)?;
    Ok(command_response(
        &request_id,
        "import_session_history",
        Outcome {
            success: result.success,
            replayed: result.replayed,
            error: result.error,
            job_id: Value::String(job_id),
            agent_id: nullable(job, "agent_id"),
            history_event_count: Some(result.history_event_count),
        },
    ))
}

pub(super) async fn mutate_run(
    state: &AppState,
    caller: &Caller,
    job: &Document,
    snapshot: Option<&Document>,
    action: &str,
    request: Request,
) -> Result<Response, ApiError> {
    require_ready(state)?;
    require_fresh_target(state).await?;
    let (parts, body) = request.into_parts();
    require_json(&parts.headers)?;
    state.limiter.check(&caller.actor)?;
    let request_id = idempotency_key(&parts.headers)?;
    let body = json_body(&parts.headers, body).await?;
    let run = project::public_run(job, snapshot);
    let run_state = run["state"].as_str().unwrap_or_default().to_string();
    if is_terminal(&run_state) {
        return Err(ApiError::new(409, "Run is terminal", "terminal_run"));
    }
    let actor = caller.actor.clone();
    let source = request_source(&parts.headers).to_string();
    let job_id = string_or(job, "job_id", "");
    let agent_id = string_or(job, "agent_id", "");
    let node_name = string_or(job, "node", "");
    let input_not_allowed = || ApiError::new(409, "Run does not accept operator input", "input_not_allowed");
    let (action_name, result) = match action {
        "input" | "close" => {
            if action == "input" {
                exact_keys(&body, &["text"])?;
            } else {
                exact_keys(&body, &[])?;
            }
            if run["input_allowed"] != true {
                return Err(input_not_allowed());
            }
            let close_input = action == "close";
            let input = if close_input {
                String::new()
            } else {
                required_string(body.get("text"), "text", 8000)?
            };
            let result = state
                .control
                .send_input(pb::SendInputRequest {
                    agent_id: agent_id.clone(),
                    node_name,
                    input,
                    close_input,
                    request_id: request_id.clone(),
                    actor,
                    source,
                })
                .await
                .map_err(grpc)?;
            let name = if close_input { "close_input" } else { "input" };
            (name, (result.success, result.replayed, result.error))
        }
        "pause" => {
            exact_keys(&body, &[])?;
            if job.get_bool("interactive") != Ok(true) {
                return Err(ApiError::new(409, "Only interactive runs can be paused", "not_interactive"));
            }
            if run_state != "running" && run_state != "paused" {
                return Err(ApiError::new(409, "Only a running run can be paused", "invalid_state"));
            }
            let result = state
                .control
                .pause_agent(pb::PauseAgentRequest { job_id, request_id, actor, source })
                .await
                .map_err(|status| ApiError::from_status(&status, true))?;
            return control_response(job, "paused", result.accepted, result.replayed, &result.error, &result.state, &result.node_name);
        }
        "resume" => {
            exact_keys(&body, &[])?;
            if run_state != "paused" && run_state != "running" {
                return Err(ApiError::new(409, "Only a paused run can be resumed", "invalid_state"));
            }
            let result = state
                .control
                .resume_agent(pb::ResumeAgentRequest { job_id, request_id, actor, source })
                .await
                .map_err(|status| ApiError::from_status(&status, true))?;
            return control_response(job, "running", result.accepted, result.replayed, &result.error, &result.state, &result.node_name);
        }
        "stop" => {
            exact_keys(&body, &["force"])?;
            let Some(Value::Bool(force)) = body.get("force") else {
                return Err(ApiError::new(422, "force must be a boolean", "validation_error"));
            };
            let result = state
                .control
                .stop_agent(pb::StopAgentRequest {
                    agent_id: agent_id.clone(),
                    node_name,
                    force: *force,
                    request_id: request_id.clone(),
                    actor,
                    source,
                    job_id: job_id.clone(),
                })
                .await
                .map_err(grpc)?;
            let name = if *force { "force_stop" } else { "stop" };
            (name, (result.success, result.replayed, result.error))
        }
        _ => return Err(ApiError::not_found()),
    };
    let (success, replayed, error) = result;
    Ok(command_response(
        &request_id,
        action_name,
        Outcome {
            success,
            replayed,
            error,
            job_id: Value::String(string_or(job, "job_id", "")),
            agent_id: nonempty(&agent_id),
            history_event_count: None,
        },
    ))
}
```

If the resolved axum 0.7.x lacks `MultipartError::status`, map with `error.body_text().contains("length limit")` instead; the 413 test pins the behavior either way.

- [ ] **Step 4: Route the mutations**

In `src/api/handlers/mod.rs` add `mod mutations;` after `mod reads;`, and replace the three dispatchers:

```rust
async fn runs(
    State(state): State<AppState>,
    caller: Caller,
    Query(params): Params,
    request: Request,
) -> Result<Response, ApiError> {
    let method = request.method().clone();
    if method == Method::GET {
        reads::list_runs(&state, &caller, &params).await
    } else if method == Method::POST {
        mutations::launch(&state, &caller, request).await
    } else {
        Err(ApiError::not_found())
    }
}

async fn run(
    State(state): State<AppState>,
    caller: Caller,
    Path(job_id): Path<String>,
    request: Request,
) -> Result<Response, ApiError> {
    let method = request.method().clone();
    // The dashboard matches `POST /runs/import` before treating the segment as a job id.
    if job_id == "import" && method == Method::POST {
        return mutations::import_run(&state, &caller, request).await;
    }
    let job = load_run(&state, &caller, &job_id).await?;
    let snapshot = reads::snapshot(&state).await?;
    if method == Method::GET {
        return Ok(json_response(
            StatusCode::OK,
            project::public_run(&job, snapshot.as_ref()),
            Cache::NoStore,
        ));
    }
    Err(method_not_allowed())
}

async fn run_action(
    State(state): State<AppState>,
    caller: Caller,
    Path((job_id, rest)): Path<(String, String)>,
    Query(params): Params,
    request: Request,
) -> Result<Response, ApiError> {
    let action = rest.trim_start_matches('/');
    if !RUN_ACTIONS.contains(&action) {
        return Err(ApiError::not_found());
    }
    let job = load_run(&state, &caller, &job_id).await?;
    let snapshot = reads::snapshot(&state).await?;
    let method = request.method().clone();
    match (&method, action) {
        (&Method::GET, "session") => reads::session(&state, &job, snapshot.as_ref()),
        (&Method::GET, "events") => reads::events(&state, &job, &params).await,
        (&Method::GET, "history") => reads::history(&state, &job, &params).await,
        (&Method::POST, "history") => mutations::import_history(&state, &caller, &job, request).await,
        (&Method::POST, "input" | "close" | "stop" | "pause" | "resume") => {
            mutations::mutate_run(&state, &caller, &job, snapshot.as_ref(), action, request).await
        }
        _ => Err(method_not_allowed()),
    }
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test --lib api::`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
cargo fmt
git add src/api/handlers/mod.rs src/api/handlers/mutations.rs src/api/handlers/tests.rs
git commit -m "feat(api): launch, import and control runs as their owner

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 11: SSE event stream

**Files:**
- Create: `src/api/handlers/stream.rs`
- Modify: `src/api/handlers/mod.rs` (`mod stream;`, `run_action`), `src/api/handlers/tests.rs`

**Interfaces:**
- Consumes: `load_run`, `param` (Task 9), `ReadModel` (Task 7), cursors (Task 4).
- Produces: `GET /api/agent-control/runs/{job}/events/stream` (`text/event-stream`). Frames: `id: <cursor>` + `event: run_event` + `data: <public event JSON>` per event; `event: gap` + `data: {"earliest_cursor": …}` then close; `event: terminal` + `data: {"state": …, "next_cursor": …}` then close; `event: error` + `data: {"code": "storage_unavailable"}` then close; `: keepalive` every 15 s. Resume from `Last-Event-ID`, else `?after=`. Polls Mongo every 250 ms.

- [ ] **Step 1: Write the failing tests**

Append to `src/api/handlers/tests.rs`:

```rust
fn insert_events(store: &crate::api::fakes::MemoryReadModel, job_id: &str, sequences: &[i64]) {
    for sequence in sequences {
        store.insert(
            "job_events",
            mongodb::bson::doc! {
                "_id": format!("{job_id}-{sequence}"), "fleet_id": "fleet-staging", "job_id": job_id,
                "agent_id": format!("agent-{job_id}"), "sequence": *sequence, "type": "assistant",
                "content": format!("line {sequence}"),
            },
        );
    }
}

fn completed_run(harness: &fakes::Harness, job_id: &str) {
    let mut job = fakes::job_doc(job_id, Some(ADA), -500);
    job.insert("state", "completed");
    harness.store.insert("jobs", job);
}

/// SSE frames: the text between blank lines.
fn frames(text: &str) -> Vec<String> {
    text.split("\n\n")
        .map(str::trim)
        .filter(|frame| !frame.is_empty())
        .map(str::to_string)
        .collect()
}

fn stream_request(path: &str, last_event_id: Option<&str>) -> Request<Body> {
    let mut request = fakes::get(path, &member_token(ADA));
    if let Some(id) = last_event_id {
        request.headers_mut().insert("last-event-id", id.parse().unwrap());
    }
    request
}

#[tokio::test]
async fn the_stream_sends_events_then_closes_on_a_terminal_run() {
    let harness = seeded();
    completed_run(&harness, "job-done");
    insert_events(&harness.store, "job-done", &[1, 2]);
    let (status, headers, text) = fakes::send_text(
        &harness.app,
        stream_request("/api/agent-control/runs/job-done/events/stream", None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(headers["content-type"].to_str().unwrap().starts_with("text/event-stream"));
    assert_eq!(headers["cache-control"], "no-store");
    let frames = frames(&text);
    assert_eq!(frames.len(), 3, "{text}");
    assert!(frames[0].contains("id: djE6MQ") && frames[0].contains("event: run_event"), "{}", frames[0]);
    assert!(frames[0].contains("\"content\":\"line 1\""), "{}", frames[0]);
    assert!(frames[1].contains("id: djE6Mg"), "{}", frames[1]);
    assert!(frames[2].contains("event: terminal") && frames[2].contains("\"state\":\"completed\""), "{}", frames[2]);
    assert!(frames[2].contains("\"next_cursor\":\"djE6Mg\""), "{}", frames[2]);
}

#[tokio::test]
async fn the_stream_resumes_after_the_last_event_id_or_after_query() {
    let harness = seeded();
    completed_run(&harness, "job-done");
    insert_events(&harness.store, "job-done", &[1, 2]);
    for request in [
        stream_request("/api/agent-control/runs/job-done/events/stream", Some("djE6MQ")),
        stream_request("/api/agent-control/runs/job-done/events/stream?after=djE6MQ", None),
    ] {
        let (_, _, text) = fakes::send_text(&harness.app, request).await;
        let frames = frames(&text);
        assert_eq!(frames.len(), 2, "{text}");
        assert!(frames[0].contains("id: djE6Mg"));
        assert!(frames[1].contains("event: terminal"));
    }
}

#[tokio::test]
async fn a_retention_gap_sends_gap_and_closes() {
    let harness = seeded();
    completed_run(&harness, "job-done");
    insert_events(&harness.store, "job-done", &[5, 6]);
    let (_, _, text) = fakes::send_text(
        &harness.app,
        stream_request("/api/agent-control/runs/job-done/events/stream", None),
    )
    .await;
    let frames = frames(&text);
    assert_eq!(frames.len(), 1, "{text}");
    assert!(frames[0].contains("event: gap") && frames[0].contains("\"earliest_cursor\":\"djE6NQ\""));
}

#[tokio::test]
async fn read_failures_end_the_stream_with_an_error_event() {
    let harness = seeded();
    harness.store.fail("job_events");
    let (_, _, text) = fakes::send_text(
        &harness.app,
        stream_request("/api/agent-control/runs/job-ada/events/stream", None),
    )
    .await;
    let frames = frames(&text);
    assert_eq!(frames.len(), 1, "{text}");
    assert!(frames[0].contains("event: error") && frames[0].contains("storage_unavailable"));
}

#[tokio::test]
async fn members_cannot_stream_other_peoples_runs() {
    let harness = seeded();
    let (status, _, body) = send(
        &harness.app,
        stream_request("/api/agent-control/runs/job-bob/events/stream", None),
    )
    .await;
    assert_eq!((status, code(&body)), (StatusCode::NOT_FOUND, Some("not_found")));
}

#[tokio::test(start_paused = true)]
async fn an_idle_stream_sends_keepalive_comments() {
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    let harness = seeded();
    let started = tokio::time::Instant::now();
    let response = harness
        .app
        .clone()
        .oneshot(stream_request("/api/agent-control/runs/job-ada/events/stream", None))
        .await
        .unwrap();
    let mut body = response.into_body();
    let mut text = String::new();
    while !text.contains(": keepalive") {
        let frame = body.frame().await.expect("the stream stays open").expect("a frame");
        if let Some(data) = frame.data_ref() {
            text.push_str(&String::from_utf8_lossy(data));
        }
    }
    assert!(started.elapsed() >= std::time::Duration::from_secs(15));
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --lib api::handlers::tests::the_stream`
Expected: FAIL (`events/stream` is not a run action → 404).

- [ ] **Step 3: Implement `src/api/handlers/stream.rs`**

```rust
//! `GET /runs/{job}/events/stream`: the run's events as server-sent events, resumable with
//! `Last-Event-ID` (or `?after=`), read from the same Mongo sequence as the polling endpoint.

use super::{load_run, param};
use crate::api::auth::Caller;
use crate::api::cursor::{decode_event_cursor, encode_event_cursor};
use crate::api::error::ApiError;
use crate::api::project::{self, integer, is_terminal, string_or};
use crate::api::store::ReadModel;
use crate::api::AppState;
use axum::http::{header, HeaderMap, HeaderValue};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use std::collections::VecDeque;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

const POLL_INTERVAL: Duration = Duration::from_millis(250);
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(15);
const BATCH: i64 = 200;

pub(super) async fn events(
    state: &AppState,
    caller: &Caller,
    job_id: &str,
    params: &[(String, String)],
    headers: &HeaderMap,
) -> Result<Response, ApiError> {
    let job = load_run(state, caller, job_id).await?;
    let resume = headers
        .get("last-event-id")
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
        .or_else(|| param(params, "after"));
    let follower = Follower {
        store: state.store.clone(),
        environment: state.config.environment.clone(),
        fleet_filter: state.config.fleet_filter().map(str::to_string),
        fleet_id: string_or(&job, "fleet_id", "undefined"),
        job_id: string_or(&job, "job_id", ""),
        after: decode_event_cursor(resume)?,
        pending: VecDeque::new(),
        done: false,
    };
    let stream = futures::stream::unfold(follower, |mut follower| async move {
        loop {
            if let Some(event) = follower.pending.pop_front() {
                return Some((Ok::<Event, Infallible>(event), follower));
            }
            if follower.done {
                return None;
            }
            follower.poll().await;
        }
    });
    let mut response = Sse::new(stream)
        .keep_alive(KeepAlive::new().interval(KEEPALIVE_INTERVAL).text("keepalive"))
        .into_response();
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    // APISIX is nginx-based; this stops it from buffering the stream.
    headers.insert("x-accel-buffering", HeaderValue::from_static("no"));
    Ok(response)
}

/// Follows one run's events from a cursor until a gap, the run's end, or a read failure.
struct Follower {
    store: Arc<dyn ReadModel>,
    environment: String,
    fleet_filter: Option<String>,
    fleet_id: String,
    job_id: String,
    after: u64,
    pending: VecDeque<Event>,
    done: bool,
}

impl Follower {
    async fn poll(&mut self) {
        let earliest = match self.store.earliest_event_sequence(&self.fleet_id, &self.job_id).await {
            Ok(earliest) => earliest.unwrap_or(0),
            Err(error) => return self.fail(&error.to_string()),
        };
        if earliest > self.after + 1 {
            return self.finish(gap(earliest));
        }
        let rows = match self
            .store
            .events_after(&self.fleet_id, &self.job_id, self.after, BATCH)
            .await
        {
            Ok(rows) => rows,
            Err(error) => return self.fail(&error.to_string()),
        };
        if rows.is_empty() {
            match self
                .store
                .job(&self.environment, self.fleet_filter.as_deref(), &self.job_id)
                .await
            {
                Ok(Some(job)) if is_terminal(&string_or(&job, "state", "")) => {
                    let data = json!({
                        "state": string_or(&job, "state", ""),
                        "next_cursor": encode_event_cursor(self.after),
                    });
                    self.finish(Event::default().event("terminal").data(data.to_string()));
                }
                Ok(_) => tokio::time::sleep(POLL_INTERVAL).await,
                Err(error) => self.fail(&error.to_string()),
            }
            return;
        }
        for row in rows {
            let sequence = integer(row.get("sequence"), 0).max(0) as u64;
            if sequence > self.after + 1 {
                return self.finish(gap(sequence));
            }
            let event = Event::default()
                .id(encode_event_cursor(sequence).unwrap_or_default())
                .event("run_event")
                .data(project::public_event(&row).to_string());
            self.pending.push_back(event);
            self.after = sequence;
        }
    }

    fn finish(&mut self, event: Event) {
        self.pending.push_back(event);
        self.done = true;
    }

    fn fail(&mut self, error: &str) {
        tracing::warn!(job_id = %self.job_id, %error, "event stream read failed");
        let data = json!({ "code": "storage_unavailable" });
        self.finish(Event::default().event("error").data(data.to_string()));
    }
}

fn gap(earliest: u64) -> Event {
    let data = json!({ "earliest_cursor": encode_event_cursor(earliest) });
    Event::default().event("gap").data(data.to_string())
}
```

In `src/api/handlers/mod.rs` add `mod stream;` and, at the top of `run_action` right after `let action = rest.trim_start_matches('/');`:

```rust
    if action == "events/stream" {
        if request.method() != Method::GET {
            return Err(method_not_allowed());
        }
        return stream::events(&state, &caller, &job_id, &params, request.headers()).await;
    }
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --lib api::`
Expected: PASS (the keepalive test runs on paused time and finishes in milliseconds).

- [ ] **Step 5: Commit**

```bash
cargo fmt
git add src/api/handlers/mod.rs src/api/handlers/stream.rs src/api/handlers/tests.rs
git commit -m "feat(api): stream run events over SSE with resume, gap and terminal frames

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 12a: Dashboard accepts live input for API-started runs

Ruling 2026-10-10: runs started through `agentctrl-api` carry `source = "agentctrl_api"`; the dashboard must allow
input for them like it does for dashboard and WarpSpeed runs.

**Files (dashboard worktree `T:/projects/SOTZ3N/stoz3n-agent-dash-p1-golden`, same branch as Task 12):**
- Modify: `server/agent-control.ts:659`
- Test: `scripts/check-agent-control.mjs`

- [ ] **Step 1: Failing check** — append to `scripts/check-agent-control.mjs`, next to the existing `publicRun`/`input_allowed` checks (reuse the script's running-job fixture and its node snapshot helper):

```js
await check("runs started through agentctrl-api accept live input", async () => {
  const run = publicRun({ ...runningInteractiveJob, source: "agentctrl_api" }, connectedSnapshot);
  assert.equal(run.input_allowed, true);
});
```
Use the script's actual names for the running interactive job fixture and snapshot; if `publicRun` is not exported, export it from `server/agent-control.ts` for the check.

- [ ] **Step 2:** `yarn test:agent-control` → the new check fails.
- [ ] **Step 3: Fix** — `server/agent-control.ts:659`:

```ts
  const managedOperatorSource = ["dashboard", "warpspeed", "agentctrl_api"].includes(String(job.source ?? ""));
```

- [ ] **Step 4:** `yarn check && yarn test:agent-control` → all pass.
- [ ] **Step 5: Commit** — `git add server/agent-control.ts scripts/check-agent-control.mjs && git commit -m "fix(agent-control): accept input for runs started through agentctrl-api"` (trailer as in Global Constraints).

### Task 12: Dashboard golden responses and the contract test

The dashboard's `scripts/check-agent-control.mjs` asserts behavior but stores no responses. This task moves its fixture data into shared JSON, has the dashboard export its actual responses for a fixed case list, and replays them against the API.

**Files (dashboard worktree `T:/projects/SOTZ3N/stoz3n-agent-dash-p1-golden`):**
- Create: `scripts/fixtures/agent-control/records.json`, `scripts/fixtures/agent-control/cases.json`, `scripts/export-agent-control-golden.mjs`, `scripts/fixtures/agent-control/golden.json` (generated)
- Modify: `package.json` (`scripts`)

**Files (agent-ctrl):**
- Create: `tests/fixtures/agent_control/{records,cases,golden}.json` (copies), `src/api/contract_tests.rs`
- Modify: `src/api/mod.rs` (`#[cfg(test)] mod contract_tests;`)

**Interfaces:**
- Consumes: `fakes::{harness_with, send, admin_token}`, `MemoryReadModel::load_records` (`{"$offset_ms": n}` dates), `FakeControl` (same request-id script as the dashboard fake), `cursor::BASE64URL`.
- Produces: `golden.json` = `{ "<case name>": { "status": <u16>, "body": <normalized JSON> } }`. Normalization (both sides): string values under keys ending in `_at` or named `timestamp`, `last_seen`, `controller_last_seen` become `"<timestamp>"`; a `next_cursor` that decodes to a page cursor becomes `"<page:<id>>"`. The Rust side also drops the API-only keys `owner_sub`, `owner_email`.

- [ ] **Step 1: Shared fixture records (dashboard)**

`scripts/fixtures/agent-control/records.json` — the data of `scripts/check-agent-control.mjs:12-200`, dates as offsets from "now":

```json
{
  "fleet_snapshots": [
    {
      "_id": "fleet-staging",
      "fleet_id": "fleet-staging",
      "environment": "staging",
      "controller_version": "test",
      "control_api_version": 2,
      "last_seen": { "$offset_ms": 0 },
      "agents": [
        {
          "name": "staging-pi-inspector",
          "backend": "pi",
          "visible": true,
          "launchable": true,
          "interactive_capable": true,
          "allow_interactive": true,
          "max_concurrent": 1
        }
      ]
    }
  ],
  "nodes": [
    { "fleet_id": "fleet-staging", "node_id": "node-1", "node_name": "worker", "connected": true, "reconciling": false, "last_seen": { "$offset_ms": 0 } }
  ],
  "jobs": [
    {
      "job_id": "job-new", "fleet_id": "fleet-staging", "environment": "staging",
      "agent_name": "staging-pi-inspector", "backend": "pi", "node": "worker", "agent_id": "agent-1",
      "source": "dashboard", "interactive": true, "state": "running", "live_state": "waiting_input",
      "session_protocol_version": 1, "last_event_sequence": 4, "task_id": "task-1", "task_ref": "#42",
      "task_title": "Fix login", "task_room_id": "!room:stozn.ai", "created_at": { "$offset_ms": -1000 },
      "error": "authorization: Bearer should-not-leak", "result_tail": "api_key=should-not-leak"
    },
    {
      "job_id": "job-old", "fleet_id": "fleet-staging", "environment": "staging",
      "agent_name": "staging-pi-inspector", "backend": "pi", "node": "worker", "source": "schedule",
      "interactive": false, "state": "completed", "created_at": { "$offset_ms": -2000 }
    },
    {
      "job_id": "job-repo", "fleet_id": "fleet-staging", "environment": "staging",
      "agent_name": "staging-claude-interactive-rw", "backend": "claude", "node": "worker",
      "source": "warpspeed", "interactive": true, "state": "completed",
      "session_id": "0b6c1234-5678-90ab-cdef-1234567890ab", "repo_url": "https://gitlab.com/pseekoo/kaizen.git",
      "branch": "agent/job-repo", "history_event_count": 2, "created_at": { "$offset_ms": -3000 }
    },
    {
      "job_id": "job-pausable", "fleet_id": "fleet-staging", "environment": "staging",
      "agent_name": "staging-pi-inspector", "backend": "claude", "node": "agent-ctrl-node-0",
      "agent_id": "agent-2", "session_id": "0b6c1d2e-0000-4000-8000-000000000001",
      "branch": "agent/job-pausable", "source": "dashboard", "interactive": true, "state": "running",
      "live_state": "running", "session_protocol_version": 1, "created_at": { "$offset_ms": -4000 }
    },
    {
      "job_id": "job-nobranch", "fleet_id": "fleet-staging", "environment": "staging",
      "agent_name": "staging-pi-inspector", "backend": "claude", "node": "agent-ctrl-node-0",
      "agent_id": "agent-5", "session_id": "0b6c1d2e-0000-4000-8000-000000000003", "source": "dashboard",
      "interactive": true, "state": "running", "live_state": "running", "session_protocol_version": 1,
      "created_at": { "$offset_ms": -4500 }
    },
    {
      "job_id": "job-paused", "fleet_id": "fleet-staging", "environment": "staging",
      "agent_name": "staging-pi-inspector", "backend": "claude", "node": "agent-ctrl-node-1",
      "agent_id": "agent-4", "session_id": "0b6c1d2e-0000-4000-8000-000000000002", "source": "dashboard",
      "interactive": true, "state": "paused", "live_state": "waiting_input", "paused_at": { "$offset_ms": -500 },
      "session_protocol_version": 1, "created_at": { "$offset_ms": -5000 }
    },
    {
      "job_id": "job-worker", "fleet_id": "fleet-staging", "environment": "staging",
      "agent_name": "staging-pi-inspector", "backend": "pi", "node": "agent-ctrl-node-0", "agent_id": "agent-3",
      "source": "schedule", "interactive": false, "state": "running", "live_state": "running",
      "session_protocol_version": 1, "created_at": { "$offset_ms": -6000 }
    }
  ],
  "job_history": [
    { "_id": "history-1", "fleet_id": "fleet-staging", "job_id": "job-repo", "position": 1, "timestamp": { "$offset_ms": -20000 }, "type": "user_input", "content": "Earlier question", "redacted": false, "truncated": false },
    { "_id": "history-2", "fleet_id": "fleet-staging", "job_id": "job-repo", "position": 2, "timestamp": { "$offset_ms": -19000 }, "type": "assistant", "content": "Earlier answer", "redacted": false, "truncated": false }
  ],
  "job_events": [
    { "_id": "event-3", "fleet_id": "fleet-staging", "job_id": "job-new", "agent_id": "agent-1", "sequence": 3, "timestamp": { "$offset_ms": 0 }, "type": "assistant", "content": "password=hunter2", "metadata": { "rawCommand": "cat /run/secrets/token", "reasoning": "private" } },
    { "_id": "event-4", "fleet_id": "fleet-staging", "job_id": "job-new", "agent_id": "agent-1", "sequence": 4, "timestamp": { "$offset_ms": 0 }, "type": "status", "content": "waiting_input", "metadata": {} }
  ],
  "control_commands": []
}
```

`scripts/fixtures/agent-control/cases.json`:

```json
[
  { "name": "overview", "method": "GET", "path": "/api/agent-control/overview" },
  { "name": "runs_page", "method": "GET", "path": "/api/agent-control/runs?limit=2" },
  { "name": "runs_bad_state", "method": "GET", "path": "/api/agent-control/runs?state=bogus" },
  { "name": "runs_by_agent", "method": "GET", "path": "/api/agent-control/runs?agent_name=staging-claude-interactive-rw" },
  { "name": "run_detail", "method": "GET", "path": "/api/agent-control/runs/job-new" },
  { "name": "run_paused", "method": "GET", "path": "/api/agent-control/runs/job-paused" },
  { "name": "run_missing", "method": "GET", "path": "/api/agent-control/runs/job-missing" },
  { "name": "run_bad_id", "method": "GET", "path": "/api/agent-control/runs/not-a-job" },
  { "name": "run_unknown_action", "method": "GET", "path": "/api/agent-control/runs/job-new/bogus" },
  { "name": "run_delete", "method": "DELETE", "path": "/api/agent-control/runs/job-new" },
  { "name": "session", "method": "GET", "path": "/api/agent-control/runs/job-new/session" },
  { "name": "session_pausable", "method": "GET", "path": "/api/agent-control/runs/job-pausable/session" },
  { "name": "events", "method": "GET", "path": "/api/agent-control/runs/job-new/events" },
  { "name": "events_gap", "method": "GET", "path": "/api/agent-control/runs/job-new/events?after=djE6MQ" },
  { "name": "events_bad_cursor", "method": "GET", "path": "/api/agent-control/runs/job-new/events?after=nope" },
  { "name": "history", "method": "GET", "path": "/api/agent-control/runs/job-repo/history" },
  { "name": "repos", "method": "GET", "path": "/api/agent-control/repos" },
  { "name": "commands", "method": "GET", "path": "/api/agent-control/commands" },
  { "name": "input", "method": "POST", "path": "/api/agent-control/runs/job-new/input", "idempotency_key": "input-request-0001", "json": { "text": "hello" } },
  { "name": "input_no_key", "method": "POST", "path": "/api/agent-control/runs/job-new/input", "json": { "text": "hello" } },
  { "name": "input_wrong_media", "method": "POST", "path": "/api/agent-control/runs/job-new/input", "idempotency_key": "input-request-0002", "content_type": "text/plain", "json": { "text": "hello" } },
  { "name": "input_unknown_field", "method": "POST", "path": "/api/agent-control/runs/job-new/input", "idempotency_key": "input-request-0003", "json": { "text": "x", "extra": 1 } },
  { "name": "input_conflict", "method": "POST", "path": "/api/agent-control/runs/job-new/input", "idempotency_key": "conflict-request-01", "json": { "text": "x" } },
  { "name": "input_rejected", "method": "POST", "path": "/api/agent-control/runs/job-new/input", "idempotency_key": "rejected-request-001", "json": { "text": "x" } },
  { "name": "input_not_allowed", "method": "POST", "path": "/api/agent-control/runs/job-pausable/input", "idempotency_key": "input-request-0004", "json": { "text": "x" } },
  { "name": "stop_unavailable", "method": "POST", "path": "/api/agent-control/runs/job-new/stop", "idempotency_key": "unavailable-request-1", "json": { "force": false } },
  { "name": "stop_force", "method": "POST", "path": "/api/agent-control/runs/job-new/stop", "idempotency_key": "stop-request-000001", "json": { "force": true } },
  { "name": "pause", "method": "POST", "path": "/api/agent-control/runs/job-pausable/pause", "idempotency_key": "pause-request-00001", "json": {} },
  { "name": "pause_not_interactive", "method": "POST", "path": "/api/agent-control/runs/job-worker/pause", "idempotency_key": "pause-request-00002", "json": {} },
  { "name": "resume", "method": "POST", "path": "/api/agent-control/runs/job-paused/resume", "idempotency_key": "resume-request-0001", "json": {} },
  { "name": "resume_terminal", "method": "POST", "path": "/api/agent-control/runs/job-old/resume", "idempotency_key": "resume-request-0002", "json": {} },
  { "name": "launch_repo", "method": "POST", "path": "/api/agent-control/runs", "idempotency_key": "launch-request-0001", "json": { "agent_name": "staging-claude-interactive-rw", "interactive": true, "repo_url": "https://gitlab.com/pseekoo/kaizen.git" } },
  { "name": "launch_at_capacity", "method": "POST", "path": "/api/agent-control/runs", "idempotency_key": "atcapacity-request-1", "json": { "agent_name": "staging-claude-interactive-rw", "interactive": true, "repo_url": "https://gitlab.com/pseekoo/kaizen.git" } },
  { "name": "launch_bad_branch", "method": "POST", "path": "/api/agent-control/runs", "idempotency_key": "launch-request-0002", "json": { "agent_name": "staging-claude-interactive-rw", "interactive": true, "repo_url": "https://gitlab.com/pseekoo/kaizen.git", "base_branch": "../dev" } }
]
```

- [ ] **Step 2: Export script (dashboard)**

`scripts/export-agent-control-golden.mjs`:

```js
// Exports this dashboard's /api/agent-control responses for the shared fixtures in
// scripts/fixtures/agent-control/ as golden JSON. AgentCtrl's public run API replays them
// (stoz3n-agent-ctrl src/api/contract_tests.rs) to keep both implementations of the contract equal.
// Run: yarn golden:agent-control [out-file]
import { Buffer } from "node:buffer";
import { readFileSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import * as grpc from "@grpc/grpc-js";
import * as protoLoader from "@grpc/proto-loader";
import { agentControlError, handleAgentControlApi } from "../dist/server/agent-control.js";

const here = path.dirname(fileURLToPath(import.meta.url));
const fixtures = path.join(here, "fixtures/agent-control");
const now = Date.now();

function revive(value) {
  if (Array.isArray(value)) return value.map(revive);
  if (value && typeof value === "object") {
    const keys = Object.keys(value);
    if (keys.length === 1 && keys[0] === "$offset_ms") return new Date(now + value.$offset_ms);
    return Object.fromEntries(Object.entries(value).map(([key, item]) => [key, revive(item)]));
  }
  return value;
}

const records = revive(JSON.parse(readFileSync(path.join(fixtures, "records.json"), "utf8")));
const cases = JSON.parse(readFileSync(path.join(fixtures, "cases.json"), "utf8"));

// The same in-memory Mongo semantics as scripts/check-agent-control.mjs.
function comparable(value) {
  return value instanceof Date ? value.getTime() : value;
}
function matches(row, filter = {}) {
  for (const [key, expected] of Object.entries(filter)) {
    if (key === "$or") {
      if (!expected.some((part) => matches(row, part))) return false;
      continue;
    }
    const actual = row[key];
    if (expected && typeof expected === "object" && !(expected instanceof Date)) {
      if ("$gt" in expected && !(comparable(actual) > comparable(expected.$gt))) return false;
      if ("$lt" in expected && !(comparable(actual) < comparable(expected.$lt))) return false;
      continue;
    }
    if (comparable(actual) !== comparable(expected)) return false;
  }
  return true;
}
class Cursor {
  constructor(rows) {
    this.rows = [...rows];
  }
  sort(spec) {
    const entries = Object.entries(spec);
    this.rows.sort((a, b) => {
      for (const [field, direction] of entries) {
        const av = comparable(a[field]);
        const bv = comparable(b[field]);
        if (av < bv) return -1 * direction;
        if (av > bv) return 1 * direction;
      }
      return 0;
    });
    return this;
  }
  limit(count) {
    this.rows = this.rows.slice(0, count);
    return this;
  }
  async toArray() {
    return this.rows;
  }
  async next() {
    return this.rows[0] ?? null;
  }
}
class FakeCollection {
  constructor(rows) {
    this.rows = rows;
  }
  find(filter = {}) {
    return new Cursor(this.rows.filter((row) => matches(row, filter)));
  }
  async findOne(filter = {}) {
    return this.rows.find((row) => matches(row, filter)) ?? null;
  }
}
const db = { collection: (name) => new FakeCollection(records[name] ?? []) };

// FleetControl scripted by request-id prefix, as in scripts/check-agent-control.mjs.
const definition = protoLoader.loadSync(path.join(here, "../server/agentctrl/v1.proto"), {
  keepCase: true, longs: String, enums: Number, defaults: true, oneofs: true,
});
const fleetControl = grpc.loadPackageDefinition(definition).agentctrl.v1.FleetControl;
function mutation(call, callback) {
  const id = String(call.request.request_id ?? "");
  if (id.startsWith("conflict")) return callback({ code: grpc.status.ALREADY_EXISTS, details: "request fingerprint mismatch" });
  if (id.startsWith("unavailable")) return callback({ code: grpc.status.UNAVAILABLE, details: "down" });
  if (id.startsWith("atcapacity")) return callback({ code: grpc.status.RESOURCE_EXHAUSTED, message: "node at capacity" });
  if (id.startsWith("sessionunknown")) return callback({ code: grpc.status.FAILED_PRECONDITION, message: "session id unknown" });
  if (id.startsWith("notstate")) return callback({ code: grpc.status.FAILED_PRECONDITION, details: "job is not paused" });
  if (id.startsWith("notinteractive")) return callback({ code: grpc.status.FAILED_PRECONDITION, details: "agent does not permit operator interaction" });
  if (id.startsWith("rejected")) return callback(null, { success: false, error: "policy denied", replayed: false });
  callback(null, {
    success: true,
    agent_id: call.request.agent_id || "agent-launched",
    job_id: "job-new",
    replayed: id.startsWith("replay"),
    history_event_count: call.request.session_file ? 2 : 0,
  });
}
function controlRpc(state, nodeName) {
  return (call, callback) => mutation(call, (error, value) => callback(error, value && {
    job_id: value.job_id, state, accepted: value.success === true, node_name: nodeName,
    error: value.error ?? "", request_id: call.request.request_id, replayed: value.replayed,
  }));
}
const server = new grpc.Server();
server.addService(fleetControl.service, {
  SpawnAgent: mutation,
  SendInput: mutation,
  StopAgent: mutation,
  ImportSession: mutation,
  ImportSessionHistory: mutation,
  PauseAgent: controlRpc("paused", ""),
  ResumeAgent: controlRpc("running", "agent-ctrl-node-1"),
  ListRepositories: (_call, callback) => callback(null, {
    repositories: [{ name: "kaizen", url: "https://gitlab.com/pseekoo/kaizen.git", branch: "main", reachable: true, error: "" }],
  }),
});
const port = await new Promise((resolve, reject) => {
  server.bindAsync("127.0.0.1:0", grpc.ServerCredentials.createInsecure(), (error, bound) => (error ? reject(error) : resolve(bound)));
});
const config = {
  endpoint: `127.0.0.1:${port}`,
  token: "control-token",
  environment: "staging",
  fleetId: "fleet-staging",
  mutationsEnabled: true,
};
const claims = { sub: "operator@example.test", role: "admin" };

const TIMESTAMP_KEYS = new Set(["timestamp", "last_seen", "controller_last_seen"]);
function normalize(value, key = "") {
  if (Array.isArray(value)) return value.map((item) => normalize(item));
  if (value && typeof value === "object") {
    return Object.fromEntries(Object.entries(value).map(([name, item]) => [name, normalize(item, name)]));
  }
  if (typeof value === "string" && (key.endsWith("_at") || TIMESTAMP_KEYS.has(key))) return "<timestamp>";
  if (typeof value === "string" && key === "next_cursor") {
    try {
      const parsed = JSON.parse(Buffer.from(value, "base64url").toString("utf8"));
      if (parsed && parsed.v === 1 && typeof parsed.id === "string") return `<page:${parsed.id}>`;
    } catch {
      // An event cursor ("v1:<n>") is not JSON and stays as it is.
    }
  }
  return value;
}

const golden = {};
for (const testCase of cases) {
  const headers = {};
  if (testCase.idempotency_key) headers["idempotency-key"] = testCase.idempotency_key;
  let body;
  if (testCase.json !== undefined) {
    headers["content-type"] = testCase.content_type ?? "application/json";
    body = JSON.stringify(testCase.json);
  }
  const request = new Request(`https://dash.example${testCase.path}`, { method: testCase.method, headers, body });
  let response;
  try {
    response = await handleAgentControlApi(request, new URL(request.url), claims, config, db, async () => null, new Headers());
  } catch (error) {
    response = agentControlError(error, new Headers());
  }
  if (!response) throw new Error(`${testCase.name}: not an agent-control path`);
  golden[testCase.name] = { status: response.status, body: normalize(await response.json()) };
}
const out = process.argv[2] ?? path.join(fixtures, "golden.json");
writeFileSync(out, `${JSON.stringify(golden, null, 2)}\n`);
server.forceShutdown();
console.log(`wrote ${Object.keys(golden).length} golden responses to ${out}`);
```

In `package.json` `scripts`, after `"test:agent-control"` add:

```json
    "golden:agent-control": "yarn build:server && node scripts/export-agent-control-golden.mjs",
```

- [ ] **Step 3: Generate and commit (dashboard)**

```bash
cd T:/projects/SOTZ3N/stoz3n-agent-dash-p1-golden
yarn install --frozen-lockfile
yarn golden:agent-control
yarn test:agent-control
```

Expected: `wrote 34 golden responses to …/golden.json`; the existing check still passes. Spot-check `golden.json`: `run_detail.status` 200 with `"error": "authorization: Bearer [REDACTED]"`, `events_gap.status` 410, `input.status` 202, `pause.body.node_name` `"agent-ctrl-node-0"`, `launch_bad_branch.status` 422.

```bash
git add scripts/fixtures/agent-control/records.json scripts/fixtures/agent-control/cases.json scripts/fixtures/agent-control/golden.json scripts/export-agent-control-golden.mjs package.json
git commit -m "test(agent-control): export golden responses for the shared fixtures

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
git push -u origin test/agent-control-golden-export
```

Open a Gitea PR to `dev` (test-only change; no deploy impact).

- [ ] **Step 4: Write the failing contract test (agent-ctrl)**

```bash
cd T:/projects/SOTZ3N/stoz3n-agent-ctrl-p1-api
mkdir -p tests/fixtures/agent_control
cp T:/projects/SOTZ3N/stoz3n-agent-dash-p1-golden/scripts/fixtures/agent-control/records.json tests/fixtures/agent_control/
cp T:/projects/SOTZ3N/stoz3n-agent-dash-p1-golden/scripts/fixtures/agent-control/cases.json tests/fixtures/agent_control/
cp T:/projects/SOTZ3N/stoz3n-agent-dash-p1-golden/scripts/fixtures/agent-control/golden.json tests/fixtures/agent_control/
```

Add to `src/api/mod.rs` after the `fakes` declaration:

```rust
#[cfg(test)]
mod contract_tests;
```

Create `src/api/contract_tests.rs`:

```rust
//! Replays the dashboard's golden agent-control responses (exported by stoz3n-agent-dash
//! `scripts/export-agent-control-golden.mjs` from the same records and cases) against this API.

use super::cursor::BASE64URL;
use super::fakes::{self, admin_token};
use axum::body::Body;
use axum::http::Request;
use base64::Engine as _;
use serde_json::{json, Map, Value};

const RECORDS: &str = include_str!("../../tests/fixtures/agent_control/records.json");
const CASES: &str = include_str!("../../tests/fixtures/agent_control/cases.json");
const GOLDEN: &str = include_str!("../../tests/fixtures/agent_control/golden.json");
/// Run fields only the public API returns.
const API_ONLY_FIELDS: [&str; 2] = ["owner_sub", "owner_email"];
const TIMESTAMP_KEYS: [&str; 3] = ["timestamp", "last_seen", "controller_last_seen"];

fn page_cursor_id(value: &str) -> Option<String> {
    let parsed: Value = serde_json::from_slice(&BASE64URL.decode(value).ok()?).ok()?;
    (parsed.get("v") == Some(&json!(1)))
        .then(|| parsed.get("id")?.as_str().map(str::to_string))
        .flatten()
}

fn normalize(value: &Value, key: &str) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.iter().map(|item| normalize(item, "")).collect()),
        Value::Object(map) => Value::Object(
            map.iter()
                .filter(|(name, _)| !API_ONLY_FIELDS.contains(&name.as_str()))
                .map(|(name, item)| (name.clone(), normalize(item, name)))
                .collect(),
        ),
        Value::String(_) if key.ends_with("_at") || TIMESTAMP_KEYS.contains(&key) => {
            Value::String("<timestamp>".into())
        }
        Value::String(text) if key == "next_cursor" => page_cursor_id(text)
            .map_or_else(|| value.clone(), |id| Value::String(format!("<page:{id}>"))),
        _ => value.clone(),
    }
}

fn request(case: &Value, token: &str) -> Request<Body> {
    let mut builder = Request::builder()
        .method(case["method"].as_str().expect("method"))
        .uri(case["path"].as_str().expect("path"))
        .header("authorization", format!("Bearer {token}"));
    if let Some(key) = case["idempotency_key"].as_str() {
        builder = builder.header("idempotency-key", key);
    }
    match case.get("json") {
        Some(body) => builder
            .header("content-type", case["content_type"].as_str().unwrap_or("application/json"))
            .body(Body::from(body.to_string()))
            .unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    }
}

#[tokio::test]
async fn responses_match_the_dashboard_golden_fixtures() {
    let harness = fakes::harness();
    harness.store.load_records(
        &serde_json::from_str(RECORDS).expect("records.json"),
        chrono::Utc::now().timestamp_millis(),
    );
    let cases: Vec<Value> = serde_json::from_str(CASES).expect("cases.json");
    let golden: Map<String, Value> = serde_json::from_str(GOLDEN).expect("golden.json");
    assert_eq!(golden.len(), cases.len(), "golden.json and cases.json are out of sync");
    let token = admin_token();
    let mut mismatches = Vec::new();
    for case in &cases {
        let name = case["name"].as_str().expect("name");
        let (status, _, body) = fakes::send(&harness.app, request(case, &token)).await;
        let expected = &golden[name];
        let actual = json!({ "status": status.as_u16(), "body": normalize(&body, "") });
        let wanted = json!({ "status": expected["status"], "body": normalize(&expected["body"], "") });
        if actual != wanted {
            mismatches.push(format!("{name}\n  dashboard: {wanted}\n  api:       {actual}"));
        }
    }
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
}
```

`fakes::harness()` uses `fleet_id = "fleet-staging"`, environment `staging`, mutations enabled — the same config as the export script.

- [ ] **Step 5: Run it**

Run: `cargo test --lib api::contract_tests -- --nocapture`
Expected: PASS. A mismatch prints both bodies; fix the API side (never edit `golden.json` by hand — regenerate it from the dashboard if the dashboard changed).

- [ ] **Step 6: Commit**

```bash
git add src/api/mod.rs src/api/contract_tests.rs tests/fixtures/agent_control/records.json tests/fixtures/agent_control/cases.json tests/fixtures/agent_control/golden.json
git commit -m "test(api): replay the dashboard's golden agent-control responses

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 13: Staging manifests, NetworkPolicy, CI rollout, route spec

**Files:**
- Create: `deploy/k8s/templates/api-deployment.tmpl.yaml`, `deploy/k8s/templates/api-service.tmpl.yaml`, `deploy/k8s/templates/api-networkpolicy.tmpl.yaml`, `deploy/k8s/agentctrl-api.obachan.app.toml`
- Modify: `deploy/k8s/render.py` (`_ORDER` 43-60, `_DEFAULTS` 63-164, `load_instance` 356-372, `_subs` 538-620, `_skipped_resources` 623-635, `main` 693-750), `deploy/k8s/templates/networkpolicy.tmpl.yaml` (last line), `deploy/k8s/instances/staging.toml`, `deploy/k8s/test_shared_workspaces.py` (`RenderTests`, line 162-181), `.gitea/workflows/deploy.yml` (after the node `rollout status`)

**Interfaces:**
- Instance keys: `api_enabled` (bool, default false), `api_replicas` (2), `api_hostname`, `api_zitadel_issuer`, `api_zitadel_project_id` (required when enabled), `api_ingress_namespace` (`apisix`).
- `render.py --instance <i> --api-enabled` prints `true`/`false`.
- Resources (staging only): Service and Deployment `agentctrl-api` (port 8080, labels `app.kubernetes.io/name: agent-ctrl`, `app.kubernetes.io/component: api`), NetworkPolicy `agentctrl-api-ingress`; the controller policy gains an ingress rule from the API pods on 50051.

The API's Mongo access needs no policy of its own: the namespace has no default-deny egress (only ingress policies exist), and Mongo is the external mongo-gw. The API reads only the four Mongo keys of `agent-ctrl-runtime`, never its model or Git credentials, and the control credential, never the node credential.

- [ ] **Step 1: Write the failing tests**

In `deploy/k8s/test_shared_workspaces.py`, `RenderTests.test_controller_network_policy_is_default_deny_with_staging_dashboard_allow`, change `self.assertEqual(policy.count("port: 50051"), 2)` to `3` and add after it `self.assertIn("app.kubernetes.io/component: api", policy)`.

Add to `RenderTests`:

```python
    def test_staging_serves_the_public_api_behind_apisix_only(self):
        staging = dict(render.render(render.load_instance("staging", "test-image")))
        prod = dict(render.render(render.load_instance("prod", "test-image")))
        api = staging["api-deployment"]
        self.assertIn("name: agentctrl-api", api)
        self.assertIn("replicas: 2", api)
        self.assertIn("exec agent-ctrl api --bind 0.0.0.0:8080", api)
        self.assertIn('value: "https://id-dev.obachan.dev"', api)
        self.assertIn('value: "394361582064240430"', api)
        self.assertIn('value: "stoz3n-agent-ctrl-staging"', api)
        self.assertIn('value: "http://agent-ctrl-controller.agent-ctrl.svc.cluster.local:50051"', api)
        self.assertIn("name: AGENT_CTRL_CONTROL_AUTH_TOKEN", api)
        self.assertNotIn("AGENT_CTRL_NODE_AUTH_TOKEN", api)
        self.assertNotIn("envFrom", api, "only the Mongo keys of the runtime Secret are projected")
        for key in ("AGENT_CTRL_MONGO_URI", "AGENT_CTRL_MONGO_DB", "MONGODB_CA_PEM_B64", "MONGODB_CLIENT_PEM_B64"):
            self.assertIn(f"key: {key}", api)
        self.assertIn("path: /healthz", api)
        self.assertIn("readOnlyRootFilesystem: true", api)
        service = staging["api-service"]
        self.assertIn("port: 8080", service)
        self.assertIn("agents-dev.obachan.dev", service)
        policy = staging["api-networkpolicy"]
        self.assertIn("kubernetes.io/metadata.name: apisix", policy)
        self.assertIn("port: 8080", policy)
        self.assertNotIn("ipBlock:", policy)
        for resource in ("api-deployment", "api-service", "api-networkpolicy"):
            self.assertNotIn(resource, prod)
        self.assertNotIn("app.kubernetes.io/component: api", prod["networkpolicy"])

    def test_an_enabled_api_needs_its_identity_settings(self):
        cfg = render.load_instance("staging", "test-image")
        render._validate_api(cfg)
        for key in ("api_hostname", "api_zitadel_issuer", "api_zitadel_project_id"):
            with self.assertRaises(SystemExit):
                render._validate_api({**cfg, key: ""})
        with self.assertRaises(SystemExit):
            render._validate_api({**cfg, "api_zitadel_issuer": "https://id-dev.obachan.dev/"})
        render._validate_api({**cfg, "api_enabled": False, "api_hostname": ""})

    def test_ci_rolls_out_the_api_only_where_it_is_enabled(self):
        import contextlib
        import io

        for instance, expected in (("staging", "true"), ("prod", "false")):
            out = io.StringIO()
            with contextlib.redirect_stdout(out):
                self.assertEqual(render.main(["--instance", instance, "--api-enabled"]), 0)
            self.assertEqual(out.getvalue().strip(), expected)
        workflow = (Path(__file__).resolve().parents[2] / ".gitea/workflows/deploy.yml").read_text(encoding="utf-8")
        self.assertIn("rollout status deploy/agentctrl-api", workflow)

    def test_the_route_spec_registers_the_apisix_gated_host(self):
        spec = tomllib.loads((Path(__file__).parent / "agentctrl-api.obachan.app.toml").read_text(encoding="utf-8"))
        self.assertEqual(
            (spec["name"], spec["hostname"], spec["gate"], spec["port"], spec["health_path"]),
            ("agentctrl-api", "agents-dev.obachan.dev", "apisix", 8080, "/healthz"),
        )
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd T:/projects/SOTZ3N/stoz3n-agent-ctrl-p1-api && py -3 -m unittest discover -s deploy/k8s -p 'test_*.py'` (CI uses `python3`)
Expected: FAIL (`KeyError: 'api-deployment'`, missing `_validate_api`).

- [ ] **Step 3: Templates**

`deploy/k8s/templates/api-deployment.tmpl.yaml`:

```yaml
# SPDX-License-Identifier: Apache-2.0
# Public run API (`agent-ctrl api`). Reached only through obachan's APISIX bearer-only route for
# {api_hostname}; it verifies every Zitadel token again itself.
apiVersion: apps/v1
kind: Deployment
metadata:
  name: agentctrl-api
  namespace: {namespace}
  labels:
    app.kubernetes.io/name: agent-ctrl
    app.kubernetes.io/component: api
  annotations:
    reloader.stakater.com/auto: "true"
spec:
  replicas: {api_replicas}
  selector:
    matchLabels:
      app.kubernetes.io/name: agent-ctrl
      app.kubernetes.io/component: api
  template:
    metadata:
      labels:
        app.kubernetes.io/name: agent-ctrl
        app.kubernetes.io/component: api
    spec:
      securityContext:
        runAsNonRoot: true
        runAsUser: 10001
        runAsGroup: 10001
        seccompProfile:
          type: RuntimeDefault
{image_pull_secrets_block}
      containers:
        - name: api
          image: "{image}:{image_tag}"
          imagePullPolicy: Always
          command: ["sh", "-c"]
          args:
            - |
              set -eu
              # Materialize the mongo-gw mTLS files the URI references
              # (tlsCAFile=/tmp/mongo-ca.crt, tlsCertificateKeyFile=/tmp/mongo-client.pem).
              if [ -n "${{MONGODB_CA_PEM_B64:-}}" ]; then
                echo "$MONGODB_CA_PEM_B64" | base64 -d > /tmp/mongo-ca.crt
                echo "$MONGODB_CLIENT_PEM_B64" | base64 -d > /tmp/mongo-client.pem
                chmod 600 /tmp/mongo-ca.crt /tmp/mongo-client.pem
              fi
              exec agent-ctrl api --bind 0.0.0.0:8080
          env:
            - name: AGENT_CTRL_CONTROLLER
              value: "http://agent-ctrl-controller.{namespace}.svc.cluster.local:50051"
            - name: AGENT_CTRL_CONTROL_AUTH_TOKEN
              valueFrom:
                secretKeyRef:
                  name: {control_auth_secret_name}
                  key: password
            - name: AGENT_CTRL_ENVIRONMENT
              value: "{instance}"
            - name: AGENT_CTRL_FLEET_ID
              value: "stoz3n-agent-ctrl-{instance}"
            - name: AGENT_CTRL_MANAGED_MUTATIONS_ENABLED
              value: "true"
            - name: AGENT_PLATFORM_ISSUER
              value: "{api_zitadel_issuer}"
            - name: AGENT_PLATFORM_PROJECT_ID
              value: "{api_zitadel_project_id}"
            # Only the Mongo keys of the runtime Secret: the API never sees model or Git credentials.
            - name: AGENT_CTRL_MONGO_URI
              valueFrom:
                secretKeyRef:
                  name: agent-ctrl-runtime
                  key: AGENT_CTRL_MONGO_URI
            - name: AGENT_CTRL_MONGO_DB
              valueFrom:
                secretKeyRef:
                  name: agent-ctrl-runtime
                  key: AGENT_CTRL_MONGO_DB
            - name: MONGODB_CA_PEM_B64
              valueFrom:
                secretKeyRef:
                  name: agent-ctrl-runtime
                  key: MONGODB_CA_PEM_B64
            - name: MONGODB_CLIENT_PEM_B64
              valueFrom:
                secretKeyRef:
                  name: agent-ctrl-runtime
                  key: MONGODB_CLIENT_PEM_B64
            - name: HOME
              value: /tmp
          ports:
            - name: http
              containerPort: 8080
              protocol: TCP
          readinessProbe:
            httpGet:
              path: /healthz
              port: http
            periodSeconds: 10
          livenessProbe:
            httpGet:
              path: /healthz
              port: http
            initialDelaySeconds: 10
            periodSeconds: 20
          resources:
            requests:
              cpu: 20m
              memory: 32Mi
            limits:
              cpu: 500m
              memory: 256Mi
          volumeMounts:
            - name: tmp
              mountPath: /tmp
          securityContext:
            allowPrivilegeEscalation: false
            readOnlyRootFilesystem: true
            capabilities:
              drop: ["ALL"]
      volumes:
        - name: tmp
          emptyDir: {{}}
```

`deploy/k8s/templates/api-service.tmpl.yaml`:

```yaml
# SPDX-License-Identifier: Apache-2.0
apiVersion: v1
kind: Service
metadata:
  name: agentctrl-api
  namespace: {namespace}
  labels:
    app.kubernetes.io/name: agent-ctrl
    app.kubernetes.io/component: api
  annotations:
    agent-ctrl.stozn.ai/public-host: "{api_hostname}"
spec:
  type: ClusterIP
  selector:
    app.kubernetes.io/name: agent-ctrl
    app.kubernetes.io/component: api
  ports:
    - name: http
      port: 8080
      targetPort: http
      protocol: TCP
```

`deploy/k8s/templates/api-networkpolicy.tmpl.yaml`:

```yaml
# SPDX-License-Identifier: Apache-2.0
apiVersion: networking.k8s.io/v1
kind: NetworkPolicy
metadata:
  name: agentctrl-api-ingress
  namespace: {namespace}
  labels:
    app.kubernetes.io/name: agent-ctrl
    app.kubernetes.io/component: api
spec:
  podSelector:
    matchLabels:
      app.kubernetes.io/name: agent-ctrl
      app.kubernetes.io/component: api
  policyTypes:
    - Ingress
  ingress:
    # Only obachan's APISIX data plane reaches the public run API.
    - from:
        - namespaceSelector:
            matchLabels:
              kubernetes.io/metadata.name: {api_ingress_namespace}
      ports:
        - protocol: TCP
          port: 8080
```

In `deploy/k8s/templates/networkpolicy.tmpl.yaml` replace the last line `{dashboard_ingress_block}` with:

```
{dashboard_ingress_block}{api_ingress_block}
```

- [ ] **Step 4: `render.py`**

Append to `_ORDER` after `("controller-deployment.tmpl.yaml", "controller-deployment"),`:

```python
    ("api-service.tmpl.yaml", "api-service"),
    ("api-networkpolicy.tmpl.yaml", "api-networkpolicy"),
    ("api-deployment.tmpl.yaml", "api-deployment"),
```

Add to `_DEFAULTS` (before the closing brace):

```python
    # Public run API (`agent-ctrl api`) behind obachan's APISIX. Off by default: prod has none.
    "api_enabled": False,
    "api_replicas": 2,
    "api_hostname": "",
    "api_zitadel_issuer": "",
    "api_zitadel_project_id": "",
    "api_ingress_namespace": "apisix",
```

Add after `_dashboard_ingress_block`:

```python
def _api_ingress_block(cfg: dict) -> str:
    if not cfg.get("api_enabled"):
        return ""
    return (
        "    # The public run API (agent-ctrl api) calls FleetControl.\n"
        "    - from:\n"
        "        - podSelector:\n"
        "            matchLabels:\n"
        "              app.kubernetes.io/name: agent-ctrl\n"
        "              app.kubernetes.io/component: api\n"
        "      ports:\n"
        "        - protocol: TCP\n"
        "          port: 50051\n"
    )


def _validate_api(cfg: dict) -> None:
    """An enabled public API needs its host and Zitadel identity."""
    if not cfg.get("api_enabled"):
        return
    missing = [k for k in ("api_hostname", "api_zitadel_issuer", "api_zitadel_project_id") if not cfg.get(k)]
    if missing:
        raise SystemExit(f"api_enabled requires {missing}")
    issuer = cfg["api_zitadel_issuer"]
    if not issuer.startswith("https://") or issuer.endswith("/"):
        raise SystemExit("api_zitadel_issuer must be an https URL without a trailing slash")
```

In `load_instance`, before `return cfg` add `_validate_api(cfg)`.

In `_subs`, add `"api_ingress_block": _api_ingress_block(cfg),` after `"dashboard_ingress_block": …,`.

In `_skipped_resources`, before `return skip` add:

```python
    if not cfg.get("api_enabled"):
        skip.update({"api-service", "api-networkpolicy", "api-deployment"})
```

In `main`, add the flag after `--node-rollout-target`:

```python
    ap.add_argument(
        "--api-enabled",
        action="store_true",
        help="print whether the instance runs the public run API (true/false) and exit",
    )
```

and after the `if args.node_rollout_target:` block:

```python
    if args.api_enabled:
        print("true" if cfg.get("api_enabled") else "false")
        return 0
```

- [ ] **Step 5: Instance, workflow and route spec**

Append to `deploy/k8s/instances/staging.toml`:

```toml
# Public run API (agent-ctrl api) for WarpSpeed, behind obachan APISIX's bearer-only route.
# Zitadel staging agent-platform project; roles agent-admin / agent-user.
api_enabled = true
api_replicas = 2
api_hostname = "agents-dev.obachan.dev"
api_zitadel_issuer = "https://id-dev.obachan.dev"
api_zitadel_project_id = "394361582064240430"
api_ingress_namespace = "apisix"
```

In `.gitea/workflows/deploy.yml`, directly after `kubectl -n "$namespace" rollout status "$node_rollout_target" --timeout=600s` add:

```bash
          if [ "$(python3 deploy/k8s/render.py --instance "$instance" --api-enabled)" = true ]; then
            kubectl -n "$namespace" rollout status deploy/agentctrl-api --timeout=300s
          fi
```

Create `deploy/k8s/agentctrl-api.obachan.app.toml`:

```toml
# SPDX-License-Identifier: Apache-2.0
# Caddy route registration for the public run API only, run by an operator with
# `obachan apps routes register` (Task 18). The workload is rendered by render.py; APISIX owns
# auth on this host (obachan-config [[envs.staging.platform.apisix.app_routes]] agentctrl-api).
name = "agentctrl-api"
image = "harbor.pseekoo.io/pseekoo-stoz3n-agent-ctrl/app"
image_tag = "staging"
port = 8080
health_path = "/healthz"
hostname = "agents-dev.obachan.dev"
gate = "apisix"
```

`ci-deployer-rbac.yaml` already grants Services, Deployments and NetworkPolicies in `agent-ctrl`; no RBAC change.

- [ ] **Step 6: Run the tests to verify they pass**

Run: `py -3 -m unittest discover -s deploy/k8s -p 'test_*.py' && py -3 deploy/k8s/render.py --instance staging --image-tag test --print > /dev/null && py -3 deploy/k8s/render.py --instance prod --image-tag test --print > /dev/null && node --test deploy/k8s/gitlab-scoped.test.cjs`
Expected: all PASS; both renders succeed. Visually check `py -3 deploy/k8s/render.py --instance staging --image-tag test --print | sed -n '/name: agentctrl-api-ingress/,/port: 8080/p'`.

- [ ] **Step 7: Commit**

```bash
git add deploy/k8s/render.py deploy/k8s/instances/staging.toml deploy/k8s/test_shared_workspaces.py deploy/k8s/templates/networkpolicy.tmpl.yaml deploy/k8s/templates/api-deployment.tmpl.yaml deploy/k8s/templates/api-service.tmpl.yaml deploy/k8s/templates/api-networkpolicy.tmpl.yaml deploy/k8s/agentctrl-api.obachan.app.toml .gitea/workflows/deploy.yml
git commit -m "deploy(api): run agentctrl-api on staging behind APISIX only

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 14: Model spend attribution (branches on Task 0)

**Precondition:** `docs/p1-litellm-attribution-spike.md` exists with `MECHANISM`. If `MECHANISM: none`, skip this task and report the gap.

Pick the variant from the spike:

| `MECHANISM` | `OWNER_ATTRIBUTION_HEADER` | `owner_attribution(email)` | LiteLLM change |
|---|---|---|---|
| `tags-header` | `"x-litellm-tags"` | `format!("owner:{email}")` | none |
| `end-user-header` | `"x-stozn-owner"` | `email.to_string()` | passiv `general_settings.user_header_name: x-stozn-owner` merged and deployed (Task 0 Step 4) |

Per CLI: **claude** is always wired here (it is the spec's required case). **codex** gets Step 5 only if its spike row is PASS. **pi, kimi, dsh** stay untagged in P1 unless their rows are PASS; a PASS row for any of them is a plan amendment (a new task modeled on Step 5 using the setting recorded in the evidence file) made before this task starts, never improvised during it.

**Files:**
- Modify: `src/workspace_handoff.rs` (env name), `src/remote/controller.rs` (`apply_owner_env`), `deploy/k8s/render.py` (`_codex_config`, `_DEFAULTS`), `deploy/k8s/instances/staging.toml`, `deploy/k8s/test_shared_workspaces.py`

**Interfaces:**
- Consumes: `apply_owner_env` (Task 2).
- Produces: env `AGENT_CTRL_OWNER_ATTRIBUTION` (= `owner_attribution(owner_email)`) and `ANTHROPIC_CUSTOM_HEADERS` gaining `"<header>: <value>"` on every owned run with an email; instance key `owner_attribution_header` (default `""`).

- [ ] **Step 1: Write the failing tests**

In `src/remote/controller.rs` module `import_session_tests` (shown for `tags-header`; with `end-user-header` the expected header is `x-stozn-owner: ada@example.test` and the env value `ada@example.test`):

```rust
    #[test]
    fn owned_runs_tag_their_model_requests_with_the_owner() {
        let mut job = crate::state::Job::new("repo-runner", "claude");
        job.owner_sub = "394000000000000001".into();
        job.owner_email = "ada@example.test".into();
        let mut env = std::collections::HashMap::new();
        env.insert("ANTHROPIC_CUSTOM_HEADERS".to_string(), "x-profile: kept".to_string());
        super::apply_owner_env(&mut env, &job);
        assert_eq!(
            env.get("ANTHROPIC_CUSTOM_HEADERS").map(String::as_str),
            Some("x-profile: kept\nx-litellm-tags: owner:ada@example.test")
        );
        assert_eq!(
            env.get("AGENT_CTRL_OWNER_ATTRIBUTION").map(String::as_str),
            Some("owner:ada@example.test")
        );

        let mut unowned = std::collections::HashMap::new();
        super::apply_owner_env(&mut unowned, &crate::state::Job::new("repo-runner", "claude"));
        assert!(!unowned.contains_key("ANTHROPIC_CUSTOM_HEADERS"));
    }
```

In `deploy/k8s/test_shared_workspaces.py` `RenderTests` (only if codex PASSED):

```python
    def test_codex_sends_the_owner_attribution_header_on_staging(self):
        staging = configmap_block(dict(render.render(render.load_instance("staging", "test-image")))["configmap"], "codex-config.toml")
        self.assertIn('env_http_headers = { "x-litellm-tags" = "AGENT_CTRL_OWNER_ATTRIBUTION" }', staging)
        prod = configmap_block(dict(render.render(render.load_instance("prod", "test-image")))["configmap"], "codex-config.toml")
        self.assertNotIn("env_http_headers", prod)
```

`codex-config.toml` is the ConfigMap key that holds `{codex_config}` (`templates/configmap.tmpl.yaml:489`); prod uses the subscription branch of `_codex_config`, so it never gets the header.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --lib owned_runs_tag_their_model_requests` and `py -3 -m unittest discover -s deploy/k8s -p 'test_*.py'`
Expected: FAIL.

- [ ] **Step 3: Implement the controller side**

In `src/workspace_handoff.rs` after `OWNER_EMAIL_ENV`:

```rust
/// The owner as the run's model requests carry it for LiteLLM spend attribution.
pub const OWNER_ATTRIBUTION_ENV: &str = "AGENT_CTRL_OWNER_ATTRIBUTION";
```

In `src/remote/controller.rs` above `apply_owner_env` (values from the variant table):

```rust
/// Request header LiteLLM attributes spend by (P1 attribution spike).
const OWNER_ATTRIBUTION_HEADER: &str = "x-litellm-tags";

fn owner_attribution(owner_email: &str) -> String {
    format!("owner:{owner_email}")
}
```

At the end of `apply_owner_env` (after the git lines), add:

```rust
    let attribution = owner_attribution(&job.owner_email);
    let header = format!("{OWNER_ATTRIBUTION_HEADER}: {attribution}");
    let custom_headers = match env
        .get("ANTHROPIC_CUSTOM_HEADERS")
        .filter(|headers| !headers.is_empty())
    {
        Some(existing) => format!("{existing}\n{header}"),
        None => header,
    };
    env.insert("ANTHROPIC_CUSTOM_HEADERS".to_string(), custom_headers);
    env.insert(
        crate::workspace::handoff::OWNER_ATTRIBUTION_ENV.to_string(),
        attribution,
    );
```

- [ ] **Step 4: Run the controller tests**

Run: `cargo test --lib owned_runs_tag_their_model_requests && cargo test --locked`
Expected: PASS.

- [ ] **Step 5: codex (only if its spike row is PASS)**

In `deploy/k8s/render.py` `_DEFAULTS` add:

```python
    # Header the passiv-routed codex config sends AGENT_CTRL_OWNER_ATTRIBUTION in; empty = none.
    "owner_attribution_header": "",
```

In `_codex_config`, in the `else:` (passiv) branch, after the `body = (…)` assignment add:

```python
        if cfg.get("owner_attribution_header"):
            header = json.dumps(cfg["owner_attribution_header"])
            body += f'env_http_headers = {{ {header} = "AGENT_CTRL_OWNER_ATTRIBUTION" }}\n'
```

In `deploy/k8s/instances/staging.toml` add:

```toml
# LiteLLM spend attribution (P1 spike): codex sends the run owner in this header.
owner_attribution_header = "x-litellm-tags"
```

(`"x-stozn-owner"` with the `end-user-header` variant.) Run `py -3 -m unittest discover -s deploy/k8s -p 'test_*.py'` — PASS.

- [ ] **Step 6: Record what stays untagged**

Append to `docs/p1-litellm-attribution-spike.md`:

```markdown
## Wired in P1
- claude: ANTHROPIC_CUSTOM_HEADERS (controller, every owned run)
- codex: <"model_providers.passiv.env_http_headers (render.py)" or "not wired: spike FAIL">

## Untagged in P1
<one line per CLI whose row is FAIL, e.g. "pi 0.52.6: no per-provider header setting">
```

- [ ] **Step 7: Commit**

```bash
git add src/workspace_handoff.rs src/remote/controller.rs deploy/k8s/render.py deploy/k8s/instances/staging.toml deploy/k8s/test_shared_workspaces.py docs/p1-litellm-attribution-spike.md
git commit -m "feat(controller): attribute owned runs' model spend in LiteLLM

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

(Stage only the files you changed; `render.py`, `staging.toml` and the test file only when Step 5 applied.)

- [ ] **Step 8: Full verification and push of the agent-ctrl branch**

```bash
cargo fmt -- --check
cargo clippy --all-targets -- -D warnings
cargo test --locked
py -3 -m unittest discover -s deploy/k8s -p 'test_*.py'
node --test deploy/k8s/gitlab-scoped.test.cjs
git push -u origin feat/p1-agentctrl-api
```

Expected: all clean. Open a Gitea PR `feat/p1-agentctrl-api` → `dev` titled "P1: public run API (agentctrl-api), run owners, owner git author and attribution". Do not merge yet (Task 18 sequences the merges).

---

### Task 15: obachan — `bearer_only` app routes

**Files (worktree `T:/projects/.worktrees/obachan-apisix-bearer-only`):**
- Modify: `src/obachan/config/schema.py:2911-2980` (`PlatformAPISIXAppRouteConfig`)
- Modify: `src/obachan/runners/apisix_manifests.py:585-800` (new builders, `build_app_routes`)
- Test: `tests/unit/test_apisix_app_routes.py`

**Interfaces:**
- Produces: `PlatformAPISIXAppRouteConfig.bearer_only: bool = False`, `spa_origin: str | None` (required unless `bearer_only`); `build_app_bearer_plugin_config(cfg, *, oidc_client_secret, oidc_client_id, env) -> dict`; `build_app_bearer_rate_limit_plugin(cfg) -> dict`; `build_app_routes` renders, for a bearer-only entry, one route per `oidc_paths` pattern with id `app-<name>-<host>-bearer-<idx>`, priority 30, and no login, callback, key-auth or public routes.

- [ ] **Step 1: Write the failing tests**

Append to `tests/unit/test_apisix_app_routes.py`:

```python
# ---------------------------------------------------------------------------
# bearer_only — API hosts for bearer-token clients (agentctrl-api)
# ---------------------------------------------------------------------------


def _agentctrl_entry(**overrides: object) -> PlatformAPISIXAppRouteConfig:
    values: dict[str, object] = {
        "name": "agentctrl-api",
        "hosts": ["agents-dev.obachan.dev"],
        "upstream": "agentctrl-api.agent-ctrl.svc.cluster.local:8080",
        "oidc_client_id": "394361583406417710",
        "bearer_only": True,
    }
    values.update(overrides)
    return PlatformAPISIXAppRouteConfig(**values)  # type: ignore[arg-type]


def test_bearer_only_makes_spa_origin_optional() -> None:
    entry = _agentctrl_entry()
    assert entry.bearer_only is True
    assert entry.spa_origin is None


def test_browser_app_routes_still_require_spa_origin() -> None:
    import pytest

    with pytest.raises(Exception, match="spa_origin is required"):
        PlatformAPISIXAppRouteConfig(
            name="x", hosts=["x.example.com"], upstream="x.x.svc.cluster.local:8080"
        )


def test_bearer_only_rejects_key_auth_and_public_paths() -> None:
    import pytest

    with pytest.raises(Exception, match="no public_paths"):
        _agentctrl_entry(public_paths=["/public/*"])
    with pytest.raises(Exception, match="no key-auth"):
        _agentctrl_entry(key_auth_paths=["/api/*"])
    with pytest.raises(Exception, match="no key-auth"):
        _agentctrl_entry(consumer_allowlist=["svc-x"])


def test_bearer_only_renders_only_jwks_validated_bearer_routes() -> None:
    from obachan.runners.apisix_manifests import build_app_oidc_plugin_config, build_app_routes

    cfg = _CfgWithAppRoutesAndRateLimit([_agentctrl_entry()])
    routes = build_app_routes(
        cfg, oidc_client_secret="sec", oidc_client_id="shared", env="staging", session_secret="s"
    )
    assert [route_id for route_id, _ in routes] == [
        "app-agentctrl-api-agents-dev-obachan-dev-bearer-0"
    ]
    payload = routes[0][1]
    assert payload["uri"] == "/*"
    assert payload["priority"] == 30
    assert payload["hosts"] == ["agents-dev.obachan.dev"]
    assert payload["upstream"] == {
        "type": "roundrobin",
        "nodes": {"agentctrl-api.agent-ctrl.svc.cluster.local:8080": 1},
    }
    oidc = payload["plugins"]["openid-connect"]
    assert oidc["client_id"] == "394361583406417710"
    assert oidc["client_secret"] == "sec"
    assert oidc["bearer_only"] is True
    assert oidc["use_jwks"] is True
    assert oidc["unauth_action"] == "deny"
    assert oidc["set_userinfo_header"] is True
    assert oidc["set_access_token_header"] is True
    assert oidc["access_token_in_authorization_header"] is True
    assert (
        oidc["discovery"]
        == build_app_oidc_plugin_config(cfg, "agents-dev.obachan.dev", env="staging")["discovery"]
    )
    assert "session" not in oidc
    assert "redirect_uri" not in oidc
    assert payload["plugins"]["limit-count"] == {
        "count": 3000,
        "time_window": 60,
        "rejected_code": 429,
        "rejected_msg": "Rate limit exceeded",
        "policy": "local",
        "key_type": "var",
        "key": "remote_addr",
    }
    assert "cors" not in payload["plugins"]
    assert "redirect" not in payload["plugins"]


def test_bearer_only_with_a_browser_origin_adds_cors() -> None:
    from obachan.runners.apisix_manifests import build_app_routes

    entry = _agentctrl_entry(spa_origin="https://agents-ui-dev.obachan.dev")
    routes = build_app_routes(_CfgWithAppRoutesAndRateLimit([entry]), env="staging")
    assert routes[0][1]["plugins"]["cors"]["allow_origins"] == "https://agents-ui-dev.obachan.dev"


def test_browser_and_bearer_apps_render_side_by_side() -> None:
    from obachan.runners.apisix_manifests import build_app_routes

    cfg = _CfgWithAppRoutesAndRateLimit([_shirudo_config_entry(), _agentctrl_entry()])
    ids = [route_id for route_id, _ in build_app_routes(cfg, env="staging", session_secret="s")]
    assert ids == [
        "app-shirudo-api-shirudo-api-dev-obachan-dev-oidc-login-0",
        "app-shirudo-api-shirudo-api-dev-obachan-dev-oidc-callback-0",
        "app-shirudo-api-shirudo-api-dev-obachan-dev-keyauth-0",
        "app-shirudo-api-shirudo-api-dev-obachan-dev-oidc-0",
        "app-agentctrl-api-agents-dev-obachan-dev-bearer-0",
    ]
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd T:/projects/.worktrees/obachan-apisix-bearer-only && uv sync --extra dev --python 3.13 && uv run pytest tests/unit/test_apisix_app_routes.py -q`
Expected: FAIL (`bearer_only` is an extra field).

- [ ] **Step 3: Schema**

In `PlatformAPISIXAppRouteConfig`, replace the `spa_origin` field with:

```python
    spa_origin: str | None = Field(
        default=None,
        min_length=1,
        description=(
            "Browser SPA origin allowed by CORS and post-login redirect target. Required unless "
            "bearer_only; on a bearer-only host it only enables CORS."
        ),
    )
    bearer_only: bool = Field(
        default=False,
        description=(
            "API host for bearer-token clients: APISIX validates the Zitadel JWT against the "
            "issuer JWKS and denies requests without one. Renders no /_login, /_oidc/callback, "
            "key-auth or public routes and keeps no session."
        ),
    )
```

and add at the end of the class:

```python
    @model_validator(mode="after")
    def _routes_match_the_auth_mode(self) -> PlatformAPISIXAppRouteConfig:
        if self.bearer_only:
            if self.public_paths:
                raise ValueError("bearer_only app routes render no public_paths")
            if "key_auth_paths" in self.model_fields_set or self.consumer_allowlist:
                raise ValueError("bearer_only app routes render no key-auth routes")
        elif not self.spa_origin:
            raise ValueError("spa_origin is required unless bearer_only is true")
        return self
```

- [ ] **Step 4: Renderer**

In `src/obachan/runners/apisix_manifests.py`, add after `build_app_oidc_plugin_config`:

```python
def build_app_bearer_plugin_config(
    cfg: object,
    *,
    oidc_client_secret: str = "REPLACE_ME",  # noqa: S107
    oidc_client_id: str = "obachan-faas",
    env: str = "prod",
) -> dict[str, Any]:
    """openid-connect plugin config for a bearer-only app host.

    APISIX validates the caller's JWT against the issuer JWKS and denies requests without a
    valid one; with no browser login there is no redirect URI and no session. The upstream gets
    the token in ``Authorization`` and its claims in ``X-Userinfo``.
    """
    id_host = _zitadel_issuer_host(cfg, env)
    return {
        "client_id": oidc_client_id,
        "client_secret": oidc_client_secret,
        "discovery": f"https://{id_host}/.well-known/openid-configuration",
        "bearer_only": True,
        "use_jwks": True,
        "realm": "obachan-apps",
        "unauth_action": "deny",
        "set_userinfo_header": True,
        "set_access_token_header": True,
        "access_token_in_authorization_header": True,
    }


BEARER_FLOOD_GUARD_COUNT = 3000


def build_app_bearer_rate_limit_plugin(cfg: object) -> dict[str, Any]:
    """Coarse flood guard for bearer-only app routes; the upstream enforces per-user limits.

    APISIX sees Caddy's address as `remote_addr`, so every caller shares this bucket.
    """
    return {
        **build_rate_limit_plugin(cfg),
        "count": BEARER_FLOOD_GUARD_COUNT,
        "key_type": "var",
        "key": "remote_addr",
    }


def _bearer_only_routes(
    cfg: object,
    app_route: Any,
    host: str,
    upstream: dict[str, int],
    *,
    oidc_client_secret: str,
    oidc_client_id: str,
    env: str,
) -> list[tuple[str, dict[str, Any]]]:
    routes: list[tuple[str, dict[str, Any]]] = []
    for idx, uri in enumerate(app_route.oidc_paths):
        plugins: dict[str, Any] = {
            "openid-connect": build_app_bearer_plugin_config(
                cfg,
                oidc_client_secret=oidc_client_secret,
                oidc_client_id=oidc_client_id,
                env=env,
            ),
            "limit-count": build_app_bearer_rate_limit_plugin(cfg),
        }
        if app_route.spa_origin:
            plugins["cors"] = _cors_plugin(app_route.spa_origin)
        route_id = _app_route_id(app_route.name, host, "bearer", idx)
        routes.append(
            (
                route_id,
                {
                    "id": route_id,
                    "uri": uri,
                    "priority": 30,
                    "hosts": [host],
                    "upstream": {"type": "roundrobin", "nodes": dict(upstream)},
                    "plugins": plugins,
                },
            )
        )
    return routes
```

In `build_app_routes`, directly after `app_oidc_client_id = app_route.oidc_client_id or oidc_client_id` add:

```python
            if app_route.bearer_only:
                rendered.extend(
                    _bearer_only_routes(
                        cfg,
                        app_route,
                        host,
                        upstream,
                        oidc_client_secret=oidc_client_secret,
                        oidc_client_id=app_oidc_client_id,
                        env=env,
                    )
                )
                continue
            # The schema requires an origin for every browser app route.
            spa_origin: str = app_route.spa_origin or ""
```

and replace the four later uses of `app_route.spa_origin` in that loop (the two `"redirect": {"uri": …}` values and the two `_cors_plugin(…)` arguments) with `spa_origin`. Add one line to the `build_app_routes` docstring: "Bearer-only entries render only JWKS-validated bearer data routes (see `_bearer_only_routes`)."

- [ ] **Step 5: Run the tests to verify they pass**

Run: `uv run pytest tests/unit/test_apisix_app_routes.py -q && uv run pytest -q && uv run ruff check . && uv run ruff format --check .`
Expected: PASS, including the unchanged `test_build_app_routes_default_renders_no_public_routes` byte-identity test.

- [ ] **Step 6: Commit and push**

```bash
git add src/obachan/config/schema.py src/obachan/runners/apisix_manifests.py tests/unit/test_apisix_app_routes.py
git commit -m "feat(apisix): bearer-only app routes validated against Zitadel JWKS

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
git push -u origin feat/apisix-bearer-only
```

Open a Gitea PR `feat/apisix-bearer-only` → `dev`.

---

### Task 16: obachan-config — the `agentctrl-api` route and DNS record

**Files (worktree `T:/projects/.worktrees/obachan-config-agentctrl-api`, branch from `origin/main`):**
- Modify: `obachan.toml` — after the staging `shirudo-api` app route (line 2928-2932 on 944cee3) and after the `shirudo-api-dev` DNS record (line 630-634)

**Interfaces:**
- Consumes: `bearer_only` (Task 15). The file only loads with an obachan that has Task 15 (`extra="forbid"`), so this PR merges after Task 15's.

- [ ] **Step 1: Add the app route**

After the `[[envs.staging.platform.apisix.app_routes]]` block for `shirudo-api` add:

```toml
# agentctrl-api: AgentCtrl's public run API for WarpSpeed (P1). Bearer-only: APISIX validates the
# Zitadel access token against the id-dev JWKS and the API verifies it again; no login or
# callback routes, so the client needs no redirect URI. oidc_client_id is the WarpSpeed
# device-code client (its tokens carry it in aud); JWKS validation needs no client secret.
# Caddy forwards the host to APISIX via the registry route (gate = "apisix", registered from
# stoz3n-agent-ctrl deploy/k8s/agentctrl-api.obachan.app.toml).
[[envs.staging.platform.apisix.app_routes]]
name = "agentctrl-api"
hosts = ["agents-dev.obachan.dev"]
upstream = "agentctrl-api.agent-ctrl.svc.cluster.local:8080"
oidc_client_id = "394361583406417710"
bearer_only = true
```

- [ ] **Step 2: Add the DNS record**

After `[dns.zones.obachan-dev.records.shirudo-api-dev]` add:

```toml
# AgentCtrl public run API (P1), terminated at the staging obachan ingress like shirudo-api-dev.
[dns.zones.obachan-dev.records.agents-dev]
name = "agents-dev"
type = "A"
target = "128.140.27.186"
ttl = 300
```

- [ ] **Step 3: Verify the config loads with the new schema**

```bash
cd T:/projects/.worktrees/obachan-config-agentctrl-api
uv run --project T:/projects/.worktrees/obachan-apisix-bearer-only python -c "from obachan.config.loader import load_config; c = load_config('obachan.toml', env='staging'); print([(r.name, r.bearer_only) for r in c.platform.apisix.app_routes])"
```

Expected: `[('shirudo-api', False), ('agentctrl-api', True)]`.

- [ ] **Step 4: Commit and push**

```bash
git add obachan.toml
git commit -m "config(staging): agentctrl-api bearer-only APISIX route and agents-dev DNS

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
git push -u origin feat/agentctrl-api-route
```

Open a Gitea PR `feat/agentctrl-api-route` → `main`, noting it depends on the obachan PR from Task 15.

---

### Task 17: WarpSpeed — allowlisted base-URL override for the E2E

P1 does not switch WarpSpeed to the new host (that is P3). This adds an environment override, accepted only for the new staging origin, so the E2E can run an installed build against `agents-dev.obachan.dev`.

**Files (worktree `T:/projects/dev-ops/warp-p1-base-url-override`):**
- Modify: `crates/managed_agent_sessions/src/lib.rs:22` (constants), `:1412-1422` (`profile`), imports (line 1-5)
- Modify: `crates/managed_agent_sessions/Cargo.toml` (`log`)
- Test: `crates/managed_agent_sessions/src/tests.rs`

**Interfaces:**
- Produces: `STOZN_BASE_URL=https://agents-dev.obachan.dev` makes every managed-session request (and `validate_destination_url`) use that origin; any other value is ignored with a warning; unset keeps `https://dash-backend-dev.stozn.ai`. `fn staging_base_url(override_value: Option<&str>) -> &'static str`.

- [ ] **Step 1: Write the failing test**

Append to `crates/managed_agent_sessions/src/tests.rs`:

```rust
#[test]
fn only_allowlisted_origins_override_the_managed_session_base_url() {
    assert_eq!(staging_base_url(None), STAGING_BASE_URL);
    assert_eq!(staging_base_url(Some("")), STAGING_BASE_URL);
    assert_eq!(
        staging_base_url(Some("https://agents-dev.obachan.dev")),
        "https://agents-dev.obachan.dev"
    );
    assert_eq!(
        staging_base_url(Some("https://agents-dev.obachan.dev/")),
        "https://agents-dev.obachan.dev"
    );
    assert_eq!(staging_base_url(Some("https://evil.example")), STAGING_BASE_URL);
    assert_eq!(staging_base_url(Some("http://agents-dev.obachan.dev")), STAGING_BASE_URL);
}
```

- [ ] **Step 2: Run it to verify it fails**

Run (reusing a sibling build cache per the worktree convention): `cd T:/projects/dev-ops/warp-p1-base-url-override && mkdir -p target/debug && CARGO_TARGET_DIR=T:/projects/dev-ops/warp-dev-release/target cargo nextest run -p managed_agent_sessions only_allowlisted_origins`
Expected: compile error `cannot find function staging_base_url`.

- [ ] **Step 3: Implement**

In `crates/managed_agent_sessions/Cargo.toml` `[dependencies]` add `log.workspace = true`.

In `lib.rs` add `use std::sync::OnceLock;` to the imports, and after `const STAGING_BASE_URL: …;` add:

```rust
/// Debug override of the staging managed-session origin, used to exercise the AgentCtrl public
/// API before WarpSpeed switches to it. Only these origins are accepted because every request
/// carries the user's access token.
const BASE_URL_OVERRIDE_ENV: &str = "STOZN_BASE_URL";
const BASE_URL_OVERRIDES: &[&str] = &["https://agents-dev.obachan.dev"];
```

Replace `fn profile` (line 1412-1422) with:

```rust
fn staging_base_url(override_value: Option<&str>) -> &'static str {
    let requested = override_value
        .map(|value| value.trim().trim_end_matches('/'))
        .filter(|value| !value.is_empty());
    let Some(requested) = requested else {
        return STAGING_BASE_URL;
    };
    match BASE_URL_OVERRIDES.iter().find(|allowed| **allowed == requested) {
        Some(allowed) => allowed,
        None => {
            log::warn!(
                "{BASE_URL_OVERRIDE_ENV}={requested} is not an allowed managed-session origin; using {STAGING_BASE_URL}"
            );
            STAGING_BASE_URL
        }
    }
}

fn profile(environment: ManagedEnvironment) -> &'static EnvironmentProfile {
    static STAGING: OnceLock<EnvironmentProfile> = OnceLock::new();
    match environment {
        ManagedEnvironment::Staging => STAGING.get_or_init(|| EnvironmentProfile {
            alias: "staging",
            base_url: staging_base_url(std::env::var(BASE_URL_OVERRIDE_ENV).ok().as_deref()),
            fleet_id: STAGING_FLEET_ID,
            zitadel: &STAGING_ZITADEL,
        }),
    }
}
```

- [ ] **Step 4: Run the tests, format and lint**

```bash
CARGO_TARGET_DIR=T:/projects/dev-ops/warp-dev-release/target cargo nextest run -p managed_agent_sessions
./script/format
CARGO_TARGET_DIR=T:/projects/dev-ops/warp-dev-release/target cargo clippy -p managed_agent_sessions --all-targets --all-features -- -D warnings
```

Expected: all PASS, no format diff, no clippy warnings.

- [ ] **Step 5: Commit, push, PR, merge to dev**

```bash
git add crates/managed_agent_sessions/Cargo.toml crates/managed_agent_sessions/src/lib.rs crates/managed_agent_sessions/src/tests.rs
git commit -m "feat(managed-sessions): allow the AgentCtrl API origin as a staging override

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
git push -u gitea feat/managed-base-url-override
git push -u gitlab feat/managed-base-url-override
```

Open a Gitea PR to `dev` using `.github/pull_request_template.md` (no changelog line: debug-only). Merge it to `dev` before any build (Task 18 Step 7 builds from `dev`).

---

### Task 18: Staging rollout

Merge order matters: obachan-config only loads with the new obachan; the APISIX route needs the API Service; WarpSpeed builds only from `dev`.

- [ ] **Step 1: agent-ctrl**

Merge the `feat/p1-agentctrl-api` PR into `dev` (its CI ran `cargo test --locked` and the render tests). The push to `dev` builds the image and deploys staging; it rolls the controller and the node StatefulSet as every agent-ctrl deploy does, so warn active session owners first. Then:

```bash
export KUBECONFIG=~/.kube/obachan-staging
K="kubectl --server https://staging-gateway:6443 --tls-server-name 10.96.0.1 -n agent-ctrl"
$K rollout status deploy/agentctrl-api --timeout=300s
$K logs deploy/agentctrl-api --tail=20
$K port-forward deploy/agentctrl-api 18080:8080 &
curl -s localhost:18080/healthz
curl -s -o /dev/null -w '%{http_code}\n' localhost:18080/api/agent-control/overview
kill %1
```

Expected: rollout complete; log line `agent-ctrl api listening`; `ok`; `401`. If the readiness probe fails while the pod logs are healthy, the CNI is applying `agentctrl-api-ingress` to kubelet probes: add an `ipBlock` for the node CIDR to `api-networkpolicy.tmpl.yaml` in a follow-up PR rather than widening it to all namespaces.

- [ ] **Step 2: obachan**

Merge the `feat/apisix-bearer-only` PR into `dev`.

- [ ] **Step 3: obachan-config**

Merge the `feat/agentctrl-api-route` PR into `main`.

- [ ] **Step 4: DNS**

```bash
cd T:/projects/.worktrees/obachan-config-agentctrl-api && git fetch origin && git checkout --detach origin/main
OBACHAN="uv run --project T:/projects/.worktrees/obachan-apisix-bearer-only obachan"
git -C T:/projects/.worktrees/obachan-apisix-bearer-only fetch origin && git -C T:/projects/.worktrees/obachan-apisix-bearer-only checkout --detach origin/dev
$OBACHAN --help | head -20
$OBACHAN dns drift --zone obachan-dev
```

`dns drift` lists `agents-dev` as missing. Create it with `dns add` using the env option exactly as `$OBACHAN --help` shows it (the command reads `ctx.obj["env"]`), first with `--dry-run`, then with `--confirm`:

```bash
$OBACHAN --env staging dns add agents-dev --type A --target 128.140.27.186 --zone obachan-dev --ttl 300 --dry-run
$OBACHAN --env staging dns add agents-dev --type A --target 128.140.27.186 --zone obachan-dev --ttl 300 --confirm
```

Expected afterwards: `dns drift --zone obachan-dev` reports no drift for `agents-dev`; `nslookup agents-dev.obachan.dev` returns 128.140.27.186. If `dns add` rewrote `obachan.toml`, `git diff` must be empty against `origin/main` (the record is already there); discard nothing else.

- [ ] **Step 5: APISIX routes**

```bash
$OBACHAN deploy platform --component apisix --env staging --dry-run
$OBACHAN deploy platform --component apisix --env staging --confirm
```

Expected: the dry run lists route `app-agentctrl-api-agents-dev-obachan-dev-bearer-0`; the apply logs it as registered.

- [ ] **Step 6: Caddy route**

With the operator's `OBACHAN_API_URL` and `OBACHAN_API_ADMIN_TOKEN` exported (never echoed):

```bash
cd T:/projects/SOTZ3N/stoz3n-agent-ctrl && git fetch origin && git checkout --detach origin/dev
$OBACHAN apps routes register --spec deploy/k8s/agentctrl-api.obachan.app.toml --env staging
curl -s -o /dev/null -w '%{http_code}\n' https://agents-dev.obachan.dev/api/agent-control/overview
```

Expected: register succeeds (the API reconciles the Certificate and Caddy route on PUT); after the certificate issues, the curl prints `401` (APISIX denies a missing bearer). A `404`/`502` from Caddy means the route or certificate is not live yet; re-check with `$OBACHAN apps routes list --env staging`.

- [ ] **Step 7: WarpSpeed build from dev**

Only after the Task 17 PR is merged:

```bash
git -C T:/projects/dev-ops/warp-dev-release pull --ff-only gitea dev
cd T:/projects/dev-ops/warp-dev-release
powershell -ExecutionPolicy Bypass -File ./script/windows/install-warpspeed.ps1
```

This installs a new build next to the running instances; do not restart, close or relaunch any running WarpSpeed window. The E2E uses the new build's CLI only.

---

### Task 19: Staging E2E

Evidence goes to `docs/superpowers/evidence/2026-10-p1-e2e.md` in `T:/projects/dev-ops/warp-p1-agentctrl-api` (branch `docs/p1-agentctrl-api`), one section per check with the commands and their outputs (job ids, status codes, commit author lines, spend-log `request_id`s). Never paste a token.

Prerequisites: an admin account (agent-admin) and a member test account holding only `agent-user` on project 394361582064240430 in Zitadel id-dev (grant it in the Zitadel console if it does not exist).

- [ ] **Step 1: Bearer tokens for curl (device flow, per account)**

```bash
ISSUER=https://id-dev.obachan.dev
CLIENT=394361583406417710
SCOPE="openid profile email offline_access urn:zitadel:iam:org:project:id:394361582064240430:aud urn:zitadel:iam:org:projects:roles"
device_token() {
  local start device
  start=$(curl -s -X POST "$ISSUER/oauth/v2/device_authorization" -d "client_id=$CLIENT" --data-urlencode "scope=$SCOPE")
  echo "approve in a browser: $(echo "$start" | jq -r .verification_uri_complete)" >&2
  device=$(echo "$start" | jq -r .device_code)
  read -r -p "press enter once approved " _ >&2
  curl -s -X POST "$ISSUER/oauth/v2/token" -d grant_type=urn:ietf:params:oauth:grant-type:device_code -d "device_code=$device" -d "client_id=$CLIENT" | jq -r .access_token
}
ADMIN_TOKEN=$(device_token)    # approve as the admin
MEMBER_TOKEN=$(device_token)   # approve as the member, in a private window
API=https://agents-dev.obachan.dev/api/agent-control
curl -s -o /dev/null -w 'admin overview %{http_code}\n' -H "Authorization: Bearer $ADMIN_TOKEN" "$API/overview"
curl -s -o /dev/null -w 'member overview %{http_code}\n' -H "Authorization: Bearer $MEMBER_TOKEN" "$API/overview"
```

Expected: both `200`.

- [ ] **Step 2: WarpSpeed against the new host (admin)**

Use the CLI shim the installer writes (`%USERPROFILE%\.local\bin\warpspeed.cmd`, which runs the build in `%LOCALAPPDATA%\Programs\WarpSpeed\current.txt`, i.e. the Task 18 Step 7 build), signed in to STOZN as the admin:

```powershell
$env:STOZN_BASE_URL = "https://agents-dev.obachan.dev"
& "$HOME\.local\bin\warpspeed.cmd" agent-ctrl repos --json
& "$HOME\.local\bin\warpspeed.cmd" agent-ctrl start --agent claude --repo https://git-dev.obachan.dev/stozn/warpspeed.git --prompt "Create e2e/p1-owner.txt containing the word owner, commit it with the message 'p1 owner e2e', then wait for input."
& "$HOME\.local\bin\warpspeed.cmd" agent-ctrl list
& "$HOME\.local\bin\warpspeed.cmd" agent-ctrl open <job>
& "$HOME\.local\bin\warpspeed.cmd" agent-ctrl input <job> "Reply with DONE."
& "$HOME\.local\bin\warpspeed.cmd" agent-ctrl stop <job>
Remove-Item Env:STOZN_BASE_URL
```

Expected: `repos` lists the staging repositories; `start` prints the job id; `list` shows it with state `running`; `open` shows the run's events (WarpSpeed's viewer long-polls `events?wait_ms`); `input` is accepted; `stop` is accepted. Confirm in the API: `curl -s -H "Authorization: Bearer $ADMIN_TOKEN" "$API/runs/<job>" | jq '{owner_sub, owner_email, source, state}'` shows the admin as owner and `source: "warpspeed"`.

- [ ] **Step 3: SSE**

Start a second run the same way (or reuse one that is still running) and, while it works:

```bash
curl -sN -H "Authorization: Bearer $ADMIN_TOKEN" -H "Accept: text/event-stream" "$API/runs/<job>/events/stream" | head -40
```

Expected: `id: …` / `event: run_event` / `data: {…}` frames, `: keepalive` comments at 15 s gaps, and `event: terminal` when the run ends. Resume check: copy one `id:` value and rerun with `-H "Last-Event-ID: <id>"`; the first frame is the next event.

- [ ] **Step 4: Ownership**

```bash
curl -s -o /dev/null -w 'member reads admin run %{http_code}\n' -H "Authorization: Bearer $MEMBER_TOKEN" "$API/runs/<job>"
curl -s -H "Authorization: Bearer $MEMBER_TOKEN" "$API/runs?limit=200" | jq --arg job "<job>" '[.items[].job_id] | index($job)'
curl -s -o /dev/null -w 'member stops admin run %{http_code}\n' -X POST -H "Authorization: Bearer $MEMBER_TOKEN" -H "Content-Type: application/json" -H "Idempotency-Key: e2e-member-stop-0001" -d '{"force":false}' "$API/runs/<job>/stop"
curl -s -o /dev/null -w 'no role %{http_code}\n' -H "Authorization: Bearer not-a-token" "$API/overview"
```

Expected: `404`, `null`, `404`, `401`.

- [ ] **Step 5: Owner as commit author**

```bash
scratch=$(mktemp -d)
git -C "$scratch" init -q
git -C "$scratch" fetch -q https://git-dev.obachan.dev/stozn/warpspeed.git "agent/<job>"
git -C "$scratch" log -3 --format='%h %an <%ae> | committer %cn <%ce> | %s' FETCH_HEAD
rm -rf "$scratch"
```

Expected: the run's commits (the agent's `p1 owner e2e` and, if any, `agent-ctrl run <job> end`) show the admin's display name and email as author and the node identity (`stoz3n-agent-staging` / `agents@staging.pseekoo.io` or the node's git config) as committer. Afterwards delete the test branch: `git push https://git-dev.obachan.dev/stozn/warpspeed.git --delete agent/<job>` (only this branch).

- [ ] **Step 6: Owner on model spend (Claude)**

```bash
LITELLM_MASTER_KEY=$(bao kv get -field=litellm_master_key obachan/staging/passiv-litellm)
K="kubectl --server https://staging-gateway:6443 --tls-server-name 10.96.0.1 -n agent-ctrl"
$K run p1-e2e-spend --rm -i --restart=Never --image curlimages/curl:8.10.1 -- \
  curl -s -H "Authorization: Bearer $LITELLM_MASTER_KEY" \
  "http://passiv-litellm.passiv-litellm.svc.cluster.local:4000/spend/logs?start_date=$(date -u +%F)&end_date=$(date -u -d tomorrow +%F)" \
  | jq --arg owner "<admin email>" '[.[] | select(((.request_tags // []) | tostring | contains($owner)) or (.end_user == $owner)) | {request_id, model, request_tags, end_user, startTime}] | length, .[0]'
unset LITELLM_MASTER_KEY
```

Expected: a non-zero count of rows from the Step 2 run's time window tagged with the admin's email (`owner:<email>` in `request_tags`, or `end_user` = email, per the Task 0 mechanism).

- [ ] **Step 7: Record and push the evidence**

```bash
cd T:/projects/dev-ops/warp-p1-agentctrl-api
git add docs/superpowers/evidence/2026-10-p1-e2e.md
git commit -m "docs(p1): staging E2E evidence for the AgentCtrl public run API

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
git push gitea docs/p1-agentctrl-api && git push gitlab docs/p1-agentctrl-api
unset ADMIN_TOKEN MEMBER_TOKEN
```

---

## Spec coverage

| Spec section | Tasks |
|---|---|
| §1 Service (`agent-ctrl api`, axum, FleetClient seam, `source`, Mongo reads, `src/api/`) | 4, 7, 8, 9 |
| §2 Contract (paths, idempotency, 64 KiB, rate limit, readiness, error mapping) | 4, 8, 9, 10, 12 |
| §2 SSE endpoint | 11 |
| §2 Golden fixtures | 12 |
| §3 APISIX `bearer_only` + limit-count, obachan-config route, Caddy `gate="apisix"` | 15, 16, 13 (spec file), 18 |
| §3 API verification rules, roles, `X-Userinfo` | 5 |
| §3 Owner fields, member/admin visibility, ownerless admin-only, `requested_by` | 1, 7, 9, 10 |
| §4 Git author on start/import/resume, handoff/pause commits | 2, 3 |
| §4 Model attribution + spike | 0, 14 |
| §5 Deployment, NetworkPolicy, instance keys, workflow | 13, 18 |
| §6 Error handling (503s, fail-closed JWKS, SSE error/disconnect) | 4, 5, 9, 11 |
| §7 Testing (handlers, ownership, auth, SSE, git author, contract, obachan, E2E) | 1–3, 5, 9–12, 15, 19 |
| §8 Open items (LiteLLM attribute; WarpSpeed override) | 0, 14, 17 |
