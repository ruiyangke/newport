#!/usr/bin/env python3
import importlib.util
import json
from pathlib import Path
import tempfile
import shutil
import subprocess
import unittest

spec = importlib.util.spec_from_file_location("merge_updates", Path(__file__).with_name("merge-update-manifests.py"))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class Manifests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.fragments = []
        for target in ["darwin-aarch64", "windows-x86_64", "windows-aarch64"]:
            name = target + ".package"
            (self.root / name).write_bytes(b"package")
            (self.root / (name + ".sig")).write_text("test signature")
            path = self.root / (target + ".json")
            path.write_text(json.dumps({"version": "1.2.3", "pub_date": "2026-01-01T00:00:00Z", "platforms": {
                target: {"signature": "test signature", "url": f"https://github.com/ruiyangke/porthop/releases/download/v1.2.3/{name}"}
            }}))
            self.fragments.append(path)

    def test_preserves_every_platform(self):
        result = module.merge(self.fragments, "1.2.3", self.root)
        self.assertEqual(set(result["platforms"]), {"darwin-aarch64", "windows-x86_64", "windows-aarch64"})

    def test_rejects_missing_platform(self):
        with self.assertRaises(ValueError):
            module.merge(self.fragments[:2], "1.2.3", self.root)

    def test_rejects_mixed_versions(self):
        with self.assertRaises(ValueError):
            module.merge(self.fragments, "1.2.4", self.root)

    def test_rejects_duplicate_platform(self):
        with self.assertRaises(ValueError):
            module.merge(self.fragments + self.fragments[:1], "1.2.3", self.root)

    def test_rejects_missing_payload(self):
        (self.root / "windows-aarch64.package").unlink()
        with self.assertRaises(ValueError):
            module.merge(self.fragments, "1.2.3", self.root)

    def test_rejects_mismatched_signature(self):
        (self.root / "windows-x86_64.package.sig").write_text("wrong signature")
        with self.assertRaises(ValueError):
            module.merge(self.fragments, "1.2.3", self.root)


@unittest.skipUnless(shutil.which("minisign"), "minisign is required for signature verification")
class Signatures(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        fixture = Path(__file__).resolve().parent.parent / "src-tauri/tests/fixtures/updater"
        shutil.copy2(fixture / "payload.txt", self.root / "payload.txt")
        self.key = (fixture / "public.key").read_text().strip()
        self.manifest = {"version": "2.0.0", "platforms": {"windows-aarch64": {
            "url": "https://github.com/ruiyangke/porthop/releases/download/v2.0.0/payload.txt",
            "signature": (fixture / "payload.txt.sig").read_text().strip(),
        }}}

    def test_accepts_signature_bound_to_version(self):
        module.verify_packages(self.manifest, self.root, self.key)

    def test_rejects_modified_package(self):
        (self.root / "payload.txt").write_text("tampered")
        with self.assertRaises(subprocess.CalledProcessError):
            module.verify_packages(self.manifest, self.root, self.key)

    def test_rejects_changed_announced_version(self):
        self.manifest["version"] = "3.0.0"
        with self.assertRaises(ValueError):
            module.verify_packages(self.manifest, self.root, self.key)

    def test_rejects_other_signing_key(self):
        config = json.loads((Path(__file__).resolve().parent.parent / "src-tauri/tauri.conf.json").read_text())
        with self.assertRaises(subprocess.CalledProcessError):
            module.verify_packages(self.manifest, self.root, config["plugins"]["updater"]["pubkey"])


if __name__ == "__main__":
    unittest.main()
