use super::CachePolicy;
use crate::policy::lru_list::LruList;
use crate::policy::AdmissionDecision;

use parking_lot::Mutex;
use std::hash::Hash;

/// The unified mutable state of the ARC algorithm, held under a single lock.
#[derive(Debug)]
struct ArcState<K: Eq + Hash + Clone> {
  // `p` is the target cost of the T1 (recency) list.
  p: u64,
  // T1: Recently used, seen once. "Top 1"
  t1: LruList<K>,
  // T2: Frequently used, seen at least twice. "Top 2"
  t2: LruList<K>,
  // B1: Ghost list of recent evictees from T1. "Bottom 1"
  b1: LruList<K>,
  // B2: Ghost list of recent evictees from T2. "Bottom 2"
  b2: LruList<K>,
}

impl<K: Eq + Hash + Clone> ArcState<K> {
  fn new() -> Self {
    Self {
      p: 0,
      t1: LruList::new(),
      t2: LruList::new(),
      b1: LruList::new(),
      b2: LruList::new(),
    }
  }

  // Core logic to select and evict a victim, moving it to a ghost list.
  // Called when the cache is full and a new item needs to be admitted.
  fn replace(&mut self, capacity: u64, key_in_b2: bool) -> Option<(K, u64)> {
    let t1_cost = self.t1.current_total_cost();
    if t1_cost > 0 && (t1_cost >= self.p || (key_in_b2 && t1_cost == self.p)) {
      if let Some((key, cost)) = self.t1.pop_back() {
        self.b1.push_front(key.clone(), cost);
        if self.b1.current_total_cost() > capacity {
          self.b1.pop_back();
        }
        return Some((key, cost));
      }
    } else if let Some((key, cost)) = self.t2.pop_back() {
      self.b2.push_front(key.clone(), cost);
      if self.b2.current_total_cost() > capacity {
        self.b2.pop_back();
      }
      return Some((key, cost));
    }
    None
  }
}

/// An eviction policy based on the Adaptive Replacement Cache (ARC) algorithm.
#[derive(Debug)]
pub struct ArcPolicy<K: Eq + Hash + Clone> {
  state: Mutex<ArcState<K>>,
  capacity: u64,
}

impl<K: Eq + Hash + Clone> ArcPolicy<K> {
  pub fn new(capacity: usize) -> Self {
    Self {
      state: Mutex::new(ArcState::new()),
      capacity: capacity as u64,
    }
  }
}

