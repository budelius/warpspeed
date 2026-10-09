# P0 — Identity and Model Access Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** WarpSpeed signs in with Zitadel (device code + refresh), the STOZN dashboard accepts those tokens for AgentCtrl, and passiv mints short-lived per-user model keys for in-cluster services.

**Architecture:** A new Zitadel project `agent-platform` (roles `agent-user`, `agent-admin`) with a public native app `warpspeed` is provisioned by a new idempotent `obachan auth provision-agent-platform-sso` command. WarpSpeed's `managed_agent_sessions` swaps the dashboard device login for the OAuth 2.0 device authorization grant and refreshes access tokens inside `dashboard_token()`. The dashboard verifies Zitadel RS256 access tokens on `/api/agent-control/*` next to its HS256 session JWTs. passiv gets `POST /api/passiv/internal/user-keys`, guarded by per-service bearer tokens and blocked at the public edge.

**Tech Stack:** Python 3.13 + click + granny (obachan), Python + FastAPI + LiteLLM (passiv), TypeScript on node:http + `node:crypto` (dashboard), Rust + blocking reqwest (WarpSpeed).

Spec: `docs/superpowers/specs/2026-10-08-self-hosted-agent-platform-design.md` §P0.

## Global Constraints

- Staging only. Zitadel issuer `https://id-dev.obachan.dev`; WarpSpeed keeps its single `staging` profile (user decision 2026-10-07; no prod profile, no `--agent-env` flag).
- Zitadel project name `agent-platform`; role keys exactly `agent-user` and `agent-admin`; app name `warpspeed`; org `obachan`.
- WarpSpeed maps `agent-admin` → role `admin`, `agent-user` → role `user`; no role → sign-in fails with "Your account has no access to the agent platform (id-dev.obachan.dev)".
- Access tokens are refreshed when fewer than 60 s remain. Token validation allows 60 s clock leeway.
- passiv user keys: TTL max 86 400 s; plaintext returned once; never written to the KeyStore; never a fallback to a shared key.
- Never `git add -A` in the warp tree; stage explicit paths. Every repo: work on a feature branch in its own worktree, push to its remotes (`gitea` and `gitlab` for warp; `origin` for the others).
- Commit messages end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

## Deviations from the spec (recorded here and in the spec)

- Refresh token storage reuses WarpSpeed's DPAPI-protected `stozn-auth.bin` (format 2) instead of a new OS credential-store dependency: same OS-user protection on Windows, no new crate.
- No `agent-gateway` introspection app in P0; APISIX is P1 and creates it there.
- passiv's internal endpoint takes the user identity in the request body from an authenticated service, not from forward-auth headers.

## Repos, worktrees, branches

| Repo | Base | Worktree | Branch |
|---|---|---|---|
| obachan | `origin/dev` | `T:/projects/dev-ops/obachan-agent-platform-sso` | `feat/agent-platform-sso` |
| passiv | `origin/main` | `T:/projects/dev-ops/passiv-internal-user-keys` | `feat/internal-user-keys` |
| stoz3n-agent-dash | `origin/dev` | `T:/projects/SOTZ3N/stoz3n-agent-dash-zitadel-bearer` | `feat/zitadel-bearer-agent-control` |
| warp | `docs/self-hosted-agent-backend` | `T:/projects/dev-ops/warp-self-hosted-agents` | `feat/p0-zitadel-login` |

Create each with `git -C <repo> fetch origin && git -C <repo> worktree add <path> -b <branch> <base>` (warp: `git -C T:/projects/dev-ops/warp-self-hosted-agents switch -c feat/p0-zitadel-login`).

Parallelism: Tasks 1, 3 and 5 are independent. Task 2 needs Task 1; Task 4 needs Task 3; Task 6 needs Task 2 (it compiles in the provisioned ids); Task 7 needs Task 6; Task 8 needs everything.

---

### Task 1: obachan — `provision-agent-platform-sso`

**Files:**
- Create: `src/obachan/agent_platform_oidc.py`
- Modify: `src/obachan/cli/auth.py` (new command after `provision_developer_sso`, ~line 160)
- Test: `tests/unit/test_agent_platform_oidc.py`

**Interfaces:**
- Consumes: granny `ZitadelClient` methods `request`, `ensure_project`, `find_project`, `ensure_role`, `find_oidc_app`, `get_app`; obachan `_scope_client_to_org` (from `obachan.developer_services_oidc`), `merge_kv_fields` (from `obachan.cli.vault`), `_zitadel_admin_client(env)`.
- Produces: `provision_agent_platform_oidc(cfg, env, client) -> dict[str, str]` with keys `project_id`, `app_id`, `client_id`; OpenBao `obachan/<env>/warpspeed/runtime` fields `oidc_project_id`, `oidc_app_id`, `oidc_client_id`.

- [ ] **Step 1: Write the failing tests**

```python
# SPDX-License-Identifier: Apache-2.0
from types import SimpleNamespace

import pytest
from granny.zitadel.client import ZitadelError

from obachan import agent_platform_oidc
from obachan.agent_platform_oidc import (
    AGENT_PLATFORM_ROLES,
    WARPSPEED_APP_BODY,
    provision_agent_platform_oidc,
)


class FakeClient:
    def __init__(self, existing_app=None, unchanged=False) -> None:
        self._session = SimpleNamespace(headers={})
        self.unchanged = unchanged
        self.calls: list[tuple] = []
        self.roles: list[str] = []
        self.existing_app = existing_app

    def request(self, method, path, body=None):
        self.calls.append((method, path, body))
        if method == "PUT" and self.unchanged:
            raise ZitadelError(400, '{"code":9,"message":"No changes (COMMAND-1m88i)"}')
        if path == "/admin/v1/orgs/_search":
            return {"result": [{"id": "org-1", "name": "obachan"}]}
        if path == "/management/v1/projects/project-1/apps/oidc":
            return {"appId": "app-1", "clientId": "client-1"}
        return {}

    def ensure_project(self, name):
        assert name == "agent-platform"
        return "project-1"

    def ensure_role(self, project_id, role_key, *, display_name=None, group=""):
        self.roles.append(role_key)
        return {"key": role_key}

    def find_oidc_app(self, project_id, name):
        return self.existing_app

    def get_app(self, project_id, app_id):
        return {"oidcConfig": {"clientId": "client-1", **WARPSPEED_APP_BODY}}


@pytest.fixture
def stored(monkeypatch):
    writes = []
    monkeypatch.setattr(
        agent_platform_oidc,
        "_persist",
        lambda env, fields: writes.append((env, fields)),
    )
    return writes


def _cfg():
    return SimpleNamespace(platform=SimpleNamespace(zitadel=SimpleNamespace(org_name="obachan")))


def test_creates_project_roles_assertion_and_native_device_app(stored) -> None:
    client = FakeClient()
    result = provision_agent_platform_oidc(_cfg(), "staging", client)

    assert result == {"project_id": "project-1", "app_id": "app-1", "client_id": "client-1"}
    assert client.roles == list(AGENT_PLATFORM_ROLES)
    assert (
        "PUT",
        "/management/v1/projects/project-1",
        {"name": "agent-platform", "projectRoleAssertion": True, "projectRoleCheck": True},
    ) in client.calls
    created = [c for c in client.calls if c[1].endswith("/apps/oidc")]
    assert created == [
        ("POST", "/management/v1/projects/project-1/apps/oidc", {"name": "warpspeed", **WARPSPEED_APP_BODY})
    ]
    assert WARPSPEED_APP_BODY["appType"] == "OIDC_APP_TYPE_NATIVE"
    assert WARPSPEED_APP_BODY["authMethodType"] == "OIDC_AUTH_METHOD_TYPE_NONE"
    assert WARPSPEED_APP_BODY["grantTypes"] == [
        "OIDC_GRANT_TYPE_DEVICE_CODE",
        "OIDC_GRANT_TYPE_REFRESH_TOKEN",
    ]
    assert WARPSPEED_APP_BODY["accessTokenType"] == "OIDC_TOKEN_TYPE_JWT"
    assert WARPSPEED_APP_BODY["accessTokenRoleAssertion"] is True
    assert stored == [
        (
            "staging",
            {"oidc_project_id": "project-1", "oidc_app_id": "app-1", "oidc_client_id": "client-1"},
        )
    ]


def test_existing_app_is_updated_in_place_not_recreated(stored) -> None:
    client = FakeClient(existing_app={"id": "app-1", "oidcConfig": {"clientId": "client-1"}})
    result = provision_agent_platform_oidc(_cfg(), "staging", client)

    assert result["client_id"] == "client-1"
    assert not [c for c in client.calls if c[0] == "POST" and c[1].endswith("/apps/oidc")]
    assert (
        "PUT",
        "/management/v1/projects/project-1/apps/app-1/oidc_config",
        WARPSPEED_APP_BODY,
    ) in client.calls


def test_rerun_with_nothing_to_change_succeeds(stored) -> None:
    client = FakeClient(
        existing_app={"id": "app-1", "oidcConfig": {"clientId": "client-1"}}, unchanged=True
    )
    assert provision_agent_platform_oidc(_cfg(), "staging", client)["app_id"] == "app-1"


def test_other_zitadel_errors_still_fail(stored) -> None:
    class Broken(FakeClient):
        def request(self, method, path, body=None):
            if method == "PUT":
                raise ZitadelError(403, "permission denied")
            return super().request(method, path, body)

    with pytest.raises(ZitadelError):
        provision_agent_platform_oidc(_cfg(), "staging", Broken())
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `uv run pytest tests/unit/test_agent_platform_oidc.py -q`
Expected: FAIL with `ModuleNotFoundError: No module named 'obachan.agent_platform_oidc'`.

- [ ] **Step 3: Implement `src/obachan/agent_platform_oidc.py`**

```python
# SPDX-License-Identifier: Apache-2.0
"""Zitadel project and native device-code app for WarpSpeed's agent platform login."""

