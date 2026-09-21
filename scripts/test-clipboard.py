#!/usr/bin/env python3
"""Exercise the actual server installer, snapshot receiver and xclip entry point."""
import io
import tarfile
import shutil
from concurrent.futures import ThreadPoolExecutor
import os
from pathlib import Path
import subprocess
import tempfile
import time
import unittest
import uuid

SOURCE = Path(__file__).resolve().parents[1] / "src-tauri/src"
HELPER = (SOURCE / "porthop-clip.sh").read_bytes()
INSTALLER = (SOURCE / "clipboard-install.sh").read_text()


class ClipboardTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.home = Path(self.tmp.name)
        self.env = dict(os.environ, HOME=str(self.home), PORTHOP_CLIPBOARD_NATIVE="0")
        for key in ("DISPLAY", "WAYLAND_DISPLAY", "WAYLAND_SOCKET", "XDG_RUNTIME_DIR"):
            self.env.pop(key, None)
        # Linux servers have util-linux flock. On macOS the tests use the same
        # inherited-fd kernel locking through this test-only adapter.
        if not shutil.which("flock"):
            tools = self.home / "test-bin"
            tools.mkdir()
            lock = tools / "flock"
            lock.write_text('#!/usr/bin/env python3\nimport fcntl, sys\nfcntl.flock(int(sys.argv[2]), fcntl.LOCK_SH if sys.argv[1] == "-s" else fcntl.LOCK_EX)\n')
            lock.chmod(0o700)
            self.env["PATH"] = str(tools) + os.pathsep + self.env["PATH"]
        self.bin = self.home / ".local/bin"
        self.helper = self.bin / "porthop-clip"
        self.shim = self.bin / "xclip"
        self.state = self.home / ".cache/porthop/clipboard/snapshot.tar"
        self.token = str(uuid.uuid4())
        self.install()
        self.run_helper("--begin", self.token)

    def command(self, args, payload=None, ok=True):
        result = subprocess.run(args, input=payload, capture_output=True, env=self.env, timeout=10)
        if ok:
            self.assertEqual(result.returncode, 0, result.stderr.decode())
        else:
            self.assertNotEqual(result.returncode, 0)
        return result

    def install(self, ok=True):
        return self.command(["sh", "-c", INSTALLER], HELPER, ok)

    def run_helper(self, *args, payload=None, ok=True):
        return self.command([str(self.helper), *args], payload, ok)

    def xclip(self, *args, ok=True):
        return self.command([str(self.shim), *args], ok=ok)

    def push(self, formats):
        targets = [*formats, "TARGETS"]
        if "text/plain" in formats:
            targets.append("UTF8_STRING")
        formats = dict(formats, TARGETS=("\n".join(targets) + "\n").encode())
        archive = io.BytesIO()
        with tarfile.open(fileobj=archive, mode="w") as tar:
            for name, data in formats.items():
                entry = tarfile.TarInfo(name)
                entry.size = len(data)
                tar.addfile(entry, io.BytesIO(data))
        return self.run_helper("--receive", self.token, payload=archive.getvalue())

    def native_tools(self, local=False):
        self.env["PORTHOP_CLIPBOARD_NATIVE"] = "1"
        tools = self.home / "native-bin"
        tools.mkdir()
        # Bound native calls on macOS too; production Linux uses coreutils timeout.
        if not shutil.which("timeout"):
            timer = tools / "timeout"
            timer.write_text('''#!/usr/bin/env python3
import os, signal, subprocess, sys
process = subprocess.Popen(sys.argv[4:], start_new_session=True)
try:
    sys.exit(process.wait(timeout=float(sys.argv[3])))
except subprocess.TimeoutExpired:
    os.killpg(process.pid, signal.SIGKILL)
    process.wait()
    sys.exit(124)
''')
            timer.chmod(0o700)
        native = self.bin / "xclip" if local else tools / "xclip"
        if local:
            native.unlink()
        native.write_text('''#!/usr/bin/env bash
set -eu
[[ ! -e /dev/fd/9 ]] || exit 90
printf '%s|%s\\n' "${DISPLAY:-}" "${XAUTHORITY:-}" >> "$HOME/native-calls"
[[ "${XCLIP_TEST_HANG:-}" != 1 ]] || exec sleep 60
if [[ "${XCLIP_TEST_FAIL:-}" == 1 ]]; then printf 'partial failed output'; exit 1; fi
operation=''; target=''
while (( $# )); do
    case "$1" in
        -in|-out) operation="$1" ;;
        -target) target="$2"; shift ;;
    esac
    shift
done
if [[ "$operation" == -in ]]; then
    cat > "$HOME/native-data"
    printf '%s' "$target" > "$HOME/native-target"
else
    [[ "$target" == "$(cat "$HOME/native-target")" ]] || exit 1
    cat "$HOME/native-data"
fi
''')
        native.chmod(0o700)
        # Put the Porthop shim first to prove native lookup skips itself.
        self.env["PATH"] = str(self.bin) + os.pathsep + str(tools) + os.pathsep + self.env["PATH"]
        return native

    def wayland_tools(self):
        self.native_tools()
        tool = self.home / "native-bin/wl-copy"
        tool.write_text('''#!/usr/bin/env bash
set -eu
[[ ! -e /dev/fd/9 ]] || exit 90
printf '%s|%s\\n' "${WAYLAND_DISPLAY:-}" "${XDG_RUNTIME_DIR:-}" >> "$HOME/wayland-calls"
[[ "${WL_TEST_HANG:-}" != 1 ]] || exec sleep 60
[[ "${WL_TEST_FAIL:-}" != 1 ]] || exit 1
[[ "$1" == --type ]]
printf '%s' "$2" > "$HOME/wayland-type"
cat > "$HOME/wayland-data"
''')
        tool.chmod(0o700)
        self.env.update(WAYLAND_DISPLAY="wayland-1", XDG_RUNTIME_DIR=str(self.home / "runtime"))

    def test_wayland_preferred_and_binary_formats_preserved(self):
        self.wayland_tools()
        for mime, data in [("text/plain", b"hello\n"), ("image/png", bytes(range(256))),
                           ("text/html", b"<b>hello</b>"), ("text/uri-list", b"https://example.com\n")]:
            self.assertIn(b"Wayland clipboard updated", self.push({mime: data}).stdout)
            self.assertEqual((self.home / "wayland-data").read_bytes(), data)
            self.assertEqual((self.home / "wayland-type").read_text(), mime)
            self.assertEqual(self.xclip("-o", "-t", mime).stdout, data)
        self.assertFalse((self.home / "native-calls").exists())
        self.assertIn("wayland-1|", (self.home / "wayland-calls").read_text())
        self.push({})
        self.assertEqual((self.home / "wayland-data").read_bytes(), b"")
        self.assertEqual((self.home / "wayland-type").read_text(), "text/plain")

    def test_wayland_failure_falls_back_to_x_then_files(self):
        self.wayland_tools()
        self.env["WL_TEST_FAIL"] = "1"
        self.assertIn(b"Native X", self.push({"text/plain": b"X fallback"}).stdout)
        self.env["XCLIP_TEST_FAIL"] = "1"
        self.assertIn(b"file-backed", self.push({"text/plain": b"file fallback"}).stdout)
        self.assertEqual(self.xclip("-o").stdout, b"file fallback")
        self.env["WL_TEST_HANG"] = "1"
        started = time.monotonic()
        self.assertIn(b"file-backed", self.push({"text/plain": b"bounded"}).stdout)
        self.assertLess(time.monotonic() - started, 5)
        self.env["PORTHOP_CLIPBOARD_NATIVE"] = "0"
        calls = (self.home / "wayland-calls").read_bytes()
        self.push({"text/plain": b"disabled"})
        self.assertEqual((self.home / "wayland-calls").read_bytes(), calls)

    def test_wayland_requires_session_environment_and_clears_x_marker(self):
        self.wayland_tools()
        del self.env["WAYLAND_DISPLAY"]
        runtime = self.env.pop("XDG_RUNTIME_DIR")
        self.assertIn(b"Native X", self.push({"text/plain": b"X"}).stdout)
        self.assertFalse((self.home / "wayland-calls").exists())
        self.env["XDG_RUNTIME_DIR"] = runtime
        self.assertIn(b"Wayland", self.push({"text/plain": b"Wayland"}).stdout)
        self.assertFalse((self.state.parent / "native-display").exists())
        self.assertEqual(self.xclip("-o").stdout, b"Wayland")

    def test_native_default_display_and_format_fallback(self):
        self.native_tools()
        self.assertIn(b"Native X clipboard updated on DISPLAY=:0", self.push({
            "text/plain": b"Mac text", "text/html": b"<b>Mac text</b>"
        }).stdout)
        self.assertEqual((self.home / "native-data").read_bytes(), b"Mac text")
        self.assertEqual((self.home / "native-target").read_text(), "UTF8_STRING")
        # A subsequent desktop copy proves that the shim really delegates reads.
        (self.home / "native-data").write_bytes(b"desktop text")
        self.assertEqual(self.xclip("-o").stdout, b"desktop text")
        self.assertEqual(self.run_helper("text").stdout, b"Mac text")
        self.assertEqual(self.xclip("-o", "-t", "text/html").stdout, b"<b>Mac text</b>")
        self.assertTrue(all(call == ":0|" for call in (self.home / "native-calls").read_text().splitlines()))

    def test_native_preserves_display_and_authorization(self):
        self.native_tools()
        self.env.update(DISPLAY="localhost:10.0", XAUTHORITY="/custom/user cookie")
        self.push({"text/plain": b"forwarded"})
        self.assertEqual(self.xclip("-o").stdout, b"forwarded")
        self.assertTrue(all(call == "localhost:10.0|/custom/user cookie"
                            for call in (self.home / "native-calls").read_text().splitlines()))
        self.env["DISPLAY"] = ":7"
        self.assertEqual(self.xclip("-o").stdout, b"forwarded")
        self.assertNotIn(":7", (self.home / "native-calls").read_text())

    def test_native_failures_and_timeout_keep_snapshot_available(self):
        self.native_tools()
        self.push({"text/plain": b"old"})
        self.env["XCLIP_TEST_FAIL"] = "1"
        # No partial failed native output may leak into fallback reads.
        self.assertEqual(self.xclip("-o").stdout, b"old")
        self.assertIn(b"file-backed", self.push({"text/plain": b"new"}).stdout)
        self.assertFalse((self.state.parent / "native-display").exists())
        self.assertEqual(self.xclip("-o").stdout, b"new")
        del self.env["XCLIP_TEST_FAIL"]
        self.env["XCLIP_TEST_HANG"] = "1"
        started = time.monotonic()
        self.assertIn(b"file-backed", self.push({"text/plain": b"timeout fallback"}).stdout)
        self.assertLess(time.monotonic() - started, 5)
        self.assertEqual(self.xclip("-o").stdout, b"timeout fallback")
        del self.env["XCLIP_TEST_HANG"]
        self.assertIn(b"Native X", self.push({"text/plain": b"recovered"}).stdout)

    def test_native_image_and_empty_clipboard(self):
        self.native_tools()
        image = b"\\x89PNG\\r\\n\\x1a\\n" + bytes(range(256))
        self.push({"image/png": image})
        self.assertEqual((self.home / "native-target").read_text(), "image/png")
        self.assertEqual(self.xclip("-o", "-t", "image/png").stdout, image)
        self.push({})
        self.assertEqual((self.home / "native-data").read_bytes(), b"")
        self.assertEqual((self.home / "native-target").read_text(), "UTF8_STRING")
        self.run_helper("--clear", self.token)
        self.assertFalse((self.state.parent / "native-display").exists())

    def test_existing_local_native_xclip_is_preserved(self):
        native = self.native_tools(local=True)
        before = native.read_bytes()
        self.assertIn(b"preserved", self.install().stdout)
        self.assertEqual(native.read_bytes(), before)
        self.assertIn(b"Native X", self.push({"text/plain": b"local native"}).stdout)

    def test_text_binary_formats_and_empty_clipboard(self):
        text = "hello ' $HOME `no shell` 世界\n\n".encode()
        png = b"\x89PNG\r\n\x1a\n" + bytes(range(256)) * 20000
        self.push({"text/plain": text, "text/html": b"<b>hi</b>", "image/png": png})
        self.assertEqual(self.xclip("-o").stdout, text)
        self.assertEqual(self.xclip("-selection", "clipboard", "-o", "-t", "image/png").stdout, png)
        self.assertEqual(self.xclip("-sel", "p", "-out", "-target", "text/html").stdout, b"<b>hi</b>")
        targets = self.xclip("-o", "-t", "TARGETS").stdout.splitlines()
        self.assertIn(b"UTF8_STRING", targets)
        self.assertIn(b"image/png", targets)
        self.assertEqual(self.run_helper("data", "public.png").stdout, png)
        destination = self.home / "image.png"
        self.run_helper("image", str(destination))
        self.assertEqual(destination.read_bytes(), png)
        self.push({"text/plain": b""})
        self.assertEqual(self.xclip("-o").stdout, b"")
        self.xclip("-o", "-t", "image/png", ok=False)
        self.push({})
        self.xclip("-o", ok=False)
        self.assertEqual(self.xclip("-o", "-t", "TARGETS").stdout, b"TARGETS\n")

    def test_read_only_arguments(self):
        self.push({"text/plain": b"original"})
        for args in [[], ["-i"], ["-selection"], ["-selection", "bad", "-o"],
                     ["--receive", self.token], ["-display", ":0", "-o"]]:
            self.xclip(*args, ok=False)
        self.assertEqual(self.xclip("-o").stdout, b"original")

    def test_permissions_ownership_and_cleanup(self):
        self.push({"text/plain": b"private"})
        self.assertEqual(self.state.stat().st_mode & 0o777, 0o600)
        self.assertEqual(self.state.parent.stat().st_mode & 0o777, 0o700)
        other = str(uuid.uuid4())
        self.run_helper("--begin", other, ok=False)
        self.run_helper("--receive", other, payload=b"broken", ok=False)
        self.run_helper("--clear", other)
        self.assertEqual(self.xclip("-o").stdout, b"private")
        self.run_helper("--heartbeat", self.token)
        self.run_helper("--clear", self.token)
        self.assertFalse(self.state.exists())
        self.xclip("-o", ok=False)

    def test_expiry_and_invalid_transfer_preserve_previous_snapshot(self):
        self.push({"text/plain": b"original"})
        for payload in (b"broken", b"partial tar"):
            self.run_helper("--receive", self.token, payload=payload, ok=False)
        self.assertEqual(self.xclip("-o").stdout, b"original")
        os.utime(self.state, (time.time() - 121, time.time() - 121))
        self.assertIn(b"expired", self.xclip("-o", ok=False).stderr)
        # Expiry blocks reads; deleting stored data requires a live SSH cleanup.
        self.assertTrue(self.state.exists())

    def test_install_preserves_conflicts_and_reports_path(self):
        self.assertIn(b"PORTHOP_SHIM_PATH=missing", self.install().stdout)
        self.env["PATH"] = str(self.bin) + os.pathsep + self.env["PATH"]
        self.assertIn(b"PORTHOP_SHIM_PATH=ready", self.install().stdout)
        self.shim.unlink()
        self.shim.write_text("unrelated executable")
        self.assertIn(b"already exists", self.install(ok=False).stderr)
        self.assertEqual(self.shim.read_text(), "unrelated executable")
        self.shim.unlink()
        self.shim.mkdir()
        self.install(ok=False)
        self.assertTrue(self.shim.is_dir())
        self.shim.rmdir()
        self.helper.write_text("unrelated helper")
        self.install(ok=False)
        self.assertEqual(self.helper.read_text(), "unrelated helper")

    def test_atomic_read_during_updates(self):
        first, second = b"a" * 500000, b"b" * 500000
        self.push({"text/plain": first})
        def write():
            for i in range(8):
                self.push({"text/plain": second if i % 2 else first})
        def read():
            for _ in range(10):
                self.assertIn(self.xclip("-o").stdout, (first, second))
        with ThreadPoolExecutor(max_workers=3) as pool:
            futures = [pool.submit(write), pool.submit(read), pool.submit(read)]
            for result in futures:
                result.result()

    def test_same_client_resumes_but_old_session_cannot_write_or_clear(self):
        self.run_helper("--clear", self.token)
        client = str(uuid.uuid4())
        old = self.token
        self.run_helper("--begin", old, client)
        self.push({"text/plain": b"before restart"})
        self.token = str(uuid.uuid4())
        self.run_helper("--begin", self.token, client)
        self.push({"text/plain": b"after restart"})
        self.run_helper("--begin", str(uuid.uuid4()), str(uuid.uuid4()), ok=False)
        self.run_helper("--heartbeat", old, ok=False)
        payload = self.state.read_bytes()
        self.run_helper("--receive", old, payload=payload, ok=False)
        self.run_helper("--clear", old)
        self.assertEqual(self.xclip("-o").stdout, b"after restart")

    def test_new_session_rejects_delayed_old_upload_and_cleanup(self):
        old = self.token
        self.push({"text/plain": b"old"})
        lease = self.state.parent / "session"
        os.utime(lease, (time.time() - 121, time.time() - 121))
        self.token = str(uuid.uuid4())
        self.run_helper("--begin", self.token)
        self.push({"text/plain": b"new"})
        payload = self.state.read_bytes()
        self.run_helper("--receive", old, payload=payload, ok=False)
        self.run_helper("--clear", old)
        self.assertEqual(self.xclip("-o").stdout, b"new")


if __name__ == "__main__":
    unittest.main()
