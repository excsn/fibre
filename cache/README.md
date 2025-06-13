# **`Fibre Cache`**

`fibre_cache` offer best-in-class, high-performance, concurrent, multimode sync/async cache for Rust. It is built from the heart and soul of a Java-turned-Rust engineer to bring the comprehensive, flexible, and ergonomic "it just works" feeling of top-tier Java caches (like Caffeine) to the Rust ecosystem.

## Motivation

The current state of caching in Rust, while offering some excellent high-performance options, often feels specialized and inflexible. Caches like `stretto` are brilliant but opinionated, while others lack the rich, general-purpose feature set needed for complex applications.

`fibre_cache` was born from the desire for a cache that is **both incredibly fast and uncompromisingly flexible**. It aims to be the definitive, general-purpose caching library for Rust, providing a complete toolset that feels both powerful to experts and intuitive to newcomers.

## Core Philosophy
*   **No Compromises:** Get the raw speed of a systems language without sacrificing the rich, ergonomic APIs you love.
*   **Flexibility First:** From swappable eviction policies to runtime-agnostic async loaders, the cache adapts to your needs, not the other way around.
*   **Performance by Design:** Built from first principles with a sharded, hybrid-locking concurrent core to minimize contention and maximize throughput.
*   **Idiomatic Rust:** Embrace Rust's strengths, like the ownership model, to provide features impossible in other languages, such as first-class support for non-`Clone` values.

---

## Quickstart

Get started quickly by adding `fibre_cache` and its dependencies to your `Cargo.toml`:

```toml
[dependencies]
fibre_cache = { git = "https://github.com/excsn/fibre" } # Or path = "..."
tokio = { version = "1", features = ["full"] }
```

