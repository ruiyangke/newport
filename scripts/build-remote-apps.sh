#!/bin/sh
# Build Newport with Remote apps, using a private newport-remote checkout.
#
#   scripts/build-remote-apps.sh [--run] [cargo build arguments...]
#   scripts/build-remote-apps.sh --clean
#
# NEWPORT_REMOTE_DIR names the newport-remote checkout (default:
# ../newport-remote, beside this repository). Without access to it, build the
# free edition with plain `cargo build`.
#
# Adds newport-remote's streaming crates to newport-gpui with `cargo add`,
# builds newport-app with NEWPORT_REMOTE_APPS=1 (which compiles Remote apps
# streaming in), and takes the crates out of the manifests and lockfile again
# as soon as the build ends, so the checkout never keeps a reference to the
# private crates. --run then opens the signed development bundle. --clean only
# takes the crates out, for a build that was killed before it could.
#
# Install on the Remote apps page sends a compiled agent to the server; the
# build embeds one archive per server architecture. They come from
# newport-remote's static agents, which are rebuilt in Docker when missing or
# older than the agent's sources. Servers never receive source.
#
# Remote app windows run inside Newport's GPUI application, so the streaming
# crates must use this GPUI, not a second copy. Their crates.io `gpui-pre`
# crates are patched to the revision this lockfile uses.
set -eu
newport=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd -P)
cd "$newport"
crates="newport-remote-client newport-remote-config newport-remote-protocol
    newport-remote-provider-gpui newport-remote-transport newport-remote-app"

# Take the private crates out of Newport's manifests, keeping every other
# edit, including ones another tool made during the build.
clean() {
    present=
    for crate in $crates; do
        if grep -q "^$crate = " crates/newport-gpui/Cargo.toml; then
            present="$present $crate"
        fi
    done
    if [ -n "$present" ]; then
        # shellcheck disable=SC2086
        cargo remove --quiet -p newport-gpui $present
    fi
    # Resolving offline drops the private packages from the lockfile and
    # leaves every other entry as it was.
    cargo metadata --format-version 1 --offline >/dev/null
    if grep -q 'newport-remote' Cargo.lock crates/newport-gpui/Cargo.toml; then
        echo "Cargo.lock or crates/newport-gpui/Cargo.toml still names newport-remote." >&2
        return 1
    fi
}

if [ "${1:-}" = --clean ]; then
    clean
    exit
fi

remote=${NEWPORT_REMOTE_DIR:-$newport/../newport-remote}
[ -d "$remote/crates/newport-remote-client" ] || {
    echo "No newport-remote checkout at $remote. Set NEWPORT_REMOTE_DIR." >&2
    exit 2
}
remote=$(CDPATH= cd -- "$remote" && pwd -P)
case "$remote/" in
    "$newport"/*) echo "Keep the newport-remote checkout outside $newport." >&2; exit 2 ;;
esac
run=0
if [ "${1:-}" = --run ]; then
    run=1
    shift
fi

# This GPUI: `source = "git+URL?rev=REV#REV"` for gpui-pre in the lockfile.
source=$(awk '/^name = "gpui-pre"$/ { found = 1; next }
    found && /^source = / { print; exit }
    found && /^\[\[package\]\]/ { exit }' Cargo.lock)
case "$source" in
    'source = "git+'*'?rev='*'#'*'"') ;;
    *) echo "Cargo.lock has no git revision for gpui-pre: $source" >&2; exit 1 ;;
esac
gpui_url=$(printf '%s\n' "$source" | sed -E 's/^source = "git\+([^?]+)\?rev=.*$/\1/')
gpui_rev=$(printf '%s\n' "$source" | sed -E 's/^.*#([0-9a-f]+)"$/\1/')

# Copies to fall back on if taking the crates out fails.
saved=$(mktemp -d)
cp Cargo.lock "$saved/Cargo.lock"
cp crates/newport-gpui/Cargo.toml "$saved/Cargo.toml"
cleaned=0
finish() {
    [ "$cleaned" = 1 ] && return
    cleaned=1
    if ! clean; then
        echo "Restoring Cargo.lock and crates/newport-gpui/Cargo.toml from before the build." >&2
        cp "$saved/Cargo.lock" Cargo.lock
        cp "$saved/Cargo.toml" crates/newport-gpui/Cargo.toml
    fi
    rm -rf "$saved"
}
trap finish EXIT
trap 'finish; exit 130' INT
trap 'finish; exit 143' TERM

for crate in client config protocol provider-gpui transport; do
    cargo add --quiet -p newport-gpui --path "$remote/crates/newport-remote-$crate"
done
# The install command and release identity, without the standalone client.
cargo add --quiet -p newport-gpui --path "$remote/crates/newport-remote-app" \
    --no-default-features --features install

# The compiled agent archives Install sends, at stable paths so an unchanged
# agent doesn't rebuild Newport. A static agent is rebuilt first when anything
# it is built from is newer.
agents="$newport/target/remote-agent"
mkdir -p "$agents"
sh "$remote/scripts/ensure-static-agent.sh" amd64 arm64
for pair in x86_64:amd64 aarch64:arm64; do
    arch=${pair%%:*}
    (cd "$remote" && cargo xtask deploy-agent --archive "$agents/$arch.tar.gz.new" --arch "$arch")
    if cmp -s "$agents/$arch.tar.gz.new" "$agents/$arch.tar.gz"; then
        rm "$agents/$arch.tar.gz.new"
    else
        mv "$agents/$arch.tar.gz.new" "$agents/$arch.tar.gz"
    fi
done
export NEWPORT_REMOTE_AGENT_X86_64="$agents/x86_64.tar.gz"
export NEWPORT_REMOTE_AGENT_AARCH64="$agents/aarch64.tar.gz"
# This GPUI for the crates.io `gpui-pre` crates. Command-line configuration
# keeps the patch out of the manifests. The URL and revision contain no
# spaces, so the words split cleanly.
patch="--config patch.crates-io.gpui-pre.git='$gpui_url'
    --config patch.crates-io.gpui-pre.rev='$gpui_rev'
    --config patch.crates-io.gpui-pre-platform.git='$gpui_url'
    --config patch.crates-io.gpui-pre-platform.rev='$gpui_rev'"
# Resolve the added crates once, without --locked, before the caller's build,
# and refuse a graph that would still contain a second GPUI.
# shellcheck disable=SC2086
duplicates=$(cargo tree $patch --quiet -p newport-app --duplicates --depth 0 -e normal |
    grep -E '^gpui-pre(-platform)? ' || true)
if [ -n "$duplicates" ]; then
    echo "Two copies of GPUI would be built:" >&2
    printf '%s\n' "$duplicates" >&2
    exit 1
fi
# shellcheck disable=SC2086
NEWPORT_REMOTE_APPS=1 cargo xtask env -- cargo build $patch -p newport-app "$@"
# The private crates are only needed to compile; they are out of the
# manifests again before the app starts.
finish
[ "$run" = 1 ] || exit 0

# Launch as `cargo run` would, through the development runner.
profile=debug
for argument in "$@"; do
    [ "$argument" = --release ] && profile=release
done
exec cargo xtask dev-run "${CARGO_TARGET_DIR:-$newport/target}/$profile/newport"
