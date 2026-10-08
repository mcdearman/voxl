#!/bin/sh
# Builds the Haskell example plugin into Cargo's output directory, where the `plugins` example
# looks for it. Needs GHC. Usage: plugins/swirl/build.sh [debug|release]
set -eu
root=$(cd "$(dirname "$0")/../.." && pwd)
exec "$root/bindings/haskell/build-plugin.sh" swirl "$root/plugins/swirl/Swirl.hs" "$root/target/${1:-debug}"
