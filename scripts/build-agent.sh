#!/usr/bin/env bash
# Compatibility entry point for local builds and CI.
set -euo pipefail
cd "$(dirname "$0")/.."
exec node scripts/build-linux-agents.mjs
