# API Reference: `fibre_cache`

### 1. Introduction & Core Concepts

This library provides a high-performance, concurrent, thread-safe caching solution for Rust, with first-class support for both synchronous and asynchronous applications.

#### Core Handles
The primary entry points for interacting with the cache are two handle types:

*   **`Cache<K, V, H>`**: A handle for synchronous, blocking operations.
*   **`AsyncCache<K, V, H>`**: A handle for asynchronous, non-blocking operations compatible with any Rust async runtime.

Both handles are lightweight wrappers around a shared cache core, and converting between them is a zero-cost operation.

#### Architectural Principles
*   **Builder Pattern**: Caches are created exclusively through the `CacheBuilder`, which provides a fluent API for configuration.
*   **Sharded Concurrency**: The cache is internally partitioned into multiple "shards," each with its own lock. This design minimizes lock contention and allows for high-throughput concurrent access.
*   **Background Maintenance**: A background "janitor" thread is responsible for all periodic cleanup, including enforcing capacity and expiring items based on Time-to-Live (TTL) or Time-to-Idle (TTI). This keeps user-facing operations fast and predictable.
*   **Pluggable Policies**: Eviction logic is abstracted via the `CachePolicy` trait, with sensible defaults (`TinyLFU`) for bounded caches.

#### Pervasive Patterns
*   **Key/Value Constraints**: To be stored in the cache, keys (`K`) and values (`V`) must satisfy certain trait bounds, which are most restrictive when all features are used.
    *   `K`: `Eq + Hash + Clone + Send + Sync + 'static`
    *   `V`: `Send + Sync + 'static`
*   **Cost**: Most insertion methods require a `cost` parameter (`u64`), which represents the "size" of the entry. For caches where all items are considered equal size, this can simply be `1`. The cache's `capacity` is the sum of the costs of all items.

---

### 2. Configuration

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
*   `pub fn cache_policy<Policy>(mut self, policy: Policy) -> Self`
    *   *where `Policy: CachePolicy<K, V> + 'static`*
    *   Sets a custom eviction policy. Defaults to `TinyLfuPolicy` for bounded caches and `NullPolicy` for unbounded ones.
*   `pub fn loader(mut self, f: impl Fn(K) -> (V, u64) + Send + Sync + 'static) -> Self`
    *   Sets the synchronous loader function, used by `get_with`.
*   `pub fn async_loader<F, Fut>(mut self, f: F) -> Self`
    *   *where `F: Fn(K) -> Fut + Send + Sync + 'static`, `Fut: Future<Output = (V, u64)> + Send + 'static`*
    *   Sets the asynchronous loader function, used by `get_with`.
*   `pub fn spawner(mut self, spawner: Arc<dyn TaskSpawner>) -> Self`
    *   Provides a custom task spawner for the `async_loader`. Required if using an `async_loader` without the `tokio` feature.
*   `pub fn hasher(mut self, hasher: H) -> Self`
    *   Sets a custom hasher for the cache.
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

#### **enum** `TimerWheelMode`
Preset configurations for the cache's internal timer for TTL/TTI.

*   `Default`
*   `HighPrecisionShortLived`
*   `LowPrecisionLongLived`

---

### 3. Main Types and Their Public Methods

#### **struct** `Cache<K, V, H>`
A handle to the cache for synchronous operations.

*   `pub fn to_async(&self) -> AsyncCache<K, V, H>`
    *   *where `K: Eq + Hash + Clone + Send + 'static`, `V: Send + Sync`, `H: BuildHasher + Clone`*
    *   Converts to an `AsyncCache` handle. This is a zero-cost `Arc` clone.
*   `pub fn metrics(&self) -> MetricsSnapshot`
    *   Returns a point-in-time snapshot of the cache's metrics.
*   `pub fn get(&self, key: &K) -> Option<Arc<V>>`
    *   *where `K: Eq + Hash + Clone + Send + 'static`, `V: Send + Sync`, `H: BuildHasher + Clone`*
    *   Retrieves a value from the cache.
*   `pub fn get_with(&self, key: &K) -> Arc<V>`
    *   *where `K: Eq + Hash + Clone + Send + Sync + 'static`, `V: Send + Sync + 'static`, `H: BuildHasher + Clone + Send + Sync + 'static`*
    *   Retrieves a value, using the configured `loader` if the key is not present.
*   `pub fn entry(&self, key: K) -> Entry<'_, K, V, H>`
    *   *where `K: Eq + Hash + Clone + Send + 'static`, `V: Send + Sync`, `H: BuildHasher + Clone`*
    *   Creates a view into a specific entry, allowing for atomic "get-or-insert" operations.
*   `pub fn insert(&self, key: K, value: V, cost: u64)`
    *   *where `K: Eq + Hash + Clone + Send + Sync + 'static`, `V: Send + Sync`, `H: BuildHasher + Clone + Sync`*
    *   Inserts a key-value-cost triple into the cache.
