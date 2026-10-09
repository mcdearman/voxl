#!/bin/sh
# The full suite. CI runs exactly this, and so should anyone about to merge into master.
#
#     scripts/check.sh           build, test, lint, and build every example plugin
#     scripts/check.sh --miri    also run the ECS tests under Miri (needs nightly + miri)
#     scripts/check.sh --miri-only
#
# The Haskell plugin tests skip themselves when GHC isn't installed. Set MIRA_REQUIRE_GHC=1
# (as CI does) to make a missing GHC a failure instead.
set -eu
cd "$(dirname "$0")/.."

step() { printf '\n== %s\n' "$1"; }

miri() {
    step "miri"
    # The systems' worker threads live as long as the process, which Miri counts as a leak;
    # the flag turns that check off (and with it the check for leaked memory).
    MIRIFLAGS="${MIRIFLAGS:-} -Zmiri-ignore-leaks" cargo +nightly miri test --lib -- ecs::
}

if [ "${1:-}" = "--miri-only" ]; then
    miri
    exit 0
fi

# A workflow file that doesn't parse fails without running anything, so catch it here.
if command -v actionlint >/dev/null 2>&1; then
    step "workflows"
    actionlint
fi

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
    # The Haskell is kept free of hlint hints (CI has hlint; locally it runs if installed).
    if command -v hlint >/dev/null 2>&1; then
        step "hlint"
        hlint bindings plugins
    fi
elif [ "${MIRA_REQUIRE_GHC:-0}" = 1 ]; then
    echo "GHC is required (MIRA_REQUIRE_GHC=1) but not installed" >&2
    exit 1
else
    echo "GHC not installed: the Haskell plugins were not built"
fi

if [ "${1:-}" = "--miri" ]; then
    miri
fi

printf '\nAll checks passed.\n'
