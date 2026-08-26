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

/// A coarse clock for hot read paths: the janitor refreshes it once per tick, so
/// reads cost an atomic load instead of a syscall. Reads lag real time by at most
/// one janitor tick; 0 means never refreshed and falls back to the precise clock.
#[derive(Default)]
pub(crate) struct CoarseClock {
  nanos: std::sync::atomic::AtomicU64,
}

impl CoarseClock {
  pub(crate) fn refresh(&self) {
    self
      .nanos
      .store(now_duration().as_nanos() as u64, std::sync::atomic::Ordering::Relaxed);
  }

  #[inline]
  pub(crate) fn now(&self) -> u64 {
    match self.nanos.load(std::sync::atomic::Ordering::Relaxed) {
      0 => now_duration().as_nanos() as u64,
      n => n,
    }
  }
}