from __future__ import annotations

from typing import TYPE_CHECKING, Any

import click

from obachan.developer_services_oidc import _scope_client_to_org

if TYPE_CHECKING:
    from granny.zitadel.client import ZitadelClient

    from obachan.config.schema import ObachanConfig

PROJECT_NAME = "agent-platform"
APP_NAME = "warpspeed"
AGENT_PLATFORM_ROLES = ("agent-user", "agent-admin")

# granny's create_oidc_app only builds confidential web apps, so the native public client is
# written with the raw Management API. accessTokenRoleAssertion puts the roles into the JWT
# access token itself, which the dashboard and APISIX validate without calling userinfo.
WARPSPEED_APP_BODY: dict[str, Any] = {
    "redirectUris": [],
    "responseTypes": ["OIDC_RESPONSE_TYPE_CODE"],
    "grantTypes": ["OIDC_GRANT_TYPE_DEVICE_CODE", "OIDC_GRANT_TYPE_REFRESH_TOKEN"],
    "appType": "OIDC_APP_TYPE_NATIVE",
    "authMethodType": "OIDC_AUTH_METHOD_TYPE_NONE",
    "postLogoutRedirectUris": [],
    "version": "OIDC_VERSION_1_0",
    "devMode": False,
    "accessTokenType": "OIDC_TOKEN_TYPE_JWT",
    "accessTokenRoleAssertion": True,
    "idTokenRoleAssertion": True,
    "idTokenUserinfoAssertion": True,
}


def _put(client: ZitadelClient, path: str, body: dict[str, Any]) -> None:
    """PUT that treats Zitadel's "No changes" rejection of an identical update as success."""
    from granny.zitadel.client import ZitadelError

    try:
        client.request("PUT", path, body=body)
    except ZitadelError as exc:
        if exc.status != 400 or "No changes" not in exc.body:
            raise


def _persist(env: str, fields: dict[str, str]) -> None:
    from obachan.cli.vault import merge_kv_fields

    merge_kv_fields(env, "warpspeed/runtime", fields, cluster_env=env)


def provision_agent_platform_oidc(
    cfg: ObachanConfig, env: str, client: ZitadelClient
) -> dict[str, str]:
    """Ensure the agent-platform project, its roles and the WarpSpeed app; persist the ids."""
    _scope_client_to_org(client, cfg.platform.zitadel.org_name)
    project_id = client.ensure_project(PROJECT_NAME)
    _put(
        client,
        f"/management/v1/projects/{project_id}",
        {"name": PROJECT_NAME, "projectRoleAssertion": True, "projectRoleCheck": True},
    )
    for role in AGENT_PLATFORM_ROLES:
        client.ensure_role(project_id, role, display_name=role)

    existing = client.find_oidc_app(project_id, APP_NAME)
    if existing is None:
        created = client.request(
            "POST",
            f"/management/v1/projects/{project_id}/apps/oidc",
            body={"name": APP_NAME, **WARPSPEED_APP_BODY},
        )
        app_id = created.get("appId") or created.get("id")
        client_id = created.get("clientId")
    else:
        app_id = existing.get("id") or existing.get("appId")
        client_id = (existing.get("oidcConfig") or {}).get("clientId") or existing.get("clientId")
        if not isinstance(app_id, str) or not app_id:
            raise click.ClickException("Zitadel warpspeed application has no app id")
        _put(client, f"/management/v1/projects/{project_id}/apps/{app_id}/oidc_config", WARPSPEED_APP_BODY)

    if not isinstance(app_id, str) or not isinstance(client_id, str) or not client_id:
        raise click.ClickException("Zitadel did not return complete warpspeed identifiers")
    _persist(
        env,
        {"oidc_project_id": project_id, "oidc_app_id": app_id, "oidc_client_id": client_id},
    )
    return {"project_id": project_id, "app_id": app_id, "client_id": client_id}
```

- [ ] **Step 4: Add the CLI command in `src/obachan/cli/auth.py` after `provision_developer_sso`**

```python
@auth.command("provision-agent-platform-sso")
@click.option("--env", required=True, type=click.Choice(["staging", "prod"]))
@destructive()
def provision_agent_platform_sso(env: str, dry_run: bool, confirm: bool) -> None:
    """Provision the agent-platform Zitadel project and WarpSpeed's device-code app."""
    del confirm
    from obachan.agent_platform_oidc import (
        AGENT_PLATFORM_ROLES,
        APP_NAME,
        PROJECT_NAME,
        provision_agent_platform_oidc,
    )
    from obachan.config.loader import load_config

    if dry_run:
        click.echo(
            f"dry-run: would ensure Zitadel project {PROJECT_NAME!r} with roles "
            f"{', '.join(AGENT_PLATFORM_ROLES)} and native app {APP_NAME!r} (device code, "
            f"refresh token, JWT access tokens) and persist ids at obachan/{env}/warpspeed/runtime"
        )
        return
    cfg = load_config(Path("obachan.toml"), env=env)
    result = provision_agent_platform_oidc(cfg, env, _zitadel_admin_client(env))
    click.echo(
        f"agent platform SSO ready: project={result['project_id']} "
        f"app={result['app_id']} client={result['client_id']}"
    )
```

- [ ] **Step 5: Run tests and lint**

Run: `uv run pytest tests/unit/test_agent_platform_oidc.py tests/unit/test_developer_services_oidc.py -q && uv run ruff check src/obachan/agent_platform_oidc.py src/obachan/cli/auth.py tests/unit/test_agent_platform_oidc.py`
Expected: all pass, no lint findings.

- [ ] **Step 6: Commit and push**

```bash
git add src/obachan/agent_platform_oidc.py src/obachan/cli/auth.py tests/unit/test_agent_platform_oidc.py
git commit -m "feat(auth): provision the agent-platform project and WarpSpeed device-code app"
git push -u origin feat/agent-platform-sso
```

Open a PR to `dev` with the repo's usual tooling (`tea pulls create` against git-dev.obachan.dev).

---

### Task 2: Provision staging and verify a device-code login (operator task)

**Files:** none in git; results recorded in Task 6's constants and in `docs/superpowers/plans/2026-10-09-p0-identity-and-model-access.md` (append a "Provisioned ids" note).

**Interfaces:**
- Consumes: Task 1 command.
- Produces: staging `PROJECT_ID`, `CLIENT_ID` used by Tasks 5 and 6; a role grant for the operator's staging user.

- [ ] **Step 1: Dry run, then provision** (from the Task 1 worktree, with obachan-config's `obachan.toml` as cwd; staging kubeconfig per `agentctrl-staging-access` memory)

```bash
uv run --project T:/projects/dev-ops/obachan-agent-platform-sso obachan auth provision-agent-platform-sso --env staging --dry-run
uv run --project T:/projects/dev-ops/obachan-agent-platform-sso obachan auth provision-agent-platform-sso --env staging --confirm
```
Expected: `agent platform SSO ready: project=<pid> app=<aid> client=<cid>`. Run it a second time; expected the same ids and no new app.

- [ ] **Step 2: Grant the operator `agent-admin`**

```bash
export ZITADEL_DOMAIN=https://id-dev.obachan.dev
export ZITADEL_API_TOKEN=$(kubectl --kubeconfig=$HOME/.kube/obachan-staging -n zitadel get secret iam-admin-pat -o jsonpath='{.data.pat}' | base64 -d)
granny zitadel grant-user-role agent-platform <staging-username> agent-admin
```

- [ ] **Step 3: Confirm discovery and the device flow by hand**

```bash
curl -s https://id-dev.obachan.dev/.well-known/openid-configuration | jq '{device_authorization_endpoint, token_endpoint, jwks_uri, issuer}'
SCOPE="openid profile email offline_access urn:zitadel:iam:org:project:id:<pid>:aud urn:zitadel:iam:org:projects:roles"
curl -s -X POST https://id-dev.obachan.dev/oauth/v2/device_authorization -d client_id=<cid> --data-urlencode "scope=$SCOPE"
# open verification_uri_complete in a browser, approve, then:
curl -s -X POST https://id-dev.obachan.dev/oauth/v2/token -d client_id=<cid> \
  -d grant_type=urn:ietf:params:oauth:grant-type:device_code -d device_code=<device_code>
```
Expected: endpoints `https://id-dev.obachan.dev/oauth/v2/{device_authorization,token,keys}`; the token response has `access_token`, `refresh_token`, `expires_in`. Decode the access token payload (`cut -d. -f2 | base64 -d`) and confirm: `aud` contains `<pid>`, and `urn:zitadel:iam:org:project:<pid>:roles` (or `urn:zitadel:iam:org:project:roles`) contains `agent-admin`. If the roles claim is missing, set "Assert Roles on Authentication" on the project in the console and record that the PUT in Task 1 did not take effect.

