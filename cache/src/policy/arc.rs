use super::CachePolicy;
use crate::policy::lru_list::LruList;
use crate::policy::AdmissionDecision;

use parking_lot::Mutex;
use std::hash::Hash;

/// An eviction policy based on the Adaptive Replacement Cache (ARC) algorithm.
#[derive(Debug)]
pub struct ArcPolicy<K: Eq + Hash + Clone> {
  // `p` is the target size (cost) of the T1 (recency) list.
  // The target size of T2 is `capacity - p`.
  p: Mutex<u64>,
  capacity: u64,

  // T1: Recently used, seen once. "Top 1"
  t1: Mutex<LruList<K>>,
  // T2: Frequently used, seen at least twice. "Top 2"
  t2: Mutex<LruList<K>>,
  // B1: Ghost list of recent evictees from T1. "Bottom 1"
  b1: Mutex<LruList<K>>,
  // B2: Ghost list of recent evictees from T2. "Bottom 2"
  b2: Mutex<LruList<K>>,
}

impl<K: Eq + Hash + Clone> ArcPolicy<K> {
  pub fn new(capacity: usize) -> Self {
    Self {
      p: Mutex::new(0),
      capacity: capacity as u64,
      t1: Mutex::new(LruList::new()),
      t2: Mutex::new(LruList::new()),
      b1: Mutex::new(LruList::new()),
      b2: Mutex::new(LruList::new()),
    }
  }

  // Core logic to select and evict a victim, moving it to a ghost list.
  fn replace(&self) -> Option<(K, u64)> {
    let p_guard = self.p.lock();
    let mut t1_guard = self.t1.lock();

    let t1_cost = t1_guard.current_total_cost();
    let lru_t1_is_in_b2 = t1_guard
      .tail
      .map(|idx| self.b2.lock().contains(&t1_guard.nodes[idx].key))
      .unwrap_or(false);

    if t1_cost > 0 && (t1_cost > *p_guard || (lru_t1_is_in_b2 && t1_cost == *p_guard)) {
      // Evict from T1, move it to the B1 ghost list.
      if let Some((key, cost)) = t1_guard.pop_back() {
        let mut b1_guard = self.b1.lock();
        b1_guard.push_front(key.clone(), cost);
        if b1_guard.current_total_cost() > self.capacity {
          b1_guard.pop_back();
        }
        return Some((key, cost));
      }
    } else {
      // Evict from T2, move it to the B2 ghost list.
      let mut t2_guard = self.t2.lock();
      if let Some((key, cost)) = t2_guard.pop_back() {
        let mut b2_guard = self.b2.lock();
        b2_guard.push_front(key.clone(), cost);
        if b2_guard.current_total_cost() > self.capacity {
          b2_guard.pop_back();
        }
        return Some((key, cost));
      }
    }
    None
  }
}

impl<K, V> CachePolicy<K, V> for ArcPolicy<K>
where
  K: Eq + Hash + Clone + Send + Sync,
  V: Send + Sync,
{
  fn on_access(&self, key: &K, cost: u64) {
    let mut t1_guard = self.t1.lock();
    let mut t2_guard = self.t2.lock();

    // Case 1: Hit in T1 (item seen once). Promote it to T2.
    if t1_guard.remove(key).is_some() {
      t2_guard.push_front(key.clone(), cost);
      return;
    }

    // Case 2: Hit in T2. Just move it to the front (most recently used).
    if t2_guard.contains(key) {
      t2_guard.move_to_front(key);
    }
  }

  fn on_admit(&self, key: &K, cost: u64) -> AdmissionDecision<K> {
    // --- ADMISSION ---
    // ARC always admits. The core logic happens here.

    let mut p_guard = self.p.lock();
    let mut t1_guard = self.t1.lock();
    let mut t2_guard = self.t2.lock();
    let mut b1_guard = self.b1.lock();
    let mut b2_guard = self.b2.lock();

    // Case 1: The key is in T1 or T2. This is a cache update, not a new insert.
    // This case should ideally be handled by `on_access` logic, but we check here too.
    if t1_guard.contains(key) || t2_guard.contains(key) {
      // It's an update, so just ensure it's in T2.
      if t1_guard.remove(key).is_some() {
        t2_guard.push_front(key.clone(), cost);
      }
      return AdmissionDecision::Admit;
    }

    // Case 2: The key is in the B1 ghost list (a "recency" miss).
    // This suggests T1's target size `p` should grow.
    if b1_guard.remove(key).is_some() {
      let b1_cost = b1_guard.current_total_cost();
      let b2_cost = b2_guard.current_total_cost();
      let delta = if b1_cost >= b2_cost {
        1
      } else {
        b2_cost / b1_cost
      };
      *p_guard = (*p_guard + delta).min(self.capacity);
      self.replace();
    }
    // Case 3: The key is in the B2 ghost list (a "frequency" miss).
    // This suggests T2's target size should grow (i.e., `p` should shrink).
    else if b2_guard.remove(key).is_some() {
      let b1_cost = b1_guard.current_total_cost();
      let b2_cost = b2_guard.current_total_cost();
      let delta = if b2_cost >= b1_cost {
        1
      } else {
        b1_cost / b2_cost
      };
      *p_guard = p_guard.saturating_sub(delta);
      self.replace();
    }

    // Case 4: A complete miss. The key is not in any list.
    // Check if the cache is full (T1 + T2).
    let l1_cost = t1_guard.current_total_cost();
    let l2_cost = t2_guard.current_total_cost();
    if l1_cost + l2_cost >= self.capacity {
      self.replace();
    }

    // Finally, insert the new item into T1 (the probationary recency list).
    t1_guard.push_front(key.clone(), cost);

    AdmissionDecision::Admit
  }

  fn on_remove(&self, key: &K) {
    if self.t1.lock().remove(key).is_some() {
      return;
    }
    self.t2.lock().remove(key);
  }

  fn evict(&self, mut cost_to_free: u64) -> (Vec<K>, u64) {
    let mut victims = Vec::new();
    let mut total_cost_freed = 0;
    while total_cost_freed < cost_to_free {
      if let Some((key, cost)) = self.replace() {
        total_cost_freed += cost;
        victims.push(key);
      } else {
        // No more items to evict
        break;
      }
    }
    (victims, total_cost_freed)
  }

  fn clear(&self) {
    *self.p.lock() = 0;
    self.t1.lock().clear();
    self.t2.lock().clear();
    self.b1.lock().clear();
    self.b2.lock().clear();
  }
}
