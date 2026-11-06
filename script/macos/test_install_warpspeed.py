#!/usr/bin/env python3
import os
import pathlib
import plistlib
import shutil
import subprocess
import sys
import tempfile
import unittest

INSTALLER = pathlib.Path(__file__).with_name("install-warpspeed")


@unittest.skipUnless(sys.platform == "darwin", "requires macOS signing tools")
class InstallWarpSpeedTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = pathlib.Path(self.temporary.name)
        self.source = self.root / "source.app"
        contents = self.source / "Contents"
        binary_dir = contents / "MacOS"
        binary_dir.mkdir(parents=True)
        shutil.copy("/bin/sleep", binary_dir / "warp-oss")
        with (contents / "Info.plist").open("wb") as plist:
            plistlib.dump({
                "CFBundleIdentifier": "dev.warp.WarpOss",
                "CFBundleExecutable": "warp-oss",
                "CFBundlePackageType": "APPL",
            }, plist)
        subprocess.run([
            "codesign", "--force", "--sign", "-", str(self.source)
        ], check=True, capture_output=True)
        self.destination = self.root / "Applications" / "warpspeed-latest.app"
        self.home = self.root / "home"
        self.home.mkdir()

    def install(self):
        environment = dict(os.environ)
        environment["HOME"] = str(self.home)
        return subprocess.run([
            "bash", str(INSTALLER), "--from-bundle", str(self.source),
            "--destination", str(self.destination),
        ], capture_output=True, text=True, env=environment)

    def test_installs_signed_bundle_without_dmg(self):
        result = self.install()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue((self.destination / "Contents/MacOS/warp-oss").is_file())
        subprocess.run([
            "codesign", "--verify", "--deep", "--strict", str(self.destination)
        ], check=True, capture_output=True)
        self.assertEqual(list(self.destination.parent.glob(".warpspeed-install.*")), [])

    def test_preserves_previous_installation_in_backup(self):
        self.destination.mkdir(parents=True)
        (self.destination / "marker").write_text("previous installation")
        result = self.install()
        self.assertEqual(result.returncode, 0, result.stderr)
        backups = list(self.destination.parent.glob(".warpspeed-backup.*"))
        self.assertEqual(len(backups), 1)
        self.assertEqual((backups[0] / "previous.app/marker").read_text(), "previous installation")
        self.assertFalse((self.destination / "marker").exists())

    def test_invalid_signature_leaves_existing_installation_untouched(self):
        self.destination.mkdir(parents=True)
        (self.destination / "marker").write_text("previous installation")
        with (self.source / "Contents/MacOS/warp-oss").open("ab") as binary:
            binary.write(b"tampered")
        result = self.install()
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual((self.destination / "marker").read_text(), "previous installation")
        self.assertEqual(list(self.destination.parent.glob(".warpspeed-backup.*")), [])

    def test_reinstall_preserves_profile_and_session_storage(self):
        profile = self.home / ".warp-oss"
        state = self.home / "Library/Application Support/dev.warp.WarpOss"
        profile.mkdir(parents=True)
        state.mkdir(parents=True)
        (profile / "agent-session").write_text("resume")
        (state / "session-checkpoint").write_text("window")

        self.assertEqual(self.install().returncode, 0)
        self.assertEqual(self.install().returncode, 0)

        self.assertEqual((profile / "agent-session").read_text(), "resume")
        self.assertEqual((state / "session-checkpoint").read_text(), "window")

    def test_refuses_running_destination(self):
        result = self.install()
        self.assertEqual(result.returncode, 0, result.stderr)
        process = subprocess.Popen([
            str(self.destination / "Contents/MacOS/warp-oss"), "60"
        ])
        try:
            result = self.install()
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("running sessions were left untouched", result.stderr)
            self.assertIsNone(process.poll())
            self.assertEqual(list(self.destination.parent.glob(".warpspeed-backup.*")), [])
        finally:
            process.terminate()
            process.wait(timeout=10)

    def test_refuses_symlink_destination(self):
        self.destination.parent.mkdir(parents=True)
        self.destination.symlink_to(self.source, target_is_directory=True)
        result = self.install()
        self.assertNotEqual(result.returncode, 0)
        self.assertTrue(self.destination.is_symlink())


if __name__ == "__main__":
    unittest.main()
