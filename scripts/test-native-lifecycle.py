#!/usr/bin/env python3
"""Smoke-test an already-built macOS bundle with an isolated offline fixture profile.

No accessibility permission, real servers, Keychain entries, or clipboard access.
"""
import os
import json
from pathlib import Path
import signal
import sqlite3
import subprocess
import tempfile
import time

root = Path(__file__).resolve().parent.parent
binary = root / 'src-tauri/target/release/bundle/macos/Porthop.app/Contents/MacOS/porthop'
if not binary.exists():
    raise SystemExit('Build the app first: npm run tauri build -- --bundles app')
with tempfile.TemporaryDirectory(prefix='porthop-native-smoke-') as directory:
    profile = Path(directory, 'profile')
    profile.mkdir()
    fixture_key = Path(directory, 'fixture-key')
    fixture_key.write_bytes(os.urandom(32))
    fixture_key.chmod(0o600)
    Path(profile, 'config.json').write_text(json.dumps({"servers": [{
        "id": "8d43df10-73f7-4358-b818-c41dddc2006e", "name": "Offline fixture",
        "sshHost": "127.0.0.1", "sshPort": 1, "sshUser": "fixture", "clipboardEnabled": True,
    }], "tunnels": []}))
    env = dict(os.environ, PORTHOP_DATA_DIR=str(profile), PORTHOP_TEST_VAULT_KEY_FILE=str(fixture_key))
    app = subprocess.Popen([str(binary)], env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    try:
        deadline = time.monotonic() + 10
        while not Path(profile, 'profiles.stronghold').exists():
            if app.poll() is not None:
                raise RuntimeError(f'App exited during startup: {app.communicate()}')
            if time.monotonic() > deadline:
                raise TimeoutError('App did not initialize isolated profiles')
            time.sleep(0.1)
        deadline = time.monotonic() + 10
        while not Path(profile, 'preferences.json').exists():
            if app.poll() is not None or time.monotonic() > deadline:
                raise RuntimeError('Frontend did not initialize Store preferences')
            time.sleep(0.1)
        assert json.loads(Path(profile, 'preferences.json').read_text())['sidebarWidth'] == 204
        assert app.poll() is None, 'App must remain running after initialization'
        metrics = profile / 'metrics.sqlite3'
        assert metrics.stat().st_mode & 0o777 == 0o600
        with sqlite3.connect(metrics) as db:
            assert db.execute('PRAGMA user_version').fetchone()[0] == 2
            assert db.execute('SELECT version FROM _sqlx_migrations WHERE success=1').fetchall() == [(1,)]
            assert db.execute('SELECT COUNT(*) FROM samples').fetchone()[0] == 0

        second = subprocess.run([str(binary)], env=env, capture_output=True, timeout=5)
        assert second.returncode == 0 and b'already running' in second.stderr, second.stderr
        app.send_signal(signal.SIGTERM)
        output, error = app.communicate(timeout=10)
        assert app.returncode == 0, f'Graceful shutdown failed: {app.returncode}: {error!r}'
        assert not Path(profile, 'config.json').exists(), 'Plaintext profile must be removed after verified migration'
        assert Path(profile, 'profiles.stronghold').stat().st_size > 0
        diagnostics = Path(profile, 'logs', 'porthop.log').read_text()
        assert 'Desktop and background workers initialized' in diagnostics, diagnostics
        assert 'Background workers and SSH sessions stopped' in diagnostics, diagnostics
        assert 'Restoring clipboard sharing for 1 saved profiles' in diagnostics, diagnostics
        assert profile.stat().st_mode & 0o777 == 0o700
        before = Path(profile, 'profiles.stronghold').read_bytes()
        app = subprocess.Popen([str(binary), '--autostart'], env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        time.sleep(2)
        assert app.poll() is None, 'Background launch must remain running'
        app.send_signal(signal.SIGTERM)
        output, error = app.communicate(timeout=10)
        assert app.returncode == 0, error
        assert Path(profile, 'profiles.stronghold').read_bytes() == before, 'Reopening must preserve the vault'
        with sqlite3.connect(metrics) as db:
            assert db.execute('SELECT COUNT(*) FROM _sqlx_migrations').fetchone()[0] == 1

        assert not Path(profile, 'config.json').exists()
        assert Path(profile, 'logs', 'porthop.log').read_text().count('Restoring clipboard sharing for 1 saved profiles') == 2
        print('PASS: native encrypted migration/reopen, SQL plugin migration/reopen, Store IPC, background launch, logging, instance lock and graceful shutdown')
    finally:
        if app.poll() is None:
            app.kill()
            app.wait()
