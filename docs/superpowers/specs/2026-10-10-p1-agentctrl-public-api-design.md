# P1 — AgentCtrl public run API — Design

Date: 2026-10-10
Status: approved in discussion (2026-10-10)
Builds on: `2026-10-08-self-hosted-agent-platform-design.md` (P1 outline), P0 (Zitadel identity, done 2026-10-10).
Target: staging only (`agent-ctrl` namespace, Zitadel `https://id-dev.obachan.dev`, project `394361582064240430`).

## Goal

WarpSpeed reaches AgentCtrl through obachan's APISIX at `agents-dev.obachan.dev` with its Zitadel token, instead
of through the STOZN dashboard. Each run has an owner: members see and control their own runs, admins all. Commits
made by a run name its owner as author, and model spend is attributable to the owner.

## Non-goals

- Switching WarpSpeed to the new host by default (P3). P1 proves it works with a base-URL override.
- One pod per run (P2), webhooks and triggers (P5), per-user model budgets.
- Removing the dashboard's `/api/agent-control/*` routes; the dashboard keeps serving its web page.

## Decisions (2026-10-10)

1. The API mirrors the dashboard's `/api/agent-control/*` contract one-to-one, plus one SSE endpoint.
2. APISIX validates the Zitadel bearer (new `bearer_only` app-route mode) and the API validates it again.
3. Runs keep the node's shared passiv key; the owner is attached to model requests as a LiteLLM tag/end-user.

## 1. Service

- New clap subcommand `agent-ctrl api --bind 0.0.0.0:8080` in the existing `agent-ctrl` crate (axum on the existing
  tokio runtime), shipped in the same image, run as its own Deployment `agentctrl-api` (2 replicas).
- Talks to the controller with the existing `FleetClient` (`src/remote/client.rs`) using the control token from
  `agent-ctrl-control-auth`; `source = "agentctrl_api"` (or `"warpspeed"` when the request carries
  `X-Stozn-Client: warpspeed`, as the dashboard does).
- Reads Mongo (`jobs`, `job_events`, `job_history`, `control_commands`) for paged lists, events and audit, with the
  same queries and cursor encodings the dashboard uses (`server/agent-control.ts` on stoz3n-agent-dash `origin/dev`).
- New module tree `src/api/` (router, auth, ownership, handlers per resource, SSE); no changes to controller logic
  beyond the owner fields and env passing below.

## 2. API contract

Paths, request/response JSON, status codes, `Idempotency-Key` rules (`^[A-Za-z0-9._:-]{16,128}$` → gRPC
`request_id`, `Idempotency-Replayed` response header), 64 KiB body cap, rate limit (30 mutations / 60 s / actor →
429 `rate_limited`), readiness gates (stale controller 45 s / no nodes → 503) and error mapping (`node_at_capacity`
409, `session_unknown` 412, `invalid_state`/`not_interactive`/`conflict` 409, `validation_error` 422,
`controller_unavailable` 503) are identical to the dashboard's:

- `GET /api/agent-control/overview`
- `GET /api/agent-control/runs?limit&state&agent_name&node_name&task_id&cursor` → `{items, next_cursor}`
- `POST /api/agent-control/runs` · `POST /api/agent-control/runs/import` (multipart, ≤32 MiB session file)
- `GET /api/agent-control/repos` · `GET /api/agent-control/commands?cursor`
- `GET /api/agent-control/runs/{job}` · `GET /api/agent-control/runs/{job}/session`
- `GET /api/agent-control/runs/{job}/events?after&limit&wait_ms` (410 on gap) · `GET|POST …/history`
- `POST /api/agent-control/runs/{job}/{input|close|stop|pause|resume}`

New: `GET /api/agent-control/runs/{job}/events/stream` — `text/event-stream`; each event `id:` is the same opaque
cursor as `next_cursor`, `event: run_event` with the event item JSON as `data`; `Last-Event-ID` (or `?after=`)
resumes; a retention gap sends `event: gap` with `{earliest_cursor}` and closes; a terminal run sends
`event: terminal` and closes; `: keepalive` comments every 15 s. Backed by the same Mongo `sequence` cursor (poll
interval 250 ms, as today).

The contract is pinned by reusing the dashboard's `scripts/check-agent-control.mjs` fixtures as golden responses.

## 3. Identity and ownership

