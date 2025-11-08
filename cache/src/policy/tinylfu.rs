use super::CachePolicy;
use super::slru::SlruPolicy;
use crate::policy::AdmissionDecision;
use crate::policy::lru_list::LruList;

use parking_lot::Mutex;
use std::hash::Hash;

/// A policy that implements the W-TinyLFU scheme described in the paper.
/// It composes a "window" LRU cache with a "main" SLRU cache.
#[derive(Debug)]
pub struct TinyLfuPolicy<K: Eq + Hash + Clone> {
  sketch: cms::CountMinSketch,
  // The Window Cache (LRU, no admission policy) - ~1% of capacity
  window: Mutex<LruList<K>>,
  // The Main Cache (SLRU Policy) - ~99% of capacity
  main: SlruPolicy<K>,
  window_target_cost: u64,
}

impl<K: Eq + Hash + Clone> TinyLfuPolicy<K> {
  pub fn new(total_cache_cost_capacity: u64) -> Self {
    let cms_reset_threshold = (total_cache_cost_capacity * 10).max(100);

    // Per the W-TinyLFU paper, the window is ~1% of the total cache.
    let window_target_cost = if total_cache_cost_capacity == 0 {
      0
    } else {
      ((total_cache_cost_capacity as f64 * 0.01).round() as u64).max(1)
    };

    // The main cache is the other 99%.
    let main_cache_cost = total_cache_cost_capacity.saturating_sub(window_target_cost);

    Self {
      sketch: cms::CountMinSketch::new(cms_reset_threshold as usize),
      window: Mutex::new(LruList::new()),
      main: SlruPolicy::new(main_cache_cost),
      window_target_cost,
    }
  }

  /// A helper function that handles the logic for an access/replacement.
  /// It increments the frequency sketch and updates the item's position
  /// in either the window or the main cache.
  fn access_internal(&self, key: &K, cost: u64) {
    self.sketch.increment(key);
    let mut window_guard = self.window.lock();
    if window_guard.contains(key) {
      window_guard.push_front(key.clone(), cost);
      return;
    }
    drop(window_guard);
    self.main.access_internal(key, cost);
  }
}

impl<K, V> CachePolicy<K, V> for TinyLfuPolicy<K>
where
  K: Eq + Hash + Clone + Send + Sync + 'static,
  V: Send + Sync + 'static,
{
  fn on_access(&self, key: &K, cost: u64) {
    self.access_internal(key, cost);
  }

  fn on_admit(&self, key: &K, cost: u64) -> AdmissionDecision<K> {
    self.sketch.increment(key);

    // If the item is already in the main cache, this is just an access/update.
    let is_in_main = {
      let prob_guard = self.main.probationary.lock();
      if prob_guard.contains(key) {
        true
      } else {
        drop(prob_guard);
        self.main.protected.lock().contains(key)
      }
    };
    if is_in_main {
      self.main.access_internal(key, cost);
      return AdmissionDecision::Admit;
    }

    // Otherwise, it's either new or in the window. Add it to the window.
    let mut window_guard = self.window.lock();
    window_guard.push_front(key.clone(), cost);

    // Now, manage the window if it's over capacity.
    let mut rejected_candidates = Vec::new();
    while window_guard.current_total_cost() > self.window_target_cost {
      let (candidate_key, candidate_cost) = match window_guard.pop_back() {
        Some(c) => c,
        None => break, // Should not happen if cost > 0
      };

      // Must drop the lock on the window before trying to lock the main cache.
      drop(window_guard);

      let main_victim_key_opt = self.main.peek_lru();
      let admit_candidate = main_victim_key_opt
        .as_ref()
        .map_or(true, |main_victim_key| {
          self.sketch.estimate(&candidate_key) >= self.sketch.estimate(main_victim_key)
        });

      if admit_candidate {
        // Promote candidate from window to main.
        self.main.admit_internal(candidate_key, candidate_cost);
      } else {
        // Candidate is not admitted to main. It is dropped from the policy.
        // This is an eviction *from the policy*, not necessarily the cache map yet.
        // The janitor may remove it from the map later if it's still present.
        rejected_candidates.push(candidate_key);
      }

      // Re-acquire lock for the next loop iteration.
      window_guard = self.window.lock();
    }

    if rejected_candidates.is_empty() {
      AdmissionDecision::Admit
    } else {
      // These candidates were rejected for promotion, so they should be evicted.
      AdmissionDecision::AdmitAndEvict(rejected_candidates)
    }
  }

  fn on_remove(&self, key: &K) {
    if self.window.lock().remove(key).is_some() {
      return;
    }
    <SlruPolicy<K> as CachePolicy<K, V>>::on_remove(&self.main, key);
  }

  fn evict(&self, cost_to_free: u64) -> (Vec<K>, u64) {
    if cost_to_free == 0 {
      return (Vec::new(), 0);
    }
    // The janitor calls this when the *overall cache* is over capacity.
    // The W-TinyLFU policy dictates that eviction pressure should be applied
    // to the main cache, not the window.
    let (main_victims, main_cost_freed) =
      <SlruPolicy<K> as CachePolicy<K, V>>::evict(&self.main, cost_to_free);

    (main_victims, main_cost_freed)
  }

  fn clear(&self) {
    self.window.lock().clear();
    <SlruPolicy<K> as CachePolicy<K, V>>::clear(&self.main);
    self.sketch.clear();
  }
}

