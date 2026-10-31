use crate::entry::CacheEntry;
use crate::entry_api_async::{AsyncEntry, AsyncOccupiedEntry, AsyncVacantEntry};
use crate::error::ComputeResult;
use crate::iter::{AsyncSnapshotIter, IterStream, DEFAULT_ITER_BATCH_SIZE};
use crate::loader::LoadFuture;
use crate::shared::CacheShared;
use crate::{time, Cache, EvictionReason, MetricsSnapshot};

use std::borrow::Borrow;
use std::cell::Cell;
use std::hash::{BuildHasher, Hash};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use crate::store::DeferredRead;
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
    self.shared.try_flush_for_introspection();
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
  pub async fn get<Q, F, R>(&self, key: &Q, f: F) -> Option<R>
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
      let guard = shard.map.read_async().await;
      if let Some((found_key, entry_in_guard)) = guard.get_key_value_hashed(key, hash) {
        if !entry_in_guard.is_expired_coarse(self.shared.time_to_idle, &self.shared.clock) {
          result = Some(f(&*entry_in_guard.value));
          deferred = self.on_hit(found_key, hash, entry_in_guard, shard_index, &guard);
        }
      }
    }

    if let Some(read) = deferred {
      shard.map.write_async().await.apply_deferred(read);
    }

    if result.is_some() {
      self.shared.metrics.record_hits(1);
    } else {
      self.shared.metrics.record_misses(1);
    }

    result
  }

  /// Asynchronously fetches a value from the cache, returning a clone of the `Arc`.
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
  pub async fn fetch<Q>(&self, key: &Q) -> Option<Arc<V>>
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
      let guard = shard.map.read_async().await;
      if let Some((found_key, entry_in_guard)) = guard.get_key_value_hashed(key, hash) {
        if !entry_in_guard.is_expired_coarse(self.shared.time_to_idle, &self.shared.clock) {
          deferred = self.on_hit(found_key, hash, entry_in_guard, shard_index, &guard);
          value = Some(entry_in_guard.value());
        }
      }
    }

    if let Some(read) = deferred {
      shard.map.write_async().await.apply_deferred(read);
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
  pub async fn peek<Q>(&self, key: &Q) -> Option<Arc<V>>
  where
    K: Borrow<Q>,
    Q: Eq + Hash + Equivalent<K> + ?Sized,
  {
    let shard = self.shared.store.get_shard(key);
    let guard = shard.map.read_async().await;

    if let Some(entry) = guard.get(key) {
      if entry.is_expired_coarse(self.shared.time_to_idle, &self.shared.clock) {
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
  /// Capacity is enforced before this returns.
  pub async fn insert(&self, key: K, value: V, cost: u64)
  where
    K: Clone + Sync,
    V: Sync,
    H: Send + Sync,
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
      let mut guard = shard.map.write_async().await;
      guard.insert_hashed(key, new_cache_entry, key_hash)
    };




  }

  /// Asynchronously inserts a key-value pair into the cache with a specific cost
  /// and Time-to-Live (TTL).
  ///
  /// This TTL overrides any global TTL that was configured on the cache builder.
  /// Reads honour the TTL and the janitor removes the entry at expiry. The first
  /// call starts the janitor if the cache has none.
  pub async fn insert_with_ttl(&self, key: K, value: V, cost: u64, ttl: Duration)
  where
    K: Clone + Sync,
    V: Sync,
    H: Send + Sync,
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
      let mut guard = shard.map.write_async().await;
      guard.insert_hashed(key, new_cache_entry, key_hash)
    };




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

    if let Some(entry) = guard.get_mut(key) {
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

  /// Removes an entry from the cache, returning the stored value if the key
  /// was found.
  ///
  /// The eviction listener is notified with `EvictionReason::Invalidated` and
  /// the `invalidations` metric is incremented, exactly as with `invalidate`.
  pub async fn remove<Q>(&self, key: &Q) -> Option<Arc<V>>
  where
    K: Borrow<Q> + Clone,
    Q: Eq + Hash + Equivalent<K> + ?Sized,
    V: Sync,
  {
    let shard = self.shared.store.get_shard(key);

    let removed_entry: Option<(K, CacheEntry<V>)>;
    {
      // New scope for the guard
      let mut guard = shard.map.write_async().await;
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
  pub async fn invalidate<Q>(&self, key: &Q) -> bool
  where
    K: Borrow<Q> + Clone,
    Q: Eq + Hash + Equivalent<K> + ?Sized,
    V: Sync,
  {
    self.remove(key).await.is_some()
  }

  /// Asynchronously removes all entries from the cache.
  pub async fn clear(&self) {
    // 1. Asynchronously acquire all write locks.
    let mut shard_guards =
      future::join_all(self.shared.store.iter_shards().map(|s| s.map.write_async())).await;

    // 2. Clear each shard; the shard map clears its policy.
    for guard in shard_guards.iter_mut() {
      guard.clear();
    }

  }

  /// Private helper for cache hits. Returns a read the shard's log or ring could
  /// not take, for the caller to apply after releasing the guard.
  #[inline]
  fn on_hit(
    &self,
    key: &K,
    _hash: u64,
    entry: &CacheEntry<V>,
    shard_index: usize,
    map: &crate::store::ShardMap<K, V, H>,
  ) -> Option<DeferredRead<K>>
  where
    K: Clone,
  {
    if self.shared.time_to_idle.is_some() {
      entry.update_last_accessed_coarse(&self.shared.clock);
    }

    match self.shared.builtin_reads {
      crate::shared::BuiltinReads::Ring => {
        let node = entry.node.load(Ordering::Relaxed);
        if self.shared.store.shards[shard_index].read_ring.push(node) {
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
        if self.shared.store.shards[shard_index].read_log.push(key, cost) {
          None
        } else {
          Some(DeferredRead::Key(key.clone(), cost))
        }
      }
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
    self.shared.try_flush_for_introspection();
    IterStream::new(self, DEFAULT_ITER_BATCH_SIZE)
  }

  /// Returns a concurrent-safe stream with a custom batch size.
  ///
  /// A larger batch size may have better throughput but will hold shard locks
  /// for slightly longer during batch-refill operations.
  pub fn iter_stream_with_batch_size(&self, batch_size: usize) -> IterStream<K, V, H> {
    self.shared.try_flush_for_introspection();
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
    self.shared.try_flush_for_introspection();
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
          let mut found = HashMap::new();
          let mut deferred = Vec::new();
          {
            let guard = shard.map.read_async().await;
            for key in shard_keys {
              if let Some(entry) = guard.get(key.borrow()) {
                if !entry.is_expired_coarse(shared.time_to_idle, &shared.clock) {
                  if shared.time_to_idle.is_some() {
                    entry.update_last_accessed_coarse(&self.shared.clock);
                  }
                  match shared.builtin_reads {
                    crate::shared::BuiltinReads::Ring => {
                      let node = entry.node.load(Ordering::Relaxed);
                      if !shard.read_ring.push(node) {
                        deferred.push(DeferredRead::Node(node));
                      }
                    }
                    crate::shared::BuiltinReads::Freq => guard.touch_read(entry.node.load(Ordering::Relaxed)),
                    crate::shared::BuiltinReads::None => {
                      if shared.track_reads && !shard.read_log.push(&key, entry.cost()) {
                        deferred.push(DeferredRead::Key(key.clone(), entry.cost()));
                      }
                    }
                  }
                  found.insert(key.clone(), entry.value());
                }
              }
            }
          }
          if !deferred.is_empty() {
            let mut guard = shard.map.write_async().await;
            for read in deferred {
              guard.apply_deferred(read);
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

    // One bulk update per multiget call, so a single stripe is fine.
    let hits = final_map.len() as u64;
    self.shared.metrics.record_hits(hits);
    self.shared.metrics.record_misses(total_reqs - hits);

    final_map
  }

  /// Inserts multiple key-value-cost triples into the cache.
  ///
  /// Groups items by shard and inserts each group under that shard's write lock.
  /// Capacity is enforced during the insert.
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
            let new_cache_entry =
              CacheEntry::new(value, cost, shared.time_to_live, shared.time_to_idle);
            guard.insert(key.clone(), new_cache_entry);
          }
        }
      });

    future::join_all(insert_futs).await;
  }

  /// Removes multiple entries from the cache, returning the key-value pairs
  /// that were present.
  ///
  /// This is more efficient than calling `remove` in a loop. The returned
  /// pairs are in no particular order.
  #[cfg(feature = "bulk")]
  pub async fn multi_remove<I, Q>(&self, keys: I) -> Vec<(K, Arc<V>)>
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

    let remove_futs = keys_by_shard
      .into_iter()
      .enumerate()
      .filter(|(_, keys)| !keys.is_empty())
      .map(|(i, shard_keys)| {
        let shared = Arc::clone(&self.shared);
        async move {
          let mut removed = Vec::new();
          let shard = &shared.store.shards[i];
          let mut guard = shard.map.write_async().await;
          for key in shard_keys {
            if let Some(entry) = guard.remove(&key) {
                shared
                .metrics
                .invalidations
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
              shared
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
        }
      });

    future::join_all(remove_futs)
      .await
      .into_iter()
      .flatten()
      .collect()
  }

  /// Removes multiple entries from the cache.
  #[cfg(feature = "bulk")]
  pub async fn multi_invalidate<I, Q>(&self, keys: I)
  where
    I: IntoIterator<Item = Q>,
    K: From<Q>,
  {
    let _ = self.multi_remove(keys).await;
  }
}

impl<K, V, H> AsyncCache<K, V, H>
where
  K: Eq + Hash + Clone + Send + Sync + 'static,
  V: Send + Sync,
  H: BuildHasher + Clone + Send + Sync + 'static,
{
  /// Forces a maintenance pass across all shards.
  ///
  /// Refreshes the clock reads use, applies every pending read to the policy on
  /// each shard and removes every entry past its TTL or TTI with a full scan. An
  /// entry in its stale-while-revalidate grace stays. Capacity needs no pass: an
  /// insert evicts before it returns.
  ///
  /// Awaits each shard's locks, so a task holding an `AsyncEntry` can finish.
  ///
  /// Primarily useful for deterministic testing and manual cache management.
  pub async fn run_maintenance(&self) {
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
      let _guard = shard.maintenance_lock.lock_async().await;
      let expired = shard.map.write_async().await.expire_all(now);
      janitor_context.account_expired(expired);
    }
  }

  /// Asynchronously retrieves a value for the given key, computing it with the
  /// configured loader function if the key is not present.
  ///
  /// This method provides thundering herd protection, ensuring that if multiple
  /// tasks request a missing key simultaneously, the loader function is only
  /// executed once. The load runs off this task, so dropping this future leaves
  /// it running for its waiters. A sync loader runs on a pool of up to 64 threads
  /// per cache.
  ///
  /// # Panics
  ///
  /// Panics on a miss when the cache has no loader. Panics when the loader panics
  /// for this key or its task is dropped, as when its runtime shuts down, in the
  /// caller and in every waiter on it; the next call loads again. A `TokioSpawner`
  /// keeps the runtime it was built on, so once that runtime is gone every miss
  /// panics.
  pub async fn fetch_with(&self, key: &K) -> Arc<V>
  where
    K: Clone + Send + Sync + 'static,
    V: Send + Sync + 'static,
    H: BuildHasher + Clone + Send + Sync,
  {
    let hash = crate::store::hash_key(&self.shared.store.hasher, key);
    let shard_index = self.shared.get_shard_index_from_hash(hash);
    let shard_ref = &self.shared.store.shards[shard_index];

    let mut deferred = None;
    let mut stale = false;
    let hit_value = {
      let guard = shard_ref.map.read_async().await;
      if let Some((found_key, entry_in_guard)) = guard.get_key_value_hashed(key, hash) {
        let expires_at_nanos = entry_in_guard.expires_at.load(Ordering::Relaxed);

        if expires_at_nanos == 0 {
          // No TTL, fresh if TTI has not expired.
          if !entry_in_guard.is_expired_coarse(self.shared.time_to_idle, &self.shared.clock) {
            deferred = self.on_hit(found_key, hash, entry_in_guard, shard_index, &guard);
            self.shared.metrics.record_hits(1);
            Some(entry_in_guard.value())
          } else {
            None
          }
        } else {
          let now_nanos = crate::time::now_duration().as_nanos() as u64;
          if now_nanos < expires_at_nanos {
            // CASE A: Fresh Hit
            if !entry_in_guard.is_expired_coarse(self.shared.time_to_idle, &self.shared.clock) {
              deferred = self.on_hit(found_key, hash, entry_in_guard, shard_index, &guard);
              self.shared.metrics.record_hits(1);
              Some(entry_in_guard.value())
            } else {
              None
            }
          } else if let Some(grace_period) = self.shared.stale_while_revalidate {
            // CASE B: Stale Hit
            if now_nanos < expires_at_nanos + grace_period.as_nanos() as u64 {
              stale = true;
              Some(entry_in_guard.value())
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
      shard_ref.map.write_async().await.apply_deferred(read);
    }
    if stale {
      self.trigger_background_load(key).await;
    }

    if let Some(val) = hit_value {
      return val;
    }

    // CASE C: Miss or fully expired.
    self.load_value_awaiting(key).await
  }

  /// Private helper to trigger a background refresh for stale items.
  async fn trigger_background_load(&self, key: &K)
  where
    K: Clone + 'static,
    V: 'static,
  {
    if self.shared.loader.is_none() {
      return;
    }
    let hash = crate::store::hash_key(&self.shared.store.hasher, &key);
    let index = hash as usize & (self.shared.pending_loads.len() - 1);
    let mut pending = self.shared.pending_loads[index].lock_async().await;
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
  /// This implements thundering herd protection for asynchronous callers.
  async fn load_value_awaiting(&self, key: &K) -> Arc<V>
  where
    K: Clone + Send + Sync + 'static,
    V: Send + Sync + 'static,
    H: BuildHasher + Clone + Send + Sync,
  {
    if self.shared.loader.is_none() {
      panic!("{}", crate::loader::NO_LOADER);
    }

    // Only one leader is chosen per key. The cached value is not read again
    // here, to keep the shard lock out of this lock's scope.
    let hash = crate::store::hash_key(&self.shared.store.hasher, &key);
    let index = hash as usize & (self.shared.pending_loads.len() - 1);
    let (future, am_leader) = {
      let mut pending = self.shared.pending_loads[index].lock_async().await;
      if let Some(existing) = pending.get(key).filter(|load| !load.is_dropped()) {
        if existing.claim() {
          // A refresh still in the queue: this miss runs it now.
          self.shared.metrics.record_misses(1);
          (existing.clone(), true)
        } else {
          // We will get a value, so this counts as a HIT for us.
          self.shared.metrics.record_hits(1);
          (existing.clone(), false)
        }
      } else {
        // This is the only time a MISS is recorded for the entire operation.
        self.shared.metrics.record_misses(1);
        let future = Arc::new(LoadFuture::new(None));
        pending.insert(key.clone(), future.clone());
        (future, true)
      }
    };

    // The load runs off this task, so dropping this future leaves it running for the waiters.
    if am_leader {
      CacheShared::spawn_loader_task(Arc::clone(&self.shared), key.clone(), future.clone(), false);
    }

    (&*future).await
  }
}