- [ ] **Step 4: Record the ids** — append to this plan file under a new heading `## Provisioned ids (staging)` the issuer, project id, client id and the endpoint paths, then commit in the warp worktree (`git add docs/superpowers/plans/2026-10-09-p0-identity-and-model-access.md`).

---

### Task 3: passiv — internal user keys endpoint and edge block

**Files:**
- Create: `src/passiv/api/internal_keys.py`
- Modify: `src/passiv/api/app.py` (register routes inside `build_app()`, before `add_user_management_routes(app)` at ~line 1410)
- Modify: `deploy/obachan/passiv-edge/Caddyfile` (inside the `@gated` handler, before `handle /api/passiv/*`)
- Modify: `deploy/obachan/passiv-api/obachan.app.toml`, `deploy/obachan/passiv-api/obachan.prod.app.toml` (`secret_keys` += `passiv_internal_service_tokens`)
- Test: `tests/unit/test_internal_keys.py`

**Interfaces:**
- Consumes: `TenantResolver`-style helpers `primary_tenant_slug`, `safe_username`, `CallerIdentity` from `passiv.api.tenant`; `LiteLLMClient.ensure_team`, `ensure_user`, `generate_key`; `lift` and `litellm()` closures in `build_app`; `register_secret` from `passiv.state.redaction`.
- Produces: `POST /api/passiv/internal/user-keys`, header `Authorization: Bearer <service token>`, body `{"email": str, "username": str, "groups": [str], "models": [str], "ttl_seconds": int, "purpose": str}`, response `201 {"key": str, "key_alias": str, "expires_in": int, "user_id": str, "team_id": str, "tenant_slug": str}`. Env `PASSIV_INTERNAL_SERVICE_TOKENS` = comma-separated `service:token` pairs.

- [ ] **Step 1: Write the failing tests**

```python
from __future__ import annotations

import pytest

fastapi = pytest.importorskip("fastapi")
from fastapi.testclient import TestClient  # noqa: E402

from passiv.api.app import build_app  # noqa: E402

TOKENS = "agentctrl-api:tok-api,agentctrl-mode:tok-mode"


@pytest.fixture
def client(monkeypatch, tmp_path):
    monkeypatch.setattr("passiv.api.keystore.DEFAULT_DB_PATH", tmp_path / "keys.db")
    monkeypatch.setenv("PASSIV_INTERNAL_SERVICE_TOKENS", TOKENS)
    generated: list[dict] = []
    monkeypatch.setattr(
        "passiv.api.app.LiteLLMClient.ensure_team",
        lambda self, *, team_alias, metadata=None: {"team_id": f"tid-{team_alias}"},
    )
    monkeypatch.setattr(
        "passiv.api.app.LiteLLMClient.ensure_user",
        lambda self, *, user_email, teams=None: {"user_id": f"uid-{user_email}"},
    )

    def _generate(self, **kw):
        generated.append(kw)
        return {"key": f"sk-short-{len(generated)}", **kw}

    monkeypatch.setattr("passiv.api.app.LiteLLMClient.generate_key", _generate)
    c = TestClient(build_app())
    c.generated = generated
    return c


def _body(**overrides):
    body = {
        "email": "ada@example.com",
        "username": "ada",
        "groups": ["tenant-mezzanin"],
        "models": ["claude-sonnet"],
        "ttl_seconds": 3600,
        "purpose": "agent-run job-1",
    }
    body.update(overrides)
    return body


def test_requires_a_known_service_token(client) -> None:
    assert client.post("/api/passiv/internal/user-keys", json=_body()).status_code == 401
    bad = {"Authorization": "Bearer nope"}
    assert client.post("/api/passiv/internal/user-keys", json=_body(), headers=bad).status_code == 401


def test_mints_a_short_lived_key_bound_to_the_user(client) -> None:
    response = client.post(
        "/api/passiv/internal/user-keys",
        json=_body(),
        headers={"Authorization": "Bearer tok-api"},
    )
    assert response.status_code == 201
    data = response.json()
    assert data["key"] == "sk-short-1"
    assert data["tenant_slug"] == "mezzanin"
    assert data["user_id"] == "uid-ada@example.com"
    assert data["expires_in"] == 3600
    [kw] = client.generated
    assert kw["user_id"] == "uid-ada@example.com"
    assert kw["team_id"] == "tid-team-mezzanin"
    assert kw["models"] == ["claude-sonnet"]
    assert kw["duration"] == "3600s"
    assert kw["key_alias"].startswith("svc-agentctrl-api-ada-")
    assert kw["metadata"] == {
        "passiv_tenant_slug": "mezzanin",
        "service": "agentctrl-api",
        "purpose": "agent-run job-1",
        "subject_email": "ada@example.com",
    }


@pytest.mark.parametrize(
    "override",
    [
        {"ttl_seconds": 0},
        {"ttl_seconds": 86_401},
        {"models": []},
        {"email": ""},
        {"purpose": "x" * 201},
    ],
)
def test_rejects_invalid_requests(client, override) -> None:
    response = client.post(
        "/api/passiv/internal/user-keys",
        json=_body(**override),
        headers={"Authorization": "Bearer tok-mode"},
    )
    assert response.status_code == 422
    assert client.generated == []


def test_users_without_tenant_group_land_in_default(client) -> None:
    response = client.post(
        "/api/passiv/internal/user-keys",
        json=_body(groups=[]),
        headers={"Authorization": "Bearer tok-mode"},
    )
    assert response.json()["tenant_slug"] == "default"
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `uv run pytest tests/unit/test_internal_keys.py -q`
Expected: FAIL (404 for the route / assertion errors).

- [ ] **Step 3: Implement `src/passiv/api/internal_keys.py`**

```python
"""Short-lived per-user LiteLLM keys for authenticated in-cluster services."""

from __future__ import annotations

import os
import secrets as pysecrets
import uuid

from fastapi import HTTPException, Request
from pydantic import BaseModel, Field

from passiv.api.tenant import CallerIdentity, primary_tenant_slug, safe_username

MAX_TTL_SECONDS = 86_400


class UserKeyRequest(BaseModel):
    email: str = Field(min_length=3, max_length=320)
    username: str = Field(default="", max_length=128)
    groups: list[str] = Field(default_factory=list, max_length=64)
    models: list[str] = Field(min_length=1, max_length=32)
    ttl_seconds: int = Field(gt=0, le=MAX_TTL_SECONDS)
    purpose: str = Field(min_length=1, max_length=200)


def service_tokens() -> dict[str, str]:
    """Parse ``PASSIV_INTERNAL_SERVICE_TOKENS`` (``service:token`` pairs, comma-separated)."""
    tokens: dict[str, str] = {}
    for pair in os.environ.get("PASSIV_INTERNAL_SERVICE_TOKENS", "").split(","):
        service, _, token = pair.strip().partition(":")
        if service and token:
            tokens[service] = token
    return tokens


def authenticate_service(request: Request) -> str:
    header = request.headers.get("authorization", "")
    supplied = header[7:].strip() if header.lower().startswith("bearer ") else ""
    if supplied:
        for service, token in service_tokens().items():
            if pysecrets.compare_digest(supplied, token):
                return service
    raise HTTPException(status_code=401, detail={"error": "service token required"})


def key_request_identity(body: UserKeyRequest) -> CallerIdentity:
    return CallerIdentity(email=body.email, username=body.username, groups=tuple(body.groups))


def key_alias(service: str, identity: CallerIdentity) -> str:
    return f"svc-{service}-{safe_username(identity)}-{uuid.uuid4().hex[:8]}"


__all__ = [
    "MAX_TTL_SECONDS",
    "UserKeyRequest",
    "authenticate_service",
    "key_alias",
    "key_request_identity",
    "primary_tenant_slug",
]
```

- [ ] **Step 4: Register the route in `build_app()` (`src/passiv/api/app.py`), next to the other internal routes (~line 703)**

```python
    @app.post("/api/passiv/internal/user-keys", status_code=201)
    def internal_user_key(request: Request, body: UserKeyRequest) -> dict[str, Any]:
        """Mint a short-lived key for a user on behalf of an authenticated service."""
        service = authenticate_service(request)
        identity = key_request_identity(body)
        slug = primary_tenant_slug(identity)
        client = litellm()
        team = lift(
            "litellm",
            lambda: client.ensure_team(
                team_alias=f"team-{slug}", metadata={"passiv_tenant_slug": slug}
            ),
        )
        team_id = str(team.get("team_id") or "")
        user = lift(
            "litellm",
            lambda: client.ensure_user(user_email=body.email, teams=[team_id] if team_id else None),
        )
        user_id = str(user.get("user_id") or "")
        alias = key_alias(service, identity)
        created = lift(
            "litellm",
            lambda: client.generate_key(
                key_alias=alias,
                user_id=user_id or None,
                team_id=team_id or None,
                models=body.models,
                duration=f"{body.ttl_seconds}s",
                metadata={
                    "passiv_tenant_slug": slug,
                    "service": service,
                    "purpose": body.purpose,
                    "subject_email": body.email,
                },
            ),
        )
        key = created.get("key") or created.get("token")
        if not key:
            raise HTTPException(status_code=502, detail={"error": "litellm returned no key"})
        register_secret(key)
        return {
            "key": key,
            "key_alias": alias,
            "expires_in": body.ttl_seconds,
            "user_id": user_id,
            "team_id": team_id,
            "tenant_slug": slug,
        }
