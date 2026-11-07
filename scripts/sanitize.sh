#!/usr/bin/env bash
# The Rust crates' tests under a sanitizer: nightly -Zsanitizer with std rebuilt the same way.
#   scripts/sanitize.sh address|thread [extra cargo test arguments]
set -eu
cd "$(dirname "$0")/.."
kind=${1:?address or thread}
shift
toolchain=nightly-2026-10-05
host=$(rustc "+$toolchain" -vV | sed -n 's/^host: //p')
# The test harness unwinds; a profile that aborts would build std both ways.
export CARGO_PROFILE_DEV_PANIC=unwind
export RUSTFLAGS="-Zsanitizer=$kind"
export RUSTDOCFLAGS="$RUSTFLAGS"
export TSAN_OPTIONS="halt_on_error=1:second_deadlock_stack=1:suppressions=$PWD/scripts/sanitize/tsan.supp"
# Left out: a test that bounds how late a recording ends, which instrumentation slows past its bound.
exec cargo "+$toolchain" test -Zbuild-std --target "$host" --target-dir "${SANITIZE_TARGET:-target}/$kind" \
  --locked --no-fail-fast -p orma-common -p orma-proto -p orma-collectd -p ormad --lib --tests "$@" \
  -- --skip duration_and_size_limits_end_recordings
