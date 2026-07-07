#!/usr/bin/env bash
#
# Run a fibre_miri test under miri with many-seeds interleaving exploration.
#
# Miri is a UB / data-race / UAF detector; `-Zmiri-many-seeds` re-runs each test
# under many schedules (essential for weak-memory bugs - a default single run
# routinely passes with an ordering bug present). Run BOTH an isolated test and
# the full file: isolating changes the global schedule, so a bug can hide in one
# framing and show in the other at the same seed number.
#
# Usage:
#   channels/scripts/miri.sh mpsc_bounded                 # whole file, seeds 0..64
#   channels/scripts/miri.sh mpsc_bounded 0..256          # custom seed range
#   channels/scripts/miri.sh mpsc_bounded 0..256 batch_paths_recycle_chunks
#
set -euo pipefail

cd "$(dirname "$0")/../.."

TEST="${1:?usage: miri.sh <test-file> [seed-range] [name-filter]}"
SEEDS="${2:-0..64}"
NAMEFILTER="${3:-}"

echo ">>> miri  --test ${TEST}  many-seeds=${SEEDS}  filter='${NAMEFILTER}'"
echo

MIRIFLAGS="-Zmiri-many-seeds=${SEEDS} ${MIRIFLAGS:-}" \
  cargo +nightly miri test -p fibre_miri --test "${TEST}" ${NAMEFILTER:+"${NAMEFILTER}"}
