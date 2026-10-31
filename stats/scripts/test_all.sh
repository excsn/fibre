#!/bin/sh
# Every feature and target combination fibre_stats claims. Linux is compiled
# here and run only where this script runs on Linux.
set -eu
cd "$(dirname "$0")/.."

step() {
  echo "==> $*"
  "$@"
}

step cargo test -p fibre_stats
step cargo test -p fibre_stats --all-features
step cargo check -p fibre_stats --target wasm32-unknown-unknown
step cargo check -p fibre_stats --target wasm32-unknown-unknown --features wasm-bindgen,high_res
step cargo check -p fibre_stats --target wasm32-unknown-unknown --features collect-process,collect-host,collect-tokio
if [ "$(uname -s)" != Linux ]; then
  step cargo check -p fibre_stats --all-features --tests --target x86_64-unknown-linux-gnu
fi
step cargo clippy -p fibre_stats --all-features --all-targets -- -D warnings
RUSTDOCFLAGS='-D warnings' step cargo doc -p fibre_stats --all-features --no-deps
