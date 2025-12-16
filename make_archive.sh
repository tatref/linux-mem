#!/bin/bash
#
# Usage
# Curent git commit
# ./make_archive.sh
# 
# Release
# ./make_archive.sh 1.0
#

set -euo pipefail

unset LANG


COMMIT="$(git rev-parse --short HEAD)"
DATE="$(date +%F)"
VERSION="${1-$COMMIT}"

SHORT_VERSION="$VERSION"
LONG_VERSION="$VERSION $DATE"

cargo clean
cargo update
RUSTFLAGS="-C target-cpu=x86-64-v2" cargo +nightly zigbuild --release --target x86_64-unknown-linux-gnu.2.17

cross b --release --target x86_64-pc-windows-gnu --bin kpageflags-viewer

echo "$LONG_VERSION" > VERSION

ARCHIVE="linux-mem-$SHORT_VERSION.tar.xz"
tar -cJf "$ARCHIVE" \
  --transform 's:target/x86_64-unknown-linux-gnu/release/::' \
  --transform 's:target/x86_64-pc-windows-gnu/release/::' \
  --transform "s:^:linux-mem-$SHORT_VERSION/:" \
  README.md VERSION \
  target/x86_64-unknown-linux-gnu/release/{memstats,procinfo,shmem,kpageflags-viewer} \
  target/x86_64-pc-windows-gnu/release/kpageflags-viewer.exe \
  proc_snap/snap.py

echo Done
ls -lh "$ARCHIVE"


