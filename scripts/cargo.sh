#!/usr/bin/env bash
# cargo on the workspace with rustup's cargo, so rust-toolchain.toml picks the toolchain. A
# distribution cargo on PATH ignores that file and is too old for the edition.
set -eu
home="${CARGO_HOME:-$HOME/.cargo}"
cargo="${CARGO:-}"
if [ -z "$cargo" ]; then
  if [ -x "$home/bin/cargo" ]; then cargo="$home/bin/cargo"; else cargo=cargo; fi
fi
export PATH="$home/bin:$PATH"
cd "$(dirname "$0")/.."
exec "$cargo" "$@"
