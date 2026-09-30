#!/usr/bin/env python3
"""Validate frontend Git decoders against the disposable real-SSH workflow."""
import os
from pathlib import Path
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parent.parent


def main():
    with tempfile.TemporaryDirectory(prefix="newport-git-contract-") as directory:
        env = {
            **os.environ,
            "NEWPORT_TEST_FILTER": "remote_tests::git::",
            "NEWPORT_GIT_TRACE_PATH": str(Path(directory) / "responses.ndjson"),
        }
        subprocess.run(
            [sys.executable, str(ROOT / "scripts/test-remote.py")],
            cwd=ROOT, env=env, check=True,
        )
        subprocess.run(
            ["npm", "exec", "--", "vitest", "run", "src/api/gitLiveContract.test.ts"],
            cwd=ROOT, env=env, check=True,
        )


if __name__ == "__main__":
    main()
