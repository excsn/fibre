#!/usr/bin/env bash
#
# Real-hardware stress gates (channels/verify/stress/tests), release build.
#
# Usage:
#   channels/verify/scripts/stress.sh                    # every gate
#   channels/verify/scripts/stress.sh mpsc_bounded       # one channel's gates
#
set -euo pipefail

cd "$(dirname "$0")/../../.."

FILE="${1:-}"
echo ">>> stress  file='${FILE:-all}'"
echo
cargo test -p fibre_stress --release ${FILE:+--test "${FILE}"} -- --test-threads=1
