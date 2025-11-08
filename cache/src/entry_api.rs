use crate::policy::AccessEvent;
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

impl<'a, K, V, H> Entry<'a, K, V, H>
where
  K: Eq + Hash + Clone + Send,
  V: Send + Sync,
  H: BuildHasher + Clone,
{
  /// Ensures a value is in the entry by inserting the default if empty, and returns
  /// an `Arc` to the value.
  pub fn or_insert(self, default: V, cost: u64) -> Arc<V> {
    match self {
      Entry::Occupied(o) => o.get(),
      Entry::Vacant(v) => v.insert(default, cost),
    }
  }

  /// Ensures a value is in the entry by inserting the result of the default function if empty,
  /// and returns an `Arc` to the value.
  /// The closure is only computed if the entry is vacant.
  pub fn or_insert_with<F>(self, default: F, cost: u64) -> Arc<V>
  where
    F: FnOnce() -> V,
  {
    match self {
      Entry::Occupied(o) => o.get(),
      Entry::Vacant(v) => v.insert(default(), cost),
    }
  }

  /// Ensures a value is in the entry by inserting the default value of the type if empty,
  /// and returns an `Arc` to the value.
  /// This requires `V` to implement `Default`.
  pub fn or_default(self, cost: u64) -> Arc<V>
  where
    V: Default,
  {
    match self {
      Entry::Occupied(o) => o.get(),
      Entry::Vacant(v) => v.insert(V::default(), cost),
    }
  }
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
  K: Eq + Hash + Clone + Send,
  V: Send + Sync,
  H: BuildHasher + Clone,
{
  /// Gets a reference to the key that would be used to insert the value.
  pub fn key(&self) -> &K {
    &self.key
  }

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

    // The key is cloned once for the event and once for the map insertion.
    let key_for_event = self.key.clone();

    // Insert the new entry into the hash map.
    self.shard_guard.insert(self.key, new_cache_entry);

    // We must drop the guard for the current shard before any other operations
    // that might try to lock other shards, although in this new model, we don't.
    // It's still good practice.
    drop(self.shard_guard);

    // Get the shard again to access its event buffer.
    let shard = self.shared.store.get_shard(&key_for_event);

    // Record the write event for the janitor to process later.
    let _ = shard
      .event_buffer_tx
      .try_send(AccessEvent::Write(key_for_event, cost));

    // Update metrics
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

    value_arc_to_return
  }

  /// Inserts the given default value into the cache and returns an `Arc` to it.
  pub fn or_insert(self, default: V, cost: u64) -> Arc<V> {
    self.insert(default, cost)
  }

  /// Inserts the value returned by the closure into the cache and returns an `Arc` to it.
  /// The closure is only called if the entry is vacant.
  pub fn or_insert_with<F>(self, default: F, cost: u64) -> Arc<V>
  where
    F: FnOnce() -> V,
  {
    self.insert(default(), cost)
  }

  /// Inserts the default value of the type `V` into the cache and returns an `Arc` to it.
  /// This requires that `V` implements the `Default` trait.
  pub fn or_default(self, cost: u64) -> Arc<V>
  where
    V: Default,
  {
    self.insert(V::default(), cost)
  }
}
