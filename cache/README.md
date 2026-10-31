# `fibre_cache`

[![crates.io](https://img.shields.io/crates/v/fibre_cache.svg)](https://crates.io/crates/fibre_cache)
[![docs.rs](https://docs.rs/fibre_cache/badge.svg)](https://docs.rs/fibre_cache)
[![License: MPL-2.0](https://img.shields.io/badge/License-MPL%202.0-brightgreen.svg)](https://opensource.org/licenses/MPL-2.0)

`fibre_cache` is a sharded concurrent cache for Rust with sync and async handles on one instance, built-in S3-FIFO and W-TinyLFU eviction, TTL and TTI, a self-populating loader and no `V: Clone` requirement. The task guide is [README.USAGE.md](README.USAGE.md) and the call surface is [API_REFERENCE.md](API_REFERENCE.md).

## Install

```toml
[dependencies]
fibre_cache = "0.4"
```

| Feature | Default | Enables |
| :--- | :--- | :--- |
| `serde` | yes | `to_snapshot`, `build_from_snapshot` and `build_from_snapshot_async` |
| `random` | yes | `RandomPolicy` |
| `bulk` | yes | `multi_insert`, `multi_remove`, `multi_invalidate` and the sync `multiget` |
| `tokio` | yes | `TokioSpawner`, the default spawner for an `async_loader`. The crate currently needs this feature to compile. |
| `rapidhash` | no | `CacheBuilder::rapidhash` |
| `full` | no | All of the above |

## What to reach for

| Problem | Piece |
| :--- | :--- |
| Build a cache with a capacity | `CacheBuilder::capacity`, then `build` or `build_async` |
| Use one cache from sync and async code | `Cache::to_async`, `AsyncCache::to_sync` |
| Read without taking an `Arc` | `get` with a closure |
| Read and keep the value | `fetch` |
| Read without touching recency or TTI | `peek` |
| Fill the cache on a miss with one load per key | `loader` or `async_loader`, then `fetch_with` |
| Serve a stale value while refreshing it | `stale_while_revalidate`, then `fetch_with` |
| Expire entries | `time_to_live`, `time_to_idle`, `insert_with_ttl` |
| Tighten expiry timing | `janitor_tick_interval`, `timer_mode` |
| Get or insert atomically | `entry` |
| Mutate a value in place | `compute`, `try_compute`, `compute_val`, `try_compute_val` |
| Operate on many keys | `multiget`, `multi_insert`, `multi_remove`, `multi_invalidate` |
| Change the eviction policy | `tinylfu`, `s3fifo`, `null_policy`, `cache_policy_factory` |
| Be told about removals | `eviction_listener` |
| Save and restore the contents | `to_snapshot`, `build_from_snapshot` |
| Walk the contents | `iter`, `iter_snapshot`, `iter_stream`, `iter_snapshot_async` |
| Read hit ratio and eviction counts | `metrics` |
| Run the loader on a runtime other than Tokio | `spawner` with a `TaskSpawner` |

## Notable Users

[Hi Stakes Markets Game](https://www.histakesgame.com) - The world's most advanced financial simulator, available on iPhone and Android.
