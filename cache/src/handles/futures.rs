use crate::entry::CacheEntry;
use crate::entry_api_async::{AsyncEntry, AsyncOccupiedEntry, AsyncVacantEntry};
use crate::loader::LoadFuture;
use crate::policy::AccessEvent;
use crate::shared::CacheShared;
use crate::{time, Cache, EvictionReason, MetricsSnapshot};

use std::cell::Cell;
use std::hash::{BuildHasher, Hash};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use ahash::{HashMap, HashMapExt};
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
  pub async fn get(&self, key: &K) -> Option<Arc<V>> {
    let shard_index = self.shared.get_shard_index(key);
    let shard = &self.shared.store.shards[shard_index];
    let entry_arc_opt: Option<Arc<CacheEntry<V>>>;
    let value_arc_opt: Option<Arc<V>>;

    // Scope for the ReadGuard
    {
      let guard = shard.map.read_async().await;
      if let Some(entry_in_guard) = guard.get(key) {
        if entry_in_guard.is_expired(self.shared.time_to_idle) {
          entry_arc_opt = None;
          value_arc_opt = None;
        } else {
          // Clone necessary info while guard is active
          entry_arc_opt = Some(entry_in_guard.clone());
          value_arc_opt = Some(entry_in_guard.value());
        }
      } else {
        entry_arc_opt = None;
        value_arc_opt = None;
      }
    } // guard is dropped here, HL_S_ReadLock released

    if let Some(entry_arc) = entry_arc_opt {
      // Call on_hit *after* guard is dropped
      self.on_hit(key, &entry_arc, shard_index);
      value_arc_opt
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
  pub async fn peek(&self, key: &K) -> Option<Arc<V>> {
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

    let removed_entry: Option<Arc<CacheEntry<V>>>;
    {
      // New scope for the guard
      let mut guard = shard.map.write_async().await;
      removed_entry = guard.remove(key);
    } // `guard` (and L_shard) is released here.

    if let Some(entry) = removed_entry {
      if let Some(wheel) = &shard.timer_wheel {
        if let Some(handle) = &entry.ttl_timer_handle {
          wheel.cancel(handle);
        }
        if let Some(handle) = &entry.tti_timer_handle {
          wheel.cancel(handle);
        }
      }

      self.shared.get_cache_policy(&key).on_remove(key);
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
  fn on_hit(&self, key: &K, entry: &Arc<CacheEntry<V>>, shard_index: usize)
  where
    K: Clone,
  {
    if self.shared.time_to_idle.is_some() {
      entry.update_last_accessed();
    }

    // For a read hit, we can call the policy's update logic directly.
    self
      .shared
      .get_cache_policy_shard_idx(shard_index)
      .on_access(key, entry.cost());
    self.shared.metrics.hits.fetch_add(1, Ordering::Relaxed);
  }

  /// Helper to run maintenance on a shard if the maintenance lock is not contended.
  fn _run_opportunistic_maintenance(&self, key: &K, shard: &crate::store::Shard<K, V, H>)
  where
    K: Eq + Hash + Clone + Send,
    V: Send + Sync,
    H: BuildHasher + Clone,
  {
    let should_run = RNG.with(|rng| {
      let mut val = rng.get();
      // Xorshift algorithm
      val ^= val << 13;
      val ^= val >> 17;
      val ^= val << 5;
      rng.set(val);
      // Check if the lowest bits match our mask
      (val & self.shared.maintenance_probability_mask) == 0
    });

    if !should_run {
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
      crate::task::janitor::perform_shard_maintenance(shard, shard_index, &janitor_context);
    }
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
  pub async fn multiget<I, Q>(&self, keys: I) -> HashMap<K, Arc<V>>
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
          let guard = shard.map.read_async().await;
          let mut found = HashMap::new();

          for key in shard_keys {
            if let Some(entry) = guard.get(&key) {
              if !entry.is_expired(shared.time_to_idle) {
                // This is a hit. Update TTI and send a Read event to the janitor.
                if shared.time_to_idle.is_some() {
                  entry.update_last_accessed();
                }
                // For a read hit, we can call the policy's update logic directly.
                self
                  .shared
                  .get_cache_policy(&key)
                  .on_access(&key, entry.cost());
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
  pub async fn get_with(&self, key: &K) -> Arc<V>
  where
    K: Clone + Send + Sync + 'static,
    V: Send + Sync + 'static, // V: Clone needed for the &LoadFuture Future impl
    H: BuildHasher + Clone + Send + Sync,
  {
    let shard_index = self.shared.get_shard_index(key);
    let shard_ref = &self.shared.store.shards[shard_index]; // Keep a ref to shard for event_tx later

    // 1. Optimistic read lock
    let entry_arc_opt: Option<Arc<CacheEntry<V>>>;
    let value_to_return_opt: Option<Arc<V>>;
    let mut is_stale_hit = false;

    // Scope for the ReadGuard
    {
      let guard = shard_ref.map.read_async().await;
      if let Some(entry_in_guard) = guard.get(key) {
        let expires_at_nanos = entry_in_guard.expires_at.load(Ordering::Relaxed);

        if expires_at_nanos == 0 {
          // No TTL, always fresh if present
          if entry_in_guard.is_expired(self.shared.time_to_idle) {
            // Check TTI
            entry_arc_opt = None;
            value_to_return_opt = None;
          } else {
            entry_arc_opt = Some(entry_in_guard.clone());
            value_to_return_opt = Some(entry_in_guard.value());
          }
        } else {
          // Entry has a TTL
          let now_nanos = crate::time::now_duration().as_nanos() as u64;
          if now_nanos < expires_at_nanos {
            // CASE A: Fresh Hit
            if entry_in_guard.is_expired(self.shared.time_to_idle) {
              // Double check TTI
              entry_arc_opt = None;
              value_to_return_opt = None;
            } else {
              entry_arc_opt = Some(entry_in_guard.clone());
              value_to_return_opt = Some(entry_in_guard.value());
            }
          } else if let Some(grace_period) = self.shared.stale_while_revalidate {
            // CASE B: Stale Hit possible
            if now_nanos < expires_at_nanos + grace_period.as_nanos() as u64 {
              // Stale but within grace. Return stale, trigger background refresh.
              entry_arc_opt = Some(entry_in_guard.clone()); // Stale entry
              value_to_return_opt = Some(entry_in_guard.value()); // Stale value
              is_stale_hit = true;
              // trigger_background_load will be called later, after guard drops
            } else {
              // Fully expired past grace
              entry_arc_opt = None;
              value_to_return_opt = None;
            }
          } else {
            // Expired, no SWR
            entry_arc_opt = None;
            value_to_return_opt = None;
          }
        }
      } else {
        // Not in map
        entry_arc_opt = None;
        value_to_return_opt = None;
      }
    } // guard is dropped here, HL_S_ReadLock released

    if let Some(value_to_return) = value_to_return_opt {
      // This was a hit (either fresh or stale that can be returned)
      if let Some(ref entry_arc) = entry_arc_opt {
        // Should always be Some if value_to_return_opt is Some
        self.on_hit(key, entry_arc, shard_index);
      }
      if is_stale_hit {
        self.trigger_background_load(key);
      }
      return value_to_return;
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

  /// Private helper for the "miss" path of `get_with_async`.
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
      //    The initial optimistic raw_get in get_with handles the "already cached" case.
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
