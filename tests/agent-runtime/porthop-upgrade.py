#!/usr/bin/env python3
"""Run as an unprivileged user in a disposable Linux container.

The artifact directory must contain Porthop-0.2.3-macos-arm64.app.tar.gz,
porthop-install.sh from v0.2.3:src-tauri/src/agent-install.sh, and a Linux
newport-agent built from the current checkout. See docs/testing.md.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import select
import signal
import struct
import subprocess
import tarfile
import tempfile
import time
import uuid


ARCHIVE = "Porthop-0.2.3-macos-arm64.app.tar.gz"
ARCHIVE_SHA256 = "06ec9f371ef0713b549c54b3eec260331f80770a0cdf6cbdae334378d0806751"
INSTALLER_SHA256 = "3b09dce730321441a46a24aaee5d9ea1c8e8c1914d09aeee4054c5e95370fb3d"
RELEASE = "https://github.com/ruiyangke/newport/releases/tag/v0.2.3"
OLD_NAMES = ("xclip", "wl-paste", "xdg-open", "porthop-browser")
NEW_NAMES = ("xclip", "xsel", "wl-paste", "xdg-open", "newport-browser")
CUSTOM_NAMES = ("custom-copy", "custom-absolute", "custom-open", "custom-chain", "dangling-helper")


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def release_agent(archive):
    """Read the published app's include_bytes! ELF payload, without running it.

    ELF segments and section tables determine its exact file extent. The old
    desktop embedded duplicate copies; require all copies for this CPU to agree.
    """
    assert sha256(archive.read_bytes()) == ARCHIVE_SHA256, "release archive checksum mismatch"
    with tarfile.open(archive) as bundle:
        desktop = bundle.extractfile("Porthop.app/Contents/MacOS/porthop").read()
    machine = {"aarch64": 183, "x86_64": 62}[platform.machine()]
    candidates = {}
    start = 0
    copies = 0
    while (offset := desktop.find(b"\x7fELF\x02\x01\x01", start)) >= 0:
        start = offset + 1
        try:
            (_, kind, cpu, version, _, phoff, shoff, _, ehsize,
             phsize, phnum, shsize, shnum, _) = struct.unpack_from("<16sHHIQQQIHHHHHH", desktop, offset)
            if not (cpu == machine and kind in (2, 3) and version == 1 and ehsize == 64
                    and phsize == 56 and shsize == 64 and 0 < phnum < 64 and 0 < shnum < 256):
                continue
            end = max(64, phoff + phsize * phnum, shoff + shsize * shnum)
            for index in range(phnum):
                segment = struct.unpack_from("<IIQQQQQQ", desktop, offset + phoff + index * phsize)
                end = max(end, segment[2] + segment[5])
            for index in range(shnum):
                section = struct.unpack_from("<IIQQQQIIQQ", desktop, offset + shoff + index * shsize)
                if section[1] != 8:  # SHT_NOBITS occupies memory, not file bytes.
                    end = max(end, section[4] + section[5])
            if end > 128 * 1024 * 1024 or offset + end > len(desktop):
                continue
            payload = desktop[offset:offset + end]
            if b"porthop-agent/5" in payload:
                candidates[sha256(payload)] = payload
                copies += 1
        except struct.error:
            continue
    assert len(candidates) == 1, f"expected one distinct released agent, found {len(candidates)}"
    digest, payload = next(iter(candidates.items()))
    print(json.dumps({"release": RELEASE, "archiveSha256": ARCHIVE_SHA256,
                      "architecture": platform.machine(), "embeddedCopies": copies,
                      "oldAgentSha256": digest, "oldAgentBytes": len(payload)}), flush=True)
    return payload


def run(args, env, data=None, check=True):
    result = subprocess.run(args, env=env, input=data, capture_output=True, timeout=30)
    if check:
        assert result.returncode == 0, (args, result.returncode, result.stderr.decode(errors="replace"))
    return result


def read_exact(pipe, length):
    data = bytearray()
    deadline = time.monotonic() + 15
    while len(data) < length:
        remaining = deadline - time.monotonic()
        assert remaining > 0 and select.select([pipe], [], [], remaining)[0], "agent reply timed out"
        chunk = os.read(pipe.fileno(), length - len(data))
        assert chunk, "agent closed its protocol stream"
        data.extend(chunk)
    return bytes(data)


def read_control(pipe):
    length, = struct.unpack(">I", read_exact(pipe, 4))
    assert 0 < length <= 16384, f"invalid control frame length {length}"
    return json.loads(read_exact(pipe, length))


def verify_runtime(agent, digest, env):
    """Use the same initialize/ping/shutdown exchange as verify_candidate."""
    with subprocess.Popen([str(agent), "runtime", "--stdio"], env=env,
                          stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                          stderr=subprocess.PIPE, bufsize=0) as process:
        try:
            hello = read_control(process.stdout)
            assert hello["protocol"] == "newport.runtime" and 1 in hello["versions"]
            assert hello["buildSha256"] == digest
            for service in ("monitor", "git", "integration"):
                assert 1 in hello["capabilities"][service]["versions"], service
            for operation in (
                {"method": "initialize", "version": 1, "sessionId": str(uuid.uuid4()), "transportGeneration": 1},
                {"method": "ping"},
                {"method": "shutdown"},
            ):
                request = {"id": str(uuid.uuid4()), "runtimeId": hello["instanceId"], "operation": operation}
                body = json.dumps(request).encode()
                process.stdin.write(struct.pack(">I", len(body)) + body)
                reply = read_control(process.stdout)
                assert reply["id"] == request["id"] and reply["error"] is None, reply
            process.stdin.close()
            assert process.wait(timeout=10) == 0, process.stderr.read().decode(errors="replace")
        finally:
            if process.poll() is None:
                process.kill()


def commands(bin_dir):
    return {path.name: os.readlink(path) if path.is_symlink() else "regular-file"
            for path in sorted(bin_dir.iterdir())}


def scenario(root, label, shell, integrations, preserve, old_bytes, new_bytes, old_installer, new_installer):
    home = root / label
    home.mkdir()
    bin_dir = home / ".local/bin"
    bin_dir.mkdir(parents=True)
    env = {**os.environ, "HOME": str(home), "SHELL": f"/usr/bin/{shell}",
           "PATH": f"{bin_dir}:/usr/bin:/bin", "ZDOTDIR": str(home),
           "XDG_CONFIG_HOME": str(home / ".config")}
    for name in ("XDG_RUNTIME_DIR", "DISPLAY", "WAYLAND_DISPLAY", "XAUTHORITY", "NEWPORT_SESSION", "PORTHOP_SESSION"):
        env.pop(name, None)
    rc = home / {"bash": ".bashrc", "zsh": ".zshrc", "fish": ".config/fish/conf.d/newport.fish"}[shell]
    rc.parent.mkdir(parents=True, exist_ok=True)
    rc.write_text("# my shell settings\n")
    if preserve:
        (bin_dir / "xdg-open").write_text("#!/bin/sh\nprintf 'my browser\\n'\n")
        (bin_dir / "xdg-open").chmod(0o755)
        (bin_dir / "xclip").symlink_to("/usr/bin/xclip")
    native_before = {name: sha256(Path(name).read_bytes()) for name in ("/usr/bin/xclip", "/usr/bin/wl-paste")}
    installed = run(["sh", str(old_installer), sha256(old_bytes)], env, old_bytes)
    old = bin_dir / "porthop-agent"
    assert sha256(old.read_bytes()) == sha256(old_bytes)
    assert run([str(old), "--version"], env).stdout.strip() == b"porthop-agent/5"
    for name in OLD_NAMES:
        if not (preserve and name in ("xdg-open", "xclip")):
            assert (bin_dir / name).is_symlink() and os.readlink(bin_dir / name) == "porthop-agent", name
    for name, target in zip(CUSTOM_NAMES, ("porthop-agent", str(old), "../bin/porthop-agent", "custom-copy", "porthop-open")):
        (bin_dir / name).symlink_to(target)
    (bin_dir / "unrelated-link").symlink_to("/usr/bin/printf")
    (bin_dir / "unrelated-file").write_text("keep this command\n")
    before = commands(bin_dir)
    print(json.dumps({"case": label, "oldInstall": installed.stdout.decode().strip(), "before": before}), flush=True)

    with subprocess.Popen([str(old), "serve", "release-upgrade", "--clipboard", "--browser"], env=env,
                          stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                          stderr=subprocess.PIPE, bufsize=0) as running:
        try:
            header = read_exact(running.stdout, 5)
            assert header[:1] == b"R"
            assert read_exact(running.stdout, struct.unpack(">I", header[1:])[0]) == b"porthop-agent/5"
            assert (home / ".cache/porthop/clipboard").is_dir(), "old runtime did not create its state"
            digest = sha256(new_bytes)
            # A rejected upload must leave the live Porthop installation alone.
            failed = run(["sh", str(new_installer), "0" * 64], env, new_bytes, check=False)
            assert failed.returncode != 0 and running.poll() is None
            assert commands(bin_dir) == before
            run(["sh", str(new_installer), digest], env, new_bytes)
            candidate = home / ".local/share/newport/needle/builds" / digest / "newport-agent"
            assert commands(bin_dir) == before and running.poll() is None, "staging changed Porthop"
            verify_runtime(candidate, digest, env)
            assert running.poll() is None, "verification stopped Porthop before activation"
            arguments = [str(candidate), "deployment", "install", digest]
            if integrations:
                arguments.append("--integrations")
            activated = json.loads(run(arguments, env).stdout)
            assert activated == {"build": digest, "notice": None}, activated
            assert running.wait(timeout=5) == -signal.SIGKILL, "the old runtime was not stopped by cleanup"
        finally:
            if running.poll() is None:
                running.kill()

    for name in ("porthop-agent", "porthop-browser", "porthop-clip", "porthop-open", *CUSTOM_NAMES):
        assert not os.path.lexists(bin_dir / name), f"left legacy entry {name}"
    assert not os.path.lexists(home / ".cache/porthop"), "left the old clipboard cache"
    assert os.readlink(bin_dir / "unrelated-link") == "/usr/bin/printf"
    assert (bin_dir / "unrelated-file").read_text() == "keep this command\n"
    for name in NEW_NAMES:
        path = bin_dir / name
        if preserve and name == "xdg-open":
            assert not path.is_symlink() and run([str(path)], env).stdout == b"my browser\n"
        elif preserve and name == "xclip":
            assert os.readlink(path) == "/usr/bin/xclip"
        elif integrations:
            assert path.is_symlink() and os.readlink(path) == "newport-agent", name
        else:
            assert not os.path.lexists(path), name
    entry = bin_dir / "newport-agent"
    assert entry.is_symlink() and entry.resolve() == candidate
    assert sha256(entry.read_bytes()) == digest
    assert run([str(entry), "--version"], env).stdout.strip() == b"newport-agent/1"
    verify_runtime(entry, digest, env)
    assert rc.read_text().startswith("# my shell settings\n")
    assert rc.read_text().count("# >>> Newport >>>") == int(integrations)
    after = commands(bin_dir)
    shell_after = rc.read_text()
    # Reinstall the exact build: no duplicate shell block or changed commands.
    assert json.loads(run(arguments, env).stdout) == activated
    assert commands(bin_dir) == after and rc.read_text() == shell_after
    for name, checksum in native_before.items():
        assert sha256(Path(name).read_bytes()) == checksum, f"changed native command {name}"
    print(json.dumps({"case": label, "result": "PASS", "after": after,
                      "oldRuntimeStopped": True, "oldStateRemoved": True,
                      "runtimeHandshake": "initialize/ping/shutdown", "reinstallIdempotent": True}), flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("artifacts", type=Path)
    args = parser.parse_args()
    assert platform.system() == "Linux" and os.geteuid() != 0, "run as an unprivileged Linux container user"
    old_installer = args.artifacts / "porthop-install.sh"
    assert sha256(old_installer.read_bytes()) == INSTALLER_SHA256, "wrong old installer fixture"
    new_installer = Path(__file__).resolve().parents[2] / "crates/newport-backend/src/agent-install.sh"
    old_bytes = release_agent(args.artifacts / ARCHIVE)
    new_bytes = (args.artifacts / "newport-agent").read_bytes()
    print(json.dumps({"newAgentSha256": sha256(new_bytes), "uid": os.geteuid()}), flush=True)
    cases = [("bash", "bash", True, False), ("zsh", "zsh", True, False),
             ("fish", "fish", True, False), ("monitor-only", "bash", False, False),
             ("preserve-native", "bash", True, True)]
    with tempfile.TemporaryDirectory(prefix="newport-release-upgrade-") as directory:
        for label, shell, integrations, preserve in cases:
            scenario(Path(directory), label, shell, integrations, preserve,
                     old_bytes, new_bytes, old_installer, new_installer)
    print(f"PASS: {len(cases)} released Porthop upgrade scenarios", flush=True)


if __name__ == "__main__":
    main()
