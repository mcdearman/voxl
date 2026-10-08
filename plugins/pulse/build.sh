#!/bin/sh
# Builds the C example plugin into Cargo's output directory, where the `plugins` example
# looks for it. Usage: plugins/pulse/build.sh [debug|release]
set -eu
root=$(cd "$(dirname "$0")/../.." && pwd)
out="$root/target/${1:-debug}"
case "$(uname -s)" in
    Darwin) lib=libpulse.dylib ;;
    *) lib=libpulse.so ;;
esac
mkdir -p "$out"
# Compile beside the target and rename, so the engine never sees a half-written library.
cc -shared -fPIC -std=c11 -O2 -Wall -I "$root/include" -o "$out/$lib.tmp" "$root/plugins/pulse/pulse.c" -lm
mv "$out/$lib.tmp" "$out/$lib"
echo "built $out/$lib"
