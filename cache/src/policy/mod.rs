pub mod arc;
pub mod clock;
pub mod fifo;
pub mod lru;
mod lru_list;
pub mod null;
pub mod random;
pub mod s3fifo;
pub mod sieve;
pub mod slru;
pub mod tinylfu;

#[derive(Debug)]
pub enum AdmissionDecision<K> {
  Admit,
  Reject,
  AdmitAndEvict(Vec<K>), // K is the key of the victim
}

/// A trait for implementing cache admission/eviction policies.
///
/// The policy is responsible for tracking item usage and deciding which items
/// to admit or evict when the cache is over capacity.
pub trait CachePolicy<K, V>: Send + Sync {
  /// Called when an item is accessed (read or written).
  /// The policy should update its internal tracking structures.
  fn on_access(&self, key: &K, cost: u64);

  /// Returns `true` if this policy makes use of `on_access` events.
  ///
  /// Policies that ignore access information (e.g. a no-op policy for
  /// unbounded caches) can return `false`, allowing the cache to skip
  /// recording read accesses entirely on the hot path.
  fn uses_access_events(&self) -> bool {
    true
  }

  /// Called when an item is being admitted.
  ///
  /// `AdmitAndEvict` names keys to evict. `Reject` does not stop the insert.
  fn on_admit(&self, key: &K, cost: u64) -> AdmissionDecision<K>;

  /// Called when an item is manually invalidated or evicted due to TTL/TTI.
  /// The policy should clean up any state associated with the key.
  fn on_remove(&self, key: &K);

  /// Called on an insert that leaves the shard over its share of the capacity.
  ///
  /// The policy must identify and return a set of victim keys to be evicted,
  /// along with the total cost of those victims. The number of victims should
  /// be enough to free up at least `cost_to_free` from the cache.
  fn evict(&self, cost_to_free: u64) -> (Vec<K>, u64);

  /// Clears all state from the policy.
  fn clear(&self);
}
