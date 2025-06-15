# **`fibre_cache`**

`fibre_cache` offers a best-in-class, most flexible, high-performance, concurrent, multimode sync/async cache for Rust. It is built from the heart and soul of a Java-turned-Rust engineer to bring the comprehensive, flexible, and ergonomic "it just works" feeling of top-tier Java caches (like Caffeine) to the Rust ecosystem.

## ⚠️ Current Status: Beta

Please note that `fibre_cache` is currently in an active beta phase. The core functionality is robust and well-tested, but the public API is still evolving.

This means:
*   **APIs May Change:** Until the `1.0` release, breaking changes may be introduced in minor version updates (`0.x.y`).
*   **Production Use:** While we strive for stability, using this library in a mission-critical production environment is **at your own risk**. Please be prepared to adapt to changes and conduct thorough testing for your specific use case.

We highly encourage you to try it out, provide feedback and report any issues. Contributions are always welcome as we work towards a stable release

## Motivation

The current state of caching in Rust, while offering some excellent high-performance options, often feels specialized and inflexible. Caches can be brilliant but opinionated, while others lack the rich, general-purpose feature set needed for complex applications.

`fibre_cache` was born from the desire for a cache that is **both incredibly fast and uncompromisingly flexible**. It aims to be the definitive, general-purpose caching library for Rust, providing a complete toolset that feels both powerful to experts and intuitive to newcomers.

## Core Philosophy
*   **No Compromises:** Get the raw speed of a systems language without sacrificing the rich, ergonomic APIs you love.
*   **Flexibility First:** From swappable eviction policies to runtime-agnostic async loaders, the cache adapts to your needs, not the other way around.
*   **Performance by Design:** Built from first principles with a sharded, hybrid-locking concurrent core to minimize contention and maximize throughput.
*   **Idiomatic Rust:** Embrace Rust's strengths, like the ownership model, to provide features impossible in other languages, such as first-class support for non-`Clone` values.

---

## Quickstart

Get started quickly by adding `fibre_cache` to your `Cargo.toml`:

```toml
[dependencies]
# From crates.io:
# fibre_cache = "^0"

tokio = { version = "1", features = ["full"] }
```

A simple example using the `CacheBuilder` and the self-populating async loader:

```rust
use fibre_cache::CacheBuilder;
use std::time::Duration;

#[tokio::main]
async fn main() {
    // Build a cache that can hold up to 10,000 items.
    // Configure it with a 10-minute TTL and an async loader.
    let cache = CacheBuilder::default()
        .capacity(10_000)
        .time_to_live(Duration::from_secs(600))
        .async_loader(|key: String| async move {
            println!("CacheLoader: Simulating fetch for key '{}'...", key);
            // In a real app, this would be a database call or API request.
            tokio::time::sleep(Duration::from_millis(50)).await;
            (format!("Value for {}", key), 1) // Return (value, cost)
        })
        .build_async()
        .expect("Failed to build cache");

    // 1. First access to "my-key": miss. The loader is called automatically.
    // The .get_with() method handles the thundering herd problem.
    let value1 = cache.get_with(&"my-key".to_string()).await;
    println!("Received: {}", value1);
    assert_eq!(*value1, "Value for my-key");

    // 2. Second access to "my-key": hit. The value is returned instantly.
    let value2 = cache.get_with(&"my-key".to_string()).await;
    println!("Received: {}", value2);
    assert_eq!(*value2, "Value for my-key");

    // Check the metrics
    let metrics = cache.metrics();
    assert_eq!(metrics.hits, 1);
    assert_eq!(metrics.misses, 1);
    println!("\nCache Metrics: {:#?}", metrics);
}
```

---

## Feature Overview

`fibre_cache` combines the best features from across the caching landscape into one comprehensive package.

#### **Key Architectural Pillars**
*   **High Concurrency via Sharding:** Partitions data across independently locked shards to minimize contention.
*   **Hybrid Locking:** A custom `HybridRwLock` provides raw, blocking speed for sync operations and true, non-blocking suspension for async operations.
*   **No `V: Clone` Required:** Stores values in an `Arc<V>`, making `V: Clone` unnecessary for most operations—a core advantage over many other Rust caches.
*   **Unified Sync/Async API:** Zero-cost `to_sync()` and `to_async()` methods allow seamless conversion between handles.
*   **Trait-Based Policies:** A flexible `CachePolicy` trait allows for swapping out admission and eviction algorithms.

