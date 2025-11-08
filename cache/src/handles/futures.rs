use crate::entry::CacheEntry;
use crate::entry_api_async::{AsyncEntry, AsyncOccupiedEntry, AsyncVacantEntry};
use crate::error::ComputeResult;
use crate::iter::{AsyncSnapshotIter, IterStream, DEFAULT_ITER_BATCH_SIZE};
use crate::loader::LoadFuture;
use crate::policy::AccessEvent;
use crate::shared::CacheShared;
use crate::task::janitor::COOPERATIVE_MAINTENANCE_DRAIN_LIMIT;
use crate::{time, Cache, EvictionReason, MetricsSnapshot};

use std::borrow::Borrow;
use std::cell::Cell;
use std::hash::{BuildHasher, Hash};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use ahash::{HashMap, HashMapExt};
use equivalent::Equivalent;
use futures_util::future;

thread_local! {
  // A simple, fast Xorshift random number generator for probabilistic checks.
  // Each thread gets its own state, avoiding contention.
  static RNG: Cell<u32> = Cell::new(1);
}

// --- AsyncCache ---

/// A thread-safe, asynchronous cache.
#[derive(Debug)]
pub struct AsyncCache<K: Send, V: Send + Sync, H = ahash::RandomState> {
  pub(crate) shared: Arc<CacheShared<K, V, H>>,
}

impl<K, V, H> Clone for AsyncCache<K, V, H>
where
  K: Send,
  V: Send + Sync,
  H: BuildHasher,
{
  fn clone(&self) -> Self {
    Self {
      shared: self.shared.clone(),
    }
  }
}

