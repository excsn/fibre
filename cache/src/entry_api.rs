use crate::policy::AdmissionDecision;
use crate::shared::CacheShared;
use crate::sync::WriteGuard;
use std::hash::{BuildHasher, Hash};
use std::sync::Arc;

/// A view into a single entry in the cache, which may either be occupied or vacant.
///
/// This enum is constructed by the [`Cache::entry`] method.
// The lifetime 'a is the lifetime of the write guard on the shard.
pub enum Entry<'a, K: Send, V: Send + Sync, H> {
  /// An occupied entry.
  Occupied(OccupiedEntry<'a, K, V, H>),
  /// A vacant entry.
  Vacant(VacantEntry<'a, K, V, H>),
}

/// A view into an occupied entry in a `Cache`.
pub struct OccupiedEntry<'a, K: Send, V: Send, H> {
  pub(crate) key: K,
  pub(crate) shard_guard:
    WriteGuard<'a, std::collections::HashMap<K, Arc<crate::entry::CacheEntry<V>>, H>>,
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
pub struct VacantEntry<'a, K: Send, V: Send + Sync, H> {
  pub(crate) key: K,
  pub(crate) shared: &'a Arc<CacheShared<K, V, H>>,
  pub(crate) shard_guard:
    WriteGuard<'a, std::collections::HashMap<K, Arc<crate::entry::CacheEntry<V>>, H>>,
}

impl<'a, K, V, H> VacantEntry<'a, K, V, H>
where
  K: Eq + Hash + Clone + Send + Sync,
  V: Send + Sync,
  H: BuildHasher + Clone + Sync,
{
  /// Gets a reference to the key that would be used to insert the value.
  pub fn key(&self) -> &K {
    &self.key
  }

  //TODO Needs insert_async or async entry api

  /// Inserts a new value into the cache at this entry's key.
  ///
  ///
  /// Returns an `Arc` pointing to the newly inserted value.
  pub fn insert(mut self, value: V, cost: u64) -> Arc<V> {
    let new_cache_entry = Arc::new(crate::entry::CacheEntry::new(
      value,
      cost,
      self.shared.time_to_live,
      self.shared.time_to_idle,
    ));
    let value_arc_to_return = new_cache_entry.value();

    let access_info = crate::policy::AccessInfo {
      key: &self.key,
      entry: &new_cache_entry,
    };

    let admission_decision = self.shared.eviction_policy.on_admit(&access_info);
    let mut victims_to_process: Option<Vec<K>> = None;
    let mut actually_inserted = false;

    match admission_decision {
      AdmissionDecision::Admit => {
        self.shard_guard.insert(self.key.clone(), new_cache_entry); // Clone key if self.key is moved
        self
          .shared
          .metrics
          .inserts
          .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self
          .shared
          .metrics
          .keys_admitted
          .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        actually_inserted = true;
      }
      AdmissionDecision::Reject => {
        self
          .shared
          .metrics
          .keys_rejected
          .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        // Value not inserted.
      }
      AdmissionDecision::AdmitAndEvict(ref victim_keys_ref) => {
        // USE ref
        self.shard_guard.insert(self.key.clone(), new_cache_entry); // Clone key
        self
          .shared
          .metrics
          .inserts
          .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self
          .shared
          .metrics
          .keys_admitted
          .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        victims_to_process = Some(victim_keys_ref.clone()); // CLONE
        actually_inserted = true;
      }
    }

    // Cost updates only if actually inserted
    if actually_inserted {
      self
        .shared
        .metrics
        .current_cost
        .fetch_add(cost, std::sync::atomic::Ordering::Relaxed);
      self
        .shared
        .metrics
        .total_cost_added
        .fetch_add(cost, std::sync::atomic::Ordering::Relaxed);
    }

    // Drop the guard for the current shard BEFORE processing victims from other shards.
    drop(self.shard_guard);

    if let Some(victims) = victims_to_process {
      self.shared.process_evicted_victims(victims); // Uses sync version
    }
    value_arc_to_return
  }
}
