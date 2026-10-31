# Cache Arena
**Test Machine:** MacBook M4 Pro (14 cores)

fibre_cache against moka, quick_cache and stretto through one fixed-duration workload harness. Re-run with `cargo run --release` in `cache/arena`.

## Results

- [HIT](./hit.md) - every operation reads a resident key
- [HOT](./hot.md) - every worker reads the same resident key
- [INSERT](./insert.md) - every operation inserts a key not yet seen, into a full cache
- [ZIPF](./zipf.md) - Zipf-distributed reads over ten times the capacity, inserting on a miss

Interpretation: [FINDINGS.md](./FINDINGS.md).

Raw per-cell data: [raw/results.tsv](./raw/results.tsv).

## Run

- Measured at 2026-10-31T04:12:05Z.
- Power mode: high power.
- Load average 6.18 at the start, 20.08 at the end (the end figure is the run's own workers).
- Versions: fibre_cache.s3fifo 0.4.12, fibre_cache.tinylfu 0.4.12, moka 0.12.16, quick_cache 0.6.24, stretto 0.8.4.

## Engines

| Engine | Eviction policy | Built with |
| :--- | :--- | :--- |
| `fibre_cache.s3fifo` | S3-FIFO, the default | `CacheBuilder::default().capacity(n)` |
| `fibre_cache.tinylfu` | W-TinyLFU | `CacheBuilder::default().capacity(n).tinylfu()` |
| `moka` | TinyLFU admission, LRU eviction | `sync::Cache::new(n)`, `future::Cache::new(n)` |
| `quick_cache` | S3-FIFO | `sync::Cache::new(n)` |
| `stretto` | TinyLFU admission, SampledLFU eviction | `CacheBuilder::new(10 n, n)` with internal cost ignored, `AsyncCacheBuilder` with `tokio::spawn` |

## Method

Each cell runs every engine in turn for a fixed number of seconds and reports how many operations its workers completed. Workers stay alive for the whole run, keys are shuffled and dealt round-robin and each worker checks the stop flag every 256 operations. Sync workers are OS threads; async workers are tasks on a multi-thread tokio runtime with one worker thread per task; mixed puts half the workers on each side of the same instance.

Keys and values are `u64` and every entry costs 1, so the capacity is a number of entries. `hit`, `hot` and `insert` fill the cache to capacity before timing starts. `hit` reads resident keys, `hot` has every worker read the key filled last, `insert` adds keys outside the filled range and `zipf` draws keys from a Zipf distribution (exponent 0.99) over ten times the capacity starting from an empty cache, inserting on a miss.

A sampler thread reads the entry count every millisecond; the peak it saw is reported against the capacity, which is how an implementation's capacity bound is checked under load.

Timing starts once every worker has reached a barrier. The power mode and load average are recorded at the start and the end of the run.