mod cms {
  use std::hash::{BuildHasher, Hash, Hasher};
  use std::sync::atomic::{AtomicUsize, Ordering};

  #[derive(Debug)]
  pub(super) struct CountMinSketch {
    counters: Vec<Vec<AtomicUsize>>,
    hashers: Vec<ahash::RandomState>,
    increments: AtomicUsize,
    capacity: usize,
  }

  impl CountMinSketch {
    pub fn new(reset_threshold: usize) -> Self {
      const DEPTH: usize = 4;
      let width = (reset_threshold * 2 / DEPTH).max(256).next_power_of_two();
      let mut counters = Vec::with_capacity(DEPTH);
      for _ in 0..DEPTH {
        let mut row = Vec::with_capacity(width);
        for _ in 0..width {
          row.push(AtomicUsize::new(0));
        }
        counters.push(row);
      }
      let mut hashers = Vec::with_capacity(DEPTH);
      for _ in 0..DEPTH {
        hashers.push(ahash::RandomState::new());
      }
      Self {
        counters,
        hashers,
        increments: AtomicUsize::new(0),
        capacity: reset_threshold,
      }
    }

    pub fn increment<K: Hash>(&self, key: &K) {
      for i in 0..self.counters.len() {
        let mut hasher = self.hashers[i].build_hasher();
        key.hash(&mut hasher);
        let index = hasher.finish() as usize % self.counters[i].len();
        self.counters[i][index].fetch_add(1, Ordering::Relaxed);
      }
      let prev = self.increments.fetch_add(1, Ordering::Relaxed) + 1;
      if prev >= self.capacity {
        self.reset();
      }
    }

    pub fn estimate<K: Hash>(&self, key: &K) -> usize {
      let mut min_count = usize::MAX;
      for i in 0..self.counters.len() {
        let mut hasher = self.hashers[i].build_hasher();
        key.hash(&mut hasher);
        let index = hasher.finish() as usize % self.counters[i].len();
        min_count = min_count.min(self.counters[i][index].load(Ordering::Relaxed));
      }
      min_count
    }

    fn reset(&self) {
      self.increments.store(0, Ordering::Relaxed);
      for row in &self.counters {
        for counter in row {
          let current_val = counter.load(Ordering::Relaxed);
          counter.store(current_val / 2, Ordering::Relaxed);
        }
      }
    }

    pub fn clear(&self) {
      self.increments.store(0, Ordering::Relaxed);
      for row in &self.counters {
        for counter in row {
          counter.store(0, Ordering::Relaxed);
        }
      }
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn test_new_item_goes_to_window() {
    let policy: TinyLfuPolicy<i32> = TinyLfuPolicy::new(101);

    let decision = <TinyLfuPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &1, 1);

    assert!(matches!(decision, AdmissionDecision::Admit));
    assert!(
      policy.window.lock().contains(&1),
      "Item should be in window"
    );
    assert!(!policy.main.probationary.lock().contains(&1));
  }

  #[test]
  fn test_window_overflow_causes_rejection() {
    let policy: TinyLfuPolicy<i32> = TinyLfuPolicy::new(101); // window=1, main=100

    // 1. Prime the main cache with a high-frequency item (key 100).
    policy.main.admit_internal(100, 1);
    for _ in 0..5 {
      policy.sketch.increment(&100);
    }

    // 2. Insert a low-frequency item (key 1) into the window.
    <TinyLfuPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &1, 1);
    assert!(policy.window.lock().contains(&1));

    // 3. Insert another item to overflow the window. This makes item 1 a candidate.
    // Since freq(1)=1 is less than freq(100)=5, candidate 1 should be rejected.
    let decision = <TinyLfuPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &2, 1);

