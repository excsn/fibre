# API Reference: `fibre_cache`

### 1. Introduction & Core Concepts

This library provides a high-performance, concurrent, thread-safe caching solution for Rust, with first-class support for both synchronous and asynchronous applications.

#### Getting Started

```rust
use fibre_cache::CacheBuilder;
use std::time::Duration;
use std::sync::Arc;

// Create a cache with a maximum cost of 1000, and a 5-second TTL.
// For bounded caches, the library defaults to a high-performance TinyLFU eviction policy.
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

// For the most efficient read, use `get` with a closure.
// This avoids cloning the Arc and accesses the value in place.
let length = cache.get(&"key1".to_string(), |v| v.to_string().len()).unwrap();
assert_eq!(length, 3); // length of "100"

// After 5 seconds, a background task will remove the expired value.
```

#### Core Handles

The primary entry points for interacting with the cache are two handle types:

*   **`Cache<K, V, H>`**: A handle for synchronous, blocking operations. Ideal for use in standard, multi-threaded applications.
*   **`AsyncCache<K, V, H>`**: A handle for asynchronous, non-blocking operations compatible with any Rust async runtime (e.g., Tokio, async-std).

Both handles are lightweight wrappers around a shared cache core, and converting between them via `.to_sync()` or `.to_async()` is a zero-cost operation (an `Arc` clone).

#### Architectural Principles

*   **Builder Pattern**: Caches are created exclusively through the `CacheBuilder`, which provides a fluent API for configuration.
*   **Sharded Concurrency**: The cache is internally partitioned into multiple "shards," each with its own lock, eviction policy, and timer wheel. This design minimizes lock contention and allows for high-throughput concurrent access.
*   **Background Maintenance**: A background "janitor" thread is responsible for all periodic cleanup, including enforcing capacity and expiring items based on Time-to-Live (TTL) or Time-to-Idle (TTI). This keeps user-facing operations fast and predictable.
*   **Pluggable Policies**: Eviction logic is abstracted via the `CachePolicy` trait, with a sensible default (`TinyLfuPolicy`) for bounded caches.

#### Pervasive Patterns

*   **Key/Value Constraints**: To be stored in the cache, keys (`K`) and values (`V`) must satisfy certain trait bounds, which are most restrictive when all features are used.
    *   `K`: `Eq + Hash + Clone + Send + Sync + 'static`
    *   `V`: `Send + Sync + 'static` (Note: `V` does **not** require `Clone`).
*   **Cost**: Most insertion methods require a `cost` parameter (`u64`), which represents the "size" of the entry. For caches where all items are considered equal size, this can simply be `1`. The cache's `capacity` is the sum of the costs of all items.

---

### 2. Configuration (`CacheBuilder`)

All cache instances are created and configured using the `CacheBuilder`.

#### **struct** `CacheBuilder<K, V, H>`

The fluent builder for creating `Cache` and `AsyncCache` instances.

##### **Constructors**

*   `pub fn new() -> Self`
    *   *where `H: BuildHasher + Default`*
    *   Creates a new builder with default settings (unbounded capacity, `ahash` hasher).
*   `pub fn default() -> Self`
    *   *where `K: Send, V: Send`, `H` is `ahash::RandomState`*
    *   The standard `default` constructor.
*   `pub fn rapidhash() -> Self`
    *   *(feature: `rapidhash`)*
    *   *where `H` is `rapidhash::RapidRandomState`*
    *   A convenience constructor for using the `rapidhash` hasher.

##### **Configuration Methods**

*   `pub fn capacity(mut self, capacity: u64) -> Self`
    *   Sets the maximum total cost of the cache. If set, an eviction policy will be active.
*   `pub fn unbounded(mut self) -> Self`
    *   Sets the cache to have "unbounded" capacity (`u64::MAX`). This is the default.
*   `pub fn shards(mut self, shards: usize) -> Self`
    *   Sets the number of concurrent shards. The value is rounded up to the next power of two. Defaults to a value based on CPU count.
*   `pub fn time_to_live(mut self, duration: Duration) -> Self`
    *   Sets a default time-to-live (TTL) for all entries in the cache.
*   `pub fn time_to_idle(mut self, duration: Duration) -> Self`
    *   Sets a default time-to-idle (TTI) for all entries. An entry is evicted if it's not accessed for this duration.
*   `pub fn stale_while_revalidate(mut self, duration: Duration) -> Self`
    *   Sets a duration after an entry's TTL expires during which the cache can serve the stale value while asynchronously refreshing it in the background. Requires a `loader` to be configured.
*   `pub fn eviction_listener<Listener>(mut self, listener: Listener) -> Self`
    *   *where `Listener: EvictionListener<K, V> + 'static`*
    *   Registers a listener to receive notifications when entries are evicted. The listener is called on a dedicated background task.
