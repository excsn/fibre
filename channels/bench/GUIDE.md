# fibre_bench - throughput & profiling harness

`fibre_bench` is a small CLI, separate from the Criterion suites in `channels/benches/`. Criterion
runs many short, sampled iterations with its own statistics machinery - great for tracking
regressions, bad for profiling, since the sampling overhead and short iteration length pollute a
flamegraph. `fibre_bench` instead runs **one long, fixed-size workload start to finish**, with a
stall watchdog, so it's meant to be run directly under a profiler:

```sh
cargo flamegraph -p fibre_bench -- -c spsc -m sync -C 128 -i 1000000 -b 8
```

It supports every channel family in the crate (`spsc`, `spmc`, `mpsc`, `mpmc-v2`, `mpmc-v3`),
sync and async, bounded/unbounded/rendezvous (where applicable), and single-item or batched
(`send_batch`/`recv_batch`) sends.

## Building & running

From the workspace root:

```sh
cargo build --release -p fibre_bench
cargo run --release -p fibre_bench -- <flags>
```

or from `channels/bench/`:

```sh
cargo run --release -- <flags>
```

Always use `--release` for real numbers - the workspace's `[profile.release]` keeps debug symbols
(`debug = true`) specifically so release builds stay flamegraph-friendly.

There's also a `diagnostics` feature (`cargo run --release --features diagnostics -- ...`) that
enables `fibre/diagnostics`; a stall report is printed via `fibre::telemetry::print_stall_report()`
at the end of every run regardless (and immediately, if the watchdog detects a stall).

## Flags

| Flag | Short | Default | Meaning |
|---|---|---|---|
| `--channel` | `-c` | `mpmc-v3` | Channel family: `mpmc-v3`, `mpmc-v2`, `mpsc`, `spmc`, `spsc` |
| `--mode` | `-m` | `sync` | `sync` or `async` |
| `--flavor` | `-f` | `bounded` | `bounded`, `unbounded`, or `rendezvous` |
| `--producers` | `-p` | `1` | Number of producer threads/tasks |
| `--consumers` | `-n` | `1` | Number of consumer threads/tasks |
| `--capacity` | `-C` | `128` | Channel capacity (bounded only) |
| `--items` | `-i` | `1000000` | Total items to send |
| `--batch-size` | `-b` | `0` | Batch size for `send_batch`/`recv_batch`; `0` = single-item `send`/`recv` |
| `--stall-ms` | | `500` | Watchdog poll interval, in ms |
| `--stall-count` | | `4` | Consecutive stalled ticks before the watchdog aborts (deadlock detection) |

The watchdog prints a `[watchdog] sent=.. recv=.. in_flight=..` line every `stall_ms` while
running. If `sent`/`received` don't move for `stall_count` consecutive ticks, it aborts (exit code
2) and prints a best-effort classification (lost wakeup / lost item(s) / mutual deadlock) - full
detail (ring length, waiter counts) is only available for `mpmc-v3` sync, which exposes
`debug_state`.

## Results file

Every run also appends one JSON line to `channels/bench/results.jsonl` (always on, no flag needed)
- config, the full watchdog tick history, and the final summary, so a session's worth of runs can
be reviewed directly from the file instead of needing terminal output pasted in. Not committed
(gitignored). Shape of one line:

```json
{"ts":1751378653,"channel":"spsc","mode":"sync","flavor":"bounded","capacity":1024,"producers":1,"consumers":1,"items":700000000,"batch_size":8,"stall_ms":500,"stall_count":4,"duration_ms":14523.891,"sent":700000000,"received":700000000,"throughput_m_per_sec":48.2,"ticks":[{"elapsed_ms":500,"sent":24123456,"received":24000000,"in_flight":123456,"ring_len":null,"send_waiters":null,"recv_waiters":null}]}
```

`ring_len`/`send_waiters`/`recv_waiters` are `null` except for `mpmc-v3` sync (the only handle with
`debug_state`).

## Channel constraints

- **`spsc`**: `--producers 1 --consumers 1` (enforced), `bounded` only.
- **`spmc`**: `--producers 1` (enforced), `bounded` only. It's a broadcast channel - every
  consumer receives every item, so `received` in the output is `items * consumers`; the reported
  throughput uses `sent` (dispatch rate), not `received`, so it's comparable to the other channel
  types.
- **`mpsc`**: `--consumers 1` (enforced), no `rendezvous`.
- **`mpmc-v2`** / **`mpmc-v3`**: any producer/consumer count, all three flavors. `mpmc-v3` only has
  its own implementation for `bounded`; `unbounded`/`rendezvous` under `-c mpmc-v3` silently
  delegate to `mpmc-v2`'s implementation.
