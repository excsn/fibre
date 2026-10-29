# cache_arena

Cross-implementation cache benchmarks: fibre_cache against moka, quick_cache and stretto through one fixed-duration workload harness.

Results live in [docs/](./docs/).

## Running

```sh
cd cache/arena
cargo run --release                                              # ten default cells, every engine, 10 s each
cargo run --release -- --list                                    # print the cells, run nothing
cargo run --release -- --engine fibre_cache.s3fifo,quick_cache   # head-to-head
cargo run --release -- --cell "zipf async 64" --secs 30          # one cell, longer
cargo run --release -- --out results.txt                         # also append the raw lines to a file
cargo run --release -- --machine "MacBook M4 Pro (14 cores)"     # label the pages
cargo run --release -- --rerender docs/raw/results.tsv           # rewrite the pages, measure nothing
```

A run writes its pages to `docs/` (`--docs DIR` moves them): one Markdown page per op, an index with the run's machine state, the engines and the method, `raw/results.tsv` and `raw/results.json`. `docs/FINDINGS.md` is written by hand.

Each cell runs every engine in turn for `--secs` and reports how many operations it completed. A run takes cells × engines × secs. The first and last lines record the power mode and load.

## Cells

A cell is `<op> <mode> <workers>`.

| Op | Workload |
| :--- | :--- |
| `hit` | The cache is filled to capacity first; every operation is a get of a resident key. |
| `insert` | The cache is filled to capacity first; every operation inserts a key outside it. |
| `zipf` | Keys from a Zipf distribution (exponent 0.99) over ten times the capacity, starting empty; a miss inserts the key. |
| `hot` | The cache is filled to capacity first; every worker reads the key filled last on every operation. |

| Mode | Workers |
| :--- | :--- |
| `sync` | OS threads. |
| `async` | Tasks on a multi-thread tokio runtime with one worker thread per task. |
| `mixed` | Half on each side of the same instance. |

Default cells: zipf sync 8, zipf sync 64, zipf async 8, zipf async 64, zipf mixed 64, insert sync 1, insert sync 64, insert async 64, hit sync 1, hit sync 8, hit async 64, hot sync 8, hot async 8.

## Output

Each engine-cell line carries the throughput (`mops`), the hit ratio and three entry counts:

| Field | Meaning |
| :--- | :--- |
| `peak_len` | Highest entry count a sampler thread read during the run, once a millisecond. |
| `end_len` | Entry count when the workers stopped. |
| `settled_len` | Entry count after the implementation's deferred work ran (`run_maintenance`, `run_pending_tasks`, `wait`). |

An implementation that holds its capacity shows all three at or below `--capacity`. One that defers or drops writes shows a peak above it or a hit ratio no policy could reach at that capacity. Compare `zipf` throughput only between implementations whose entry counts and hit ratios show they did the same work.

## Engines

Every engine stores `u64` keys and values, costs each entry 1 and is sized to `--capacity` entries, with everything else at its default.

| Engine | Eviction policy | Configuration | Entry count | Mixed |
| :--- | :--- | :--- | :--- | :--- |
| `fibre_cache.s3fifo` | S3-FIFO, the default | `CacheBuilder::default().capacity(n)` from the crate in this repository | `metrics().current_cost` | `to_async()` on one instance |
| `fibre_cache.tinylfu` | W-TinyLFU | The same with `.tinylfu()` | `metrics().current_cost` | `to_async()` on one instance |
| `moka` | TinyLFU admission, LRU eviction | `sync::Cache::new(n)`, `future::Cache::new(n)` | `entry_count()` | `-`: the sync and future caches are separate instances |
| `quick_cache` | S3-FIFO | `sync::Cache::new(n)` | `len()` | one shared instance |
| `stretto` | TinyLFU admission, SampledLFU eviction | `CacheBuilder::new(10 n, n)` with internal cost ignored; `AsyncCacheBuilder` with `tokio::spawn` | `len()` | `-`: the sync and async caches are separate instances |

quick_cache has no async API, so its async workers call the sync methods from inside their tasks.

## Before a run

`cargo test` runs every engine through every op and mode for a few milliseconds and checks that each one completes operations. It is not a benchmark.

Nothing else should be running. Check the power mode and load first.
