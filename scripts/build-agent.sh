#!/usr/bin/env bash
# Build portable Linux agents and embed them in desktop builds.
set -euo pipefail
cd "$(dirname "$0")/.."
envdir="$PWD/tools/agent/target/cross-build-env"
if ! [ -x "$envdir/bin/python" ]; then
    python3 -m venv "$envdir"
fi
# Zig 0.16's libc strnlen vectorizes past a terminating NUL across page
# boundaries. This crashes libgit2's printf calls during real remote pulls.
# Validate cached environments too: changing only the install pin leaves old
# developer and CI environments using the broken runtime.
if ! "$envdir/bin/python" -c 'from importlib.metadata import version; assert version("cargo-zigbuild") == "0.23.4" and version("ziglang") == "0.15.2"' 2>/dev/null; then
    "$envdir/bin/python" -m pip install 'cargo-zigbuild==0.23.4' 'ziglang==0.15.2'
fi
zigdir="$("$envdir/bin/python" -c 'import pathlib, ziglang; print(pathlib.Path(ziglang.__file__).parent)')"
export PATH="$zigdir:$envdir/bin:$PATH"
# Cargo does not fingerprint Zig's bundled libc. Keep toolchain builds separate
# so an old linked runtime cannot survive a compiler version change.
export CARGO_TARGET_DIR="$PWD/tools/agent/target/linux-zig-0.15.2"
rustup target add x86_64-unknown-linux-musl aarch64-unknown-linux-musl
cargo zigbuild --manifest-path tools/agent/Cargo.toml --release --locked --bin newport-agent \
    --target x86_64-unknown-linux-musl --target aarch64-unknown-linux-musl
mkdir -p src-tauri/agents
for arch in x86_64 aarch64; do
    cp "$CARGO_TARGET_DIR/$arch-unknown-linux-musl/release/newport-agent" "src-tauri/agents/newport-agent-$arch"
done
