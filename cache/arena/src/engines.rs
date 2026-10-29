//! The implementations under test, each configured for a capacity in entries
//! with every entry costing 1 and everything else at its default.

use crate::{AsyncCache, Engine, SyncCache};
use std::sync::Arc;

pub struct Fibre;

#[derive(Clone)]
pub struct FibreSync(fibre_cache::Cache<u64, u64>);

#[derive(Clone)]
pub struct FibreAsync(fibre_cache::AsyncCache<u64, u64>);

fn fibre_build(capacity: u64) -> fibre_cache::Cache<u64, u64> {
  fibre_cache::CacheBuilder::default().capacity(capacity).build().expect("fibre_cache build")
}

impl SyncCache for FibreSync {
  fn get(&self, key: u64) -> bool {
    self.0.fetch(&key).is_some()
  }
  fn insert(&self, key: u64) {
    self.0.insert(key, key, 1);
  }
  fn len(&self) -> u64 {
    self.0.metrics().current_cost
  }
  fn settle(&self) {
    self.0.run_maintenance();
  }
}

impl AsyncCache for FibreAsync {
  async fn get(&self, key: u64) -> bool {
    self.0.fetch(&key).await.is_some()
  }
  async fn insert(&self, key: u64) {
    self.0.insert(key, key, 1).await;
  }
  fn len(&self) -> u64 {
    self.0.metrics().current_cost
  }
  async fn settle(&self) {
    self.0.run_maintenance().await;
  }
}

impl Engine for Fibre {
  const NAME: &'static str = "fibre_cache.s3fifo";
  type Sync = FibreSync;
  type Async = FibreAsync;
  fn build_sync(capacity: u64) -> Option<FibreSync> {
    Some(FibreSync(fibre_build(capacity)))
  }
  fn build_async(capacity: u64) -> Option<FibreAsync> {
    Some(FibreAsync(fibre_build(capacity).to_async()))
  }
  fn build_mixed(capacity: u64) -> Option<(FibreSync, FibreAsync)> {
    let c = fibre_build(capacity);
    let a = c.to_async();
    Some((FibreSync(c), FibreAsync(a)))
  }
}

/// fibre_cache with the W-TinyLFU built-in selected on the builder.
pub struct FibreTinyLfu;

fn fibre_tinylfu_build(capacity: u64) -> fibre_cache::Cache<u64, u64> {
  fibre_cache::CacheBuilder::default().capacity(capacity).tinylfu().build().expect("fibre_cache build")
}

impl Engine for FibreTinyLfu {
  const NAME: &'static str = "fibre_cache.tinylfu";
  type Sync = FibreSync;
  type Async = FibreAsync;
  fn build_sync(capacity: u64) -> Option<FibreSync> {
    Some(FibreSync(fibre_tinylfu_build(capacity)))
  }
  fn build_async(capacity: u64) -> Option<FibreAsync> {
    Some(FibreAsync(fibre_tinylfu_build(capacity).to_async()))
  }
  fn build_mixed(capacity: u64) -> Option<(FibreSync, FibreAsync)> {
    let c = fibre_tinylfu_build(capacity);
    let a = c.to_async();
    Some((FibreSync(c), FibreAsync(a)))
  }
}

pub struct Moka;

#[derive(Clone)]
pub struct MokaSync(moka::sync::Cache<u64, u64>);

#[derive(Clone)]
pub struct MokaAsync(moka::future::Cache<u64, u64>);

impl SyncCache for MokaSync {
  fn get(&self, key: u64) -> bool {
    self.0.get(&key).is_some()
  }
  fn insert(&self, key: u64) {
    self.0.insert(key, key);
  }
  fn len(&self) -> u64 {
    self.0.entry_count()
  }
  fn settle(&self) {
    self.0.run_pending_tasks();
  }
}

impl AsyncCache for MokaAsync {
  async fn get(&self, key: u64) -> bool {
    self.0.get(&key).await.is_some()
  }
  async fn insert(&self, key: u64) {
    self.0.insert(key, key).await;
  }
  fn len(&self) -> u64 {
    self.0.entry_count()
  }
  async fn settle(&self) {
    self.0.run_pending_tasks().await;
  }
}

impl Engine for Moka {
  const NAME: &'static str = "moka";
  type Sync = MokaSync;
  type Async = MokaAsync;
  fn build_sync(capacity: u64) -> Option<MokaSync> {
    Some(MokaSync(moka::sync::Cache::new(capacity)))
  }
  fn build_async(capacity: u64) -> Option<MokaAsync> {
    Some(MokaAsync(moka::future::Cache::new(capacity)))
  }
  /// `moka::sync::Cache` and `moka::future::Cache` are separate instances.
  fn build_mixed(_capacity: u64) -> Option<(MokaSync, MokaAsync)> {
    None
  }
}

/// quick_cache has no async API. The async side calls the sync methods from
/// inside the task, as an async caller of quick_cache does.
pub struct Quick;

#[derive(Clone)]
pub struct QuickCache(Arc<quick_cache::sync::Cache<u64, u64>>);

impl SyncCache for QuickCache {
  fn get(&self, key: u64) -> bool {
    self.0.get(&key).is_some()
  }
  fn insert(&self, key: u64) {
    self.0.insert(key, key);
  }
  fn len(&self) -> u64 {
    self.0.len() as u64
  }
  fn settle(&self) {}
}

