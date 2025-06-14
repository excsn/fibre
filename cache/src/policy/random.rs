#![cfg(feature = "random")]

use crate::policy::AdmissionDecision;

use super::CachePolicy;

use parking_lot::Mutex;
use rand::seq::IteratorRandom;
use std::collections::HashMap;
use std::hash::Hash;

/// An eviction policy that evicts entries randomly when the cache is full.
#[derive(Debug)]
pub struct RandomPolicy<K> {
  // We store the keys and their costs in a map for efficient removal
  // and cost lookup. The Mutex ensures thread-safety.
  items: Mutex<HashMap<K, u64>>,
}

impl<K> RandomPolicy<K> {
  pub fn new() -> Self {
    Self {
      items: Mutex::new(HashMap::new()),
    }
  }
}

impl<K, V> CachePolicy<K, V> for RandomPolicy<K>
where
  K: Eq + Hash + Clone + Send + Sync,
  V: Send + Sync,
{
  /// A random policy does not care about access patterns. This is a no-op.
  fn on_access(&self, _key: &K, _cost: u64) {}

  /// A random policy always admits new items. We just need to start tracking it.
  fn on_admit(&self, key: &K, cost: u64) -> AdmissionDecision<K> {
    let mut items = self.items.lock();
    items.insert(key.clone(), cost);
    AdmissionDecision::Admit // Always admit.
  }

  /// When an item is removed from the cache, we must stop tracking it.
  fn on_remove(&self, key: &K) {
    let mut items = self.items.lock();
    items.remove(key);
  }

  /// The core eviction logic.
  fn evict(&self, mut cost_to_free: u64) -> (Vec<K>, u64) {
    let mut victims = Vec::new();
    let mut total_cost_freed = 0;
    let mut items = self.items.lock();
    let mut rng = rand::rng();

    while cost_to_free > 0 && !items.is_empty() {
      let key_to_evict = match items.keys().choose(&mut rng) {
        Some(k) => k.clone(),
        None => break,
      };

      if let Some(cost) = items.remove(&key_to_evict) {
        cost_to_free = cost_to_free.saturating_sub(cost);
        total_cost_freed += cost;
        victims.push(key_to_evict);
      }
    }

    (victims, total_cost_freed)
  }

  /// Clears all tracked items.
  fn clear(&self) {
    let mut items = self.items.lock();
    items.clear();
  }
}