```

Add to the imports at the top of `app.py`:

```python
from passiv.api.internal_keys import (
    UserKeyRequest,
    authenticate_service,
    key_alias,
    key_request_identity,
)
from passiv.api.tenant import primary_tenant_slug
```
(If `primary_tenant_slug` or `register_secret` is already imported, do not duplicate it.)

- [ ] **Step 5: Run the tests**

Run: `uv run pytest tests/unit/test_internal_keys.py tests/unit/test_api_app.py tests/unit/test_tenant.py -q && uv run ruff check src tests`
Expected: all pass.

- [ ] **Step 6: Block internal routes at the public edge** — in `deploy/obachan/passiv-edge/Caddyfile`, inside the `@gated` site block, directly before `handle /api/passiv/* {`:

```caddy
	handle /api/passiv/internal/* {
		respond 404
	}
```
Gateway-controls callers use the in-cluster service URL (`passiv-api.passiv-api.svc.cluster.local:4100`) and are unaffected. If the repo has Caddyfile tests (`grep -rl Caddyfile tests`), add an assertion that the internal block precedes the `/api/passiv/*` handler.

- [ ] **Step 7: Declare the secret** — add `"passiv_internal_service_tokens"` to `secret_keys` in both `deploy/obachan/passiv-api/obachan.app.toml` and `obachan.prod.app.toml`. Do not deploy yet (Task 4 seeds OpenBao first).

- [ ] **Step 8: Commit and push**

```bash
git add src/passiv/api/internal_keys.py src/passiv/api/app.py tests/unit/test_internal_keys.py deploy/obachan/passiv-edge/Caddyfile deploy/obachan/passiv-api/obachan.app.toml deploy/obachan/passiv-api/obachan.prod.app.toml
git commit -m "feat(api): mint short-lived per-user keys for in-cluster services"
git push -u origin feat/internal-user-keys
```

---

### Task 4: passiv — seed, deploy to staging, verify a minted key

**Interfaces:**
- Consumes: Task 3.
- Produces: OpenBao `obachan/staging/passiv-api` property `passiv_internal_service_tokens` = `agentctrl-api:<random>,agentctrl-mode:<random>`; the same per-service tokens stored for later consumers at `obachan/staging/agent-ctrl/passiv-internal` (`agentctrl_api_token`, `agentctrl_mode_token`).

- [ ] **Step 1: Seed OpenBao before deploying** (ExternalSecret reconciliation is atomic; see `passiv/docs/geo-research-gateway.md:36-37`). Generate two tokens with `python -c "import secrets;print(secrets.token_urlsafe(32))"` and write them with the repo's documented vault tooling (`deploy/obachan/README.md:113-118`).
- [ ] **Step 2: Merge `feat/internal-user-keys` to `main` via PR and deploy staging** with the repo's documented deploy flow for `passiv-api` and `passiv-edge` (staging spec `envs=["staging"]`).
- [ ] **Step 3: Verify in-cluster**

```bash
kubectl --kubeconfig=$HOME/.kube/obachan-staging -n passiv-api run keytest --rm -i --restart=Never --image=curlimages/curl -- \
  curl -s -X POST http://passiv-api.passiv-api.svc.cluster.local:4100/api/passiv/internal/user-keys \
  -H "Authorization: Bearer <agentctrl-api token>" -H 'Content-Type: application/json' \
  -d '{"email":"<operator email>","username":"<operator>","groups":[],"models":["claude-sonnet"],"ttl_seconds":600,"purpose":"p0-e2e"}'
```
Expected: 201 with a `key`. Then `curl https://gateway-dev.mezzanin.eu/v1/messages -H "x-api-key: <key>" -H "anthropic-version: 2023-06-01" -H 'content-type: application/json' -d '{"model":"claude-sonnet","max_tokens":16,"messages":[{"role":"user","content":"ping"}]}'` returns a message, and the spend appears under the user in `GET /api/passiv/tenant/cost`.
- [ ] **Step 4: Verify the edge block** — `curl -s -o /dev/null -w '%{http_code}' https://passiv-dev.mezzanin.eu/api/passiv/internal/user-keys -X POST` from outside returns `404` (or the SSO redirect when unauthenticated, never 401/422 from passiv-api).
- [ ] **Step 5: After 10 minutes** the key returns 401 from the gateway.

---

### Task 5: dashboard — accept Zitadel access tokens on `/api/agent-control/*`

**Files:**
- Create: `server/zitadel-token.ts`
- Modify: `server/app.ts` (the agent-control branch at ~line 625; `Config`/`getConfig()` ~line 152/232)
- Modify: `deploy/obachan/runtime-secret-staging.yaml` (literal `AGENT_PLATFORM_PROJECT_ID`)
- Modify: `scripts/check-agent-control.mjs` (new checks)

**Interfaces:**
- Consumes: `extractRoles(userinfo, oidc)` (app.ts:1203, exported), `canonicalMxidServerName(config)`, `readSession(request, config)`, `SessionClaims`.
- Produces: `verifyZitadelAccessToken(token, opts): Promise<ZitadelAccessClaims | null>`; config `agentPlatform: { issuer: string; projectId: string } | null` from env `AGENT_PLATFORM_ISSUER` (default `OIDC_ISSUER`) and `AGENT_PLATFORM_PROJECT_ID`.

- [ ] **Step 1: Write the failing checks** — append to `scripts/check-agent-control.mjs` (after the existing top-level `handleDashboardRequest` checks):

```js
import { generateKeyPairSync, sign as cryptoSign } from "node:crypto";

const { publicKey, privateKey } = generateKeyPairSync("rsa", { modulusLength: 2048 });
const jwk = { ...publicKey.export({ format: "jwk" }), kid: "k1", alg: "RS256", use: "sig" };
const ISSUER = "https://id.example";
const PROJECT = "agent-project";

function zitadelToken(payload, { kid = "k1", key = privateKey } = {}) {
  const enc = (v) => Buffer.from(JSON.stringify(v)).toString("base64url");
  const head = enc({ alg: "RS256", kid, typ: "JWT" });
  const body = enc(payload);
  const sig = cryptoSign("RSA-SHA256", Buffer.from(`${head}.${body}`), key).toString("base64url");
  return `${head}.${body}.${sig}`;
}

const realFetch = globalThis.fetch;
globalThis.fetch = async (input, init) => {
  const url = String(input);
  if (url === `${ISSUER}/oauth/v2/keys`) return new Response(JSON.stringify({ keys: [jwk] }));
  if (url === `${ISSUER}/oidc/v1/userinfo`) {
    return new Response(JSON.stringify({ sub: "2847", preferred_username: "ada", name: "Ada" }));
  }
  return realFetch(input, init);
};

const { verifyZitadelAccessToken } = await import("../dist/server/zitadel-token.js");
const now = Math.floor(Date.now() / 1000);
const good = {
  iss: ISSUER,
  sub: "2847",
  aud: ["client-1", PROJECT],
  exp: now + 300,
  nbf: now - 5,
  [`urn:zitadel:iam:org:project:${PROJECT}:roles`]: { "agent-admin": { org: "obachan" } },
};
const opts = { issuer: ISSUER, projectId: PROJECT };

await check("zitadel: a valid admin token verifies with its roles", async () => {
  const claims = await verifyZitadelAccessToken(zitadelToken(good), opts);
  assert.equal(claims?.sub, "2847");
  assert.deepEqual(claims?.roles, ["agent-admin"]);
  assert.equal(claims?.preferredUsername, "ada");
});
await check("zitadel: wrong audience is rejected", async () => {
  assert.equal(await verifyZitadelAccessToken(zitadelToken({ ...good, aud: ["other"] }), opts), null);
});
await check("zitadel: wrong issuer is rejected", async () => {
  assert.equal(await verifyZitadelAccessToken(zitadelToken({ ...good, iss: "https://evil" }), opts), null);
});
await check("zitadel: expired beyond leeway is rejected, within leeway accepted", async () => {
  assert.equal(await verifyZitadelAccessToken(zitadelToken({ ...good, exp: now - 61 }), opts), null);
  assert.notEqual(await verifyZitadelAccessToken(zitadelToken({ ...good, exp: now - 30 }), opts), null);
});
await check("zitadel: a token signed by another key is rejected", async () => {
  const other = generateKeyPairSync("rsa", { modulusLength: 2048 }).privateKey;
  assert.equal(await verifyZitadelAccessToken(zitadelToken(good, { key: other }), opts), null);
});
await check("zitadel: HS256 dashboard tokens are not accepted by the Zitadel verifier", async () => {
  assert.equal(await verifyZitadelAccessToken(sessionToken({ sub: "x", role: "admin", kind: "session" }), opts), null);
});
await check("agent-control: admin Zitadel bearer passes the guard, user role is forbidden", async () => {
  process.env.AGENT_PLATFORM_ISSUER = ISSUER;
  process.env.AGENT_PLATFORM_PROJECT_ID = PROJECT;
  const admin = await handleDashboardRequest(new Request("https://dash.example/api/agent-control/overview", {
    headers: { authorization: `Bearer ${zitadelToken(good)}` },
  }));
  assert.equal(admin.status, 404); // passes auth, then hits "not configured" like the HS256 check above
  const userToken = zitadelToken({
    ...good,
    [`urn:zitadel:iam:org:project:${PROJECT}:roles`]: { "agent-user": { org: "obachan" } },
  });
  const user = await handleDashboardRequest(new Request("https://dash.example/api/agent-control/overview", {
    headers: { authorization: `Bearer ${userToken}` },
  }));
  assert.equal(user.status, 403);
});
```

If the existing "unconfigured deployments" check returns a status other than 404 for an admin, use that same status in the admin assertion — the point is that Zitadel admin and HS256 admin behave identically.

- [ ] **Step 2: Run to verify failure**

Run: `yarn test:agent-control`
Expected: FAIL — `Cannot find module '../dist/server/zitadel-token.js'`.

- [ ] **Step 3: Implement `server/zitadel-token.ts`**

```ts
import { createPublicKey, verify as cryptoVerify, type JsonWebKey } from "node:crypto";

export interface ZitadelVerifyOptions {
  issuer: string;
  projectId: string;
}

export interface ZitadelAccessClaims {
  sub: string;
  roles: string[];
  preferredUsername: string | null;
  name: string | null;
}

const LEEWAY_SECONDS = 60;
const JWKS_TTL_MS = 10 * 60 * 1000;
const USERINFO_TTL_MS = 5 * 60 * 1000;

let jwksCache: { issuer: string; fetchedAt: number; keys: Array<JsonWebKey & { kid?: string }> } | null = null;
const userinfoCache = new Map<string, { at: number; value: Record<string, unknown> }>();

async function signingKeys(issuer: string, forceRefresh: boolean) {
  if (!forceRefresh && jwksCache && jwksCache.issuer === issuer && Date.now() - jwksCache.fetchedAt < JWKS_TTL_MS) {
    return jwksCache.keys;
  }
  const response = await fetch(`${issuer}/oauth/v2/keys`);
  if (!response.ok) return [];
  const body = (await response.json()) as { keys?: Array<JsonWebKey & { kid?: string }> };
  jwksCache = { issuer, fetchedAt: Date.now(), keys: body.keys ?? [] };
  return jwksCache.keys;
}

function decodePart<T>(part: string): T | null {
  try {
    return JSON.parse(Buffer.from(part, "base64url").toString("utf8")) as T;
  } catch {
    return null;
  }
}

function rolesFrom(payload: Record<string, unknown>, projectId: string): string[] {
  const roles = new Set<string>();
  for (const key of [`urn:zitadel:iam:org:project:${projectId}:roles`, "urn:zitadel:iam:org:project:roles"]) {
    const value = payload[key];
    if (value && typeof value === "object" && !Array.isArray(value)) {
      for (const role of Object.keys(value as Record<string, unknown>)) roles.add(role);
    }
  }
  return [...roles];
}

async function userinfo(issuer: string, token: string): Promise<Record<string, unknown>> {
  const cached = userinfoCache.get(token);
  if (cached && Date.now() - cached.at < USERINFO_TTL_MS) return cached.value;
  const response = await fetch(`${issuer}/oidc/v1/userinfo`, { headers: { authorization: `Bearer ${token}` } });
  const value = response.ok ? ((await response.json()) as Record<string, unknown>) : {};
  if (userinfoCache.size > 1000) userinfoCache.clear();
  userinfoCache.set(token, { at: Date.now(), value });
  return value;
}

/** Verifies a Zitadel RS256 JWT access token for the agent-platform project; null when invalid. */
export async function verifyZitadelAccessToken(
  token: string,
  options: ZitadelVerifyOptions,
): Promise<ZitadelAccessClaims | null> {
  const parts = token.split(".");
  if (parts.length !== 3) return null;
  const [headerB64, payloadB64, signatureB64] = parts;
  const header = decodePart<{ alg?: string; kid?: string }>(headerB64);
  const payload = decodePart<Record<string, unknown>>(payloadB64);
  if (!header || !payload || header.alg !== "RS256") return null;

  let keys = await signingKeys(options.issuer, false);
  if (!keys.some((key) => key.kid === header.kid)) keys = await signingKeys(options.issuer, true);
  const jwk = keys.find((key) => key.kid === header.kid);
  if (!jwk) return null;
  const valid = cryptoVerify(
    "RSA-SHA256",
    Buffer.from(`${headerB64}.${payloadB64}`),
    createPublicKey({ key: jwk, format: "jwk" }),
    Buffer.from(signatureB64, "base64url"),
  );
  if (!valid) return null;

  const now = Math.floor(Date.now() / 1000);
  const audience = Array.isArray(payload.aud) ? payload.aud : [payload.aud];
  if (payload.iss !== options.issuer) return null;
  if (!audience.includes(options.projectId)) return null;
  if (typeof payload.exp !== "number" || payload.exp + LEEWAY_SECONDS <= now) return null;
  if (typeof payload.nbf === "number" && payload.nbf - LEEWAY_SECONDS > now) return null;
  if (typeof payload.sub !== "string" || !payload.sub) return null;

  const info = await userinfo(options.issuer, token);
  return {
    sub: payload.sub,
    roles: rolesFrom(payload, options.projectId),
    preferredUsername: typeof info.preferred_username === "string" ? info.preferred_username : null,
    name: typeof info.name === "string" ? info.name : null,
  };
}
```

- [ ] **Step 4: Wire it into `server/app.ts`**

Config (inside `Config` interface and `getConfig()`):

```ts
  agentPlatform: { issuer: string; projectId: string } | null;
