#!/bin/sh
# Packages a release build: the plugin host, and everything needed to write plugins for it
# without building the engine (the C header, the Haskell and Rust bindings, the examples).
#
#     scripts/package.sh <version> <target>       after `cargo build --release --example host`
set -eu
cd "$(dirname "$0")/.."
version=$1
target=$2
name="voxl-$version-$target"
stage="dist/$name"

rm -rf "$stage"
mkdir -p "$stage/bin" "$stage/crates"
cp target/release/examples/host "$stage/bin/voxl-host"
cp -R include bindings plugins "$stage/"
cp -R crates/voxl_plugin "$stage/crates/"
cp README.md LICENSE docs/PLUGINS.md "$stage/"
# The example plugins expect to be built from a checkout; say so rather than ship scripts
# that point at paths the package doesn't have.
find "$stage/plugins" -name build.sh -delete

tar -czf "dist/$name.tar.gz" -C dist "$name"
rm -rf "$stage"
echo "packaged dist/$name.tar.gz"
