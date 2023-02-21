#!/bin/bash
#
# Compile using a container for EL7
#
# Usage:
# ./build.sh <cargo command>
# ./build.sh cargo b --release --bin groupstats
#
# Creates 2 volumes for temp storage: CARGO_HOME AND SCCACHE_DIR
#

os=el7

mkdir -p $PWD/target.$os

podman run \
	--rm \
	-v $PWD/target.$os:/target \
	-v $PWD:/src \
	-v CARGO_HOME:/cargo_home \
	-v SCCACHE_DIR:/sccache \
	-e CARGO_TARGET_DIR=/target \
	-e CARGO_HOME=/cargo_home \
	-e RUSTC_WRAPPER=sccache \
	-e SCCACHE_DIR=/sccache \
	rust:$os \
	$@
