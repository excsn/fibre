# API Reference: `fibre_cache`

### 1. Introduction & Core Concepts

This library provides a high-performance, concurrent, thread-safe caching solution for Rust, with first-class support for both synchronous and asynchronous applications.

#### Getting Started

```rust
use fibre_cache::CacheBuilder;
use std::time::Duration;

// Create a cache with a maximum cost of 1000, and a 5-second TTL.
let cache = CacheBuilder::new()
    .capacity(1000)
    .time_to_live(Duration::from_secs(5))
    .build::<String, i32>()
    .unwrap();

// Insert a value. The `cost` is 1.
cache.insert("key1".to_string(), 100, 1);

// Retrieve the value using `fetch`, which returns an Arc<V>.
let value = cache.fetch(&"key1".to_string()).unwrap();
assert_eq!(*value, 100);

// After 5 seconds, the value will be expired and automatically removed.
```

#### Core Handles
The primary entry points for interacting with the cache are two handle types:

*   **`Cache<K, V, H>`**: A handle for synchronous, blocking operations.
*   **`AsyncCache<K, V, H>`**: A handle for asynchronous, non-blocking operations compatible with any Rust async runtime.

Both handles are lightweight wrappers around a shared cache core, and converting between them is a zero-cost operation.

#### Architectural Principles
*   **Builder Pattern**: Caches are created exclusively through the `CacheBuilder`, which provides a fluent API for configuration.
*   **Sharded Concurrency**: The cache is internally partitioned into multiple "shards," each with its own lock. This design minimizes lock contention and allows for high-throughput concurrent access.
*   **Background Maintenance**: A background "janitor" thread is responsible for all periodic cleanup, including enforcing capacity and expiring items based on Time-to-Live (TTL) or Time-to-Idle (TTI). This keeps user-facing operations fast and predictable.
*   **Pluggable Policies**: Eviction logic is abstracted via the `CachePolicy` trait, with sensible defaults (`TinyLfuPolicy`) for bounded caches.

#### Pervasive Patterns
*   **Key/Value Constraints**: To be stored in the cache, keys (`K`) and values (`V`) must satisfy certain trait bounds, which are most restrictive when all features are used.
    *   `K`: `Eq + Hash + Clone + Send + Sync + 'static`
    *   `V`: `Send + Sync + 'static`
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
    *   Sets the maximum total cost of the cache.
*   `pub fn unbounded(mut self) -> Self`
    *   Sets the cache to have "unbounded" capacity (`u64::MAX`). This is the default.
*   `pub fn shards(mut self, shards: usize) -> Self`
    *   Sets the number of concurrent shards. The value is rounded up to the next power of two. Defaults to a value based on CPU count.
*   `pub fn time_to_live(mut self, duration: Duration) -> Self`
    *   Sets a time-to-live (TTL) for all entries.
*   `pub fn time_to_idle(mut self, duration: Duration) -> Self`
    *   Sets a time-to-idle (TTI) for all entries.
*   `pub fn stale_while_revalidate(mut self, duration: Duration) -> Self`
    *   Sets a duration after an entry's TTL expires during which the cache can serve the stale value while asynchronously refreshing it. Requires a loader.
*   `pub fn eviction_listener<Listener>(mut self, listener: Listener) -> Self`
    *   *where `Listener: EvictionListener<K, V> + 'static`*
    *   Registers a listener to receive notifications when entries are evicted.
*   `pub fn cache_policy_factory<F>(mut self, factory: F) -> Self`
    *   *where `F: Fn() -> Box<dyn CachePolicy<K, V>> + Send + Sync + 'static`*
    *   Sets a custom eviction policy via a factory function that is called for each shard. Defaults to `TinyLfuPolicy` for bounded caches and `NullPolicy` for unbounded ones.
*   `pub fn loader(mut self, f: impl Fn(K) -> (V, u64) + Send + Sync + 'static) -> Self`
    *   Sets the synchronous loader function, used by [`Cache::fetch_with`]. When `fetch_with` is called for a key that isn't in the cache, this closure is executed to compute its value.
*   `pub fn async_loader<F, Fut>(mut self, f: F) -> Self`
    *   *where `F: Fn(K) -> Fut + Send + Sync + 'static`, `Fut: Future<Output = (V, u64)> + Send + 'static`*
    *   Sets the asynchronous loader function, used by [`AsyncCache::fetch_with`].
*   `pub fn spawner(mut self, spawner: Arc<dyn TaskSpawner>) -> Self`
    *   Provides a custom task spawner for the `async_loader`. Required if using an `async_loader` without the `tokio` feature.
*   `pub fn hasher(mut self, hasher: H) -> Self`
    *   Sets a custom hasher for the cache.
*   `pub fn maintenance_chance(mut self, denominator: u32) -> Self`
    *   Sets the probability (`1 / denominator`) of running maintenance on insert. Use presets from the `maintenance_frequency` module for convenience.
