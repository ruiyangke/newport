"""Validate a release tag against all package versions before using signing secrets."""
import json
import os
from pathlib import Path
import re
import subprocess
import tomllib

root = Path(__file__).resolve().parents[2]
tag = os.environ["RELEASE_TAG"]
if not re.fullmatch(r"v\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?", tag):
    raise SystemExit("Expected a version tag such as v0.3.0")
versions = [json.loads((root / path).read_text())["version"]
            for path in ["package.json", "package-lock.json", "src-tauri/tauri.conf.json"]]
versions.append(tomllib.loads((root / "src-tauri/Cargo.toml").read_text())["package"]["version"])
if any("v" + version != tag for version in versions):
    raise SystemExit("Release tag and package versions must match")
commit = subprocess.check_output(["git", "rev-parse", f"refs/tags/{tag}^{{commit}}"], cwd=root, text=True).strip()
head = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=root, text=True).strip()
if commit != head:
    raise SystemExit("Checkout must match the release tag")
if os.environ.get("GITHUB_OUTPUT"):
    with open(os.environ["GITHUB_OUTPUT"], "a") as output:
        output.write(f"commit={commit}\n")
