"""Exercise release version and immutable-tag validation in an isolated Git repo."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

SCRIPT = Path(__file__).with_name("verify-release.py")


class ReleaseValidation(unittest.TestCase):
    def test_version_and_commit_must_match_tag(self):
        with tempfile.TemporaryDirectory(prefix="newport-release-test-") as directory:
            root = Path(directory)
            (root / "scripts/ci").mkdir(parents=True)
            (root / "src-tauri").mkdir()
            shutil.copy(SCRIPT, root / "scripts/ci/verify-release.py")
            for name in ["package.json", "package-lock.json", "src-tauri/tauri.conf.json"]:
                (root / name).write_text(json.dumps({"version": "1.2.3"}))
            cargo = root / "src-tauri/Cargo.toml"
            cargo.write_text('[package]\nversion = "1.2.3"\n')
            def git(*args):
                return subprocess.check_output(["git", "-c", "commit.gpgsign=false", "-c", "tag.gpgsign=false", *args], cwd=root, stderr=subprocess.DEVNULL, text=True).strip()
            git("init")
            git("add", ".")
            git("-c", "user.name=Test", "-c", "user.email=test@example.invalid", "commit", "-m", "Fixture")
            git("tag", "v1.2.3")
            output = root / "output"
            env = {**os.environ, "RELEASE_TAG": "v1.2.3", "GITHUB_OUTPUT": str(output)}
            def check():
                return subprocess.run(["python3", "scripts/ci/verify-release.py"], cwd=root,
                                      env=env, capture_output=True, text=True)
            self.assertEqual(check().returncode, 0)
            self.assertEqual(output.read_text(), f"commit={git('rev-parse', 'HEAD')}\n")
            cargo.write_text('[package]\nversion = "1.2.4"\n')
            self.assertIn("versions must match", check().stderr)
            git("checkout", "--", "src-tauri/Cargo.toml")
            git("-c", "user.name=Test", "-c", "user.email=test@example.invalid", "commit", "--allow-empty", "-m", "Different commit")
            self.assertIn("must match the release tag", check().stderr)
            env["RELEASE_TAG"] = "main"
            self.assertIn("Expected a version tag", check().stderr)


class SigningMigration(unittest.TestCase):
    def test_new_identity_retains_only_authorized_keychain_groups(self):
        import importlib.util
        spec = importlib.util.spec_from_file_location("signing", Path(__file__).parents[1] / "check-signing.py")
        signing = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(signing)
        ent = {"com.apple.application-identifier": "TEAM.app.newport", "keychain-access-groups": ["TEAM.app.newport", "TEAM.com.porthop.desktop"]}
        profile = {"Entitlements": {"com.apple.application-identifier": "TEAM.app.newport", "keychain-access-groups": ["TEAM.*"]}}
        signing.validate(ent, profile, "app.newport")
        for bad in [
            {**ent, "com.apple.application-identifier": "TEAM.ke.ry.porthop"},
            {**ent, "keychain-access-groups": ["TEAM.app.newport"]},
            {**ent, "keychain-access-groups": ["TEAM.app.newport", "TEAM.ke.ry.porthop"]},
            {**ent, "keychain-access-groups": ["TEAM.app.newport", "OTHER.com.porthop.desktop"]},
        ]:
            with self.assertRaises(ValueError):
                signing.validate(bad, profile, "app.newport")


if __name__ == "__main__":
    unittest.main()
