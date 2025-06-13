use crate::policy::{AccessInfo, AdmissionDecision, CachePolicy};

/// A default "no-op" eviction policy for unbounded caches or simple cases.
/// It does nothing and never evicts anything.
#[derive(Debug, Default)]
pub struct NullPolicy;

impl<K, V> CachePolicy<K, V> for NullPolicy
where
  K: Send + Sync,
  V: Send + Sync,
{
  
  fn on_access(&self, _info: &AccessInfo<K, V>) {}

  fn on_admit(&self, _info: &AccessInfo<K, V>) -> AdmissionDecision<K> {
    AdmissionDecision::Admit // Always admit new items.
  }

  fn on_remove(&self, _key: &K) {}

  fn evict(&self, _cost_to_free: u64) -> (Vec<K>, u64) {
    (Vec::new(), 0)
  }

  fn clear(&self) {}
}
