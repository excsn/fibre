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

/// The unified mutable state of the Clock algorithm, held under a single lock.
#[derive(Debug)]
struct ClockState<K> {
  items: HashMap<K, ClockEntry>,
  order: Vec<K>,
  hand: usize,
}

impl<K: Eq + Hash + Clone> ClockState<K> {
  fn new() -> Self {
    Self {
      items: HashMap::new(),
      order: Vec::new(),
      hand: 0,
    }
  }
}

/// An eviction policy based on the Clock (or Second-Chance) algorithm.
/// It provides an efficient approximation of LRU.
#[derive(Debug)]
pub struct ClockPolicy<K> {
  state: Mutex<ClockState<K>>,
}

impl<K> ClockPolicy<K> {
  pub fn new() -> Self {
    Self {
      state: Mutex::new(ClockState {
        items: HashMap::new(),
        order: Vec::new(),
        hand: 0,
      }),
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
    let mut state = self.state.lock();
    if let Some(entry) = state.items.get_mut(key) {
      entry.referenced = true;
    }
  }

  /// On insert, add the new item to the clock.
  fn on_admit(&self, key: &K, cost: u64) -> AdmissionDecision<K> {
    let mut state = self.state.lock();
    if !state.items.contains_key(key) {
      state.items.insert(
        key.clone(),
        ClockEntry {
          cost,
          referenced: false,
        },
      );
      state.order.push(key.clone());
    }
    AdmissionDecision::Admit
  }

  /// When an item is removed, stop tracking it.
  fn on_remove(&self, key: &K) {
    let mut state = self.state.lock();
    if state.items.remove(key).is_some() {
      if let Some(pos) = state.order.iter().position(|k| k == key) {
        state.order.remove(pos);
        if pos <= state.hand && state.hand > 0 {
          state.hand -= 1;
        }
      }
    }
  }

  /// Evict items using the Clock hand sweep algorithm.
  fn evict(&self, mut cost_to_free: u64) -> (Vec<K>, u64) {
    let mut victims = Vec::new();
    let mut total_cost_freed = 0;
    let mut state = self.state.lock();

    while cost_to_free > 0 && !state.order.is_empty() {
      let mut swept = 0;
      let order_len = state.order.len();

      while swept < order_len * 2 {
        if state.hand >= state.order.len() {
          state.hand = 0;
        }

        // Clone the key so we hold no reference into `state.order` during
        // the subsequent mutable accesses to `state.items` or `state.order`.
        let hand = state.hand;
        let key = state.order[hand].clone();
        let referenced = state.items.get(&key).map_or(false, |e| e.referenced);

        if referenced {
          state.items.get_mut(&key).unwrap().referenced = false;
          state.hand += 1;
        } else {
          state.order.remove(hand);
          let cost = state.items.remove(&key).unwrap().cost;
          cost_to_free = cost_to_free.saturating_sub(cost);
          total_cost_freed += cost;
          victims.push(key);
          break;
        }
        swept += 1;
      }
    }
    (victims, total_cost_freed)
  }

  /// Clear all internal tracking.
  fn clear(&self) {
    let mut state = self.state.lock();
    state.items.clear();
    state.order.clear();
    state.hand = 0;
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn test_new_item_admitted() {
    let policy = ClockPolicy::<i32>::new();
    <ClockPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &1, 1);

    let state = policy.state.lock();

    assert!(state.items.contains_key(&1));
    assert_eq!(state.items.get(&1).unwrap().cost, 1);
    assert!(
      !state.items.get(&1).unwrap().referenced,
      "New item should not be referenced"
    );
    assert_eq!(*state.order, vec![1]);
  }

  #[test]
  fn test_access_sets_referenced_bit() {
    let policy = ClockPolicy::<i32>::new();
    <ClockPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &1, 1);
    assert!(!policy.state.lock().items.get(&1).unwrap().referenced);

    <ClockPolicy<i32> as CachePolicy<i32, ()>>::on_access(&policy, &1, 1);
    assert!(
      policy.state.lock().items.get(&1).unwrap().referenced,
      "Accessed item should be referenced"
    );
  }

