use super::CachePolicy;
use super::slru::SlruState;
use crate::policy::AdmissionDecision;
use crate::policy::lru_list::LruList;

use parking_lot::Mutex;
use std::hash::Hash;

/// The unified mutable state of W-TinyLFU, held under a single lock.
struct TinyLfuState<K: Eq + Hash + Clone> {
  window: LruList<K>,
  main: SlruState<K>,
  sketch: cms::CountMinSketch,
}

/// A policy that implements the W-TinyLFU scheme described in the paper.
/// It composes a "window" LRU cache with a "main" SLRU cache.
#[derive(Debug)]
pub struct TinyLfuPolicy<K: Eq + Hash + Clone> {
  state: Mutex<TinyLfuState<K>>,
  window_target_cost: u64,
  main_prot_capacity: u64,
}

impl<K: Eq + Hash + Clone + std::fmt::Debug> std::fmt::Debug for TinyLfuState<K> {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_struct("TinyLfuState")
      .field("window", &self.window)
      .field("main", &self.main)
      .finish()
  }
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

    // Pre-compute the SLRU protected capacity for use in access_internal calls.
    let main_prot_capacity = if main_cache_cost == 0 {
      0
    } else {
      let prob_capacity = ((main_cache_cost as f64 * 0.20).round() as u64).max(1);
      main_cache_cost.saturating_sub(prob_capacity)
    };

    Self {
      state: Mutex::new(TinyLfuState {
        window: LruList::new(),
        main: SlruState::new_with_capacity(main_cache_cost),
        sketch: cms::CountMinSketch::new(cms_reset_threshold as usize),
      }),
      window_target_cost,
      main_prot_capacity,
    }
  }
}

impl<K, V> CachePolicy<K, V> for TinyLfuPolicy<K>
where
  K: Eq + Hash + Clone + Send + Sync + 'static,
  V: Send + Sync + 'static,
{
  fn on_access(&self, key: &K, cost: u64) {
    let mut state = self.state.lock();
    state.sketch.increment(key);
    if state.window.contains(key) {
      state.window.push_front(key.clone(), cost);
      return;
    }
    state.main.access_internal(key, cost, self.main_prot_capacity);
  }

  fn on_admit(&self, key: &K, cost: u64) -> AdmissionDecision<K> {
    let mut state = self.state.lock();
    state.sketch.increment(key);

    // If the item is already in the main cache, this is just an access/update.
    let is_in_main =
      state.main.probationary.contains(key) || state.main.protected.contains(key);
    if is_in_main {
      state.main.access_internal(key, cost, self.main_prot_capacity);
      return AdmissionDecision::Admit;
    }

    // Otherwise, it's either new or in the window. Add it to the window.
    state.window.push_front(key.clone(), cost);

    // Manage the window if it's over capacity.
    let mut rejected_candidates = Vec::new();
    while state.window.current_total_cost() > self.window_target_cost {
      let (candidate_key, candidate_cost) = match state.window.pop_back() {
        Some(c) => c,
        None => break,
      };

      // All data is now under one lock — no drop/re-acquire needed.
      let main_victim_key_opt = state.main.peek_lru();
      let admit_candidate = main_victim_key_opt
        .as_ref()
        .map_or(true, |main_victim_key| {
          state.sketch.estimate(&candidate_key) >= state.sketch.estimate(main_victim_key)
        });

      if admit_candidate {
        state.main.admit_internal(candidate_key, candidate_cost);
      } else {
        rejected_candidates.push(candidate_key);
      }
    }

    if rejected_candidates.is_empty() {
      AdmissionDecision::Admit
    } else {
      AdmissionDecision::AdmitAndEvict(rejected_candidates)
    }
  }

  fn on_remove(&self, key: &K) {
    let mut state = self.state.lock();
    if state.window.remove(key).is_some() {
      return;
    }
    if state.main.probationary.remove(key).is_some() {
      return;
    }
    state.main.protected.remove(key);
  }

  fn evict(&self, cost_to_free: u64) -> (Vec<K>, u64) {
    if cost_to_free == 0 {
      return (Vec::new(), 0);
    }
    self.state.lock().main.evict_items(cost_to_free, self.main_prot_capacity)
  }

  fn clear(&self) {
    let mut state = self.state.lock();
    state.window.clear();
    state.main.probationary.clear();
    state.main.protected.clear();
    state.sketch.clear();
  }
}

mod cms {
  use std::hash::{BuildHasher, Hash, Hasher};

  #[derive(Debug)]
  pub(super) struct CountMinSketch {
    // Plain usize counters — synchronization is handled by the outer TinyLfuState Mutex.
    counters: Vec<Vec<usize>>,
    hashers: Vec<ahash::RandomState>,
    increments: usize,
    capacity: usize,
  }

  impl CountMinSketch {
    pub fn new(reset_threshold: usize) -> Self {
      const DEPTH: usize = 4;
      let width = (reset_threshold * 2 / DEPTH).max(256).next_power_of_two();
      let counters = (0..DEPTH).map(|_| vec![0usize; width]).collect();
      let hashers = (0..DEPTH).map(|_| ahash::RandomState::new()).collect();
      Self {
        counters,
        hashers,
        increments: 0,
        capacity: reset_threshold,
      }
    }

    pub fn increment<K: Hash>(&mut self, key: &K) {
      for i in 0..self.counters.len() {
        let mut hasher = self.hashers[i].build_hasher();
        key.hash(&mut hasher);
        let index = hasher.finish() as usize % self.counters[i].len();
        self.counters[i][index] += 1;
      }
      self.increments += 1;
      if self.increments >= self.capacity {
        self.reset();
      }
    }

