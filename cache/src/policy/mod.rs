pub mod arc;
pub mod clock;
pub mod fifo;
pub mod lru;
pub mod null;
pub mod random;
pub mod sieve;
pub mod slru;
pub mod tinylfu;

use crate::entry::CacheEntry;
use std::sync::Arc;

/// An event that is recorded on every access to the cache.
/// These events are buffered and processed by a background maintenance task.
#[derive(Debug)]
pub(crate) enum AccessEvent<K> {
  Read(K),
  Write(K, u64), // Write events include the cost of the item.
}

#[derive(Debug)]
pub enum AdmissionDecision<K> {
  Admit,
  Reject,
  AdmitAndEvict(Vec<K>), // K is the key of the victim
}

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

  /// Called when an item is being admitted.
  ///
  /// The policy can use this to decide if the new item should be admitted,
  /// potentially rejecting it to protect more valuable items. Returning `false`
  /// will cause the insertion to be aborted.
  fn on_admit(&self, key: &K, cost: u64) -> AdmissionDecision<K>;

  /// Called when an item is manually invalidated or evicted due to TTL/TTI.
  /// The policy should clean up any state associated with the key.
  fn on_remove(&self, key: &K);

  /// Called by the janitor when the cache is over capacity.
  ///
  /// The policy must identify and return a set of victim keys to be evicted,
  /// along with the total cost of those victims. The number of victims should
  /// be enough to free up at least `cost_to_free` from the cache.
  fn evict(&self, cost_to_free: u64) -> (Vec<K>, u64);

  /// Clears all state from the policy.
  fn clear(&self);
}
