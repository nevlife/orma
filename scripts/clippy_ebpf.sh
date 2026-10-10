#!/usr/bin/env bash
# clippy on the eBPF crate, which only builds for the BPF target with the pinned nightly.
set -eu
toolchain=$(sed -n 's/^const EBPF_TOOLCHAIN: &str = "\(.*\)";$/\1/p' "$(dirname "$0")/../daemon/armo_daemon/build.rs")
exec "$(dirname "$0")/cargo.sh" "+$toolchain" clippy -p armo-ebpf --release \
  --target bpfel-unknown-none -Z build-std=core -- -D warnings
