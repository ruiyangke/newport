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
        with tempfile.TemporaryDirectory(prefix="porthop-release-test-") as directory:
            root = Path(directory)
            (root / "scripts/ci").mkdir(parents=True)
            (root / "src-tauri").mkdir()
            shutil.copy(SCRIPT, root / "scripts/ci/verify-release.py")
            for name in ["package.json", "package-lock.json", "src-tauri/tauri.conf.json"]:
                (root / name).write_text(json.dumps({"version": "1.2.3"}))
            cargo = root / "src-tauri/Cargo.toml"
            cargo.write_text('[package]\nversion = "1.2.3"\n')
            def git(*args):
                return subprocess.check_output(["git", *args], cwd=root, stderr=subprocess.DEVNULL, text=True).strip()
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


if __name__ == "__main__":
    unittest.main()
