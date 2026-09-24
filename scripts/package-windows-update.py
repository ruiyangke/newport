#!/usr/bin/env python3
"""Sign an NSIS installer for the updater and emit a platform manifest fragment."""
import argparse
import datetime
import hashlib
import json
import os
from pathlib import Path
import shutil
import struct
import subprocess


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("installer", type=Path)
    parser.add_argument("--arch", choices=["x86_64", "aarch64"], required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    root = Path(__file__).resolve().parent.parent
    config = json.loads((root / "src-tauri/tauri.conf.json").read_text())
    version = config["version"]
    # NSIS's installer stub may be x86 even when the installed app is ARM64.
    # Inspect the actual application instead of guessing from the stub.
    binary = root / "src-tauri/target/release/porthop.exe"
    data = binary.read_bytes()
    offset = struct.unpack_from("<I", data, 0x3C)[0]
    expected = {"x86_64": 0x8664, "aarch64": 0xAA64}[args.arch]
    if data[offset:offset + 4] != b"PE\0\0" or struct.unpack_from("<H", data, offset + 4)[0] != expected:
        parser.error("Application architecture does not match the requested update target")
    if not os.environ.get("TAURI_SIGNING_PRIVATE_KEY"):
        parser.error("TAURI_SIGNING_PRIVATE_KEY is required")
    args.output.mkdir(parents=True, exist_ok=True)
    installer = args.output / f"Porthop-{version}-windows-{args.arch}-setup.exe"
    shutil.copy2(args.installer, installer)
    key = os.environ["TAURI_SIGNING_PRIVATE_KEY"]
    if "\n" not in key and len(key) < 1024 and Path(key).is_file():
        os.environ["TAURI_SIGNING_PRIVATE_KEY_PATH"] = key
        del os.environ["TAURI_SIGNING_PRIVATE_KEY"]
    os.environ.setdefault("TAURI_SIGNING_PRIVATE_KEY_PASSWORD", "")
    subprocess.run(["node", str(root / "node_modules/@tauri-apps/cli/tauri.js"),
                    "signer", "sign", "--app-version", version, str(installer)], check=True)
    manifest = {
        "version": version,
        "notes": f"Porthop {version}",
        "pub_date": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        "platforms": {f"windows-{args.arch}": {
            "signature": Path(str(installer) + ".sig").read_text().strip(),
            "url": f"https://github.com/ruiyangke/porthop/releases/download/v{version}/{installer.name}",
        }},
    }
    (args.output / f"windows-{args.arch}.json").write_text(json.dumps(manifest, indent=2) + "\n")
    (args.output / f"SHA256SUMS-windows-{args.arch}").write_text(
        f"{hashlib.sha256(installer.read_bytes()).hexdigest()}  {installer.name}\n")


if __name__ == "__main__":
    main()