impl<K, V, H> AsyncCache<K, V, H>
where
  K: Clone + Eq + Hash + Send,
  V: Send + Sync,
  H: BuildHasher + Clone,
{
  /// Converts this asynchronous `AsyncCache` into a synchronous `Cache`.
  /// This is a zero-cost conversion.
  pub fn to_sync(&self) -> Cache<K, V, H> {
    Cache {
      shared: self.shared.clone(),
    }
  }

  pub fn metrics(&self) -> MetricsSnapshot {
    return self.shared.metrics.snapshot();
  }

  /// Looks up an entry and, if found, applies a closure to the value.
  ///
  /// This is the most efficient way to read a value from the cache as it
  /// does not require cloning the underlying `Arc`. The provided closure `f`
  /// is executed while a read lock is held on the shard, so it should be fast.
  ///
  /// # Returns
  ///
  /// - `Some(R)` if the key was found, where `R` is the return value of the closure.
  /// - `None` if the key was not found.
  pub async fn get<Q, F, R>(&self, key: &Q, f: F) -> Option<R>
  where
    K: Borrow<Q>,
    Q: Eq + Hash + Equivalent<K> + ?Sized,
    F: FnOnce(&V) -> R,
  {
    let shard_index = self.shared.get_shard_index(key);
    let shard = &self.shared.store.shards[shard_index];
    let hit_info: Option<(K, Arc<CacheEntry<V>>)>;
    let result: Option<R>;

    // Scope for the ReadGuard
    {
      let guard = shard.map.read_async().await;
      if let Some((found_key, entry_in_guard)) = guard.get_key_value(key) {
        if entry_in_guard.is_expired(self.shared.time_to_idle) {
          hit_info = None;
          result = None;
        } else {
          // Execute the closure while the lock is held
          result = Some(f(entry_in_guard.value().as_ref()));
          // Clone the entry to call on_hit outside the lock
          hit_info = Some((found_key.clone(), entry_in_guard.clone()));
        }
      } else {
        hit_info = None;
        result = None;
      }
    } // guard is dropped here

    if let Some((found_key, entry_arc)) = hit_info {
      self.on_hit(found_key, &entry_arc, shard_index);
    } else if result.is_none() {
      self.shared.metrics.misses.fetch_add(1, Ordering::Relaxed);
    }

    result
  }

  /// Asynchronously fetches a value from the cache, returning a clone of the `Arc`.
  ///
  /// This operation is fast and does not block other readers. It will increment
  /// the reference count of the value's `Arc`.
  ///
  /// NOTE: Prefer get, compute will block as long as Arc ref count > 1
  pub async fn fetch<Q>(&self, key: &Q) -> Option<Arc<V>>
  where
    K: Borrow<Q>,
    Q: Eq + Hash + Equivalent<K> + ?Sized,
  {
    let shard_index = self.shared.get_shard_index(key);
    let shard = &self.shared.store.shards[shard_index];
    let hit_info: Option<(K, Arc<CacheEntry<V>>)>;

    // Scope for the ReadGuard
    {
      let guard = shard.map.read_async().await;
      if let Some((found_key, entry_in_guard)) = guard.get_key_value(key) {
        if entry_in_guard.is_expired(self.shared.time_to_idle) {
          hit_info = None;
        } else {
          // Clone necessary info while guard is active
          hit_info = Some((found_key.clone(), entry_in_guard.clone()));
        }
      } else {
        hit_info = None;
      }
    } // guard is dropped here, HL_S_ReadLock released

    if let Some((found_key, entry_arc)) = hit_info {
      // Call on_hit *after* guard is dropped
      self.on_hit(found_key, &entry_arc, shard_index);
      Some(entry_arc.value())
    } else {
      self.shared.metrics.misses.fetch_add(1, Ordering::Relaxed);
      None
    }
  }

  /// "Peeks" at a value in the cache without updating its recency or access time.
  ///
  /// This method will not update the entry's position in the eviction policy
  /// (e.g., it won't be marked as recently used in an LRU cache) and will not
  /// reset its time-to-idle timer.
  ///
  /// Returns a clone of the `Arc` containing the value if the key is found
  /// and the entry is not expired. Returns `None` otherwise.
  pub async fn peek<Q>(&self, key: &Q) -> Option<Arc<V>>
  where
    K: Borrow<Q>,
    Q: Eq + Hash + Equivalent<K> + ?Sized,
  {
    let shard = self.shared.store.get_shard(key);
    let guard = shard.map.read_async().await;

    if let Some(entry) = guard.get(key) {
      if entry.is_expired(self.shared.time_to_idle) {
        None
      } else {
        Some(entry.value())
      }
    } else {
      None
    }
  }

  /// Asynchronously creates a view into a specific entry in the map.
  ///
  /// This method acquires an async write lock on the shard for the given key,
  /// ensuring that the returned `Entry` has exclusive access.
  pub async fn entry(&self, key: K) -> AsyncEntry<'_, K, V, H> {
    let shard = self.shared.store.get_shard(&key);
    let guard = shard.map.write_async().await;

    if guard.contains_key(&key) {
      AsyncEntry::Occupied(AsyncOccupiedEntry {
        key,
        shard_guard: guard,
      })
    } else {
      AsyncEntry::Vacant(AsyncVacantEntry {
        key,
        shared: &self.shared,
        shard_guard: guard,
      })
    }
  }

  /// Inserts a key-value pair into the cache with a specific cost.
  ///
  /// In a future milestone, this will become an `async fn` to handle
  /// waiting for capacity.
  pub async fn insert(&self, key: K, value: V, cost: u64)
  where
    K: Clone + Sync,
    V: Sync,
    H: Send + Sync,
  {
    let mut new_cache_entry = CacheEntry::new(
      value,
      cost,
      self.shared.time_to_live,
      self.shared.time_to_idle,
    );

    let shard = self.shared.store.get_shard(&key);

    if let Some(wheel) = &shard.timer_wheel {
      let key_hash = crate::store::hash_key(&self.shared.store.hasher, &key);
      let ttl_handle = self
        .shared
        .time_to_live
        .map(|ttl| wheel.schedule(key_hash, ttl));
      let tti_handle = None;
      new_cache_entry.set_timer_handles(ttl_handle, tti_handle);
    }

    let old_entry: Option<Arc<CacheEntry<V>>>;
    {
      let mut guard = shard.map.write_async().await;
      old_entry = guard.insert(key.clone(), Arc::new(new_cache_entry));
    }

    if let Some(entry) = old_entry {
      if let Some(wheel) = &shard.timer_wheel {
        if let Some(handle) = &entry.ttl_timer_handle {
          wheel.cancel(handle);
        }
        if let Some(handle) = &entry.tti_timer_handle {
          wheel.cancel(handle);
        }
      }
      let old_cost = entry.cost();
      self
        .shared
        .metrics
        .current_cost
        .fetch_sub(old_cost, Ordering::Relaxed);
    }

    let _ = shard
      .event_buffer_tx
      .try_send(AccessEvent::Write(key.clone(), cost));

    self.shared.metrics.inserts.fetch_add(1, Ordering::Relaxed);
    self
      .shared
      .metrics
      .keys_admitted
      .fetch_add(1, Ordering::Relaxed);
    self
      .shared
      .metrics
      .current_cost
      .fetch_add(cost, Ordering::Relaxed);
    self
      .shared
      .metrics
      .total_cost_added
      .fetch_add(cost, Ordering::Relaxed);

    self._run_opportunistic_maintenance(&key, &shard);
  }

  /// Asynchronously inserts a key-value pair into the cache with a specific cost
  /// and Time-to-Live (TTL).
  ///
  /// This TTL overrides any global TTL that was configured on the cache builder.
  pub async fn insert_with_ttl(&self, key: K, value: V, cost: u64, ttl: Duration)
  where
    K: Clone + Sync,
    V: Sync,
    H: Send + Sync,
  {
    let expires_at = time::now_duration().as_nanos() as u64 + ttl.as_nanos() as u64;
    let mut new_cache_entry =
      CacheEntry::new_with_custom_expiry(value, cost, expires_at, self.shared.time_to_idle);

    let shard = self.shared.store.get_shard(&key);

    if let Some(wheel) = &shard.timer_wheel {
      let key_hash = crate::store::hash_key(&self.shared.store.hasher, &key);
      let ttl_handle = Some(wheel.schedule(key_hash, ttl));
      let tti_handle = None;
      new_cache_entry.set_timer_handles(ttl_handle, tti_handle);
    }

    let old_entry: Option<Arc<CacheEntry<V>>>;
    {
      let mut guard = shard.map.write_async().await;
      old_entry = guard.insert(key.clone(), Arc::new(new_cache_entry));
    }

    if let Some(entry) = old_entry {
      if let Some(wheel) = &shard.timer_wheel {
        if let Some(handle) = &entry.ttl_timer_handle {
          wheel.cancel(handle);
        }
        if let Some(handle) = &entry.tti_timer_handle {
          wheel.cancel(handle);
        }
      }
      let old_cost = entry.cost();
      self
        .shared
        .metrics
        .current_cost
        .fetch_sub(old_cost, Ordering::Relaxed);
    }

    let _ = shard
      .event_buffer_tx
      .try_send(AccessEvent::Write(key.clone(), cost));

    self.shared.metrics.inserts.fetch_add(1, Ordering::Relaxed);
    self
      .shared
      .metrics
      .keys_admitted
      .fetch_add(1, Ordering::Relaxed);
    self
      .shared
      .metrics
      .current_cost
      .fetch_add(cost, Ordering::Relaxed);
    self
      .shared
      .metrics
      .total_cost_added
      .fetch_add(cost, Ordering::Relaxed);

    self._run_opportunistic_maintenance(&key, &shard);
  }

  /// Asynchronously and atomically computes a new value for a key, waiting if necessary.
  ///
  /// This function provides a blocking, "wait-and-succeed" version of the
  /// read-modify-write pattern. It repeatedly calls `try_compute` in a loop
  /// until the modification is successful.
  ///
  /// The closure `f` will be called with a mutable reference `&mut V` to the
  /// value if the key exists.
  ///
  /// NOTE: Use the `get` method for reads where possible to mitigate blocking.
  pub async fn compute<Q, F>(&self, key: &Q, mut f: F) -> bool
  where
    K: Borrow<Q>,
    Q: Eq + Hash + Equivalent<K> + ?Sized,
    F: FnMut(&mut V),
  {
    loop {
      // We must call the async `try_compute` here.
      let opt = self.try_compute(key, &mut f).await;

      match opt {
        Some(true) => return true,
        Some(false) => {}
        None => return false,
      }
      // Yield control back to the Tokio runtime. This is the async equivalent
      // of `thread::yield_now()`, preventing the executor from being starved.
      tokio::task::yield_now().await;
    }
  }

  pub async fn try_compute<Q, F>(&self, key: &Q, f: F) -> Option<bool>
  where
    K: Borrow<Q>,
    Q: Eq + Hash + Equivalent<K> + ?Sized,
    F: FnOnce(&mut V),
  {
    return match self.try_compute_val(key, f).await {
      ComputeResult::Ok(_) => Some(true),
      ComputeResult::Fail => Some(false),
      ComputeResult::NotFound => None,
    };
  }

  /// Atomically computes a new value for a key.
  /// The provided closure is called with a mutable reference to the value
  /// if the key exists and no other threads are currently reading it.
  pub async fn try_compute_val<Q, F, R>(&self, key: &Q, f: F) -> ComputeResult<R>
  where
    K: Borrow<Q>,
    Q: Eq + Hash + Equivalent<K> + ?Sized,
    F: FnOnce(&mut V) -> R,
  {
    // The implementation is identical to the synchronous version.
    let shard = self.shared.store.get_shard(key);
    let mut guard = shard.map.write_async().await;

    if let Some(entry_arc) = guard.get_mut(key) {
      if let Some(entry) = Arc::get_mut(entry_arc) {
        if let Some(value) = Arc::get_mut(&mut entry.value) {
          let user_value = f(value);
          self.shared.metrics.updates.fetch_add(1, Ordering::Relaxed);
          return ComputeResult::Ok(user_value);
        }
      }
      // If any of the `if let` checks fail, it means we couldn't get
      // exclusive access, so the computation fails.
      return ComputeResult::Fail;
    }

    return ComputeResult::NotFound; // Key does not exist
  }

  // Atomically computes a new value for a key, waiting if necessary.
  ///
  /// This function provides a blocking, "wait-and-succeed" version of the
  /// read-modify-write pattern. It repeatedly calls `try_compute` in a loop
  /// until the modification is successful.
  ///
  /// The closure `f` will be called with a mutable reference `&mut V` to the
  /// value if the key exists.
  ///
  /// # Panics
  ///
  /// This function will not panic, but it can loop indefinitely if another
  /// thread holds an `Arc` to the value and never releases it, preventing
  /// `try_compute` from ever succeeding. This is a form of livelock.
  ///
  /// NOTE: Use the `get` method for reads where possible to mitigate blocking.
  pub async fn compute_val<Q, F, R>(&self, key: &Q, mut f: F) -> ComputeResult<R>
  where
    K: Borrow<Q>,
    Q: Eq + Hash + Equivalent<K> + ?Sized,
    F: FnMut(&mut V) -> R,
  {
    // Loop, calling the non-blocking `try_compute` until it succeeds.
    // The `FnMut` bound is necessary because the closure might be called
    // multiple times if there's a race, although that's extremely unlikely.
    // The *modification* inside the closure, however, will only be applied once.
    loop {
      let result = self.try_compute_val(key, &mut f).await;

      if !matches!(result, ComputeResult::Fail) {
        return result;
      }

      // Yield control back to the Tokio runtime. This is the async equivalent
      // of `thread::yield_now()`, preventing the executor from being starved.
      tokio::task::yield_now().await;
    }
  }

  /// Removes an entry from the cache, returning `true` if the key was found.
  pub async fn invalidate<Q>(&self, key: &Q) -> bool
  where
    K: Borrow<Q> + Clone,
    Q: Eq + Hash + Equivalent<K> + ?Sized,
    V: Sync,
  {
    let shard = self.shared.store.get_shard(key);

    let removed_entry: Option<(K, Arc<CacheEntry<V>>)>;
    {
      // New scope for the guard
      let mut guard = shard.map.write_async().await;
      removed_entry = guard.remove_entry(key);
    } // `guard` (and L_shard) is released here.

    if let Some((found_key, entry)) = removed_entry {
      if let Some(wheel) = &shard.timer_wheel {
        if let Some(handle) = &entry.ttl_timer_handle {
          wheel.cancel(handle);
        }
        if let Some(handle) = &entry.tti_timer_handle {
          wheel.cancel(handle);
        }
      }

      self.shared.get_cache_policy(key).on_remove(&found_key);
      self
        .shared
        .metrics
        .invalidations
        .fetch_add(1, Ordering::Relaxed);
      self
        .shared
        .metrics
        .current_cost
        .fetch_sub(entry.cost(), Ordering::Relaxed);

      if let Some(sender) = &self.shared.notification_sender {
        let _ = sender.try_send((found_key, entry.value(), EvictionReason::Invalidated));
      }
      true
    } else {
      false
    }
  }

  /// Asynchronously removes all entries from the cache.
  pub async fn clear(&self) {
    // 1. Asynchronously acquire all write locks.
    let mut shard_guards =
      future::join_all(self.shared.store.iter_shards().map(|s| s.map.write_async())).await;

    // 2. Iterate through each shard, notify the corresponding policy for each
    //    key being removed, and then clear the shard's map.
    for (i, guard) in shard_guards.iter_mut().enumerate() {
      let policy = &self.shared.cache_policy[i];
      for key in guard.keys() {
        policy.on_remove(key);
      }
      guard.clear();
    }

    // 3. Reset all policy states.
    for policy in self.shared.cache_policy.iter() {
      policy.clear();
    }

    // 4. Reset metrics and cost gate.
    self
      .shared
      .metrics
      .current_cost
      .store(0, std::sync::atomic::Ordering::Relaxed);
  }

  /// Private helper for cache hits.
  fn on_hit(&self, key: K, entry: &Arc<CacheEntry<V>>, shard_index: usize)
  where
    K: Clone,
  {
    if self.shared.time_to_idle.is_some() {
      entry.update_last_accessed();
    }

    // Defer the policy update by recording the access in the shard's batcher.
    let shard = &self.shared.store.shards[shard_index];
    shard
      .read_access_batcher
      .record_access(key, entry.cost(), &self.shared.store.hasher);

    self.shared.metrics.hits.fetch_add(1, Ordering::Relaxed);
  }

  /// Helper to run maintenance on a shard if the maintenance lock is not contended.
  fn _run_opportunistic_maintenance(&self, key: &K, shard: &crate::store::Shard<K, V, H>)
  where
    K: Eq + Hash + Clone + Send,
    V: Send + Sync,
    H: BuildHasher + Clone,
  {
    // The check is now a simple, fast method call on the shard's FastRng.
    if !shard
      .rng
      .should_run(self.shared.maintenance_probability_denominator)
    {
      return;
    }

    if let Some(_guard) = shard.maintenance_lock.try_lock() {
      let shard_index = self.shared.get_shard_index(key);
      let janitor_context = crate::task::janitor::JanitorContext {
        store: Arc::clone(&self.shared.store),
        metrics: Arc::clone(&self.shared.metrics),
        cache_policy: self.shared.cache_policy.clone(),
        capacity: self.shared.capacity,
        time_to_idle: self.shared.time_to_idle,
        notification_sender: self
          .shared
          .notification_sender
          .as_ref()
          .map(|val| val.clone()),
      };
      crate::task::janitor::perform_shard_maintenance(
        shard,
        shard_index,
        &janitor_context,
        COOPERATIVE_MAINTENANCE_DRAIN_LIMIT,
      );
    }
  }
}

