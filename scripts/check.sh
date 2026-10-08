#!/bin/sh
# The full suite. CI runs exactly this, and so should anyone about to merge into master.
#
#     scripts/check.sh           build, test, lint, and build every example plugin
#     scripts/check.sh --miri    also run the ECS tests under Miri (needs nightly + miri)
#
# The Haskell plugin tests skip themselves when GHC isn't installed. Set VOXL_REQUIRE_GHC=1
# (as CI does) to make a missing GHC a failure instead.
set -eu
cd "$(dirname "$0")/.."

step() { printf '\n== %s\n' "$1"; }

step "build"
cargo build --workspace --all-targets

step "test"
cargo test --workspace

step "clippy"
cargo clippy --workspace --all-targets -- -D warnings

step "example plugins"
plugins/pulse/build.sh
if command -v ghc >/dev/null 2>&1; then
    plugins/swirl/build.sh
    plugins/chase/build.sh
elif [ "${VOXL_REQUIRE_GHC:-0}" = 1 ]; then
    echo "GHC is required (VOXL_REQUIRE_GHC=1) but not installed" >&2
    exit 1
else
    echo "GHC not installed: the Haskell plugins were not built"
fi

if [ "${1:-}" = "--miri" ]; then
    step "miri"
    cargo +nightly miri test --lib -- ecs::
fi

printf '\nAll checks passed.\n'
