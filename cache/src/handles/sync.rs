use crate::entry::CacheEntry;
use crate::entry_api::{OccupiedEntry, VacantEntry};
use crate::error::ComputeResult;
use crate::iter::{DEFAULT_ITER_BATCH_SIZE, Iter, SnapshotIter};
use crate::loader::LoadFuture;
use crate::shared::CacheShared;
use crate::{AsyncCache, Entry, EvictionReason, MetricsSnapshot, time};

use std::borrow::Borrow;
use std::cell::Cell;
use std::hash::{BuildHasher, Hash};
use std::sync::Arc;
use crate::store::DeferredRead;
use std::sync::atomic::Ordering;
use std::thread;
use std::time::Duration;

use ahash::{HashMap, HashMapExt};
use equivalent::Equivalent;
#[cfg(feature = "bulk")]
use rayon::iter::{IndexedParallelIterator, IntoParallelIterator, ParallelIterator};

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

impl<K, V, H> Clone for Cache<K, V, H>
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

impl<K, V, H> Cache<K, V, H>
where
  K: Eq + Hash + Clone + Send,
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
    self.shared.flush_for_introspection();
    return self.shared.metrics.snapshot();
  }

  /// Looks up an entry and, if found, applies a closure to the value.
  ///
  /// The provided closure `f` is executed while a read lock is held on the
  /// shard, so it should be fast.
  ///
  /// # Returns
  ///
  /// - `Some(R)` if the key was found, where `R` is the return value of the closure.
  /// - `None` if the key was not found.
  #[inline]
  pub fn get<Q, F, R>(&self, key: &Q, f: F) -> Option<R>
  where
    K: Borrow<Q>,
    Q: Eq + Hash + Equivalent<K> + ?Sized,
    F: FnOnce(&V) -> R,
  {
    let hash = crate::store::hash_key(&self.shared.store.hasher, key);
    let shard_index = self.shared.get_shard_index_from_hash(hash);
    let shard = &self.shared.store.shards[shard_index];
    let mut result = None;

    let mut deferred = None;
    {
      let guard = shard.map.read();
      if let Some((found_key, entry_in_guard)) = guard.get_key_value_hashed(key, hash) {
        if !entry_in_guard.is_expired_coarse(self.shared.time_to_idle, &self.shared.clock) {
          result = Some(f(&*entry_in_guard.value));
          deferred = self.on_hit(found_key, hash, entry_in_guard, shard_index, &guard);
        }
      }
    }
    if let Some(read) = deferred {
      shard.map.write().apply_deferred(read);
    }

    if result.is_some() {
      self.shared.metrics.record_hits(1);
    } else {
      self.shared.metrics.record_misses(1);
    }

    result
  }

  /// Fetches a value from the cache, returning a clone of the `Arc` if the key
  /// is found and the entry is not expired.
  ///
  /// Expiry is judged on the janitor's clock, refreshed once per tick, so an
  /// entry can still be returned for up to one tick past its deadline. See
  /// `CacheBuilder::janitor_tick_interval`.
  ///
  /// This operation is fast and does not block other readers. It will increment
  /// the reference count of the value's `Arc`.
  ///
  /// NOTE: Prefer get, compute will block as long as Arc ref count > 1
  #[inline]
  pub fn fetch<Q>(&self, key: &Q) -> Option<Arc<V>>
  where
    K: Borrow<Q>,
    Q: Eq + Hash + Equivalent<K> + ?Sized,
  {
    let hash = crate::store::hash_key(&self.shared.store.hasher, key);
    let shard_index = self.shared.get_shard_index_from_hash(hash);
    let shard = &self.shared.store.shards[shard_index];
    let mut value = None;

    let mut deferred = None;
    {
      let guard = shard.map.read();
      if let Some((found_key, entry_in_guard)) = guard.get_key_value_hashed(key, hash) {
        if !entry_in_guard.is_expired_coarse(self.shared.time_to_idle, &self.shared.clock) {
          deferred = self.on_hit(found_key, hash, entry_in_guard, shard_index, &guard);
          value = Some(entry_in_guard.value());
        }
      }
    }
    if let Some(read) = deferred {
      shard.map.write().apply_deferred(read);
    }

    if value.is_some() {
      self.shared.metrics.record_hits(1);
    } else {
      self.shared.metrics.record_misses(1);
    }

    value
  }

  /// "Peeks" at a value in the cache without updating its recency or access time.
  ///
  /// This method will not update the entry's position in the eviction policy
  /// (e.g., it won't be marked as recently used in an LRU cache) and will not
  /// reset its time-to-idle timer.
  ///
  /// Returns a clone of the `Arc` containing the value if the key is found
  /// and the entry is not expired. Returns `None` otherwise.
  pub fn peek<Q>(&self, key: &Q) -> Option<Arc<V>>
  where
    K: Borrow<Q>,
    Q: Eq + Hash + Equivalent<K> + ?Sized,
  {
    let shard = self.shared.store.get_shard(key);
    let guard = shard.map.read();

    if let Some(entry) = guard.get(key) {
      if entry.is_expired_coarse(self.shared.time_to_idle, &self.shared.clock) {
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
    let new_cache_entry = CacheEntry::new(
      value,
      cost,
      self.shared.time_to_live,
      self.shared.time_to_idle,
    );

    let key_hash = crate::store::hash_key(&self.shared.store.hasher, &key);
    let shards = &self.shared.store.shards;
    let shard_index = key_hash as usize & (shards.len() - 1);
    let shard = &*shards[shard_index];

    let _replaced = {
      let mut guard = shard.map.write();
      guard.insert_hashed(key, new_cache_entry, key_hash)
    };




  }

  /// Inserts a key-value pair into the cache with a specific cost and Time-to-Live (TTL).
  ///
  /// This TTL overrides any global TTL that was configured on the cache builder.
  /// Reads honour the TTL and the janitor removes the entry at expiry. The first
  /// call starts the janitor if the cache has none.
  pub fn insert_with_ttl(&self, key: K, value: V, cost: u64, ttl: Duration)
  where
    K: Sync,
    V: Sync,
    H: Sync,
  {
    self.shared.ensure_janitor();
    let expires_at = time::now_duration().as_nanos() as u64 + ttl.as_nanos() as u64;

    let new_cache_entry =
      CacheEntry::new_with_custom_expiry(value, cost, expires_at, self.shared.time_to_idle);

    let key_hash = crate::store::hash_key(&self.shared.store.hasher, &key);
    let shards = &self.shared.store.shards;
    let shard_index = key_hash as usize & (shards.len() - 1);
    let shard = &*shards[shard_index];

    let _replaced = {
      let mut guard = shard.map.write();
      guard.insert_hashed(key, new_cache_entry, key_hash)
    };




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
  /// # Livelock
  ///
  /// This function can loop indefinitely if another thread holds an `Arc` to the
  /// value and never releases it, preventing `try_compute` from ever succeeding.
  ///
  /// NOTE: Use the `get` method for reads where possible to mitigate blocking.
  pub fn compute<Q, F>(&self, key: &Q, mut f: F) -> bool
  where
    K: Borrow<Q>,
    Q: Eq + Hash + Equivalent<K> + ?Sized,
    F: FnMut(&mut V),
  {
    // Loop, calling the non-blocking `try_compute` until it succeeds.
    // The `FnMut` bound is necessary because the closure might be called
    // multiple times if there's a race, although that's extremely unlikely.
    // The *modification* inside the closure, however, will only be applied once.
    loop {
      let opt = self.try_compute(key, &mut f);

      match opt {
        Some(true) => return true,
        Some(false) => {}
        None => return false,
      }
      // The operation failed because another thread is holding an Arc to the value.
      // Yield the current thread to the OS scheduler to give other threads
      // a chance to run and potentially drop their Arcs.
      thread::yield_now();
    }
  }

  pub fn try_compute<Q, F>(&self, key: &Q, f: F) -> Option<bool>
  where
    K: Borrow<Q>,
    Q: Eq + Hash + Equivalent<K> + ?Sized,
    F: FnOnce(&mut V),
  {
    return match self.try_compute_val(key, f) {
      ComputeResult::Ok(_) => Some(true),
      ComputeResult::Fail => Some(false),
      ComputeResult::NotFound => None,
    };
  }

  /// Atomically computes a new value for a key.
  /// The provided closure is called with a mutable reference to the value
  /// if the key exists and no other threads are currently reading it.
  pub fn try_compute_val<Q, F, R>(&self, key: &Q, f: F) -> ComputeResult<R>
  where
    K: Borrow<Q>,
    Q: Eq + Hash + Equivalent<K> + ?Sized,
    F: FnOnce(&mut V) -> R,
  {
    let shard = self.shared.store.get_shard(key);
    let mut guard = shard.map.write();

    if let Some(entry) = guard.get_mut(key) {
      // Fails while another thread holds an `Arc` to the value.
      if let Some(value) = Arc::get_mut(&mut entry.value) {
        let user_value = f(value);
        self.shared.metrics.updates.fetch_add(1, Ordering::Relaxed);
        return ComputeResult::Ok(user_value);
      }

      // If any of the `if let` checks fail, it means we couldn't get
      // exclusive access, so the computation fails.
      return ComputeResult::Fail;
    }

    return ComputeResult::NotFound; // Key does not exist
  }

  /// Atomically computes a new value for a key, waiting if necessary.
  ///
  /// This function provides a blocking, "wait-and-succeed" version of the
  /// read-modify-write pattern. It repeatedly calls `try_compute_val` in a loop
  /// until the modification is successful.
  ///
  /// The closure `f` will be called with a mutable reference `&mut V` to the
  /// value if the key exists.
  ///
  /// # Livelock
  ///
  /// This function can loop indefinitely if another thread holds an `Arc` to the
  /// value and never releases it, preventing `try_compute_val` from ever
  /// succeeding.
  ///
  /// NOTE: Use the `get` method for reads where possible to mitigate blocking.
  pub fn compute_val<Q, F, R>(&self, key: &Q, mut f: F) -> ComputeResult<R>
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
      let result = self.try_compute_val(key, &mut f);

      if !matches!(result, ComputeResult::Fail) {
        return result;
      }

      // The operation failed because another thread is holding an Arc to the value.
      // Yield the current thread to the OS scheduler to give other threads
      // a chance to run and potentially drop their Arcs.
      thread::yield_now();
    }
  }

  /// Removes an entry from the cache, returning the stored value if the key
  /// was found.
  ///
  /// The eviction listener is notified with `EvictionReason::Invalidated` and
  /// the `invalidations` metric is incremented, exactly as with `invalidate`.
  pub fn remove<Q>(&self, key: &Q) -> Option<Arc<V>>
  where
    K: Borrow<Q>,
    Q: Eq + Hash + Equivalent<K> + ?Sized,
    V: Sync,
  {
    let shard = self.shared.store.get_shard(key);

    let removed_entry: Option<(K, CacheEntry<V>)>;
    {
      // New scope for the guard
      let mut guard = shard.map.write();
      removed_entry = guard.remove_entry(key);
    } // `guard` (and L_shard) is released here.

    if let Some((found_key, entry)) = removed_entry {
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

      let value = entry.value();
      if let Some(sender) = &self.shared.notification_sender {
        let _ = sender.try_send((found_key, value.clone(), EvictionReason::Invalidated));
      }
      Some(value)
    } else {
      None
    }
  }

  /// Removes an entry from the cache, returning `true` if the key was found.
  pub fn invalidate<Q>(&self, key: &Q) -> bool
  where
    K: Borrow<Q>,
    Q: Eq + Hash + Equivalent<K> + ?Sized,
    V: Sync,
  {
    self.remove(key).is_some()
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

    // 2. Clear each shard; the shard map clears its policy.
    for guard in shard_guards.iter_mut() {
      guard.clear();
    }

  }

  /// Returns a concurrent-safe iterator over the key-value pairs in the cache.
  ///
  /// The iterator fetches items in batches, holding a lock on only one shard
  /// at a time for a very brief period.
  ///
  /// # Consistency
  ///
  /// This iterator does **not** provide a point-in-time snapshot of the cache.
  /// Items inserted after a shard has been scanned will be missed. Items may be
  /// modified or deleted by other threads while iteration is in progress.
  ///
  /// # Example
  /// ```
  /// # use fibre_cache::{Cache, CacheBuilder};
  /// // The Cache must be constructed from the builder using .build()
  /// let cache = CacheBuilder::<u32, String>::new().build().unwrap();
  ///
  /// cache.insert(1, "one".to_string(), 1);
  /// cache.insert(2, "two".to_string(), 1);
  ///
  /// for (key, value) in cache.iter() {
  ///     println!("{}: {}", key, &*value);
  /// }
  /// ```
  pub fn iter(&self) -> Iter<'_, K, V, H> {
    self.shared.flush_for_introspection();
    Iter::new(self, DEFAULT_ITER_BATCH_SIZE)
  }

  /// Returns a concurrent-safe iterator with a custom batch size.
  ///
  /// A larger batch size may have better throughput but will hold shard locks
  /// for slightly longer during batch-refill operations.
  pub fn iter_with_batch_size(&self, batch_size: usize) -> Iter<'_, K, V, H> {
    self.shared.flush_for_introspection();
    Iter::new(self, batch_size.max(1))
  }

  /// Returns an iterator over a semi-consistent snapshot of the cache.
  ///
  /// This iterator is created by taking a point-in-time snapshot of keys from one
  /// shard at a time. It has stronger consistency than `iter()` and is useful
  /// when you need a more stable view of the cache during iteration.
  ///
  /// # Consistency
  /// - For any given shard, the set of keys visited is fixed when that shard is
  ///   first scanned.
  /// - It may see items that are inserted into shards it has not yet visited.
  /// - It will not see items inserted into shards it has already passed.
  ///
  /// # Performance
  /// This iterator is more memory-efficient than snapshotting the entire key set
  /// at once, using memory proportional to the size of the largest shard.
  pub fn iter_snapshot(&self) -> SnapshotIter<'_, K, V, H> {
    self.shared.flush_for_introspection();
    SnapshotIter::new(self)
  }

  /// Private helper method to run common logic on a cache hit.
  /// This includes updating metadata for TTI and notifying the eviction policy.
  #[inline]
  fn on_hit(&self, key: &K, _hash: u64, entry: &CacheEntry<V>, shard_idx: usize, map: &crate::store::ShardMap<K, V, H>) -> Option<DeferredRead<K>> {
    if self.shared.time_to_idle.is_some() {
      entry.update_last_accessed_coarse(&self.shared.clock);
    }

    match self.shared.builtin_reads {
      crate::shared::BuiltinReads::Ring => {
        let node = entry.node.load(Ordering::Relaxed);
        if self.shared.store.shards[shard_idx].read_ring.push(node) {
          None
        } else {
          Some(DeferredRead::Node(node))
        }
      }
      crate::shared::BuiltinReads::Freq => {
        map.touch_read(entry.node.load(Ordering::Relaxed));
        None
      }
      crate::shared::BuiltinReads::None => {
        if !self.shared.track_reads {
          return None;
        }
        let cost = entry.cost();
        if self.shared.store.shards[shard_idx].read_log.push(key, cost) {
          None
        } else {
          Some(DeferredRead::Key(key.clone(), cost))
        }
      }
    }
  }

}

