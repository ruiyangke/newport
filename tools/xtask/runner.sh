#!/bin/sh
# Cargo runner: `newport` goes through `xtask dev-run`, which bundles, signs and
# launches it; every other binary, including xtask itself, runs directly.
case "${1##*/}" in
newport) ;;
*) exec "$@" ;;
esac
repo=$(CDPATH='' cd -- "$(dirname -- "$0")/../.." && pwd -P) || exit
# Drop the package variables the outer `cargo run` set for newport; they would
# make the nested build of xtask look changed and rebuild it.
for name in $(env | sed -n -E 's/^((CARGO_MANIFEST|CARGO_PKG)_[A-Z_]*|CARGO_CRATE_NAME|CARGO_BIN_NAME|CARGO_PRIMARY_PACKAGE|OUT_DIR)=.*/\1/p'); do
  unset "$name"
done
exec env -u CARGO_BUILD_TARGET cargo run --quiet --locked \
  --manifest-path "$repo/tools/xtask/Cargo.toml" -- dev-run "$@"