    pub fn estimate<K: Hash>(&self, key: &K) -> usize {
      let mut min_count = usize::MAX;
      for i in 0..self.counters.len() {
        let mut hasher = self.hashers[i].build_hasher();
        key.hash(&mut hasher);
        let index = hasher.finish() as usize % self.counters[i].len();
        min_count = min_count.min(self.counters[i][index]);
      }
      min_count
    }

    fn reset(&mut self) {
      self.increments = 0;
      for row in &mut self.counters {
        for counter in row {
          *counter /= 2;
        }
      }
    }

    pub fn clear(&mut self) {
      self.increments = 0;
      for row in &mut self.counters {
        for counter in row {
          *counter = 0;
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
      policy.state.lock().window.contains(&1),
      "Item should be in window"
    );
    assert!(!policy.state.lock().main.probationary.contains(&1));
  }

  #[test]
  fn test_window_overflow_causes_rejection() {
    let policy: TinyLfuPolicy<i32> = TinyLfuPolicy::new(101); // window=1, main=100

    // 1. Prime the main cache with a high-frequency item (key 100).
    policy.state.lock().main.admit_internal(100, 1);
    for _ in 0..5 {
      policy.state.lock().sketch.increment(&100);
    }

    // 2. Insert a low-frequency item (key 1) into the window.
    <TinyLfuPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &1, 1);
    assert!(policy.state.lock().window.contains(&1));

    // 3. Insert another item to overflow the window. This makes item 1 a candidate.
    // Since freq(1)=1 is less than freq(100)=5, candidate 1 should be rejected.
    let decision = <TinyLfuPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &2, 1);

    if let AdmissionDecision::AdmitAndEvict(victims) = decision {
      assert_eq!(victims, vec![1], "Rejected candidate should be the victim");
    } else {
      panic!("Expected AdmitAndEvict, got {:?}", decision);
    }

    assert!(policy.state.lock().window.contains(&2));
    assert!(!policy.state.lock().window.contains(&1));
    assert!(!policy.state.lock().main.probationary.contains(&1));
    assert!(!policy.state.lock().main.protected.contains(&1));
  }

  #[test]
  fn test_window_overflow_causes_admission() {
    let policy: TinyLfuPolicy<i32> = TinyLfuPolicy::new(101); // window=1, main=100

    // 1. Prime the main cache with a low-frequency item (key 100, freq=1).
    policy.state.lock().main.admit_internal(100, 1);
    policy.state.lock().sketch.increment(&100);

    // 2. Insert a higher-frequency item and get it into the window as a candidate.
    <TinyLfuPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &1, 1);
    for _ in 0..5 {
      policy.state.lock().sketch.increment(&1);
    }

    // 3. Overflow the window.
    <TinyLfuPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &2, 1);

    // Since candidate 1 (freq=5) is more frequent than victim 100 (freq=1),
    // it should be admitted to the main cache's probationary segment.
    assert!(policy.state.lock().main.probationary.contains(&1));
  }

  #[test]
  fn test_admission_logic_rejects_infrequent_item() {
    // Setup: TinyLfu with window=1, main=100.
    let policy: TinyLfuPolicy<i32> = TinyLfuPolicy::new(101); // window_target_cost = 1

    // 1. Prime the main cache with a "hot victim" (key 100, freq=10).
    policy.state.lock().main.admit_internal(100, 1);
    for _ in 0..10 {
      policy.state.lock().sketch.increment(&100);
    }

    // 2. Insert a "cold candidate" (key 1) into the window.
    let _decision1 = <TinyLfuPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &1, 1);
    assert!(policy.state.lock().window.contains(&1));

    // 3. Insert another item (key 2) to overflow the window.
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
    assert!(policy.state.lock().window.contains(&2));
    assert!(!policy.state.lock().window.contains(&1));
    assert_eq!(policy.state.lock().window.current_total_cost(), 1);
  }

  #[test]
  fn test_replacement_of_existing_item() {
    let policy: TinyLfuPolicy<i32> = TinyLfuPolicy::new(101);
    // 1. Admit an item, it goes to window.
    <TinyLfuPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &1, 1);
    assert!(policy.state.lock().window.contains(&1));
    assert_eq!(policy.state.lock().sketch.estimate(&1), 1);

    // 2. Promote it to main.
    <TinyLfuPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &2, 1); // Evicts 1 from window
    assert!(policy.state.lock().main.probationary.contains(&1));
    assert!(!policy.state.lock().window.contains(&1));

    // 3. "Re-admit" the item with a new cost. This should be treated as an access.
    let decision = <TinyLfuPolicy<i32> as CachePolicy<i32, ()>>::on_admit(&policy, &1, 5);
    assert!(matches!(decision, AdmissionDecision::Admit)); // No eviction decision.
    assert_eq!(policy.state.lock().sketch.estimate(&1), 2, "Frequency should increase");

    // The item should be promoted to protected in SLRU.
    assert!(!policy.state.lock().main.probationary.contains(&1));
    assert!(policy.state.lock().main.protected.contains(&1));

    let state = policy.state.lock();
    let cost = state
      .main
      .protected
      .lookup
      .get(&1)
      .map(|&idx| state.main.protected.nodes[idx].cost)
      .unwrap();

    assert_eq!(cost, 5, "Cost should be updated");
  }
}
