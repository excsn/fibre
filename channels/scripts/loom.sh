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
#   channels/scripts/loom.sh spsc_bounded             # one channel's models
#   channels/scripts/loom.sh spsc_bounded::two_items 3  # one test, deeper bound
#   PREEMPT=3 channels/scripts/loom.sh                # env override
#
# The channel arg is scoped under the `loom_tests` module automatically, so a
# short name like `spsc_bounded` becomes the filter `loom_tests::spsc_bounded`.
# This matters: libtest filters are SUBSTRING matches, so a bare `spsc` would
# also match every real `spsc::bounded_sync::tests::*` unit test and run THEM
# under `--cfg loom` — where they panic building loom primitives outside a
# `loom::model`. Anchoring to `loom_tests::` makes that impossible regardless
# of how a channel's real modules are named. Pass an already-qualified
# `loom_tests::...` filter and it's used verbatim.
#
# Heavy runs can be checkpointed across Ctrl-C:
#   LOOM_CHECKPOINT_FILE=/tmp/loom.json LOOM_CHECKPOINT_INTERVAL=100000 \
#     channels/scripts/loom.sh spsc_bounded
#
set -euo pipefail

cd "$(dirname "$0")/../.."

ARG="${1:-}"
if [ -z "${ARG}" ]; then
  FILTER="loom_tests"
elif [ "${ARG#loom_tests}" != "${ARG}" ]; then
  FILTER="${ARG}"                 # already qualified (starts with loom_tests)
else
  FILTER="loom_tests::${ARG}"     # scope a short channel name under the module
fi
PREEMPT="${2:-${PREEMPT:-2}}"

echo ">>> loom  filter='${FILTER}'  max_preemptions=${PREEMPT}"
echo

LOOM_MAX_PREEMPTIONS="${PREEMPT}" \
RUSTFLAGS="--cfg loom" \
  cargo test -p fibre --lib --release "${FILTER}"
