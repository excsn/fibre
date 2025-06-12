pub mod tinylfu;

use crate::entry::CacheEntry;
use std::sync::Arc;

/// Provides necessary information about an access to the eviction policy.
pub struct AccessInfo<'a, K, V> {
  pub key: &'a K,
  pub entry: &'a Arc<CacheEntry<V>>,
  // Add other fields as needed, e.g., current cache cost.
}

/// A trait for implementing cache admission/eviction policies.
///
/// The policy is responsible for tracking item usage and deciding which items
/// to admit or evict when the cache is over capacity.
pub trait CachePolicy<K, V>: Send + Sync {
  /// Called when an item is accessed (read or written).
  /// The policy should update its internal tracking structures.
  fn on_access(&self, info: &AccessInfo<K, V>);

  /// Called when an item is inserted.
  ///
  /// The policy can use this to decide if the new item should be admitted,
  /// potentially rejecting it to protect more valuable items. Returning `false`
  /// will cause the insertion to be aborted.
  fn on_insert(&self, info: &AccessInfo<K, V>) -> bool;

  /// Called when an item is manually invalidated or evicted due to TTL/TTI.
  /// The policy should clean up any state associated with the key.
  fn on_remove(&self, key: &K);

  /// Called by the janitor when the cache is over capacity.
  ///
  /// The policy must identify and return a set of victim keys to be evicted.
  /// The number of victims should be enough to free up at least `cost_to_free`
  /// from the cache.
  fn evict(&self, cost_to_free: u64) -> Vec<K>;

  /// Clears all state from the policy.
  fn clear(&self);
}

/// A default "no-op" eviction policy for unbounded caches or simple cases.
/// It does nothing and never evicts anything.
#[derive(Debug, Default)]
pub struct NullPolicy;

impl<K, V> CachePolicy<K, V> for NullPolicy
where
  K: Send + Sync,
  V: Send + Sync,
{
  fn on_access(&self, _info: &AccessInfo<K, V>) {}

  fn on_insert(&self, _info: &AccessInfo<K, V>) -> bool {
    true // Always admit new items.
  }

  fn on_remove(&self, _key: &K) {}

  fn evict(&self, _cost_to_free: u64) -> Vec<K> {
    Vec::new() // Never evicts.
  }

  fn clear(&self) {}
}
