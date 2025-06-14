use super::CachePolicy;
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
  fn on_access(&self, _key: &K, _cost: u64) {}

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

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn test_new_item_admitted_to_front() {
    let policy = Fifo::<i32>::new();
    <Fifo<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &1, 1);
    <Fifo<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &2, 1);

    let list = policy.list.lock();
    // The list's internal representation has the MRU (newest) at the head.
    assert_eq!(list.keys_as_vec(), vec![2, 1]);
    assert_eq!(list.current_total_cost(), 2);
  }

  #[test]
  fn test_access_is_a_noop() {
    let policy = Fifo::<i32>::new();
    <Fifo<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &1, 1);
    <Fifo<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &2, 1);

    let keys_before = policy.list.lock().keys_as_vec();

    // Accessing an item should not change its order in a FIFO queue.
    <Fifo<i32> as CachePolicy<i32, ()>>::on_access(&policy, &1, 1);

    let keys_after = policy.list.lock().keys_as_vec();
    assert_eq!(
      keys_before, keys_after,
      "Access should not change FIFO order"
    );
  }

  #[test]
  fn test_re_admit_existing_key_is_a_noop() {
    let policy = Fifo::<i32>::new();
    <Fifo<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &1, 1);
    <Fifo<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &2, 1);

    let keys_before = policy.list.lock().keys_as_vec();
    let cost_before = policy.list.lock().current_total_cost();

    // Re-admitting an existing key should not change its order or cost.
    <Fifo<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &1, 99); // Use different cost

    let keys_after = policy.list.lock().keys_as_vec();
    let cost_after = policy.list.lock().current_total_cost();

    assert_eq!(
      keys_before, keys_after,
      "Re-admitting should not change FIFO order"
    );
    assert_eq!(
      cost_before, cost_after,
      "Re-admitting should not update cost"
    );
  }

  #[test]
  fn test_evict_removes_oldest_item() {
    let policy = Fifo::<i32>::new();
    <Fifo<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &1, 1); // Oldest
    <Fifo<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &2, 2);
    <Fifo<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &3, 3); // Newest

    // Evict one item. It should be the first one that was inserted (key 1).
    let (victims, cost_freed) = <Fifo<i32> as CachePolicy<i32, ()>>::evict(&policy, 1);

    assert_eq!(victims, vec![1]);
    assert_eq!(cost_freed, 1);

    let list = policy.list.lock();
    assert_eq!(list.keys_as_vec(), vec![3, 2]);
    assert_eq!(list.current_total_cost(), 5);
  }

  #[test]
  fn test_evict_multiple_items() {
    let policy = Fifo::<i32>::new();
    <Fifo<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &1, 1); // Oldest
    <Fifo<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &2, 1);
    <Fifo<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &3, 1); // Newest

    // Evict two items. Should be 1 and 2.
    let (victims, cost_freed) = <Fifo<i32> as CachePolicy<i32, ()>>::evict(&policy, 2);

    assert_eq!(victims, vec![1, 2]);
    assert_eq!(cost_freed, 2);

    let list = policy.list.lock();
    assert_eq!(list.keys_as_vec(), vec![3]);
    assert_eq!(list.current_total_cost(), 1);
  }

  #[test]
  fn test_on_remove_cleans_up_state() {
    let policy = Fifo::<i32>::new();
    <Fifo<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &1, 1);
    <Fifo<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &2, 2);
    assert_eq!(policy.list.lock().current_total_cost(), 3);

    // Remove an item from the middle of the queue
    <Fifo<i32> as CachePolicy<i32, ()>>::on_remove(&policy, &1);

    let list = policy.list.lock();
    assert!(!list.contains(&1));
    assert_eq!(list.keys_as_vec(), vec![2]);
    assert_eq!(list.current_total_cost(), 2);
  }

  #[test]
  fn test_clear_resets_state() {
    let policy = Fifo::<i32>::new();
    <Fifo<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &1, 1);
    <Fifo<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &2, 1);

    <Fifo<i32> as CachePolicy<i32, ()>>::clear(&policy);

    assert_eq!(policy.list.lock().current_total_cost(), 0);
    assert!(policy.list.lock().keys_as_vec().is_empty());
  }
}