A simple example using the `CacheBuilder` and the self-populating `CacheLoader`:

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
    // The .get_with_async() method handles the thundering herd problem.
    let value1 = cache.get_with_async(&"my-key".to_string()).await;
    println!("Received: {}", value1);
    assert_eq!(*value1, "Value for my-key");

    // 2. Second access to "my-key": hit. The value is returned instantly.
    let value2 = cache.get_with_async(&"my-key".to_string()).await;
    println!("Received: {}", value2);
    assert_eq!(*value2, "Value for my-key");

    // Check the metrics
    let metrics = cache.metrics();
    assert_eq!(metrics.hits, 1);
    assert_eq!(metrics.misses, 1);
    println!("\nCache Metrics: {:?}", metrics);
}
```

---

## Feature Overview

`fibre_cache` combines the best features from across the caching landscape into one comprehensive package.

#### **Key Architectural Pillars**
*   **High Concurrency via Sharding:** Partitions data across independently locked shards to minimize contention.
*   **Hybrid Locking:** A custom `HybridRwLock` provides raw, blocking speed for sync operations and true, non-blocking suspension for async operations.
*   **Non-`Clone` Value Support:** Stores values in an `Arc<V>`, making `V: Clone` unnecessary for most operations—a core advantage over many other Rust caches.
*   **Unified Sync/Async API:** Zero-cost `to_sync()` and `to_async()` methods allow seamless conversion between handles.
*   **Trait-Based Policies:** A flexible `CachePolicy` trait allows for swapping out admission and eviction algorithms.

### **Implemented Features**
| Feature | Details |
| :--- | :--- |
| **Atomic `entry` API** | A fully atomic get-or-insert API mimicking `HashMap`, safe for concurrent use. |
| **`CacheLoader`** | Self-populating cache via `get_with`. Supports both sync (`loader`) and async (`async_loader`) functions. |
| **Thundering Herd Protection**| Guarantees a loader is executed only once for concurrent misses on the same key. |
| **Runtime Agnostic** | The `async_loader` can be used with any async runtime via a `TaskSpawner` trait. |
| **Stale-While-Revalidate** | Can serve stale data for a configured grace period while refreshing it in the background, hiding latency. |
| **Comprehensive Policies** | Includes **TinyLfu**, **SIEVE**, **SLRU**, **ARC**, **LRU**, **FIFO**, and **Random** eviction/admission policies. |
| **Bulk Operations** | Efficient, parallelized `get_all`, `insert_all`, and `invalidate_all` methods. |
| **In-place Mutation** | A `compute` method for safe, atomic mutation of existing values. |
| **Eviction Listener** | Receive non-blocking notifications for every item removal on a dedicated background thread. |
| **Persistence (`serde`)**| An opt-in `serde` feature provides a flexible `to_snapshot()` and `build_from_snapshot()` API. |
| **Metrics & Stats** | A zero-dependency, queryable metrics system for deep observability. |


## Feature Comparison

This table compares `fibre_cache` against other popular high-performance caching libraries.

| Feature Group | Feature | Moka / Caffeine | Stretto / Ristretto | **`fibre_cache`** | Key Advantages & Notes |
| :--- | :--- | :--- | :--- | :--- | :--- |
| **Core Architecture** | **Concurrency Model** | Sharded, Fine-Grained Locks | Sharded, Fine-Grained Locks | ✅ Sharded, Custom Hybrid Lock | Custom lock provides true non-blocking async waits. |
| | **Sync & Async API** | ✅ | ✅ | ✅ | Fully interoperable sync and async handles. |
| | **`V: Clone` Required?** | Yes | No (but `K`/`V` fixed) | ✅ **No** | **Core Advantage:** `Arc<V>`-based storage supports non-cloneable values. |
| | | | | | |
| **Eviction & Policies**| **Cost-Based Eviction** | ✅ | ✅ | ✅ | |
| | **TTL / TTI Support** | ✅ | ✅ | ✅ | Handled by a dedicated background task. |
| | **Pluggable Policies** | Limited | ❌ (TinyLFU is integral) | ✅ **Highly Flexible** | A `CachePolicy` trait allows for swappable strategies. |
| | **Available Policies** | Segmented-LRU | Sampled-LFU (TinyLFU) | ✅ **Most Comprehensive**<br/>*TinyLfu, SIEVE, SLRU, ARC...* | Offers a wide range of modern and classic algorithms. |
| | **Admission Control** | Implicit in SLRU | ✅ Probabilistic Filter | ✅ **Integrated** | Enables scan resistance and smart admission within the policy. |
| | | | | | |
| **API & Ergonomics** | **`entry` API** | ✅ (Excellent `entry` API) | ❌ | ✅ | Atomic "get-or-insert" for both sync and async. |
| | **`compute`** | ✅ | ❌ | ✅ | |
| | **Bulk Operations** | ✅ | ✅ | ✅ | Efficient, parallelized `get_all`, `insert_all`, etc. |
| | | | | | |
| **Advanced Patterns**| **`CacheLoader`** | ✅ | ❌ | ✅ **Runtime Agnostic** | `TaskSpawner` trait makes it work with any async runtime. |
| | **Thundering Herd Protection** | ✅ | N/A | ✅ | Built into the `CacheLoader` via a custom `LoadFuture`. |
| | **Stale-While-Revalidate** | ✅ | ❌ | ✅ | Hides latency by serving stale data while refreshing. |
| | **Weak/Soft References** | ✅ | ❌ | ❌ (Deferred) | An advanced memory management feature for a future version. |
| | | | | | |
| **Production Features**| **Eviction Listener** | ✅ | ✅ | ✅ | Non-blocking notifications via a dedicated task. |
| | **Metrics & Stats** | ✅ | ✅ | ✅ | |
| | **Persistence (`serde`)** | ❌ | ❌ | ✅ **Serializable Snapshot** | `to_snapshot()` and `build_from_snapshot()` offer a flexible API. |

# License

Licensed under the [MPL-2.0](https://www.mozilla.org/en-US/MPL/2.0/).