- **APISIX:** obachan's `PlatformAPISIXAppRouteConfig` gains `bearer_only: bool` (default false). When true,
  `spa_origin` is optional, no `/_login` or `/_oidc/callback` routes are rendered, and the OIDC route uses
  `bearer_only: true`, `use_jwks: true`, `unauth_action: "deny"`, `set_userinfo_header: true`,
  `set_access_token_header: true`, no session, plus `limit-count` keyed per consumer/`remote_addr`. obachan-config
  adds the `agentctrl-api` route (`agents-dev.obachan.dev` → `agentctrl-api.agent-ctrl.svc.cluster.local:8080`,
  `oidc_client_id` = the `warpspeed` client `394361583406417710`, since JWKS validation needs no client secret,
  `bearer_only = true`) and registers the Caddy route with `gate="apisix"`.
- **API:** verifies the bearer itself with the dashboard's rules (RS256 only, string `kid`, JWKS cache 10 min with
  forced refetch throttled to 60 s, exact `iss`, project in `aud`, `exp`/`nbf` with 60 s leeway, roles only from
  `urn:zitadel:iam:org:project:394361582064240430:roles`). `agent-admin` → admin, `agent-user` → member, neither →
  401. The bearer is authoritative; `X-Userinfo` is only used for display name/email when present.
- **Owner:** `Job` gains `owner_sub` and `owner_email` (serde default, mirrored to Mongo `jobs`); `SpawnAgentRequest`
  and `ImportSessionRequest` gain the same fields; `JobInfo` exposes them; `publicRun` gains `owner_sub` and
  `owner_email`. Members: list queries filter on `owner_sub`, and every per-run route returns 404 for runs they do
  not own. Admins see all. Runs created before P1 (no owner) are visible to admins only. The dashboard's actor
  string stays in `requested_by` for audit.

## 4. Run identity

- **Git author:** the controller adds `GIT_AUTHOR_NAME`/`GIT_AUTHOR_EMAIL` (owner display name and email) to the
  run's spawn env, on start, import and resume; the committer stays the node identity. `push_run_work` and the
  handoff/pause commits use the same author. Runs without an owner keep today's behavior.
- **Model attribution:** runs keep the node's shared passiv key. The owner is attached to every model request as
  LiteLLM end-user/tag metadata — for Claude via `ANTHROPIC_CUSTOM_HEADERS` (e.g. `x-litellm-tags: owner:<email>`
  or the end-user header LiteLLM is configured to read), and for the other CLIs through their equivalent setting.
  P1 starts with a spike that confirms, per CLI (claude, codex, pi, kimi, dsh), whether a custom header or user field
  reaches LiteLLM's spend log; CLIs that cannot carry it stay untagged and are listed in the plan.

## 5. Deployment (staging)

- `deploy/k8s/render.py` + templates: `agentctrl-api` Deployment and Service (port 8080), env for Mongo (TLS bootstrap
  as the controller), control token, Zitadel issuer/project; NetworkPolicy: APISIX namespace → API 8080, API →
  controller 50051, API → Mongo; instance keys in `instances/staging.toml` (hostname, issuer, project, replicas).
- `.gitea/workflows/deploy.yml`: add `rollout status deploy/agentctrl-api`.
- obachan: renderer + schema change (own PR, deployed with `obachan deploy platform --component apisix --env
  staging`); obachan-config: the route entry and DNS record for `agents-dev.obachan.dev`.

## 6. Error handling

- Controller unreachable → 503 `controller_unavailable`; Mongo unreachable → 503; never a 500 for dependency
  failures.
- Zitadel JWKS unreachable → 401 (fail closed), as in the dashboard.
- SSE: client disconnect ends the server task; Mongo errors during a stream send `event: error` and close so the
  client falls back to polling.

## 7. Testing

- agent-ctrl: axum handler tests with a fake FleetControl (trait seam over `FleetClient`) and an in-memory event
  store; ownership filter tests (member sees own only, 404 on others, admin sees all, ownerless admin-only); auth
  tests (wrong aud/iss/alg/kid, expired, generic-only roles claim); SSE tests (resume by `Last-Event-ID`, gap,
  terminal, keepalive); git-author env on spawn/import/resume.
- Contract test: the dashboard's golden responses replayed against the new handlers.
- obachan: renderer tests for `bearer_only` (no login/callback routes, plugin config, limit-count).
- Staging E2E: WarpSpeed with `STOZN_BASE_URL`-style override to `https://agents-dev.obachan.dev` (`list`, `start`,
  `open` with SSE, `input`, `stop`), an `agent-user` account cannot see an admin's run, a run's commit shows the
  owner as author, the owner appears on the run's model requests in LiteLLM for Claude.

## 8. Open items for the plan

- The exact LiteLLM attribute (tag vs end-user header) and its `general_settings` (spike).
- Whether WarpSpeed needs a base-URL override setting for the E2E (a small client change behind a debug env var is
  acceptable in P1; the real switch is P3).
