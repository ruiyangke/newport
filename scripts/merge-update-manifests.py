#!/usr/bin/env python3
"""Combine platform fragments only when all required release artifacts are present."""
import argparse
import base64
import subprocess
import tempfile
import json
from pathlib import Path
from urllib.parse import unquote, urlparse


def merge(paths, version, assets):
    result = {"version": version, "notes": f"Porthop {version}", "platforms": {}}
    for path in paths:
        fragment = json.loads(path.read_text())
        if fragment["version"] != version:
            raise ValueError("Cannot merge different release versions")
        result.setdefault("pub_date", fragment["pub_date"])
        for target, entry in fragment["platforms"].items():
            if target in result["platforms"]:
                raise ValueError(f"Duplicate update target: {target}")
            url = urlparse(entry["url"])
            expected_prefix = f"/ruiyangke/porthop/releases/download/v{version}/"
            if url.scheme != "https" or url.netloc != "github.com" or not url.path.startswith(expected_prefix):
                raise ValueError("Update URL does not match this release")
            filename = unquote(url.path.rsplit("/", 1)[-1])
            if Path(filename).name != filename or not (assets / filename).is_file():
                raise ValueError(f"Missing update artifact: {filename}")
            signature = assets / (filename + ".sig")
            if not entry["signature"] or signature.read_text().strip() != entry["signature"]:
                raise ValueError(f"Missing or inconsistent signature: {filename}")
            result["platforms"][target] = entry
    required = {"darwin-aarch64", "windows-x86_64", "windows-aarch64"}
    if not required.issubset(result["platforms"]):
        raise ValueError(f"Missing release targets: {required - result['platforms'].keys()}")
    return result


def verify_packages(manifest, assets, public_key):
    # Verify using the app's pinned key, not merely the presence of a .sig file.
    with tempfile.TemporaryDirectory() as directory:
        key = Path(directory) / "public.key"
        signature = Path(directory) / "signature"
        key.write_bytes(base64.b64decode(public_key, validate=True))
        for entry in manifest["platforms"].values():
            signature_text = base64.b64decode(entry["signature"], validate=True).decode("utf-8")
            signature.write_text(signature_text)
            filename = unquote(urlparse(entry["url"]).path.rsplit("/", 1)[-1])
            subprocess.run(["minisign", "-Vm", str(assets / filename), "-p", str(key), "-x", str(signature)], check=True)
            # The trusted comment is safe to inspect only after signature verification.
            comment = next(line.removeprefix("trusted comment: ") for line in signature_text.splitlines() if line.startswith("trusted comment: "))
            signed = next((field.removeprefix("version:") for field in comment.split("\t") if field.startswith("version:")), None)
            if signed != manifest["version"]:
                raise ValueError("Artifact signature is not bound to this release version")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("fragments", type=Path, nargs="+")
    parser.add_argument("--version", required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    result = merge(args.fragments, args.version, args.output.parent)
    config = json.loads((Path(__file__).resolve().parent.parent / "src-tauri/tauri.conf.json").read_text())
    verify_packages(result, args.output.parent, config["plugins"]["updater"]["pubkey"])
    args.output.write_text(json.dumps(result, indent=2) + "\n")


if __name__ == "__main__":
    main()
