#!/usr/bin/env bash
#
# Shuttle models of the connection driver (src/conn.rs, mod model), release build under --cfg shuttle.
# Uses its own target dir so the cfg switch does not invalidate normal builds.
#
# Usage:
#   http/verify/shuttle.sh            # every model
#   http/verify/shuttle.sh dropped    # name filter
#
set -euo pipefail

cd "$(dirname "$0")/../.."

FILTER="${1:-}"
echo ">>> fibre_http shuttle  filter='${FILTER}'"
RUSTFLAGS="--cfg shuttle" CARGO_TARGET_DIR=target/shuttle \
  cargo test -p fibre_http --release --lib model:: -- ${FILTER:+"${FILTER}"}