*   `pub fn timer_mode(mut self, mode: TimerWheelMode) -> Self`
    *   Configures the internal expiration timer using a preset.
*   `pub fn timer_tick_duration(mut self, duration: Duration) -> Self`
    *   Sets the granularity of the internal expiration timer.
*   `pub fn timer_wheel_size(mut self, size: usize) -> Self`
    *   Sets the number of slots in the internal expiration timer.

##### **Build Methods**
*   `pub fn build(mut self) -> Result<Cache<K, V, H>, BuildError>`
    *   *where `K: Eq + Hash + Clone + Send + Sync + 'static`, `V: Send + Sync + 'static`, `H: BuildHasher + Clone + Send + Sync + 'static`*
    *   Builds the synchronous `Cache`.
*   `pub fn build_async(mut self) -> Result<AsyncCache<K, V, H>, BuildError>`
    *   *where `K: Eq + Hash + Clone + Send + Sync + 'static`, `V: Send + Sync + 'static`, `H: BuildHasher + Clone + Send + Sync + 'static`*
    *   Builds the asynchronous `AsyncCache`.
*   `pub fn build_from_snapshot(mut self, snapshot: CacheSnapshot<K, V>) -> Result<Cache<K, V, H>, BuildError>`
    *   *(feature: `serde`)*
    *   *where `K: ... + DeserializeOwned`, `V: ... + DeserializeOwned + Clone`*
    *   Builds a sync cache and populates it from a previously created snapshot.
*   `pub fn build_from_snapshot_async(mut self, snapshot: CacheSnapshot<K, V>) -> Result<AsyncCache<K, V, H>, BuildError>`
    *   *(feature: `serde`)*
    *   *where `K: ... + DeserializeOwned`, `V: ... + DeserializeOwned + Clone`*
    *   Builds an async cache and populates it from a snapshot.

---

### 3. Main Types and Public Methods

#### **struct** `Cache<K, V, H>`
A handle to the cache for synchronous operations.

*   `pub fn to_async(&self) -> AsyncCache<K, V, H>`
    *   Converts to an `AsyncCache` handle. This is a zero-cost `Arc` clone.
*   `pub fn metrics(&self) -> MetricsSnapshot`
    *   Returns a point-in-time snapshot of the cache's metrics.
*   `pub fn get<F, R>(&self, key: &K, f: F) -> Option<R>`
    *   *where `F: FnOnce(&V) -> R`*
    *   Looks up an entry and, if found, applies a closure to the value without cloning it. This is the most efficient read operation.
*   `pub fn fetch(&self, key: &K) -> Option<Arc<V>>`
    *   Retrieves a clone of the `Arc` pointing to the value if the key is found and not expired.
*   `pub fn peek(&self, key: &K) -> Option<Arc<V>>`
    *   Retrieves a value without updating its recency or access time (i.e., does not affect LRU or TTI).
*   `pub fn fetch_with(&self, key: &K) -> Arc<V>`
    *   Retrieves a value, using the configured synchronous `loader` if the key is not present. This method provides thundering herd protection.
*   `pub fn entry(&self, key: K) -> Entry<'_, K, V, H>`
    *   Creates a view into a specific entry, allowing for atomic "get-or-insert" operations.
*   `pub fn insert(&self, key: K, value: V, cost: u64)`
    *   Inserts a key-value-cost triple into the cache.
*   `pub fn insert_with_ttl(&self, key: K, value: V, cost: u64, ttl: Duration)`
    *   Inserts an entry with a specific TTL that overrides the cache's default.
*   `pub fn compute<F>(&self, key: &K, mut f: F)`
    *   *where `F: FnMut(&mut V)`*
    *   Atomically applies a closure to a mutable reference of a value. **Warning**: This method will wait (by repeatedly yielding the thread) until it can get exclusive access. If another thread holds an `Arc` to the value indefinitely, this can cause a livelock. Use with caution.
*   `pub fn try_compute<F>(&self, key: &K, f: F) -> bool`
    *   *where `F: FnOnce(&mut V)`*
    *   Atomically applies a closure to a mutable reference of a value if and only if no other thread holds a reference (`Arc`) to it. Returns `true` on success. This is the non-blocking version of `compute`.
*   `pub fn invalidate(&self, key: &K) -> bool`
    *   Removes an entry from the cache, returning `true` if the key was found.
*   `pub fn clear(&self)`
    *   Removes all entries from the cache.
*   `pub fn to_snapshot(&self) -> CacheSnapshot<K, V>`
    *   *(feature: `serde`)*
    *   Creates a serializable snapshot of the cache's current state. Pauses all other operations.
*   `pub fn multiget<I, Q>(&self, keys: I) -> HashMap<K, Arc<V>>`
    *   *(feature: `bulk`)*
    *   Retrieves multiple values from the cache, parallelizing lookups across shards.
*   `pub fn multi_insert<I>(&self, items: I)`
    *   *(feature: `bulk`)*
