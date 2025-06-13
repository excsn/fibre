use crate::policy::{AdmissionDecision};

use super::{AccessInfo, CachePolicy};
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
  fn on_access(&self, info: &AccessInfo<K, V>) {
    let mut items = self.items.lock();
    if let Some(entry) = items.get_mut(info.key) {
      entry.referenced = true;
    }
  }

  /// On insert, add the new item to the clock.
  fn on_admit(&self, info: &AccessInfo<K, V>) -> AdmissionDecision<K> {
    let mut order = self.order.lock();
    let mut items = self.items.lock();

    if !items.contains_key(info.key) {
      items.insert(
        info.key.clone(),
        ClockEntry {
          cost: info.entry.cost(),
          referenced: false,
        },
      );
      order.push(info.key.clone());
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
