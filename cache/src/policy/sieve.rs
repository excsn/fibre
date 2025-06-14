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

/// An eviction policy based on the SIEVE algorithm (SImple and Efficent VEsion).
/// It is scan-resistant and has very low overhead.
#[derive(Debug)]
pub struct SievePolicy<K> {
  // A queue of keys ordered by insertion time (front is most recent).
  order: Mutex<VecDeque<K>>,
  // A map for quick O(1) lookups of an item's state.
  items: Mutex<HashMap<K, SieveEntry>>,
  // The "hand" pointer, indicating the next eviction candidate to check.
  hand: Mutex<usize>,
}

impl<K> SievePolicy<K> {
  pub fn new() -> Self {
    Self {
      order: Mutex::new(VecDeque::new()),
      items: Mutex::new(HashMap::new()),
      hand: Mutex::new(0),
    }
  }
}

impl<K, V> CachePolicy<K, V> for SievePolicy<K>
where
  K: Eq + Hash + Clone + Send + Sync,
  V: Send + Sync,
{
  /// On access, simply mark the item's `visited` flag as true.
  fn on_access(&self, key: &K, cost: u64) {
    let mut items = self.items.lock();
    if let Some(entry) = items.get_mut(key) {
      entry.visited = true;
    }
  }

  /// On insert, add the new item to the front of the queue with `visited = false`.
  fn on_admit(&self, key: &K, cost: u64) -> AdmissionDecision<K> {
    let mut order = self.order.lock();
    let mut items = self.items.lock();

    // A key can be re-inserted, so we remove any old instance first.
    if items.contains_key(key) {
      order.retain(|k| k != key);
    }

    items.insert(
      key.clone(),
      SieveEntry {
        cost,
        visited: false,
      },
    );
    order.push_front(key.clone());

    AdmissionDecision::Admit // SIEVE always admits new items.
  }

  /// When an item is removed, stop tracking it.
  fn on_remove(&self, key: &K) {
    let mut order = self.order.lock();
    let mut items = self.items.lock();

    items.remove(key);
    order.retain(|k| k != key);

    // Reset the hand if it pointed past the end of the list.
    let mut hand = self.hand.lock();
    if *hand >= order.len() {
      *hand = 0;
    }
  }

  /// Evict items using the SIEVE "hand" algorithm.
  fn evict(&self, mut cost_to_free: u64) -> (Vec<K>, u64) {
    let mut victims = Vec::new();
    let mut total_cost_freed = 0;
    let mut order = self.order.lock();
    let mut items = self.items.lock();
    let mut hand = self.hand.lock();

    while cost_to_free > 0 && !order.is_empty() {
      // The hand should always point to an object, from oldest to newest.
      // In our VecDeque, oldest is at the back (highest index).
      // Let's iterate from the back of the queue.

      let mut found_victim = false;
      // Start scanning from the current hand position towards the front.
      // The hand's position is relative to the *end* of the queue.
      while *hand < order.len() {
        let index = order.len() - 1 - *hand;
        let key = &order[index];

        if let Some(entry) = items.get_mut(key) {
          if !entry.visited {
            // This is our victim.
            let key_to_evict = order.remove(index).unwrap();
            let cost = items.remove(&key_to_evict).unwrap().cost;

            cost_to_free = cost_to_free.saturating_sub(cost);
            total_cost_freed += cost;
            victims.push(key_to_evict);

            // The hand position doesn't need to change, as the next item
            // to check has now shifted into the current hand's relative spot.
            found_victim = true;
            break;
          } else {
            // It was visited. Give it a second chance and move the hand.
            entry.visited = false;
            *hand += 1;
          }
        } else {
          // Should not happen if state is consistent. Move hand anyway.
          *hand += 1;
        }
      }

      // If we swept the entire cache and found no victim (all were visited),
      // reset the hand and evict the oldest item on the next pass.
      if !found_victim {
        *hand = 0;
      }

      // If the outer loop needs to continue, it will re-scan from the new hand position.
      // This ensures that if cost_to_free requires multiple evictions, we continue correctly.
      if !found_victim && cost_to_free > 0 && !order.is_empty() {
        // We swept everything and still need to evict. Evict the LRU item.
        let lru_index = order.len() - 1;
        let key_to_evict = order.remove(lru_index).unwrap();
        let cost = items.remove(&key_to_evict).unwrap().cost;
        cost_to_free = cost_to_free.saturating_sub(cost);
        total_cost_freed += cost;
        victims.push(key_to_evict);
        *hand = 0;
      }
    }

    (victims, total_cost_freed)
  }

  /// Clear all internal tracking.
  fn clear(&self) {
    self.order.lock().clear();
    self.items.lock().clear();
    *self.hand.lock() = 0;
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

    let items = policy.items.lock();
    let order = policy.order.lock();

    assert!(items.contains_key(&1));
    assert!(items.contains_key(&2));
    assert!(
      !items.get(&1).unwrap().visited,
      "New item should not be visited"
    );
    assert!(!items.get(&2).unwrap().visited);
    // Newest items are at the front of the VecDeque
    assert_eq!(*order, vec![2, 1]);
  }

  #[test]
  fn test_access_sets_visited_bit() {
    let policy = SievePolicy::<i32>::new();
    <SievePolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &1, 1);
    assert!(!policy.items.lock().get(&1).unwrap().visited);

    // Access the item
    <SievePolicy<i32> as CachePolicy<i32, ()>>::on_access(&policy, &1, 1);

    assert!(
      policy.items.lock().get(&1).unwrap().visited,
      "Accessed item's visited bit should be true"
    );
    // Order should not change on access
    assert_eq!(*policy.order.lock(), vec![1]);
  }

  #[test]
  fn test_re_admit_moves_to_front() {
    let policy = SievePolicy::<i32>::new();
    <SievePolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &1, 1);
    <SievePolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &2, 1);
    assert_eq!(*policy.order.lock(), vec![2, 1]);

    // Re-admitting an item moves it to the front (MRU)
    <SievePolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &1, 1);
    assert_eq!(*policy.order.lock(), vec![1, 2]);
  }

  #[test]
  fn test_evict_unvisited_item() {
    let policy = SievePolicy::<i32>::new();
    <SievePolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &1, 1); // Oldest, at the back of VecDeque
    <SievePolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &2, 1);

    // Hand starts at 0, scanning from the back (oldest). Item 1 is unvisited.
    let (victims, cost_freed) = <SievePolicy<i32> as CachePolicy<i32, ()>>::evict(&policy, 1);

    assert_eq!(victims, vec![1]);
    assert_eq!(cost_freed, 1);
    assert!(!policy.items.lock().contains_key(&1));
    assert_eq!(*policy.order.lock(), vec![2]);
    assert_eq!(
      *policy.hand.lock(),
      0,
      "Hand should not advance after eviction"
    );
  }

  #[test]
  fn test_evict_spares_visited_item() {
    let policy = SievePolicy::<i32>::new();
    <SievePolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &1, 1); // Oldest
    <SievePolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &2, 1);

    // Visit the oldest item, giving it a second chance
    <SievePolicy<i32> as CachePolicy<i32, ()>>::on_access(&policy, &1, 1);

    // Eviction process:
    // 1. Hand at 0, scans from back, finds item 1.
    // 2. Item 1 is visited. Its visited bit is flipped to false. Hand increments to 1.
    // 3. Hand at 1, scans from back, finds item 2.
    // 4. Item 2 is unvisited. It is evicted.
    let (victims, cost_freed) = <SievePolicy<i32> as CachePolicy<i32, ()>>::evict(&policy, 1);

    assert_eq!(victims, vec![2]);
    assert_eq!(cost_freed, 1);

    let items = policy.items.lock();
    assert!(items.contains_key(&1));
    assert!(
      !items.get(&1).unwrap().visited,
      "Item 1's visited bit should be cleared"
    );
    assert_eq!(
      *policy.hand.lock(),
      1,
      "Hand should have advanced past item 1"
    );
  }

  #[test]
  fn test_evict_resets_hand_after_full_sweep() {
    let policy = SievePolicy::<i32>::new();
    <SievePolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &1, 1);
    <SievePolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &2, 1);

    // Visit both items
    <SievePolicy<i32> as CachePolicy<i32, ()>>::on_access(&policy, &1, 1);
    <SievePolicy<i32> as CachePolicy<i32, ()>>::on_access(&policy, &2, 1);

    // First eviction call:
    // - Hand spares item 1, hand -> 1
    // - Hand spares item 2, hand -> 2
    // - Loop finishes, hand is reset to 0. No victim found.
    let (victims, cost_freed) = <SievePolicy<i32> as CachePolicy<i32, ()>>::evict(&policy, 0);
    assert!(victims.is_empty());
    assert_eq!(
      *policy.hand.lock(),
      0,
      "Hand should reset after a full sweep with no evictions"
    );

    // Second eviction call:
    // - Hand at 0, scans from back, finds item 1. It is now unvisited.
    // - Item 1 is evicted.
    let (victims2, _) = <SievePolicy<i32> as CachePolicy<i32, ()>>::evict(&policy, 1);
    assert_eq!(victims2, vec![1]);
  }

  #[test]
  fn test_on_remove_cleans_up_state() {
    let policy = SievePolicy::<i32>::new();
    <SievePolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &1, 1);
    <SievePolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &2, 1);

    <SievePolicy<i32> as CachePolicy<i32, ()>>::on_remove(&policy, &1);

    let items = policy.items.lock();
    let order = policy.order.lock();
    assert!(!items.contains_key(&1));
    assert_eq!(*order, vec![2]);
  }

  #[test]
  fn test_clear_resets_state() {
    let policy = SievePolicy::<i32>::new();
    <SievePolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &1, 1);
    *policy.hand.lock() = 5; // Set non-default hand

    <SievePolicy<i32> as CachePolicy<i32, ()>>::clear(&policy);

    assert!(policy.items.lock().is_empty());
    assert!(policy.order.lock().is_empty());
    assert_eq!(*policy.hand.lock(), 0);
  }
}