impl<K: Send, V: Send, H> AsyncCache<K, V, H>
where
  K: Eq + Hash + Clone + Send + Sync + 'static,
  V: Send + Sync + 'static,
  H: BuildHasher + Clone + Send + 'static,
{
  /// Returns a concurrent-safe stream over the key-value pairs in the cache.
  ///
  /// The stream fetches items in batches with a default size, holding an async
  /// lock on only one shard at a time.
  ///
  /// # Consistency
  /// This stream does **not** provide a point-in-time snapshot of the cache.
  /// Items inserted after a shard has been scanned will be missed. Items may be
  /// modified or deleted by other threads while iteration is in progress.
  ///
  /// # Example
  /// ```
  /// # use fibre_cache::{AsyncCache, CacheBuilder};
  /// # use futures_util::stream::StreamExt;
  /// # #[tokio::main]
  /// # async fn main() {
  /// let cache = CacheBuilder::<u32, String>::new().build_async().unwrap();
  /// cache.insert(1, "one".to_string(), 1).await;
  ///
  /// let mut stream = cache.iter_stream();
  /// while let Some((key, value)) = stream.next().await {
  ///     println!("{}: {}", key, &*value);
  /// }
  /// # }
  /// ```
  pub fn iter_stream(&self) -> IterStream<K, V, H> {
    IterStream::new(self, DEFAULT_ITER_BATCH_SIZE)
  }

  /// Returns a concurrent-safe stream with a custom batch size.
  ///
  /// A larger batch size may have better throughput but will hold shard locks
  /// for slightly longer during batch-refill operations.
  pub fn iter_stream_with_batch_size(&self, batch_size: usize) -> IterStream<K, V, H> {
    IterStream::new(self, batch_size.max(1))
  }
  
  /// Returns an async iterator over a semi-consistent snapshot of the cache.
  ///
  /// This iterator is created by taking a point-in-time snapshot of keys from one
  /// shard at a time. It has stronger consistency than `iter_stream()` and is useful
  /// when you need a more stable view of the cache during iteration.
  ///
  /// # Consistency
  /// - For any given shard, the set of keys visited is fixed when that shard is
  ///   first scanned.
  /// - It may see items that are inserted into shards it has not yet visited.
  /// - It will not see items inserted into shards it has already passed.
  ///
  /// # Usage
  /// ```ignore
  /// let mut iter = cache.iter_snapshot_async();
  /// while let Some((key, value)) = iter.next().await {
  ///     // ...
  /// }
  /// ```
  pub fn iter_snapshot_async(&self) -> AsyncSnapshotIter<'_, K, V, H> {
    AsyncSnapshotIter::new(self)
  }
}

