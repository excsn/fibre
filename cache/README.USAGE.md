# Usage Guide: `fibre-cache`

This guide provides a detailed overview of `fibre-cache`'s features, core concepts, and API. It is intended for developers who want to understand how to effectively configure and use the library in their applications.

## Table of Contents
1.  [Core Concepts](#core-concepts)
2.  [Quick Start Examples](#quick-start-examples)
    *   [Synchronous Cache with TTL](#synchronous-cache-with-ttl)
    *   [Asynchronous Cache with Loader](#asynchronous-cache-with-loader)
3.  [Configuration with `CacheBuilder`](#configuration-with-cachebuilder)
4.  [Main API: `Cache` and `AsyncCache`](#main-api-cache-and-asynccache)
    *   [Core Operations](#core-operations)
    *   [Loader-Based Operations](#loader-based-operations)
    *   [Atomic Operations](#atomic-operations)
    *   [Bulk Operations](#bulk-operations)
5.  [Advanced Features](#advanced-features)
    *   [Eviction Policies](#eviction-policies)
    *   [Eviction Listener](#eviction-listener)
    *   [Persistence (Snapshots)](#persistence-snapshots)
6.  [Error Handling](#error-handling)

## Core Concepts

*   **`Cache` and `AsyncCache` Handles**: These are the two primary structs for interacting with the cache. `Cache` provides a synchronous, blocking API, while `AsyncCache` provides a non-blocking, `async` API. They are lightweight handles that can be cloned cheaply and share the same underlying cache instance.
*   **Sharding**: To achieve high concurrency, the cache's data is partitioned across multiple shards. Each shard is managed by its own lock, minimizing contention when different keys are accessed by different threads.
*   **Capacity and Cost**: A `fibre-cache` instance can be "bounded" by a `capacity`. Each item inserted into the cache has a `cost` (a `u64` value). The cache is considered over capacity when the sum of all item costs exceeds the configured capacity. For simple cases, a cost of `1` per item is common.
*   **Background Janitor**: A dedicated background thread, the "janitor," is responsible for all cleanup tasks. This includes removing items that have expired due to Time-to-Live (TTL) or Time-to-Idle (TTI), and evicting items when the cache is over capacity. This design ensures that user-facing operations like `get` and `insert` remain fast and predictable.
*   **Eviction Policies**: The logic for deciding which items to evict when the cache is full is determined by a `CachePolicy`. The library provides a wide range of modern and classic policies (e.g., `TinyLfu`, `ARC`, `SIEVE`, `LRU`) and allows for custom implementations.

## Quick Start Examples

### Synchronous Cache with TTL

This example creates a simple cache that holds up to 100 items, with each item expiring 5 seconds after it's inserted.

```rust
use fibre_cache::CacheBuilder;
use std::thread;
use std::time::Duration;

fn main() {
    let cache = CacheBuilder::default()
        .capacity(100)
        .time_to_live(Duration::from_secs(5))
        .build()
        .expect("Failed to build cache");

    cache.insert("key1".to_string(), 100, 1);
    
    assert!(cache.get(&"key1".to_string()).is_some());
    println!("Item 'key1' is in the cache.");

    println!("Waiting for 6 seconds for item to expire...");
    thread::sleep(Duration::from_secs(6));

    assert!(cache.get(&"key1".to_string()).is_none());
    println!("Item 'key1' has expired and is no longer in the cache.");
}
```

### Asynchronous Cache with Loader

This example demonstrates a "read-through" cache. When `get_with` is called for a key that isn't present, the provided `async_loader` is automatically executed to fetch the value.

```rust
use fibre_cache::CacheBuilder;
use tokio::time::Duration;

#[tokio::main]
async fn main() {
    let cache = CacheBuilder::default()
        .capacity(100)
        .async_loader(|key: String| async move {
            println!("Loader executed for key: {}", key);
            // In a real app, this would be a database or API call.
            (format!("value for {}", key), 1)
        })
        .build_async()
        .unwrap();

    // First call: a cache miss, the loader runs.
    let value1 = cache.get_with(&"my-key".to_string()).await;
    assert_eq!(*value1, "value for my-key");

    // Second call: a cache hit, the loader does not run.
    let value2 = cache.get_with(&"my-key".to_string()).await;
    assert_eq!(*value2, "value for my-key");

    assert_eq!(cache.metrics().misses, 1);
    assert_eq!(cache.metrics().hits, 1);
}
```

## Configuration with `CacheBuilder`

All caches must be configured and created via the `CacheBuilder`. It provides a fluent API for setting up all aspects of the cache.

**Primary Struct:** `fibre_cache::CacheBuilder<K, V, H>`

**Common Configuration Methods:**
*   `.capacity(u64)`: Sets the maximum total cost of all items in the cache.
*   `.unbounded()`: Creates a cache with no capacity limit. This is the default.
*   `.time_to_live(Duration)`: Sets a global Time-to-Live for all items.
*   `.time_to_idle(Duration)`: Sets a global Time-to-Idle for all items.
*   `.stale_while_revalidate(Duration)`: Configures a grace period for serving stale data while refreshing.
*   `.cache_policy(impl CachePolicy)`: Sets a custom eviction policy.
*   `.loader(impl Fn)`: Sets a synchronous loader function.
*   `.async_loader(impl Fn)`: Sets an asynchronous loader function.
*   `.eviction_listener(impl EvictionListener)`: Registers a callback for item evictions.
*   `.shards(usize)`: Sets the number of concurrent shards (will be rounded to the next power of two).

**Build Methods:**
*   `.build() -> Result<Cache, BuildError>`: Creates a synchronous `Cache`.
*   `.build_async() -> Result<AsyncCache, BuildError>`: Creates an asynchronous `AsyncCache`.

## Main API: `Cache` and `AsyncCache`

These are the primary handles for interacting with the cache. Their APIs are largely symmetric, with `async` versions of methods in `AsyncCache`.

### Core Operations

*   `pub fn get(&self, key: &K) -> Option<Arc<V>>`
    *   Retrieves a clone of the `Arc` containing the value if the key exists and is not expired.
*   `pub fn insert(&self, key: K, value: V, cost: u64)`
    *   Inserts a key-value-cost triple into the cache. If the key already exists, its value and cost are updated.
*   `pub fn insert_with_ttl(&self, key: K, value: V, cost: u64, ttl: Duration)`
    *   Inserts an item with a specific TTL that overrides the cache's global default.
*   `pub fn invalidate(&self, key: &K) -> bool`
    *   Removes an item from the cache, returning `true` if the item was present.
*   `pub fn clear(&self)`
    *   Removes all items from the cache. This is a "stop-the-world" operation that locks all shards.
*   `pub fn metrics(&self) -> MetricsSnapshot`
    *   Returns a point-in-time snapshot of the cache's performance metrics.

### Loader-Based Operations

*   `pub fn get_with(&self, key: &K) -> Arc<V>`
    *   Retrieves a value for the given key. If the key is not present, it executes the configured synchronous or asynchronous loader to compute, insert, and return the value. This method provides thundering herd protection.

### Atomic Operations

*   `pub fn entry(&self, key: K) -> Entry<'_, K, V, H>`
    *   Provides an API similar to `std::collections::HashMap::entry` for atomic "get-or-insert" operations. The returned `Entry` enum can be either `Occupied` or `Vacant`.
*   `pub fn compute<F>(&self, key: &K, f: F) -> bool`
    *   Atomically applies a closure `F` to a mutable reference of a value. This succeeds only if no other thread is holding a reference (`Arc`) to the value, ensuring safe, in-place mutation.

### Bulk Operations

*(Requires the `bulk` feature flag)*
These methods are more efficient than calling their singular counterparts in a loop, as they parallelize work across shards.

*   `pub fn multiget<I, Q>(&self, keys: I) -> HashMap<K, Arc<V>>`
    *   Retrieves multiple values from the cache.
*   `pub fn multi_insert<I>(&self, items: I)`
    *   Inserts multiple key-value-cost triples into the cache.
*   `pub fn multi_invalidate<I, Q>(&self, keys: I)`
    *   Removes multiple entries from the cache.

## Advanced Features

### Eviction Policies

You can provide a custom eviction policy to the `CacheBuilder`. The library provides several high-performance implementations:
*   `fibre_cache::policy::TinyLfuPolicy`: The default for bounded caches. A modern, high-hit-rate policy that balances recency and frequency.
*   `fibre_cache::policy::ArcPolicy`: A classic adaptive policy that dynamically balances between LRU and LFU strategies.
*   `fibre_cache::policy::SievePolicy`: A very recent, low-overhead, and scan-resistant policy.
*   `fibre_cache::policy::SlruPolicy`: Segmented LRU, which is more scan-resistant than plain LRU.
*   `fibre_cache::policy::LruPolicy`: Classic Least Recently Used.
*   `fibre_cache::policy::FifoPolicy`: First-In, First-Out.
*   `fibre_cache::policy::RandomPolicy`: Evicts a random item when at capacity.

### Eviction Listener

You can observe all item removals by implementing the `EvictionListener` trait and passing it to the builder.

**Trait:** `fibre_cache::listener::EvictionListener<K, V>`
*   `fn on_evict(&self, key: K, value: Arc<V>, reason: EvictionReason)`
    *   This method is called on a dedicated background thread with the key, value, and reason for removal (`Capacity`, `Expired`, or `Invalidated`). This ensures listener logic never blocks cache operations.

### Persistence (Snapshots)

*(Requires the `serde` feature flag)*
The library provides a mechanism to save and restore the cache's state.

*   `pub fn to_snapshot(&self) -> CacheSnapshot<K, V>`
    *   A method on `Cache` and `AsyncCache` that creates a serializable representation of all non-expired items. This is a "stop-the-world" operation.
*   `CacheBuilder::build_from_snapshot(snapshot: CacheSnapshot)`
    *   A method on the builder to construct a new cache and pre-populate it with data from a snapshot.

## Error Handling

The only place where errors can occur in the public API is during the construction of the cache.

**Enum:** `fibre_cache::error::BuildError`
*   `ZeroCapacity`: Returned if a bounded cache is configured with a capacity of zero.
*   `ZeroShards`: Returned if the cache is configured with zero shards.
*   `SpawnerRequired`: Returned if an `async_loader` is used without providing a `TaskSpawner` or enabling the `tokio` feature.