#!/usr/bin/env bash
#
# Loom model checks for migrated channels (src/loom_tests/, one file per
# channel; primitive switch in src/internal/sync.rs).
#
# Loom explores every interleaving (including spurious compare_exchange_weak
# failures), so it catches lost wakeups / protocol bugs that sampling-based
# miri can miss. It's exponential — keep preemptions bounded (2–3 catches
# essentially everything) and keep models tiny.
#
# Usage:
#   channels/scripts/loom.sh                          # all loom_tests, 2 preemptions
#   channels/scripts/loom.sh mpsc_bounded             # one channel's models
#   channels/scripts/loom.sh mpsc_bounded::spsc 3     # one test, deeper bound
#   PREEMPT=3 channels/scripts/loom.sh                # env override
#
# Heavy runs can be checkpointed across Ctrl-C:
#   LOOM_CHECKPOINT_FILE=/tmp/loom.json LOOM_CHECKPOINT_INTERVAL=100000 \
#     channels/scripts/loom.sh mpsc_bounded
#
set -euo pipefail

cd "$(dirname "$0")/../.."

FILTER="${1:-loom_tests}"
PREEMPT="${2:-${PREEMPT:-2}}"

echo ">>> loom  filter='${FILTER}'  max_preemptions=${PREEMPT}"
echo

LOOM_MAX_PREEMPTIONS="${PREEMPT}" \
RUSTFLAGS="--cfg loom" \
  cargo test -p fibre --lib --release "${FILTER}"
