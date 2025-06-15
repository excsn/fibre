use crate::entry::CacheEntry;
use crate::entry_api::{OccupiedEntry, VacantEntry};
use crate::loader::LoadFuture;
use crate::policy::AccessEvent;
use crate::shared::CacheShared;
use crate::task::janitor::COOPERATIVE_MAINTENANCE_DRAIN_LIMIT;
use crate::{time, AsyncCache, Entry, EvictionReason, MetricsSnapshot};

use std::cell::Cell;
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

thread_local! {
  // A simple, fast Xorshift random number generator for probabilistic checks.
  // Each thread gets its own state, avoiding contention.
  static RNG: Cell<u32> = Cell::new(1);
}

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
  pub fn get<F, R>(&self, key: &K, f: F) -> Option<R>
  where
    F: FnOnce(&V) -> R,
  {
    let shard_index = self.shared.get_shard_index(key);
    let shard = &self.shared.store.shards[shard_index];
    let entry_arc_opt: Option<Arc<CacheEntry<V>>>;
    let result: Option<R>;

    // Scope the read guard
    {
      let guard = shard.map.read();
      if let Some(entry_in_guard) = guard.get(key) {
        if entry_in_guard.is_expired(self.shared.time_to_idle) {
          entry_arc_opt = None;
          result = None;
        } else {
          // Execute the closure while the lock is held
          result = Some(f(entry_in_guard.value().as_ref()));
          // Clone the entry to call on_hit outside the lock
          entry_arc_opt = Some(entry_in_guard.clone());
        }
      } else {
        entry_arc_opt = None;
        result = None;
      }
    } // Read lock is dropped here.

    if let Some(entry_arc) = entry_arc_opt {
      self.on_hit(key, &entry_arc, shard_index);
    } else if result.is_none() {
      // Only count a miss if we didn't get a result inside the lock.
      self.shared.metrics.misses.fetch_add(1, Ordering::Relaxed);
    }

    result
  }

  /// Fetches a value from the cache, returning a clone of the `Arc` if the key
  /// is found and the entry is not expired.
  ///
  /// This operation is fast and does not block other readers. It will increment
  /// the reference count of the value's `Arc`.
  ///
  /// NOTE: Prefer get, compute will block as long as Arc ref count > 1
  pub fn fetch(&self, key: &K) -> Option<Arc<V>> {
    let shard_index = self.shared.get_shard_index(key);
    let shard = &self.shared.store.shards[shard_index];
    let entry_arc_opt: Option<Arc<CacheEntry<V>>>;

    // Scope the read guard to release the lock as soon as possible.
    {
      let guard = shard.map.read();
      entry_arc_opt = guard.get(key).and_then(|entry| {
        if entry.is_expired(self.shared.time_to_idle) {
          None
        } else {
          Some(entry.clone())
        }
      });
    } // Read lock is dropped here.

    if let Some(entry_arc) = entry_arc_opt {
      self.on_hit(key, &entry_arc, shard_index);
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
  pub fn peek(&self, key: &K) -> Option<Arc<V>> {
    let shard = self.shared.store.get_shard(key);
    let guard = shard.map.read();

    if let Some(entry) = guard.get(key) {
      if entry.is_expired(self.shared.time_to_idle) {
        // Do not update miss count for a peek
        None
      } else {
        // Do not update hit count or call on_hit for a peek
        Some(entry.value())
      }
    } else {
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
    let guard = shard.map.write();

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

    let shard = self.shared.store.get_shard(&key);

    // Schedule timers on this shard's specific TimerWheel.
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
      let mut guard = shard.map.write();
      old_entry = guard.insert(key.clone(), Arc::new(new_cache_entry));
    }

    if let Some(entry) = old_entry {
      // Cancel timers on this shard's specific TimerWheel.
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

    self._run_opportunistic_maintenance(&key, shard);
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
    let expires_at = time::now_duration().as_nanos() as u64 + ttl.as_nanos() as u64;

    let mut new_cache_entry =
      CacheEntry::new_with_custom_expiry(value, cost, expires_at, self.shared.time_to_idle);

    let shard = self.shared.store.get_shard(&key);

    // Schedule this specific TTL on this shard's timer wheel.
    if let Some(wheel) = &shard.timer_wheel {
      let key_hash = crate::store::hash_key(&self.shared.store.hasher, &key);
      let ttl_handle = Some(wheel.schedule(key_hash, ttl));
      let tti_handle = None;
      new_cache_entry.set_timer_handles(ttl_handle, tti_handle);
    }

    let old_entry: Option<Arc<CacheEntry<V>>>;
    {
      let mut guard = shard.map.write();
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

    self._run_opportunistic_maintenance(&key, shard);
  }

  /// Atomically computes a new value for a key, waiting if necessary.
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
  pub fn compute<F>(&self, key: &K, mut f: F)
  where
    F: FnMut(&mut V),
  {
    // Loop, calling the non-blocking `try_compute` until it succeeds.
    // The `FnMut` bound is necessary because the closure might be called
    // multiple times if there's a race, although that's extremely unlikely.
    // The *modification* inside the closure, however, will only be applied once.
    loop {
      if self.try_compute(key, &mut f) {
        // The operation succeeded.
        return;
      }
      // The operation failed because another thread is holding an Arc to the value.
      // Yield the current thread to the OS scheduler to give other threads
      // a chance to run and potentially drop their Arcs.
      thread::yield_now();
    }
  }

  /// Atomically computes a new value for a key.
  /// The provided closure is called with a mutable reference to the value
  /// if the key exists and no other threads are currently reading it.
  pub fn try_compute<F>(&self, key: &K, f: F) -> bool
  where
    F: FnOnce(&mut V),
  {
    let shard = self.shared.store.get_shard(key);
    let mut guard = shard.map.write();

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

    let removed_entry: Option<Arc<CacheEntry<V>>>;
    {
      // New scope for the guard
      let mut guard = shard.map.write();
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

      self.shared.get_cache_policy(key).on_remove(key);
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
      .map(|s| s.map.write())
      .collect::<Vec<_>>();

    // 2. Iterate through each shard, notify the corresponding policy for each
    //    key being removed, and then clear the shard's map.
    for (i, guard) in shard_guards.iter_mut().enumerate() {
      let policy = &self.shared.cache_policy[i];
      for key in guard.keys() {
        // This is the crucial step you identified.
        policy.on_remove(key);
      }
      guard.clear();
    }

    // 3. Although on_remove was called, calling policy.clear() is still a
    //    good practice to reset any other aggregate state the policy might have.
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

  /// Private helper method to run common logic on a cache hit.
  /// This includes updating metadata for TTI and notifying the eviction policy.
  fn on_hit(&self, key: &K, entry: &Arc<CacheEntry<V>>, shard_idx: usize) {
    if self.shared.time_to_idle.is_some() {
      entry.update_last_accessed();
    }

    // Defer the policy update by recording the access in the shard's batcher.
    // This is a very fast, low-contention operation.
    let shard = &self.shared.store.shards[shard_idx];
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
      // We need the shard_index to select the correct policy.
      let hash = crate::store::hash_key(&self.shared.store.hasher, key);
      let shard_index = hash as usize & (self.shared.store.shards.len() - 1);
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

impl<K, V, H> Cache<K, V, H>
where
  K: Eq + Hash + Clone + Send + Sync + 'static,
  V: Send + Sync,
  H: BuildHasher + Clone + Send + Sync + 'static,
{
  pub fn fetch_with(&self, key: &K) -> Arc<V>
  where
    K: Clone + 'static,
    V: 'static,
  {
    // 1. Optimistic read lock.
    let shard_index = self.shared.get_shard_index(key);
    if let Some(entry) = self.shared.store.shards[shard_index].map.read().get(key) {
      let expires_at_nanos = entry.expires_at.load(Ordering::Relaxed);
      if expires_at_nanos == 0 {
        // No TTL, it's a fresh hit
        self.on_hit(key, entry, shard_index);
        return entry.value();
      }

      let now_nanos = crate::time::now_duration().as_nanos() as u64;

      // CASE A: Fresh Hit
      if now_nanos < expires_at_nanos {
        self.on_hit(key, entry, shard_index);
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
  fn trigger_background_load(&self, key: &K)
  where
    K: Clone + 'static,
    V: 'static,
  {
    // Try to acquire the pending_loads lock without blocking.
    // If we can't get it, it means another thread is already handling
    // a load for this or another key. It's safe to just give up; that
    // other thread's work will likely benefit us anyway.
    let hash = crate::store::hash_key(&self.shared.store.hasher, &key);
    let index = hash as usize & (self.shared.pending_loads.len() - 1);
    let pending_loads_lock = &self.shared.pending_loads[index];
    if let Some(mut pending) = pending_loads_lock.try_lock() {
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

  /// Private helper for the "miss" path of `fetch_with`.
  ///
  /// This implements the full thundering herd protection logic for synchronous callers.
  fn load_value_blocking(&self, key: &K) -> Arc<V>
  where
    K: Clone + Send + Sync + 'static,
    V: Send + Sync + 'static,
    H: BuildHasher + Clone + Send + Sync,
  {
    let mut am_leader = false;
    let future = loop {
      // 1. Lock the pending loads map to ensure only one "leader" is chosen.
      let hash = crate::store::hash_key(&self.shared.store.hasher, &key);
      let index = hash as usize & (self.shared.pending_loads.len() - 1);
      let pending_loads_lock = &self.shared.pending_loads[index];
      let mut pending = pending_loads_lock.lock();

      // 2. Check for an existing `LoadFuture`. If another thread is already
      //    loading this key, we become a "waiter".
      //    DO NOT call self.shared.raw_get(key) here to prevent AB-BA deadlock.
      //    The initial optimistic raw_get in fetch_with handles the "already cached" case.
      if let Some(existing_future) = pending.get(key) {
        // We will get a value, so this counts as a HIT for us.
        self.shared.metrics.hits.fetch_add(1, Ordering::Relaxed);
        am_leader = false;
        break existing_future.clone();
      }

      // 3. If we reach here, we are the "leader".
      //    This is the only time a MISS is recorded for the entire operation.
      self.shared.metrics.misses.fetch_add(1, Ordering::Relaxed);
      //    Create a new future, insert it as a placeholder.
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
  pub fn multiget<I, Q>(&self, keys: I) -> HashMap<K, Arc<V>>
  where
    I: IntoIterator<Item = Q>,
    K: From<Q>,
    H: BuildHasher + Clone + Sync,
  {
    // Group keys by shard index to minimize lock acquisitions.
    let mut keys_by_shard: Vec<Vec<K>> = vec![Vec::new(); self.shared.store.iter_shards().count()];
    let mut total_reqs = 0;
    for key in keys.into_iter().map(K::from) {
      let hash = crate::store::hash_key(&self.shared.store.hasher, &key);
      let index = hash as usize % keys_by_shard.len();
      keys_by_shard[index].push(key);
      total_reqs += 1;
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
        let guard = shard.map.read();
        let mut found = HashMap::new();

        for key in shard_keys {
          if let Some(entry) = guard.get(key) {
            if !entry.is_expired(self.shared.time_to_idle) {
              // This is a hit. Update TTI and send a Read event to the janitor.
              if self.shared.time_to_idle.is_some() {
                entry.update_last_accessed();
              }

              self
                .shared
                .get_cache_policy(key)
                .on_access(key, entry.cost());
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
  pub fn multi_insert<I>(&self, items: I)
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

    // Process shards in parallel.
    items_by_shard
      .into_par_iter()
      .enumerate()
      .for_each(|(i, shard_items)| {
        if shard_items.is_empty() {
          return;
        }

        let shard = &self.shared.store.shards[i];
        let mut guard = shard.map.write();

        for (key, value, cost) in shard_items {
          let mut new_cache_entry = CacheEntry::new(
            value,
            cost,
            self.shared.time_to_live,
            self.shared.time_to_idle,
          );

          // Schedule timers for the new entry.
          if let Some(wheel) = &shard.timer_wheel {
            let key_hash = crate::store::hash_key(&self.shared.store.hasher, &key);
            let ttl_handle = self
              .shared
              .time_to_live
              .map(|ttl| wheel.schedule(key_hash, ttl));
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
            self
              .shared
              .metrics
              .current_cost
              .fetch_sub(old_entry.cost(), std::sync::atomic::Ordering::Relaxed);
          }

          // Update total cost and send write event to janitor.
          self
            .shared
            .metrics
            .current_cost
            .fetch_add(cost, std::sync::atomic::Ordering::Relaxed);
          let _ = shard
            .event_buffer_tx
            .try_send(AccessEvent::Write(key, cost));
        }
      });
  }

  /// Removes multiple entries from the cache.
  ///
  /// This is more efficient than calling `invalidate` in a loop.
  #[cfg(feature = "bulk")]
  pub fn multi_invalidate<I, Q>(&self, keys: I)
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

    // Process shards in parallel.
    keys_by_shard
      .par_iter()
      .enumerate()
      .for_each(|(i, shard_keys)| {
        if shard_keys.is_empty() {
          return;
        }

        let shard = &self.shared.store.shards[i];
        let mut guard = shard.map.write(); // Acquire write lock

        for key in shard_keys {
          if let Some(entry) = guard.remove(key) {
            // Cancel any timers associated with the removed entry.
            if let Some(wheel) = &shard.timer_wheel {
              if let Some(handle) = &entry.ttl_timer_handle {
                wheel.cancel(handle);
              }
              if let Some(handle) = &entry.tti_timer_handle {
                wheel.cancel(handle);
              }
            }

            self.shared.get_cache_policy(key).on_remove(key);
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
