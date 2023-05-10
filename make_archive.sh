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

. token


COMMIT="$(git rev-parse --short HEAD)"
DATE="$(date +%F)"
VERSION="${1-$COMMIT}"

SHORT_VERSION="$VERSION"
LONG_VERSION="$VERSION $DATE"


cargo clean
RUSTFLAGS="-C target-cpu=x86-64-v2" cargo zigbuild --release --target x86_64-unknown-linux-gnu.2.12

echo "$LONG_VERSION" > VERSION

ARCHIVE="linux-mem-$SHORT_VERSION.tar.gz"
tar -cJf "$ARCHIVE" \
  --transform 's:target/x86_64-unknown-linux-gnu/release/::' \
  --transform "s:^:linux-mem-$SHORT_VERSION/:" \
  README.md VERSION \
  target/x86_64-unknown-linux-gnu/release/{memstats,procinfo,shmem} \
  proc_snap/snap.py

echo Done
ls -lh "$ARCHIVE"


