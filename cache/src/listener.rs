use std::fmt;

/// Describes the reason an entry was removed from the cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvictionReason {
  /// The entry was removed due to exceeding the cache's capacity.
  Capacity,
  /// The entry was removed because its time-to-live (TTL) expired.
  Expired,
  /// The entry was removed because it was manually invalidated.
  Invalidated,
}

impl fmt::Display for EvictionReason {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      EvictionReason::Capacity => write!(f, "evicted due to capacity"),
      EvictionReason::Expired => write!(f, "evicted due to expiration (TTL/TTI)"),
      EvictionReason::Invalidated => write!(f, "manually invalidated"),
    }
  }
}

/// A listener that can be registered with the cache to receive notifications
/// when entries are evicted.
///
/// The `on_evict` method is called with the key, value, and reason for
/// the eviction. This happens on a dedicated background task to avoid
/// blocking cache operations.
pub trait EvictionListener<K, V>: Send + Sync {
  fn on_evict(&self, key: K, value: V, reason: EvictionReason);
}