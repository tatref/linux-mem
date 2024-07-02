#!/bin/bash

set -eu

RUSTFLAGS="-C target-cpu=x86-64-v2" cargo +nightly-2024-03-10 zigbuild --release --target x86_64-unknown-linux-gnu.2.12
