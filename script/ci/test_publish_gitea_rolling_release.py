import hashlib
import importlib.util
import os
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

SCRIPT_PATH = Path(__file__).with_name("publish_gitea_rolling_release.py")
SPEC = importlib.util.spec_from_file_location("publish_gitea_rolling_release", SCRIPT_PATH)
assert SPEC is not None and SPEC.loader is not None
PUBLISHER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(PUBLISHER)


class PublishGiteaRollingReleaseTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp_dir = tempfile.TemporaryDirectory()
        self.assets_dir = Path(self.temp_dir.name)
        self.commit = "0123456789abcdef0123456789abcdef01234567"
        for name in PUBLISHER.EXPECTED_ASSETS:
            path = self.assets_dir / name
            if name.endswith(".source-commit.txt"):
                path.write_text(f"{self.commit}\n", encoding="utf-8")
            elif not name.endswith(".sha256"):
                path.write_bytes(f"contents of {name}".encode())
        for name in ("WarpOss-macos-arm64.zip", "WarpOss-windows-x64-setup.exe"):
            artifact = self.assets_dir / name
            digest = hashlib.sha256(artifact.read_bytes()).hexdigest()
            checksum_name = f"ci-artifacts/{name}" if name.endswith(".zip") else name
            (self.assets_dir / f"{name}.sha256").write_text(
                f"{digest}  {checksum_name}\n", encoding="utf-8"
            )
        self.environment = {
            "GITEA_TOKEN": "secret",
            "GITHUB_SERVER_URL": "https://gitea.example.test",
            "GITHUB_REPOSITORY": "owner/repository",
            "GITHUB_REF_NAME": "dev",
            "GITHUB_SHA": self.commit,
            "GITHUB_RUN_ID": "42",
            "GITHUB_RUN_ATTEMPT": "3",
        }

    def tearDown(self) -> None:
        self.temp_dir.cleanup()

    def test_replaces_assets_only_after_every_upload_succeeds(self) -> None:
        next_id = iter(range(100, 106))
        uploaded = []

        def upload(*args, **kwargs):
            asset = {"id": next(next_id), "name": args[-1]}
            uploaded.append(asset)
            return asset

        api_calls = []

        def request(base_url, token, path, method="GET", payload=None, allow_not_found=False):
            api_calls.append((path, method, payload, allow_not_found))
            if "/releases/tags/" in path:
                return {"id": 7}
            if path.endswith("/assets") and method == "GET":
                return [
                    {"id": 1, "name": PUBLISHER.EXPECTED_ASSETS[0]},
                    {"id": 2, "name": f"{PUBLISHER.EXPECTED_ASSETS[1]}.upload-old"},
                    {"id": 3, "name": "manually-attached.txt"},
                    *uploaded,
                ]
            return {}

        with (
            mock.patch.dict(os.environ, self.environment, clear=True),
            mock.patch.object(sys, "argv", [str(SCRIPT_PATH), str(self.assets_dir)]),
            mock.patch.object(PUBLISHER, "api_request", side_effect=request),
            mock.patch.object(PUBLISHER, "upload_asset", side_effect=upload) as upload_mock,
            mock.patch.object(PUBLISHER, "force_tag") as force_tag_mock,
        ):
            PUBLISHER.main()

        self.assertEqual(upload_mock.call_count, len(PUBLISHER.EXPECTED_ASSETS))
        force_tag_mock.assert_called_once_with(
            "https://gitea.example.test",
            "owner/repository",
            "secret",
            "continuous-dev",
            self.commit,
        )
        deleted_ids = {
            int(path.rsplit("/", 1)[-1])
            for path, method, _, _ in api_calls
            if method == "DELETE"
        }
        self.assertEqual(deleted_ids, {1, 2})
        renames = [
            payload["name"]
            for path, method, payload, _ in api_calls
            if method == "PATCH" and "/assets/" in path
        ]
        self.assertEqual(renames, list(PUBLISHER.EXPECTED_ASSETS))
        macos_checksum = self.assets_dir / "WarpOss-macos-arm64.zip.sha256"
        self.assertTrue(macos_checksum.read_text().endswith("  WarpOss-macos-arm64.zip\n"))

    def test_rejects_artifact_from_another_commit(self) -> None:
        marker = self.assets_dir / "WarpOss-windows-x64.source-commit.txt"
        marker.write_text("different-commit\n", encoding="utf-8")
        with (
            mock.patch.dict(os.environ, self.environment, clear=True),
            mock.patch.object(sys, "argv", [str(SCRIPT_PATH), str(self.assets_dir)]),
            self.assertRaisesRegex(RuntimeError, "was not built from"),
        ):
            PUBLISHER.main()


if __name__ == "__main__":
    unittest.main()
