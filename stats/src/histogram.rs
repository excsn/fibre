//! A histogram of durations with exponential buckets.
//!
//! Each power of two is split into eight buckets, so a bucket is at most 12.5%
//! wide relative to its lower bound and a value read back from the bucket's
//! midpoint is within 6.25% of the true one. Two histograms add bucket by
//! bucket, so they can be merged in any order and in any place.

use std::time::Duration;

/// How many buckets each power of two is split into, as a shift.
pub const SUB_BUCKET_BITS: u32 = 3;

const SUB_BUCKETS: u64 = 1 << SUB_BUCKET_BITS;

/// The bucket index for a value in nanoseconds.
///
/// Values below eight are their own bucket. Indexes 8 to 23 are never produced.
pub fn bucket_index(ns: u64) -> u16 {
  if ns < SUB_BUCKETS {
    return ns as u16;
  }
  let octave = 63 - ns.leading_zeros();
  let sub = (ns >> (octave - SUB_BUCKET_BITS)) & (SUB_BUCKETS - 1);
  (octave as u16) * (SUB_BUCKETS as u16) + sub as u16
}

/// The inclusive bounds of a bucket in nanoseconds. An index that is never
/// produced returns `(0, 0)`.
pub fn bucket_bounds(index: u16) -> (u64, u64) {
  let index = index as u64;
  if index < SUB_BUCKETS {
    return (index, index);
  }
  let octave = (index / SUB_BUCKETS) as u32;
  if octave < SUB_BUCKET_BITS {
    return (0, 0);
  }
  let sub = index % SUB_BUCKETS;
  let shift = octave - SUB_BUCKET_BITS;
  let lower = (SUB_BUCKETS + sub) << shift;
  let upper = lower + ((1u64 << shift) - 1);
  (lower, upper)
}

/// Durations recorded in one window of one series.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Histogram {
  count: u64,
  sum_ns: u64,
  min_ns: u64,
  max_ns: u64,
  buckets: Vec<(u16, u32)>,
}

impl Histogram {
  pub fn new() -> Self {
    Histogram::default()
  }

