use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use crossbeam_utils::CachePadded;

const COUNTER_STRIPES: usize = 64;

#[inline]
fn counter_stripe() -> usize {
  static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
  thread_local!(static STRIPE: usize = NEXT.fetch_add(1, Ordering::Relaxed) & (COUNTER_STRIPES - 1));
  STRIPE.with(|s| *s)
}

/// A monotonic counter striped by thread, so concurrent increments stay in
/// each core's cache. `load` sums the stripes.
#[derive(Debug)]
pub(crate) struct StripedCounter {
  stripes: Box<[CachePadded<AtomicU64>]>,
}

impl StripedCounter {
  fn new() -> Self {
    Self { stripes: (0..COUNTER_STRIPES).map(|_| CachePadded::new(AtomicU64::new(0))).collect() }
  }

  #[inline]
  pub(crate) fn fetch_add(&self, n: u64, _order: Ordering) -> u64 {
    self.stripes[counter_stripe()].fetch_add(n, Ordering::Relaxed)
  }

  #[inline]
  pub(crate) fn fetch_sub(&self, n: u64, _order: Ordering) -> u64 {
    self.stripes[counter_stripe()].fetch_sub(n, Ordering::Relaxed)
  }

  /// Stripes wrap on subtraction, so the sum is taken modulo 2^64.
  pub(crate) fn load(&self, _order: Ordering) -> u64 {
    self.stripes.iter().fold(0u64, |a, s| a.wrapping_add(s.load(Ordering::Relaxed)))
  }
}

/// A thread-safe, internal metrics collector for the cache.
/// All fields are atomic to allow for lock-free updates.
#[derive(Debug)]
pub struct Metrics {
  // --- Hit/Miss Ratios ---
  // Striped by thread, so readers of one shard do not share a line. Summed in `snapshot()`.
  hits: Box<[CachePadded<AtomicU64>]>,
  misses: Box<[CachePadded<AtomicU64>]>,
  stripe_mask: usize,

  // --- Throughput ---
  pub(crate) inserts: StripedCounter,
  pub(crate) updates: StripedCounter,
  pub(crate) invalidations: StripedCounter,

  // --- Eviction Stats ---
  pub(crate) evicted_by_capacity: StripedCounter,
  pub(crate) evicted_by_ttl: StripedCounter,
  pub(crate) evicted_by_tti: StripedCounter,

  // --- Admission Policy (for TinyLFU) ---
  pub(crate) keys_admitted: StripedCounter,
  pub(crate) keys_rejected: StripedCounter,

  // --- Cost / Size ---
  pub(crate) current_cost: StripedCounter,
  pub(crate) total_cost_added: StripedCounter,

  // --- Timestamps for Uptime ---
  created_at: Instant,
  shards: std::sync::OnceLock<Box<[Arc<crate::store::ShardCounters>]>>,
}

impl Metrics {
  /// Creates a new `Metrics` instance, capturing the creation time.
  ///
  /// `num_stripes` must be a power of two; the cache passes its shard count.
  pub(crate) fn new(num_stripes: usize) -> Self {
    let num_stripes = num_stripes.max(1).next_power_of_two();
    let create_stripes = || -> Box<[CachePadded<AtomicU64>]> {
      (0..num_stripes)
        .map(|_| CachePadded::new(AtomicU64::new(0)))
        .collect()
    };
    Self {
      hits: create_stripes(),
      misses: create_stripes(),
      stripe_mask: num_stripes - 1,
      inserts: StripedCounter::new(),
      updates: StripedCounter::new(),
      invalidations: StripedCounter::new(),
      evicted_by_capacity: StripedCounter::new(),
      evicted_by_ttl: StripedCounter::new(),
      evicted_by_tti: StripedCounter::new(),
      keys_admitted: StripedCounter::new(),
      keys_rejected: StripedCounter::new(),
      current_cost: StripedCounter::new(),
      total_cost_added: StripedCounter::new(),
      created_at: Instant::now(), // Set the creation time here.
      shards: std::sync::OnceLock::new(),
    }
  }

  pub(crate) fn attach_shards(&self, shards: Box<[Arc<crate::store::ShardCounters>]>) {
    let _ = self.shards.set(shards);
  }

  #[inline]
  fn local_sum(&self, f: impl Fn(&crate::store::ShardCounters) -> &AtomicU64) -> u64 {
    self
      .shards
      .get()
      .map_or(0, |s| s.iter().fold(0u64, |a, c| a.wrapping_add(f(c).load(Ordering::Relaxed))))
  }

  pub(crate) fn current_cost_total(&self) -> u64 {
    self.current_cost.load(Ordering::Relaxed).wrapping_add(self.local_sum(|c| &c.cost_delta))
  }

  /// Records `n` hits on the calling thread's stripe.
  #[inline]
  pub(crate) fn record_hits(&self, n: u64) {
    self.hits[counter_stripe() & self.stripe_mask].fetch_add(n, Ordering::Relaxed);
  }

  /// Records `n` misses on the calling thread's stripe.
  #[inline]
  pub(crate) fn record_misses(&self, n: u64) {
    self.misses[counter_stripe() & self.stripe_mask].fetch_add(n, Ordering::Relaxed);
  }

  /// Creates a point-in-time snapshot of the current metrics.
  pub(crate) fn snapshot(&self) -> MetricsSnapshot {
    let hits: u64 = self.hits.iter().map(|s| s.load(Ordering::Relaxed)).sum();
    let misses: u64 = self.misses.iter().map(|s| s.load(Ordering::Relaxed)).sum();
    let total_lookups = hits + misses;

    MetricsSnapshot {
      hits,
      misses,
      hit_ratio: if total_lookups == 0 {
        0.0
      } else {
        hits as f64 / total_lookups as f64
      },
      inserts: self.inserts.load(Ordering::Relaxed) + self.local_sum(|c| &c.inserts),
      updates: self.updates.load(Ordering::Relaxed),
      invalidations: self.invalidations.load(Ordering::Relaxed),
      evicted_by_capacity: self.evicted_by_capacity.load(Ordering::Relaxed) + self.local_sum(|c| &c.evicted_by_capacity),
      evicted_by_ttl: self.evicted_by_ttl.load(Ordering::Relaxed),
      evicted_by_tti: self.evicted_by_tti.load(Ordering::Relaxed),
      keys_admitted: self.keys_admitted.load(Ordering::Relaxed) + self.local_sum(|c| &c.keys_admitted),
      keys_rejected: self.keys_rejected.load(Ordering::Relaxed),
      current_cost: self.current_cost_total(),
      total_cost_added: self.total_cost_added.load(Ordering::Relaxed) + self.local_sum(|c| &c.total_cost_added),
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