*   `pub fn cache_policy_factory<F>(mut self, factory: F) -> Self`
    *   *where `F: Fn() -> Box<dyn CachePolicy<K, V>> + Send + Sync + 'static`*
    *   Sets a custom eviction policy via a factory function that is called for each shard. Defaults to `TinyLfuPolicy` for bounded caches and `NullPolicy` for unbounded ones.
*   `pub fn loader(mut self, f: impl Fn(K) -> (V, u64) + Send + Sync + 'static) -> Self`
    *   Sets the synchronous loader function, used by [`Cache::fetch_with`]. When `fetch_with` is called for a key that isn't in the cache, this closure is executed to compute its value and cost.
*   `pub fn async_loader<F, Fut>(mut self, f: F) -> Self`
    *   *where `F: Fn(K) -> Fut + Send + Sync + 'static`, `Fut: Future<Output = (V, u64)> + Send + 'static`*
    *   Sets the asynchronous loader function, used by [`AsyncCache::fetch_with`].
*   `pub fn spawner(mut self, spawner: Arc<dyn TaskSpawner>) -> Self`
    *   Provides a custom task spawner for the `async_loader`. Required if using an `async_loader` without the `tokio` feature enabled.
*   `pub fn hasher(mut self, hasher: H) -> Self`
    *   Sets a custom hasher for the cache.
*   `pub fn maintenance_chance(mut self, denominator: u32) -> Self`
    *   Sets the probability (`1 / denominator`) of running opportunistic maintenance on each `insert` operation. Use presets from the `maintenance_frequency` module for convenience (e.g., `maintenance_frequency::RESPONSIVE`).
*   `pub fn timer_mode(mut self, mode: TimerWheelMode) -> Self`
    *   Configures the internal expiration timer using a convenient preset (`Default`, `HighPrecisionShortLived`, `LowPrecisionLongLived`).
*   `pub fn timer_tick_duration(mut self, duration: Duration) -> Self`
    *   Sets the granularity (tick duration) of the internal expiration timer.
*   `pub fn timer_wheel_size(mut self, size: usize) -> Self`
    *   Sets the number of slots in the internal expiration timer wheel.

##### **Build Methods**

*   `pub fn build(mut self) -> Result<Cache<K, V, H>, BuildError>`
    *   *where `K: Eq + Hash + Clone + Send + Sync + 'static`, `V: Send + Sync + 'static`, `H: BuildHasher + Clone + Send + Sync + 'static`*
    *   Finalizes the configuration and builds the synchronous `Cache`.
*   `pub fn build_async(mut self) -> Result<AsyncCache<K, V, H>, BuildError>`
    *   *where `K: Eq + Hash + Clone + Send + Sync + 'static`, `V: Send + Sync + 'static`, `H: BuildHasher + Clone + Send + Sync + 'static`*
    *   Finalizes the configuration and builds the asynchronous `AsyncCache`.
*   `pub fn build_from_snapshot(mut self, snapshot: CacheSnapshot<K, V>) -> Result<Cache<K, V, H>, BuildError>`
    *   *(feature: `serde`)*
    *   *where `K: ... + DeserializeOwned`, `V: ... + DeserializeOwned + Clone`*
    *   Builds a sync cache and pre-populates it with entries from a previously created snapshot.
*   `pub fn build_from_snapshot_async(mut self, snapshot: CacheSnapshot<K, V>) -> Result<AsyncCache<K, V, H>, BuildError>`
    *   *(feature: `serde`)*
    *   *where `K: ... + DeserializeOwned`, `V: ... + DeserializeOwned + Clone`*
    *   Builds an async cache and pre-populates it from a snapshot.

---

### 3. Main Types and Public Methods

#### **struct** `Cache<K, V, H>`

A handle to the cache for synchronous operations.

*   `pub fn to_async(&self) -> AsyncCache<K, V, H>`
    *   Converts to an `AsyncCache` handle. This is a zero-cost `Arc` clone.
*   `pub fn metrics(&self) -> MetricsSnapshot`
    *   Returns a point-in-time snapshot of the cache's metrics.
*   `pub fn get<Q, F, R>(&self, key: &Q, f: F) -> Option<R>`
    *   *where `F: FnOnce(&V) -> R`*
    *   Looks up an entry and, if found, applies a closure to the value without cloning it. This is the most efficient read operation.
*   `pub fn fetch<Q>(&self, key: &Q) -> Option<Arc<V>>`
    *   Retrieves a clone of the `Arc` pointing to the value if the key is found and not expired.
*   `pub fn peek<Q>(&self, key: &Q) -> Option<Arc<V>>`
    *   Retrieves a value without updating its recency or access time (i.e., does not affect LRU, TTI, etc.).
*   `pub fn fetch_with(&self, key: &K) -> Arc<V>`
    *   Retrieves a value, using the configured synchronous `loader` if the key is not present. This method provides thundering herd protection.
