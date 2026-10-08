# Self-hosted agent platform: AgentCtrl replaces Oz and Warp Agent Mode — Design

Date: 2026-10-08
Status: architecture approved in discussion (2026-10-08); P0 detailed below, P1–P6 get their own specs
Builds on: `2026-10-02-move-session-to-agentctrl-design.md`, `2026-10-04-agentctrl-fleet-design.md`,
`2026-10-07-agentctrl-pause-and-second-node-design.md`.

## Goal

WarpSpeed stops depending on Warp's servers (`app.warp.dev`, `oz.warp.dev`) for agent features.
AgentCtrl is the base: it runs cloud agent runs and hosts a new Agent Mode backend. passiv is the
only path to models. obachan's APISIX is the front door for WarpSpeed's agent traffic. Zitadel is
the identity for all of it.

Users are a team: each person signs in with their own Zitadel identity, has their own model budget
and metering, and authors their own commits. One shared fleet; no hard isolation between tenants.

## Non-goals

- Multi-tenant isolation (separate customers, billing, credential sets).
- Remote child agents started by Agent Mode (Oz orchestration). Can return later on top of P1.
- Warp Drive, session sharing, and the other non-agent Warp server features.
- Changing the agent CLIs (claude, pi, codex) themselves.

## Today

| Area | Today | Source |
|---|---|---|
| Agent Mode | Warp's server runs the model loop: protobuf `Request` → `POST {server_root}/ai/multi-agent`, SSE of base64 protobuf `ResponseEvent`s; client executes tool calls | `app/src/ai/agent/api/impl.rs`, `crates/warp_multi_agent_client` |
| Oz cloud runs | REST `/api/v1/agent/*` on Warp's server, viewed through Warp session sharing; on by default via cargo features | `app/src/server/server_api/ai.rs`, `app/Cargo.toml:517` |
| AgentCtrl client | Separate path: `crates/managed_agent_sessions` → `dash-backend-dev.stozn.ai/api/agent-control/*`, long-poll events | `crates/managed_agent_sessions/src/lib.rs` |
| WarpSpeed login | STOZN dashboard device login → 30-day HS256 JWT signed by the dashboard; only the dashboard accepts it | `app/src/stozn_auth.rs`, `stoz3n-agent-dash/server/app.ts:3185-3345` |
| AgentCtrl | gRPC `FleetControl`; runs are processes in node pods; one shared model key and git identity; admin-only | `stoz3n-agent-ctrl` (`T:/projects/SOTZ3N`, branch `dev`) |
| passiv | LiteLLM gateway `gateway[-dev].mezzanin.eu`: Anthropic `/v1/messages`, OpenAI chat/responses, SSE; per-key budgets, RPM/TPM; per-user keys via `tenant/me` behind oauth2-proxy | `passiv/src/passiv/api/{app,tenant}.py` |
| obachan APISIX | Zitadel `openid-connect` and key-auth routes from `[[platform.apisix.app_routes]]`; used by FaaS, metrics, shirudo-api | `obachan/src/obachan/runners/apisix_manifests.py` |
| Zitadel | One instance per environment: `id-dev.obachan.dev` (staging), `id.pseekoo.io` (prod) | `obachan-config/obachan.toml` |

## Target architecture

```
WarpSpeed ── Zitadel device-code login (per environment) ──────────────┐
   │                                                                    │ bearer access token
   ├─ Agent Mode ── POST /ai/multi-agent (protobuf + SSE) ──► agent-mode[-dev].obachan.dev
   ├─ Cloud runs ── /v1/runs, SSE events ───────────────────► agents[-dev].obachan.dev
   │                                                                    │
   │                                       obachan APISIX (openid-connect, bearer only;
   │                                       limit-count per user)
   │                                            │                       │
   │                                     agentctrl-mode           agentctrl-api
   │                                     (model loop)             (REST + SSE)
   │                                            │                       │ gRPC FleetControl
   │                                            │                AgentCtrl controller → nodes/run pods
   │                                            │                       │
   │                                            └──► passiv ◄───────────┘
   │                                   short-lived per-user keys (passiv internal API)
   │                                   gateway[-dev].mezzanin.eu: budgets, metering, models
```

