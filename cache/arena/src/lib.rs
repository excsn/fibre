//! Cross-implementation cache benchmarks: fibre_cache against moka, quick_cache
//! and stretto through one fixed-duration workload harness.

pub mod cli;
pub mod engines;
pub mod harness;
pub mod machine;
pub mod report;

use std::future::Future;

/// A cache seen from synchronous code. Keys and values are `u64`, every entry
/// costs 1 and the capacity is a number of entries.
pub trait SyncCache: Clone + Send + Sync + 'static {
  fn get(&self, key: u64) -> bool;
  fn insert(&self, key: u64);
  /// Entries the implementation reports holding.
  fn len(&self) -> u64;
  /// Runs whatever pending work the implementation defers.
  fn settle(&self);
}

/// A cache seen from async code.
pub trait AsyncCache: Clone + Send + Sync + 'static {
  fn get(&self, key: u64) -> impl Future<Output = bool> + Send;
  fn insert(&self, key: u64) -> impl Future<Output = ()> + Send;
  fn len(&self) -> u64;
  fn settle(&self) -> impl Future<Output = ()> + Send;
}

/// One implementation under test. A builder returns `None` for a mode the
/// implementation has no API for; mixed needs one instance reachable from both
/// sync and async code.
pub trait Engine {
  const NAME: &'static str;
  type Sync: SyncCache;
  type Async: AsyncCache;
  fn build_sync(capacity: u64) -> Option<Self::Sync>;
  /// Called inside the run's tokio runtime.
  fn build_async(capacity: u64) -> Option<Self::Async>;
  /// Called inside the run's tokio runtime.
  fn build_mixed(capacity: u64) -> Option<(Self::Sync, Self::Async)>;
}
