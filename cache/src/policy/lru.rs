use crate::policy::AdmissionDecision;

use super::{AccessInfo, CachePolicy};

use parking_lot::Mutex;
use std::collections::{HashMap, VecDeque};
use std::hash::Hash;

/// An eviction policy that evicts the least recently used entries.
#[derive(Debug)]
pub struct Lru<K> {
  // A queue of keys ordered by recent use (front is most recent).
  pub(crate) order: Mutex<VecDeque<K>>,
  // A map for quick O(1) existence checks and cost lookups.
  pub(crate) items: Mutex<HashMap<K, u64>>,
}

impl<K> Lru<K> {
  pub fn new() -> Self {
    Self {
      order: Mutex::new(VecDeque::new()),
      items: Mutex::new(HashMap::new()),
    }
  }
}

impl<K, V> CachePolicy<K, V> for Lru<K>
where
  K: Eq + Hash + Clone + Send + Sync,
  V: Send + Sync,
{

  /// When an item is accessed, move it to the front of the usage queue.
  fn on_access(&self, info: &AccessInfo<K, V>) {
    let mut order = self.order.lock();
    // Find the key in the queue and move it to the front.
    if let Some(pos) = order.iter().position(|k| k == info.key) {
      if let Some(key) = order.remove(pos) {
        order.push_front(key);
      }
    }
  }

  /// When an item is inserted, it is the most recently used.
  fn on_admit(&self, info: &AccessInfo<K, V>) -> AdmissionDecision<K> {
    let mut order = self.order.lock();
    let mut items = self.items.lock();

    // Begin tracking the new item.
    items.insert(info.key.clone(), info.entry.cost());
    order.push_front(info.key.clone());

    AdmissionDecision::Admit // LRU always admits new items.
  }

  /// When an item is removed, stop tracking it.
  fn on_remove(&self, key: &K) {
    let mut order = self.order.lock();
    let mut items = self.items.lock();

    items.remove(key);
    order.retain(|k| k != key);
  }

  /// Evict the least recently used items until enough cost is freed.
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
