use crate::policy::AdmissionDecision;

use super::{AccessInfo, CachePolicy};

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
pub struct Sieve<K> {
  // A queue of keys ordered by insertion time (front is most recent).
  order: Mutex<VecDeque<K>>,
  // A map for quick O(1) lookups of an item's state.
  items: Mutex<HashMap<K, SieveEntry>>,
  // The "hand" pointer, indicating the next eviction candidate to check.
  hand: Mutex<usize>,
}

impl<K> Sieve<K> {
  pub fn new() -> Self {
    Self {
      order: Mutex::new(VecDeque::new()),
      items: Mutex::new(HashMap::new()),
      hand: Mutex::new(0),
    }
  }
}

impl<K, V> CachePolicy<K, V> for Sieve<K>
where
  K: Eq + Hash + Clone + Send + Sync,
  V: Send + Sync,
{
  
  /// On access, simply mark the item's `visited` flag as true.
  fn on_access(&self, info: &AccessInfo<K, V>) {
    let mut items = self.items.lock();
    if let Some(entry) = items.get_mut(info.key) {
      entry.visited = true;
    }
  }

  /// On insert, add the new item to the front of the queue with `visited = false`.
  fn on_admit(&self, info: &AccessInfo<K, V>) -> AdmissionDecision<K> {
    let mut order = self.order.lock();
    let mut items = self.items.lock();

    // A key can be re-inserted, so we remove any old instance first.
    if items.contains_key(info.key) {
      order.retain(|k| k != info.key);
    }

    items.insert(
      info.key.clone(),
      SieveEntry {
        cost: info.entry.cost(),
        visited: false,
      },
    );
    order.push_front(info.key.clone());

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
