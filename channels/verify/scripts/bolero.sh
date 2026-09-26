#!/usr/bin/env bash
#
# Operation-sequence properties (channels/verify/bolero/tests) as property tests.
# For fuzzing a single target instead: cargo bolero test -p fibre_bolero <target>
#
# Usage:
#   channels/verify/scripts/bolero.sh                  # every property
#   channels/verify/scripts/bolero.sh mpsc_bounded     # one channel's properties
#   FIBRE_BOLERO_ITERATIONS=100000 channels/verify/scripts/bolero.sh   # inputs per property (default 10000)
#
set -euo pipefail

cd "$(dirname "$0")/../../.."

FILE="${1:-}"
echo ">>> bolero  file='${FILE:-all}'"
echo
cargo test -p fibre_bolero --release ${FILE:+--test "${FILE}"}
