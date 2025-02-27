#!/bin/bash

set -eu

RUSTFLAGS="-C target-cpu=x86-64-v2" cargo +nightly zigbuild --release --target x86_64-unknown-linux-gnu.2.17
