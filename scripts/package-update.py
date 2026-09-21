#!/usr/bin/env python3
"""Package a signed, stapled macOS app for GitHub and Porthop's updater."""
import argparse
import datetime
import hashlib
import json
import os
from pathlib import Path
import plistlib
import subprocess
import tarfile


def run(*args):
    result = subprocess.run(args)
    if result.returncode:
        raise SystemExit(f"Release validation or packaging failed: {args[0]} (exit {result.returncode})")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("app", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    app = args.app.resolve()
    info = plistlib.loads((app / "Contents/Info.plist").read_bytes())
    version = info["CFBundleShortVersionString"]
    root = Path(__file__).resolve().parent.parent
    config = json.loads((root / "src-tauri/tauri.conf.json").read_text())
    if version != config["version"] or info["CFBundleIdentifier"] != config["identifier"]:
        parser.error("App version/identifier does not match the release source")
    run("codesign", "--verify", "--deep", "--strict", str(app))
    run("xcrun", "stapler", "validate", str(app))
    run("spctl", "--assess", "--type", "execute", str(app))
    arch = subprocess.check_output(["lipo", "-archs", str(app / "Contents/MacOS" / info["CFBundleExecutable"])], text=True).strip()
    if arch != "arm64":
        parser.error("This release pipeline currently packages Apple Silicon only")
    if not os.environ.get("TAURI_SIGNING_PRIVATE_KEY"):
        parser.error("Set TAURI_SIGNING_PRIVATE_KEY to the update key path or contents")
    args.output.mkdir(parents=True, exist_ok=True)
    name = f"Porthop-{version}-macos-arm64"
    archive = args.output / (name + ".app.tar.gz")
    # Preserve the final signed/stapled bundle. Signing before stapling the
    # archive would make the updater signature invalid after repackaging.
    with tarfile.open(archive, "w:gz") as tar:
        tar.add(app, arcname="Porthop.app")
    os.environ.setdefault("TAURI_SIGNING_PRIVATE_KEY_PASSWORD", "")
    run(str(root / "node_modules/.bin/tauri"), "signer", "sign", "--app-version", version, str(archive))
    zip_path = args.output / (name + ".zip")
    run("ditto", "-c", "-k", "--sequesterRsrc", "--keepParent", str(app), str(zip_path))
    manifest = {
        "version": version,
        "notes": f"Porthop {version}",
        "pub_date": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        "platforms": {"darwin-aarch64": {
            "signature": Path(str(archive) + ".sig").read_text().strip(),
            "url": f"https://github.com/ruiyangke/porthop/releases/download/v{version}/{archive.name}",
        }},
    }
    (args.output / "latest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    (args.output / "SHA256SUMS").write_text("".join(
        f"{hashlib.sha256(path.read_bytes()).hexdigest()}  {path.name}\n"
        for path in (archive, zip_path)
    ))
    print(f"Release artifacts: {args.output.resolve()}")


if __name__ == "__main__":
    main()
