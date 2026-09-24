#!/usr/bin/env python3
"""Run the Rust desktop backend against an isolated, real Linux OpenSSH server."""
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import tempfile
import time
import uuid

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
    run(docker, "info", "--format", "{{.ServerVersion}}")
    run("bash", str(ROOT / "scripts/build-agent.sh"), cwd=ROOT)
    name = "porthop-e2e-" + uuid.uuid4().hex[:12]
    image = "porthop-test-remote:local"
    run(docker, "build", "-t", image, str(ROOT / "tests/remote"))
    with tempfile.TemporaryDirectory(prefix="porthop-remote-") as directory:
        folder = Path(directory)
        run("ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-f", str(folder / "client"))
        agent = None
        try:
            run(docker, "run", "--detach", "--name", name,
                "--publish", "127.0.0.1::22",
                "--mount", f"type=bind,src={directory},dst=/fixture-key,readonly",
                image, stdout=subprocess.DEVNULL)
            published = json.loads(subprocess.check_output(
                [docker, "inspect", "--format", '{{json .NetworkSettings.Ports}}', name]))
            port = published["22/tcp"][0]["HostPort"]
            for _ in range(60):
                result = subprocess.run([docker, "exec", name, "sh", "-c",
                    "test -s /home/fixture/.ssh/authorized_keys && "
                    "ss -ltn | grep -q ':22 ' && ss -ltn | grep -q ':8080 '"],
                    stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
                if result.returncode == 0:
                    break
                time.sleep(0.5)
            else:
                raise RuntimeError("Remote fixture did not become ready within 30 seconds")
            # Pin the host key via the container control plane, never disable host verification.
            host_key = subprocess.check_output([docker, "exec", name, "cat",
                "/etc/ssh/ssh_host_ed25519_key.pub"], text=True).strip()
            (folder / "known_hosts").write_text(f"[127.0.0.1]:{port} {host_key}\n")
            socket = str(folder / "agent.sock")
            agent = subprocess.Popen(["ssh-agent", "-D", "-a", socket],
                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            for _ in range(50):
                if Path(socket).exists():
                    break
                time.sleep(0.1)
            env = {**os.environ, "SSH_AUTH_SOCK": socket,
                   "PORTHOP_TEST_SSH_PORT": port,
                   "PORTHOP_TEST_IDENTITY": str(folder / "client"),
                   "PORTHOP_TEST_KNOWN_HOSTS": str(folder / "known_hosts")}
            run("ssh-add", str(folder / "client"), env=env)
            run("cargo", "test", "--manifest-path", "src-tauri/Cargo.toml", "remote_tests::",
                "--", "--ignored", "--nocapture", "--test-threads=1", cwd=ROOT, env=env,
                timeout=300)
        except BaseException:
            subprocess.run([docker, "logs", "--tail", "100", name], check=False)
            raise
        finally:
            if agent is not None:
                agent.terminate()
                try:
                    agent.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    agent.kill()
                    agent.wait()
            subprocess.run([docker, "rm", "--force", name], check=False,
                           stdout=subprocess.DEVNULL)


if __name__ == "__main__":
    main()
