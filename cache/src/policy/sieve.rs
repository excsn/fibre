use crate::policy::AdmissionDecision;

use super::CachePolicy;

use parking_lot::Mutex;
use std::collections::{HashMap, VecDeque};
use std::hash::Hash;

/// An entry tracked by the SIEVE policy, containing its cost and visited status.
#[derive(Clone, Copy, Debug)]
struct SieveEntry {
  cost: u64,
  visited: bool,
}

/// The unified mutable state of the SIEVE algorithm, held under a single lock.
#[derive(Debug)]
struct SieveState<K> {
  order: VecDeque<K>,
  items: HashMap<K, SieveEntry>,
  hand: usize,
}

impl<K: Eq + Hash + Clone> SieveState<K> {
  fn new() -> Self {
    Self {
      order: VecDeque::new(),
      items: HashMap::new(),
      hand: 0,
    }
  }
}

/// An eviction policy based on the SIEVE algorithm (SImple and Efficent VEsion).
/// It is scan-resistant and has very low overhead.
#[derive(Debug)]
pub struct SievePolicy<K> {
  state: Mutex<SieveState<K>>,
}

impl<K> SievePolicy<K> {
  pub fn new() -> Self {
    Self {
      state: Mutex::new(SieveState {
        order: VecDeque::new(),
        items: HashMap::new(),
        hand: 0,
      }),
    }
  }
}

