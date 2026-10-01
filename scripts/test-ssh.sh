#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
exec cargo run --locked --manifest-path tools/test-harness/Cargo.toml -- ssh "$@"