impl<K, V, H> Cache<K, V, H>
where
  K: Eq + Hash + Clone + Send + Sync + 'static,
  V: Send + Sync,
  H: BuildHasher + Clone + Send + Sync + 'static,
{
  /// Retrieves a value, computing it with the configured loader on a miss.
  ///
  /// One load runs per key however many callers wait on it. A sync loader runs on
  /// the calling thread. A stale entry in its `stale_while_revalidate` grace is
  /// returned at once and refreshed in the background.
  ///
  /// # Panics
  ///
  /// Panics on a miss when the cache has no loader. Panics when the loader panics
  /// for this key or an `async_loader`'s task is dropped, as when its runtime shuts
  /// down, in the caller and in every waiter on it; the next call loads again.
  /// Panics when a sync loader calls `fetch_with` on its own key. A loader cycle
  /// across threads (A loads B loads A) is not detected and hangs.
  pub fn fetch_with(&self, key: &K) -> Arc<V>
  where
    K: Clone + 'static,
    V: 'static,
  {
    let hash = crate::store::hash_key(&self.shared.store.hasher, key);
    let shard_index = self.shared.get_shard_index_from_hash(hash);
    let shard = &self.shared.store.shards[shard_index];

    let mut deferred = None;
    let mut stale = false;
    let hit_value = {
      let guard = shard.map.read();
      if let Some((found_key, entry)) = guard.get_key_value_hashed(key, hash) {
        let expires_at_nanos = entry.expires_at.load(Ordering::Relaxed);
        if expires_at_nanos == 0 {
          // No TTL, fresh hit.
          if !entry.is_expired_coarse(self.shared.time_to_idle, &self.shared.clock) {
            deferred = self.on_hit(found_key, hash, entry, shard_index, &guard);
            self.shared.metrics.record_hits(1);
            Some(entry.value())
          } else {
            None
          }
        } else {
          let now_nanos = crate::time::now_duration().as_nanos() as u64;
          if now_nanos < expires_at_nanos {
            // CASE A: Fresh Hit
            if !entry.is_expired_coarse(self.shared.time_to_idle, &self.shared.clock) {
              deferred = self.on_hit(found_key, hash, entry, shard_index, &guard);
              self.shared.metrics.record_hits(1);
              Some(entry.value())
            } else {
              None
            }
          } else if let Some(grace_period) = self.shared.stale_while_revalidate {
            // CASE B: Stale Hit
            if now_nanos < expires_at_nanos + grace_period.as_nanos() as u64 {
              stale = true;
              Some(entry.value())
            } else {
              None
            }
          } else {
            None
          }
        }
      } else {
        None
      }
    };
    if let Some(read) = deferred {
      shard.map.write().apply_deferred(read);
    }
    if stale {
      self.trigger_background_load(key);
    }

    if let Some(val) = hit_value {
      return val;
    }

    // CASE C: Miss (or expired beyond grace period)
    self.load_value_blocking(key)
  }

  // Helper function to contain the logic for triggering a refresh.
  fn trigger_background_load(&self, key: &K)
  where
    K: Clone + 'static,
    V: 'static,
  {
    if self.shared.loader.is_none() {
      return;
    }
    let hash = crate::store::hash_key(&self.shared.store.hasher, &key);
    let index = hash as usize & (self.shared.pending_loads.len() - 1);
    let mut pending = self.shared.pending_loads[index].lock();
    if pending.get(key).is_some_and(|load| !load.is_dropped()) {
      return;
    }
    let future = Arc::new(LoadFuture::queued());
    pending.insert(key.clone(), future.clone());
    drop(pending);
    CacheShared::enqueue_refresh(&self.shared, key.clone(), future);
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
    let inline_loader = match &self.shared.loader {
      Some(crate::loader::Loader::Sync(loader)) => Some(loader.clone()),
      Some(crate::loader::Loader::Async(_)) => None,
      None => panic!("{}", crate::loader::NO_LOADER),
    };
    let me = thread::current().id();

    // Only one leader is chosen per key. The cached value is not read again
    // here, to keep the shard lock out of this lock's scope.
    let hash = crate::store::hash_key(&self.shared.store.hasher, &key);
    let index = hash as usize & (self.shared.pending_loads.len() - 1);
    let (future, am_leader) = {
      let mut pending = self.shared.pending_loads[index].lock();
      if let Some(existing) = pending.get(key).filter(|load| !load.is_dropped()) {
        if existing.claim() {
          // A refresh still in the queue: this miss runs it now.
          self.shared.metrics.record_misses(1);
          if inline_loader.is_some() {
            let _ = existing.leader.set(me);
          }
          (existing.clone(), true)
        } else {
          // We will get a value, so this counts as a HIT for us.
          self.shared.metrics.record_hits(1);
          (existing.clone(), false)
        }
      } else {
        // This is the only time a MISS is recorded for the entire operation.
        self.shared.metrics.record_misses(1);
        let future = Arc::new(LoadFuture::new(inline_loader.is_some().then_some(me)));
        pending.insert(key.clone(), future.clone());
        (future, true)
      }
    };

    if am_leader {
      match inline_loader {
        Some(loader) => match self.shared.run_sync_load(&*loader, key.clone(), &future) {
          Ok(value) => return value,
          Err(panic) => std::panic::resume_unwind(panic),
        },
        None => CacheShared::spawn_loader_task(Arc::clone(&self.shared), key.clone(), future.clone(), false),
      }
    } else if future.leader.get() == Some(&me) {
      panic!("{}", crate::loader::LOADER_REENTERED);
    }

    let mut inner = future.inner.lock();
    loop {
      match &inner.state {
        crate::loader::State::Complete(value) => return value.clone(),
        crate::loader::State::Panicked => panic!("{}", crate::loader::LOADER_PANICKED),
        crate::loader::State::Dropped => panic!("{}", crate::loader::LOAD_DROPPED),
        crate::loader::State::Computing => {
          if future.is_dropped() {
            future.drop_load_locked(&mut inner);
            continue;
          }
          inner
            .waiters
            .push_back(crate::loader::Waiter::Sync(thread::current()));
          drop(inner);
          if !future.is_dropped() {
            thread::park();
          }
          inner = future.inner.lock();
        }
      }
    }
  }

  /// Forces a synchronous maintenance pass across all shards.
  ///
  /// Refreshes the clock reads use, applies every pending read to the policy on
  /// each shard and removes every entry past its TTL or TTI with a full scan. An
  /// entry in its stale-while-revalidate grace stays. Capacity needs no pass: an
  /// insert evicts before it returns.
  ///
  /// Primarily useful for deterministic testing and manual cache management.
  pub fn run_maintenance(&self) {
    use crate::task::janitor::JanitorContext;

    let janitor_context = JanitorContext {
      store: Arc::clone(&self.shared.store),
      clock: Arc::clone(&self.shared.clock),
      metrics: Arc::clone(&self.shared.metrics),
      time_to_idle: self.shared.time_to_idle,
      notification_sender: self.shared.notification_sender.as_ref().map(|s| s.clone()),
    };

    self.shared.clock.refresh();
    let now = crate::time::now_duration().as_nanos() as u64;
    for shard in self.shared.store.shards.iter() {
      let _guard = shard.maintenance_lock.lock();
      let expired = shard.map.write().expire_all(now);
      janitor_context.account_expired(expired);
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
    // The input `I` must be convertible into a parallel iterator.
    I: IntoParallelIterator,
    // The items from the iterator must be borrowable as `&Q` and thread-safe.
    I::Item: Borrow<Q> + Send + Sync,
    // The cache's key `K` must be borrowable as `&Q` for HashMap lookups.
    K: Borrow<Q>,
    // `Q` is the borrowed type (e.g., `str`) that can be used for lookups.
    Q: Eq + Hash + Equivalent<K> + ?Sized,
  {
    // We collect the keys first to get a definite count for metrics.
    // This does not clone the key data itself, just the iterator items (e.g., `&str`).
    let keys_vec: Vec<I::Item> = keys.into_par_iter().collect();
    let total_reqs = keys_vec.len() as u64;

    let final_map = keys_vec
      .into_par_iter()
      // Use fold/reduce, a powerful pattern for parallel aggregation.
      .fold(
        // 1. Identity: Each thread gets its own empty HashMap to work with.
        || HashMap::new(),
        // 2. Fold: This closure is run in parallel by many threads.
        // It takes a thread-local accumulator (`mut acc`) and one item from the input.
        |mut acc, key_item| {
          // Get the borrowed `&Q` from the iterator item.
          let q: &Q = key_item.borrow();

          let hash = crate::store::hash_key(&self.shared.store.hasher, q);
          let shard_index = self.shared.get_shard_index_from_hash(hash);
          let shard = &self.shared.store.shards[shard_index];

          let mut deferred = None;
          let hit_value: Option<(K, Arc<V>)> = {
            let guard = shard.map.read();
            if let Some((found_key, entry)) = guard.get_key_value_hashed(q, hash) {
              if !entry.is_expired_coarse(self.shared.time_to_idle, &self.shared.clock) {
                deferred = self.on_hit(found_key, hash, entry, shard_index, &guard);
                Some((found_key.clone(), entry.value()))
              } else {
                None
              }
            } else {
              None
            }
          };
          if let Some(read) = deferred {
            shard.map.write().apply_deferred(read);
          }

          if let Some((key_clone, value)) = hit_value {
            acc.insert(key_clone, value);
          }

          acc
        },
      )
      // 3. Reduce: Merge the HashMaps from all threads into a single one.
      .reduce(
        || HashMap::new(),
        |mut a, b| {
          a.extend(b);
          a
        },
      );

    // Update global metrics after all parallel work is done. This is one
    // bulk update per multiget call, so a single stripe is fine.
    let hits = final_map.len() as u64;
    self.shared.metrics.record_hits(hits);
    if total_reqs > hits {
      self.shared.metrics.record_misses(total_reqs - hits);
    }

    final_map
  }

  /// Inserts multiple key-value-cost triples into the cache.
  ///
  /// Groups items by shard and inserts each group under that shard's write lock.
  /// Capacity is enforced during the insert.
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
          let new_cache_entry = CacheEntry::new(
            value,
            cost,
            self.shared.time_to_live,
            self.shared.time_to_idle,
          );
          guard.insert(key.clone(), new_cache_entry);
        }
      });
  }

  /// Removes multiple entries from the cache, returning the key-value pairs
  /// that were present.
  ///
  /// This is more efficient than calling `remove` in a loop. The returned
  /// pairs are in no particular order.
  #[cfg(feature = "bulk")]
  pub fn multi_remove<I, Q>(&self, keys: I) -> Vec<(K, Arc<V>)>
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
    let removed_by_shard: Vec<Vec<(K, Arc<V>)>> = keys_by_shard
      .into_par_iter()
      .enumerate()
      .map(|(i, shard_keys)| {
        let mut removed = Vec::new();
        if shard_keys.is_empty() {
          return removed;
        }

        let shard = &self.shared.store.shards[i];
        let mut guard = shard.map.write(); // Acquire write lock

        for key in shard_keys {
          if let Some(entry) = guard.remove(&key) {
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

            let value = entry.value();
            if let Some(sender) = &self.shared.notification_sender {
              let _ = sender.try_send((key.clone(), value.clone(), EvictionReason::Invalidated));
            }
            removed.push((key, value));
          }
        }
        removed
      })
      .collect();

    removed_by_shard.into_iter().flatten().collect()
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
    let _ = self.multi_remove(keys);
  }
}