*   `pub fn insert_with_ttl(&self, key: K, value: V, cost: u64, ttl: Duration)`
    *   *where `K: Eq + Hash + Clone + Send + Sync + 'static`, `V: Send + Sync`, `H: BuildHasher + Clone + Sync`*
    *   Inserts an entry with a specific TTL that overrides the cache's default.
*   `pub fn compute<F>(&self, key: &K, f: F) -> bool`
    *   *where `F: FnOnce(&mut V)`, `K: Eq + Hash + Clone + Send + 'static`, `V: Send + Sync`, `H: BuildHasher + Clone`*
    *   Atomically applies a closure to a mutable reference of a value if no other thread is reading it.
*   `pub fn invalidate(&self, key: &K) -> bool`
    *   *where `K: Eq + Hash + Clone + Send + 'static`, `V: Send + Sync`, `H: BuildHasher + Clone`*
    *   Removes an entry from the cache.
*   `pub fn clear(&self)`
    *   *where `K: Eq + Hash + Clone + Send + 'static`, `V: Send + Sync`, `H: BuildHasher + Clone`*
    *   Removes all entries from the cache.
*   `pub fn to_snapshot(&self) -> CacheSnapshot<K, V>`
    *   *(feature: `serde`)*
    *   *where `K: ... + Serialize`, `V: ... + Serialize + Clone`*
    *   Creates a serializable snapshot of the cache's current state. Pauses all other operations.
*   `pub fn multiget<I, Q>(&self, keys: I) -> HashMap<K, Arc<V>>`
    *   *(feature: `bulk`)*
    *   *where `I: IntoIterator<Item = Q>`, `K: From<Q>`, `H: BuildHasher + Clone + Sync`*
    *   Retrieves multiple values from the cache, parallelizing lookups across shards.
*   `pub fn multi_insert<I>(&self, items: I)`
    *   *(feature: `bulk`)*
    *   *where `I: IntoIterator<Item = (K, V, u64)>`*
    *   Inserts multiple entries into the cache.
*   `pub fn multi_invalidate<I, Q>(&self, keys: I)`
    *   *(feature: `bulk`)*
    *   *where `I: IntoIterator<Item = Q>`, `K: From<Q>`*
    *   Removes multiple entries from the cache.

---

#### **struct** `AsyncCache<K, V, H>`
A handle to the cache for asynchronous operations.

*   `pub fn to_sync(&self) -> Cache<K, V, H>`
    *   *where `K: Clone + Eq + Hash + Send`, `V: Send + Sync`, `H: BuildHasher + Clone`*
    *   Converts to a `Cache` handle. This is a zero-cost `Arc` clone.
*   `pub fn metrics(&self) -> MetricsSnapshot`
    *   Returns a point-in-time snapshot of the cache's metrics.
*   `pub fn get(&self, key: &K) -> Option<Arc<V>>`
    *   *where `K: Clone + Eq + Hash + Send`, `V: Send + Sync`, `H: BuildHasher + Clone`*
    *   Retrieves a value from the cache.
*   `pub async fn get_with(&self, key: &K) -> Arc<V>`
    *   *where `K: Clone + Eq + Hash + Send + Sync + 'static`, `V: Send + Sync + 'static`, `H: BuildHasher + Clone + Send + Sync + 'static`*
    *   Asynchronously retrieves a value, using the configured loader if the key is not present.
*   `pub async fn entry(&self, key: K) -> AsyncEntry<'_, K, V, H>`
    *   *where `K: Clone + Eq + Hash + Send`, `V: Send + Sync`, `H: BuildHasher + Clone`*
    *   Asynchronously creates a view into a specific entry.
*   `pub async fn insert(&self, key: K, value: V, cost: u64)`
    *   *where `K: Clone + Eq + Hash + Send + Sync`, `V: Send + Sync`, `H: BuildHasher + Clone + Send + Sync`*
    *   Asynchronously inserts a key-value-cost triple.
*   `pub async fn insert_with_ttl(&self, key: K, value: V, cost: u64, ttl: Duration)`
    *   *where `K: Clone + Eq + Hash + Send + Sync`, `V: Send + Sync`, `H: BuildHasher + Clone + Send + Sync`*
    *   Asynchronously inserts an entry with a specific TTL.
*   `pub async fn compute<F>(&self, key: &K, f: F) -> bool`
    *   *where `F: FnOnce(&mut V)`, `K: Clone + Eq + Hash + Send`, `V: Send + Sync`, `H: BuildHasher + Clone`*
    *   Asynchronously applies a closure to a mutable reference of a value.
*   `pub async fn invalidate(&self, key: &K) -> bool`
    *   *where `K: Clone + Eq + Hash + Send`, `V: Send + Sync`, `H: BuildHasher + Clone`*
    *   Asynchronously removes an entry from the cache.
