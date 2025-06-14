use crate::policy::lru_list::LruList;
use crate::policy::AdmissionDecision;

use super::CachePolicy;

use parking_lot::Mutex;
use std::hash::Hash;

/// An eviction policy that evicts the least recently used entries.
#[derive(Debug)]
pub struct LruPolicy<K: Eq + Hash + Clone> {
  // Replace the two old fields with a single, efficient list.
  list: Mutex<LruList<K>>,
}

impl<K: Eq + Hash + Clone> LruPolicy<K> {
  pub fn new() -> Self {
    Self {
      list: Mutex::new(LruList::new()),
    }
  }
}

impl<K, V> CachePolicy<K, V> for LruPolicy<K>
where
  K: Eq + Hash + Clone + Send + Sync,
  V: Send + Sync,
{
  /// When an item is accessed, move it to the front of the usage queue.
  fn on_access(&self, key: &K, _cost: u64) {
    // This is now an O(1) operation.
    self.list.lock().move_to_front(key);
  }

  /// When an item is inserted, it is the most recently used.
  fn on_admit(&self, key: &K, cost: u64) -> AdmissionDecision<K> {
    // This is now an O(1) operation.
    self.list.lock().push_front(key.clone(), cost);
    AdmissionDecision::Admit // LRU always admits new items.
  }

  /// When an item is removed, stop tracking it.
  fn on_remove(&self, key: &K) {
    // This is now an O(1) operation.
    self.list.lock().remove(key);
  }

  /// Evict the least recently used items until enough cost is freed.
  fn evict(&self, mut cost_to_free: u64) -> (Vec<K>, u64) {
    let mut victims = Vec::new();
    let mut total_cost_freed = 0;
    let mut list = self.list.lock();

    while cost_to_free > 0 {
      // pop_back is O(1).
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