### Rules every sub-project follows

1. **Identity.** Every request from WarpSpeed carries a Zitadel access token. APISIX validates it
   and forwards the caller's identity to the upstream (`X-Userinfo`). Services trust identity only
   from APISIX, never from the request body.
2. **Roles.** Zitadel project `agent-platform` with roles `agent-user` and `agent-admin`. Members see
   and control their own runs and conversations; admins see all.
3. **Model access.** Services never hold a long-lived shared key for user work. They ask passiv for a
   short-lived key bound to the user (P0 §3) and use it for that user's run or conversation, so
   spend, budgets and logs land on the user.
4. **Git identity.** Commits made for a user carry that user as author. The node's forge identity
   (`obachan-agents`) still pushes; P5 adds the user to the PR description and reviewers.
5. **Environments.** Staging and prod are separate end to end (Zitadel, APISIX host, passiv, AgentCtrl).
   WarpSpeed holds one login per environment and shows which one it is using.
6. **Upstream merges.** WarpSpeed changes stay in new modules and settings. Oz code is disabled by
   cargo feature (P6), not deleted or rewritten, so upstream Warp merges keep applying.
7. **License.** `warp-proto-apis` is AGPL-3.0. `agentctrl-mode` compiles its generated code, so it is
   AGPL-compatible and offers its source to its network users. Accepted on 2026-10-08.

## Sub-projects

| # | Sub-project | Repos | Depends on | Delivers |
|---|---|---|---|---|
| P0 | Identity and model access | obachan-config, passiv, stoz3n-agent-dash, warp | — | Zitadel project, roles and WarpSpeed native app; WarpSpeed device-code login; dashboard accepts Zitadel tokens; passiv short-lived user keys |
| P1 | AgentCtrl public run API | stoz3n-agent-ctrl, obachan, obachan-config | P0 | `agentctrl-api` (REST + SSE over `FleetControl`) behind APISIX at `agents[-dev].obachan.dev`; run ownership; per-user model keys and commit author for runs |
| P2 | Run isolation and scale | stoz3n-agent-ctrl | P1, pause/second-node spec | One pod per interactive run (the node launches and watches it); per-run credentials, resource limits, namespace quota |
| P3 | WarpSpeed cloud-run client | warp | P1 | `managed_agent_sessions` on the new API: SSE viewer, environment choice, PR link at run end, hand a cloud run back to a local pane |
| P4 | Agent Mode backend | stoz3n-agent-ctrl, warp, obachan-config | P0 | `agentctrl-mode` implementing `warp_multi_agent_api` (`/ai/multi-agent`, `/ai/passive-suggestions`) on passiv; WarpSpeed "agent server" setting |
| P5 | Triggers and PRs | stoz3n-agent-ctrl, obachan-config | P1 | Gitea/GitLab webhooks and Matrix triggers through APISIX into `TriggerAgent`; AgentCtrl opens the PR at run end |
| P6 | Retire Oz in WarpSpeed | warp | P3, P4 | Oz cargo features off in WarpSpeed builds; Oz entry points hidden; Warp server only used where still needed |

Order: P0 → (P1 ∥ P4) → (P3, P2, P5) → P6. The pause/second-node work (2026-10-07 spec, Spec A)
lands before P2.

### P1 outline — AgentCtrl public run API

- New binary `agentctrl-api` in the agent-ctrl repo (Rust, axum), talking gRPC to the controller with
  the control token. Replaces the dashboard's `/api/agent-control/*` as WarpSpeed's API; the
  dashboard keeps its own routes for its web page.
- Endpoints mirror today's dashboard set under `/v1`: runs list/create/import/get, session
  descriptor, input/close/stop/pause/resume, repos. Events: `GET /v1/runs/{id}/events` as SSE with
  `Last-Event-ID` resume, built on the controller's existing event cursor.
