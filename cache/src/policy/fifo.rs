use crate::policy::AdmissionDecision;

use super::{AccessInfo, CachePolicy};
use parking_lot::Mutex;
use std::collections::{HashMap, VecDeque};
use std::hash::Hash;

/// An eviction policy that evicts entries in a First-In, First-Out (FIFO) manner.
#[derive(Debug)]
pub struct Fifo<K> {
  // A queue tracking the insertion order of keys.
  order: Mutex<VecDeque<K>>,
  // A map for quick O(1) existence checks and cost lookups.
  items: Mutex<HashMap<K, u64>>,
}

impl<K> Fifo<K> {
  pub fn new() -> Self {
    Self {
      order: Mutex::new(VecDeque::new()),
      items: Mutex::new(HashMap::new()),
    }
  }
}

impl<K, V> CachePolicy<K, V> for Fifo<K>
where
  K: Eq + Hash + Clone + Send + Sync,
  V: Send + Sync,
{
  /// A FIFO policy does not care about access patterns. This is a no-op.
  fn on_access(&self, _info: &AccessInfo<K, V>) {}

  /// On insert, add the new item to the front of the queue.
  fn on_admit(&self, key: &K, cost: u64) -> AdmissionDecision<K> {
    let mut order = self.order.lock();
    let mut items = self.items.lock();

    // If the key already exists, do nothing. FIFO does not update on re-insert.
    if !items.contains_key(key) {
      items.insert(key.clone(), cost);
      order.push_front(key.clone());
    }

    AdmissionDecision::Admit // FIFO always admits.
  }

  /// When an item is removed, stop tracking it.
  fn on_remove(&self, key: &K) {
    let mut order = self.order.lock();
    let mut items = self.items.lock();

    if items.remove(key).is_some() {
      order.retain(|k| k != key);
    }
  }

  /// Evict the oldest items until enough cost is freed.
  fn evict(&self, mut cost_to_free: u64) -> (Vec<K>, u64) {
    let mut victims = Vec::new();
    let mut total_cost_freed = 0;
    let mut order = self.order.lock();
    let mut items = self.items.lock();

    while cost_to_free > 0 {
      if let Some(key) = order.pop_back() {
        if let Some(cost) = items.remove(&key) {
          cost_to_free = cost_to_free.saturating_sub(cost);
          total_cost_freed += cost;
          victims.push(key);
        }
      } else {
        break;
      }
    }
    (victims, total_cost_freed)
  }

  /// Clear all internal tracking.
  fn clear(&self) {
    self.order.lock().clear();
    self.items.lock().clear();
  }
}
