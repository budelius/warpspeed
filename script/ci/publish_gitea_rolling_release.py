#!/usr/bin/env python3

import base64
import hashlib
import json
import os
import subprocess
import sys
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path
from typing import Any


def expected_assets(channel: str) -> tuple[str, ...]:
    return (
        f"WarpSpeed-{channel}-macos-arm64.zip",
        f"WarpSpeed-{channel}-macos-arm64.zip.sha256",
        f"WarpSpeed-{channel}-macos-arm64.source-commit.txt",
        f"WarpSpeed-{channel}-windows-x64-setup.exe",
        f"WarpSpeed-{channel}-windows-x64-setup.exe.sha256",
        f"WarpSpeed-{channel}-windows-x64.source-commit.txt",
    )


def required_env(name: str) -> str:
    value = os.environ.get(name, "").strip()
    if not value:
        raise RuntimeError(f"{name} is required")
    return value


def api_request(
    base_url: str,
    token: str,
    path: str,
    method: str = "GET",
    payload: dict[str, Any] | None = None,
    allow_not_found: bool = False,
) -> Any:
    data = None if payload is None else json.dumps(payload).encode()
    request = urllib.request.Request(
        f"{base_url}{path}",
        data=data,
        method=method,
        headers={
            "Accept": "application/json",
            "Authorization": f"token {token}",
            "Content-Type": "application/json",
        },
    )
    try:
        with urllib.request.urlopen(request) as response:
            body = response.read()
    except urllib.error.HTTPError as error:
        if allow_not_found and error.code == 404:
            return None
        body = error.read().decode(errors="replace")
        raise RuntimeError(f"Gitea API {method} {path} returned {error.code}: {body}") from error
    return json.loads(body) if body else {}


def upload_asset(
    base_url: str,
    token: str,
    repository: str,
    release_id: int,
    source: Path,
    name: str,
) -> dict[str, Any]:
    endpoint = (
        f"{base_url}/repos/{repository}/releases/{release_id}/assets"
        f"?name={urllib.parse.quote(name, safe='')}"
    )
    result = subprocess.run(
        [
            "curl",
            "--config",
            "-",
            "--fail-with-body",
            "--silent",
            "--show-error",
            "--retry",
            "3",
            "--retry-all-errors",
            "--retry-delay",
            "5",
            "--request",
            "POST",
            "--header",
            "Accept: application/json",
            "--form",
            f"attachment=@{source}",
            endpoint,
        ],
        input=f'header = "Authorization: token {token}"\n',
        check=True,
        capture_output=True,
        text=True,
    )
    return json.loads(result.stdout)


