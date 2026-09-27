#!/usr/bin/env python3
"""Run the Rust desktop backend against an isolated, real Linux OpenSSH server."""
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import tomllib

ROOT = Path(__file__).resolve().parent.parent

# Explicit context keeps tests off unrelated Docker engines.
os.environ["DOCKER_CONTEXT"] = os.environ.get("PORTHOP_TEST_DOCKER_CONTEXT", "orbstack")


def run(*args, **kwargs):
    return subprocess.run(args, check=True, **kwargs)


def interrupted(_signum, _frame):
    raise KeyboardInterrupt


def main():
    signal.signal(signal.SIGTERM, interrupted)
    docker = os.environ.get("PORTHOP_TEST_DOCKER") or shutil.which("docker")
    if not docker:
        for candidate in [Path.home() / ".orbstack/bin/docker",
                          Path("/Applications/OrbStack.app/Contents/MacOS/xbin/docker"),
                          Path("/Applications/Docker.app/Contents/Resources/bin/docker")]:
            if candidate.exists():
                docker = str(candidate)
                break
    if not docker:
        raise SystemExit("Docker is required. Set PORTHOP_TEST_DOCKER if it is not on PATH.")
    # Testcontainers uses the Docker API, not the CLI context setting.
    endpoint = json.loads(subprocess.check_output(
        [docker, "context", "inspect", os.environ["DOCKER_CONTEXT"]]))[0]["Endpoints"]["docker"]["Host"]
    if not endpoint.startswith("unix://"):
        raise SystemExit("Remote tests require a local Docker socket (OrbStack is supported).")
    run(docker, "info", "--format", "{{.ServerVersion}}")
    run("node", str(ROOT / "scripts/build-agent.mjs"), cwd=ROOT)
    toolchain = tomllib.loads((ROOT / "rust-toolchain.toml").read_text())["toolchain"]["channel"]
    run(docker, "build", "--build-arg", f"RUST_VERSION={toolchain}",
        "-t", "porthop-test-remote:local", str(ROOT / "tests/remote"))
    env = {**os.environ, "DOCKER_HOST": endpoint}
    run("cargo", "run", "--locked", "--manifest-path", "src-tauri/Cargo.toml",
        "--example", "remote_tests", cwd=ROOT, env=env)


if __name__ == "__main__":
    main()