```
```ts
    agentPlatform: (() => {
      const projectId = optionalEnv("AGENT_PLATFORM_PROJECT_ID");
      const issuer = optionalEnv("AGENT_PLATFORM_ISSUER") || optionalEnv("OIDC_ISSUER");
      return projectId && issuer ? { issuer: issuer.replace(/\/$/, ""), projectId } : null;
    })(),
```

New helper next to `readSession`:

```ts
/** Dashboard session JWT, or a Zitadel agent-platform access token sent as a bearer. */
async function readAgentControlClaims(request: Request, config: Config): Promise<SessionClaims | null> {
  const session = readSession(request, config);
  if (session) return session;
  if (!config.agentPlatform) return null;
  const header = request.headers.get("authorization");
  if (!header || !header.toLowerCase().startsWith("bearer ")) return null;
  const verified = await verifyZitadelAccessToken(header.slice(7).trim(), config.agentPlatform);
  if (!verified) return null;
  const role = verified.roles.includes("agent-admin") ? "admin" : verified.roles.includes("agent-user") ? "user" : null;
  if (!role) return null;
  const localpart = verified.preferredUsername ?? `zitadel-${verified.sub}`;
  const serverName = canonicalMxidServerName(config) ?? new URL(config.agentPlatform.issuer).host;
  return {
    sub: `@${localpart}:${serverName}`,
    role,
    kind: "session",
    auth_method: "zitadel",
    display_name: verified.name ?? localpart,
    exp: Math.floor(Date.now() / 1000) + 60,
    roles: verified.roles,
  };
}
```

In the agent-control branch (~line 626) replace `const claims = readSession(request, config);` with `const claims = await readAgentControlClaims(request, config);`. Leave every other `readSession` call unchanged. Add `import { verifyZitadelAccessToken } from "./zitadel-token.js";` at the top (match the file's existing relative-import extension style).

- [ ] **Step 5: Run checks**

Run: `yarn check && yarn test:agent-control && node scripts/check-oidc-roles.mjs`
Expected: all checks pass, including the existing HS256 ones.

- [ ] **Step 6: Staging config** — in `deploy/obachan/runtime-secret-staging.yaml` under `spec.target.template.data`, add `AGENT_PLATFORM_PROJECT_ID: "<pid from Task 2>"` (non-secret literal). `OIDC_ISSUER` is already injected and is the default issuer.

- [ ] **Step 7: Commit, push, PR to dev**

```bash
git add server/zitadel-token.ts server/app.ts scripts/check-agent-control.mjs deploy/obachan/runtime-secret-staging.yaml
git commit -m "feat(agent-control): accept Zitadel agent-platform access tokens"
git push -u origin feat/zitadel-bearer-agent-control
```
Apply the runtime secret the way the 2026-10-03 rollout did (`kubectl apply -f deploy/obachan/runtime-secret-staging.yaml` against staging), then merge to `dev` (deploys staging).

---

### Task 6: WarpSpeed — Zitadel device flow and refreshing token store

**Files:**
- Create: `crates/managed_agent_sessions/src/zitadel_auth.rs`
- Create: `crates/managed_agent_sessions/src/zitadel_auth_tests.rs`
- Modify: `crates/managed_agent_sessions/src/lib.rs` (profile fields, public login API, `dashboard_token()`, storage format, remove dashboard device-login code)
- Modify: `crates/managed_agent_sessions/src/tests.rs` (`fixture_profile`, remove tests of deleted dashboard-login code)
- Modify: `crates/managed_agent_sessions/Cargo.toml` (`base64.workspace = true`)

**Interfaces:**
- Consumes: `EnvironmentProfile`, `protect_auth_bytes`/`unprotect_auth_bytes`, `stozn_auth_path()`, `HttpStatusError` in `lib.rs`.
- Produces (public API, names unchanged for `app/src/stozn_auth.rs`): `begin_stozn_login() -> Result<StoznLoginFlow>`, `complete_stozn_login(StoznLoginFlow) -> Result<StoznIdentity>`, `stored_stozn_identity() -> Option<StoznIdentity>`, `logout_stozn() -> Result<()>`, `StoznLoginFlow::{verification_url, user_code}`. Internal: `zitadel_auth::{ZitadelProfile, DeviceStart, start_device_login, poll_device_login, refresh, identity_from_access_token, TokenSet}`.

- [ ] **Step 1: Write the failing tests** in `zitadel_auth_tests.rs`

```rust
use std::io::{Read as _, Write as _};
use std::net::TcpListener;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde_json::json;

