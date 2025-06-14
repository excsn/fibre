use crate::policy::AdmissionDecision;

use super::CachePolicy;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::hash::Hash;

/// An entry tracked by the Clock policy, holding its referenced status.
#[derive(Clone, Copy, Debug)]
struct ClockEntry {
  cost: u64,
  referenced: bool,
}

/// An eviction policy based on the Clock (or Second-Chance) algorithm.
/// It provides an efficient approximation of LRU.
#[derive(Debug)]
pub struct ClockPolicy<K> {
  // A map storing the state of each item.
  items: Mutex<HashMap<K, ClockEntry>>,
  // A vector of keys representing the circular "clock face".
  order: Mutex<Vec<K>>,
  // The "hand" of the clock, pointing to the next eviction candidate.
  hand: Mutex<usize>,
}

impl<K> ClockPolicy<K> {
  pub fn new() -> Self {
    Self {
      items: Mutex::new(HashMap::new()),
      order: Mutex::new(Vec::new()),
      hand: Mutex::new(0),
    }
  }
}

impl<K, V> CachePolicy<K, V> for ClockPolicy<K>
where
  K: Eq + Hash + Clone + Send + Sync,
  V: Send + Sync,
{
  /// On access, set the "referenced" bit to true.
  fn on_access(&self, key: &K, _cost: u64) {
    let mut items = self.items.lock();
    if let Some(entry) = items.get_mut(key) {
      entry.referenced = true;
    }
  }

  /// On insert, add the new item to the clock.
  fn on_admit(&self, key: &K, cost: u64) -> AdmissionDecision<K> {
    let mut order = self.order.lock();
    let mut items = self.items.lock();

    if !items.contains_key(key) {
      items.insert(
        key.clone(),
        ClockEntry {
          cost,
          referenced: false,
        },
      );
      order.push(key.clone());
    }

    AdmissionDecision::Admit // Clock always admits.
  }

  /// When an item is removed, stop tracking it.
  fn on_remove(&self, key: &K) {
    let mut order = self.order.lock();
    let mut items = self.items.lock();
    let mut hand = self.hand.lock();

    if items.remove(key).is_some() {
      if let Some(pos) = order.iter().position(|k| k == key) {
        order.remove(pos);
        // If the removed item was before or at the hand, adjust the hand.
        if pos <= *hand && *hand > 0 {
          *hand -= 1;
        }
      }
    }
  }

  /// Evict items using the Clock hand sweep algorithm.
  fn evict(&self, mut cost_to_free: u64) -> (Vec<K>, u64) {
    let mut victims = Vec::new();
    let mut total_cost_freed = 0;
    let mut order = self.order.lock();
    let mut items = self.items.lock();
    let mut hand = self.hand.lock();

    while cost_to_free > 0 && !order.is_empty() {
      let mut swept = 0;
      let order_len = order.len();

      while swept < order_len * 2 {
        if *hand >= order.len() {
          *hand = 0;
        }

        let key = &order[*hand];
        let entry = items.get_mut(key).unwrap();

        if entry.referenced {
          entry.referenced = false;
          *hand += 1;
        } else {
          let key_to_evict = order.remove(*hand);
          let cost = items.remove(&key_to_evict).unwrap().cost;
          cost_to_free = cost_to_free.saturating_sub(cost);
          total_cost_freed += cost;
          victims.push(key_to_evict);
          break;
        }
        swept += 1;
      }
    }
    (victims, total_cost_freed)
  }

  /// Clear all internal tracking.
  fn clear(&self) {
    self.items.lock().clear();
    self.order.lock().clear();
    *self.hand.lock() = 0;
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn test_new_item_admitted() {
    let policy = ClockPolicy::<i32>::new();
    <ClockPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &1, 1);

    let items = policy.items.lock();
    let order = policy.order.lock();

    assert!(items.contains_key(&1));
    assert_eq!(items.get(&1).unwrap().cost, 1);
    assert!(
      !items.get(&1).unwrap().referenced,
      "New item should not be referenced"
    );
    assert_eq!(*order, vec![1]);
  }

  #[test]
  fn test_access_sets_referenced_bit() {
    let policy = ClockPolicy::<i32>::new();
    <ClockPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &1, 1);
    assert!(!policy.items.lock().get(&1).unwrap().referenced);

    <ClockPolicy<i32> as CachePolicy<i32, ()>>::on_access(&policy, &1, 1);
    assert!(
      policy.items.lock().get(&1).unwrap().referenced,
      "Accessed item should be referenced"
    );
  }

  #[test]
  fn test_evict_unreferenced_item() {
    let policy = ClockPolicy::<i32>::new();
    <ClockPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &1, 1);
    <ClockPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &2, 1);
    // Item 1 is not referenced, item 2 is not referenced.

    // The hand starts at 0. It should evict item 1.
    let (victims, cost_freed) = <ClockPolicy<i32> as CachePolicy<i32, ()>>::evict(&policy, 1);

    assert_eq!(victims, vec![1]);
    assert_eq!(cost_freed, 1);
    assert!(!policy.items.lock().contains_key(&1));
    assert_eq!(*policy.order.lock(), vec![2]);
    assert_eq!(
      *policy.hand.lock(),
      0,
      "Hand should reset or stay at 0 after eviction"
    );
  }

  #[test]
  fn test_evict_gives_second_chance() {
    let policy = ClockPolicy::<i32>::new();
    <ClockPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &1, 1);
    <ClockPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &2, 1);

    // Access item 1, giving it a second chance.
    <ClockPolicy<i32> as CachePolicy<i32, ()>>::on_access(&policy, &1, 1);

    // The hand is at 0, pointing to item 1.
    // It sees item 1 is referenced, so it unsets the bit and moves on.
    // It then sees item 2 is not referenced, so it evicts item 2.
    let (victims, cost_freed) = <ClockPolicy<i32> as CachePolicy<i32, ()>>::evict(&policy, 1);

    assert_eq!(victims, vec![2]);
    assert_eq!(cost_freed, 1);

    let items = policy.items.lock();
    let order = policy.order.lock();
    let hand = policy.hand.lock();

    assert!(items.contains_key(&1));
    assert!(!items.contains_key(&2));
    assert_eq!(*order, vec![1]);
    assert!(
      !items.get(&1).unwrap().referenced,
      "Referenced bit for item 1 should be cleared"
    );
    assert_eq!(*hand, 1, "Hand should have advanced past item 1");
  }

  #[test]
  fn test_evict_wraps_around_clock() {
    let policy = ClockPolicy::<i32>::new();
    <ClockPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &1, 1);
    <ClockPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &2, 1);

    // Access both items
    <ClockPolicy<i32> as CachePolicy<i32, ()>>::on_access(&policy, &1, 1);
    <ClockPolicy<i32> as CachePolicy<i32, ()>>::on_access(&policy, &2, 1);

    // First eviction call:
    // - Hand at 0 (item 1): referenced=true -> referenced=false, hand becomes 1.
    // - Hand at 1 (item 2): referenced=true -> referenced=false, hand becomes 2.
    // - Hand wraps to 0.
    // - Hand at 0 (item 1): referenced=false -> evict 1.
    let (victims, cost_freed) = <ClockPolicy<i32> as CachePolicy<i32, ()>>::evict(&policy, 1);

    assert_eq!(victims, vec![1]);
    assert_eq!(cost_freed, 1);
    assert!(!policy.items.lock().contains_key(&1));
    assert_eq!(*policy.order.lock(), vec![2]);
  }

  #[test]
  fn test_on_remove_adjusts_hand() {
    let policy = ClockPolicy::<i32>::new();
    <ClockPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &1, 1);
    <ClockPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &2, 1);
    <ClockPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &3, 1);

    // Advance hand to point to item 2 (index 1)
    let _ = <ClockPolicy<i32> as CachePolicy<i32, ()>>::evict(&policy, 0); // No-op evict just to move hand
    *policy.hand.lock() = 1;

    // Remove item 1 (which is before the hand)
    <ClockPolicy<i32> as CachePolicy<i32, ()>>::on_remove(&policy, &1);

    // Hand was at 1, pointing to key '2'. After removing key '1' at index 0,
    // key '2' is now at index 0. The hand should be adjusted.
    assert_eq!(*policy.order.lock(), vec![2, 3]);
    assert_eq!(*policy.hand.lock(), 0, "Hand should be decremented");
  }

  #[test]
  fn test_clear_resets_state() {
    let policy = ClockPolicy::<i32>::new();
    <ClockPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &1, 1);
    <ClockPolicy<i32> as CachePolicy<i32, ()>>::on_access(&policy, &1, 1);
    *policy.hand.lock() = 1;

    <ClockPolicy<i32> as CachePolicy<i32, ()>>::clear(&policy);

    assert!(policy.items.lock().is_empty());
    assert!(policy.order.lock().is_empty());
    assert_eq!(*policy.hand.lock(), 0);
  }

  #[test]
  fn test_evict_multiple_items() {
    let policy = ClockPolicy::<i32>::new();
    for i in 1..=5 {
      <ClockPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &i, 1);
    }
    // Access items 3 and 5
    <ClockPolicy<i32> as CachePolicy<i32, ()>>::on_access(&policy, &3, 1);
    <ClockPolicy<i32> as CachePolicy<i32, ()>>::on_access(&policy, &5, 1);

    // Evict 3 items.
    // 1. Evict 1 (unreferenced)
    // 2. Evict 2 (unreferenced)
    // 3. Spare 3 (referenced=true -> false)
    // 4. Evict 4 (unreferenced)
    let (victims, cost_freed) = <ClockPolicy<i32> as CachePolicy<i32, ()>>::evict(&policy, 3);

    assert_eq!(cost_freed, 3);
    assert_eq!(victims, vec![1, 2, 4]);

    let order = policy.order.lock();
    assert_eq!(*order, vec![3, 5]);

    let items = policy.items.lock();
    assert!(
      !items.get(&3).unwrap().referenced,
      "Item 3 should have its bit cleared"
    );
    assert!(
      items.get(&5).unwrap().referenced,
      "Item 5 was not scanned, should still be referenced"
    );
  }
}
