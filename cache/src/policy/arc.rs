use crate::policy::AdmissionDecision;

use super::{AccessInfo, CachePolicy};

use parking_lot::Mutex;
use std::collections::{HashMap, VecDeque};
use std::hash::Hash;

// We can reuse the LruList helper from tiny_lfu.rs if we extract it,
// or just define a simple list with a lookup map here.
type LruList<K> = (VecDeque<K>, HashMap<K, u64>);

/// An eviction policy based on the Adaptive Replacement Cache (ARC) algorithm.
#[derive(Debug)]
pub struct ArcPolicy<K> {
  // `p` is the target size of the T1 (recency) list.
  // The target size of T2 is `capacity - p`.
  p: Mutex<usize>,
  capacity: usize,

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
      capacity,
      t1: Mutex::new((VecDeque::new(), HashMap::new())),
      t2: Mutex::new((VecDeque::new(), HashMap::new())),
      b1: Mutex::new((VecDeque::new(), HashMap::new())),
      b2: Mutex::new((VecDeque::new(), HashMap::new())),
    }
  }

  // Core logic to replace a victim when the cache is full.
  fn replace(&self, _cost: u64) {
    let p_guard = self.p.lock();
    let mut t1_guard = self.t1.lock();

    // Check if T1 has more items than its target size `p`.
    if !t1_guard.0.is_empty()
      && (t1_guard.0.len() > *p_guard
        || (self.b2.lock().1.contains_key(&t1_guard.0.back().unwrap())
          && t1_guard.0.len() == *p_guard))
    {
      // Evict from T1, move it to the B1 ghost list.
      if let Some(key) = t1_guard.0.pop_back() {
        if let Some(c) = t1_guard.1.remove(&key) {
          let mut b1_guard = self.b1.lock();
          b1_guard.1.insert(key.clone(), c);
          b1_guard.0.push_front(key);
        }
      }
    } else {
      // Evict from T2, move it to the B2 ghost list.
      let mut t2_guard = self.t2.lock();
      if let Some(key) = t2_guard.0.pop_back() {
        if let Some(c) = t2_guard.1.remove(&key) {
          let mut b2_guard = self.b2.lock();
          b2_guard.1.insert(key.clone(), c);
          b2_guard.0.push_front(key);
        }
      }
    }
  }
}

impl<K, V> CachePolicy<K, V> for ArcPolicy<K>
where
  K: Eq + Hash + Clone + Send + Sync,
  V: Send + Sync,
{
  fn on_access(&self, info: &AccessInfo<K, V>) {
    let mut t1_guard = self.t1.lock();
    let mut t2_guard = self.t2.lock();

    // Case 1: Hit in T1 (item seen once). Promote it to T2.
    if let Some(cost) = t1_guard.1.remove(info.key) {
      t1_guard.0.retain(|k| k != info.key);
      t2_guard.1.insert(info.key.clone(), cost);
      t2_guard.0.push_front(info.key.clone());
      return;
    }

    // Case 2: Hit in T2. Just move it to the front (most recently used).
    if t2_guard.1.contains_key(info.key) {
      t2_guard.0.retain(|k| k != info.key);
      t2_guard.0.push_front(info.key.clone());
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
    if t1_guard.1.contains_key(key) || t2_guard.1.contains_key(key) {
      // It's an update, so just ensure it's in T2.
      if let Some(c) = t1_guard.1.remove(key) {
        t1_guard.0.retain(|k| k != key);
        t2_guard.1.insert(key.clone(), c);
        t2_guard.0.push_front(key.clone());
      }
      return AdmissionDecision::Admit;
    }

    // Case 2: The key is in the B1 ghost list (a "recency" miss).
    // This suggests T1's target size `p` should grow.
    if let Some(_c) = b1_guard.1.remove(key) {
      b1_guard.0.retain(|k| k != key);
      let b1_len = b1_guard.0.len();
      let b2_len = b2_guard.0.len();
      let delta = if b1_len >= b2_len { 1 } else { b2_len / b1_len };
      *p_guard = (*p_guard + delta).min(self.capacity);
      self.replace(cost);
    }
    // Case 3: The key is in the B2 ghost list (a "frequency" miss).
    // This suggests T2's target size should grow (i.e., `p` should shrink).
    else if let Some(_c) = b2_guard.1.remove(key) {
      b2_guard.0.retain(|k| k != key);
      let b1_len = b1_guard.0.len();
      let b2_len = b2_guard.0.len();
      let delta = if b2_len >= b1_len { 1 } else { b1_len / b2_len };
      *p_guard = p_guard.saturating_sub(delta);
      self.replace(cost);
    }

    // Case 4: A complete miss. The key is not in any list.
    // Check if the cache is full (L1 + L2).
    let l1_len = t1_guard.1.len();
    let l2_len = t2_guard.1.len();
    if l1_len + l2_len >= self.capacity {
      self.replace(cost);
    }

    // Finally, insert the new item into T1 (the probationary recency list).
    t1_guard.1.insert(key.clone(), cost);
    t1_guard.0.push_front(key.clone());

    AdmissionDecision::Admit
  }

  fn on_remove(&self, key: &K) {
    if self.t1.lock().1.remove(key).is_some() {
      self.t1.lock().0.retain(|k| k != key);
    }
    if self.t2.lock().1.remove(key).is_some() {
      self.t2.lock().0.retain(|k| k != key);
    }
  }

  fn evict(&self, mut cost_to_free: u64) -> (Vec<K>, u64) {
    let victims = Vec::new();
    let total_cost_freed = 0;
    while cost_to_free > 0 {
      self.replace(1);
      cost_to_free = cost_to_free.saturating_sub(1);
    }
    (victims, total_cost_freed)
  }

  fn clear(&self) {
    *self.p.lock() = 0;
    self.t1.lock().0.clear();
    self.t1.lock().1.clear();
    self.t2.lock().0.clear();
    self.t2.lock().1.clear();
    self.b1.lock().0.clear();
    self.b1.lock().1.clear();
    self.b2.lock().0.clear();
    self.b2.lock().1.clear();
  }
}
