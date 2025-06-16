# Usage Guide: `fibre_cache`

This guide provides a detailed overview of `fibre_cache`'s features, core concepts, and API. It is intended for developers who want to understand how to effectively configure and use the library in their applications.

## Table of Contents
1.  [Core Concepts](#core-concepts)
2.  [API Patterns](#api-patterns)
3.  [Quick Start Examples](#quick-start-examples)
    *   [Synchronous Cache with TTL](#synchronous-cache-with-ttl)
    *   [Asynchronous Cache with Loader](#asynchronous-cache-with-loader)
    *   [Cache with Stale-While-Revalidate](#cache-with-stale-while-revalidate)
4.  [Configuration with `CacheBuilder`](#configuration-with-cachebuilder)
5.  [Main API: `Cache` and `AsyncCache`](#main-api-cache-and-asynccache)
    *   [Core Operations](#core-operations)
    *   [Loader-Based Operations](#loader-based-operations)
    *   [Atomic Operations](#atomic-operations)
    *   [Bulk Operations](#bulk-operations)
6.  [Advanced Features](#advanced-features)
    *   [Eviction Policies](#eviction-policies)
    *   [Eviction Listener](#eviction-listener)
    *   [Persistence (Snapshots)](#persistence-snapshots)
7.  [Error Handling](#error-handling)

## Core Concepts

*   **`Cache` and `AsyncCache` Handles**: These are the two primary structs for interacting with the cache. `Cache` provides a synchronous, blocking API, while `AsyncCache` provides a non-blocking, `async` API. They are lightweight handles that can be cloned cheaply and share the same underlying cache instance.
*   **Sharding**: To achieve high concurrency, the cache's data is partitioned across multiple shards. Each shard is managed by its own lock, minimizing contention when different keys are accessed by different threads.
*   **Capacity and Cost**: A `fibre_cache` instance can be "bounded" by a `capacity`. Each item inserted into the cache has a `cost` (a `u64` value). The cache is considered over capacity when the sum of all item costs exceeds the configured capacity. For simple cases where all items are the same size, a cost of `1` per item is common.
*   **Background Janitor**: A dedicated background thread, the "janitor," is responsible for all cleanup tasks. This includes removing items that have expired due to Time-to-Live (TTL) or Time-to-Idle (TTI), and evicting items when the cache is over capacity. This design ensures that user-facing operations like `get` and `insert` remain fast and predictable.
*   **Eviction Policies**: The logic for deciding which items to evict when the cache is full is determined by a `CachePolicy`. The library provides a wide range of modern and classic policies and allows for custom implementations. For bounded caches, the default is `TinyLfuPolicy`, a high-performance, scan-resistant policy suitable for most workloads.

## API Patterns

There are two important patterns to understand when using `fibre_cache`:

1.  **Values are stored in `Arc<V>`**: The cache stores values inside an `Arc`. This means that `fetch` and other retrieval methods return an `Arc<V>`. This design choice is crucial: it allows multiple threads to hold references to the same cached value without needing `V` to be `Clone`, and it makes retrievals very fast as only the `Arc`'s reference count is modified.

2.  **Flexible Key Lookups (`&Q`)**: Many lookup methods (like `get`, `fetch`, `invalidate`) accept a key of type `&Q`. This powerful pattern allows you to perform lookups using a borrowed type, such as using a `&str` to look up a `String` key. This avoids unnecessary allocations and improves performance.

    ```rust
    // A cache with String keys.
    let cache: fibre_cache::Cache<String, i32> = fibre_cache::CacheBuilder::new().build().unwrap();
    cache.insert("my-key".to_string(), 123, 1);
    
    // We can fetch using a &str, no need to create a new String.
    let value = cache.fetch("my-key").unwrap();
    assert_eq!(*value, 123);
    ```

## Quick Start Examples

### Synchronous Cache with TTL

This example creates a simple cache that holds up to 100 items, with each item expiring 5 seconds after it's inserted.

```rust
use fibre_cache::CacheBuilder;
use std::thread;
use std::time::Duration;
use std::sync::Arc;

fn main() {
    // Create a cache that expires items 5 seconds after insertion.
    let cache = CacheBuilder::<String, i32>::new()
        .capacity(100)
        .time_to_live(Duration::from_secs(5))
        .build()
        .expect("Failed to build cache");

    cache.insert("key1".to_string(), 100, 1);
    
    // fetch() returns an Arc<V>, which is efficient.
    let value: Arc<i32> = cache.fetch("key1").unwrap();
    assert_eq!(*value, 100);
    println!("Item 'key1' is in the cache.");

    println!("Waiting for 6 seconds for item to expire...");
    thread::sleep(Duration::from_secs(6));

    // The background janitor will have removed the expired item.
    assert!(cache.fetch("key1").is_none());
    println!("Item 'key1' has expired and is no longer in the cache.");
}
```

### Asynchronous Cache with Loader

This example demonstrates a "read-through" cache. When `fetch_with` is called for a key that isn't present, the provided `async_loader` is automatically executed to fetch the value. This feature includes **thundering herd protection**, ensuring the loader is only called once even if many tasks request the same missing key simultaneously.

```rust
use fibre_cache::CacheBuilder;
use tokio::time::Duration;
use std::sync::Arc;

#[tokio::main]
async fn main() {
    let cache = CacheBuilder::default()
        .capacity(100)
        .async_loader(|key: String| async move {
            println!("-> Loader executed for key: {}", key);
            // In a real app, this would be a database query or API call.
            tokio::time::sleep(Duration::from_millis(100)).await;
            (format!("value for {}", key), 1) // Return the value and its cost
        })
        .build_async()
        .unwrap();

    // First call: a cache miss, the loader runs.
    println!("Requesting 'my-key' for the first time...");
    let value1: Arc<String> = cache.fetch_with(&"my-key".to_string()).await;
    assert_eq!(*value1, "value for my-key");

    // Second call: a cache hit, the loader does not run.
    println!("Requesting 'my-key' for the second time...");
    let value2: Arc<String> = cache.fetch_with(&"my-key".to_string()).await;
    assert_eq!(*value2, "value for my-key");

    let metrics = cache.metrics();
    assert_eq!(metrics.misses, 1);
    assert_eq!(metrics.hits, 1);
    println!("Metrics confirm 1 miss and 1 hit.");
}
```

### Cache with Stale-While-Revalidate

This feature improves latency by serving stale data from the cache immediately while triggering a background refresh.

```rust
use fibre_cache::CacheBuilder;
use std::thread;
use std::time::Duration;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

fn main() {
    let load_counter = Arc::new(AtomicUsize::new(0));

    let cache = CacheBuilder::default()
        .time_to_live(Duration::from_secs(2)) // Item is fresh for 2 seconds.
        .stale_while_revalidate(Duration::from_secs(10)) // Can be served stale for 10 more seconds.
        .loader({
            let counter = load_counter.clone();
            move |key: String| {
                let version = counter.fetch_add(1, Ordering::SeqCst) + 1;
                println!("[Loader] Loading version {} for key '{}'...", version, key);
                (format!("Version {}", version), 1)
            }
        })
        .build()
        .unwrap();

    // 1. Initial load.
    let value1 = cache.fetch_with(&"my-data".to_string());
    println!("Received: {}", *value1);
    assert_eq!(*value1, "Version 1");

    // 2. Wait for the item to become stale.
    println!("\nWaiting 3 seconds for item to become stale...");
    thread::sleep(Duration::from_secs(3));

    // 3. Request the stale item. It is returned immediately.
    let value2 = cache.fetch_with(&"my-data".to_string());
    println!("IMMEDIATELY Received (stale): {}", *value2);
    assert_eq!(*value2, "Version 1");
    println!("A background refresh has been triggered.");
    
    // 4. Wait for the refresh to complete and request again.
    thread::sleep(Duration::from_millis(100)); // Give refresh time to finish.
    let value3 = cache.fetch_with(&"my-data".to_string());
    println!("Received (refreshed): {}", *value3);
    assert_eq!(*value3, "Version 2");
    assert_eq!(load_counter.load(Ordering::Relaxed), 2);
}
```

## Configuration with `CacheBuilder`

All caches must be configured and created via the `CacheBuilder`. It provides a fluent API for setting up all aspects of the cache.

**Primary Struct:** `fibre_cache::CacheBuilder<K, V, H>`

**Common Configuration Methods:**
*   `.capacity(u64)`: Sets the maximum total cost of all items in the cache. Activates the eviction policy.
*   `.unbounded()`: Creates a cache with no capacity limit. This is the default.
*   `.time_to_live(Duration)`: Sets a global Time-to-Live for all items.
*   `.time_to_idle(Duration)`: Sets a global Time-to-Idle for all items.
*   `.stale_while_revalidate(Duration)`: Configures a grace period after TTL for serving stale data while refreshing it in the background. Requires a loader.
*   `.cache_policy_factory(impl Fn)`: Sets a custom eviction policy via a factory function. See [Eviction Policies](#eviction-policies).
*   `.loader(impl Fn)`: Sets a synchronous loader function for `Cache::fetch_with`.
*   `.async_loader(impl Fn)`: Sets an asynchronous loader function for `AsyncCache::fetch_with`.
*   `.eviction_listener(impl EvictionListener)`: Registers a callback for item evictions. See [Eviction Listener](#eviction-listener).
*   `.shards(usize)`: Sets the number of concurrent shards (will be rounded to the next power of two). Defaults to `num_cpus * 8`.

**Build Methods:**
*   `.build() -> Result<Cache, BuildError>`: Creates a synchronous `Cache`.
*   `.build_async() -> Result<AsyncCache, BuildError>`: Creates an asynchronous `AsyncCache`.

## Main API: `Cache` and `AsyncCache`

These are the primary handles for interacting with the cache. Their APIs are largely symmetric, with `async` versions of methods in `AsyncCache`.

### Core Operations

*   `get<Q, F, R>(&self, key: &Q, f: F) -> Option<R>`
    *   The most efficient read method. Looks up an entry and, if found, applies the closure `f` to the value *without* cloning the value's `Arc`. The closure is executed while a read lock is held on the shard, so it should be fast.
    *   **Note**: This method uses a generic `&Q` key, allowing you to use borrowed types for lookups (e.g., `&str` for a `String` key).
*   `fetch<Q>(&self, key: &Q) -> Option<Arc<V>>`
    *   Retrieves a clone of the `Arc` containing the value if the key exists and is not expired. Also uses the `&Q` pattern for efficient lookups.
*   `peek<Q>(&self, key: &Q) -> Option<Arc<V>>`
    *   Retrieves a value without updating its recency or access time. This is useful for inspecting the cache without affecting the eviction policy's state (e.g., LRU order). Also uses the `&Q` pattern.
*   `insert(&self, key: K, value: V, cost: u64)`
    *   Inserts a key-value-cost triple into the cache. If the key already exists, its value and cost are updated. This method takes an owned `K`.
*   `insert_with_ttl(&self, key: K, value: V, cost: u64, ttl: Duration)`
    *   Inserts an item with a specific TTL that overrides the cache's global default. Takes an owned `K`.
*   `invalidate<Q>(&self, key: &Q) -> bool`
    *   Removes an item from the cache, returning `true` if the item was present. Uses the `&Q` pattern.
*   `clear(&self)`
    *   Removes all items from the cache. This is a "stop-the-world" operation that locks all shards.
*   `metrics(&self) -> MetricsSnapshot`
    *   Returns a point-in-time snapshot of the cache's performance metrics.

### Loader-Based Operations

*   `fetch_with(&self, key: &K) -> Arc<V>`
    *   Retrieves a value for the given key. If the key is not present, it executes the configured synchronous or asynchronous loader to compute, insert, and return the value. This method provides **thundering herd protection**.
    *   **Note**: This method takes an `&K` because if the item is not found, an owned `K` will need to be cloned to pass to the loader function.

### Atomic Operations

*   `entry(&self, key: K) -> Entry<'_, K, V, H>`
    *   Provides an API similar to `std::collections::HashMap::entry` for atomic "get-or-insert" operations. The returned `Entry` enum can be either `Occupied` or `Vacant`. Takes an owned `K`.
*   `compute<Q, F>(&self, key: &Q, f: F) -> bool`
    *   Atomically applies a closure `f` to a mutable reference of a value. Returns `true` if the key existed and the computation was applied. **Warning**: This method waits (by yielding the thread/task) until it can get exclusive access. If another part of your code holds an `Arc` to the value indefinitely (preventing the `Arc`'s reference count from becoming 1), this method will loop forever, creating a **livelock**.
*   `try_compute<Q, F>(&self, key: &Q, f: F) -> Option<bool>`
    *   The non-waiting version of `compute`. It attempts to apply the closure once. Returns `Some(true)` on success, `Some(false)` if exclusive access could not be obtained (e.g., another `Arc` to the value exists), and `None` if the key was not found.
*   `compute_val<Q, F, R>(&self, key: &Q, mut f: F) -> ComputeResult<R>`
    *   A waiting version of `try_compute_val`. Atomically applies a closure `f` that returns a value of type `R`. Returns `ComputeResult::Ok(R)` on success or `ComputeResult::NotFound` if the key does not exist. **Warning**: This can livelock for the same reason as `compute`.
*   `try_compute_val<Q, F, R>(&self, key: &Q, f: F) -> ComputeResult<R>`
    *   A non-waiting version of `compute_val`. It attempts to apply the closure once and returns `ComputeResult::Ok(R)` on success, `ComputeResult::Fail` if exclusive access could not be obtained, or `ComputeResult::NotFound` if the key was not found.

### Bulk Operations

*(Requires the `bulk` feature flag)*
These methods are more efficient than calling their singular counterparts in a loop, as they parallelize work across shards.

*   `multiget<I, Q>(&self, keys: I) -> HashMap<K, Arc<V>>`
*   `multi_insert<I>(&self, items: I)`
*   `multi_invalidate<I, Q>(&self, keys: I)`

## Advanced Features

### Eviction Policies

You can provide a custom eviction policy to the `CacheBuilder`. The library provides several high-performance implementations, each with different trade-offs:

*   **`TinyLfuPolicy`**: (Default for bounded caches) A sophisticated policy that protects frequently used items from being evicted by infrequently used items, making it scan-resistant. **Best for general-purpose workloads.**
*   **`SievePolicy`**: A very recent, simple, and high-performance policy that provides excellent scan resistance with minimal memory overhead. A strong alternative to TinyLFU for its simplicity and performance.
*   **`ArcPolicy`**: Adaptive Replacement Cache. A sophisticated policy that balances between recency (LRU) and frequency (LFU) to offer high hit rates under mixed workloads.
*   **`SlruPolicy`**: Segmented LRU. Divides the cache into probationary and protected segments to provide better scan resistance than plain LRU.
*   **`LruPolicy`**: Classic Least Recently Used. Simple and effective for workloads without large, infrequent scans.
*   **`FifoPolicy`**: First-In, First-Out. Evicts items in the order they were inserted, ignoring access patterns.
*   **`ClockPolicy`**: An approximation of LRU that offers better performance under high concurrency by avoiding list manipulations on access.
*   **`RandomPolicy`**: Evicts a random item when at capacity. (Requires `random` feature).

### Eviction Listener

You can observe all item removals by implementing the `EvictionListener` trait and passing it to the builder.

**Trait:** `fibre_cache::listener::EvictionListener<K, V>`
*   `fn on_evict(&self, key: K, value: Arc<V>, reason: EvictionReason)`
    *   This method is called on a dedicated background thread with the key, value, and reason for removal (`Capacity`, `Expired`, or `Invalidated`). This ensures listener logic never blocks cache operations.

### Persistence (Snapshots)

*(Requires the `serde` feature flag)*
The library provides a mechanism to save and restore the cache's state.

*   `to_snapshot(&self) -> CacheSnapshot<K, V>`
    *   A method on `Cache` and `AsyncCache` that creates a serializable representation of all non-expired items. This is a "stop-the-world" operation that briefly locks all shards.
*   `CacheBuilder::build_from_snapshot(snapshot: CacheSnapshot)`
    *   A method on the builder to construct a new cache and pre-populate it with data from a snapshot.

## Error Handling

The only place where errors can occur in the public API is during the construction of the cache.

**Enum:** `fibre_cache::error::BuildError`
*   `ZeroCapacity`: Returned if a bounded cache is configured with a capacity of zero.
*   `ZeroShards`: Returned if the cache is configured with zero shards.
*   `SpawnerRequired`: Returned if an `async_loader` is used without providing a `TaskSpawner` or