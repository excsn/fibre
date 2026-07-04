#!/usr/bin/env bash
#
# Run fibre_cache lib unit tests under miri with many-seeds interleaving
# exploration. Default filter targets the sync primitives (HybridMutex /
# HybridRwLock), whose tests live in the lib (src/sync/*), not a separate
# test crate — miri-only lifecycle tests in src/sync/miri_tests.rs are
# compiled in automatically under miri.
#
# Miri is a UB / data-race / UAF / leak detector; `-Zmiri-many-seeds` re-runs
# each test under many schedules (essential for weak-memory bugs — a default
# single run routinely passes with an ordering bug present).
#
# The #[ignore]d starvation proof test is sleep/timing based and stays
# excluded here; do not pass --ignored under miri.
#
# Usage:
#   cache/scripts/miri.sh                        # sync:: tests, seeds 0..64
#   cache/scripts/miri.sh 0..256                 # custom seed range
#   cache/scripts/miri.sh 0..256 sync::rwlock    # narrower name filter
#   cache/scripts/miri.sh 0..64 ''               # whole lib (all unit tests)
#
set -euo pipefail

cd "$(dirname "$0")/../.."

SEEDS="${1:-0..64}"
FILTER="${2-sync::}"

echo ">>> miri  -p fibre_cache --lib  many-seeds=${SEEDS}  filter='${FILTER}'"
echo

MIRIFLAGS="-Zmiri-many-seeds=${SEEDS} ${MIRIFLAGS:-}" \
  cargo +nightly miri test -p fibre_cache --lib ${FILTER:+"${FILTER}"}