*   `pub fn entry(&self, key: K) -> Entry<'_, K, V, H>`
    *   Creates a view into a specific entry, allowing for atomic "get-or-insert" operations.
*   `pub fn insert(&self, key: K, value: V, cost: u64)`
    *   Inserts a key-value-cost triple into the cache.
*   `pub fn insert_with_ttl(&self, key: K, value: V, cost: u64, ttl: Duration)`
    *   Inserts an entry with a specific TTL that overrides the cache's default.
*   `pub fn compute<Q, F>(&self, key: &Q, mut f: F) -> bool`
    *   *where `F: FnMut(&mut V)`*
    *   Atomically applies a closure to a mutable reference of a value. Returns `true` if the key existed and the computation was applied, `false` otherwise. **Warning**: This method will wait (by repeatedly yielding the thread) until it can get exclusive access. If another thread holds an `Arc` to the value indefinitely, this can cause a livelock.
*   `pub fn try_compute<Q, F>(&self, key: &Q, f: F) -> Option<bool>`
    *   *where `F: FnOnce(&mut V)`*
    *   Atomically applies a closure to a mutable reference of a value if and only if no other thread holds a reference (`Arc`) to it. Returns `Some(true)` on success, `Some(false)` if the value was locked by another reader, and `None` if the key was not found. This is the non-blocking version of `compute`.
*   `pub fn compute_val<Q, F, R>(&self, key: &Q, mut f: F) -> ComputeResult<R>`
    *   *where `F: FnMut(&mut V) -> R`*
    *   Waiting version of `try_compute_val`. Atomically applies a closure that returns a value. Can livelock. Returns `ComputeResult::Ok(R)` on success or `ComputeResult::NotFound` if the key does not exist.
*   `pub fn try_compute_val<Q, F, R>(&self, key: &Q, f: F) -> ComputeResult<R>`
    *   *where `F: FnOnce(&mut V) -> R`*
    *   Non-blocking version of `compute_val`. Returns `ComputeResult::Ok(R)` on success, `ComputeResult::Fail` if the value was locked, and `ComputeResult::NotFound` if the key was not found.
*   `pub fn invalidate<Q>(&self, key: &Q) -> bool`
    *   Removes an entry from the cache, returning `true` if the key was found.
*   `pub fn clear(&self)`
    *   Removes all entries from the cache.
*   `pub fn to_snapshot(&self) -> CacheSnapshot<K, V>`
    *   *(feature: `serde`)*
    *   Creates a serializable snapshot of the cache's current state. Pauses all other operations during creation.
*   `pub fn multiget<I, Q>(&self, keys: I) -> HashMap<K, Arc<V>>`
    *   *(feature: `bulk`)*
    *   Retrieves multiple values from the cache, parallelizing lookups across shards.
*   `pub fn multi_insert<I>(&self, items: I)`
    *   *(feature: `bulk`)*
    *   Inserts multiple key-value-cost items in parallel.
*   `pub fn multi_invalidate<I, Q>(&self, keys: I)`
    *   *(feature: `bulk`)*
    *   Removes multiple items in parallel.

---

#### **struct** `AsyncCache<K, V, H>`

A handle to the cache for asynchronous operations. All methods are async equivalents of the `Cache` methods.

*   `pub fn to_sync(&self) -> Cache<K, V, H>`
*   `pub fn metrics(&self) -> MetricsSnapshot`
*   `pub async fn get<Q, F, R>(&self, key: &Q, f: F) -> Option<R>`
*   `pub async fn fetch<Q>(&self, key: &Q) -> Option<Arc<V>>`
*   `pub async fn peek<Q>(&self, key: &Q) -> Option<Arc<V>>`
*   `pub async fn fetch_with(&self, key: &K) -> Arc<V>`
*   `pub async fn entry(&self, key: K) -> AsyncEntry<'_, K, V, H>`
*   `pub async fn insert(&self, key: K, value: V, cost: u64)`
*   `pub async fn insert_with_ttl(&self, key: K, value: V, cost: u64, ttl: Duration)`
*   `pub async fn compute<Q, F>(&self, key: &Q, mut f: F) -> bool`
*   `pub async fn try_compute<Q, F>(&self, key: &Q, f: F) -> Option<bool>`
*   `pub async fn compute_val<Q, F, R>(&self, key: &Q, mut f: F) -> ComputeResult<R>`
*   `pub async fn try_compute_val<Q, F, R>(&self, key: &Q, f: F) -> ComputeResult<R>`
*   `pub async fn invalidate<Q>(&self, key: &Q) -> bool`
*   `pub async fn clear(&self)`
*   `pub async fn to_snapshot(&self) -> CacheSnapshot<K, V>`
    *   *(feature: `serde`)*
