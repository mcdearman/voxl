#!/bin/sh
# Builds the Haskell game plugin into Cargo's output directory. Needs GHC.
# Usage: plugins/chase/build.sh [debug|release]
set -eu
root=$(cd "$(dirname "$0")/../.." && pwd)
exec "$root/bindings/haskell/build-plugin.sh" chase "$root/plugins/chase/Chase.hs" "$root/target/${1:-debug}"