- **`rendezvous` has no batch API** - `-b` > 0 with `-f rendezvous` is rejected at startup
  regardless of channel.

## Example commands

**SPSC** (single producer, single consumer):
```sh
# single-item, sync
cargo run --release -p fibre_bench -- -c spsc -m sync -C 1024 -i 1000000
# single-item, async
cargo run --release -p fibre_bench -- -c spsc -m async -C 1024 -i 1000000
# batched, sync - batch size 8
cargo run --release -p fibre_bench -- -c spsc -m sync -C 1024 -i 1000000 -b 8
# batched, async
cargo run --release -p fibre_bench -- -c spsc -m async -C 1024 -i 1000000 -b 8
```

**SPMC** (single producer, broadcast to N consumers):
```sh
cargo run --release -p fibre_bench -- -c spmc -m sync -n 4 -C 128 -i 1000000
cargo run --release -p fibre_bench -- -c spmc -m sync -n 4 -C 128 -i 1000000 -b 64
```

**MPSC** (N producers, single consumer):
```sh
cargo run --release -p fibre_bench -- -c mpsc -m sync -f bounded -p 4 -C 128 -i 1000000
cargo run --release -p fibre_bench -- -c mpsc -m async -f unbounded -p 4 -i 1000000 -b 32
```

**MPMC v2** (lock-based; bounded/unbounded/rendezvous):
```sh
cargo run --release -p fibre_bench -- -c mpmc-v2 -m sync -f bounded -p 4 -n 4 -C 128 -i 1000000
cargo run --release -p fibre_bench -- -c mpmc-v2 -m sync -f rendezvous -p 4 -n 4 -i 1000000
cargo run --release -p fibre_bench -- -c mpmc-v2 -m async -f bounded -p 4 -n 4 -C 128 -i 1000000 -b 16
```

**MPMC v3** (Vyukov ring + tombstone handoff; bounded only for its own code path):
```sh
cargo run --release -p fibre_bench -- -c mpmc-v3 -m sync -p 4 -n 4 -C 128 -i 1000000
cargo run --release -p fibre_bench -- -c mpmc-v3 -m async -p 4 -n 4 -C 128 -i 1000000 -b 32
```

## Profiling with `cargo flamegraph`

Use `-p fibre_bench`, not `--bin fibre_bench`.

**Pick `-i` for wall-clock time, not item count.** A sampling profiler needs the process to run
for several seconds to gather a useful number of samples - `-i 1000000` finishes in tens of
milliseconds for most of these combos, which produces flamegraphs with as few as 17-35 total
samples (checked: `total_samples="..."` in the SVG's `<svg id="frames" ...>` tag) - nowhere near
enough to see anything past `all`/`thread_start`. Back into `-i` from a rough throughput estimate
(criterion numbers, or a quick non-flamegraph `cargo run --release` first) so the run lasts
roughly 10-15s; that's enough for hundreds to low-thousands of samples with this binary.

The values below are confirmed (rerun and checked `total_samples` in the resulting SVG):

```sh
# SPSC sync, Cap-1024/Batch-8 - 754 samples
cargo flamegraph -p fibre_bench -o flame_cap1024_batch8_sync.svg   -- -c spsc -m sync  -C 1024 -b 8  -i 700000000

# SPSC sync, Cap-1024/Batch-64 - 673 samples
cargo flamegraph -p fibre_bench -o flame_cap1024_batch64_sync.svg  -- -c spsc -m sync  -C 1024 -b 64 -i 1000000000

# SPSC sync, Cap-128/Batch-8 - 860 samples
cargo flamegraph -p fibre_bench -o flame_cap128_batch8_sync.svg    -- -c spsc -m sync  -C 128  -b 8  -i 350000000

# SPSC async, Cap-1024/Batch-8 - 1375 samples
cargo flamegraph -p fibre_bench -o flame_cap1024_batch8_async.svg  -- -c spsc -m async -C 1024 -b 8  -i 1000000000

# SPSC sync, Cap-1024, single-item (no batch) - only 75 samples at 450M, needs more; try ~4-5x
cargo flamegraph -p fibre_bench -o flame_cap1024_single_sync.svg   -- -c spsc -m sync  -C 1024        -i 2000000000
```

`-o <name>.svg` keeps each run from clobbering the last (`cargo flamegraph` defaults to
`flamegraph.svg` in cwd otherwise).