    if let AdmissionDecision::AdmitAndEvict(victims) = decision {
      assert_eq!(victims, vec![1], "Rejected candidate should be the victim");
    } else {
      panic!("Expected AdmitAndEvict, got {:?}", decision);
    }

    // Since item 1 (freq=1) is less frequent than item 100 (freq=5), it should be
    // rejected for promotion and simply dropped.
    assert!(policy.window.lock().contains(&2));
    assert!(!policy.window.lock().contains(&1));
    assert!(!policy.main.probationary.lock().contains(&1));
    assert!(!policy.main.protected.lock().contains(&1));
  }

  #[test]
  fn test_window_overflow_causes_admission() {
    let policy: TinyLfuPolicy<i32> = TinyLfuPolicy::new(101); // window=1, main=100

    // 1. Prime the main cache with a low-frequency item (key 100, freq=1).
    policy.main.admit_internal(100, 1);
    policy.sketch.increment(&100);

    // 2. Insert a higher-frequency item and get it into the window as a candidate.
    <TinyLfuPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &1, 1);
    for _ in 0..5 {
      policy.sketch.increment(&1);
    }

    // 3. Overflow the window.
    <TinyLfuPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &2, 1);

    // Since candidate 1 (freq=5) is more frequent than victim 100 (freq=1),
    // it should be admitted to the main cache's probationary segment.
    assert!(policy.main.probationary.lock().contains(&1));
  }

  #[test]
  fn test_admission_logic_rejects_infrequent_item() {
    // Setup: TinyLfu with window=1, main=100.
    let policy: TinyLfuPolicy<i32> = TinyLfuPolicy::new(101); // window_target_cost = 1

    // 1. Prime the main cache with a "hot victim" (key 100, freq=10).
    policy.main.admit_internal(100, 1);
    for _ in 0..10 {
      policy.sketch.increment(&100);
    }

    // 2. Insert a "cold candidate" (key 1) into the window.
    let _decision1 = <TinyLfuPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &1, 1);
    assert!(policy.window.lock().contains(&1));

    // 3. Insert another item (key 2) to overflow the window.
    // Candidate '1' (freq 1) will be compared to victim '100' (freq 10).
    // '1' should be rejected.
    let decision = <TinyLfuPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &2, 1);

    // 4. Assert the outcome from the decision.
    match decision {
      AdmissionDecision::AdmitAndEvict(victims) => {
        assert_eq!(
          victims,
          vec![1],
          "The cold candidate (1) should have been rejected and returned as a victim."
        );
      }
      other_decision => panic!(
        "Expected AdmitAndEvict with victim [1], got {:?}",
        other_decision
      ),
    }
    // Also check window state: it should now contain only key 2.
    assert!(policy.window.lock().contains(&2));
    assert!(!policy.window.lock().contains(&1));
    assert_eq!(policy.window.lock().current_total_cost(), 1);
  }

  #[test]
  fn test_replacement_of_existing_item() {
    let policy: TinyLfuPolicy<i32> = TinyLfuPolicy::new(101);
    // 1. Admit an item, it goes to window.
    <TinyLfuPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &1, 1);
    assert!(policy.window.lock().contains(&1));
    assert_eq!(policy.sketch.estimate(&1), 1);

    // 2. Promote it to main.
    <TinyLfuPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &2, 1); // Evicts 1 from window
    assert!(policy.main.probationary.lock().contains(&1));
    assert!(!policy.window.lock().contains(&1));

    // 3. "Re-admit" the item with a new cost. This should be treated as an access.
    let decision = <TinyLfuPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &1, 5);
    assert!(matches!(decision, AdmissionDecision::Admit)); // No eviction decision.
    assert_eq!(policy.sketch.estimate(&1), 2, "Frequency should increase");

    // The item should be promoted to protected in SLRU.
    assert!(!policy.main.probationary.lock().contains(&1));
    assert!(policy.main.protected.lock().contains(&1));

    // --- Start of Corrected Code ---
    // The test now correctly looks up the node's Index from the `lookup` map,
    // then uses that Index to access the node in the `nodes` arena to get its cost.
    let protected_list = policy.main.protected.lock();
    let cost = protected_list
      .lookup // Use the public `lookup` field for testing.
      .get(&1)
      .map(|&idx| protected_list.nodes[idx].cost) // Use public `nodes` field.
      .unwrap();
    // --- End of Corrected Code ---

    assert_eq!(cost, 5, "Cost should be updated");
  }
}