use super::*;

/// Serves `responses` in order, one per connection, repeating the last; records request bodies.
fn token_server(
    responses: Vec<(&'static str, serde_json::Value)>,
) -> (&'static str, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let log = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let served = log.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut stream = stream.unwrap();
            let mut request = [0_u8; 16_384];
            let read = stream.read(&mut request).unwrap();
            let text = String::from_utf8_lossy(&request[..read]).into_owned();
            let index = {
                let mut served = served.lock().unwrap();
                served.push(text);
                served.len() - 1
            };
            let (status, body) = &responses[index.min(responses.len() - 1)];
            let body = body.to_string();
            write!(
                stream,
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
        }
    });
    (Box::leak(format!("http://{address}").into_boxed_str()), log)
}

fn access_token(roles: &[&str], exp_in: i64) -> String {
    let role_map: serde_json::Map<String, serde_json::Value> = roles
        .iter()
        .map(|role| ((*role).to_owned(), json!({ "org": "obachan" })))
        .collect();
    let payload = json!({
        "sub": "2847",
        "exp": chrono::Utc::now().timestamp() + exp_in,
        "urn:zitadel:iam:org:project:proj-1:roles": role_map,
    });
    format!(
        "{}.{}.sig",
        URL_SAFE_NO_PAD.encode(br#"{"alg":"RS256"}"#),
        URL_SAFE_NO_PAD.encode(payload.to_string())
    )
}

fn profile(issuer: &'static str) -> ZitadelProfile {
    ZitadelProfile { issuer, client_id: "client-1", project_id: "proj-1" }
}

#[test]
fn device_start_requests_project_audience_and_validates_the_verification_url() {
    let (issuer, log) = token_server(vec![(
        "200 OK",
        json!({
            "device_code": "dev-1",
            "user_code": "ABCD-EFGH",
            "verification_uri": format!("{issuer}/device"),
            "verification_uri_complete": format!("{issuer}/device?user_code=ABCD-EFGH"),
            "expires_in": 300,
            "interval": 5
        }),
    )]);
    let start = start_device_login(&profile(issuer)).unwrap();
    assert_eq!(start.user_code, "ABCD-EFGH");
    assert_eq!(start.verification_url, format!("{issuer}/device?user_code=ABCD-EFGH"));
    let request = &log.lock().unwrap()[0];
    assert!(request.starts_with("POST /oauth/v2/device_authorization"));
    assert!(request.contains("client_id=client-1"));
    assert!(request.contains("urn%3Azitadel%3Aiam%3Aorg%3Aproject%3Aid%3Aproj-1%3Aaud"));
    assert!(request.contains("offline_access"));
}

#[test]
fn device_start_refuses_a_verification_url_on_another_origin() {
    let (issuer, _) = token_server(vec![(
        "200 OK",
        json!({
            "device_code": "dev-1",
            "user_code": "ABCD-EFGH",
            "verification_uri": "https://evil.example/device",
            "expires_in": 300,
            "interval": 5
        }),
    )]);
    assert!(start_device_login(&profile(issuer)).is_err());
}

#[test]
fn polling_waits_through_pending_and_slow_down_then_returns_tokens() {
    let token = access_token(&["agent-admin"], 3600);
    let (issuer, log) = token_server(vec![
        ("400 Bad Request", json!({ "error": "authorization_pending" })),
        ("400 Bad Request", json!({ "error": "slow_down" })),
        (
            "200 OK",
            json!({ "access_token": token, "refresh_token": "ref-1", "expires_in": 3600, "token_type": "Bearer" }),
        ),
    ]);
    let tokens = poll_device_login(&profile(issuer), "dev-1", Duration::from_millis(1), Duration::from_secs(10)).unwrap();
    assert_eq!(tokens.refresh_token, "ref-1");
    assert_eq!(log.lock().unwrap().len(), 3);
}

#[test]
fn polling_stops_on_access_denied() {
    let (issuer, _) = token_server(vec![("400 Bad Request", json!({ "error": "access_denied" }))]);
    let error = poll_device_login(&profile(issuer), "dev-1", Duration::from_millis(1), Duration::from_secs(10))
        .unwrap_err();
    assert!(format!("{error:#}").contains("denied"));
}

#[test]
fn roles_map_to_admin_user_or_no_access() {
    let admin = identity_from_access_token(&access_token(&["agent-user", "agent-admin"], 60), "proj-1", "id-dev.obachan.dev").unwrap();
    assert_eq!(admin.role, "admin");
    assert_eq!(admin.subject, "2847");
    let user = identity_from_access_token(&access_token(&["agent-user"], 60), "proj-1", "id-dev.obachan.dev").unwrap();
    assert_eq!(user.role, "user");
    let error = identity_from_access_token(&access_token(&[], 60), "proj-1", "id-dev.obachan.dev").unwrap_err();
    assert_eq!(
        format!("{error}"),
        "Your account has no access to the agent platform (id-dev.obachan.dev)"
    );
}

#[test]
fn refresh_rotates_the_refresh_token() {
    let token = access_token(&["agent-user"], 3600);
    let (issuer, log) = token_server(vec![(
        "200 OK",
        json!({ "access_token": token, "refresh_token": "ref-2", "expires_in": 3600 }),
    )]);
    let tokens = refresh(&profile(issuer), "ref-1").unwrap();
    assert_eq!(tokens.refresh_token, "ref-2");
    let request = &log.lock().unwrap()[0];
    assert!(request.contains("grant_type=refresh_token"));
    assert!(request.contains("refresh_token=ref-1"));
}

#[test]
fn refresh_without_a_new_refresh_token_keeps_the_old_one() {
    let token = access_token(&["agent-user"], 3600);
    let (issuer, _) = token_server(vec![("200 OK", json!({ "access_token": token, "expires_in": 3600 }))]);
    assert_eq!(refresh(&profile(issuer), "ref-1").unwrap().refresh_token, "ref-1");
}

#[test]
fn needs_refresh_inside_the_sixty_second_window() {
    let now = chrono::Utc::now();
    assert!(needs_refresh(now + chrono::Duration::seconds(59), now));
    assert!(!needs_refresh(now + chrono::Duration::seconds(61), now));
}
```

Add at the end of `zitadel_auth.rs`:

```rust
#[cfg(test)]
#[path = "zitadel_auth_tests.rs"]
mod tests;
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo nextest run -p managed_agent_sessions zitadel_auth`
Expected: compile errors (module/items missing).

- [ ] **Step 3: Implement `zitadel_auth.rs`**

```rust
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, Utc};
use instant::Instant;
use reqwest::blocking::Client;
use serde::Deserialize;
use serde_json::Value;

use crate::StoznIdentity;

const REFRESH_WINDOW_SECONDS: i64 = 60;
const MAX_POLL_INTERVAL: Duration = Duration::from_secs(30);

/// Compiled-in OIDC settings for one environment's Zitadel instance.
pub(crate) struct ZitadelProfile {
    pub issuer: &'static str,
    pub client_id: &'static str,
    pub project_id: &'static str,
}

impl ZitadelProfile {
    fn scope(&self) -> String {
        format!(
            "openid profile email offline_access urn:zitadel:iam:org:project:id:{}:aud urn:zitadel:iam:org:projects:roles",
            self.project_id
        )
    }

    pub(crate) fn issuer_host(&self) -> &str {
        self.issuer
            .split_once("://")
            .map_or(self.issuer, |(_, rest)| rest.trim_end_matches('/'))
    }
}

pub(crate) struct DeviceStart {
    pub device_code: String,
    pub user_code: String,
    pub verification_url: String,
    pub expires_at: DateTime<Utc>,
    pub interval: Duration,
}

pub(crate) struct TokenSet {
    pub access_token: String,
    pub access_expires_at: DateTime<Utc>,
    pub refresh_token: String,
}

#[derive(Deserialize)]
struct DeviceAuthorizationResponse {
    device_code: String,
    user_code: String,
    verification_uri: String,
    verification_uri_complete: Option<String>,
    expires_in: i64,
    interval: Option<u64>,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: i64,
}

#[derive(Deserialize)]
struct TokenError {
    error: String,
}

pub(crate) fn start_device_login(profile: &ZitadelProfile) -> Result<DeviceStart> {
    let response = Client::new()
        .post(format!("{}/oauth/v2/device_authorization", profile.issuer))
        .form(&[("client_id", profile.client_id), ("scope", profile.scope().as_str())])
        .send()
        .context("could not reach the STOZN sign-in service")?;
    if !response.status().is_success() {
        bail!("STOZN sign-in service refused the login request ({})", response.status());
    }
    let started: DeviceAuthorizationResponse =
        response.json().context("invalid STOZN sign-in response")?;
    let verification_url = started
        .verification_uri_complete
        .unwrap_or(started.verification_uri);
    validate_issuer_url(profile, &verification_url)?;
    if started.expires_in <= 0 || started.device_code.is_empty() || !valid_user_code(&started.user_code) {
        bail!("STOZN sign-in service returned an invalid login request");
    }
    Ok(DeviceStart {
        device_code: started.device_code,
        user_code: started.user_code,
        verification_url,
        expires_at: Utc::now() + chrono::Duration::seconds(started.expires_in),
        interval: Duration::from_secs(started.interval.unwrap_or(5).clamp(1, 30)),
    })
}

