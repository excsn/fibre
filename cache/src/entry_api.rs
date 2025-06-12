use crate::shared::CacheShared;
use crate::sync::HybridRwLockWriteGuard;
use std::hash::{BuildHasher, Hash};
use std::sync::Arc;

/// A view into a single entry in the cache, which may either be occupied or vacant.
///
/// This enum is constructed by the [`Cache::entry`] method.
// The lifetime 'a is the lifetime of the write guard on the shard.
pub enum Entry<'a, K: Send, V: Send, H> {
  /// An occupied entry.
  Occupied(OccupiedEntry<'a, K, V, H>),
  /// A vacant entry.
  Vacant(VacantEntry<'a, K, V, H>),
}

/// A view into an occupied entry in a `Cache`.
pub struct OccupiedEntry<'a, K: Send, V: Send, H> {
  pub(crate) key: K,
  pub(crate) shard_guard: HybridRwLockWriteGuard<'a, std::collections::HashMap<K, Arc<crate::entry::CacheEntry<V>>, H>>,
}

impl<'a, K, V: Send, H> OccupiedEntry<'a, K, V, H>
where
  K: std::cmp::Eq + Hash + Send,
  H: BuildHasher,
{
  /// Gets a reference to the key in the entry.
  pub fn key(&self) -> &K {
    &self.key
  }

  /// Gets a clone of the `Arc` pointing to the value in the entry.
  pub fn get(&self) -> Arc<V> {
    self.shard_guard.get(&self.key).unwrap().value()
  }
}

/// A view into a vacant entry in a `Cache`.
pub struct VacantEntry<'a, K: Send, V: Send, H> {
  pub(crate) key: K,
  pub(crate) shared: &'a Arc<CacheShared<K, V, H>>,
  pub(crate) shard_guard: HybridRwLockWriteGuard<'a, std::collections::HashMap<K, Arc<crate::entry::CacheEntry<V>>, H>>,
}

impl<'a, K: Send, V: Send, H> VacantEntry<'a, K, V, H>
where
  K: Eq + Hash + Clone,
  V: Send + Sync,
  H: BuildHasher + Clone,
{
  /// Gets a reference to the key that would be used to insert the value.
  pub fn key(&self) -> &K {
    &self.key
  }

  /// Inserts a new value into the cache at this entry's key.
  ///
  /// This will acquire a permit from the `CostGate` and may cause an
  /// eviction if the cache is at capacity.
  ///
  /// Returns an `Arc` pointing to the newly inserted value.
  pub fn insert(self, value: V, cost: u64) -> Arc<V> {
    // This `VacantEntry` holds the write lock, so all operations here are atomic.

    // 1. Acquire capacity from the gate. This is a blocking call.
    self.shared.cost_gate.acquire_sync(cost);

    // 2. Create the entry.
    let entry = Arc::new(crate::entry::CacheEntry::new(
      value,
      cost,
      self.shared.time_to_live,
      self.shared.time_to_idle,
    ));
    let value_arc = entry.value();

    // 3. Consult the eviction policy for admission.
    let info = crate::policy::AccessInfo {
      key: &self.key,
      entry: &entry,
    };
    if !self.shared.eviction_policy.on_insert(&info) {
      // Rejected by policy. Release the cost and return the value arc.
      // The value will be dropped when the arc goes out of scope if not used.
      self.shared.cost_gate.release(cost);
      self
        .shared
        .metrics
        .keys_rejected
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
      return value_arc;
    }

    // 4. Insert into the HashMap.
    // We need to use a mutable reference to the guard.
    let mut guard = self.shard_guard;
    guard.insert(self.key, entry);

    // 5. Update metrics.
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
    self
      .shared
      .metrics
      .total_cost_added
      .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    self
      .shared
      .metrics
      .keys_admitted
      .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

    value_arc
  }
}