### **Implemented Features**
| Feature | Details |
| :--- | :--- |
| **Atomic `entry` API** | A fully atomic get-or-insert API mimicking `HashMap`, safe for concurrent use in both sync and async contexts. |
| **`CacheLoader`** | Self-populating cache via `get_with`. Supports both sync (`loader`) and async (`async_loader`) functions. |
| **Thundering Herd Protection**| Guarantees a loader is executed only once for concurrent misses on the same key, preventing duplicate work. |
| **Runtime Agnostic** | The `async_loader` can be used with any async runtime (`tokio`, `async-std`, etc.) via a `TaskSpawner` trait. |
| **Stale-While-Revalidate** | Can serve stale data for a configured grace period while refreshing it in the background, hiding latency. |
| **Comprehensive Policies** | Includes **TinyLfu**, **SIEVE**, **SLRU**, **ARC**, **LRU**, **FIFO**, and **Random** eviction/admission policies. |
| **Bulk Operations** | Efficient, parallelized `multiget`, `multi_insert`, and `multi_invalidate` methods (via `bulk` feature). |
| **In-place Mutation** | A `compute` method for safe, atomic mutation of existing values without cloning. |
| **Eviction Listener** | Receive non-blocking notifications for every item removal on a dedicated background thread. |
| **Persistence (`serde`)**| An opt-in `serde` feature provides a flexible `to_snapshot()` and `build_from_snapshot()` API. |
| **Metrics & Stats** | A zero-dependency, queryable metrics system for deep observability. |


## Feature Comparison

| Feature Group | Feature | Moka / Caffeine | Stretto / Ristretto | **`fibre_cache`** | Notes |
| :--- | :--- | :--- | :--- | :--- | :--- |
| **Core Architecture** | **Concurrency Model** | Sharded | Sharded | ✅ **Sharded, Custom Lock** | Custom hybrid lock with distinct, optimized paths for sync and async. |
| | **Sync/Async Interoperability** | Separate Caches | ✅ | ✅ **Zero-Cost Handles** | A single cache instance can be accessed via both sync and async handles seamlessly. |
| | **`V: Clone` Required?** | **Yes** | No | ✅ **No** | `Arc<V>` storage natively supports non-cloneable values, a major ergonomic win. |
| | **Maintenance Model** | Background Thread | Sampling-based | ✅ **Janitor + Opportunistic** | A dedicated janitor thread is augmented by opportunistic maintenance on writes. |
| | **Contention-Free Access Batching**| Event Queue | Buffer-based | ✅ **Striped Left-Right** | A sophisticated, low-contention `AccessBatcher` for policy updates on the hot path. |
| | | | | | |
| **Eviction & Policies**| **Cost-Based Eviction** | ✅ | ✅ | ✅ | Items can have individual `cost` towards `capacity`. |
| | **Global TTL / TTI** | ✅ | ✅ | ✅ | Cache-wide Time-to-Live and Time-to-Idle. |
| | **Per-Item TTL** | ✅ | ❌ | ✅ | `insert_with_ttl` provides fine-grained expiration control. |
| | **Timer System for Expirations**| Hierarchical Wheel | ❌ (Sampling) | ✅ **Configurable Per-Shard Wheel** | High-precision, low-overhead expiration management via `TimerWheelMode`. |
| | **Pluggable Policies** | Limited | ❌ | ✅ **Highly Flexible** | A `CachePolicy` trait allows for easy implementation of custom strategies. |
| | **Available Policies** | Segmented-LRU | TinyLFU | ✅ **Most Comprehensive** | **TinyLFU**, **ARC**, **SLRU**, **SIEVE**, **LRU**, **FIFO**, and **Random**. |
| | | | | | |
| **API & Ergonomics** | **`entry` API** | ✅ | ❌ | ✅ | Atomic "get-or-insert" for both sync and async. |
| | **Read API Ergonomics** | `get` (clones Arc) | `get` (returns ref) | ✅ **`get` & `fetch`** | Offers both a zero-cost `get` (with closure) and an `Arc`-cloning `fetch`. |
| | **`compute`** | ✅ | ❌ | ✅ | Safe, atomic, in-place mutation of values. |
| | **Bulk Operations** | ✅ | ✅ | ✅ | Parallelized `multiget`, `multi_insert`, etc. (via `bulk` feature) |
| | | | | | |
| **Advanced Patterns**| **`CacheLoader`** | ✅ | ❌ | ✅ **Runtime Agnostic** | `TaskSpawner` trait makes it work with any async runtime. |
| | **Thundering Herd Protection** | ✅ | N/A | ✅ | Built into `get_with` to prevent duplicate work. |
| | **Stale-While-Revalidate** | ✅ | ❌ | ✅ | Hides refresh latency by serving stale data. |
| | | | | | |
| **Production Features**| **Eviction Listener** | ✅ | ✅ | ✅ | Decoupled notifications on a dedicated background task to prevent blocking. |
| | **Metrics & Stats** | ✅ | ✅ | ✅ | Detailed, contention-free metrics via `MetricsSnapshot`. |
| | **Persistence (`serde`)** | ❌ | ❌ | ✅ **Serializable Snapshot** | `to_snapshot()` and `build_from_snapshot()` API (via `serde` feature). |

# License

Licensed under the [MPL-2.0](https://www.mozilla.org/en-US/MPL/2.0/).

Of course. Here is the final, expanded feature comparison table.