def force_tag(server_url: str, repository: str, token: str, tag: str, commit: str) -> None:
    credentials = base64.b64encode(f"x-access-token:{token}".encode()).decode()
    env = os.environ.copy()
    env.update(
        {
            "GIT_CONFIG_COUNT": "1",
            "GIT_CONFIG_KEY_0": "http.extraheader",
            "GIT_CONFIG_VALUE_0": f"Authorization: Basic {credentials}",
        }
    )
    subprocess.run(["git", "tag", "--force", tag, commit], check=True, env=env)
    subprocess.run(
        [
            "git",
            "push",
            "--force",
            f"{server_url}/{repository}.git",
            f"refs/tags/{tag}:refs/tags/{tag}",
        ],
        check=True,
        env=env,
    )


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as file:
        for chunk in iter(lambda: file.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def main() -> None:
    token = required_env("GITEA_TOKEN")
    server_url = required_env("GITHUB_SERVER_URL").rstrip("/")
    repository = required_env("GITHUB_REPOSITORY")
    branch = required_env("GITHUB_REF_NAME")
    commit = required_env("GITHUB_SHA")
    run_id = required_env("GITHUB_RUN_ID")
    run_attempt = os.environ.get("GITHUB_RUN_ATTEMPT", "1")
    if branch not in {"main", "dev"}:
        raise RuntimeError(f"rolling releases are restricted to main and dev, got {branch!r}")

    channel = "stable" if branch == "main" else "latest"
    asset_names = expected_assets(channel)
    assets_dir = Path(sys.argv[1] if len(sys.argv) > 1 else "release-assets").resolve()
    assets = [assets_dir / name for name in asset_names]
    missing = [str(path) for path in assets if not path.is_file()]
    if missing:
        raise RuntimeError(f"missing release assets: {', '.join(missing)}")

    for platform in ("macos-arm64", "windows-x64"):
        marker = assets_dir / f"WarpSpeed-{channel}-{platform}.source-commit.txt"
        if marker.read_text(encoding="utf-8").strip() != commit:
            raise RuntimeError(f"{marker.name} was not built from {commit}")

    for artifact in (
        assets_dir / f"WarpSpeed-{channel}-macos-arm64.zip",
        assets_dir / f"WarpSpeed-{channel}-windows-x64-setup.exe",
    ):
        checksum_file = artifact.with_name(f"{artifact.name}.sha256")
        expected = checksum_file.read_text(encoding="utf-8").split()[0].lower()
        actual = sha256(artifact)
        if actual != expected:
            raise RuntimeError(f"checksum mismatch for {artifact.name}: {actual} != {expected}")
        checksum_file.write_text(f"{actual}  {artifact.name}\n", encoding="utf-8")

    api_url = f"{server_url}/api/v1"
    tag = f"continuous-{branch}"
    release_path = f"/repos/{repository}/releases/tags/{urllib.parse.quote(tag, safe='')}"
    release = api_request(api_url, token, release_path, allow_not_found=True)
    if release is None:
        force_tag(server_url, repository, token, tag, commit)
        release = api_request(
            api_url,
            token,
            f"/repos/{repository}/releases",
            method="POST",
            payload={
                "tag_name": tag,
                "target_commitish": commit,
                "name": f"WarpSpeed {channel} desktop builds",
                "body": "Publishing desktop artifacts…",
                "draft": False,
                "prerelease": branch == "dev",
            },
        )
    if not isinstance(release, dict) or "id" not in release:
        raise RuntimeError("Gitea did not return a release ID")
    release_id = int(release["id"])

    uploaded = []
    suffix = f"upload-{run_id}-{run_attempt}"
    for source in assets:
        uploaded.append(
            (
                source.name,
                upload_asset(
                    api_url,
                    token,
                    repository,
                    release_id,
                    source,
                    f"{source.name}.{suffix}",
                ),
            )
        )

    current_assets = api_request(
        api_url,
        token,
        f"/repos/{repository}/releases/{release_id}/assets",
    )
    if not isinstance(current_assets, list):
        raise RuntimeError("Gitea did not return the release asset list")
    force_tag(server_url, repository, token, tag, commit)

    uploaded_ids = {int(asset["id"]) for _, asset in uploaded}
    for asset in current_assets:
        asset_id = int(asset["id"])
        if asset_id in uploaded_ids:
            continue
        name = str(asset["name"])
        is_previous_upload = any(
            name.startswith(f"{base}.upload-") for base in asset_names
        )
        if name in asset_names or is_previous_upload:
            api_request(
                api_url,
                token,
                f"/repos/{repository}/releases/{release_id}/assets/{asset_id}",
                method="DELETE",
            )

    for final_name, asset in uploaded:
        api_request(
            api_url,
            token,
            f"/repos/{repository}/releases/{release_id}/assets/{int(asset['id'])}",
            method="PATCH",
            payload={"name": final_name},
        )

    short_commit = commit[:8]
    commit_url = f"{server_url}/{repository}/commit/{commit}"
    body = f"""Automated, rolling desktop builds from `{branch}` at \
[`{short_commit}`]({commit_url}).

- **macOS:** Apple Silicon (`arm64`), self-signed application bundle.
- **Windows:** x64, unsigned installer. Windows may show a SmartScreen warning.

Verify downloads with the adjacent `.sha256` files. Assets are replaced only after both
platform builds for the same commit succeed.
"""
    api_request(
        api_url,
        token,
        f"/repos/{repository}/releases/{release_id}",
        method="PATCH",
        payload={
            "tag_name": tag,
            "target_commitish": commit,
            "name": f"WarpSpeed {channel} desktop builds ({short_commit})",
            "body": body,
            "draft": False,
            "prerelease": branch == "dev",
        },
    )
    print(f"Published {len(uploaded)} assets to {server_url}/{repository}/releases/tag/{tag}")


if __name__ == "__main__":
    try:
        main()
    except Exception as error:
        print(f"error: {error}", file=sys.stderr)
        raise SystemExit(1)