/// Polls the token endpoint per RFC 8628 until approval, denial, expiry or `max_wait`.
pub(crate) fn poll_device_login(
    profile: &ZitadelProfile,
    device_code: &str,
    interval: Duration,
    max_wait: Duration,
) -> Result<TokenSet> {
    let started = Instant::now();
    let mut interval = interval;
    loop {
        if started.elapsed() >= max_wait {
            bail!("STOZN login request expired");
        }
        let response = Client::new()
            .post(format!("{}/oauth/v2/token", profile.issuer))
            .form(&[
                ("client_id", profile.client_id),
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ("device_code", device_code),
            ])
            .send()
            .context("STOZN login polling failed")?;
        if response.status().is_success() {
            return token_set(response.json().context("invalid STOZN token response")?, None);
        }
        let error: TokenError = response.json().context("invalid STOZN token error")?;
        match error.error.as_str() {
            "authorization_pending" => {}
            "slow_down" => interval = (interval + Duration::from_secs(5)).min(MAX_POLL_INTERVAL),
            "access_denied" => bail!("STOZN login was denied in the browser"),
            "expired_token" => bail!("STOZN login request expired"),
            other => bail!("STOZN login failed: {other}"),
        }
        thread::sleep(interval);
    }
}

pub(crate) fn refresh(profile: &ZitadelProfile, refresh_token: &str) -> Result<TokenSet> {
    let response = Client::new()
        .post(format!("{}/oauth/v2/token", profile.issuer))
        .form(&[
            ("client_id", profile.client_id),
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
        ])
        .send()
        .context("could not reach the STOZN sign-in service")?;
    if !response.status().is_success() {
        let error = response
            .json::<TokenError>()
            .map(|error| error.error)
            .unwrap_or_else(|_| "unknown".to_owned());
        bail!(RefreshRejected(error));
    }
    token_set(response.json().context("invalid STOZN token response")?, Some(refresh_token))
}

/// The token endpoint rejected a refresh; the stored sign-in is no longer usable.
#[derive(Debug)]
pub(crate) struct RefreshRejected(pub String);

impl std::fmt::Display for RefreshRejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "STOZN sign-in expired ({}); sign in again", self.0)
    }
}

impl std::error::Error for RefreshRejected {}

fn token_set(response: TokenResponse, previous_refresh: Option<&str>) -> Result<TokenSet> {
    let refresh_token = response
        .refresh_token
        .or_else(|| previous_refresh.map(str::to_owned))
        .context("STOZN sign-in did not return a refresh token")?;
    if response.access_token.is_empty() || response.expires_in <= 0 {
        bail!("STOZN sign-in returned an invalid access token");
    }
    Ok(TokenSet {
        access_token: response.access_token,
        access_expires_at: Utc::now() + chrono::Duration::seconds(response.expires_in),
        refresh_token,
    })
}

pub(crate) fn needs_refresh(access_expires_at: DateTime<Utc>, now: DateTime<Utc>) -> bool {
    access_expires_at - now < chrono::Duration::seconds(REFRESH_WINDOW_SECONDS)
}

/// Reads identity and agent-platform role from the access token's payload. The token was just
/// received from the issuer over TLS; resource servers verify its signature.
pub(crate) fn identity_from_access_token(
    access_token: &str,
    project_id: &str,
    issuer_host: &str,
) -> Result<StoznIdentity> {
    let payload = access_token
        .split('.')
        .nth(1)
        .context("STOZN sign-in returned a malformed access token")?;
    let claims: Value = serde_json::from_slice(
        &URL_SAFE_NO_PAD
            .decode(payload)
            .context("STOZN sign-in returned a malformed access token")?,
    )
    .context("STOZN sign-in returned a malformed access token")?;
    let roles: Vec<&str> = [
        format!("urn:zitadel:iam:org:project:{project_id}:roles"),
        "urn:zitadel:iam:org:project:roles".to_owned(),
    ]
    .iter()
    .filter_map(|key| claims.get(key).and_then(Value::as_object))
    .flat_map(|roles| roles.keys().map(String::as_str))
    .collect();
    let role = if roles.contains(&"agent-admin") {
        "admin"
    } else if roles.contains(&"agent-user") {
        "user"
    } else {
        bail!("Your account has no access to the agent platform ({issuer_host})");
    };
    let subject = claims
        .get("sub")
        .and_then(Value::as_str)
        .filter(|sub| !sub.is_empty())
        .context("STOZN sign-in returned an access token without a subject")?;
    let display_name = ["name", "preferred_username", "email"]
        .iter()
        .find_map(|key| claims.get(*key).and_then(Value::as_str))
        .map(str::to_owned);
    Ok(StoznIdentity {
        subject: subject.to_owned(),
        role: role.to_owned(),
        display_name,
    })
}

fn validate_issuer_url(profile: &ZitadelProfile, url: &str) -> Result<()> {
    let allowed = reqwest::Url::parse(profile.issuer)?;
    let requested = reqwest::Url::parse(url).context("STOZN sign-in returned an invalid URL")?;
    let same_origin = requested.scheme() == allowed.scheme()
        && requested.host_str() == allowed.host_str()
        && requested.port_or_known_default() == allowed.port_or_known_default();
    if !same_origin
        || !requested.username().is_empty()
        || requested.password().is_some()
        || requested.fragment().is_some()
    {
        bail!("STOZN sign-in returned an unsafe login URL");
    }
    Ok(())
}

