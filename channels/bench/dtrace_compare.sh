#!/usr/bin/env bash
# Compares fibre_bench SPSC sync vs async under dtrace: counts calls into SpscShared's
# register/wake_one (shared by both), plus the sync-specific real OS park/unpark and the
# async-specific Future poll counts. (Tokio's own wake_by_val/schedule_task were tried and
# dropped - confirmed via `nm` to be fully inlined into Harness::poll in this build, no
# standalone symbol to probe.)
#
# batch=0 (single-item send/recv) uses different async symbols than batch>0
# (send_batch/recv_batch) - handled automatically below.
#
# Usage: ./dtrace_compare.sh [capacity] [batch] [items]
# Pass batch=0 for plain send()/recv() baseline; defaults otherwise match the
# Cap-1024/Batch-64 case investigated this session.
#
# Requires sudo (dtrace needs elevated privileges on macOS). The build step runs as
# the invoking user; only the `dtrace` calls themselves are escalated.

set -euo pipefail

CAPACITY="${1:-1024}"
BATCH="${2:-64}"
ITEMS="${3:-50000000}"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
BIN="$WORKSPACE_ROOT/target/release/fibre_bench"

echo "=== Building fibre_bench (release) ==="
(cd "$WORKSPACE_ROOT" && cargo build --release -p fibre_bench)

WORKDIR="$(mktemp -d)"
trap 'rm -rf "$WORKDIR"' EXIT

SYNC_D="$WORKDIR/spsc_sync.d"
ASYNC_D="$WORKDIR/spsc_async.d"

cat > "$SYNC_D" <<'EOF'
pid$target::*register*:entry   { @c[probefunc] = count(); }
pid$target::*wake_one*:entry   { @c[probefunc] = count(); }
pid$target::*park*:entry       { @c[probefunc] = count(); }
pid$target::*unpark*:entry     { @c[probefunc] = count(); }
EOF

if [ "$BATCH" = "0" ]; then
  # Single-item mode (fibre_bench -b 0): SendFuture/ReceiveFuture, not the batch Futures.
  # ReceiveFuture::poll itself gets fully inlined in this build (confirmed via `nm` - no
  # standalone symbol), but it just delegates to SpscShared::poll_recv_internal, which does
  # have its own symbol, so we probe that instead for the recv-side equivalent.
  cat > "$ASYNC_D" <<'EOF'
pid$target::*register*:entry                          { @c[probefunc] = count(); }
pid$target::*wake_one*:entry                          { @c[probefunc] = count(); }
pid$target::*spsc*bounded_async*SendFuture*poll*:entry { @c[probefunc] = count(); }
pid$target::*poll_recv_internal*:entry                 { @c[probefunc] = count(); }
EOF
else
  cat > "$ASYNC_D" <<'EOF'
pid$target::*register*:entry                       { @c[probefunc] = count(); }
pid$target::*wake_one*:entry                       { @c[probefunc] = count(); }
pid$target::*spsc*bounded_async*SendBatchFuture*poll*:entry { @c[probefunc] = count(); }
pid$target::*spsc*bounded_async*RecvBatchFuture*poll*:entry { @c[probefunc] = count(); }
EOF
fi

# -Z: don't abort if a probe clause matches zero probes (only relevant while still
# narrowing down real symbol names - everything above is confirmed to exist via `nm`).
echo
echo "=== SYNC - Cap-$CAPACITY / Batch-$BATCH / Items-$ITEMS ==="
sudo dtrace -Z -s "$SYNC_D" -c "$BIN -c spsc -m sync -C $CAPACITY -b $BATCH -i $ITEMS"

echo
echo "=== ASYNC - Cap-$CAPACITY / Batch-$BATCH / Items-$ITEMS ==="
sudo dtrace -Z -s "$ASYNC_D" -c "$BIN -c spsc -m async -C $CAPACITY -b $BATCH -i $ITEMS"

echo
echo "=== Done. Compare the two @c aggregations above. ==="
