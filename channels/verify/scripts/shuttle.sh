#!/usr/bin/env bash
#
# Shuttle models (channels/verify/shuttle/tests), release build under --cfg shuttle.
# Uses its own target dir so the cfg switch does not invalidate normal builds.
#
# Usage:
#   channels/verify/scripts/shuttle.sh                     # every model
#   channels/verify/scripts/shuttle.sh mpsc_bounded        # one channel's models
#   channels/verify/scripts/shuttle.sh mpsc_bounded pct    # name filter
#
set -euo pipefail

cd "$(dirname "$0")/../../.."

FILE="${1:-}"
FILTER="${2:-}"
echo ">>> shuttle  file='${FILE:-all}'  filter='${FILTER}'"
echo
RUSTFLAGS="--cfg shuttle" CARGO_TARGET_DIR=target/shuttle \
  cargo test -p fibre_shuttle --release ${FILE:+--test "${FILE}"} -- ${FILTER:+"${FILTER}"}
