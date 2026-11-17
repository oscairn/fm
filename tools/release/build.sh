#!/bin/sh
# SPDX-License-Identifier: AGPL-3.0-or-later
# Build one platform's single-file .clap (the fm-clap cdylib, renamed) from a clean release build.
#
#   build.sh <platform> <outdir>      e.g. build.sh macos-arm64 dist
#
# Writes <outdir>/<NAME>-<platform>.clap; the cargo target dir goes under <outdir>/build-<platform>.
# Must run on a machine of the platform's OS (each release job runs on its own runner).
set -eu
cd "$(dirname "$0")/../.."
. tools/release/release.conf

platform=${1:?usage: build.sh <platform> <outdir>}
out=${2:?usage: build.sh <platform> <outdir>}

case $platform in
  macos-*) want=Darwin ;;
  linux-*) want=Linux ;;
  windows-*) want=Windows ;;
  *) echo "build.sh: unknown platform '$platform' (macos-*, linux-* or windows-*)" >&2; exit 2 ;;
esac
os=$(uname -s)
case $os in MINGW*|MSYS*|CYGWIN*) os=Windows ;; esac
[ "$os" = "$want" ] || {
    echo "build.sh: platform '$platform' needs $want, this is $os; build each platform on its own runner" >&2
    exit 1; }

mkdir -p "$out"
build=$out/build-$platform
rm -rf "$build"
cargo build --locked --release -p "$CARGO_PACKAGE" --target-dir "$build"
case $want in
  Darwin) lib=lib$CARGO_LIB.dylib ;;
  Linux) lib=lib$CARGO_LIB.so ;;
  Windows) lib=$CARGO_LIB.dll ;;
esac
clap=$build/release/$lib
cp "$clap" "$out/$NAME-$platform.clap"
echo "build.sh: built $out/$NAME-$platform.clap"