*   `pub async fn multiget<I, Q>(&self, keys: I) -> HashMap<K, Arc<V>>`
*   `pub async fn multi_insert<I>(&self, items: I)`
    *   *(feature: `bulk`)*
*   `pub async fn multi_invalidate<I, Q>(&self, keys: I)`
    *   *(feature: `bulk`)*

---

### 4. Entry API

The Entry API provides a way to atomically manipulate a single cache entry, preventing race conditions for "get-or-insert" logic. It is available for both sync and async handles.

#### **enum** `Entry<'a, K, V, H>` / `AsyncEntry<'a, K, V, H>`

A view into a single cache entry, which is either occupied or vacant.

*   `Occupied(OccupiedEntry<'a, K, V, H>)` / `Occupied(AsyncOccupiedEntry<'a, K, V, H>)`
*   `Vacant(VacantEntry<'a, K, V, H>)` / `Vacant(AsyncVacantEntry<'a, K, V, H>)`

##### **Common Methods on `Entry` and `AsyncEntry`**

*   `pub fn or_insert(self, default: V, cost: u64) -> Arc<V>`
*   `pub fn or_insert_with<F>(self, default: F, cost: u64) -> Arc<V>`
*   `pub fn or_default(self, cost: u64) -> Arc<V>`

##### **struct** `OccupiedEntry` / `AsyncOccupiedEntry`

*   `pub fn key(&self) -> &K`
*   `pub fn get(&self) -> Arc<V>`

##### **struct** `VacantEntry` / `AsyncVacantEntry`

*   `pub fn key(&self) -> &K`
*   `pub fn insert(self, value: V, cost: u64) -> Arc<V>`

---

### 5. Policies and Listeners

#### **trait** `EvictionListener<K, V>`

A trait for receiving notifications when entries are evicted from the cache.

*   `fn on_evict(&self, key: K, value: Arc<V>, reason: EvictionReason)`
    *   Called by a background task when an entry is removed. `reason` indicates if it was due to capacity, expiration, or manual invalidation.

#### **trait** `CachePolicy<K, V>`

The core trait for implementing an eviction policy. The following policies are provided:

*   **`TinyLfuPolicy`**: (Default for bounded caches) A sophisticated policy that protects frequently used items from being evicted by infrequently used items, making it scan-resistant. It combines an LRU window with an SLRU main cache.
*   **`ArcPolicy`**: Adaptive Replacement Cache. A sophisticated policy that balances between recency (LRU) and frequency (LFU) to offer high hit rates under mixed workloads.
*   **`SlruPolicy`**: Segmented LRU. Divides the cache into probationary and protected segments to provide better scan resistance than plain LRU.
*   **`SievePolicy`**: A very simple and efficient eviction algorithm that provides good scan resistance with low overhead.
*   **`LruPolicy`**: Standard Least Recently Used. Evicts the item that hasn't been accessed for the longest time.
*   **`FifoPolicy`**: First-In, First-Out. Evicts items in the order they were inserted.
*   **`ClockPolicy`**: An approximation of LRU that offers better performance under high concurrency by avoiding list manipulations on access.
*   **`RandomPolicy`**: Evicts a random item. (Requires `random` feature).
*   **`NullPolicy`**: (Default for unbounded caches) A no-op policy that never evicts.

---

### 6. Supporting Types

#### **enum** `TimerWheelMode`

Presets for the internal expiration timer.
*   `Default` | `HighPrecisionShortLived` | `LowPrecisionLongLived`

#### **enum** `EvictionReason`

Indicates why an item was removed from the cache.
*   `Capacity` | `Expired` | `Invalidated`

#### **enum** `ComputeResult<R>`

The result of a `compute_val` or `try_compute_val` operation.

*   `Ok(R)`: The computation was successful and returned a value of type `R`.
*   `Fail`: The computation failed because another thread held a reference to the value.
*   `NotFound`: The key was not found in the cache.

#### **struct** `MetricsSnapshot`

A point-in-time, public-facing snapshot of the cache's performance metrics.
*   `pub hits: u64`
*   `pub misses: u64`
*   `pub hit_ratio: f64`
*   ...and many others for insertions, evictions, cost, and uptime.

#### **struct** `CacheSnapshot<K, V>`

*(Requires feature `serde`)*
A serializable, point-in-time snapshot of the cache's data, used for persistence.

#### **trait** `TaskSpawner`

A trait for spawning a future onto an asynchronous runtime, used by `async_loader`.

---

### 7. Error Handling

#### **enum** `BuildError`

Errors that can occur when building a cache via `CacheBuilder`.

*   `ZeroCapacity`: The cache was configured with a capacity of zero, which is not allowed.
*   `ZeroShards`: The cache was configured with zero shards, which is not allowed.
*   `SpawnerRequired`: An `async_loader` was provided, but no `TaskSpawner` was configured and the `tokio` feature is not enabled.