*   `pub async fn clear(&self)`
    *   *where `K: Clone + Eq + Hash + Send`, `V: Send + Sync`, `H: BuildHasher + Clone`*
    *   Asynchronously removes all entries from the cache.
*   `pub async fn to_snapshot(&self) -> CacheSnapshot<K, V>`
    *   *(feature: `serde`)*
    *   *where `K: ... + Serialize`, `V: ... + Serialize + Clone`*
    *   Asynchronously creates a serializable snapshot of the cache's current state.
*   `pub async fn multiget<I, Q>(&self, keys: I) -> HashMap<K, Arc<V>>`
    *   *where `I: IntoIterator<Item = Q>`, `K: From<Q> + ...`, `V: Send + Sync`, `H: BuildHasher + Clone`*
    *   Asynchronously retrieves multiple values from the cache.
*   `pub async fn multi_insert<I>(&self, items: I)`
    *   *(feature: `bulk`)*
    *   *where `I: IntoIterator<Item = (K, V, u64)>`*
    *   Asynchronously inserts multiple entries.
*   `pub async fn multi_invalidate<I, Q>(&self, keys: I)`
    *   *(feature: `bulk`)*
    *   *where `I: IntoIterator<Item = Q>`, `K: From<Q>`*
    *   Asynchronously removes multiple entries.

---

#### **enum** `Entry<'a, K, V, H>`
A view into a single synchronous entry in a cache, which may either be occupied or vacant.

*   `Occupied(OccupiedEntry<'a, K, V, H>)`
*   `Vacant(VacantEntry<'a, K, V, H>)`

#### **struct** `OccupiedEntry<'a, K, V, H>`
*   `pub fn key(&self) -> &K`
*   `pub fn get(&self) -> Arc<V>`

#### **struct** `VacantEntry<'a, K, V, H>`
*   `pub fn key(&self) -> &K`
*   `pub fn insert(mut self, value: V, cost: u64) -> Arc<V>`

---

#### **enum** `AsyncEntry<'a, K, V, H>`
A view into a single asynchronous entry in a cache.

*   `Occupied(AsyncOccupiedEntry<'a, K, V, H>)`
*   `Vacant(AsyncVacantEntry<'a, K, V, H>)`

#### **struct** `AsyncOccupiedEntry<'a, K, V, H>`
*   `pub fn key(&self) -> &K`
*   `pub fn get(&self) -> Arc<V>`

#### **struct** `AsyncVacantEntry<'a, K, V, H>`
*   `pub fn key(&self) -> &K`
*   `pub fn insert(mut self, value: V, cost: u64) -> Arc<V>`

---

### 4. Public Traits and Their Methods

#### **trait** `EvictionListener<K, V>`
A trait for receiving notifications when entries are evicted from the cache.

*   `fn on_evict(&self, key: K, value: Arc<V>, reason: EvictionReason)`
    *   Called by a background task when an entry is removed.

#### **trait** `TaskSpawner`
A trait for spawning a future onto an asynchronous runtime.

*   `fn spawn(&self, future: Pin<Box<dyn Future<Output = ()> + Send>>)`
    *   Spawns a type-erased future.

---

### 5. Public Enums (Non-Config)

#### **enum** `EvictionReason`
Describes the reason an entry was removed from the cache.

*   `Capacity`: The entry was removed due to exceeding the cache's capacity.
*   `Expired`: The entry was removed because its TTL or TTI expired.
*   `Invalidated`: The entry was removed because it was manually invalidated.

---

### 6. Public Structs (Data-oriented)

#### **struct** `MetricsSnapshot`
A point-in-time, public-facing snapshot of the cache's metrics.

*   `pub hits: u64`
*   `pub misses: u64`
*   `pub hit_ratio: f64`
*   `pub inserts: u64`
*   `pub updates: u64`
*   `pub invalidations: u64`
*   `pub evicted_by_capacity: u64`
*   `pub evicted_by_ttl: u64`
*   `pub evicted_by_tti: u64`
*   `pub keys_admitted: u64`
*   `pub keys_rejected: u64`
*   `pub current_cost: u64`
*   `pub total_cost_added: u64`
*   `pub uptime_secs: u64`

---

#### **struct** `CacheSnapshot<K, V>`
*(Requires feature `serde`)*
A serializable, point-in-time snapshot of the cache's data. Used with `Cache::to_snapshot` and `CacheBuilder::build_from_snapshot`.

---

### 7. Error Handling

#### **enum** `BuildError`
Errors that can occur when building a cache via `CacheBuilder`.

*   `ZeroCapacity`: The cache was configured with a capacity of zero.
*   `ZeroShards`: The cache was configured with zero shards.
*   `SpawnerRequired`: An `async_loader` was provided, but no `TaskSpawner` was configured and the `tokio` feature is not enabled.