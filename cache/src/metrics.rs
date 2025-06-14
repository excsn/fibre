use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use crossbeam_utils::CachePadded;

/// A thread-safe, internal metrics collector for the cache.
/// All fields are atomic to allow for lock-free updates.
#[derive(Debug)]
pub struct Metrics {
  // --- Hit/Miss Ratios ---
  pub(crate) hits: CachePadded<AtomicU64>,
  pub(crate) misses: CachePadded<AtomicU64>,

  // --- Throughput ---
  pub(crate) inserts: CachePadded<AtomicU64>,
  pub(crate) updates: CachePadded<AtomicU64>,
  pub(crate) invalidations: CachePadded<AtomicU64>,

  // --- Eviction Stats ---
  pub(crate) evicted_by_capacity: CachePadded<AtomicU64>,
  pub(crate) evicted_by_ttl: CachePadded<AtomicU64>,
  pub(crate) evicted_by_tti: CachePadded<AtomicU64>,

  // --- Admission Policy (for TinyLFU) ---
  pub(crate) keys_admitted: CachePadded<AtomicU64>,
  pub(crate) keys_rejected: CachePadded<AtomicU64>,

  // --- Cost / Size ---
  pub(crate) current_cost: CachePadded<AtomicU64>,
  pub(crate) total_cost_added: CachePadded<AtomicU64>,

  // --- Timestamps for Uptime ---
  created_at: Instant,
}

// Manual implementation of Default to handle the non-default `Instant`.
impl Default for Metrics {
  fn default() -> Self {
    Self {
      hits: CachePadded::new(AtomicU64::new(0)),
      misses: CachePadded::new(AtomicU64::new(0)),
      inserts: CachePadded::new(AtomicU64::new(0)),
      updates: CachePadded::new(AtomicU64::new(0)),
      invalidations: CachePadded::new(AtomicU64::new(0)),
      evicted_by_capacity: CachePadded::new(AtomicU64::new(0)),
      evicted_by_ttl: CachePadded::new(AtomicU64::new(0)),
      evicted_by_tti: CachePadded::new(AtomicU64::new(0)),
      keys_admitted: CachePadded::new(AtomicU64::new(0)),
      keys_rejected: CachePadded::new(AtomicU64::new(0)),
      current_cost: CachePadded::new(AtomicU64::new(0)),
      total_cost_added: CachePadded::new(AtomicU64::new(0)),
      created_at: Instant::now(), // Set the creation time here.
    }
  }
}

impl Metrics {
  /// Creates a new `Metrics` instance, capturing the creation time.
  pub(crate) fn new() -> Self {
    // `default()` now correctly initializes `created_at`.
    Self::default()
  }

  /// Creates a point-in-time snapshot of the current metrics.
  pub(crate) fn snapshot(&self) -> MetricsSnapshot {
    let hits = self.hits.load(Ordering::Relaxed);
    let misses = self.misses.load(Ordering::Relaxed);
    let total_lookups = hits + misses;

    MetricsSnapshot {
      hits,
      misses,
      hit_ratio: if total_lookups == 0 {
        0.0
      } else {
        hits as f64 / total_lookups as f64
      },
      inserts: self.inserts.load(Ordering::Relaxed),
      updates: self.updates.load(Ordering::Relaxed),
      invalidations: self.invalidations.load(Ordering::Relaxed),
      evicted_by_capacity: self.evicted_by_capacity.load(Ordering::Relaxed),
      evicted_by_ttl: self.evicted_by_ttl.load(Ordering::Relaxed),
      evicted_by_tti: self.evicted_by_tti.load(Ordering::Relaxed),
      keys_admitted: self.keys_admitted.load(Ordering::Relaxed),
      keys_rejected: self.keys_rejected.load(Ordering::Relaxed),
      current_cost: self.current_cost.load(Ordering::Relaxed),
      total_cost_added: self.total_cost_added.load(Ordering::Relaxed),
      uptime_secs: self.created_at.elapsed().as_secs(),
    }
  }
}

/// A point-in-time, public-facing snapshot of the cache's metrics.
#[derive(Clone)]
pub struct MetricsSnapshot {
  /// The number of successful lookups.
  pub hits: u64,
  /// The number of failed lookups.
  pub misses: u64,
  /// The cache hit ratio (hits / (hits + misses)).
  pub hit_ratio: f64,
  /// The total number of items inserted into the cache.
  pub inserts: u64,
  /// The total number of in-place updates via `compute`.
  pub updates: u64,
  /// The total number of manual invalidations.
  pub invalidations: u64,
  /// The number of items evicted due to exceeding capacity.
  pub evicted_by_capacity: u64,
  /// The number of items evicted due to TTL expiration.
  pub evicted_by_ttl: u64,
  /// The number of items evicted due to TTI expiration.
  pub evicted_by_tti: u64,
  /// The number of keys admitted by the admission policy (e.g., TinyLFU).
  pub keys_admitted: u64,
  /// The number of keys rejected by the admission policy.
  pub keys_rejected: u64,
  /// The current total cost of all items in the cache.
  pub current_cost: u64,
  /// The cumulative cost of all items ever inserted.
  pub total_cost_added: u64,
  /// The number of seconds the cache has been running.
  pub uptime_secs: u64,
}

impl fmt::Debug for MetricsSnapshot {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("MetricsSnapshot")
      .field("hits", &self.hits)
      .field("misses", &self.misses)
      .field("hit_ratio", &format!("{:.2}%", self.hit_ratio * 100.0))
      .field("inserts", &self.inserts)
      .field("updates", &self.updates)
      .field("invalidations", &self.invalidations)
      .field("evicted_by_capacity", &self.evicted_by_capacity)
      .field("evicted_by_ttl", &self.evicted_by_ttl)
      .field("evicted_by_tti", &self.evicted_by_tti)
      .field("keys_admitted", &self.keys_admitted)
      .field("keys_rejected", &self.keys_rejected)
      .field("current_cost", &self.current_cost)
      .field("total_cost_added", &self.total_cost_added)
      .field("uptime_secs", &self.uptime_secs)
      .finish()
  }
}