impl AsyncCache for QuickCache {
  async fn get(&self, key: u64) -> bool {
    self.0.get(&key).is_some()
  }
  async fn insert(&self, key: u64) {
    self.0.insert(key, key);
  }
  fn len(&self) -> u64 {
    self.0.len() as u64
  }
  async fn settle(&self) {}
}

impl Engine for Quick {
  const NAME: &'static str = "quick_cache";
  type Sync = QuickCache;
  type Async = QuickCache;
  fn build_sync(capacity: u64) -> Option<QuickCache> {
    Some(QuickCache(Arc::new(quick_cache::sync::Cache::new(capacity as usize))))
  }
  fn build_async(capacity: u64) -> Option<QuickCache> {
    Self::build_sync(capacity)
  }
  fn build_mixed(capacity: u64) -> Option<(QuickCache, QuickCache)> {
    let c = Self::build_sync(capacity)?;
    Some((c.clone(), c))
  }
}

/// Counters at ten times the capacity, as stretto's documentation recommends,
/// with its internal per-entry cost ignored so the capacity is in entries.
pub struct Stretto;

#[derive(Clone)]
pub struct StrettoSync(stretto::Cache<u64, u64>);

#[derive(Clone)]
pub struct StrettoAsync(stretto::AsyncCache<u64, u64>);

impl SyncCache for StrettoSync {
  fn get(&self, key: u64) -> bool {
    self.0.get(&key).is_some()
  }
  fn insert(&self, key: u64) {
    self.0.insert(key, key, 1);
  }
  fn len(&self) -> u64 {
    self.0.len() as u64
  }
  fn settle(&self) {
    let _ = self.0.wait();
  }
}

impl AsyncCache for StrettoAsync {
  async fn get(&self, key: u64) -> bool {
    self.0.get(&key).await.is_some()
  }
  async fn insert(&self, key: u64) {
    self.0.insert(key, key, 1).await;
  }
  fn len(&self) -> u64 {
    self.0.len() as u64
  }
  async fn settle(&self) {
    let _ = self.0.wait().await;
  }
}

impl Engine for Stretto {
  const NAME: &'static str = "stretto";
  type Sync = StrettoSync;
  type Async = StrettoAsync;
  fn build_sync(capacity: u64) -> Option<StrettoSync> {
    let c = stretto::CacheBuilder::new(capacity as usize * 10, capacity as i64)
      .set_ignore_internal_cost(true)
      .finalize()
      .expect("stretto build");
    Some(StrettoSync(c))
  }
  fn build_async(capacity: u64) -> Option<StrettoAsync> {
    let c = stretto::AsyncCacheBuilder::new(capacity as usize * 10, capacity as i64)
      .set_ignore_internal_cost(true)
      .finalize(tokio::spawn)
      .expect("stretto build");
    Some(StrettoAsync(c))
  }
  /// `stretto::Cache` and `stretto::AsyncCache` are separate instances.
  fn build_mixed(_capacity: u64) -> Option<(StrettoSync, StrettoAsync)> {
    None
  }
}

/// What the pages say about each engine: its eviction policy as the crate
/// documents it and how the arena builds it.
pub struct EngineDoc {
  pub name: &'static str,
  pub policy: &'static str,
  pub built_with: &'static str,
}

pub const DOCS: [EngineDoc; 5] = [
  EngineDoc {
    name: Fibre::NAME,
    policy: "S3-FIFO, the default",
    built_with: "`CacheBuilder::default().capacity(n)`",
  },
  EngineDoc {
    name: FibreTinyLfu::NAME,
    policy: "W-TinyLFU",
    built_with: "`CacheBuilder::default().capacity(n).tinylfu()`",
  },
  EngineDoc {
    name: Moka::NAME,
    policy: "TinyLFU admission, LRU eviction",
    built_with: "`sync::Cache::new(n)`, `future::Cache::new(n)`",
  },
  EngineDoc {
    name: Quick::NAME,
    policy: "S3-FIFO",
    built_with: "`sync::Cache::new(n)`",
  },
  EngineDoc {
    name: Stretto::NAME,
    policy: "TinyLFU admission, SampledLFU eviction",
    built_with: "`CacheBuilder::new(10 n, n)` with internal cost ignored, `AsyncCacheBuilder` with `tokio::spawn`",
  },
];

/// Runs one cell, or returns `None` when the engine has no API for its mode.
pub type Runner = fn(&crate::harness::Cfg) -> Option<crate::harness::Report>;

/// The engines this crate measures, in the order cells run them.
pub fn builtins() -> Vec<(&'static str, Runner)> {
  use crate::harness::run;
  vec![
    (Fibre::NAME, run::<Fibre> as Runner),
    (FibreTinyLfu::NAME, run::<FibreTinyLfu>),
    (Moka::NAME, run::<Moka>),
    (Quick::NAME, run::<Quick>),
    (Stretto::NAME, run::<Stretto>),
  ]
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn every_engine_is_documented() {
    for (name, _) in builtins() {
      assert!(DOCS.iter().any(|d| d.name == name), "{name}");
    }
  }
}
