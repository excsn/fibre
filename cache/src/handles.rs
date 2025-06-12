use futures_util::future;

use crate::entry::CacheEntry;
use crate::entry_api::{OccupiedEntry, VacantEntry};
use crate::policy::AccessInfo;
use crate::shared::CacheShared;
use crate::{Entry, EvictionReason};

use std::hash::{BuildHasher, Hash};
use std::sync::atomic::Ordering;
use std::sync::Arc;

/// A thread-safe, synchronous cache.
#[derive(Debug)]
pub struct Cache<K: Send, V: Send, H = ahash::RandomState> {
  pub(crate) shared: Arc<CacheShared<K, V, H>>,
}

/// A thread-safe, asynchronous cache.
#[derive(Debug)]
pub struct AsyncCache<K: Send, V: Send, H = ahash::RandomState> {
  pub(crate) shared: Arc<CacheShared<K, V, H>>,
}

impl<K: Send, V: Send, H> Cache<K, V, H>
where
  K: Eq + Hash + Clone + 'static,
  H: BuildHasher + Clone,
{
  /// Retrieves a value from the cache.
  ///
  /// Returns a clone of the `Arc` containing the value if the key is found
  /// and the entry is not expired. Returns `None` otherwise.
  pub fn get(&self, key: &K) -> Option<Arc<V>> {
    let shard = self.shared.store.get_shard(key);
    let guard = shard.read();

    if let Some(entry) = guard.get(key) {
      if entry.is_expired(self.shared.time_to_idle) {
        self.shared.metrics.misses.fetch_add(1, Ordering::Relaxed);
        None
      } else {
        entry.update_last_accessed();
        let info = AccessInfo { key, entry };
        self.shared.eviction_policy.on_access(&info);
        self.shared.metrics.hits.fetch_add(1, Ordering::Relaxed);
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
    let guard = shard.write_sync();

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
  pub fn insert(&self, key: K, value: V, cost: u64) {
    self.shared.cost_gate.acquire_sync(cost);

    let shard = self.shared.store.get_shard(&key);
    let mut guard = shard.write_sync();

    let entry = Arc::new(CacheEntry::new(
      value,
      cost,
      self.shared.time_to_live,
      self.shared.time_to_idle,
    ));

    let info = AccessInfo {
      key: &key,
      entry: &entry,
    };
    if !self.shared.eviction_policy.on_insert(&info) {
      self.shared.cost_gate.release(cost);
      self
        .shared
        .metrics
        .keys_rejected
        .fetch_add(1, Ordering::Relaxed);
      return;
    }

    if let Some(old_entry) = guard.insert(key, entry) {
      // Overwrote an existing entry. Release its cost.
      self.shared.cost_gate.release(old_entry.cost());
      self
        .shared
        .metrics
        .current_cost
        .fetch_sub(old_entry.cost(), Ordering::Relaxed);
    }

    self.shared.metrics.inserts.fetch_add(1, Ordering::Relaxed);
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
    self
      .shared
      .metrics
      .keys_admitted
      .fetch_add(1, Ordering::Relaxed);
  }

  /// Atomically computes a new value for a key.
  /// The provided closure is called with a mutable reference to the value
  /// if the key exists and no other threads are currently reading it.
  pub fn compute<F>(&self, key: &K, f: F) -> bool
  where
    F: FnOnce(&mut V),
  {
    let shard = self.shared.store.get_shard(key);
    let mut guard = shard.write_sync();

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
  pub fn invalidate(&self, key: &K) -> bool {
    let shard = self.shared.store.get_shard(key);
    let mut guard = shard.write_sync();

    if let Some(entry) = guard.remove(key) {
      self.shared.eviction_policy.on_remove(key);
      self.shared.cost_gate.release(entry.cost());
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
        if let Ok(value) = Arc::try_unwrap(entry.value()) {
          let _ = sender.try_send((key.clone(), value, EvictionReason::Invalidated));
        }
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
      .map(|s| s.write_sync())
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
    let total_cost = self
      .shared
      .metrics
      .current_cost
      .swap(0, std::sync::atomic::Ordering::Relaxed);
    self.shared.cost_gate.release(total_cost);
  }

  /// Converts this synchronous `Cache` into an asynchronous `AsyncCache`.
  /// This is a zero-cost conversion.
  pub fn to_async(self) -> AsyncCache<K, V, H> {
    AsyncCache {
      shared: self.shared.clone(),
    }
  }
}

// --- AsyncCache Implementation ---
// For this milestone, the async methods can just mirror the sync ones,
// as none of them are inherently `async` yet (no awaiting on IO or locks).
// In later milestones, `insert` will become `async` to await the CostGate.

impl<K: Send, V: Send, H> AsyncCache<K, V, H>
where
  K: Eq + Hash,
  H: BuildHasher + Clone,
{
  /// Retrieves a value from the cache.
  pub fn get(&self, key: &K) -> Option<Arc<V>> {
    let shard = self.shared.store.get_shard(key);
    let guard = shard.read();

    if let Some(entry) = guard.get(key) {
      if entry.is_expired(self.shared.time_to_idle) {
        // The janitor will clean this up eventually. For now, treat as a miss.
        // An active approach would be to acquire a write lock here and remove,
        // but that can lead to lock escalation and performance degradation.
        // A passive "read-only" check is faster.
        self.shared.metrics.misses.fetch_add(1, Ordering::Relaxed);
        None
      } else {
        entry.update_last_accessed();
        entry.increment_frequency();
        self.shared.metrics.hits.fetch_add(1, Ordering::Relaxed);
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
  pub async fn entry(&self, key: K) -> Entry<'_, K, V, H> {
    let shard = self.shared.store.get_shard(&key);
    let guard = shard.write_async().await;

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
  /// In a future milestone, this will become an `async fn` to handle
  /// waiting for capacity.
  pub async fn insert(&self, key: K, value: V, cost: u64) {
    self.shared.cost_gate.acquire_async(cost).await;

    let shard = self.shared.store.get_shard(&key);
    let mut guard = shard.write_async().await;

    let entry = Arc::new(CacheEntry::new(
      value,
      cost,
      self.shared.time_to_live,
      self.shared.time_to_idle,
    ));

    let info = AccessInfo {
      key: &key,
      entry: &entry,
    };
    if !self.shared.eviction_policy.on_insert(&info) {
      self.shared.cost_gate.release(cost);
      self
        .shared
        .metrics
        .keys_rejected
        .fetch_add(1, Ordering::Relaxed);
      return;
    }

    if let Some(old_entry) = guard.insert(key, entry) {
      self.shared.cost_gate.release(old_entry.cost());
      self
        .shared
        .metrics
        .current_cost
        .fetch_sub(old_entry.cost(), Ordering::Relaxed);
    }

    self.shared.metrics.inserts.fetch_add(1, Ordering::Relaxed);
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
    self
      .shared
      .metrics
      .keys_admitted
      .fetch_add(1, Ordering::Relaxed);
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
    let mut guard = shard.write_async().await;

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
  pub async fn invalidate(&self, key: &K) -> bool {
    let shard = self.shared.store.get_shard(key);
    let mut guard = shard.write_async().await;

    if let Some(entry) = guard.remove(key) {
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
      true
    } else {
      false
    }
  }

  /// Asynchronously removes all entries from the cache.
  pub async fn clear(&self) {
    // 1. Asynchronously acquire all write locks.
    let mut shard_guards =
      future::join_all(self.shared.store.iter_shards().map(|s| s.write_async())).await;

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
    self.shared.cost_gate.release(total_cost);
  }
  
  /// Converts this asynchronous `AsyncCache` into a synchronous `Cache`.
  /// This is a zero-cost conversion.
  pub fn to_sync(self) -> Cache<K, V, H> {
    Cache {
      shared: self.shared.clone(),
    }
  }
}