impl<K: Send, V: Send, H> AsyncCache<K, V, H>
where
  K: Eq + Hash + Clone + Send + Sync + 'static,
  V: Send + Sync,
  H: BuildHasher + Clone + Send,
{
  /// Asynchronously retrieves multiple values from the cache.
  ///
  /// This method is more efficient than calling `get` in a loop as it can
  /// execute lookups across different cache shards concurrently.
  ///
  /// Returns a `HashMap` containing the keys and values that were found.
  pub async fn multiget<I, Q>(&self, keys: I) -> HashMap<K, Arc<V>>
  where
    I: IntoIterator,
    I::Item: std::borrow::Borrow<Q>,
    Q: Eq + Hash + Equivalent<K> + Sync + ?Sized + ToOwned<Owned = K>,
    K: std::borrow::Borrow<Q>,
  {
    let num_shards = self.shared.store.shards.len();
    let mut keys_by_shard: Vec<Vec<K>> = Vec::with_capacity(num_shards);
    for _ in 0..num_shards {
      keys_by_shard.push(Vec::new());
    }

    let mut total_reqs = 0;

    // The structure of this loop is the same.
    // The key conversion is the only necessary change to satisfy the structure.
    for item in keys.into_iter() {
      let q: &Q = item.borrow();
      let hash = crate::store::hash_key(&self.shared.store.hasher, q);
      let index = hash as usize % num_shards;
      // This conversion is required because `keys_by_shard` must hold owned `K`s.
      keys_by_shard[index].push(q.to_owned());
      total_reqs += 1;
    }

    let get_futs = keys_by_shard
      .into_iter()
      .enumerate()
      .filter(|(_, keys)| !keys.is_empty())
      .map(|(i, shard_keys)| {
        let shared = Arc::clone(&self.shared);
        async move {
          let shard = &shared.store.shards[i];
          let guard = shard.map.read_async().await;
          let mut found = HashMap::new();

          for key in shard_keys {
            if let Some(entry) = guard.get(key.borrow()) {
              if !entry.is_expired(shared.time_to_idle) {
                if shared.time_to_idle.is_some() {
                  entry.update_last_accessed();
                }
                shared.get_cache_policy(&key).on_access(&key, entry.cost());
                found.insert(key.clone(), entry.value());
              }
            }
          }
          found
        }
      });

    let results_by_shard: Vec<HashMap<K, Arc<V>>> = future::join_all(get_futs).await;

    let mut final_map = HashMap::new();
    for map in results_by_shard {
      final_map.extend(map);
    }

    let hits = final_map.len() as u64;
    self
      .shared
      .metrics
      .hits
      .fetch_add(hits, std::sync::atomic::Ordering::Relaxed);
    self
      .shared
      .metrics
      .misses
      .fetch_add(total_reqs - hits, std::sync::atomic::Ordering::Relaxed);

    final_map
  }

  /// Inserts multiple key-value-cost triples into the cache.
  ///
  /// This operation is non-blocking and pushes write events to a queue for
  /// background processing. The cache may be temporarily over capacity until
  /// the janitor task evicts items.
  #[cfg(feature = "bulk")]
  pub async fn multi_insert<I>(&self, items: I)
  where
    I: IntoIterator<Item = (K, V, u64)>,
  {
    let num_shards = self.shared.store.iter_shards().count();

    let mut items_by_shard: Vec<Vec<(K, V, u64)>> = Vec::with_capacity(num_shards);
    for _ in 0..num_shards {
      items_by_shard.push(Vec::new());
    }

    for (key, value, cost) in items.into_iter() {
      let hash = crate::store::hash_key(&self.shared.store.hasher, &key);
      let index = hash as usize % items_by_shard.len();
      items_by_shard[index].push((key, value, cost));
    }

    // Process each shard concurrently using async tasks.
    let insert_futs = items_by_shard
      .into_iter()
      .enumerate()
      .filter(|(_, shard_items)| !shard_items.is_empty())
      .map(|(i, shard_items)| {
        let shared = Arc::clone(&self.shared);
        async move {
          let shard = &shared.store.shards[i];
          let mut guard = shard.map.write_async().await;

          for (key, value, cost) in shard_items {
            let mut new_cache_entry =
              CacheEntry::new(value, cost, shared.time_to_live, shared.time_to_idle);

            // Schedule timers for the new entry.
            if let Some(wheel) = &shard.timer_wheel {
              let key_hash = crate::store::hash_key(&shared.store.hasher, &key);
              let ttl_handle = shared.time_to_live.map(|ttl| wheel.schedule(key_hash, ttl));
              let tti_handle = None; // TTI handled by janitor sampling
              new_cache_entry.set_timer_handles(ttl_handle, tti_handle);
            }

            // Insert and handle any replaced entry.
            if let Some(old_entry) = guard.insert(key.clone(), Arc::new(new_cache_entry)) {
              // Cancel timers for the replaced entry.
              if let Some(wheel) = &shard.timer_wheel {
                if let Some(handle) = &old_entry.ttl_timer_handle {
                  wheel.cancel(handle);
                }
                if let Some(handle) = &old_entry.tti_timer_handle {
                  wheel.cancel(handle);
                }
              }
              // Adjust cost for the replaced entry.
              shared
                .metrics
                .current_cost
                .fetch_sub(old_entry.cost(), std::sync::atomic::Ordering::Relaxed);
            }

            // Update total cost and send write event to janitor.
            shared
              .metrics
              .current_cost
              .fetch_add(cost, std::sync::atomic::Ordering::Relaxed);
            let _ = shard
              .event_buffer_tx
              .try_send(AccessEvent::Write(key, cost));
          }
        }
      });

    future::join_all(insert_futs).await;
  }

  /// Removes multiple entries from the cache.
  #[cfg(feature = "bulk")]
  pub async fn multi_invalidate<I, Q>(&self, keys: I)
  where
    I: IntoIterator<Item = Q>,
    K: From<Q>,
  {
    // Group keys by shard index to minimize lock acquisitions.
    let mut keys_by_shard: Vec<Vec<K>> = vec![Vec::new(); self.shared.store.iter_shards().count()];
    for key in keys.into_iter().map(K::from) {
      let hash = crate::store::hash_key(&self.shared.store.hasher, &key);
      let index = hash as usize % keys_by_shard.len();
      keys_by_shard[index].push(key);
    }

    let invalidate_futs = keys_by_shard
      .into_iter()
      .enumerate()
      .filter(|(_, keys)| !keys.is_empty())
      .map(|(i, shard_keys)| {
        let shared = Arc::clone(&self.shared);
        async move {
          let shard = &shared.store.shards[i];
          let mut guard = shard.map.write_async().await;
          for key in shard_keys {
            if let Some(entry) = guard.remove(&key) {
              // Cancel any timers associated with the removed entry.
              if let Some(wheel) = &shard.timer_wheel {
                if let Some(handle) = &entry.ttl_timer_handle {
                  wheel.cancel(handle);
                }
                if let Some(handle) = &entry.tti_timer_handle {
                  wheel.cancel(handle);
                }
              }

              self.shared.get_cache_policy(&key).on_remove(&key);
              shared
                .metrics
                .invalidations
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
              shared
                .metrics
                .current_cost
                .fetch_sub(entry.cost(), std::sync::atomic::Ordering::Relaxed);

              if let Some(sender) = &self.shared.notification_sender {
                let _ = sender.try_send((key.clone(), entry.value(), EvictionReason::Invalidated));
              }
            }
          }
        }
      });

    future::join_all(invalidate_futs).await;
  }
}