impl<K, V> CachePolicy<K, V> for ArcPolicy<K>
where
  K: Eq + Hash + Clone + Send + Sync,
  V: Send + Sync,
{
  fn on_access(&self, key: &K, cost: u64) {
    let mut state = self.state.lock();
    // If it's in T1, promote it to T2.
    if state.t1.remove(key).is_some() {
      state.t2.push_front(key.clone(), cost);
      return;
    }
    // If it's in T2, just refresh it.
    if state.t2.contains(key) {
      state.t2.push_front(key.clone(), cost);
    }
  }

  fn on_admit(&self, key: &K, cost: u64) -> AdmissionDecision<K> {
    let mut state = self.state.lock();

    // Handle re-admission of an existing key — treat as a high-frequency access.
    if state.t1.remove(key).is_some() {
      state.t2.push_front(key.clone(), cost);
      return AdmissionDecision::Admit;
    }
    if state.t2.contains(key) {
      state.t2.push_front(key.clone(), cost);
      return AdmissionDecision::Admit;
    }

    let mut key_in_b2 = false;

    // Check ghost lists and adapt `p`.
    if state.b1.remove(key).is_some() {
      let b2_cost = state.b2.current_total_cost();
      let b1_cost = state.b1.current_total_cost();
      let delta = if b1_cost > 0 && b2_cost > b1_cost {
        (b2_cost as f64 / b1_cost as f64).round() as u64
      } else {
        1
      }
      .max(1);
      state.p = (state.p + delta).min(self.capacity);
    } else if state.b2.remove(key).is_some() {
      key_in_b2 = true;
      let b1_cost = state.b1.current_total_cost();
      let b2_cost = state.b2.current_total_cost();
      let delta = if b2_cost > 0 && b1_cost > b2_cost {
        (b1_cost as f64 / b2_cost as f64).round() as u64
      } else {
        1
      }
      .max(1);
      state.p = state.p.saturating_sub(delta);
    }

    let t2_cost = state.t2.current_total_cost();
    if state.t1.current_total_cost() + t2_cost >= self.capacity {
      state.replace(self.capacity, key_in_b2);
    }

    // Insert the new item into T1.
    state.t1.push_front(key.clone(), cost);

    AdmissionDecision::Admit
  }

  fn on_remove(&self, key: &K) {
    let mut state = self.state.lock();
    if state.t1.remove(key).is_some() {
      return;
    }
    if state.t2.remove(key).is_some() {
      return;
    }
    if state.b1.remove(key).is_some() {
      return;
    }
    state.b2.remove(key);
  }

  fn evict(&self, cost_to_free: u64) -> (Vec<K>, u64) {
    let mut state = self.state.lock();
    let mut victims = Vec::new();
    let mut total_cost_freed = 0;
    while total_cost_freed < cost_to_free {
      let tail_key = state.t1.tail.map(|idx| state.t1.nodes[idx].key.clone());
      let key_in_b2 = tail_key.map_or(false, |k| state.b2.contains(&k));

      if let Some((key, cost)) = state.replace(self.capacity, key_in_b2) {
        total_cost_freed += cost;
        victims.push(key);
      } else {
        break;
      }
    }
    (victims, total_cost_freed)
  }

  fn clear(&self) {
    let mut state = self.state.lock();
    state.p = 0;
    state.t1.clear();
    state.t2.clear();
    state.b1.clear();
    state.b2.clear();
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn test_new_item_goes_to_t1() {
    let policy = ArcPolicy::<i32>::new(10);
    <ArcPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &1, 1);
    assert!(policy.state.lock().t1.contains(&1));
    assert!(!policy.state.lock().t2.contains(&1));
    assert_eq!(policy.state.lock().t1.current_total_cost(), 1);
  }

  #[test]
  fn test_access_promotes_from_t1_to_t2() {
    let policy = ArcPolicy::<i32>::new(10);
    <ArcPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &1, 1); // Goes to T1
    <ArcPolicy<i32> as CachePolicy<i32, ()>>::on_access(&policy, &1, 1); // Promoted to T2

    assert!(!policy.state.lock().t1.contains(&1));
    assert!(policy.state.lock().t2.contains(&1));
    assert_eq!(policy.state.lock().t2.current_total_cost(), 1);
  }

  #[test]
  fn test_re_admit_acts_as_access() {
    let policy = ArcPolicy::<i32>::new(10);
    <ArcPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &1, 1); // Goes to T1
    <ArcPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &1, 2); // Re-admit with new cost, should act like access and promote

    assert!(!policy.state.lock().t1.contains(&1));
    assert!(policy.state.lock().t2.contains(&1));
    assert_eq!(
      policy.state.lock().t2.current_total_cost(),
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
    assert!(!policy.state.lock().t1.contains(&1));
    assert!(policy.state.lock().t1.contains(&2)); // 2 remains
    assert!(policy.state.lock().t1.contains(&3)); // 3 is added
    assert!(
      policy.state.lock().b1.contains(&1),
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
    assert!(!policy.state.lock().t2.contains(&1));
    assert!(policy.state.lock().t2.contains(&2)); // 2 remains
    assert!(policy.state.lock().t1.contains(&3)); // 3 is added to t1
    assert!(
      policy.state.lock().b2.contains(&1),
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
    assert_eq!(policy.state.lock().p, 0);
    assert!(policy.state.lock().b1.contains(&1));

    // Now, a hit on ghost item 1 (from B1)
    <ArcPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &1, 1);

    // p should have grown. delta = max(1, b2/b1) = max(1, 0/0) -> 1. So p becomes 1.
    assert_eq!(policy.state.lock().p, 1, "p should increase after a B1 hit");
    assert!(
      !policy.state.lock().b1.contains(&1),
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
    policy.state.lock().p = 1; // Manually set p > 0

    // Evict 1 from T2 to B2
    <ArcPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &3, 1);
    assert_eq!(policy.state.lock().p, 1);
    assert!(policy.state.lock().b2.contains(&1));

    // Now, a hit on ghost item 1 (from B2)
    <ArcPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &1, 1);

    // p should have shrunk. delta = max(1, b1/b2) = max(1, 1/0) -> 1. So p becomes 0.
    assert_eq!(policy.state.lock().p, 0, "p should decrease after a B2 hit");
    assert!(
      !policy.state.lock().b2.contains(&1),
      "Ghost item should be consumed"
    );
  }

  #[test]
  fn test_on_remove_cleans_up_all_lists() {
    let policy = ArcPolicy::<i32>::new(10); // Use large capacity to avoid evictions

    // Manually populate the lists to test `on_remove` in isolation.
    policy.state.lock().t1.push_front(1, 1);
    policy.state.lock().t2.push_front(2, 1);
    policy.state.lock().b1.push_front(3, 1);
    policy.state.lock().b2.push_front(4, 1);

    // Verify initial state
    assert!(policy.state.lock().t1.contains(&1));
    assert!(policy.state.lock().t2.contains(&2));
    assert!(policy.state.lock().b1.contains(&3));
    assert!(policy.state.lock().b2.contains(&4));

    // Remove from each list and assert
    <ArcPolicy<i32> as CachePolicy<i32, ()>>::on_remove(&policy, &1); // Remove from T1
    assert!(!policy.state.lock().t1.contains(&1));

    <ArcPolicy<i32> as CachePolicy<i32, ()>>::on_remove(&policy, &2); // Remove from T2
    assert!(!policy.state.lock().t2.contains(&2));

    <ArcPolicy<i32> as CachePolicy<i32, ()>>::on_remove(&policy, &3); // Remove from B1
    assert!(!policy.state.lock().b1.contains(&3));

    <ArcPolicy<i32> as CachePolicy<i32, ()>>::on_remove(&policy, &4); // Remove from B2
    assert!(!policy.state.lock().b2.contains(&4));
  }

  #[test]
  fn test_evict_frees_enough_cost() {
    let policy = ArcPolicy::<i32>::new(5);
    for i in 1..=5 {
      <ArcPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &i, 1);
    }
    assert_eq!(policy.state.lock().t1.current_total_cost(), 5);

    let (victims, cost_freed) = <ArcPolicy<i32> as CachePolicy<i32, ()>>::evict(&policy, 3);

    assert_eq!(cost_freed, 3, "Should free exactly 3 cost");
    assert_eq!(victims.len(), 3);
    assert_eq!(victims, vec![1, 2, 3], "Should evict LRU items from T1");

    assert_eq!(policy.state.lock().t1.current_total_cost(), 2);
    assert!(policy.state.lock().t1.contains(&4));
    assert!(policy.state.lock().t1.contains(&5));
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
      policy.state.lock().p > 0
        || policy.state.lock().t1.current_total_cost() > 0
        || policy.state.lock().t2.current_total_cost() > 0
        || policy.state.lock().b1.current_total_cost() > 0
    );

    <ArcPolicy<i32> as CachePolicy<i32, ()>>::clear(&policy);

    assert_eq!(policy.state.lock().p, 0);
    assert_eq!(policy.state.lock().t1.current_total_cost(), 0);
    assert_eq!(policy.state.lock().t2.current_total_cost(), 0);
    assert_eq!(policy.state.lock().b1.current_total_cost(), 0);
    assert_eq!(policy.state.lock().b2.current_total_cost(), 0);
  }
}
