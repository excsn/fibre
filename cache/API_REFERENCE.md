# API Reference: `fibre_cache`

A sharded concurrent cache for Rust with synchronous and asynchronous handles on one instance.

## Contents

* [1. Constraints](#1-constraints)
* [2. Configuration](#2-configuration)
    * [`CacheBuilder`](#cachebuilder)
    * [`maintenance_frequency`](#maintenance_frequency)
* [3. Handles and Iterators](#3-handles-and-iterators)
    * [`Cache`](#cache)
    * [`AsyncCache`](#asynccache)
    * [`Iter`](#iter)
    * [`IterStream`](#iterstream)
    * [`SnapshotIter`](#snapshotiter)
    * [`AsyncSnapshotIter`](#asyncsnapshotiter)
* [4. Entry API](#4-entry-api)
    * [`Entry` and `AsyncEntry`](#entry-and-asyncentry)
    * [`OccupiedEntry` and `AsyncOccupiedEntry`](#occupiedentry-and-asyncoccupiedentry)
    * [`VacantEntry` and `AsyncVacantEntry`](#vacantentry-and-asyncvacantentry)
* [5. Policies and Listeners](#5-policies-and-listeners)
    * [`EvictionListener`](#evictionlistener)
    * [`CachePolicy`](#cachepolicy)
    * [`AdmissionDecision`](#admissiondecision)
    * [Provided Policies](#provided-policies)
* [6. Supporting Types](#6-supporting-types)
    * [`TimerWheelMode`](#timerwheelmode)
    * [`EvictionReason`](#evictionreason)
    * [`ComputeResult`](#computeresult)
    * [`MetricsSnapshot`](#metricssnapshot)
    * [`Metrics`](#metrics)
    * [`CacheSnapshot`](#cachesnapshot)
    * [`TaskSpawner`](#taskspawner)
    * [`TokioSpawner`](#tokiospawner)
* [7. Error Handling](#7-error-handling)
    * [`BuildError`](#builderror)

## 1. Constraints

* `K`: `Eq + Hash + Clone + Send + Sync + 'static`
* `V`: `Send + Sync + 'static`. `V` does not require `Clone`, except on `to_snapshot` and the snapshot builders.
* `cost`: Most insertion methods take a `cost` (`u64`) representing the size of the entry. Use `1` where all items count equally.
* `capacity` limits the sum of the costs of all items, enforced as an equal share per shard. Each shard gets `capacity / shards` (at least 1) and an insert evicts when its shard goes over that share, so a shard can evict while the cache total is below capacity. An insert leaves its shard within its share when it returns. `null_policy()` lets the total exceed `capacity`. So does `shards()` with more shards than `capacity` allows, since each shard keeps a share of at least 1.

## 2. Configuration

### `CacheBuilder`

`CacheBuilder<K, V, H>` is the fluent builder for `Cache` and `AsyncCache` instances.

* `pub fn new() -> Self`
    * *where `H: BuildHasher + Default`*
    * Creates a new builder with default settings (unbounded capacity, `ahash` hasher).
* `pub fn default() -> Self`
    * *where `K: Send, V: Send`, `H` is `ahash::RandomState`*
    * The standard `default` constructor.
* `pub fn rapidhash() -> Self`
    * *(feature: `rapidhash`)*
    * *where `H` is `rapidhash::RapidRandomState`*
    * A convenience constructor for using the `rapidhash` hasher.
* `pub fn capacity(mut self, capacity: u64) -> Self`
    * Sets the maximum total cost of the cache. If set, an eviction policy will be active.
* `pub fn unbounded(mut self) -> Self`
    * Sets the cache to have "unbounded" capacity (`u64::MAX`). This is the default.
* `pub fn shards(mut self, shards: usize) -> Self`
    * Sets the number of concurrent shards. The value is rounded up to the next power of two and `0` becomes 1. Defaults to eight times the CPU count, lowered for a bounded cache so that each shard holds at least `MIN_COST_PER_SHARD` (64) cost units; calling this method disables that derivation.
* `pub const MIN_COST_PER_SHARD: u64 = 64`
    * Path: `fibre_cache::builder::MIN_COST_PER_SHARD`. The smallest cost per shard the default shard count allows for a bounded cache.
* `pub fn time_to_live(mut self, duration: Duration) -> Self`
    * Sets a default time-to-live (TTL) for all entries in the cache. Reads check expiry against a clock the janitor refreshes once per tick, 1 s by default, so `get`, `fetch` and `peek` can return an entry for up to one tick after its TTL or TTI deadline. `iter()`, `iter_stream()` and `to_snapshot()` use the exact clock. `iter_snapshot()` reads through `fetch`. The janitor removes an expired entry within about one wheel tick plus one janitor tick of its deadline, on every shard and independent of `maintenance_chance`. `janitor_tick_interval` tightens these bounds.
* `pub fn time_to_idle(mut self, duration: Duration) -> Self`
    * Reads stop returning an entry not accessed for this duration. They never stop sooner. Reads stamp the access from a coarse clock refreshed at least every `duration / 2`, so an entry can be returned for up to two refreshes past the deadline. Each janitor tick scans every shard and removes idle entries, independent of `maintenance_chance`. The scan costs about 1.3% of a core per million entries at a 1 s tick on an M4 Pro. Without an explicit `janitor_tick_interval`, the tick is `duration / 2`, capped at 1 s.
* `pub fn janitor_tick_interval(mut self, duration: Duration) -> Self`
    * Sets how often the janitor refreshes the clock reads check expiry against, sweeps shards with TTL timers due and scans for idle entries. Defaults to 1 s. With a TTI set it defaults to `time_to_idle / 2` capped at 1 s. A TTI shorter than twice the tick refreshes the clock more often than this. A shorter tick tightens the read and removal bounds at the cost of more frequent background work.
* `pub fn max_concurrent_refreshes(mut self, limit: usize) -> Self`
    * Sets how many stale refreshes run at once, 32 by default, to limit the background load on what the loader calls. It does not limit misses. Panics if `limit` is 0.
* `pub fn stale_while_revalidate(mut self, duration: Duration) -> Self`
    * Sets a duration after an entry's TTL expires during which `fetch_with` returns the stale value and refreshes it in the background. `get`, `fetch` and `peek` return `None` once the deadline passes. Removal, its `Expired` notification and `evicted_by_ttl` come at the end of the grace period. At most `max_concurrent_refreshes` refreshes run at once. The rest wait in order, one per key. A miss past the grace period takes its key's refresh from the queue. Requires a `loader` or `async_loader`.
* `pub fn eviction_listener<Listener>(mut self, listener: Listener) -> Self`
    * *where `Listener: EvictionListener<K, V> + 'static`*
    * Registers a listener to receive notifications when entries are evicted. The listener is called on a dedicated background thread.
* `pub fn cache_policy_factory<F>(mut self, factory: F) -> Self`
    * *where `F: Fn() -> Box<dyn CachePolicy<K, V>> + Send + Sync + 'static`*
    * Sets a custom eviction policy via a factory function that is called for each shard. Without a factory a bounded cache runs S3-FIFO (TinyLFU after `tinylfu()`) and an unbounded cache runs `NullPolicy`.
* `pub fn s3fifo(self) -> Self`
    * Sets the eviction policy to S3-FIFO, run inside each shard. This is the default for a bounded cache. Makes the same decisions as `S3FifoPolicy` through `cache_policy_factory`. Replaces a factory or `tinylfu()` set before it.
* `pub fn tinylfu(self) -> Self`
    * Sets the eviction policy to W-TinyLFU, run inside each shard. Makes the same decisions as `TinyLfuPolicy` through `cache_policy_factory`. Replaces a factory or `s3fifo()` set before it.
* `pub fn null_policy(self) -> Self`
    * *where `K: Sync`, `V: Sync`*
    * Sets the eviction policy to the no-op `NullPolicy`, which never evicts. Shorthand for `cache_policy_factory(|| Box::new(NullPolicy))`.
* `pub fn loader(mut self, f: impl Fn(K) -> (V, u64) + Send + Sync + 'static) -> Self`
    * Sets the synchronous loader function, used by `fetch_with`. When `fetch_with` is called for a key that isn't in the cache, this closure is executed to compute its value and cost. On a `Cache` it runs on the calling thread. Stale refreshes and an `AsyncCache` run it on a pool of up to 64 threads per cache.
* `pub fn async_loader<F, Fut>(mut self, f: F) -> Self`
    * *where `F: Fn(K) -> Fut + Send + Sync + 'static`, `Fut: Future<Output = (V, u64)> + Send + 'static`*
    * Sets the asynchronous loader function, used by `fetch_with`.
* `pub fn spawner(mut self, spawner: Arc<dyn TaskSpawner>) -> Self`
    * Provides a custom task spawner for the `async_loader`. Without one, an `async_loader` build uses `TokioSpawner`, which panics outside a Tokio runtime. Required on any other runtime.
* `pub fn hasher(mut self, hasher: H) -> Self`
    * Sets a custom hasher for the cache.
* `pub fn maintenance_chance(mut self, denominator: u32) -> Self`
    * Sets the probability (`1 / denominator`, rounded up to a power of two, default 16) that a janitor tick applies pending reads on each of two random shards. Inserts apply reads themselves. Capacity, TTL removal and TTI removal do not depend on it. Panics if `denominator` is 0. Presets are in `builder::maintenance_frequency`.
* `pub fn maintenance_on_introspection(mut self, enabled: bool) -> Self`
    * When `true`, the introspection methods `metrics()`, `iter()`, `iter_with_batch_size()`, `iter_snapshot()`, `to_snapshot()` and their async equivalents will automatically flush all pending read-access events before returning. This ensures the eviction policy's internal state (e.g., LRU order, frequency counters) is fully up-to-date when you inspect the cache. On an `AsyncCache`, `metrics`, `iter_stream` and `iter_snapshot_async` skip a shard whose lock is held, since they cannot wait for it. `to_snapshot` awaits every shard. Disabled by default.
* `pub fn timer_mode(mut self, mode: TimerWheelMode) -> Self`
    * Sets the tick and size of the per-shard TTL wheel from a preset. Later `timer_tick_duration` or `timer_wheel_size` calls override it.
* `pub fn timer_tick_duration(mut self, duration: Duration) -> Self`
    * Sets the TTL wheel's tick, 1 s by default. Removal lags a deadline by up to one wheel tick plus one janitor tick and a wheel tick shorter than the janitor tick gains nothing beyond the janitor tick. TTI does not use the wheel.
* `pub fn timer_wheel_size(mut self, size: usize) -> Self`
    * Sets the number of slots in the TTL wheel, 60 by default. A TTL longer than one revolution is tracked in laps. A size of 0 panics at the first TTL insert.
* `pub fn build(mut self) -> Result<Cache<K, V, H>, BuildError>`
    * *where `K: Eq + Hash + Clone + Send + Sync + 'static`, `V: Send + Sync + 'static`, `H: BuildHasher + Clone + Send + Sync + 'static`*
    * Finalizes the configuration and builds the synchronous `Cache`.
* `pub fn build_async(mut self) -> Result<AsyncCache<K, V, H>, BuildError>`
    * *where `K: Eq + Hash + Clone + Send + Sync + 'static`, `V: Send + Sync + 'static`, `H: BuildHasher + Clone + Send + Sync + 'static`*
    * Finalizes the configuration and builds the asynchronous `AsyncCache`.
* `pub fn build_from_snapshot(mut self, snapshot: CacheSnapshot<K, V>) -> Result<Cache<K, V, H>, BuildError>`
    * *(feature: `serde`)*
    * *where `K: ... + DeserializeOwned`, `V: ... + DeserializeOwned + Clone`*
    * Builds a sync cache and pre-populates it with entries from a previously created snapshot. The snapshot's capacity and shard count replace the builder's, except that a default shard count is still lowered to keep 64 cost units per shard.
* `pub fn build_from_snapshot_async(mut self, snapshot: CacheSnapshot<K, V>) -> Result<AsyncCache<K, V, H>, BuildError>`
    * *(feature: `serde`)*
    * *where `K: ... + DeserializeOwned`, `V: ... + DeserializeOwned + Clone`*
    * Builds an async cache and pre-populates it from a snapshot. The snapshot's capacity and shard count replace the builder's, except that a default shard count is still lowered to keep 64 cost units per shard.

### `maintenance_frequency`

Presets for `CacheBuilder::maintenance_chance`. Path: `fibre_cache::builder::maintenance_frequency`.

* `pub const RESPONSIVE: u32 = 16`
    * The default.
* `pub const THROUGHPUT: u32 = 64`
* `pub const LOW_OVERHEAD: u32 = 256`

## 3. Handles and Iterators

### `Cache`

`Cache<K, V, H>` is a handle to the cache for synchronous operations. Clones share one instance.

* `pub fn to_async(&self) -> AsyncCache<K, V, H>`
    * Converts to an `AsyncCache` handle. This is a zero-cost `Arc` clone.
* `pub fn metrics(&self) -> MetricsSnapshot`
    * Returns a point-in-time snapshot of the cache's metrics. If `maintenance_on_introspection` is enabled, pending read-access events are flushed first so the eviction policy state is current.
* `pub fn get<Q, F, R>(&self, key: &Q, f: F) -> Option<R>`
    * *where `F: FnOnce(&V) -> R`*
    * Looks up an entry and, if found, applies a closure to the value without cloning it. The closure runs under the shard's read lock.
* `pub fn fetch<Q>(&self, key: &Q) -> Option<Arc<V>>`
    * Retrieves a clone of the `Arc` pointing to the value if the key is found and not expired. Expiry is judged on the janitor's clock, so an entry can still be returned for up to one janitor tick past its deadline; see `time_to_live`.
* `pub fn peek<Q>(&self, key: &Q) -> Option<Arc<V>>`
    * Retrieves a value without updating its recency or access time (i.e., does not affect LRU, TTI, etc.).
* `pub fn fetch_with(&self, key: &K) -> Arc<V>`
    * Retrieves a value, using the configured `loader` or `async_loader` if the key is not present. This method provides thundering herd protection. A miss panics with no loader configured. A loader that panics panics the caller and every waiter on that key. The next call loads again. A sync loader calling `fetch_with` on its own key panics. A loader cycle across threads (A loads B loads A) is not detected and hangs.
* `pub fn entry(&self, key: K) -> Entry<'_, K, V, H>`
    * Creates a view into a specific entry, allowing for atomic "get-or-insert" operations. An expired entry not yet removed counts as present.
* `pub fn insert(&self, key: K, value: V, cost: u64)`
    * Inserts a key-value-cost triple into the cache.
* `pub fn insert_with_ttl(&self, key: K, value: V, cost: u64, ttl: Duration)`
    * Inserts an entry with a specific TTL that overrides the cache's default. Reads honour the TTL and the janitor removes the entry at expiry. The first call starts the janitor if the cache has none.
* `pub fn compute<Q, F>(&self, key: &Q, mut f: F) -> bool`
    * *where `F: FnMut(&mut V)`*
    * Atomically applies a closure to a mutable reference of a value. Returns `true` if the key existed and the computation was applied, `false` otherwise. An expired entry not yet removed counts as present. **Warning**: This method will wait (by repeatedly yielding the thread) until it can get exclusive access. If another thread holds an `Arc` to the value indefinitely, this can cause a livelock.
* `pub fn try_compute<Q, F>(&self, key: &Q, f: F) -> Option<bool>`
    * *where `F: FnOnce(&mut V)`*
    * Atomically applies a closure to a mutable reference of a value if and only if no other thread holds a reference (`Arc`) to it. Returns `Some(true)` on success, `Some(false)` if the value was locked by another reader and `None` if the key was not found. This is the non-blocking version of `compute`.
* `pub fn compute_val<Q, F, R>(&self, key: &Q, mut f: F) -> ComputeResult<R>`
    * *where `F: FnMut(&mut V) -> R`*
    * Waiting version of `try_compute_val`. Atomically applies a closure that returns a value. Can livelock. Returns `ComputeResult::Ok(R)` on success or `ComputeResult::NotFound` if the key does not exist.
* `pub fn try_compute_val<Q, F, R>(&self, key: &Q, f: F) -> ComputeResult<R>`
    * *where `F: FnOnce(&mut V) -> R`*
    * Non-blocking version of `compute_val`. Returns `ComputeResult::Ok(R)` on success, `ComputeResult::Fail` if the value was locked and `ComputeResult::NotFound` if the key was not found.
* `pub fn remove<Q>(&self, key: &Q) -> Option<Arc<V>>`
    * Removes an entry from the cache, returning the stored value if the key was found. The eviction listener is notified with `EvictionReason::Invalidated` and the `invalidations` metric is incremented, exactly as with `invalidate`.
* `pub fn invalidate<Q>(&self, key: &Q) -> bool`
    * Removes an entry from the cache, returning `true` if the key was found.
* `pub fn clear(&self)`
    * Removes all entries from the cache.
* `pub fn to_snapshot(&self) -> CacheSnapshot<K, V>`
    * *(feature: `serde`)*
    * *where `K: Serialize`, `V: Clone + Serialize`*
    * Creates a serializable snapshot of the cache's current state. Pauses all other operations during creation. If `maintenance_on_introspection` is enabled, pending read-access events are flushed before the snapshot is taken.
* `pub fn iter(&self) -> Iter<'_, K, V, H>`
    * Returns a weakly-consistent iterator over the key-value pairs in the cache. If `maintenance_on_introspection` is enabled, pending read-access events are flushed first.
* `pub fn iter_with_batch_size(&self, batch_size: usize) -> Iter<'_, K, V, H>`
    * Returns a weakly-consistent iterator with a custom batch size. If `maintenance_on_introspection` is enabled, pending read-access events are flushed first.
* `pub fn iter_snapshot(&self) -> SnapshotIter<'_, K, V, H>`
    * Returns a semi-consistent iterator that snapshots keys one shard at a time. If `maintenance_on_introspection` is enabled, pending read-access events are flushed first.
* `pub fn run_maintenance(&self)`
    * Refreshes the clock reads use, applies every pending read to the policy on each shard and removes every entry past its TTL or TTI with a full scan. An entry in its stale-while-revalidate grace stays. Capacity needs no pass, since an insert evicts before it returns.
* `pub fn multiget<I, Q>(&self, keys: I) -> HashMap<K, Arc<V>>`
    * *(feature: `bulk`)*
    * Retrieves multiple values from the cache, parallelizing lookups across shards.
* `pub fn multi_insert<I>(&self, items: I)`
    * *(feature: `bulk`)*
    * Inserts multiple key-value-cost items in parallel.
* `pub fn multi_remove<I, Q>(&self, keys: I) -> Vec<(K, Arc<V>)>`
    * *(feature: `bulk`)*
    * Removes multiple items in parallel, returning the key-value pairs that were present. The returned pairs are in no particular order.
* `pub fn multi_invalidate<I, Q>(&self, keys: I)`
    * *(feature: `bulk`)*
    * Removes multiple items in parallel.

### `AsyncCache`

`AsyncCache<K, V, H>` is a handle to the cache for asynchronous operations. Its methods are async equivalents of the `Cache` methods, with the same constraints.

* `pub fn to_sync(&self) -> Cache<K, V, H>`
* `pub fn metrics(&self) -> MetricsSnapshot`
    * Returns a point-in-time metrics snapshot. If `maintenance_on_introspection` is enabled, pending read-access events are flushed first.
* `pub async fn get<Q, F, R>(&self, key: &Q, f: F) -> Option<R>`
* `pub async fn fetch<Q>(&self, key: &Q) -> Option<Arc<V>>`
* `pub async fn peek<Q>(&self, key: &Q) -> Option<Arc<V>>`
* `pub async fn fetch_with(&self, key: &K) -> Arc<V>`
    * As `Cache::fetch_with`, with the load running off this task, so dropping this future leaves it running for its waiters. If the task running the load is dropped, as when its runtime shuts down, the caller and every waiter on that key panic and the next call loads again. A `TokioSpawner` keeps the runtime it was built on, so once that runtime is gone every miss panics.
* `pub async fn entry(&self, key: K) -> AsyncEntry<'_, K, V, H>`
* `pub async fn insert(&self, key: K, value: V, cost: u64)`
* `pub async fn insert_with_ttl(&self, key: K, value: V, cost: u64, ttl: Duration)`
* `pub async fn compute<Q, F>(&self, key: &Q, mut f: F) -> bool`
* `pub async fn try_compute<Q, F>(&self, key: &Q, f: F) -> Option<bool>`
* `pub async fn compute_val<Q, F, R>(&self, key: &Q, mut f: F) -> ComputeResult<R>`
* `pub async fn try_compute_val<Q, F, R>(&self, key: &Q, f: F) -> ComputeResult<R>`
* `pub async fn remove<Q>(&self, key: &Q) -> Option<Arc<V>>`
* `pub async fn invalidate<Q>(&self, key: &Q) -> bool`
* `pub async fn clear(&self)`
* `pub async fn to_snapshot(&self) -> CacheSnapshot<K, V>`
    * *(feature: `serde`)*
    * *where `K: Serialize`, `V: Clone + Serialize`*
    * If `maintenance_on_introspection` is enabled, pending read-access events are flushed before the snapshot is taken.
* `pub fn iter_stream(&self) -> IterStream<K, V, H>`
    * Returns a weakly-consistent `Stream` over the key-value pairs in the cache. If `maintenance_on_introspection` is enabled, pending read-access events are flushed first.
* `pub fn iter_stream_with_batch_size(&self, batch_size: usize) -> IterStream<K, V, H>`
    * Returns a weakly-consistent `Stream` with a custom batch size. If `maintenance_on_introspection` is enabled, pending read-access events are flushed first.
* `pub fn iter_snapshot_async(&self) -> AsyncSnapshotIter<'_, K, V, H>`
    * Returns a semi-consistent async iterator that snapshots keys one shard at a time. If `maintenance_on_introspection` is enabled, pending read-access events are flushed first.
* `pub async fn run_maintenance(&self)`
    * Async equivalent of `Cache::run_maintenance`. Awaits each shard's locks, so a task holding an `AsyncEntry` can finish.
* `pub async fn multiget<I, Q>(&self, keys: I) -> HashMap<K, Arc<V>>`
* `pub async fn multi_insert<I>(&self, items: I)`
    * *(feature: `bulk`)*
* `pub async fn multi_remove<I, Q>(&self, keys: I) -> Vec<(K, Arc<V>)>`
    * *(feature: `bulk`)*
* `pub async fn multi_invalidate<I, Q>(&self, keys: I)`
    * *(feature: `bulk`)*

### `Iter`

`Iter<'a, K, V, H>` is returned by `Cache::iter` and implements `Iterator<Item = (K, Arc<V>)>`. Its module is private, so the type cannot be named. It checks expiry on the exact clock.

### `IterStream`

`IterStream<K, V, H>` is returned by `AsyncCache::iter_stream` and implements `Stream<Item = (K, Arc<V>)>`. Its module is private, so the type cannot be named. It checks expiry on the exact clock.

### `SnapshotIter`

`SnapshotIter<'a, K, V, H>` is returned by `Cache::iter_snapshot` and implements `Iterator<Item = (K, Arc<V>)>`. Its module is private, so the type cannot be named. Each item is read through `fetch`, so it has the one-tick expiry bound, records hits and misses and resets TTI.

### `AsyncSnapshotIter`

`AsyncSnapshotIter<'a, K, V, H>` is returned by `AsyncCache::iter_snapshot_async`. Its module is private, so the type cannot be named. It is not a `Stream`. Each item is read through `fetch`, as with `SnapshotIter`.

* `pub async fn next(&mut self) -> Option<(K, Arc<V>)>`

## 4. Entry API

The Entry API atomically manipulates a single cache entry. The returned value holds the shard's write lock until it is dropped.

### `Entry` and `AsyncEntry`

`Entry<'a, K, V, H>` and `AsyncEntry<'a, K, V, H>` are a view into a single cache entry, which is either occupied or vacant.

* `Occupied(OccupiedEntry<'a, K, V, H>)` / `Occupied(AsyncOccupiedEntry<'a, K, V, H>)`
* `Vacant(VacantEntry<'a, K, V, H>)` / `Vacant(AsyncVacantEntry<'a, K, V, H>)`
* `pub fn or_insert(self, default: V, cost: u64) -> Arc<V>`
* `pub fn or_insert_with<F>(self, default: F, cost: u64) -> Arc<V>`
* `pub fn or_default(self, cost: u64) -> Arc<V>`

### `OccupiedEntry` and `AsyncOccupiedEntry`

Paths: `fibre_cache::entry_api::OccupiedEntry`, `fibre_cache::entry_api_async::AsyncOccupiedEntry`.

* `pub fn key(&self) -> &K`
* `pub fn get(&self) -> Arc<V>`

### `VacantEntry` and `AsyncVacantEntry`

Paths: `fibre_cache::entry_api::VacantEntry`, `fibre_cache::entry_api_async::AsyncVacantEntry`.

* `pub fn key(&self) -> &K`
* `pub fn insert(self, value: V, cost: u64) -> Arc<V>`
* `pub fn or_insert(self, default: V, cost: u64) -> Arc<V>`
* `pub fn or_insert_with<F>(self, default: F, cost: u64) -> Arc<V>`
* `pub fn or_default(self, cost: u64) -> Arc<V>`

## 5. Policies and Listeners

### `EvictionListener`

`EvictionListener<K, V>` receives notifications when entries are removed from the cache. Path: `fibre_cache::EvictionListener`.

* `fn on_evict(&self, key: K, value: Arc<V>, reason: EvictionReason)`
    * Called on a background thread when an entry is removed. `reason` indicates if it was due to capacity, expiration or manual invalidation. `clear()` and a replacing `insert` send nothing. Notifications go through a 128-slot channel and are dropped when it is full.

### `CachePolicy`

`CachePolicy<K, V>` is the trait for implementing an eviction policy. Path: `fibre_cache::policy::CachePolicy`. It requires `Send + Sync`.

* `fn on_access(&self, key: &K, cost: u64)`
    * Called when an item is read. Applied under the shard's write lock.
* `fn uses_access_events(&self) -> bool`
    * Defaults to `true`. A policy returning `false` from every shard lets reads skip recording accesses.
* `fn on_admit(&self, key: &K, cost: u64) -> AdmissionDecision<K>`
    * Called on an insert, under the shard's write lock.
* `fn on_remove(&self, key: &K)`
    * Called when an item is invalidated, expires or is evicted through `AdmitAndEvict`.
* `fn evict(&self, cost_to_free: u64) -> (Vec<K>, u64)`
    * Called on an insert that leaves the shard over its share of the capacity. Returns the victim keys and their total cost.
* `fn clear(&self)`
    * Clears all state from the policy.

### `AdmissionDecision`

`AdmissionDecision<K>` is the result of `on_admit`. Path: `fibre_cache::policy::AdmissionDecision`.

* `Admit`
* `Reject`: Does not stop the insert.
* `AdmitAndEvict(Vec<K>)`: The listed keys are evicted.

### Provided Policies

* `fibre_cache::policy::s3fifo::S3FifoPolicy::new(capacity: u64)`: (Default for bounded caches) A small and a main FIFO queue with a ghost set of recently evicted keys. Scan-resistant and cheaper per operation than TinyLFU.
* `fibre_cache::policy::tinylfu::TinyLfuPolicy::new(total_cache_cost_capacity: u64)`: A sophisticated policy that protects frequently used items from being evicted by infrequently used items, making it scan-resistant. It combines an LRU window with an SLRU main cache.
* `fibre_cache::policy::arc::ArcPolicy::new(capacity: usize)`: Adaptive Replacement Cache. A sophisticated policy that balances between recency (LRU) and frequency (LFU) to offer high hit rates under mixed workloads.
* `fibre_cache::policy::slru::SlruPolicy::new(capacity: u64)`: Segmented LRU. Divides the cache into probationary and protected segments to provide better scan resistance than plain LRU.
* `fibre_cache::policy::sieve::SievePolicy::new()`: A very simple and efficient eviction algorithm that provides good scan resistance with low overhead.
* `fibre_cache::policy::lru::LruPolicy::new()`: Standard Least Recently Used. Evicts the item that hasn't been accessed for the longest time.
* `fibre_cache::policy::fifo::Fifo::new()`: First-In, First-Out. Evicts items in the order they were inserted.
* `fibre_cache::policy::clock::ClockPolicy::new()`: An approximation of LRU that offers better performance under high concurrency by avoiding list manipulations on access.
* `fibre_cache::policy::random::RandomPolicy::new()`: Evicts a random item. (Requires `random` feature).
* `fibre_cache::policy::null::NullPolicy`: (Default for unbounded caches) A no-op policy that never evicts.

## 6. Supporting Types

### `TimerWheelMode`

Presets for the per-shard TTL wheel. Path: `fibre_cache::builder::TimerWheelMode`.

* `Default`: 1 s tick, 60 slots.
* `HighPrecisionShortLived`: 10 ms tick, 100 slots.
* `LowPrecisionLongLived`: 30 s tick, 120 slots.

### `EvictionReason`

Indicates why an item was removed from the cache. Path: `fibre_cache::EvictionReason`.

* `Capacity` | `Expired` | `Invalidated`

### `ComputeResult`

`ComputeResult<R>` is the result of a `compute_val` or `try_compute_val` operation. Path: `fibre_cache::error::ComputeResult`.

* `Ok(R)`: The computation was successful and returned a value of type `R`.
* `Fail`: The computation failed because another thread held a reference to the value.
* `NotFound`: The key was not found in the cache.

### `MetricsSnapshot`

A point-in-time, public-facing snapshot of the cache's performance metrics, returned by `.metrics()` on both `Cache` and `AsyncCache`.

* `pub hits: u64`
    * The number of successful lookups for keys that were present in the cache.
* `pub misses: u64`
    * The number of failed lookups for keys that were not present in the cache.
* `pub hit_ratio: f64`
    * The cache hit ratio, calculated as `hits / (hits + misses)`. A value between `0.0` and `1.0`.
* `pub inserts: u64`
    * The total number of items inserted into the cache. This includes both new items and replacements of existing items.
* `pub updates: u64`
    * The total number of in-place updates performed via the `compute` methods.
* `pub invalidations: u64`
    * The total number of items that have been manually removed via `remove`, `invalidate`, `multi_remove` or `multi_invalidate`.
* `pub evicted_by_capacity: u64`
    * The number of items evicted by an insert because the shard's cost would otherwise have exceeded its share of the capacity.
* `pub evicted_by_ttl: u64`
    * The number of items evicted because their Time-To-Live (TTL) expired.
* `pub evicted_by_tti: u64`
    * The number of items evicted because their Time-To-Idle (TTI) expired.
* `pub keys_admitted: u64`
    * Equal to `inserts` for every policy.
* `pub keys_rejected: u64`
    * Always 0. Nothing increments this counter and `AdmissionDecision::Reject` does not stop the insert.
* `pub current_cost: u64`
    * The current total cost of all items in the cache.
* `pub total_cost_added: u64`
    * The cumulative cost of all items ever inserted into the cache.
* `pub uptime_secs: u64`
    * The number of seconds the cache has been running since it was created.

### `Metrics`

The internal metrics collector. Path: `fibre_cache::metrics::Metrics`. It has no public fields or methods; read metrics through `MetricsSnapshot`.

### `CacheSnapshot`

`CacheSnapshot<K, V>` is a serializable, point-in-time snapshot of the cache's data. Path: `fibre_cache::snapshot::CacheSnapshot`. *(Requires feature `serde`)*

### `TaskSpawner`

A trait for spawning a future onto an asynchronous runtime, used by `async_loader`. Path: `fibre_cache::TaskSpawner`.

* `fn spawn(&self, future: Pin<Box<dyn Future<Output = ()> + Send>>)`

### `TokioSpawner`

The default `TaskSpawner`. Path: `fibre_cache::runtime::TokioSpawner`. *(Requires feature `tokio`)*

* `pub fn new() -> Self`
    * Uses the current Tokio runtime. Panics outside a Tokio runtime.

## 7. Error Handling

### `BuildError`

Errors that can occur when building a cache via `CacheBuilder`. Path: `fibre_cache::BuildError`.

* `ZeroCapacity`: The cache was configured with a capacity of zero, which is not allowed.
* `ZeroShards`: Returned only when a snapshot holds zero shards. `.shards(0)` gives one shard.
* `SpawnerRequired`: An `async_loader` was provided, but no `TaskSpawner` was configured and the `tokio` feature is not enabled. The crate currently needs the `tokio` feature to compile.
