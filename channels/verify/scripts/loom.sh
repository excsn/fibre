#!/usr/bin/env bash
#
# Loom model checks (channels/verify/loom/tests, one file per channel).
#
# Usage:
#   channels/verify/scripts/loom.sh                                  # every model, 2 preemptions
#   channels/verify/scripts/loom.sh mpsc_bounded                     # one channel's models
#   channels/verify/scripts/loom.sh mpsc_bounded two_items 3         # name filter, deeper bound
#   PREEMPT=3 verify/scripts/loom.sh                        # env override
#   LOOM_MAX_BRANCHES=200000 verify/scripts/loom.sh         # env override
#
# The default branch budget is raised to 1_000_000: loom's 1_000 aborts
# legitimate channel models before they reach deep interleavings. Preemptions
# stay bounded, so this lifts the ceiling without weakening exploration.
#
# Heavy runs can be checkpointed across Ctrl-C:
#   LOOM_CHECKPOINT_FILE=/tmp/loom.json LOOM_CHECKPOINT_INTERVAL=100000 \
#     channels/verify/scripts/loom.sh mpsc_bounded
#
# Files named `<channel>_advisory.rs` hold models that need SeqCst load/store
# semantics loom does not implement. They run after the channel's other models;
# a failure prints `ADVISORY FAILED` and leaves the exit status alone.
#
set -euo pipefail

cd "$(dirname "$0")/../../.."

FILE="${1:-}"
FILTER="${2:-}"
PREEMPT="${3:-${PREEMPT:-2}}"
BRANCHES="${LOOM_MAX_BRANCHES:-1000000}"

echo ">>> loom  file='${FILE:-all}'  filter='${FILTER}'  max_preemptions=${PREEMPT}  max_branches=${BRANCHES}"
echo

export LOOM_MAX_PREEMPTIONS="${PREEMPT}" LOOM_MAX_BRANCHES="${BRANCHES}"
export RUSTFLAGS="--cfg loom" CARGO_TARGET_DIR=target/loom

TESTS=()
ADVISORY=()
for f in channels/verify/loom/tests/*.rs; do
  name="$(basename "$f" .rs)"
  if [[ "$name" == *_advisory ]]; then
    [[ -z "$FILE" || "$FILE" == "$name" || "${FILE}_advisory" == "$name" ]] && ADVISORY+=("$name")
  elif [[ -z "$FILE" || "$FILE" == "$name" ]]; then
    TESTS+=(--test "$name")
  fi
done

status=0
if ((${#TESTS[@]})); then
  cargo test -p fibre_loom --release --no-fail-fast "${TESTS[@]}" -- ${FILTER:+"${FILTER}"} || status=$?
fi

for name in ${ADVISORY[@]+"${ADVISORY[@]}"}; do
  echo
  echo ">>> advisory  ${name}  (failures reported, not counted; see channels/verify/MODEL.md)"
  if ! cargo test -p fibre_loom --release --no-fail-fast --test "${name}" -- ${FILTER:+"${FILTER}"}; then
    echo ">>> ADVISORY FAILED: ${name}"
  fi
done

exit "$status"
