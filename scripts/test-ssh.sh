#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
cargo build --manifest-path tools/agent/Cargo.toml --locked
fixture_dir="$(mktemp -d /tmp/newport-ssh-test.XXXXXX)"
fixture_pid=''
cleanup() { if [ -n "$fixture_pid" ]; then kill "$fixture_pid" 2>/dev/null || true; wait "$fixture_pid" 2>/dev/null || true; fi; rm -rf "$fixture_dir"; }
trap cleanup EXIT HUP INT TERM
"${NEWPORT_TEST_PYTHON:-python3}" -c 'import paramiko' || { echo 'Install paramiko into a venv and set NEWPORT_TEST_PYTHON to its Python executable.' >&2; exit 1; }
"${NEWPORT_TEST_PYTHON:-python3}" scripts/ssh-fixture.py --directory "$fixture_dir" > "$fixture_dir/fixture.log" 2>&1 &
fixture_pid=$!
tries=0
while [ ! -f "$fixture_dir/port" ]; do
  tries=$((tries + 1))
  if [ "$tries" -gt 50 ]; then cat "$fixture_dir/fixture.log"; exit 1; fi
  sleep 0.1
done
SSH_AUTH_SOCK="$fixture_dir/agent.sock" \
NEWPORT_TEST_PASSWORD="fixture-password" \
NEWPORT_TEST_OTHER_KEY="$fixture_dir/other.pub" \
NEWPORT_TEST_SSH_PORT="$(cat "$fixture_dir/port")" \
NEWPORT_TEST_IDENTITY="$fixture_dir/client" \
NEWPORT_TEST_KNOWN_HOSTS="$fixture_dir/known_hosts" \
cargo test --manifest-path src-tauri/Cargo.toml "$@" -- --ignored --nocapture --test-threads=1 || { cat "$fixture_dir/fixture.log"; exit 1; }