  #[test]
  fn test_evict_unreferenced_item() {
    let policy = ClockPolicy::<i32>::new();
    <ClockPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &1, 1);
    <ClockPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &2, 1);

    let (victims, cost_freed) = <ClockPolicy<i32> as CachePolicy<i32, ()>>::evict(&policy, 1);

    assert_eq!(victims, vec![1]);
    assert_eq!(cost_freed, 1);
    assert!(!policy.state.lock().items.contains_key(&1));
    assert_eq!(*policy.state.lock().order, vec![2]);
    assert_eq!(
      policy.state.lock().hand,
      0,
      "Hand should reset or stay at 0 after eviction"
    );
  }

  #[test]
  fn test_evict_gives_second_chance() {
    let policy = ClockPolicy::<i32>::new();
    <ClockPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &1, 1);
    <ClockPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &2, 1);

    <ClockPolicy<i32> as CachePolicy<i32, ()>>::on_access(&policy, &1, 1);

    let (victims, cost_freed) = <ClockPolicy<i32> as CachePolicy<i32, ()>>::evict(&policy, 1);

    assert_eq!(victims, vec![2]);
    assert_eq!(cost_freed, 1);

    let state = policy.state.lock();

    assert!(state.items.contains_key(&1));
    assert!(!state.items.contains_key(&2));
    assert_eq!(*state.order, vec![1]);
    assert!(
      !state.items.get(&1).unwrap().referenced,
      "Referenced bit for item 1 should be cleared"
    );
    assert_eq!(state.hand, 1, "Hand should have advanced past item 1");
  }

  #[test]
  fn test_evict_wraps_around_clock() {
    let policy = ClockPolicy::<i32>::new();
    <ClockPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &1, 1);
    <ClockPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &2, 1);

    <ClockPolicy<i32> as CachePolicy<i32, ()>>::on_access(&policy, &1, 1);
    <ClockPolicy<i32> as CachePolicy<i32, ()>>::on_access(&policy, &2, 1);

    let (victims, cost_freed) = <ClockPolicy<i32> as CachePolicy<i32, ()>>::evict(&policy, 1);

    assert_eq!(victims, vec![1]);
    assert_eq!(cost_freed, 1);
    assert!(!policy.state.lock().items.contains_key(&1));
    assert_eq!(*policy.state.lock().order, vec![2]);
  }

  #[test]
  fn test_on_remove_adjusts_hand() {
    let policy = ClockPolicy::<i32>::new();
    <ClockPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &1, 1);
    <ClockPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &2, 1);
    <ClockPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &3, 1);

    // Advance hand to point to item 2 (index 1)
    let _ = <ClockPolicy<i32> as CachePolicy<i32, ()>>::evict(&policy, 0);
    policy.state.lock().hand = 1;

    // Remove item 1 (which is before the hand)
    <ClockPolicy<i32> as CachePolicy<i32, ()>>::on_remove(&policy, &1);

    assert_eq!(*policy.state.lock().order, vec![2, 3]);
    assert_eq!(policy.state.lock().hand, 0, "Hand should be decremented");
  }

  #[test]
  fn test_clear_resets_state() {
    let policy = ClockPolicy::<i32>::new();
    <ClockPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &1, 1);
    <ClockPolicy<i32> as CachePolicy<i32, ()>>::on_access(&policy, &1, 1);
    policy.state.lock().hand = 1;

    <ClockPolicy<i32> as CachePolicy<i32, ()>>::clear(&policy);

    assert!(policy.state.lock().items.is_empty());
    assert!(policy.state.lock().order.is_empty());
    assert_eq!(policy.state.lock().hand, 0);
  }

  #[test]
  fn test_evict_multiple_items() {
    let policy = ClockPolicy::<i32>::new();
    for i in 1..=5 {
      <ClockPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &i, 1);
    }
    <ClockPolicy<i32> as CachePolicy<i32, ()>>::on_access(&policy, &3, 1);
    <ClockPolicy<i32> as CachePolicy<i32, ()>>::on_access(&policy, &5, 1);

    let (victims, cost_freed) = <ClockPolicy<i32> as CachePolicy<i32, ()>>::evict(&policy, 3);

    assert_eq!(cost_freed, 3);
    assert_eq!(victims, vec![1, 2, 4]);

    let state = policy.state.lock();
    assert_eq!(*state.order, vec![3, 5]);
    assert!(
      !state.items.get(&3).unwrap().referenced,
      "Item 3 should have its bit cleared"
    );
    assert!(
      state.items.get(&5).unwrap().referenced,
      "Item 5 was not scanned, should still be referenced"
    );
  }
}
