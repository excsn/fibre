#!/usr/bin/env bash
#
# Run a criterion bench with an optional filter.
#
# Criterion compares against a saved baseline (previous run). WATCH OUT: an
# isolated filtered run overwrites only the baselines it touches, so a later run
# can report "no change" against a stale/partial baseline. When a comparison
# matters, save a clean baseline first (see --save-baseline / --baseline below).
#
# Usage:
#   channels/scripts/bench.sh mpsc_bounded                          # whole bench
#   channels/scripts/bench.sh mpsc_bounded 'MpscBoundedSync/Cap-128'
#   channels/scripts/bench.sh mpsc_bounded 'Cap-128|Cap-4'          # regex filter
#
# Clean A/B against a known state:
#   git checkout <baseline-commit>
#   channels/scripts/bench.sh mpsc_bounded '' --save-baseline pre
#   git checkout -
#   channels/scripts/bench.sh mpsc_bounded '' --baseline pre
#
set -euo pipefail

cd "$(dirname "$0")/../.."

BENCH="${1:?usage: bench.sh <bench-name> [criterion-filter] [extra criterion args...]}"
FILTER="${2:-}"
shift || true
shift || true  # drop $1,$2; remaining "$@" are extra criterion args

echo ">>> bench --bench ${BENCH}  filter='${FILTER}'  extra='$*'"
echo

cargo bench -p fibre --bench "${BENCH}" -- "${FILTER}" "$@"
