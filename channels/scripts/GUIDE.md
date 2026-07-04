# channels/scripts — validation & profiling wrappers

Thin wrappers around the verbose loom / miri / criterion invocations, so the
common runs are one line. All `cd` to the workspace root, so they work from
anywhere. Run them from the repo:

```sh
channels/scripts/loom.sh mpsc_bounded
channels/scripts/miri.sh mpsc_bounded
channels/scripts/bench.sh mpsc_bounded 'Cap-128'
```

## `loom.sh [test-filter] [max-preemptions]`

Exhaustive interleaving model-check of the migrated channels. Loom explores
every schedule (including spurious `compare_exchange_weak` failures), so it
catches lost wakeups / protocol bugs that sampling (miri) can miss; miri stays
the UB/UAF/payload-race detector — they're complements, run both.

```sh
channels/scripts/loom.sh                       # all loom_tests, 2 preemptions
channels/scripts/loom.sh mpsc_bounded          # one channel's models
channels/scripts/loom.sh mpsc_bounded::spsc 3  # one test, deeper bound
PREEMPT=3 channels/scripts/loom.sh             # env override
```

Loom is exponential — bound preemptions (default 2; 2–3 catches essentially
everything) and keep models tiny (cap 1–2, ≤2 spawned threads, 1–2 items).
Heavy runs can be checkpointed across Ctrl-C with `LOOM_CHECKPOINT_FILE=...`.

Architecture (`cfg(loom)` lives in exactly two source sites + Cargo.toml):

- `src/internal/sync.rs` — primitive facade: selector over cfg-free `real.rs`
  (std/parking_lot re-exports, zero cost) and `mocked.rs` (loom types + shims;
  panicking `sleep`/`park_timeout` since loom has no time model).
- `src/loom_tests/` — one file per migrated channel, driving the public API;
  gated once in `lib.rs`.
- Migrating a channel = swapping its sync imports to `crate::internal::sync`
  (atomics left on std only lose coverage, but anything BLOCKING — Mutex,
  park — must be migrated or the model deadlocks for real) + adding its
  `loom_tests/<channel>.rs`. Payload `UnsafeCell`s stay std; payload races are
  miri's job.

Migrated so far: **mpsc bounded**. Pending: spsc, mpsc unbounded, mpmc_v2
(bounded/unbounded/rendezvous), spmc, oneshot, topic.

## `miri.sh <test-file> [seed-range] [name-filter]`

Runs a `fibre_miri` test under miri with `-Zmiri-many-seeds`. Miri is the
UB/race/UAF detector; many-seeds is mandatory for weak-memory bugs (a single run
routinely passes with an ordering bug present). See `../miri/GUIDE.md` for the
suite. Run BOTH the isolated test and the full file — isolating changes the
global schedule, so a bug can hide in one framing and show in the other.

```sh
channels/scripts/miri.sh mpsc_bounded                 # whole file, 0..64
channels/scripts/miri.sh mpsc_bounded 0..256          # custom range
channels/scripts/miri.sh mpsc_bounded 0..256 batch_paths_recycle_chunks
```

## `bench.sh <bench-name> [criterion-filter] [extra criterion args...]`

Criterion filter wrapper. **Baseline caveat:** an isolated filtered run
overwrites only the baselines it touches, so a later run can report "no change"
against a stale/partial baseline — for a real A/B, save a clean baseline first:

```sh
channels/scripts/bench.sh mpsc_bounded 'MpscBoundedSync/Cap-128'
# clean A/B vs a commit:
git checkout <baseline-commit>
channels/scripts/bench.sh mpsc_bounded '' --save-baseline pre
git checkout -
channels/scripts/bench.sh mpsc_bounded '' --baseline pre
```

For profiling (flamegraph, single long run) use `fibre_bench` instead — see
`../bench/GUIDE.md`.
