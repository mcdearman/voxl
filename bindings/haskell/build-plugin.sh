#!/bin/sh
# Builds a Haskell voxl plugin: one module that exports `voxl_hs_main`, linked with the Voxl
# bindings and their C glue into a shared library.
#
#     build-plugin.sh <name> <Module.hs> <output directory>
#
# -dynamic      share one GHC runtime between every Haskell plugin and every reloaded version
# -flink-rts    make the library say which runtime it needs, since the engine isn't Haskell
# -threaded     the threaded runtime, so the non-moving collector (turned on in voxl_hs.c) can
#               mark the old generation on its own thread instead of pausing the game
set -eu
here=$(cd "$(dirname "$0")" && pwd)
name=$1
source=$2
out=$3
case "$(uname -s)" in
    Darwin) lib="lib$name.dylib" ;;
    *) lib="lib$name.so" ;;
esac
work="$out/haskell/$name"
mkdir -p "$work"
# GHC writes a C file's object beside its source, so compile a private copy of the glue:
# two plugins building at once would otherwise trip over each other.
cp "$here/cbits/voxl_hs.c" "$work/voxl_hs.c"
ghc -O1 -shared -dynamic -threaded -fPIC -flink-rts -Wall -Wno-missing-signatures \
    -I"$here/../../include" -i"$here" -outputdir "$work" \
    "$work/voxl_hs.c" "$source" -o "$work/$lib" >"$work/build.log" 2>&1 || {
    cat "$work/build.log" >&2
    exit 1
}
# Move it into place in one step, so the engine never loads a half-written library.
mv "$work/$lib" "$out/$lib"
echo "built $out/$lib"
