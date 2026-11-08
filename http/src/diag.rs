//! Counters for the connection driver, behind the `diagnostics` feature. Without it every call compiles to nothing.

#[derive(Clone, Copy)]
pub(crate) enum C {
  Turn,
  EmptyTurn,
  Lead,
  Handoff,
  Park,
  Taken,
}

#[cfg(feature = "diagnostics")]
static COUNTERS: [std::sync::atomic::AtomicU64; 9] = [const { std::sync::atomic::AtomicU64::new(0) }; 9];

#[inline]
pub(crate) fn bump(c: C) {
  #[cfg(feature = "diagnostics")]
  COUNTERS[c as usize].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
  #[cfg(not(feature = "diagnostics"))]
  let _ = c;
}

/// Nanoseconds the socket went unread after a leader left with others still waiting.
#[cfg(feature = "diagnostics")]
#[inline]
pub(crate) fn idle(ns: u64) {
  COUNTERS[6].fetch_add(ns, std::sync::atomic::Ordering::Relaxed);
}

/// Nanoseconds a blocking follower spent parked.
#[cfg(feature = "diagnostics")]
#[inline]
pub(crate) fn parked(ns: u64) {
  COUNTERS[7].fetch_add(ns, std::sync::atomic::Ordering::Relaxed);
}

/// Nanoseconds a blocking follower waited for the connection lock after waking.
#[cfg(feature = "diagnostics")]
#[inline]
pub(crate) fn relock(ns: u64) {
  COUNTERS[8].fetch_add(ns, std::sync::atomic::Ordering::Relaxed);
}

/// The driver's counters since the last `reset`.
#[cfg(feature = "diagnostics")]
#[derive(Debug, Clone, Copy, Default)]
pub struct Snapshot {
  /// Turns: one read-and-dispatch pass over a connection.
  pub turns: u64,
  /// Turns that read nothing.
  pub empty_turns: u64,
  /// Times a caller took the lead.
  pub leads: u64,
  /// Times a leaving leader woke another waiter to take over.
  pub handoffs: u64,
  /// Times a blocking follower parked.
  pub parks: u64,
  /// Events handed to callers.
  pub taken: u64,
  /// Total nanoseconds the socket went unread after a leader left with others waiting.
  pub idle_ns: u64,
  /// Total nanoseconds blocking followers spent parked.
  pub parked_ns: u64,
  /// Total nanoseconds blocking followers waited for the connection lock after waking.
  pub relock_ns: u64,
}

#[cfg(feature = "diagnostics")]
pub fn snapshot() -> Snapshot {
  let g = |i: usize| COUNTERS[i].load(std::sync::atomic::Ordering::Relaxed);
  Snapshot { turns: g(0), empty_turns: g(1), leads: g(2), handoffs: g(3), parks: g(4), taken: g(5), idle_ns: g(6), parked_ns: g(7), relock_ns: g(8) }
}

#[cfg(feature = "diagnostics")]
pub fn reset() {
  for c in &COUNTERS {
    c.store(0, std::sync::atomic::Ordering::Relaxed);
  }
}
