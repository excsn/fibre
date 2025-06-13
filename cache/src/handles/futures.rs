use crate::entry::CacheEntry;
use crate::entry_api_async::{AsyncEntry, AsyncOccupiedEntry, AsyncVacantEntry};
use crate::loader::LoadFuture;
use crate::policy::{AccessEvent, AccessInfo, AdmissionDecision};
use crate::shared::CacheShared;
use crate::{time, Cache, Entry, EvictionReason, MetricsSnapshot};

use std::hash::{BuildHasher, Hash};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use ahash::{HashMap, HashMapExt};
use futures_util::future;
#[cfg(feature = "bulk")]
use rayon::iter::{IndexedParallelIterator, IntoParallelIterator, ParallelIterator};

// --- AsyncCache Implementation ---

/// A thread-safe, asynchronous cache.
#[derive(Debug)]
pub struct AsyncCache<K: Send, V: Send + Sync, H = ahash::RandomState> {
  pub(crate) shared: Arc<CacheShared<K, V, H>>,
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

  /// Retrieves a value from the cache.
  pub fn get(&self, key: &K) -> Option<Arc<V>> {
    let shard = self.shared.store.get_shard(key);
    let guard = shard.map.read();

    if let Some(entry) = guard.get(key) {
      if entry.is_expired(self.shared.time_to_idle) {
        // The janitor will clean this up eventually. For now, treat as a miss.
        // An active approach would be to acquire a write lock here and remove,
        // but that can lead to lock escalation and performance degradation.
        // A passive "read-only" check is faster.
        self.shared.metrics.misses.fetch_add(1, Ordering::Relaxed);
        None
      } else {
        self.on_hit(key, entry, &shard.event_buffer_tx);
        Some(entry.value())
      }
    } else {
      self.shared.metrics.misses.fetch_add(1, Ordering::Relaxed);
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

    // --- NEW: Schedule timers ---
    if let Some(wheel) = &self.shared.timer_wheel {
      let mut wheel_guard = wheel.lock();
      let key_hash = crate::store::hash_key(&self.shared.store.hasher, &key);
      let ttl_handle = self
        .shared
        .time_to_live
        .map(|ttl| wheel_guard.schedule(key_hash, ttl));
      let tti_handle = self
        .shared
        .time_to_idle
        .map(|tti| wheel_guard.schedule(key_hash, tti));
      new_cache_entry.set_timer_handles(ttl_handle, tti_handle);
    }

    let shard = self.shared.store.get_shard(&key);
    let mut guard = shard.map.write_async().await;
    let old_entry = guard.insert(key.clone(), Arc::new(new_cache_entry));

    // --- NEW: Cancel timers for the replaced entry ---
    if let Some(entry) = old_entry {
      if let Some(wheel) = &self.shared.timer_wheel {
        let mut wheel_guard = wheel.lock();
        if let Some(handle) = &entry.ttl_timer_handle {
          wheel_guard.cancel(handle);
        }
        if let Some(handle) = &entry.tti_timer_handle {
          wheel_guard.cancel(handle);
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
      .try_send(AccessEvent::Write(key, cost));

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
    // Calculate the specific expiration time for this entry.
    let expires_at = time::now_duration().as_nanos() as u64 + ttl.as_nanos() as u64;

    let mut new_cache_entry =
      CacheEntry::new_with_custom_expiry(value, cost, expires_at, self.shared.time_to_idle);

    // Schedule this specific TTL on the timer wheel.
    if let Some(wheel) = &self.shared.timer_wheel {
      let wheel_guard = wheel.lock();
      let key_hash = crate::store::hash_key(&self.shared.store.hasher, &key);
      let ttl_handle = Some(wheel_guard.schedule(key_hash, ttl));
      // TTI is still governed by the global setting, if any.
      let tti_handle = self
        .shared
        .time_to_idle
        .map(|tti| wheel_guard.schedule(key_hash, tti));
      new_cache_entry.set_timer_handles(ttl_handle, tti_handle);
    }

    let shard = self.shared.store.get_shard(&key);
    let mut guard = shard.map.write_async().await;
    let old_entry = guard.insert(key.clone(), Arc::new(new_cache_entry));

    if let Some(entry) = old_entry {
      if let Some(wheel) = &self.shared.timer_wheel {
        let mut wheel_guard = wheel.lock();
        if let Some(handle) = &entry.ttl_timer_handle {
          wheel_guard.cancel(handle);
        }
        if let Some(handle) = &entry.tti_timer_handle {
          wheel_guard.cancel(handle);
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
      .try_send(AccessEvent::Write(key, cost));

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
  }

  /// Atomically computes a new value for a key.
  /// The provided closure is called with a mutable reference to the value
  /// if the key exists and no other threads are currently reading it.
  pub async fn compute<F>(&self, key: &K, f: F) -> bool
  where
    F: FnOnce(&mut V),
  {
    // The implementation is identical to the synchronous version.
    let shard = self.shared.store.get_shard(key);
    let mut guard = shard.map.write_async().await;

    if let Some(entry_arc) = guard.get_mut(key) {
      if let Some(entry) = Arc::get_mut(entry_arc) {
        if let Some(value) = Arc::get_mut(&mut entry.value) {
          f(value);
          self.shared.metrics.updates.fetch_add(1, Ordering::Relaxed);
          return true;
        }
      }
    }
    false
  }

  /// Removes an entry from the cache, returning `true` if the key was found.
  pub async fn invalidate(&self, key: &K) -> bool
  where
    K: Clone,
    V: Sync,
  {
    let shard = self.shared.store.get_shard(key);
    let mut guard = shard.map.write_async().await;

    if let Some(entry) = guard.remove(key) {
      // --- NEW: Cancel timers ---
      if let Some(wheel) = &self.shared.timer_wheel {
        let mut wheel_guard = wheel.lock();
        if let Some(handle) = &entry.ttl_timer_handle {
          wheel_guard.cancel(handle);
        }
        if let Some(handle) = &entry.tti_timer_handle {
          wheel_guard.cancel(handle);
        }
      }

      self.shared.eviction_policy.on_remove(key);
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
        let _ = sender.try_send((key.clone(), entry.value(), EvictionReason::Invalidated));
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

    // 2. Clear each shard and notify the eviction policy.
    for guard in shard_guards.iter_mut() {
      for key in guard.keys() {
        self.shared.eviction_policy.on_remove(key);
      }
      guard.clear();
    }

    // 3. Clear the eviction policy's internal state.
    self.shared.eviction_policy.clear();

    // 4. Reset metrics and cost gate.
    let total_cost = self
      .shared
      .metrics
      .current_cost
      .swap(0, std::sync::atomic::Ordering::Relaxed);
  }

  /// Private helper for cache hits.
  fn on_hit(
    &self,
    key: &K,
    entry: &Arc<CacheEntry<V>>,
    event_tx: &fibre::mpsc::BoundedSender<AccessEvent<K>>,
  ) where
    K: Clone,
  {
    if self.shared.time_to_idle.is_some() {
      entry.update_last_accessed();
    }

    let _ = event_tx.try_send(AccessEvent::Read(key.clone()));
    self.shared.metrics.hits.fetch_add(1, Ordering::Relaxed);
  }
}

impl<K: Send, V: Send, H> AsyncCache<K, V, H>
where
  K: Eq + Hash + Clone + Send + Sync + 'static,
  V: Send + Sync,
  H: BuildHasher + Clone + Send + Sync,
{
  /// Asynchronously retrieves multiple values from the cache.
  ///
  /// This method is more efficient than calling `get` in a loop as it can
  /// execute lookups across different cache shards concurrently.
  ///
  /// Returns a `HashMap` containing the keys and values that were found.
  pub async fn get_all<I, Q>(&self, keys: I) -> HashMap<K, Arc<V>>
  where
    I: IntoIterator<Item = Q>,
    K: From<Q> + Eq + Hash + Clone + Send + Sync,
    V: Send + Sync,
    H: BuildHasher + Clone,
  {
    let num_shards = self.shared.store.iter_shards().count();
    let mut keys_by_shard: Vec<Vec<K>> = Vec::with_capacity(num_shards);
    for _ in 0..num_shards {
      keys_by_shard.push(Vec::new());
    }

    let mut total_reqs = 0;
    for key in keys.into_iter().map(K::from) {
      let hash = crate::store::hash_key(&self.shared.store.hasher, &key);
      let index = hash as usize % num_shards;
      keys_by_shard[index].push(key);
      total_reqs += 1;
    }

    // Create a future for each shard that needs to be checked.
    let get_futs = keys_by_shard
      .into_iter()
      .enumerate()
      .filter(|(_, keys)| !keys.is_empty())
      .map(|(i, shard_keys)| {
        // Clone the Arc to move it into the async block.
        let shared = Arc::clone(&self.shared);
        async move {
          let shard = &shared.store.shards[i];
          // Our read lock is sync, which is fine to use in an async context
          // as it's not expected to block for long.
          let guard = shard.map.read();
          let mut found = HashMap::new();

          for key in shard_keys {
            if let Some(entry) = guard.get(&key) {
              if !entry.is_expired(shared.time_to_idle) {
                entry.update_last_accessed();
                let info = crate::policy::AccessInfo { key: &key, entry };
                shared.eviction_policy.on_access(&info);
                found.insert(key.clone(), entry.value());
              }
            }
          }
          found
        }
      });

    // Execute all shard lookups concurrently.
    let results_by_shard: Vec<HashMap<K, Arc<V>>> = future::join_all(get_futs).await;

    // Combine results and update global metrics.
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
  /// This method first acquires the total cost for all items. If the cache is
  /// not large enough, it will block until space is freed.
  #[cfg(feature = "bulk")]
  pub async fn insert_all<I>(&self, items: I)
  where
    I: IntoIterator<Item = (K, V, u64)>,
  {
    let num_shards = self.shared.store.iter_shards().count();

    let mut items_by_shard: Vec<Vec<(K, V, u64)>> = Vec::with_capacity(num_shards);
    for _ in 0..num_shards {
      items_by_shard.push(Vec::new());
    }
    let mut total_cost = 0;

    for (key, value, cost) in items.into_iter() {
      total_cost += cost;
      let hash = crate::store::hash_key(&self.shared.store.hasher, &key);
      let index = hash as usize % items_by_shard.len();
      items_by_shard[index].push((key, value, cost));
    }

    // Once we have capacity, insert into shards.
    items_by_shard
      .into_par_iter()
      .enumerate()
      .for_each(|(i, shard_items)| {
        if shard_items.is_empty() {
          return;
        }

        let shard = &self.shared.store.shards[i];
        let mut guard = shard.map.write_sync();

        for (key, value, cost) in shard_items {
          // Here we would call the full insert logic, including admission policy check.
          // For simplicity, we'll inline a condensed version.
          let entry = Arc::new(crate::entry::CacheEntry::new(
            value,
            cost,
            self.shared.time_to_live,
            self.shared.time_to_idle,
          ));

          if let Some(old_entry) = guard.insert(key.clone(), entry.clone()) {
            self
              .shared
              .metrics
              .current_cost
              .fetch_sub(old_entry.cost(), std::sync::atomic::Ordering::Relaxed);
          }

          self
            .shared
            .metrics
            .inserts
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
          self
            .shared
            .metrics
            .current_cost
            .fetch_add(cost, std::sync::atomic::Ordering::Relaxed);
        }
      });
  }

  /// Removes multiple entries from the cache.
  ///
  /// This is more efficient than calling `invalidate` in a loop.
  #[cfg(feature = "bulk")]
  pub async fn invalidate_all<I, Q>(&self, keys: I)
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
              shared.eviction_policy.on_remove(&key);
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
  pub async fn get_with(&self, key: &K) -> Arc<V>
  where
    K: Clone + Send + Sync + 'static,
    V: Send + Sync + 'static, // V: Clone needed for the &LoadFuture Future impl
    H: BuildHasher + Clone + Send + Sync,
  {
    // 1. Optimistic read lock (this is sync and fast).
    if let Some(entry) = self.shared.store.get_shard(key).map.read().get(key) {
      let expires_at_nanos = entry.expires_at.load(Ordering::Relaxed);
      if expires_at_nanos == 0 {
        let shard = self.shared.store.get_shard(key);
        self.on_hit(key, entry, &shard.event_buffer_tx); // on_hit needs to be on AsyncCache too
        return entry.value();
      }

      let now_nanos = crate::time::now_duration().as_nanos() as u64;

      // CASE A: Fresh Hit
      if now_nanos < expires_at_nanos {
        let shard = self.shared.store.get_shard(key);
        self.on_hit(key, entry, &shard.event_buffer_tx);
        return entry.value();
      }

      // CASE B: Stale Hit
      if let Some(grace_period) = self.shared.stale_while_revalidate {
        if now_nanos < expires_at_nanos + grace_period.as_nanos() as u64 {
          self.trigger_background_load(key);
          return entry.value();
        }
      }
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
    if let Some(mut pending) = self.shared.pending_loads.try_lock() {
      if pending.contains_key(key) {
        return;
      }
      let future = Arc::new(LoadFuture::new());
      pending.insert(key.clone(), future.clone());
      CacheShared::spawn_loader_task(Arc::clone(&self.shared), key.clone(), future);
    }
  }

  /// Private helper for the "miss" path of `get_with_async`.
  ///
  /// This implements thundering herd protection for asynchronous callers.
  async fn load_value_awaiting(&self, key: &K) -> Arc<V>
  where
    K: Clone + Send + Sync + 'static,
    V: Send + Sync + 'static,
    H: BuildHasher + Clone + Send + Sync,
  {
    let future = loop {
      // 1. Lock the pending loads map. Note: This is a sync mutex lock,
      //    which is acceptable here because it should be held for a very
      //    short duration (a few hashmap lookups).
      let mut pending = self.shared.pending_loads.lock();

      // 2. Double-check the main cache store.
      if let Some(entry) = self.shared.raw_get(key) {
        // A value was loaded by another task. This is a HIT for us.
        let shard = self.shared.store.get_shard(key);
        self.on_hit(key, &entry, &shard.event_buffer_tx);
        return entry.value();
      }

      // 3. Check for an existing `LoadFuture`.
      if let Some(future) = pending.get(key) {
        // We will get a value, so this counts as a HIT for us.
        self.shared.metrics.hits.fetch_add(1, Ordering::Relaxed);
        break future.clone();
      }

      // 4. We are the "leader". This is the ONLY time a MISS is recorded.
      self.shared.metrics.misses.fetch_add(1, Ordering::Relaxed);
      // Spawn the loader task.
      let new_future = Arc::new(LoadFuture::new());
      pending.insert(key.clone(), new_future.clone());
      CacheShared::spawn_loader_task(Arc::clone(&self.shared), key.clone(), new_future.clone());

      break new_future;
    };

    // 5. All async tasks wait on the same future.
    //    `.await` will poll the future, registering the waker and suspending
    //    the task if the value is not ready.
    (&*future).await
  }
}
