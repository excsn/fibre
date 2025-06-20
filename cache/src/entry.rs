use crate::task::timer::TimerHandle;
use crate::time;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// A container for a value in the cache, holding all necessary metadata.
#[derive(Debug)]
pub(crate) struct CacheEntry<V> {
  /// The user's value, wrapped in an Arc for shared ownership.
  pub(crate) value: Arc<V>,
  /// The cost associated with this entry.
  cost: u64,
  /// The expiration timestamp in nanoseconds. 0 means no TTL.
  pub(crate) expires_at: AtomicU64,
  /// The last access timestamp in nanoseconds. 0 means no TTI.
  pub(crate) last_accessed: AtomicU64,
  /// A handle to the TTL timer in the timer wheel, for cancellation.
  pub(crate) ttl_timer_handle: Option<TimerHandle>, // We will use the key's hash for the timer
  /// A handle to the TTI timer in the timer wheel.
  pub(crate) tti_timer_handle: Option<TimerHandle>,
}

impl<V> CacheEntry<V> {
  /// Creates a new `CacheEntry`.
  pub(crate) fn new(value: V, cost: u64, ttl: Option<Duration>, tti: Option<Duration>) -> Self {
    let now = time::now_duration().as_nanos() as u64;
    let expires_at = ttl.map_or(0, |d| now + d.as_nanos() as u64);
    let last_accessed = tti.map_or(0, |_| now);

    Self {
      value: Arc::new(value),
      cost,
      expires_at: AtomicU64::new(expires_at),
      last_accessed: AtomicU64::new(last_accessed),
      ttl_timer_handle: None,
      tti_timer_handle: None,
    }
  }

  /// Creates a new `CacheEntry` from a pre-calculated expiry time.
  /// Used when loading from a snapshot.
  pub(crate) fn new_with_expiry(
    value: V,
    cost: u64,
    expires_at: Option<Duration>, // This is a Duration from CACHE_EPOCH
    tti: Option<Duration>,
  ) -> Self {
    let expires_at_nanos = expires_at.map_or(0, |d| d.as_nanos() as u64);

    // When loading from a snapshot, we consider the item "just accessed"
    // for the purpose of TTI.
    let last_accessed_nanos = tti.map_or(0, |_| time::now_duration().as_nanos() as u64);

    Self {
      value: Arc::new(value),
      cost,
      expires_at: AtomicU64::new(expires_at_nanos),
      last_accessed: AtomicU64::new(last_accessed_nanos),
      ttl_timer_handle: None,
      tti_timer_handle: None,
    }
  }

  /// Creates a new `CacheEntry` with a specific expiration timestamp.
  pub(crate) fn new_with_custom_expiry(
    value: V,
    cost: u64,
    expires_at: u64, // Expiration time in nanoseconds since CACHE_EPOCH
    tti: Option<Duration>,
  ) -> Self {
    let last_accessed = tti.map_or(0, |_| time::now_duration().as_nanos() as u64);
    Self {
      value: Arc::new(value),
      cost,
      expires_at: AtomicU64::new(expires_at),
      last_accessed: AtomicU64::new(last_accessed),
      ttl_timer_handle: None,
      tti_timer_handle: None,
    }
  }

  /// Creates a new `CacheEntry` with a pre-existing value Arc and cost.
  /// Used by the janitor to create dummy entries for policy admission.
  pub(crate) fn new_with_cost(value: Arc<V>, cost: u64) -> Self {
    Self {
      value,
      cost,
      expires_at: AtomicU64::new(0),
      last_accessed: AtomicU64::new(0),
      ttl_timer_handle: None,
      tti_timer_handle: None,
    }
  }

  /// Returns a clone of the `Arc` containing the value.
  #[inline]
  pub(crate) fn value(&self) -> Arc<V> {
    self.value.clone()
  }

  /// Returns the cost of the entry.
  #[inline]
  pub(crate) fn cost(&self) -> u64 {
    self.cost
  }

  /// Updates the last accessed timestamp to the current time.
  /// This is a cheap atomic store operation.
  #[inline]
  pub(crate) fn update_last_accessed(&self) {
    self
      .last_accessed
      .store(time::now_duration().as_nanos() as u64, Ordering::Relaxed);
  }

  /// Checks if the entry is expired based on its TTL or TTI.
  #[inline]
  pub(crate) fn is_expired(&self, tti: Option<Duration>) -> bool {
    let now_nanos = time::now_duration().as_nanos() as u64;

    // Check for TTL expiration.
    let expires_at = self.expires_at.load(Ordering::Relaxed);
    if expires_at > 0 && now_nanos >= expires_at {
      return true;
    }

    // Check for TTI expiration.
    if let Some(time_to_idle) = tti {
      let last_accessed = self.last_accessed.load(Ordering::Relaxed);
      // The check `last_accessed > 0` is removed. The `Option` on `tti` is the
      // correct guard to determine if TTI logic should run.
      if now_nanos >= last_accessed + time_to_idle.as_nanos() as u64 {
        return true;
      }
    }

    false
  }

  /// Attaches timer handles to the entry after it has been created.
  pub(crate) fn set_timer_handles(
    &mut self,
    ttl_handle: Option<TimerHandle>,
    tti_handle: Option<TimerHandle>,
  ) {
    self.ttl_timer_handle = ttl_handle;
    self.tti_timer_handle = tti_handle;
  }
}
