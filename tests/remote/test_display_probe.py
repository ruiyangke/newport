#!/usr/bin/env python3
"""Exercise the production probe inside a disposable Linux container.

Mount the checkout read-only at /workspace and run this file with python3.
Provide an executable /tmp (Docker: --tmpfs /tmp:rw,exec,nosuid,nodev), Python,
and coreutils. No packages are installed or existing displays accessed.
"""

import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import time


PROBE = (
    Path(__file__).resolve().parents[2]
    / "crates/newport-backend/src/remote_apps/display_probe.sh"
).read_text()


def executable(path, body):
    path.write_text("#!/bin/sh\n" + body + "\n")
    path.chmod(0o755)


def run_probe(name, env, expected):
    started = time.monotonic()
    result = subprocess.run(
        ["/bin/sh", "-c", PROBE], env=env, capture_output=True, text=True, timeout=5
    )
    assert result.returncode == 0, (name, result.returncode, result.stderr)
    assert result.stdout == f"xwayland={expected}\n", (name, result.stdout, result.stderr)
    print(f"PASS {name} ({time.monotonic() - started:.2f}s)", flush=True)


def assert_stopped(pid):
    # Zombies are terminated and will be reaped by the container's init.
    path = Path(f"/proc/{pid}/stat")
    for _ in range(20):
        try:
            state = path.read_text().rsplit(") ", 1)[1].split()[0]
        except FileNotFoundError:
            return
        if state == "Z":
            return
        time.sleep(0.05)
    raise AssertionError(f"Probe left descendant {pid} running")


def main():
    assert Path("/.dockerenv").exists(), "Run only in a disposable Docker container"
    with tempfile.TemporaryDirectory(prefix="newport-display-probe-") as temporary:
        root = Path(temporary)
        for name in ["sh", "head", "tail", "sed", "timeout"]:
            found = shutil.which(name)
            assert found is not None, f"Fixture requires {name}"
            (root / name).symlink_to(found)
        env = dict(
            os.environ,
            PATH=str(root),
            DISPLAY=":99",
            WAYLAND_DISPLAY="existing-desktop",
            XDG_SESSION_TYPE="x11",
            PROBE_CHILD_PID=str(root / "child.pid"),
        )
        # Neither a desktop session nor Xvfb satisfies the shared prerequisites.
        executable(root / "Xvfb", "exit 0")
        cases = [
            ("missing with Xvfb", None, "missing"),
            (
                "current on stderr",
                "printf 'The X.Org Foundation Xwayland Version 24.1.6 (test)\\n' >&2",
                "24.1.6",
            ),
            ("old", "printf 'Xwayland Version 22.1.9\\n'", "22.1.9"),
            (
                "nonzero with a valid version",
                "printf 'Xwayland Version 24.1.6\\n'; exit 1",
                "unverified",
            ),
            ("unknown version", "printf 'unknown\\n'", "unverified"),
            (
                "oversized output",
                "printf 'Xwayland Version 24.1.6\\n'; "
                "/usr/bin/head -c 1000000 /dev/zero | /usr/bin/tr '\\0' x",
                "unverified",
            ),
            (
                "hung process",
                'echo $$ > "$PROBE_CHILD_PID"; exec /bin/sleep 20',
                "unverified",
            ),
            (
                "forked child",
                '/bin/sleep 20 & echo $! > "$PROBE_CHILD_PID"; wait',
                "unverified",
            ),
            (
                "child ignoring termination",
                'trap "" TERM; /bin/sleep 20 & echo $! > "$PROBE_CHILD_PID"; wait',
                "unverified",
            ),
        ]
        binary = root / "Xwayland"
        pid_file = root / "child.pid"
        for name, body, expected in cases:
            binary.unlink(missing_ok=True)
            pid_file.unlink(missing_ok=True)
            if body is not None:
                executable(binary, body)
            run_probe(name, env, expected)
            if pid_file.exists():
                assert_stopped(int(pid_file.read_text()))

        (root / "timeout").unlink()
        executable(binary, 'echo started > "$PROBE_CHILD_PID"')
        pid_file.unlink(missing_ok=True)
        run_probe("missing timeout utility", env, "unverified")
        assert not pid_file.exists(), "An unbounded version process must not start"
        (root / "Xvfb").unlink()
        binary.unlink()
        env.pop("DISPLAY")
        env.pop("WAYLAND_DISPLAY")
        env.pop("XDG_SESSION_TYPE")
        run_probe("headless without Xvfb", env, "missing")


if __name__ == "__main__":
    main()