impl<K, V, H> AsyncCache<K, V, H>
where
  K: Eq + Hash + Clone + Send + Sync + 'static,
  V: Send + Sync,
  H: BuildHasher + Clone + Send + Sync + 'static,
{
  /// Asynchronously retrieves a value for the given key, computing it with the
  /// configured loader function if the key is not present.
  ///
  /// This method provides thundering herd protection, ensuring that if multiple
  /// tasks request a missing key simultaneously, the loader function is only
  /// executed once.
  pub async fn fetch_with(&self, key: &K) -> Arc<V>
  where
    K: Clone + Send + Sync + 'static,
    V: Send + Sync + 'static,
    H: BuildHasher + Clone + Send + Sync,
  {
    let shard_index = self.shared.get_shard_index(key);
    let shard_ref = &self.shared.store.shards[shard_index]; // Keep a ref to shard for event_tx later

    // 1. Optimistic read lock
    let hit_info: Option<(K, Arc<CacheEntry<V>>)>;
    let mut is_stale_hit = false;

    // Scope for the ReadGuard
    {
      let guard = shard_ref.map.read_async().await;
      if let Some((found_key, entry_in_guard)) = guard.get_key_value(key) {
        let expires_at_nanos = entry_in_guard.expires_at.load(Ordering::Relaxed);

        if expires_at_nanos == 0 {
          // No TTL, always fresh if present
          if entry_in_guard.is_expired(self.shared.time_to_idle) {
            // Check TTI
            hit_info = None;
          } else {
            hit_info = Some((found_key.clone(), entry_in_guard.clone()));
          }
        } else {
          // Entry has a TTL
          let now_nanos = crate::time::now_duration().as_nanos() as u64;
          if now_nanos < expires_at_nanos {
            // CASE A: Fresh Hit
            if entry_in_guard.is_expired(self.shared.time_to_idle) {
              // Double check TTI
              hit_info = None;
            } else {
              hit_info = Some((found_key.clone(), entry_in_guard.clone()));
            }
          } else if let Some(grace_period) = self.shared.stale_while_revalidate {
            // CASE B: Stale Hit possible
            if now_nanos < expires_at_nanos + grace_period.as_nanos() as u64 {
              // Stale but within grace. Return stale, trigger background refresh.
              hit_info = Some((found_key.clone(), entry_in_guard.clone())); // Stale entry
              is_stale_hit = true;
              // trigger_background_load will be called later, after guard drops
            } else {
              // Fully expired past grace
              hit_info = None;
            }
          } else {
            // Expired, no SWR
            hit_info = None;
          }
        }
      } else {
        // Not in map
        hit_info = None;
      }
    } // guard is dropped here, HL_S_ReadLock released

    if let Some((found_key, ref entry_arc)) = hit_info {
      // This was a hit (either fresh or stale that can be returned)
      if is_stale_hit {
        self.trigger_background_load(&found_key);
      } else {
        // Should always be Some if value_to_return_opt is Some
        self.on_hit(found_key, entry_arc, shard_index);
      }
      return entry_arc.value();
    }

    // CASE C: Miss or fully expired.
    self.load_value_awaiting(key).await
  }

  /// Private helper to trigger a background refresh for stale items.
  fn trigger_background_load(&self, key: &K)
  where
    K: Clone + 'static,
    V: 'static,
  {
    let hash = crate::store::hash_key(&self.shared.store.hasher, &key);
    let index = hash as usize & (self.shared.pending_loads.len() - 1);
    let pending_loads_lock = &self.shared.pending_loads[index];
    if let Some(mut pending) = pending_loads_lock.try_lock() {
      if pending.contains_key(key) {
        return;
      }
      let future = Arc::new(LoadFuture::new());
      pending.insert(key.clone(), future.clone());
      CacheShared::spawn_loader_task(Arc::clone(&self.shared), key.clone(), future);
    }
  }

  /// Private helper for the "miss" path of `fetch_with_async`.
  ///
  /// This implements thundering herd protection for asynchronous callers.
  async fn load_value_awaiting(&self, key: &K) -> Arc<V>
  where
    K: Clone + Send + Sync + 'static,
    V: Send + Sync + 'static,
    H: BuildHasher + Clone + Send + Sync,
  {
    let mut am_leader = false;
    let future = loop {
      // 1. Lock the pending loads map. Note: This is a sync mutex lock,
      //    which is acceptable here because it should be held for a very
      //    short duration (a few hashmap lookups).
      let hash = crate::store::hash_key(&self.shared.store.hasher, &key);
      let index = hash as usize & (self.shared.pending_loads.len() - 1);
      let pending_loads_lock = &self.shared.pending_loads[index];
      let mut pending = pending_loads_lock.lock_async().await;

      // 2. Check for an existing `LoadFuture`.
      //    DO NOT call self.shared.raw_get(key) here to prevent AB-BA deadlock.
      //    The initial optimistic raw_get in fetch_with handles the "already cached" case.
      if let Some(existing_future) = pending.get(key) {
        // We will get a value, so this counts as a HIT for us.
        self.shared.metrics.hits.fetch_add(1, Ordering::Relaxed);
        am_leader = false;
        break existing_future.clone();
      }

      // 3. We are the "leader". This is the ONLY time a MISS is recorded.
      self.shared.metrics.misses.fetch_add(1, Ordering::Relaxed);
      // Create a new future, insert it.
      let new_future = Arc::new(LoadFuture::new());
      pending.insert(key.clone(), new_future.clone());
      am_leader = true;
      break new_future;
    }; // `pending` (MutexGuard for pending_loads) is dropped here.

    // 4. If we became the leader, spawn the loader task.
    //    This happens *after* the `pending_loads` lock is released.
    if am_leader {
      CacheShared::spawn_loader_task(Arc::clone(&self.shared), key.clone(), future.clone());
    }

    // 5. All async tasks wait on the same future.
    //    `.await` will poll the future, registering the waker and suspending
    //    the task if the value is not ready.
    (&*future).await
  }
}