- Ownership: `owner_sub` and `owner_email` on each job (from `X-Userinfo`); members are filtered to
  their own runs.
- For each run the node gets a passiv user key (P0 §3) instead of the shared one, and git author
  `owner_name <owner_email>`.
- obachan: `app_routes` gains `bearer_only` and a per-route `limit-count` keyed on the token subject.
- Idempotency keys, audit log and rate limits carry over from the dashboard design.

### P2 outline — run isolation

- The node stays the launcher: for an interactive run it creates a pod from a template (agent image,
  the run's worktree on the shared-checkouts volume, per-run secret with the user key) and streams its
  stdio over `kubectl exec`-style attach. Process mode stays for local development.
- Limits per run pod; namespace `ResourceQuota` replaces the per-node interactive cap.
- `bypassPermissions` stays acceptable only inside a run pod with no service-account token and a
  NetworkPolicy limited to passiv, the forges and DNS.

### P3 outline — WarpSpeed cloud-run client

- `managed_agent_sessions` talks to `agents[-dev].obachan.dev` with the P0 token; SSE replaces polling.
- Fleet pane and `agent-ctrl` CLI gain the environment indicator, PR link and "Bring back to this
  pane" (export the run's transcript and branch, resume locally; the reverse of the move flow).

### P4 outline — Agent Mode backend

- Starts with a spike (one week, time-boxed) that answers before any build plan:
  1. Which Warp server calls Agent Mode needs besides `/ai/multi-agent` (model list, workspace
     settings, credit/limit checks, login gating) and how WarpSpeed behaves when each fails.
  2. Whether passiv's Anthropic `/v1/messages` streams tool calls and handles cancellation correctly
     through LiteLLM (listed unverified in `obachan/docs/operations/agent-ctrl-passiv-gateway.md`).
  3. The minimal subset of `request.proto`/`response.proto`/`task.proto` (about 3,300 lines) a
     working conversation needs: tool set, task/message events, conversation ids, summarization.
- Then: `agentctrl-mode` (Rust, axum) decodes `Request`, keeps conversation state in Mongo, runs the
  model loop against passiv with the user's key, and streams `ResponseEvent`s. WarpSpeed gets an
  "agent server URL" (per environment) used only by `warp_multi_agent_client`, plus the Zitadel token
  as the bearer.

### P5 outline — triggers and PRs

- APISIX key-auth routes for forge webhooks (one key per forge), mapped to `TriggerAgent` with the
  issue/comment as the prompt and the repo from the allow-list.
- At run end AgentCtrl opens the PR/MR itself (`tea`/`glab`/`gh` per forge) and stores `mr_url`.

### P6 outline — retire Oz

- WarpSpeed build profile drops `cloud_mode`, `cloud_mode_from_local_session`, `oz_handoff`,
  `handoff_local_cloud`, `handoff_cloud_cloud`, `agent_management_view`, `scheduled_ambient_agents`,
  `cloud_environments`, `oz_launch_modal` and related features; fleet pane takes the "agents" entry
  points. Verify each remaining call to `server_root_url()` is intended.

---

## P0 — Identity and model access (detailed)

### P0 §1 Zitadel (obachan-config, both environments)

- Project `agent-platform` with roles `agent-user`, `agent-admin`; "assert roles on authentication"
  and "check authorization on authentication" on.
- Application `warpspeed`: native, PKCE, grant types authorization code, **device code** and refresh
  token; access token type JWT. Redirect URI `http://127.0.0.1/callback` (loopback, any port) for
  the browser flow fallback.
- Application `agent-gateway` (API app for APISIX introspection) moves to P1, where APISIX first
  needs it (revised 2026-10-09).
- Tokens requested with scopes `openid profile email offline_access
  urn:zitadel:iam:org:project:id:<agent-platform>:aud urn:zitadel:iam:org:projects:roles`.
- Staging users and grants: the current dashboard admins get `agent-admin`.
- Done when: a device-code login with `curl` against `id-dev.obachan.dev` returns a JWT access token
  whose audience contains the project id and whose roles claim contains the grant.

### P0 §2 WarpSpeed login (warp)

- `stozn_auth` keeps its public surface (`begin_stozn_login`, `complete_stozn_login`,
  `stored_stozn_identity`, `logout_stozn`) but implements it with the OAuth 2.0 device authorization
  grant against the environment's Zitadel issuer. The existing framed-browser opener shows the
  verification URL.
- Stored per environment: refresh token, access token and expiry in WarpSpeed's DPAPI-protected
  `stozn-auth.bin` (format 2; revised 2026-10-09: same OS-user protection, no new dependency). A request helper refreshes when fewer than 60 s remain; a failed refresh marks the identity
  signed out and the UI offers sign-in again.
- Environment config: issuer, client id, project id, API base URLs, compiled into the single
  `staging` profile (revised 2026-10-09: WarpSpeed stays staging-only per the 2026-10-07 decision; no
  prod selector). `STOZN_TOKEN` keeps working as an override for scripts.
- `managed_agent_sessions` sends the Zitadel access token to the dashboard until P3 moves it.
- Done when: sign-in, restart (silent refresh), sign-out and expired refresh all behave as above;
  unit tests cover token refresh timing and storage per environment.

### P0 §3 passiv: short-lived user keys (passiv)

- New internal endpoint `POST /api/passiv/internal/user-keys`, reachable only in-cluster and
  authenticated with a per-service token from OpenBao (`agentctrl-api`, `agentctrl-mode`).
  Body: `{subject, email, name, models, ttl_seconds, purpose}`.
- It resolves the user through the same path as `tenant/me` (LiteLLM user = email, team from the
  `tenant-<slug>` group, default tenant for users without one), then generates a LiteLLM key with
  `user_id=email`, `duration=ttl` (max 24 h), the requested models within the team's allow-list, and
  metadata `{purpose, service}`. Returns the plaintext key once.
- The user's budget lives on the LiteLLM user (`max_budget`, `budget_duration`), so all of a user's
  short-lived keys share it.
- Guardrail defaults follow machine keys (no PII masking) unless the tenant requires it.
- Done when: an in-cluster call returns a key that works for `claude-sonnet` on `/v1/messages`,
  spend shows under the user in `tenant/cost`, and the key stops working after its TTL.

### P0 §4 Dashboard accepts Zitadel tokens (stoz3n-agent-dash)

- `/api/agent-control/*` accepts either the existing dashboard JWT or a Zitadel access token
  (validated against the environment's issuer JWKS, audience = project id). `agent-admin` maps to
  today's `admin` role check.
- The `/api/auth/warpspeed/*` device login stays until P3 ships, then is removed.
- Done when: WarpSpeed on P0 lists and opens AgentCtrl runs with only a Zitadel login.

### P0 errors and edge cases

- No `agent-platform` grant: WarpSpeed shows "Your account has no access to the agent platform"
  with the issuer host, not a generic 401.
- Clock skew: token validation allows 60 s leeway.
- passiv down: services fail the run or request with a clear "model gateway unavailable" status;
  they never fall back to a shared key.

### P0 testing

- warp: unit tests for token refresh, storage, environment selection; CLI test for `STOZN_TOKEN`
  override.
- passiv: API tests for `internal/user-keys` (auth required, TTL cap, model allow-list, team
  resolution) using the existing LiteLLM test double.
- dashboard: route tests for both token kinds and role mapping.
- Staging E2E: sign in from a fresh WarpSpeed build, list runs, open one; mint a user key in-cluster
  and make one `/v1/messages` call billed to the user.

## Open questions (decided before the sub-project's own spec)

- P2: pod-per-run attach mechanism (Kubernetes attach API vs a small in-pod relay).
- P4: conversation storage retention and whether summaries stay server-side only.
- P5: Matrix trigger syntax and who may trigger runs on which repos.
