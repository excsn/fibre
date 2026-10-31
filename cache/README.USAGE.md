# Usage Guide: `fibre_cache`

This guide covers configuring a `fibre_cache` instance and using its sync and async handles, its loaders, its expiry and its eviction policies.

## Table of Contents

* [Core Concepts](#core-concepts)
* [Quick Start](#quick-start)
    * [Reading with get and fetch](#reading-with-get-and-fetch)
    * [Synchronous Cache with TTL](#synchronous-cache-with-ttl)
    * [Asynchronous Cache with Loader](#asynchronous-cache-with-loader)
    * [Self-Populating Cache with TTL](#self-populating-cache-with-ttl)
    * [Cache with Stale-While-Revalidate](#cache-with-stale-while-revalidate)
* [Configuring a Cache](#configuring-a-cache)
    * [Setting Builder Options](#setting-builder-options)
    * [Building the Handle](#building-the-handle)
* [Reading and Writing Entries](#reading-and-writing-entries)
    * [Looking Up with a Borrowed Key](#looking-up-with-a-borrowed-key)
    * [Using the Async Handle](#using-the-async-handle)
* [Loading Values on a Miss](#loading-values-on-a-miss)
* [Mutating a Value Atomically](#mutating-a-value-atomically)
* [Getting or Inserting with an Entry](#getting-or-inserting-with-an-entry)
* [Operating on Many Keys](#operating-on-many-keys)
* [Iterating over a Cache](#iterating-over-a-cache)
* [Expiring Entries](#expiring-entries)
* [Running Maintenance](#running-maintenance)
* [Choosing an Eviction Policy](#choosing-an-eviction-policy)
* [Listening for Removals](#listening-for-removals)
* [Saving and Restoring Snapshots](#saving-and-restoring-snapshots)
* [Reading Metrics](#reading-metrics)
* [Design and Comparison](#design-and-comparison)
    * [Motivation](#motivation)
    * [Core Philosophy](#core-philosophy)
    * [Architectural Principles](#architectural-principles)
    * [Key Architectural Pillars](#key-architectural-pillars)
    * [Implemented Features](#implemented-features)
    * [Feature Comparison](#feature-comparison)
* [Error Handling](#error-handling)

## Core Concepts

* **`Cache`**: The handle with a synchronous, blocking API, cloned cheaply and sharing one underlying instance.
* **`AsyncCache`**: The handle with a non-blocking `async` API over the same kind of instance, reached through `build_async` or `to_async`.
* **Shard**: A partition of the cache's data with its own lock, policy and TTL wheel, so that threads on different keys rarely contend.
* **Cost**: A `u64` each item carries towards the capacity, commonly `1` where all items count equally.
* **Capacity**: The limit on total cost, split into an equal share per shard, with an insert evicting when its shard goes over its share.
* **Capacity at insert**: An insert hands the write to its shard's eviction policy under the shard lock and removes what the policy evicts before it returns.
* **Eviction Policy**: The logic deciding which items to evict, S3-FIFO by default for a bounded cache and W-TinyLFU with `tinylfu()`.
* **Janitor**: A background thread that refreshes the clock reads use and removes expired entries once per tick, 1 s by default. A cache with no TTL, TTI, capacity or custom policy starts it on the first `insert_with_ttl`.
* **TTL**: A deadline set when an entry is inserted, after which reads stop returning it and the janitor removes it.
* **TTI**: A deadline reset by each access, after which reads stop returning the entry and the janitor removes it.
* **Wheel**: The per-shard structure that tracks TTL timers and tells the janitor which shards have one due.
* **Loader**: A function run on a cache miss by `fetch_with`, with one load per key however many callers wait on it.
* **`Arc<V>` storage**: Values are stored in an `Arc`, so `fetch` returns a cheap shared reference and `V` does not need `Clone`.

## Quick Start

### Reading with get and fetch

Add `tokio` to your `Cargo.toml` for the async programs below: `tokio = { version = "1", features = ["full"] }`.

```rust
use fibre_cache::CacheBuilder;
use std::sync::Arc;
use std::time::Duration;

fn main() {
    // Create a cache with a maximum cost of 1000 and a 5-second TTL.
    // For bounded caches, the library defaults to S3-FIFO.
    let cache = CacheBuilder::<String, i32>::new()
        .capacity(1000)
        .time_to_live(Duration::from_secs(5))
        .build()
        .unwrap();

    // Insert a value. The `cost` is 1.
    cache.insert("key1".to_string(), 100, 1);

    // Retrieve the value using `fetch`, which returns an Arc<V>.
    // This is efficient as it only clones the Arc, not the value itself.
    let value: Arc<i32> = cache.fetch(&"key1".to_string()).unwrap();
    assert_eq!(*value, 100);

    // `get` accesses the value in place under the shard's read lock.
    let length = cache.get(&"key1".to_string(), |v| v.to_string().len()).unwrap();
    assert_eq!(length, 3); // length of "100"

    // Reads stop returning the value about 5 seconds in. The janitor thread removes it within about one wheel tick plus one janitor tick of the deadline.
}
```

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

    // The janitor's clock has passed the deadline, so the read returns None.
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

### Self-Populating Cache with TTL

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
    // The .fetch_with() method handles the thundering herd problem.
    let value1 = cache.fetch_with(&"my-key".to_string()).await;
    println!("Received: {}", value1);
    assert_eq!(*value1, "Value for my-key");

    // 2. Second access to "my-key": hit. The value is returned instantly.
    let value2 = cache.fetch_with(&"my-key".to_string()).await;
    println!("Received: {}", value2);
    assert_eq!(*value2, "Value for my-key");

    // Check the metrics
    let metrics = cache.metrics();
    assert_eq!(metrics.hits, 1);
    assert_eq!(metrics.misses, 1);
    println!("\nCache Metrics: {:#?}", metrics);
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

## Configuring a Cache

All caches are configured and created via `fibre_cache::CacheBuilder<K, V, H>`, a fluent API for every aspect of the cache.

### Setting Builder Options

```rust
use fibre_cache::CacheBuilder;
use std::time::Duration;

let cache = CacheBuilder::<String, i32>::new()
    .capacity(10_000)
    .shards(16)
    .time_to_live(Duration::from_secs(60))
    .janitor_tick_interval(Duration::from_millis(100))
    .build()
    .unwrap();
```

The common options:

*   `.capacity(u64)`: Sets the maximum total cost of all items in the cache. Activates the eviction policy.
*   `.unbounded()`: Creates a cache with no capacity limit. This is the default.
*   `.time_to_live(Duration)`: Sets a global Time-to-Live for all items.
*   `.time_to_idle(Duration)`: Sets a global Time-to-Idle for all items.
*   `.janitor_tick_interval(Duration)`: Sets how often the janitor wakes, defaulting to 1 s. With a TTI set it defaults to `time_to_idle / 2` capped at 1 s. A shorter tick tightens the read and removal bounds at the cost of more background work.
*   `.timer_tick_duration(Duration)`, `.timer_wheel_size(usize)` and `.timer_mode(TimerWheelMode)`: Set the per-shard TTL wheel, 1 s and 60 slots by default. Removal lags a deadline by up to one wheel tick plus one janitor tick. A wheel tick shorter than the janitor tick gains nothing beyond the janitor tick.
*   `.maintenance_chance(u32)`: Sets how often a janitor tick applies pending reads on each of two random shards. Inserts apply reads themselves. Capacity, TTL and TTI removal do not depend on it.
*   `.stale_while_revalidate(Duration)`: Configures a grace period after TTL during which `fetch_with` returns the stale value and starts a background refresh. `get`, `fetch` and `peek` return `None` once the deadline passes. The entry is removed and counted at the end of the grace period. At most `max_concurrent_refreshes` refreshes run at once, 32 by default. The rest wait in order, one per key. A miss past the grace period takes its key's refresh from the queue. Requires a loader.
*   `.cache_policy_factory(impl Fn)`: Sets a custom eviction policy via a factory function. See [Choosing an Eviction Policy](#choosing-an-eviction-policy).
*   `.s3fifo()`: Sets the eviction policy to S3-FIFO, run inside each shard. This is the default for a bounded cache.
*   `.tinylfu()`: Sets the eviction policy to W-TinyLFU, run inside each shard.
*   `.null_policy()`: Sets the eviction policy to the no-op `NullPolicy`, which never evicts. Shorthand for `.cache_policy_factory(|| Box::new(NullPolicy))`.
*   `.loader(impl Fn)`: Sets a synchronous loader function for `fetch_with` on either handle. On a `Cache` it runs on the calling thread. Stale refreshes and an `AsyncCache` run it on a pool of up to 64 threads per cache.
*   `.async_loader(impl Fn)`: Sets an asynchronous loader function for `fetch_with` on either handle.
*   `.eviction_listener(impl EvictionListener)`: Registers a callback for item evictions. See [Listening for Removals](#listening-for-removals).
*   `.shards(usize)`: Sets the number of concurrent shards (will be rounded to the next power of two). Defaults to `num_cpus * 8`, lowered for a bounded cache so that each shard holds at least 64 cost units: a capacity of 100 runs on one shard, 1000 on eight and with 128 default shards, 8192 and above runs on all of them. Fewer shards means concurrent reads on a small cache contend more and in exchange the capacity is less unevenly split. Calling `.shards()` switches the derivation off.
*   `.maintenance_on_introspection(bool)`: When `true`, introspection methods (`metrics()`, `iter()`, `iter_snapshot()`, `to_snapshot()` and their async equivalents) will flush all pending read-access events before returning. This ensures the eviction policy's state is fully current. On an `AsyncCache`, `metrics`, `iter_stream` and `iter_snapshot_async` skip a shard whose lock is held, since they cannot wait for it. `to_snapshot` awaits every shard. Disabled by default; most useful in tests or monitoring pipelines that need deterministic eviction metadata.

### Building the Handle

`.build()` returns a synchronous `Cache` and `.build_async()` an asynchronous `AsyncCache`. Each returns `Result<_, BuildError>`. `to_async` and `to_sync` convert between them with an `Arc` clone.

```rust
use fibre_cache::CacheBuilder;

let sync_cache = CacheBuilder::<String, i32>::new().build().unwrap();
let async_cache = CacheBuilder::<String, i32>::new().build_async().unwrap();

let same_as_sync = sync_cache.to_async();
let same_as_async = async_cache.to_sync();
```

## Reading and Writing Entries

`get` is the read that hands back no `Arc`: it applies the closure to the value under the shard's read lock, so the closure should be fast. `fetch` retrieves a clone of the `Arc` if the key exists and is not expired. Expiry is judged on the janitor's clock, so an entry can still be returned for up to one janitor tick past its deadline. `peek` retrieves a value without updating its recency or access time, which is useful for inspecting the cache without affecting the policy's state.

```rust
use fibre_cache::CacheBuilder;
use std::sync::Arc;
use std::time::Duration;

let cache = CacheBuilder::<String, i32>::new().capacity(1000).build().unwrap();

cache.insert("a".to_string(), 1, 1);
cache.insert_with_ttl("b".to_string(), 2, 1, Duration::from_secs(60));

let doubled: Option<i32> = cache.get("a", |v| v * 2);
let value: Option<Arc<i32>> = cache.fetch("a");
let unaffected: Option<Arc<i32>> = cache.peek("a");

let removed: Option<Arc<i32>> = cache.remove("a");
let was_present: bool = cache.invalidate("b");
cache.clear();
```

`insert` takes an owned `K`; if the key already exists, its value and cost are updated. `insert_with_ttl` overrides the cache's global default. Reads honour the TTL and the janitor removes the entry at expiry.

`remove` returns the stored value and `invalidate` returns whether it was present. Both notify the eviction listener with `EvictionReason::Invalidated`. `clear` is a "stop-the-world" operation that locks all shards.

### Looking Up with a Borrowed Key

Lookup methods such as `get`, `fetch`, `peek`, `remove` and `invalidate` accept `&Q`, so a `String` key can be looked up with a `&str` and no allocation.

```rust
let cache: fibre_cache::Cache<String, i32> = fibre_cache::CacheBuilder::new().build().unwrap();
cache.insert("my-key".to_string(), 123, 1);

// We can fetch using a &str, no need to create a new String.
let value = cache.fetch("my-key").unwrap();
assert_eq!(*value, 123);
```

Retrieval methods return `Arc<V>`, so multiple threads can hold the same cached value without `V` being `Clone` and only the `Arc`'s reference count changes.

### Using the Async Handle

The async handle has the same methods with `.await`.

```rust
use fibre_cache::CacheBuilder;

#[tokio::main]
async fn main() {
    let cache = CacheBuilder::<String, i32>::new().capacity(1000).build_async().unwrap();

    cache.insert("a".to_string(), 1, 1).await;
    let doubled = cache.get("a", |v| v * 2).await;
    assert_eq!(doubled, Some(2));
    assert!(cache.invalidate("a").await);
}
```

## Loading Values on a Miss

`fetch_with` takes `&K`, because a miss clones the key to pass to the loader. It runs the configured synchronous or asynchronous loader to compute, insert and return the value, with **thundering herd protection**. A miss panics with no loader configured. A loader that panics panics the caller and every waiter on that key. So does an `async_loader` whose task is dropped, as when its runtime shuts down. The next call loads again. A `TokioSpawner` keeps the runtime it was built on, so once that runtime is gone every miss panics. A sync loader calling `fetch_with` on its own key panics. A loader cycle across threads (A loads B loads A) is not detected and hangs.

```rust
use fibre_cache::CacheBuilder;

let cache = CacheBuilder::default()
    .capacity(100)
    .loader(|key: String| (key.len(), 1))
    .build()
    .unwrap();

let len = cache.fetch_with(&"hello".to_string());
assert_eq!(*len, 5);
```

The [Quick Start](#quick-start) has the async loader and stale-while-revalidate programs.

## Mutating a Value Atomically

`compute` applies a closure to a mutable reference and returns whether the key existed. **Warning**: it waits (by yielding the thread or task) until it can get exclusive access. If another part of your code holds an `Arc` to the value indefinitely, it loops forever, creating a **livelock**. `compute_val` is the waiting version that returns a value and can livelock for the same reason.

`try_compute` and `try_compute_val` try once. Prefer them when a caller may hold an `Arc` to the value. An expired entry not yet removed counts as present for all four.

```rust
use fibre_cache::CacheBuilder;
use fibre_cache::error::ComputeResult;

let cache = CacheBuilder::<String, i32>::new().build().unwrap();
cache.insert("n".to_string(), 1, 1);

let applied: bool = cache.compute("n", |v| *v += 1);
assert!(applied);

// Some(true) on success, Some(false) if another Arc exists, None if the key is absent.
let outcome: Option<bool> = cache.try_compute("n", |v| *v += 1);
assert_eq!(outcome, Some(true));

match cache.try_compute_val("n", |v| *v * 10) {
    ComputeResult::Ok(n) => println!("result {n}"),
    ComputeResult::Fail => println!("another Arc to the value exists"),
    ComputeResult::NotFound => println!("no such key"),
}
```

## Getting or Inserting with an Entry

`entry` works like `std::collections::HashMap::entry` and holds the shard's write lock until the returned value is dropped. It takes an owned `K`. An expired entry not yet removed counts as occupied.

```rust
use fibre_cache::{CacheBuilder, Entry};

let cache = CacheBuilder::<String, i32>::new().build().unwrap();

let a = cache.entry("a".to_string()).or_insert(1, 1);
let b = cache.entry("b".to_string()).or_insert_with(|| 2, 1);
assert_eq!((*a, *b), (1, 2));

match cache.entry("c".to_string()) {
    Entry::Occupied(o) => println!("{} is {}", o.key(), o.get()),
    Entry::Vacant(v) => {
        v.insert(3, 1);
    }
}
```

`AsyncCache::entry` is awaited and the rest is the same: `cache.entry(key).await.or_insert(1, 1)`.

## Operating on Many Keys

*(Requires the `bulk` feature flag, except `AsyncCache::multiget`)*

These methods are more efficient than calling their singular counterparts in a loop, as they parallelize work across shards.

```rust
use fibre_cache::CacheBuilder;

let cache = CacheBuilder::<u64, String>::new().build().unwrap();

cache.multi_insert(vec![(1, "one".to_string(), 1), (2, "two".to_string(), 1), (3, "three".to_string(), 1)]);

let found = cache.multiget(vec![1u64, 2, 99]);
assert_eq!(found.len(), 2);

let removed = cache.multi_remove(vec![1u64]);
assert_eq!(removed.len(), 1);

cache.multi_invalidate(vec![2u64]);
```

## Iterating over a Cache

`iter` is weakly consistent and holds one shard's lock at a time for each batch. `iter_snapshot` fixes each shard's key set when it reaches that shard. Pick `iter` for low impact and `iter_snapshot` for a more stable view.

```rust
use fibre_cache::CacheBuilder;

let cache = CacheBuilder::<u32, String>::new().build().unwrap();
cache.insert(1, "one".to_string(), 1);

for (key, value) in cache.iter() {
    println!("{key}: {value}");
}
for (key, value) in cache.iter_with_batch_size(256) {
    println!("{key}: {value}");
}
for (key, value) in cache.iter_snapshot() {
    println!("{key}: {value}");
}
```

The async handle returns a `Stream` from `iter_stream` and an iterator with an async `next` from `iter_snapshot_async`. This example needs `futures-util` as a dependency for `StreamExt`.

```rust
use fibre_cache::CacheBuilder;
use futures_util::stream::StreamExt;

#[tokio::main]
async fn main() {
    let cache = CacheBuilder::<u32, String>::new().build_async().unwrap();
    cache.insert(1, "one".to_string(), 1).await;

    let mut stream = cache.iter_stream();
    while let Some((key, value)) = stream.next().await {
        println!("{key}: {value}");
    }

    let mut snapshot = cache.iter_snapshot_async();
    while let Some((key, value)) = snapshot.next().await {
        println!("{key}: {value}");
    }
}
```

## Expiring Entries

A background thread removes expired entries, waking once per tick (1 s by default). Each tick it sweeps every shard with a TTL timer due, so a TTL-expired entry leaves within about one wheel tick plus one janitor tick of its deadline and `maintenance_chance` does not gate the sweep. Each tick also scans every shard for idle entries when a TTI is set, at about 1.3% of a core per million entries with a 1 s tick on an M4 Pro. An idle entry stays at least its TTI after its last read and leaves within about two clock refreshes plus one janitor tick of it, where the clock refreshes each tick and at least every TTI / 2.

Reads check expiry against a clock the janitor refreshes once per tick, so `get`, `fetch` and `peek` can return an entry for up to one tick after its TTL deadline and up to two refreshes after its TTI deadline. `iter()`, `iter_stream()` and `to_snapshot()` use the exact clock. `iter_snapshot()` reads through `fetch`, so it has the one-tick bound, records hits and misses and resets TTI. `janitor_tick_interval` tightens all of these bounds.

Neither TTL nor TTI is set by default. The defaults for the rest are a 1 s janitor tick and a wheel of 60 slots at 1 s. Change them on the builder and read the results from the metrics:

```rust
use fibre_cache::CacheBuilder;
use fibre_cache::builder::TimerWheelMode;
use std::time::Duration;

let cache = CacheBuilder::<String, i32>::new()
    .time_to_live(Duration::from_millis(200))
    .time_to_idle(Duration::from_millis(100))
    .timer_mode(TimerWheelMode::HighPrecisionShortLived)
    .janitor_tick_interval(Duration::from_millis(10))
    .build()
    .unwrap();

cache.insert("a".to_string(), 1, 1);

let metrics = cache.metrics();
println!("ttl {} tti {}", metrics.evicted_by_ttl, metrics.evicted_by_tti);
```

## Running Maintenance

Capacity is enforced inside `insert`, so no maintenance pass is needed for it. Logged reads reach the eviction policy before the next write to their shard. A background janitor thread removes TTL-expired entries from every shard with a timer due on each tick and scans every shard for idle entries. For scenarios that need the policy's view of reads to be current right now, primarily tests, two explicit maintenance options are provided.

`run_maintenance` refreshes the clock reads use, applies every pending read to the policy on each shard and removes every entry past its TTL or TTI with a full scan. An entry in its stale-while-revalidate grace stays. On an `AsyncCache` it awaits each shard's locks.

`maintenance_on_introspection` is a builder option. When enabled, the cache automatically runs a partial flush, draining pending read-access events, before every call to `metrics()`, `iter()`, `iter_snapshot()`, `to_snapshot()` and their async equivalents. This ensures that the eviction policy's internal state (access frequencies, recency order) reflects all reads that have been performed. On an `AsyncCache`, `metrics`, `iter_stream` and `iter_snapshot_async` skip a shard whose lock is held, since they cannot wait for it. `to_snapshot` awaits every shard.

| | `run_maintenance` | `maintenance_on_introspection` |
|---|---|---|
| Triggers | Explicit call | Automatic on each introspection |
| Scope | Reads, TTL and TTI | Read-access flush only |
| Primary use case | Deterministic test assertions | Monitoring / metrics pipelines |

```rust
use fibre_cache::CacheBuilder;

// Example: deterministic test with run_maintenance
let cache = CacheBuilder::<u32, &str>::new()
    .capacity(10)
    .tinylfu()
    .build()
    .unwrap();

cache.insert(1, "a", 1);
cache.insert(2, "b", 1);
cache.insert(3, "c", 1);
cache.fetch(&1); // logged for the policy, applied at the next write

// Apply the pending read before asserting on the policy's view.
cache.run_maintenance();
assert_eq!(cache.metrics().current_cost, 3);

// Example: auto-flush on metrics with builder flag
let cache = CacheBuilder::<u32, &str>::new()
    .capacity(10)
    .tinylfu()
    .maintenance_on_introspection(true)
    .build()
    .unwrap();

cache.insert(1, "a", 1);
cache.fetch(&1); // pending read event

// metrics() flushes the read event before returning the snapshot.
let m = cache.metrics();
assert_eq!(m.hits, 1);
```

## Choosing an Eviction Policy

You can provide a custom eviction policy to the `CacheBuilder`. The library provides several implementations, each with different trade-offs:

*   **`S3FifoPolicy`**: (Default for bounded caches) A small FIFO queue in front of a main FIFO queue with a ghost set of recently evicted keys; a key read more than once in the small queue moves to main. Cheaper per operation than TinyLFU with a similar hit ratio on skewed workloads.
*   **`TinyLfuPolicy`**: A sophisticated policy that protects frequently used items from being evicted by infrequently used items, making it scan-resistant. Select it with `.tinylfu()` on the builder. Its reads cost more than S3-FIFO's: each one is a frequency sketch update and a list move, applied under the shard's write lock before the next write.
*   **`SievePolicy`**: A very recent, simple and high-performance policy that provides excellent scan resistance with minimal memory overhead. A strong alternative to TinyLFU for its simplicity and performance.
*   **`ArcPolicy`**: Adaptive Replacement Cache. A sophisticated policy that balances between recency (LRU) and frequency (LFU) to offer high hit rates under mixed workloads.
*   **`SlruPolicy`**: Segmented LRU. Divides the cache into probationary and protected segments to provide better scan resistance than plain LRU.
*   **`LruPolicy`**: Classic Least Recently Used. Simple and effective for workloads without large, infrequent scans.
*   **`Fifo`**: First-In, First-Out. Evicts items in the order they were inserted, ignoring access patterns.
*   **`ClockPolicy`**: An approximation of LRU that offers better performance under high concurrency by avoiding list manipulations on access.
*   **`RandomPolicy`**: Evicts a random item when at capacity. (Requires `random` feature).

S3-FIFO and TinyLFU are also built into the cache: with no `cache_policy_factory` the cache runs the chosen one inside each shard on node handles instead of keys, which is faster than the same policy through the factory. Both forms make the same decision on every operation. S3-FIFO counts a read at once under the read lock. TinyLFU and factory policies log reads per shard and apply them, in order, before the next write to that shard.

The cache has no getter for its policy. The choice is made on the builder:

```rust
use fibre_cache::CacheBuilder;
use fibre_cache::policy::sieve::SievePolicy;

// S3-FIFO, the default for a bounded cache.
let cache = CacheBuilder::<u64, String>::new()
    .capacity(10_000)
    .build()
    .unwrap();

// W-TinyLFU instead.
let cache = CacheBuilder::<u64, String>::new()
    .capacity(10_000)
    .tinylfu()
    .build()
    .unwrap();

// Any other policy through the factory, called once per shard.
let cache = CacheBuilder::<u64, String>::new()
    .capacity(10_000)
    .cache_policy_factory(|| Box::new(SievePolicy::new()))
    .build()
    .unwrap();
```

## Listening for Removals

You can observe evictions, expirations and invalidations by implementing the `EvictionListener` trait and passing it to the builder. `clear()` and a replacing `insert` send nothing. Notifications go through a 128-slot channel and are dropped when it is full.

`on_evict` is called on a dedicated background thread with the key, value and reason for removal (`Capacity`, `Expired` or `Invalidated`). This ensures listener logic never blocks cache operations. There is no listener by default.

```rust
use fibre_cache::{CacheBuilder, EvictionListener, EvictionReason};
use std::sync::Arc;

struct LogRemovals;

impl EvictionListener<String, i32> for LogRemovals {
    fn on_evict(&self, key: String, value: Arc<i32>, reason: EvictionReason) {
        println!("{key} = {value}: {reason}");
    }
}

let cache = CacheBuilder::<String, i32>::new()
    .capacity(100)
    .eviction_listener(LogRemovals)
    .build()
    .unwrap();

cache.insert("a".to_string(), 1, 1);
cache.invalidate("a");
```

## Saving and Restoring Snapshots

*(Requires the `serde` feature flag)*

`to_snapshot` is a method on `Cache` and `AsyncCache` that creates a serializable `CacheSnapshot<K, V>` of all non-expired items. This is a "stop-the-world" operation that briefly locks all shards and it needs `V: Clone`. `build_from_snapshot` constructs a new cache and pre-populates it. Serialize the snapshot with any `serde` format.

```rust
use fibre_cache::CacheBuilder;

let cache = CacheBuilder::<String, i32>::new().capacity(100).build().unwrap();
cache.insert("a".to_string(), 1, 1);

let snapshot = cache.to_snapshot();

let restored = CacheBuilder::<String, i32>::new()
    .build_from_snapshot(snapshot)
    .unwrap();
assert_eq!(*restored.fetch("a").unwrap(), 1);
```

## Reading Metrics

`metrics` returns a point-in-time `MetricsSnapshot`. If `maintenance_on_introspection` is enabled, pending read-access events are flushed first so the eviction policy state is current.

```rust
use fibre_cache::CacheBuilder;

let cache = CacheBuilder::<String, i32>::new().capacity(100).build().unwrap();
cache.insert("a".to_string(), 1, 1);
cache.fetch("a");

let metrics = cache.metrics();
assert_eq!(metrics.hits, 1);
assert_eq!(metrics.current_cost, 1);
println!("{:#?}", metrics);
```

## Design and Comparison

`fibre_cache` offers a best-in-class, most flexible, high-performance, concurrent, multimode sync/async cache for Rust. It is built from the heart and soul of a Java-turned-Rust engineer to bring the comprehensive, flexible and ergonomic "it just works" feeling of top-tier Java caches (like Caffeine) to the Rust ecosystem.

### Motivation

The current state of caching in Rust, while offering some excellent high-performance options, often feels specialized and inflexible. Caches can be brilliant but opinionated, while others lack the rich, general-purpose feature set needed for complex applications.

`fibre_cache` was born from the desire for a cache that is **both incredibly fast and uncompromisingly flexible**. It aims to be the definitive, general-purpose caching library for Rust, providing a complete toolset that feels both powerful to experts and intuitive to newcomers.

### Core Philosophy

*   **No Compromises:** Get the raw speed of a systems language without sacrificing the rich, ergonomic APIs you love.
*   **Flexibility First:** From swappable eviction policies to runtime-agnostic async loaders, the cache adapts to your needs, not the other way around.
*   **Performance by Design:** Built from first principles with a sharded, hybrid-locking concurrent core to minimize contention and maximize throughput.
*   **Idiomatic Rust:** Embrace Rust's strengths, like the ownership model, to provide features impossible in other languages, such as first-class support for non-`Clone` values.

### Architectural Principles

The primary entry points are two handle types. `Cache<K, V, H>` is for synchronous, blocking operations in standard, multi-threaded applications. `AsyncCache<K, V, H>` is for asynchronous, non-blocking operations compatible with any Rust async runtime (e.g., Tokio, async-std). Both are lightweight wrappers around a shared cache core and converting between them via `.to_sync()` or `.to_async()` is a zero-cost operation (an `Arc` clone).

*   **Builder Pattern**: Caches are created exclusively through the `CacheBuilder`, which provides a fluent API for configuration.
*   **Sharded Concurrency**: The cache is internally partitioned into multiple "shards," each with its own lock, eviction policy and timer wheel. This design minimizes lock contention and allows for high-throughput concurrent access.
*   **Capacity at insert**: An insert evicts what it must before it returns, so its shard is within its share of the capacity when the insert returns. `null_policy()` lets the total exceed `capacity`. So does `shards()` with more shards than `capacity` allows, since each shard keeps a share of at least 1. A background "janitor" thread expires items by Time-to-Live (TTL) or Time-to-Idle (TTI).
*   **Pluggable Policies**: Eviction logic is abstracted via the `CachePolicy` trait. A bounded cache runs S3-FIFO by default, TinyLFU with `tinylfu()` or any policy given through `cache_policy_factory`.

### Key Architectural Pillars

`fibre_cache` combines the best features from across the caching landscape into one comprehensive package.

*   **High Concurrency via Sharding:** Partitions data across independently locked shards to minimize contention.
*   **Hybrid Locking:** A custom `HybridRwLock` provides raw, blocking speed for sync operations and true, non-blocking suspension for async operations.
*   **No `V: Clone` Required:** Stores values in an `Arc<V>`, making `V: Clone` unnecessary for most operations-a core advantage over many other Rust caches.
*   **Unified Sync/Async API:** Zero-cost `to_sync()` and `to_async()` methods allow seamless conversion between handles.
*   **Trait-Based Policies:** A flexible `CachePolicy` trait allows for swapping out admission and eviction algorithms.

### Implemented Features

| Feature | Details |
| :--- | :--- |
| **Atomic `entry` API** | A fully atomic get-or-insert API mimicking `HashMap`, safe for concurrent use in both sync and async contexts. |
| **`CacheLoader`** | Self-populating cache via `fetch_with`. Supports both sync (`loader`) and async (`async_loader`) functions. |
| **Thundering Herd Protection**| Guarantees a loader is executed only once for concurrent misses on the same key, preventing duplicate work. |
| **Runtime Agnostic** | The `async_loader` can be used with any async runtime (`tokio`, `async-std`, etc.) via a `TaskSpawner` trait, as long as the `tokio` feature stays enabled for the crate to build. |
| **Stale-While-Revalidate** | Can serve stale data for a configured grace period while refreshing it in the background, hiding latency. |
| **Comprehensive Policies** | Includes **TinyLfu**, **S3-FIFO**, **SIEVE**, **SLRU**, **ARC**, **LRU**, **FIFO**, **Clock** and **Random** eviction/admission policies. |
| **Bulk Operations** | Efficient, parallelized `multiget`, `multi_insert` and `multi_invalidate` methods (via `bulk` feature). |
| **In-place Mutation** | A `compute` method for safe, atomic mutation of existing values without cloning. |
| **Eviction Listener** | Receive non-blocking notifications for evictions, expirations and invalidations on a dedicated background thread. |
| **Persistence (`serde`)**| A default `serde` feature provides a flexible `to_snapshot()` and `build_from_snapshot()` API. |
| **Metrics & Stats** | A zero-dependency, queryable metrics system for deep observability. |

### Feature Comparison

| Feature Group | Feature | Moka / Caffeine | Stretto / Ristretto | **`fibre_cache`** | Notes |
| :--- | :--- | :--- | :--- | :--- | :--- |
| **Core Architecture** | **Concurrency Model** | Sharded | Sharded | ✅ **Sharded, Custom Lock** | Custom hybrid lock with distinct, optimized paths for sync and async. |
| | **Sync/Async Interoperability** | Separate Caches | ✅ | ✅ **Zero-Cost Handles** | A single cache instance can be accessed via both sync and async handles seamlessly. |
| | **`V: Clone` Required?** | **Yes** | No | ✅ **No** | `Arc<V>` storage natively supports non-cloneable values, a major ergonomic win. |
| | **Maintenance Model** | Background Thread | Sampling-based | ✅ **Eviction at insert** | Capacity is enforced inside the insert; a janitor thread handles expiry. |
| | **Read Delivery to the Policy**| Event Queue | Buffer-based | ✅ **Per-shard log** | Reads are logged per shard in order and applied under the shard's write lock; S3-FIFO counts them on the entry. |
| | | | | | |
| **Eviction & Policies**| **Cost-Based Eviction** | ✅ | ✅ | ✅ | Items can have individual `cost` towards `capacity`. |
| | **Global TTL / TTI** | ✅ | ✅ | ✅ | Cache-wide Time-to-Live and Time-to-Idle. |
| | **Per-Item TTL** | ✅ | ❌ | ✅ | `insert_with_ttl` provides fine-grained expiration control. |
| | **Timer System for Expirations**| Hierarchical Wheel | ❌ (Sampling) | ✅ **Configurable Per-Shard Wheel** | TTL removal through a per-shard wheel, tuned via `TimerWheelMode`. TTI removal scans each shard each tick. |
| | **Pluggable Policies** | Limited | ❌ | ✅ **Highly Flexible** | A `CachePolicy` trait allows for easy implementation of custom strategies. |
| | **Available Policies** | Segmented-LRU | TinyLFU | ✅ **Most Comprehensive** | **TinyLFU**, **S3-FIFO**, **ARC**, **SLRU**, **SIEVE**, **LRU**, **FIFO**, **Clock** and **Random**. |
| | | | | | |
| **API & Ergonomics** | **`entry` API** | ✅ | ❌ | ✅ | Atomic "get-or-insert" for both sync and async. |
| | **Read API Ergonomics** | `get` (clones Arc) | `get` (returns ref) | ✅ **`get` & `fetch`** | Offers both a closure-based `get` and an `Arc`-returning `fetch`. |
| | **`compute`** | ✅ | ❌ | ✅ | Safe, atomic, in-place mutation of values. |
| | **Bulk Operations** | ✅ | ✅ | ✅ | Parallelized `multiget`, `multi_insert`, etc. (via `bulk` feature) |
| | | | | | |
| **Advanced Patterns**| **`CacheLoader`** | ✅ | ❌ | ✅ **Runtime Agnostic** | `TaskSpawner` trait makes it work with any async runtime. |
| | **Thundering Herd Protection** | ✅ | N/A | ✅ | Built into `fetch_with` to prevent duplicate work. |
| | **Stale-While-Revalidate** | ✅ | ❌ | ✅ | Hides refresh latency by serving stale data. |
| | | | | | |
| **Production Features**| **Eviction Listener** | ✅ | ✅ | ✅ | Decoupled notifications on a dedicated background thread to prevent blocking. |
| | **Metrics & Stats** | ✅ | ✅ | ✅ | Detailed, contention-free metrics via `MetricsSnapshot`. |
| | **Persistence (`serde`)** | ❌ | ❌ | ✅ **Serializable Snapshot** | `to_snapshot()` and `build_from_snapshot()` API (via `serde` feature). |

## Error Handling

The only place where errors can occur in the public API is during the construction of the cache. The error type is `fibre_cache::error::BuildError`, also exported at the crate root.

*   `ZeroCapacity`: Returned if a bounded cache is configured with a capacity of zero.
*   `ZeroShards`: Returned only when a snapshot holds zero shards. `.shards(0)` gives one shard.
*   `SpawnerRequired`: Declared for an `async_loader` with no `TaskSpawner` and no `tokio` feature. The crate currently needs the `tokio` feature to compile, so this variant is not returned today.

```rust
use fibre_cache::{BuildError, CacheBuilder};

match CacheBuilder::<String, i32>::new().capacity(0).build() {
    Ok(cache) => cache.insert("a".to_string(), 1, 1),
    Err(BuildError::ZeroCapacity) => eprintln!("capacity must be above zero"),
    Err(BuildError::ZeroShards) => eprintln!("the snapshot holds no shards"),
    Err(BuildError::SpawnerRequired) => eprintln!("an async loader needs a spawner"),
}
```