*   `pub fn multi_invalidate<I, Q>(&self, keys: I)`
    *   *(feature: `bulk`)*

---

#### **struct** `AsyncCache<K, V, H>`
A handle to the cache for asynchronous operations.

*   `pub fn to_sync(&self) -> Cache<K, V, H>`
    *   Converts to a `Cache` handle. This is a zero-cost `Arc` clone.
*   `pub async fn get<F, R>(&self, key: &K, f: F) -> Option<R>`
    *   *where `F: FnOnce(&V) -> R`*
    *   Asynchronously looks up an entry and applies a closure to the value.
*   `pub async fn fetch(&self, key: &K) -> Option<Arc<V>>`
    *   Asynchronously retrieves a clone of the `Arc` pointing to the value.
*   `pub async fn peek(&self, key: &K) -> Option<Arc<V>>`
    *   Asynchronously retrieves a value without updating its recency or access time.
*   `pub async fn fetch_with(&self, key: &K) -> Arc<V>`
    *   Asynchronously retrieves a value, using the configured `async_loader` if the key is not present. This method provides thundering herd protection.
*   `pub async fn entry(&self, key: K) -> AsyncEntry<'_, K, V, H>`
    *   Asynchronously creates a view into a specific entry.
*   `pub async fn insert(&self, key: K, value: V, cost: u64)`
    *   Asynchronously inserts a key-value-cost triple.
*   `pub async fn insert_with_ttl(&self, key: K, value: V, cost: u64, ttl: Duration)`
    *   Asynchronously inserts an entry with a specific TTL.
*   `pub async fn compute<F>(&self, key: &K, mut f: F)`
    *   *where `F: FnMut(&mut V)`*
    *   Asynchronously applies a closure to a mutable reference of a value. **Warning**: This method can cause a livelock if an `Arc` to the value is held indefinitely elsewhere.
*   `pub async fn try_compute<F>(&self, key: &K, f: F) -> bool`
    *   *where `F: FnOnce(&mut V)`*
    *   Asynchronously and non-blockingly attempts to apply a closure to a mutable reference of a value.
*   `pub async fn invalidate(&self, key: &K) -> bool`
*   `pub async fn clear(&self)`
*   `pub async fn to_snapshot(&self) -> CacheSnapshot<K, V>`
    *   *(feature: `serde`)*
*   `pub async fn multiget<I, Q>(&self, keys: I) -> HashMap<K, Arc<V>>`
    *   *(feature: `bulk`)*
*   `pub async fn multi_insert<I>(&self, items: I)`
    *   *(feature: `bulk`)*
*   `pub async fn multi_invalidate<I, Q>(&self, keys: I)`
    *   *(feature: `bulk`)*

---

### 4. Entry API

The Entry API provides a way to atomically manipulate a single cache entry, preventing race conditions for "get-or-insert" logic.

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
    *   Called by a background task when an entry is removed.

#### **trait** `CachePolicy<K, V>`
The core trait for implementing an eviction policy. The following policies are provided:
*   **`TinyLfuPolicy`**: (Default for bounded caches) A sophisticated policy that protects frequently used items from being evicted by infrequently used items, making it scan-resistant. It combines an LRU window with an SLRU main cache.
*   **`SlruPolicy`**: Segmented LRU. Divides the cache into probationary and protected segments to provide better scan resistance than plain LRU.
*   **`SievePolicy`**: A very simple and efficient eviction algorithm that provides good scan resistance with low overhead.
*   **`LruPolicy`**: Standard Least Recently Used. Evicts the item that hasn't been accessed for the longest time.
*   **`FifoPolicy`**: First-In, First-Out. Evicts items in the order they were inserted.
*   **`ClockPolicy`**: An approximation of LRU that offers better performance under high concurrency.
*   **`RandomPolicy`**: Evicts a random item. (Requires `random` feature).
*   **`NullPolicy`**: (Default for unbounded caches) A no-op policy that never evicts.

---

### 6. Supporting Types

#### **enum** `TimerWheelMode`
*   `Default` | `HighPrecisionShortLived` | `LowPrecisionLongLived`

#### **enum** `EvictionReason`
*   `Capacity` | `Expired` | `Invalidated`

#### **struct** `MetricsSnapshot`
*   `pub hits: u64`
*   `pub misses: u64`
*   `pub hit_ratio: f64`
*   ...and many others.

#### **struct** `CacheSnapshot<K, V>`
*(Requires feature `serde`)*
A serializable, point-in-time snapshot of the cache's data.

#### **trait** `TaskSpawner`
A trait for spawning a future onto an asynchronous runtime.

---

### 7. Error Handling

#### **enum** `BuildError`
Errors that can occur when building a cache via `CacheBuilder`.

*   `ZeroCapacity`: The cache was configured with a capacity of zero.
*   `ZeroShards`: The cache was configured with zero shards.
*   `SpawnerRequired`: An `async_loader` was provided, but no `TaskSpawner` was configured and the `tokio` feature is not enabled.