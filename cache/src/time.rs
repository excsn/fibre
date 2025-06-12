use once_cell::sync::Lazy;
use std::time::{Duration, Instant};

// The single, static reference point for all time calculations in the cache.
// It is initialized lazily on its first use.
static CACHE_EPOCH: Lazy<Instant> = Lazy::new(Instant::now);

/// Converts an `Instant` into a `Duration` since the cache's epoch.
/// This duration is serializable.
#[inline]
pub(crate) fn instant_to_duration(instant: Instant) -> Duration {
  instant.saturating_duration_since(*CACHE_EPOCH)
}

/// Converts a `Duration` from the cache's epoch back into an `Instant`.
#[inline]
pub(crate) fn duration_to_instant(duration: Duration) -> Instant {
  *CACHE_EPOCH + duration
}

/// A helper to get the current time as a `Duration` since the epoch.
#[inline]
pub(crate) fn now_duration() -> Duration {
  instant_to_duration(Instant::now())
}
