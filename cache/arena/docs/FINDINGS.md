# Findings

quick_cache is an unranked yardstick. It has no expiry or cache-level loader and its async cells call a sync `get` on a blocking lock. fibre_cache.s3fifo leads the feature-comparable engines, moka and stretto, in every cell.

13-cell run of 2026-10-30, Mops/s as in the op pages, capacity 10000, 10 s per cell. High power mode, load average 6.18 at the start; the 20.08 at the end is the run's own 64-worker cells. Every cell is from that run except zipf mixed 64, rerun on 2026-10-31 at load average 6.87 after the full run gave fibre_cache.s3fifo 16.66 and fibre_cache.tinylfu 11.35 and neither repeated. Both fibre columns are the working tree with `fibre::sync::v2::HybridRwLock` on its shards and hit and miss counters striped by thread: `fibre_cache.s3fifo` runs the S3-FIFO default and `fibre_cache.tinylfu` runs W-TinyLFU through `.tinylfu()`. The engines table in [README.md](./README.md) names every column's eviction policy.

## Reads scale with threads once readers write no shared word

hit sync 1, 8 and hit async 64: fibre_cache.s3fifo 44.4, 108.2, 224.8; quick_cache 166.4, 94.8, 104.9; moka 12.4, 6.4, 28.1; stretto 23.8, 4.3, 4.8.

quick_cache leads at one thread by 3.7x and fibre_cache.s3fifo leads at 8 threads by 1.1x and at 64 async tasks by 2.1x. quick_cache's hit is its `parking_lot::RwLock` acquire and release on a shared word; fibre_cache.s3fifo's reader touches a per-thread slot and reads a flag, so its hit cells rise with the thread count instead of flattening at one cache line's bandwidth.

## One hot key is bounded by the value's reference count

hot sync 8 and hot async 8: fibre_cache.s3fifo 11.0, 10.5; quick_cache 14.3, 8.1; moka 5.2, 5.1; fibre_cache.tinylfu 5.1, 5.5; stretto 5.1, 4.8.

Every worker fetches the same key, so the lock no longer separates them. fibre_cache.s3fifo returns an `Arc<V>`; the clone and drop of its count are two shared writes per fetch on one line. quick_cache returns a `u64` by value, so its cell is its lock pair only. fibre_cache.tinylfu records the hit in a ring that every worker writes, which is why it sits with moka and stretto here.

## Mixed Zipf holds under 64 workers

zipf sync 8, sync 64, async 8, async 64, mixed 64: fibre_cache.s3fifo 30.1, 31.3, 30.7, 31.3, 26.2; quick_cache 42.4, 21.0, 47.0, 24.8, 21.3; fibre_cache.tinylfu 23.6, 23.9, 20.9, 25.9, 19.4; moka 5.6, 1.1, 5.4, 4.0, -; stretto 3.7, 3.6, 3.9, 3.8, -.

quick_cache leads at 8 workers by 1.4x to 1.5x and fibre_cache.s3fifo leads at 64 workers by 1.5x on sync, 1.3x on async and 1.2x on mixed. fibre_cache.s3fifo's throughput rises 4% between 8 and 64 sync workers (30.1 and 31.3); quick_cache's drops by 50%.

Hit ratios on this trace: fibre_cache.s3fifo 0.777, fibre_cache.tinylfu 0.774, quick_cache 0.785, moka 0.785 to 0.788, stretto 0.615 to 0.668. Peak entry counts in every zipf cell: fibre_cache.s3fifo and fibre_cache.tinylfu 9984, moka 10000, stretto 10002 to 10003, quick_cache 10048.

## Hit ratios against what each op allows

hit sync 1, 8 and hit async 64: fibre_cache.s3fifo 0.953, 0.956, 0.954; fibre_cache.tinylfu 0.955, 0.956, 0.949; quick_cache 0.963, 0.963, 0.975; moka and stretto 1.000. Every hot cell is 1.000 for every engine.

A hit cell asks only for prefilled keys, so the shortfall from 1.0 is keys evicted during the prefill. fibre_cache runs 128 shards on this machine (8 per core, rounded up to a power of two) and gives each a budget of 78 entries, which is where the peak of 9984 (78 x 128) comes from. Prefilling exactly 10000 keys puts about 78 keys on each shard with a spread of about 9. The shards over budget evict their surplus, which at that spread comes to about 450 keys, the 4% to 5% the cells show. Each cell builds a new cache with a new hash seed, so the shortfall moves by about a hundredth between cells. moka and stretto bound the capacity across the whole cache and keep every prefilled key. quick_cache is also sharded and its 3% shortfall is likely the same effect; that is not measured here.

On zipf the ceiling is 0.800, the hit ratio of a cache that always holds the 10000 most frequent keys. moka (0.785 to 0.788), quick_cache (0.785) and fibre_cache.s3fifo (0.777) are within 0.023 of it. fibre_cache.s3fifo holds at most 9984 entries and its shards fill unevenly, which accounts for part of its 0.008 gap to quick_cache, which runs the same policy. quick_cache peaks at 10048, 48 entries (0.5%) over its capacity, in every zipf cell; holding 48 more of the most frequent keys is worth 0.0004 of hit ratio on this distribution, so its ratio and throughput still compare. stretto's 0.615 to 0.668 is the abnormal one: its TinyLFU admission rejects new keys and its insert buffer drops inserts under load, so it misses about one more operation in seven than the others.

## Inserts are 3x behind at one thread and 1.3x to 1.5x behind at 64

insert sync 1, sync 64, async 64: fibre_cache.s3fifo 13.9, 34.2, 38.1; quick_cache 42.5, 52.3, 50.2; fibre_cache.tinylfu 8.1, 21.5, 22.5; stretto 8.3, 4.4, 2.9; moka 2.8, 0.3, 1.3.

The one-thread gap is per-operation work, not contention: fibre_cache.s3fifo allocates the `Arc<V>` on every insert, frees the evicted one and runs S3-FIFO's small-queue eviction and ghost bookkeeping, where quick_cache keeps entries in a linked slab and allocates nothing per entry. The 64-worker gap is smaller because inserts to different shards do not contend in either.

## TinyLFU costs half the read throughput of S3-FIFO on this cache

fibre_cache.tinylfu against fibre_cache.s3fifo: hit sync 8 37.6 against 108.2, hit async 64 47.6 against 224.8, zipf sync 64 23.9 against 31.3, insert sync 1 8.1 against 13.9.

TinyLFU records every read in a shared 256-slot ring per shard, applied under the write lock. Its admission runs a frequency sketch on every insert; S3-FIFO bumps a per-entry byte under the read lock and admits without a sketch. Its hit ratio on this trace is 0.003 lower.