impl<K, V> CachePolicy<K, V> for SievePolicy<K>
where
  K: Eq + Hash + Clone + Send + Sync,
  V: Send + Sync,
{
  /// On access, simply mark the item's `visited` flag as true.
  fn on_access(&self, key: &K, _cost: u64) {
    let mut state = self.state.lock();
    if let Some(entry) = state.items.get_mut(key) {
      entry.visited = true;
    }
  }

  /// On insert, add the new item to the front of the queue with `visited = false`.
  fn on_admit(&self, key: &K, cost: u64) -> AdmissionDecision<K> {
    let mut state = self.state.lock();

    // A key can be re-inserted, so we remove any old instance first.
    if state.items.contains_key(key) {
      state.order.retain(|k| k != key);
    }

    state.items.insert(
      key.clone(),
      SieveEntry {
        cost,
        visited: false,
      },
    );
    state.order.push_front(key.clone());

    AdmissionDecision::Admit
  }

  /// When an item is removed, stop tracking it.
  fn on_remove(&self, key: &K) {
    let mut state = self.state.lock();
    state.items.remove(key);
    state.order.retain(|k| k != key);

    // Reset the hand if it pointed past the end of the list.
    if state.hand >= state.order.len() {
      state.hand = 0;
    }
  }

  /// Evict items using the SIEVE "hand" algorithm.
  fn evict(&self, mut cost_to_free: u64) -> (Vec<K>, u64) {
    let mut victims = Vec::new();
    let mut total_cost_freed = 0;
    let mut state = self.state.lock();

    while cost_to_free > 0 && !state.order.is_empty() {
      let mut found_victim = false;
      while state.hand < state.order.len() {
        let index = state.order.len() - 1 - state.hand;
        // Clone so we hold no reference into `state.order` while mutably
        // accessing `state.items` or removing from `state.order`.
        let key = state.order[index].clone();
        let visited = state.items.get(&key).map_or(false, |e| e.visited);

        if !visited {
          state.order.remove(index);
          let cost = state.items.remove(&key).unwrap().cost;

          cost_to_free = cost_to_free.saturating_sub(cost);
          total_cost_freed += cost;
          victims.push(key);

          found_victim = true;
          break;
        } else {
          state.items.get_mut(&key).unwrap().visited = false;
          state.hand += 1;
        }
      }

      // If we swept the entire cache with no victim (all visited), reset hand.
      if !found_victim {
        state.hand = 0;
      }

      // If still need to evict after a full sweep, evict the oldest item directly.
      if !found_victim && cost_to_free > 0 && !state.order.is_empty() {
        let lru_index = state.order.len() - 1;
        let key_to_evict = state.order.remove(lru_index).unwrap();
        let cost = state.items.remove(&key_to_evict).unwrap().cost;
        cost_to_free = cost_to_free.saturating_sub(cost);
        total_cost_freed += cost;
        victims.push(key_to_evict);
        state.hand = 0;
      }
    }

    (victims, total_cost_freed)
  }

  /// Clear all internal tracking.
  fn clear(&self) {
    let mut state = self.state.lock();
    state.order.clear();
    state.items.clear();
    state.hand = 0;
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn test_new_item_admitted() {
    let policy = SievePolicy::<i32>::new();
    <SievePolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &1, 1);
    <SievePolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &2, 1);

    let state = policy.state.lock();

    assert!(state.items.contains_key(&1));
    assert!(state.items.contains_key(&2));
    assert!(
      !state.items.get(&1).unwrap().visited,
      "New item should not be visited"
    );
    assert!(!state.items.get(&2).unwrap().visited);
    // Newest items are at the front of the VecDeque
    assert_eq!(state.order, vec![2, 1]);
  }

  #[test]
  fn test_access_sets_visited_bit() {
    let policy = SievePolicy::<i32>::new();
    <SievePolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &1, 1);
    assert!(!policy.state.lock().items.get(&1).unwrap().visited);

    <SievePolicy<i32> as CachePolicy<i32, ()>>::on_access(&policy, &1, 1);

    assert!(
      policy.state.lock().items.get(&1).unwrap().visited,
      "Accessed item's visited bit should be true"
    );
    assert_eq!(policy.state.lock().order, vec![1]);
  }

  #[test]
  fn test_re_admit_moves_to_front() {
    let policy = SievePolicy::<i32>::new();
    <SievePolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &1, 1);
    <SievePolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &2, 1);
    assert_eq!(policy.state.lock().order, vec![2, 1]);

    <SievePolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &1, 1);
    assert_eq!(policy.state.lock().order, vec![1, 2]);
  }

  #[test]
  fn test_evict_unvisited_item() {
    let policy = SievePolicy::<i32>::new();
    <SievePolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &1, 1);
    <SievePolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &2, 1);

    let (victims, cost_freed) = <SievePolicy<i32> as CachePolicy<i32, ()>>::evict(&policy, 1);

    assert_eq!(victims, vec![1]);
    assert_eq!(cost_freed, 1);
    assert!(!policy.state.lock().items.contains_key(&1));
    assert_eq!(policy.state.lock().order, vec![2]);
    assert_eq!(
      policy.state.lock().hand,
      0,
      "Hand should not advance after eviction"
    );
  }

  #[test]
  fn test_evict_spares_visited_item() {
    let policy = SievePolicy::<i32>::new();
    <SievePolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &1, 1);
    <SievePolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &2, 1);

    <SievePolicy<i32> as CachePolicy<i32, ()>>::on_access(&policy, &1, 1);

    let (victims, cost_freed) = <SievePolicy<i32> as CachePolicy<i32, ()>>::evict(&policy, 1);

    assert_eq!(victims, vec![2]);
    assert_eq!(cost_freed, 1);

    let state = policy.state.lock();
    assert!(state.items.contains_key(&1));
    assert!(
      !state.items.get(&1).unwrap().visited,
      "Item 1's visited bit should be cleared"
    );
    assert_eq!(state.hand, 1, "Hand should have advanced past item 1");
  }

  #[test]
  fn test_evict_resets_hand_after_full_sweep() {
    let policy = SievePolicy::<i32>::new();
    <SievePolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &1, 1);
    <SievePolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &2, 1);

    <SievePolicy<i32> as CachePolicy<i32, ()>>::on_access(&policy, &1, 1);
    <SievePolicy<i32> as CachePolicy<i32, ()>>::on_access(&policy, &2, 1);

    let (victims, cost_freed) = <SievePolicy<i32> as CachePolicy<i32, ()>>::evict(&policy, 0);
    assert!(victims.is_empty());
    assert_eq!(
      policy.state.lock().hand,
      0,
      "Hand should reset after a full sweep with no evictions"
    );

    let (victims2, _) = <SievePolicy<i32> as CachePolicy<i32, ()>>::evict(&policy, 1);
    assert_eq!(victims2, vec![1]);
  }

  #[test]
  fn test_on_remove_cleans_up_state() {
    let policy = SievePolicy::<i32>::new();
    <SievePolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &1, 1);
    <SievePolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &2, 1);

    <SievePolicy<i32> as CachePolicy<i32, ()>>::on_remove(&policy, &1);

    let state = policy.state.lock();
    assert!(!state.items.contains_key(&1));
    assert_eq!(state.order, vec![2]);
  }

  #[test]
  fn test_clear_resets_state() {
    let policy = SievePolicy::<i32>::new();
    <SievePolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &1, 1);
    policy.state.lock().hand = 5;

    <SievePolicy<i32> as CachePolicy<i32, ()>>::clear(&policy);

    assert!(policy.state.lock().items.is_empty());
    assert!(policy.state.lock().order.is_empty());
    assert_eq!(policy.state.lock().hand, 0);
  }
}
