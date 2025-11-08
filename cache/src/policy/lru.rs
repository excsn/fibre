//! An eviction policy based on a Least Recently Used (LRU) algorithm.
//!
//! This policy evicts the item that has not been accessed for the longest
//! period of time. It is simple and effective for many workloads but can be

//! vulnerable to cache pollution from operations that scan over many items
//! once (e.g., a "one-hit wonder").

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
  fn evict(&self, cost_to_free: u64) -> (Vec<K>, u64) {
    let mut victims = Vec::new();
    let mut total_cost_freed = 0;
    let mut list = self.list.lock();

    while total_cost_freed < cost_to_free {
      // pop_back is O(1).
      if let Some((key, cost)) = list.pop_back() {
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

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn test_new_item_admitted_to_front() {
    let policy = LruPolicy::<i32>::new();
    <LruPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &1, 1);
    <LruPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &2, 1);

    let list = policy.list.lock();
    assert_eq!(
      list.keys_as_vec(),
      vec![2, 1],
      "Newest item (2) should be at the front"
    );
    assert_eq!(list.current_total_cost(), 2);
  }

  #[test]
  fn test_access_moves_item_to_front() {
    let policy = LruPolicy::<i32>::new();
    <LruPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &1, 1);
    <LruPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &2, 1);
    <LruPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &3, 1);
    assert_eq!(policy.list.lock().keys_as_vec(), vec![3, 2, 1]);

    // Access the least recently used item (1).
    <LruPolicy<i32> as CachePolicy<i32, ()>>::on_access(&policy, &1, 1);

    // Item 1 should now be at the front.
    assert_eq!(policy.list.lock().keys_as_vec(), vec![1, 3, 2]);
  }

  #[test]
  fn test_re_admit_existing_key_moves_to_front() {
    let policy = LruPolicy::<i32>::new();
    <LruPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &1, 1);
    <LruPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &2, 1);
    assert_eq!(policy.list.lock().keys_as_vec(), vec![2, 1]);

    // Re-admitting an existing key should act like an access.
    <LruPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &1, 1);

    assert_eq!(
      policy.list.lock().keys_as_vec(),
      vec![1, 2],
      "Re-admitted key should move to front"
    );
  }

  #[test]
  fn test_re_admit_updates_cost() {
    let policy = LruPolicy::<i32>::new();
    <LruPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &1, 1);
    <LruPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &2, 2);
    assert_eq!(policy.list.lock().current_total_cost(), 3);

    // Re-admit key 1 with a new cost.
    <LruPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &1, 10);

    assert_eq!(
      policy.list.lock().current_total_cost(),
      12,
      "Cost should be updated (10 + 2)"
    );
    assert_eq!(policy.list.lock().keys_as_vec(), vec![1, 2]);
  }

  #[test]
  fn test_evict_removes_lru_item() {
    let policy = LruPolicy::<i32>::new();
    <LruPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &1, 1); // Becomes LRU
    <LruPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &2, 2);
    <LruPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &3, 3);
    <LruPolicy<i32> as CachePolicy<i32, ()>>::on_access(&policy, &2, 2); // 1 is still LRU

    // Evict one item. It should be key 1.
    let (victims, cost_freed) = <LruPolicy<i32> as CachePolicy<i32, ()>>::evict(&policy, 1);

    assert_eq!(victims, vec![1]);
    assert_eq!(cost_freed, 1);

    let list = policy.list.lock();
    assert_eq!(list.keys_as_vec(), vec![2, 3]);
    assert_eq!(list.current_total_cost(), 5);
  }

  #[test]
  fn test_evict_multiple_items() {
    let policy = LruPolicy::<i32>::new();
    <LruPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &1, 1); // LRU
    <LruPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &2, 1); // Second LRU
    <LruPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &3, 1); // MRU

    // Evict two items. Should be 1 and 2.
    let (victims, cost_freed) = <LruPolicy<i32> as CachePolicy<i32, ()>>::evict(&policy, 2);

    assert_eq!(victims, vec![1, 2]);
    assert_eq!(cost_freed, 2);

    let list = policy.list.lock();
    assert_eq!(list.keys_as_vec(), vec![3]);
    assert_eq!(list.current_total_cost(), 1);
  }

  #[test]
  fn test_on_remove_cleans_up_state() {
    let policy = LruPolicy::<i32>::new();
    <LruPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &1, 1);
    <LruPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &2, 2);
    assert_eq!(policy.list.lock().current_total_cost(), 3);

    <LruPolicy<i32> as CachePolicy<i32, ()>>::on_remove(&policy, &1);

    let list = policy.list.lock();
    assert!(!list.contains(&1));
    assert_eq!(list.keys_as_vec(), vec![2]);
    assert_eq!(list.current_total_cost(), 2);
  }

  #[test]
  fn test_clear_resets_state() {
    let policy = LruPolicy::<i32>::new();
    <LruPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &1, 1);
    <LruPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &2, 1);

    <LruPolicy<i32> as CachePolicy<i32, ()>>::clear(&policy);

    assert_eq!(policy.list.lock().current_total_cost(), 0);
    assert!(policy.list.lock().keys_as_vec().is_empty());
  }
}