  pub fn add(&mut self, duration: Duration) {
    let ns = u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX);
    self.add_ns(ns);
  }

  pub fn add_ns(&mut self, ns: u64) {
    if self.count == 0 {
      self.min_ns = ns;
      self.max_ns = ns;
    } else {
      self.min_ns = self.min_ns.min(ns);
      self.max_ns = self.max_ns.max(ns);
    }
    self.count += 1;
    self.sum_ns = self.sum_ns.saturating_add(ns);
    let index = bucket_index(ns);
    match self.buckets.binary_search_by_key(&index, |(i, _)| *i) {
      Ok(at) => self.buckets[at].1 = self.buckets[at].1.saturating_add(1),
      Err(at) => self.buckets.insert(at, (index, 1)),
    }
  }

  /// Adds every bucket of `other` to this histogram.
  pub fn merge(&mut self, other: &Histogram) {
    if other.count == 0 {
      return;
    }
    if self.count == 0 {
      *self = other.clone();
      return;
    }
    self.count += other.count;
    self.sum_ns = self.sum_ns.saturating_add(other.sum_ns);
    self.min_ns = self.min_ns.min(other.min_ns);
    self.max_ns = self.max_ns.max(other.max_ns);
    for &(index, n) in &other.buckets {
      match self.buckets.binary_search_by_key(&index, |(i, _)| *i) {
        Ok(at) => self.buckets[at].1 = self.buckets[at].1.saturating_add(n),
        Err(at) => self.buckets.insert(at, (index, n)),
      }
    }
  }

  pub fn count(&self) -> u64 {
    self.count
  }

  pub fn is_empty(&self) -> bool {
    self.count == 0
  }

  pub fn sum(&self) -> Duration {
    Duration::from_nanos(self.sum_ns)
  }

  /// The smallest duration recorded. Zero when empty.
  pub fn min(&self) -> Duration {
    Duration::from_nanos(self.min_ns)
  }

  /// The largest duration recorded. Zero when empty.
  pub fn max(&self) -> Duration {
    Duration::from_nanos(self.max_ns)
  }

  pub fn mean(&self) -> Option<Duration> {
    if self.count == 0 {
      return None;
    }
    Some(Duration::from_nanos(self.sum_ns / self.count))
  }

  /// The duration at quantile `q` in `[0, 1]`, estimated from the midpoint of
  /// the bucket holding it and clamped to the recorded min and max.
  pub fn quantile(&self, q: f64) -> Option<Duration> {
    if self.count == 0 {
      return None;
    }
    let q = q.clamp(0.0, 1.0);
    let target = ((q * self.count as f64).ceil() as u64).max(1);
    if target >= self.count {
      return Some(Duration::from_nanos(self.max_ns));
    }
    let mut seen = 0u64;
    for &(index, n) in &self.buckets {
      seen += n as u64;
      if seen >= target {
        let (lower, upper) = bucket_bounds(index);
        let mid = lower + (upper - lower) / 2;
        return Some(Duration::from_nanos(mid.clamp(self.min_ns, self.max_ns)));
      }
    }
    Some(Duration::from_nanos(self.max_ns))
  }

  /// Buckets with a count, as `(index, count)` sorted by index.
  pub fn buckets(&self) -> &[(u16, u32)] {
    &self.buckets
  }

  /// Rebuilds a histogram from its parts, for a sink or a store reading one back.
  pub fn from_parts(count: u64, sum_ns: u64, min_ns: u64, max_ns: u64, mut buckets: Vec<(u16, u32)>) -> Self {
    buckets.sort_by_key(|(i, _)| *i);
    Histogram {
      count,
      sum_ns,
      min_ns,
      max_ns,
      buckets,
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn small_values_are_exact_buckets() {
    for v in 0..8u64 {
      assert_eq!(bucket_index(v), v as u16);
      assert_eq!(bucket_bounds(v as u16), (v, v));
    }
  }

  #[test]
  fn bounds_contain_their_values() {
    for v in [8u64, 9, 15, 16, 100, 1_000, 999_999, 1_000_000, 123_456_789, u64::MAX / 3, u64::MAX] {
      let (lower, upper) = bucket_bounds(bucket_index(v));
      assert!(lower <= v && v <= upper, "{v} not in [{lower}, {upper}]");
      assert!((upper - lower) as f64 / lower as f64 <= 0.125 + 1e-12);
    }
  }

  #[test]
  fn buckets_are_contiguous() {
    let mut expected_lower = 8u64;
    for index in 24u16..(64 * 8) {
      let (lower, upper) = bucket_bounds(index);
      assert_eq!(lower, expected_lower, "index {index}");
      expected_lower = upper.wrapping_add(1);
    }
    assert_eq!(expected_lower, 0);
  }

  #[test]
  fn quantiles_and_merge() {
    let mut a = Histogram::new();
    for ms in 1..=100u64 {
      a.add(Duration::from_millis(ms));
    }
    assert_eq!(a.count(), 100);
    assert_eq!(a.min(), Duration::from_millis(1));
    assert_eq!(a.max(), Duration::from_millis(100));
    let p50 = a.quantile(0.5).unwrap().as_secs_f64();
    assert!((p50 - 0.050).abs() / 0.050 < 0.07, "p50 {p50}");
    let p99 = a.quantile(0.99).unwrap().as_secs_f64();
    assert!((p99 - 0.099).abs() / 0.099 < 0.07, "p99 {p99}");
    assert_eq!(a.quantile(1.0), Some(Duration::from_millis(100)));

    let mut b = Histogram::new();
    b.add(Duration::from_millis(500));
    let mut merged = a.clone();
    merged.merge(&b);
    assert_eq!(merged.count(), 101);
    assert_eq!(merged.max(), Duration::from_millis(500));
    assert_eq!(merged.quantile(1.0), Some(Duration::from_millis(500)));

    let mut other_order = b.clone();
    other_order.merge(&a);
    assert_eq!(other_order, merged);
  }

  #[test]
  fn empty_histogram() {
    let h = Histogram::new();
    assert!(h.is_empty());
    assert_eq!(h.quantile(0.5), None);
    assert_eq!(h.mean(), None);
    let mut m = Histogram::new();
    m.merge(&h);
    assert!(m.is_empty());
  }
}
