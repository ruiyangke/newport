#!/bin/bash
# Compatibility entry point for the native signed release packager.
set -euo pipefail
cd "$(dirname "$0")/.."
exec node scripts/release-native-macos.mjs "$@"
