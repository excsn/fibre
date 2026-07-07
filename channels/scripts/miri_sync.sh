#!/usr/bin/env bash
#
# Run the `fibre::sync` primitive tests (HybridMutex / HybridRwLock) under miri
# with many-seeds interleaving exploration. Their unit tests live in the lib
# (src/sync/*), and the miri-only lifecycle tests in src/sync/miri_tests.rs are
# compiled in automatically under miri. (This is separate from `miri.sh`, which
# drives the channel protocol tests in the `fibre_miri` test crate.)
#
# Miri is a UB / data-race / UAF / leak detector; `-Zmiri-many-seeds` re-runs
# each test under many schedules (essential for weak-memory bugs - a default
# single run routinely passes with an ordering bug present).
#
# The #[ignore]d starvation proof test is sleep/timing based and stays
# excluded here; do not pass --ignored under miri.
#
# Usage:
#   channels/scripts/miri_sync.sh                        # sync:: tests, seeds 0..64
#   channels/scripts/miri_sync.sh 0..256                 # custom seed range
#   channels/scripts/miri_sync.sh 0..256 sync::rwlock    # narrower name filter
#   channels/scripts/miri_sync.sh 0..64 ''               # whole lib (all unit tests)
#
set -euo pipefail

cd "$(dirname "$0")/../.."

SEEDS="${1:-0..64}"
FILTER="${2-sync::}"

echo ">>> miri  -p fibre --lib  many-seeds=${SEEDS}  filter='${FILTER}'"
echo

MIRIFLAGS="-Zmiri-many-seeds=${SEEDS} ${MIRIFLAGS:-}" \
  cargo +nightly miri test -p fibre --lib ${FILTER:+"${FILTER}"}
