#!/usr/bin/env bash
# Build portable Linux agents and embed them in desktop builds.
set -euo pipefail
cd "$(dirname "$0")/.."
envdir="$PWD/tools/agent/target/cross-build-env"
if ! [ -x "$envdir/bin/cargo-zigbuild" ]; then
    python3 -m venv "$envdir"
    "$envdir/bin/python" -m pip install 'cargo-zigbuild==0.23.4' 'ziglang==0.16.0'
fi
zigdir="$("$envdir/bin/python" -c 'import pathlib, ziglang; print(pathlib.Path(ziglang.__file__).parent)')"
export PATH="$zigdir:$envdir/bin:$PATH"
rustup target add x86_64-unknown-linux-musl aarch64-unknown-linux-musl
cargo zigbuild --manifest-path tools/agent/Cargo.toml --release --locked --bin porthop-agent \
    --target x86_64-unknown-linux-musl --target aarch64-unknown-linux-musl
mkdir -p src-tauri/agents
for arch in x86_64 aarch64; do
    cp "tools/agent/target/$arch-unknown-linux-musl/release/porthop-agent" "src-tauri/agents/porthop-agent-$arch"
done
