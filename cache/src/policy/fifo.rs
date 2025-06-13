use super::{AccessInfo, CachePolicy};
use crate::policy::lru_list::LruList;
use crate::policy::AdmissionDecision;

use parking_lot::Mutex;
use std::hash::Hash;

/// An eviction policy that evicts entries in a First-In, First-Out (FIFO) manner.
#[derive(Debug)]
pub struct Fifo<K: Eq + Hash + Clone> {
  // Replace the two old fields with a single, efficient list.
  list: Mutex<LruList<K>>,
}

impl<K: Eq + Hash + Clone> Fifo<K> {
  pub fn new() -> Self {
    Self {
      list: Mutex::new(LruList::new()),
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
    let mut list = self.list.lock();

    // If the key already exists, do nothing. FIFO does not update on re-insert.
    // This preserves the "First-In" part of the name.
    if !list.contains(key) {
      list.push_front(key.clone(), cost);
    }

    AdmissionDecision::Admit // FIFO always admits.
  }

  /// When an item is removed, stop tracking it.
  fn on_remove(&self, key: &K) {
    // This is now an O(1) operation.
    self.list.lock().remove(key);
  }

  /// Evict the oldest items until enough cost is freed.
  fn evict(&self, mut cost_to_free: u64) -> (Vec<K>, u64) {
    let mut victims = Vec::new();
    let mut total_cost_freed = 0;
    let mut list = self.list.lock();

    while cost_to_free > 0 {
      // pop_back is O(1) and correctly returns the oldest item.
      if let Some((key, cost)) = list.pop_back() {
        cost_to_free = cost_to_free.saturating_sub(cost);
        total_cost_freed += cost;
        victims.push(key);
      } else {
        break; // The list is empty.
      }
    }
    (victims, total_cost_freed)
  }

  /// Clear all internal tracking.
  fn clear(&self) {
    self.list.lock().clear();
  }
}