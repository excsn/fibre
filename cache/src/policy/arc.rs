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
  // This is called when the cache is full and a new item needs to be admitted.
  fn replace(
    &self,
    p_guard: &mut u64,
    t1_guard: &mut LruList<K>,
    key_in_b2: bool,
  ) -> Option<(K, u64)> {
    let t1_cost = t1_guard.current_total_cost();
    if t1_cost > 0 && (t1_cost >= *p_guard || (key_in_b2 && t1_cost == *p_guard)) {
      if let Some((key, cost)) = t1_guard.pop_back() {
        let mut b1_guard = self.b1.lock();
        b1_guard.push_front(key.clone(), cost);
        if b1_guard.current_total_cost() > self.capacity {
          b1_guard.pop_back();
        }
        return Some((key, cost));
      }
    } else {
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
    // If it's in T1, promote it to T2.
    if t1_guard.remove(key).is_some() {
      drop(t1_guard); // Release lock on T1
      self.t2.lock().push_front(key.clone(), cost);
      return;
    }
    drop(t1_guard);

    // If it's in T2, just refresh it.
    let mut t2_guard = self.t2.lock();
    if t2_guard.contains(key) {
      t2_guard.push_front(key.clone(), cost);
    }
  }

  fn on_admit(&self, key: &K, cost: u64) -> AdmissionDecision<K> {
    // Handle re-admission of an existing key directly.
    // This is an update, so we treat it as a high-frequency access
    // by moving/inserting it directly into T2.
    {
      let mut t1 = self.t1.lock();
      if t1.remove(key).is_some() {
        drop(t1);
        self.t2.lock().push_front(key.clone(), cost);
        return AdmissionDecision::Admit;
      }
      let mut t2 = self.t2.lock();
      if t2.contains(key) {
        t2.push_front(key.clone(), cost);
        return AdmissionDecision::Admit;
      }
    } // Drop locks

    let mut p = self.p.lock();
    let mut key_in_b2 = false;

    // Check ghost lists
    {
      let mut b1 = self.b1.lock();
      if b1.remove(key).is_some() {
        let b2_cost = self.b2.lock().current_total_cost();
        let b1_cost = b1.current_total_cost();
        let delta = if b1_cost > 0 && b2_cost > b1_cost {
          (b2_cost as f64 / b1_cost as f64).round() as u64
        } else {
          1
        }
        .max(1);
        *p = (*p + delta).min(self.capacity);
      } else {
        drop(b1);
        let mut b2 = self.b2.lock();
        if b2.remove(key).is_some() {
          key_in_b2 = true;
          let b1_cost = self.b1.lock().current_total_cost();
          let b2_cost = b2.current_total_cost();
          let delta = if b2_cost > 0 && b1_cost > b2_cost {
            (b1_cost as f64 / b2_cost as f64).round() as u64
          } else {
            1
          }
          .max(1);
          *p = p.saturating_sub(delta);
        }
      }
    } // Drop ghost locks

    let mut t1 = self.t1.lock();
    let t2_cost = self.t2.lock().current_total_cost();
    if t1.current_total_cost() + t2_cost >= self.capacity {
      self.replace(&mut p, &mut t1, key_in_b2);
    }

    // Finally, insert the new item into T1.
    t1.push_front(key.clone(), cost);

    AdmissionDecision::Admit
  }

  fn on_remove(&self, key: &K) {
    if self.t1.lock().remove(key).is_some() {
      return;
    }
    if self.t2.lock().remove(key).is_some() {
      return;
    }
    if self.b1.lock().remove(key).is_some() {
      return;
    }
    self.b2.lock().remove(key);
  }

  fn evict(&self, cost_to_free: u64) -> (Vec<K>, u64) {
    let mut victims = Vec::new();
    let mut total_cost_freed = 0;
    while total_cost_freed < cost_to_free {
      let mut t1 = self.t1.lock();
      let key_in_b2 = t1
        .tail
        .map(|idx| self.b2.lock().contains(&t1.nodes[idx].key))
        .unwrap_or(false);

      let mut p = self.p.lock();
      if let Some((key, cost)) = self.replace(&mut p, &mut t1, key_in_b2) {
        total_cost_freed += cost;
        victims.push(key);
      } else {
        break; // No more items to evict
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

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn test_new_item_goes_to_t1() {
    let policy = ArcPolicy::<i32>::new(10);
    <ArcPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &1, 1);
    assert!(policy.t1.lock().contains(&1));
    assert!(!policy.t2.lock().contains(&1));
    assert_eq!(policy.t1.lock().current_total_cost(), 1);
  }

  #[test]
  fn test_access_promotes_from_t1_to_t2() {
    let policy = ArcPolicy::<i32>::new(10);
    <ArcPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &1, 1); // Goes to T1
    <ArcPolicy<i32> as CachePolicy<i32, ()>>::on_access(&policy, &1, 1); // Promoted to T2

    assert!(!policy.t1.lock().contains(&1));
    assert!(policy.t2.lock().contains(&1));
    assert_eq!(policy.t2.lock().current_total_cost(), 1);
  }

  #[test]
  fn test_re_admit_acts_as_access() {
    let policy = ArcPolicy::<i32>::new(10);
    <ArcPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &1, 1); // Goes to T1
    <ArcPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &1, 2); // Re-admit with new cost, should act like access and promote

    assert!(!policy.t1.lock().contains(&1));
    assert!(policy.t2.lock().contains(&1));
    assert_eq!(
      policy.t2.lock().current_total_cost(),
      2,
      "Cost should be updated"
    );
  }

  #[test]
  fn test_eviction_from_t1() {
    let policy = ArcPolicy::<i32>::new(2);
    <ArcPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &1, 1);
    <ArcPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &2, 1);

    // T1 is now [2, 1]. p is 0. T1 cost (2) > p (0).
    // Admitting 3 should evict from T1.
    <ArcPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &3, 1);

    // LRU of T1 (key 1) should be evicted and moved to B1
    assert!(!policy.t1.lock().contains(&1));
    assert!(policy.t1.lock().contains(&2)); // 2 remains
    assert!(policy.t1.lock().contains(&3)); // 3 is added
    assert!(
      policy.b1.lock().contains(&1),
      "Evicted key should be in B1 ghost list"
    );
  }

  #[test]
  fn test_eviction_from_t2() {
    let policy = ArcPolicy::<i32>::new(2);
    // Promote 1 and 2 to T2
    <ArcPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &1, 1);
    <ArcPolicy<i32> as CachePolicy<i32, ()>>::on_access(&policy, &1, 1);
    <ArcPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &2, 1);
    <ArcPolicy<i32> as CachePolicy<i32, ()>>::on_access(&policy, &2, 1);

    // T1 is empty. T2 is [2, 1]. p is 0. T1 cost (0) == p (0).
    // Admitting 3 should evict from T2.
    <ArcPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &3, 1);

    // LRU of T2 (key 1) should be evicted and moved to B2
    assert!(!policy.t2.lock().contains(&1));
    assert!(policy.t2.lock().contains(&2)); // 2 remains
    assert!(policy.t1.lock().contains(&3)); // 3 is added to t1
    assert!(
      policy.b2.lock().contains(&1),
      "Evicted key should be in B2 ghost list"
    );
  }

  #[test]
  fn test_adaptive_p_grows_on_b1_hit() {
    let policy = ArcPolicy::<i32>::new(2);
    <ArcPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &1, 1);
    <ArcPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &2, 1);
    // T1 is [2, 1]. Evict 1 to B1.
    <ArcPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &3, 1);
    assert_eq!(*policy.p.lock(), 0);
    assert!(policy.b1.lock().contains(&1));

    // Now, a hit on ghost item 1 (from B1)
    <ArcPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &1, 1);

    // p should have grown. delta = max(1, b2/b1) = max(1, 0/0) -> 1. So p becomes 1.
    assert_eq!(*policy.p.lock(), 1, "p should increase after a B1 hit");
    assert!(
      !policy.b1.lock().contains(&1),
      "Ghost item should be consumed"
    );
  }

  #[test]
  fn test_adaptive_p_shrinks_on_b2_hit() {
    let policy = ArcPolicy::<i32>::new(2);
    // Promote 1 and 2 to T2
    <ArcPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &1, 1);
    <ArcPolicy<i32> as CachePolicy<i32, ()>>::on_access(&policy, &1, 1);
    <ArcPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &2, 1);
    <ArcPolicy<i32> as CachePolicy<i32, ()>>::on_access(&policy, &2, 1);
    *policy.p.lock() = 1; // Manually set p > 0

    // Evict 1 from T2 to B2
    <ArcPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &3, 1);
    assert_eq!(*policy.p.lock(), 1);
    assert!(policy.b2.lock().contains(&1));

    // Now, a hit on ghost item 1 (from B2)
    <ArcPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &1, 1);

    // p should have shrunk. delta = max(1, b1/b2) = max(1, 1/0) -> 1. So p becomes 0.
    assert_eq!(*policy.p.lock(), 0, "p should decrease after a B2 hit");
    assert!(
      !policy.b2.lock().contains(&1),
      "Ghost item should be consumed"
    );
  }

  #[test]
  fn test_on_remove_cleans_up_all_lists() {
    let policy = ArcPolicy::<i32>::new(10); // Use large capacity to avoid evictions

    // Manually populate the lists to test `on_remove` in isolation.
    // 1. Put 1 in T1
    policy.t1.lock().push_front(1, 1);
    // 2. Put 2 in T2
    policy.t2.lock().push_front(2, 1);
    // 3. Put 3 in B1
    policy.b1.lock().push_front(3, 1);
    // 4. Put 4 in B2
    policy.b2.lock().push_front(4, 1);

    // Verify initial state
    assert!(policy.t1.lock().contains(&1));
    assert!(policy.t2.lock().contains(&2));
    assert!(policy.b1.lock().contains(&3));
    assert!(policy.b2.lock().contains(&4));

    // Remove from each list and assert
    <ArcPolicy<i32> as CachePolicy<i32, ()>>::on_remove(&policy, &1); // Remove from T1
    assert!(!policy.t1.lock().contains(&1));

    <ArcPolicy<i32> as CachePolicy<i32, ()>>::on_remove(&policy, &2); // Remove from T2
    assert!(!policy.t2.lock().contains(&2));

    <ArcPolicy<i32> as CachePolicy<i32, ()>>::on_remove(&policy, &3); // Remove from B1
    assert!(!policy.b1.lock().contains(&3));

    <ArcPolicy<i32> as CachePolicy<i32, ()>>::on_remove(&policy, &4); // Remove from B2
    assert!(!policy.b2.lock().contains(&4));
  }

  #[test]
  fn test_evict_frees_enough_cost() {
    let policy = ArcPolicy::<i32>::new(5);
    for i in 1..=5 {
      <ArcPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &i, 1);
    }
    assert_eq!(policy.t1.lock().current_total_cost(), 5);

    let (victims, cost_freed) = <ArcPolicy<i32> as CachePolicy<i32, ()>>::evict(&policy, 3);

    assert_eq!(cost_freed, 3, "Should free exactly 3 cost");
    assert_eq!(victims.len(), 3);
    assert_eq!(victims, vec![1, 2, 3], "Should evict LRU items from T1");

    assert_eq!(policy.t1.lock().current_total_cost(), 2);
    assert!(policy.t1.lock().contains(&4));
    assert!(policy.t1.lock().contains(&5));
  }

  #[test]
  fn test_clear_empties_all_lists() {
    let policy = ArcPolicy::<i32>::new(4);
    <ArcPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &1, 1); // T1
    <ArcPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &2, 1);
    <ArcPolicy<i32> as CachePolicy<i32, ()>>::on_access(&policy, &2, 1); // T2
    <ArcPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &3, 1);
    <ArcPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &4, 1); // Evicts 1 to B1

    assert!(
      *policy.p.lock() > 0
        || policy.t1.lock().current_total_cost() > 0
        || policy.t2.lock().current_total_cost() > 0
        || policy.b1.lock().current_total_cost() > 0
    );

    <ArcPolicy<i32> as CachePolicy<i32, ()>>::clear(&policy);

    assert_eq!(*policy.p.lock(), 0);
    assert_eq!(policy.t1.lock().current_total_cost(), 0);
    assert_eq!(policy.t2.lock().current_total_cost(), 0);
    assert_eq!(policy.b1.lock().current_total_cost(), 0);
    assert_eq!(policy.b2.lock().current_total_cost(), 0);
  }
}
