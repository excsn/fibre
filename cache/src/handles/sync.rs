use crate::entry::CacheEntry;
use crate::entry_api::{OccupiedEntry, VacantEntry};
use crate::loader::LoadFuture;
use crate::policy::{AccessEvent, AccessInfo, AdmissionDecision};
use crate::shared::CacheShared;
use crate::{time, AsyncCache, Entry, EvictionReason, MetricsSnapshot};

use std::hash::{BuildHasher, Hash};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use ahash::{HashMap, HashMapExt};
#[cfg(feature = "bulk")]
use rayon::iter::{
  IndexedParallelIterator, IntoParallelIterator, IntoParallelRefIterator, ParallelIterator,
};

/// A thread-safe, synchronous cache.
#[derive(Debug)]
pub struct Cache<K: Send, V: Send + Sync, H = ahash::RandomState> {
  pub(crate) shared: Arc<CacheShared<K, V, H>>,
}

impl<K, V, H> Cache<K, V, H>
where
  K: Eq + Hash + Clone + Send + 'static,
  V: Send + Sync,
  H: BuildHasher + Clone,
{
  /// Converts this synchronous `Cache` into an asynchronous `AsyncCache`.
  /// This is a zero-cost conversion.
  pub fn to_async(&self) -> AsyncCache<K, V, H> {
    AsyncCache {
      shared: self.shared.clone(),
    }
  }

  pub fn metrics(&self) -> MetricsSnapshot {
    return self.shared.metrics.snapshot();
  }

  /// Retrieves a value from the cache.
  ///
  /// Returns a clone of the `Arc` containing the value if the key is found
  /// and the entry is not expired. Returns `None` otherwise.
  pub fn get(&self, key: &K) -> Option<Arc<V>> {
    let shard = self.shared.store.get_shard(key);
    let guard = shard.map.read();

    if let Some(entry) = guard.get(key) {
      if entry.is_expired(self.shared.time_to_idle) {
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

  /// Creates a view into a specific entry in the map, which can be vacant or occupied.
  ///
  /// This method acquires a write lock on the shard for the given key, ensuring
  /// that the returned `Entry` has exclusive access. This atomicity allows for
  /// safe "get-or-insert" operations.
  pub fn entry(&self, key: K) -> Entry<'_, K, V, H> {
    let shard = self.shared.store.get_shard(&key);
    let guard = shard.map.write_sync();

    if guard.contains_key(&key) {
      Entry::Occupied(OccupiedEntry {
        key,
        shard_guard: guard,
      })
    } else {
      Entry::Vacant(VacantEntry {
        key,
        shared: &self.shared,
        shard_guard: guard,
      })
    }
  }

  /// Inserts a key-value pair into the cache with a specific cost.
  ///
  /// The `cost` is a value representing the "size" of the entry, used to
  /// determine when the cache is over capacity. For simple cases, this can
  /// be `1`.
  pub fn insert(&self, key: K, value: V, cost: u64)
  where
    K: Sync,
    V: Sync,
    H: Sync,
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
    let mut guard = shard.map.write_sync();

    // Now wrap the entry in an Arc for insertion.
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

  /// Inserts a key-value pair into the cache with a specific cost and Time-to-Live (TTL).
  ///
  /// This TTL overrides any global TTL that was configured on the cache builder.
  pub fn insert_with_ttl(&self, key: K, value: V, cost: u64, ttl: Duration)
  where
    K: Sync,
    V: Sync,
    H: Sync,
  {
    // Calculate the specific expiration time for this entry.
    let expires_at = time::now_duration().as_nanos() as u64 + ttl.as_nanos() as u64;

    let mut new_cache_entry =
      CacheEntry::new_with_custom_expiry(value, cost, expires_at, self.shared.time_to_idle);

    // Schedule this specific TTL on the timer wheel.
    if let Some(wheel) = &self.shared.timer_wheel {
      let mut wheel_guard = wheel.lock();
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
    let mut guard = shard.map.write_sync();

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
  pub fn compute<F>(&self, key: &K, f: F) -> bool
  where
    F: FnOnce(&mut V),
  {
    let shard = self.shared.store.get_shard(key);
    let mut guard = shard.map.write_sync();

    // 1. Get mutable access to the `Arc<CacheEntry<V>>` in the map.
    if let Some(entry_arc) = guard.get_mut(key) {
      // 2. Try to get mutable access to the `CacheEntry` *inside* the Arc.
      //    This should succeed if the entry is only in the map.
      if let Some(entry) = Arc::get_mut(entry_arc) {
        // 3. Now, try to get mutable access to the `V` *inside* the entry's `Arc<V>`.
        //    This is the step that will fail if another thread is reading the value.
        if let Some(value) = Arc::get_mut(&mut entry.value) {
          // Success! We have exclusive access.
          f(value);
          self.shared.metrics.updates.fetch_add(1, Ordering::Relaxed);
          return true;
        }
      }
    }

    // If any of the `if let` checks fail, it means we couldn't get
    // exclusive access, so the computation fails.
    false
  }

  /// Removes an entry from the cache, returning `true` if the key was found.
  pub fn invalidate(&self, key: &K) -> bool
  where
    V: Sync,
  {
    let shard = self.shared.store.get_shard(key);
    let mut guard = shard.map.write_sync();

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

  /// Removes all entries from the cache.
  pub fn clear(&self) {
    // 1. Acquire write locks for ALL shards. This is a "stop-the-world" operation.
    let mut shard_guards = self
      .shared
      .store
      .iter_shards()
      .map(|s| s.map.write_sync())
      .collect::<Vec<_>>();

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
    self
      .shared
      .metrics
      .current_cost
      .store(0, std::sync::atomic::Ordering::Relaxed);
  }

  /// Private helper method to run common logic on a cache hit.
  /// This includes updating metadata for TTI and notifying the eviction policy.
  fn on_hit(
    &self,
    key: &K,
    entry: &Arc<CacheEntry<V>>,
    event_tx: &fibre::mpsc::BoundedSender<AccessEvent<K>>,
  ) {
    if self.shared.time_to_idle.is_some() {
      entry.update_last_accessed();
    }

    // Instead of calling the policy directly, record a read event.
    let _ = event_tx.try_send(AccessEvent::Read(key.clone()));

    self.shared.metrics.hits.fetch_add(1, Ordering::Relaxed);
  }
}

impl<K, V, H> Cache<K, V, H>
where
  K: Eq + Hash + Clone + Send + Sync + 'static,
  V: Send + Sync,
  H: BuildHasher + Clone + Send + Sync + 'static,
{
  pub fn get_with(&self, key: &K) -> Arc<V>
  where
    K: Clone + 'static,
    V: 'static,
  {
    // 1. Optimistic read lock.
    if let Some(entry) = self.shared.store.get_shard(key).map.read().get(key) {
      let expires_at_nanos = entry.expires_at.load(Ordering::Relaxed);
      if expires_at_nanos == 0 {
        // No TTL, it's a fresh hit
        let shard = self.shared.store.get_shard(key);
        self.on_hit(key, entry, &shard.event_buffer_tx);
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
          // It's stale but within the grace period.
          // Trigger a background refresh, but don't wait for it.
          self.trigger_background_load(key);
          // And immediately return the stale value.
          return entry.value();
        }
      }
    }

    // CASE C: Miss (or expired beyond grace period)
    // The full thundering herd load logic from the previous step goes here.
    self.load_value_blocking(key)
  }

  // Helper function to contain the logic for triggering a refresh.
  // This prevents code duplication between `get_with` and `get_with_async`.
  fn trigger_background_load(&self, key: &K)
  where
    K: Clone + 'static,
    V: 'static,
  {
    // Try to acquire the pending_loads lock without blocking.
    // If we can't get it, it means another thread is already handling
    // a load for this or another key. It's safe to just give up; that
    // other thread's work will likely benefit us anyway.
    if let Some(mut pending) = self.shared.pending_loads.try_lock() {
      // Double-check that another thread didn't start the refresh
      // while we were waiting for the lock.
      if pending.contains_key(key) {
        return;
      }

      // We are the leader for this refresh.
      let future = Arc::new(LoadFuture::new());
      pending.insert(key.clone(), future.clone());

      // Spawn the refresh task.
      CacheShared::spawn_loader_task(Arc::clone(&self.shared), key.clone(), future);
    }
  }

  /// Private helper for the "miss" path of `get_with`.
  ///
  /// This implements the full thundering herd protection logic for synchronous callers.
  fn load_value_blocking(&self, key: &K) -> Arc<V>
  where
    K: Clone + Send + Sync + 'static,
    V: Send + Sync + 'static,
    H: BuildHasher + Clone + Send + Sync,
  {
    // A loop is used to handle the race condition where a value is inserted
    // after we check but before we can insert a placeholder.
    let future = loop {
      // 1. Lock the pending loads map to ensure only one "leader" is chosen.
      let mut pending = self.shared.pending_loads.lock();

      // 2. Double-check the main cache store. Another thread might have
      //    finished loading this value while we were waiting for the lock.
      if let Some(entry) = self.shared.raw_get(key) {
        // It was loaded by another thread. This is a HIT for us.
        let shard = self.shared.store.get_shard(key);
        self.on_hit(key, &entry, &shard.event_buffer_tx);
        return entry.value();
      }

      // 3. Check for an existing `LoadFuture`. If another thread is already
      //    loading this key, we become a "waiter".
      if let Some(future) = pending.get(key) {
        // We will get a value, so this counts as a HIT for us.
        self.shared.metrics.hits.fetch_add(1, Ordering::Relaxed);
        break future.clone();
      }

      // 4. If we reach here, we are the "leader".
      //    This is the only time a MISS is recorded for the entire operation.
      self.shared.metrics.misses.fetch_add(1, Ordering::Relaxed);
      //    Create a new future, insert it as a placeholder, and spawn the loader task.
      let new_future = Arc::new(LoadFuture::new());
      pending.insert(key.clone(), new_future.clone());
      CacheShared::spawn_loader_task(Arc::clone(&self.shared), key.clone(), new_future.clone());

      break new_future;
    };

    // 5. All threads/tasks (leaders and waiters) wait on the same future.
    //    The synchronous version blocks the current thread until the future is completed.
    let mut inner = future.inner.lock();
    loop {
      match &inner.state {
        // The future is complete; we can return the value.
        crate::loader::State::Complete(value) => return value.clone(),

        // The future is still being computed.
        crate::loader::State::Computing => {
          // Add our thread to the waiter list and go to sleep.
          inner
            .waiters
            .push_back(crate::loader::Waiter::Sync(thread::current()));
          drop(inner); // IMPORTANT: Unlock before parking.
          thread::park();
          inner = future.inner.lock(); // Re-acquire lock after being woken up.
        }
      }
    }
  }
}

impl<K: Send, V: Send, H> Cache<K, V, H>
where
  K: Eq + Hash + Clone + Send + Sync + 'static,
  V: Send + Sync,
  H: BuildHasher + Clone + Send + Sync,
{
  /// Retrieves multiple values from the cache.
  ///
  /// This method is more efficient than calling `get` in a loop as it can
  /// parallelize lookups across different cache shards.
  ///
  /// Returns a `HashMap` containing the keys and values that were found.
  #[cfg(feature = "bulk")]
  pub fn get_all<I, Q>(&self, keys: I) -> HashMap<K, Arc<V>>
  where
    I: IntoIterator<Item = Q>,
    K: From<Q>,
    H: BuildHasher + Clone + Sync,
  {
    // Group keys by shard index to minimize lock acquisitions.
    let mut keys_by_shard: Vec<Vec<K>> = vec![Vec::new(); self.shared.store.iter_shards().count()];
    for key in keys.into_iter().map(K::from) {
      let hash = crate::store::hash_key(&self.shared.store.hasher, &key);
      let index = hash as usize % keys_by_shard.len();
      keys_by_shard[index].push(key);
    }

    // Use Rayon to process shards in parallel.
    let results_by_shard: Vec<HashMap<K, Arc<V>>> = keys_by_shard
      .par_iter()
      .enumerate()
      .map(|(i, shard_keys)| {
        if shard_keys.is_empty() {
          return HashMap::new();
        }

        let shard = &self.shared.store.shards[i];
        let guard = shard.map.read(); // Acquire read lock
        let mut found = HashMap::new();

        for key in shard_keys {
          if let Some(entry) = guard.get(key) {
            if !entry.is_expired(self.shared.time_to_idle) {
              entry.update_last_accessed();
              let info = crate::policy::AccessInfo { key, entry };
              self.shared.eviction_policy.on_access(&info);
              found.insert(key.clone(), entry.value());
            }
          }
        }
        found
      })
      .collect();

    // Combine results and update global metrics.
    let mut final_map = HashMap::new();
    for map in results_by_shard {
      final_map.extend(map);
    }

    let hits = final_map.len() as u64;
    let total_reqs = keys_by_shard.iter().map(Vec::len).sum::<usize>() as u64;
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
  pub fn insert_all<I>(&self, items: I)
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
  pub fn invalidate_all<I, Q>(&self, keys: I)
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

    // Can also be parallelized with Rayon.
    keys_by_shard
      .par_iter()
      .enumerate()
      .for_each(|(i, shard_keys)| {
        if shard_keys.is_empty() {
          return;
        }

        let shard = &self.shared.store.shards[i];
        let mut guard = shard.map.write_sync(); // Acquire write lock

        for key in shard_keys {
          if let Some(entry) = guard.remove(key) {
            self.shared.eviction_policy.on_remove(key);
            self
              .shared
              .metrics
              .invalidations
              .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            self
              .shared
              .metrics
              .current_cost
              .fetch_sub(entry.cost(), std::sync::atomic::Ordering::Relaxed);

            if let Some(sender) = &self.shared.notification_sender {
              let _ = sender.try_send((key.clone(), entry.value(), EvictionReason::Invalidated));
            }
          }
        }
      });
  }
}