fn valid_user_code(code: &str) -> bool {
    (4..=16).contains(&code.len())
        && code.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

#[cfg(test)]
#[path = "zitadel_auth_tests.rs"]
mod tests;
```

- [ ] **Step 4: Run the module tests**

Run: `cargo nextest run -p managed_agent_sessions zitadel_auth`
Expected: PASS (add `base64.workspace = true` to `[dependencies]` in `crates/managed_agent_sessions/Cargo.toml` and `mod zitadel_auth;` near the top of `lib.rs` if the compiler asks).

- [ ] **Step 5: Switch `lib.rs` to the Zitadel store**

1. Add `mod zitadel_auth;` after the `use` block; `use zitadel_auth::{ZitadelProfile, RefreshRejected};`.
2. Constants: replace `const STOZN_AUTH_FORMAT: u32 = 1;` with `const STOZN_AUTH_FORMAT: u32 = 2;` and add

```rust
const STAGING_ZITADEL: ZitadelProfile = ZitadelProfile {
    issuer: "https://id-dev.obachan.dev",
    client_id: STAGING_ZITADEL_CLIENT_ID,
    project_id: STAGING_ZITADEL_PROJECT_ID,
};
```
   and the compiled ids recorded by Task 2 (public identifiers, not secrets):

```rust
const STAGING_ZITADEL_CLIENT_ID: &str = "<client id recorded in Task 2 Step 4>";
const STAGING_ZITADEL_PROJECT_ID: &str = "<project id recorded in Task 2 Step 4>";
```
   The implementer copies the two literal values from this plan's "Provisioned ids (staging)" section.
3. `EnvironmentProfile` gains `zitadel: &'static ZitadelProfile`; `profile()`'s `STAGING` sets `zitadel: &STAGING_ZITADEL`; `fixture_profile` in `tests.rs` sets `zitadel: &STAGING_ZITADEL`.
4. Replace `StoznLoginFlow`, `StoredStoznAuth`, `StoznLoginStart`, `StoznLoginPoll`, `begin_stozn_login`, `complete_stozn_login` and `load_stozn_auth` with:

```rust
#[derive(Debug, Clone)]
pub struct StoznLoginFlow {
    device_code: String,
    verification_url: String,
    user_code: String,
    expires_at: DateTime<Utc>,
    poll_interval: Duration,
}

impl StoznLoginFlow {
    pub fn verification_url(&self) -> &str {
        &self.verification_url
    }

    pub fn user_code(&self) -> &str {
        &self.user_code
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct StoredStoznAuth {
    format_version: u32,
    environment_alias: String,
    access_token: String,
    access_expires_at: String,
    refresh_token: String,
    user: StoznIdentity,
}

pub fn begin_stozn_login() -> Result<StoznLoginFlow> {
    let profile = profile(ManagedEnvironment::Staging);
    let started = zitadel_auth::start_device_login(profile.zitadel)?;
    Ok(StoznLoginFlow {
        device_code: started.device_code,
        verification_url: started.verification_url,
        user_code: started.user_code,
        expires_at: started.expires_at,
        poll_interval: started.interval,
    })
}

pub fn complete_stozn_login(flow: StoznLoginFlow) -> Result<StoznIdentity> {
    let profile = profile(ManagedEnvironment::Staging);
    let remaining = (flow.expires_at - Utc::now())
        .to_std()
        .unwrap_or_default()
        .min(STOZN_LOGIN_MAX_WAIT);
    let tokens = zitadel_auth::poll_device_login(
        profile.zitadel,
        &flow.device_code,
        flow.poll_interval,
        remaining,
    )?;
    let identity = zitadel_auth::identity_from_access_token(
        &tokens.access_token,
        profile.zitadel.project_id,
        profile.zitadel.issuer_host(),
    )?;
    save_stozn_auth(&StoredStoznAuth {
        format_version: STOZN_AUTH_FORMAT,
        environment_alias: profile.alias.to_owned(),
        access_token: tokens.access_token,
        access_expires_at: tokens.access_expires_at.to_rfc3339(),
        refresh_token: tokens.refresh_token,
        user: identity.clone(),
    })?;
    Ok(identity)
}

fn load_stozn_auth() -> Result<StoredStoznAuth> {
    let path = stozn_auth_path();
    let protected =
        fs::read(&path).with_context(|| format!("failed to read {}", path.display()))?;
    let auth: StoredStoznAuth = serde_json::from_slice(&unprotect_auth_bytes(protected)?)
        .context("invalid WarpSpeed STOZN authentication state")?;
    if auth.format_version != STOZN_AUTH_FORMAT
        || auth.environment_alias != profile(ManagedEnvironment::Staging).alias
        || auth.refresh_token.is_empty()
        || !valid_dashboard_token(&auth.access_token)
    {
        bail!("WarpSpeed STOZN authentication is invalid; sign in again");
    }
    Ok(auth)
}

/// Returns a current access token, refreshing and re-saving the stored sign-in when needed.
fn stored_access_token() -> Result<String> {
    let auth = load_stozn_auth()?;
    let expires_at = DateTime::parse_from_rfc3339(&auth.access_expires_at)
        .context("invalid WarpSpeed STOZN authentication expiry")?
        .with_timezone(&Utc);
    if !zitadel_auth::needs_refresh(expires_at, Utc::now()) {
        return Ok(auth.access_token);
    }
    let zitadel = profile(ManagedEnvironment::Staging).zitadel;
    match zitadel_auth::refresh(zitadel, &auth.refresh_token) {
        Ok(tokens) => {
            let user = zitadel_auth::identity_from_access_token(
                &tokens.access_token,
                zitadel.project_id,
                zitadel.issuer_host(),
            )?;
            save_stozn_auth(&StoredStoznAuth {
                access_token: tokens.access_token.clone(),
                access_expires_at: tokens.access_expires_at.to_rfc3339(),
                refresh_token: tokens.refresh_token,
                user,
                ..auth
            })?;
            Ok(tokens.access_token)
        }
        Err(error) if error.downcast_ref::<RefreshRejected>().is_some() => {
            // Another WarpSpeed process may have rotated the refresh token first.
            let latest = load_stozn_auth()?;
            if latest.refresh_token != auth.refresh_token {
                return stored_access_token();
            }
            logout_stozn()?;
            Err(error)
        }
        Err(error) => Err(error),
    }
}
```

5. In `dashboard_token()` replace `if let Ok(auth) = load_stozn_auth() { return Ok(auth.token); }` with:

```rust
    if stozn_auth_path().exists() {
        return stored_access_token();
    }
```
6. Delete now-unused items: `validate_verification_url`, `valid_user_code` (lib.rs version), `STAGING_WEB_URL` if no remaining user (keep it if deep links/web URLs still use it — `cargo build` will tell), and the `StoznLoginStart`/`StoznLoginPoll` types.
7. Long loops: in `attach` (~line 1059) and `wait_until_resumed` (~line 1213) the token is fetched once before a loop. Move `let token = dashboard_token()?;` inside the loop body so each iteration uses a fresh token, e.g.

```rust
    loop {
        let token = dashboard_token()?;
        // existing loop body unchanged, still passing `&token`
    }
```
   If the loop is inside a `*_with(profile, token, ...)` helper, change that helper's parameter from `token: &str` to `token: &dyn Fn() -> Result<String>` and call `token()?` at the top of each iteration; tests pass `&|| Ok("token".to_owned())`.
8. In `tests.rs`: delete `stozn_login_url_is_bound_to_the_compiled_dashboard_and_request`, `stozn_confirmation_codes_are_strictly_formatted` and `stozn_login_state_requires_admin_and_visible_ascii_credentials` (they test the removed dashboard login); keep `stozn_desktop_credential_protection_round_trips_for_this_os_user`.

- [ ] **Step 6: Build and test the crate**

Run: `cargo nextest run -p managed_agent_sessions && cargo clippy -p managed_agent_sessions --all-targets -- -D warnings`
Expected: PASS, no warnings.

- [ ] **Step 7: Commit and push**

```bash
git add crates/managed_agent_sessions/Cargo.toml crates/managed_agent_sessions/src/lib.rs crates/managed_agent_sessions/src/tests.rs crates/managed_agent_sessions/src/zitadel_auth.rs crates/managed_agent_sessions/src/zitadel_auth_tests.rs Cargo.lock
git commit -m "feat(agent-ctrl): sign in with Zitadel device code and refresh tokens"
git push -u gitea feat/p0-zitadel-login && git push -u gitlab feat/p0-zitadel-login
```

---

### Task 7: WarpSpeed — UI text and app build

**Files:**
- Modify: `app/src/auth/login_slide.rs:767,780-793`, `app/src/auth/auth_view_body.rs:760,769-781` (status text)

**Interfaces:**
- Consumes: Task 6 public API (unchanged names).

- [ ] **Step 1: Update status strings** in both files so they no longer mention the dashboard approval:

```rust
StoznAuthState::Starting => "Starting STOZN sign-in…".to_owned(),
StoznAuthState::WaitingForBrowser { user_code } => {
    format!("Confirm code {user_code} in your browser to finish signing in…")
}
```
Keep the `SignedIn`, `Failed` and `SignedOut` arms unchanged. Change "Authorize WarpSpeed by STOZN in your browser" only if it refers to the dashboard; it still reads correctly for Zitadel, so leave it.

- [ ] **Step 2: Build the app and run the affected tests**

Run: `cargo check -p warp && cargo nextest run -p managed_agent_sessions && cargo clippy -p warp -p managed_agent_sessions --all-targets -- -D warnings && ./script/format`
Expected: PASS. (The 22 pre-existing `StoznAuthManager` singleton failures in `app` view tests are unrelated; do not count them as regressions, and do not add new ones.)

- [ ] **Step 3: Commit and push**

```bash
git add app/src/auth/login_slide.rs app/src/auth/auth_view_body.rs
git commit -m "feat(auth): describe the Zitadel device sign-in in the login status"
git push gitea feat/p0-zitadel-login && git push gitlab feat/p0-zitadel-login
```

---

### Task 8: End-to-end on staging and rollout

- [ ] **Step 1: Merge** the dashboard PR (Task 5) to `dev` after its runtime secret is applied; confirm `GET https://dash-backend-dev.stozn.ai/api/agent-control/overview` with a Task 2 Zitadel access token returns 200.
- [ ] **Step 2: Install WarpSpeed** from `feat/p0-zitadel-login` via `powershell -ExecutionPolicy Bypass -File ./script/windows/install-warpspeed.ps1` (never restart running WarpSpeed windows; open a new window on the new build).
- [ ] **Step 3: Sign in** from the new window: "Sign in with STOZN" opens the Zitadel device page; approve; status shows "Signed in to STOZN as …".
- [ ] **Step 4: Use AgentCtrl** — `warpspeed agent-ctrl list` and `agent-ctrl repos` from a shell, and the fleet pane, all succeed with only the Zitadel sign-in (`STOZN_TOKEN` unset, `~/.stozn.json` absent or renamed).
- [ ] **Step 5: Refresh** — force expiry by editing nothing: wait for the access token lifetime, or temporarily set the Zitadel app's access token lifetime to 5 minutes in the console, then run `agent-ctrl list` again; it succeeds and `stozn-auth.bin` changes on disk.
- [ ] **Step 6: Sign out** from settings; `agent-ctrl list` fails with the "not signed in" message.
- [ ] **Step 7: Merge** `feat/p0-zitadel-login` into `dev` via PR on git-dev.obachan.dev (stozn/warpspeed), and the obachan and passiv PRs into their base branches. Update the `self-hosted-agent-platform` memory with P0 status and the provisioned ids.

## Provisioned ids (staging)

Provisioned 2026-10-09 with `obachan auth provision-agent-platform-sso --env staging` (rerun was idempotent).

- Issuer: `https://id-dev.obachan.dev`
- Project `agent-platform`: `394361582064240430`
- App `warpspeed`: app id `394361583389640494`, client id `394361583406417710`
- Endpoints (from discovery): `/oauth/v2/device_authorization`, `/oauth/v2/token`, `/oauth/v2/keys`, `/oidc/v1/userinfo`; verification page `/device?user_code=…`
- OpenBao: `obachan/staging/warpspeed/runtime` (`oidc_project_id`, `oidc_app_id`, `oidc_client_id`)
- `agent-admin` granted to staging user `budelius` (id `381309740384256019`)

Task 6 uses `STAGING_ZITADEL_CLIENT_ID = "394361583406417710"` and `STAGING_ZITADEL_PROJECT_ID = "394361582064240430"`; Task 5 Step 6 uses `AGENT_PLATFORM_PROJECT_ID: "394361582064240430"`.
