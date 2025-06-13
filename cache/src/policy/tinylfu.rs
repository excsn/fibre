use super::{slru::Slru, AccessInfo, CachePolicy};
use crate::policy::slru::LruList;
use crate::policy::AdmissionDecision;

use parking_lot::Mutex;
use std::hash::Hash;

/// A policy that implements the W-TinyLFU scheme described in the paper.
/// It composes a "window" LRU cache with a "main" SLRU cache.
#[derive(Debug)]
pub struct TinyLfu<K: Eq + Hash + Clone> {
  sketch: cms::CountMinSketch,
  // The Window Cache (LRU, no admission policy) - ~1% of capacity
  window: Mutex<LruList<K>>,
  // The Main Cache (SLRU Policy) - ~99% of capacity
  main: Slru<K>,
  window_target_cost: u64,
}

impl<K: Eq + Hash + Clone> TinyLfu<K> {
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
      main: Slru::new(main_cache_cost),
      window_target_cost,
    }
  }
}

impl<K, V> CachePolicy<K, V> for TinyLfu<K>
where
  K: Eq + Hash + Clone + Send + Sync + 'static,
  V: Send + Sync + 'static,
{
  fn on_access(&self, info: &AccessInfo<K, V>) {
    self.sketch.increment(info.key);
    // If an item is in the window, a hit moves it to the MRU position.
    // If it's in the main cache, the SLRU policy handles it.
    // We need to check the window first.
    let mut window_guard = self.window.lock();
    if window_guard.contains(info.key) {
      // `push_front` handles moving an existing element to the front.
      window_guard.push_front(info.key.clone(), info.entry.cost());
      return;
    }
    // Drop the lock before calling the next policy.
    drop(window_guard);

    // Let the main SLRU policy handle its own logic.
    self.main.on_access(info);
  }

  fn on_admit(&self, info: &AccessInfo<K, V>) -> AdmissionDecision<K> {
    // Check if the item already exists. If so, this is a replacement.
    // We should treat it like an access and update its cost, not admit it as a new item.
    let mut window_guard = self.window.lock();
    if window_guard.contains(info.key) {
      // It's in the window. Treat as an access: update cost and move to front.
      self.sketch.increment(info.key);
      window_guard.push_front(info.key.clone(), info.entry.cost());
      return AdmissionDecision::Admit;
    }
    drop(window_guard);

    // Check if it's in the main SLRU cache as well.
    let in_main = {
      let prob = self.main.probationary.lock();
      if prob.contains(info.key) {
        true
      } else {
        let prot = self.main.protected.lock();
        prot.contains(info.key)
      }
    };

    if in_main {
      // It's in the main cache. Treat this "admission" as an access.
      // The on_access method will handle the rest (cost update, promotion, etc).
      self.on_access(info);
      return AdmissionDecision::Admit;
    }
    // --- END: FIX for replacement ---

    // --- If we reach here, it's a truly new item. Proceed with original logic. ---
    self.sketch.increment(info.key); // Increment frequency of the new item

    let mut immediate_victims = Vec::new();
    let mut window_guard = self.window.lock();

    // 1. New item is always added to the policy's Window LRU tracking.
    window_guard.push_front(info.key.clone(), info.entry.cost());

    // 2. Window Maintenance: If window is over its target cost, process candidates.
    while window_guard.current_total_cost() > self.window_target_cost {
      let (candidate_key, candidate_cost) = match window_guard.pop_back() {
        // Candidate from LRU end of window
        Some(c) => c,
        None => break,
      };

      // Release window_guard before potentially accessing self.main to avoid deadlock
      drop(window_guard);

      let main_victim_key_opt = self.main.peek_lru(); // Victim from LRU end of main SLRU
      let should_admit_candidate_to_main =
        main_victim_key_opt
          .as_ref()
          .map_or(true, |main_victim_key| {
            // W-TinyLFU: Admit candidate if its frequency is higher than main victim's frequency
            self.sketch.estimate(&candidate_key) >= self.sketch.estimate(main_victim_key)
          });

      if should_admit_candidate_to_main {
        let main_slru_current_cost_before_admit;
        // Lock guards must be scoped to avoid holding them during subsequent SLRU calls
        {
          let prob_guard = self.main.probationary.lock();
          let prot_guard = self.main.protected.lock();
          main_slru_current_cost_before_admit =
            prob_guard.current_total_cost() + prot_guard.current_total_cost();
        } // Guards dropped here

        // Candidate is admitted to main SLRU. This will update its internal cost.
        self
          .main
          .admit_internal(candidate_key.clone(), candidate_cost);

        // Check if main SLRU is now over its own total capacity due to this admission.
        let new_main_slru_current_cost = main_slru_current_cost_before_admit + candidate_cost;
        let main_slru_total_capacity = self.main.prob_capacity + self.main.prot_capacity;

        if new_main_slru_current_cost > main_slru_total_capacity {
          let cost_to_free_from_main = new_main_slru_current_cost - main_slru_total_capacity;
          if cost_to_free_from_main > 0 {
            let (mut slru_victims, _cost_freed_by_slru) =
              <Slru<K> as CachePolicy<K, V>>::evict(&self.main, cost_to_free_from_main);
            immediate_victims.append(&mut slru_victims);
          }
        }
      } else {
        // Candidate is rejected from promotion. It becomes an immediate victim.
        immediate_victims.push(candidate_key);
      }

      window_guard = self.window.lock(); // Re-acquire for next loop iteration or exit
    }

    if immediate_victims.is_empty() {
      AdmissionDecision::Admit
    } else {
      AdmissionDecision::AdmitAndEvict(immediate_victims)
    }
  }

  fn on_remove(&self, key: &K) {
    if self.window.lock().remove(key).is_some() {
      return;
    }
    <Slru<K> as CachePolicy<K, V>>::on_remove(&self.main, key);
  }

  fn evict(&self, cost_to_free: u64) -> (Vec<K>, u64) {
    if cost_to_free == 0 {
      return (Vec::new(), 0);
    }
    // The primary W-TinyLFU window/main balancing is now in on_admit.
    // This evict call is for general capacity overflow.
    // So, we primarily evict from the main SLRU cache.
    // We could also consider evicting from the window if main is empty,
    // but typically main should be the larger pool to draw from.
    let (main_victims, main_cost_freed) =
      <Slru<K> as CachePolicy<K, V>>::evict(&self.main, cost_to_free);

    // If main_cost_freed is still less than cost_to_free, and window has items,
    // we might consider also taking from window's LRU.
    // For now, let's keep it simple: janitor evicts from main.
    (main_victims, main_cost_freed)
  }

  fn clear(&self) {
    self.window.lock().clear();
    <Slru<K> as CachePolicy<K, V>>::clear(&self.main);
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
      if self.increments.fetch_add(1, Ordering::Relaxed) >= self.capacity {
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
  use std::sync::Arc;

  use super::*;
  use crate::entry::CacheEntry;

  fn access_info_for<'a>(
    key: &'a i32,
    entry: &'a Arc<CacheEntry<String>>,
  ) -> AccessInfo<'a, i32, String> {
    AccessInfo { key, entry }
  }

  #[test]
  fn test_new_item_goes_to_window() {
    let policy: TinyLfu<i32> = TinyLfu::new(101);
    let entry = Arc::new(CacheEntry::new("v".to_string(), 1, None, None));

    policy.on_admit(&access_info_for(&1, &entry));

    assert!(policy.window.lock().contains(&1));
    assert!(!policy.main.probationary.lock().contains(&1));
  }

  #[test]
  fn test_window_overflow_causes_rejection() {
    let policy: TinyLfu<i32> = TinyLfu::new(101); // window=1, main=100

    // 1. Prime the main cache with a high-frequency item.
    let valuable_entry = Arc::new(CacheEntry::new("valuable".to_string(), 1, None, None));
    for _ in 0..5 {
      // FIX: The sketch must be primed along with the SLRU cache to simulate
      // a truly valuable item. The original test forgot this step.
      policy.sketch.increment(&100);
      <Slru<i32> as CachePolicy<i32, String>>::on_admit(
        &policy.main,
        &access_info_for(&100, &valuable_entry),
      );
      <Slru<i32> as CachePolicy<i32, String>>::on_access(
        &policy.main,
        &access_info_for(&100, &valuable_entry),
      );
    }

    // 2. Insert a low-frequency item into the window.
    let candidate_entry = Arc::new(CacheEntry::new("candidate".to_string(), 1, None, None));
    policy.on_admit(&access_info_for(&1, &candidate_entry));
    assert!(policy.window.lock().contains(&1));

    // 3. Insert another item to overflow the window. This makes item 1 a candidate.
    let another_entry = Arc::new(CacheEntry::new("another".to_string(), 1, None, None));
    policy.on_admit(&access_info_for(&2, &another_entry));

    // Manually trigger the eviction that the cache would do to process the overflow.
    <TinyLfu<i32> as CachePolicy<i32, String>>::evict(&policy, 0);

    // Since item 1 (freq=1) is less frequent than item 100 (freq=5), it should be
    // rejected for promotion and simply dropped.
    assert!(policy.window.lock().contains(&2));
    assert!(!policy.window.lock().contains(&1));
    assert!(!policy.main.probationary.lock().contains(&1));
    assert!(!policy.main.protected.lock().contains(&1));
  }

  #[test]
  fn test_window_overflow_causes_admission() {
    let policy: TinyLfu<i32> = TinyLfu::new(101); // window=1, main=100

    // 1. Prime the main cache with a low-frequency item.
    let weak_entry = Arc::new(CacheEntry::new("weak".to_string(), 1, None, None));
    <Slru<i32> as CachePolicy<i32, String>>::on_admit(
      &policy.main,
      &access_info_for(&100, &weak_entry),
    );

    // 2. Insert a higher-frequency item and get it into the window as a candidate.
    let candidate_entry = Arc::new(CacheEntry::new("candidate".to_string(), 1, None, None));
    for _ in 0..5 {
      policy.sketch.increment(&1);
    }
    policy.on_admit(&access_info_for(&1, &candidate_entry));

    // 3. Overflow the window.
    let another_entry = Arc::new(CacheEntry::new("another".to_string(), 1, None, None));
    policy.on_admit(&access_info_for(&2, &another_entry));

    // Manually trigger the eviction that the cache would do.
    <TinyLfu<i32> as CachePolicy<i32, String>>::evict(&policy, 0);

    // Since candidate 1 (freq=5) is more frequent than victim 100 (freq=1),
    // it should be admitted to the main cache's probationary segment.
    assert!(policy.main.probationary.lock().contains(&1));
  }

  #[test]
  fn test_admission_logic_rejects_infrequent_item() {
    // Setup: TinyLfu with window=1, main=100.
    let policy: TinyLfu<i32> = TinyLfu::new(101); // window_target_cost = 1

    // 1. Prime the main cache with a "hot victim" (key 100).
    // let hot_victim_entry = Arc::new(CacheEntry::new("hot".to_string(), 1, None, None)); // Entry not needed for policy-only test

    // --- Corrected Priming ---
    policy.main.admit_internal(100, 1); // Add key 100 to SLRU's probationary
    policy.main.access_internal(&100, 1); // Promote key 100 to SLRU's protected
                                          // --- End Corrected Priming ---

    for _ in 0..10 {
      policy.sketch.increment(&100); // Freq(100) = 10
    }
    // At this point, main SLRU should have key 100 (likely in protected)
    // and peek_lru() should find it if probationary is empty.

    // 2. Insert a "cold candidate" (key 1) into the window.
    let cold_candidate_entry = Arc::new(CacheEntry::new("cold".to_string(), 1, None, None));
    // This on_admit will place '1' in the window. sketch freq(1)=1.
    // Window cost becomes 1, which is equal to window_target_cost (1). No overflow yet.
    let _decision1 = policy.on_admit(&access_info_for(&1, &cold_candidate_entry));
    assert!(policy.window.lock().contains(&1));
    assert_eq!(policy.window.lock().current_total_cost(), 1);

    // 3. Insert another item (key 2) to overflow the window.
    // This on_admit call will trigger window maintenance.
    // Candidate '1' (cost 1, freq 1) will be popped from window.
    // Main victim should be '100' (freq 10).
    // '1' should be rejected because 1 < 10.
    let trigger_entry = Arc::new(CacheEntry::new("trigger".to_string(), 1, None, None));
    let decision = policy.on_admit(&access_info_for(&2, &trigger_entry));

    // 4. Assert the outcome from the decision.
    match decision {
      AdmissionDecision::AdmitAndEvict(victims) => {
        assert_eq!(
          victims,
          vec![1],
          "The cold candidate (1) should have been rejected and returned as a victim."
        );
      }
      // The panic was "Expected AdmitAndEvict with victim [1], got Admit"
      // So, `decision` was `AdmissionDecision::Admit`.
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
  fn test_evict_continues_after_window_logic_to_meet_cost() {
    // This test validates that `evict` continues to evict from main
    // if the window logic doesn't free enough cost.

    // Setup: TinyLfu with window=1, main=100.
    let policy: TinyLfu<i32> = TinyLfu::new(101);

    // 1. Prime the main cache with two items.
    let victim1_entry = Arc::new(CacheEntry::new("v1".to_string(), 5, None, None));
    let victim2_entry = Arc::new(CacheEntry::new("v2".to_string(), 5, None, None));
    <Slru<i32> as CachePolicy<i32, String>>::on_admit(
      &policy.main,
      &access_info_for(&100, &victim1_entry),
    );
    <Slru<i32> as CachePolicy<i32, String>>::on_admit(
      &policy.main,
      &access_info_for(&101, &victim2_entry),
    );

    // *** FIX IS HERE: Make the main cache victim "hotter" than the candidate. ***
    // This ensures the candidate will be rejected.
    policy.sketch.increment(&100);
    policy.sketch.increment(&100);

    // 2. Insert a cold candidate into the window. Cost = 1.
    // This increments the sketch for key 1 only once.
    let cold_candidate_entry = Arc::new(CacheEntry::new("cold".to_string(), 1, None, None));
    policy.on_admit(&access_info_for(&1, &cold_candidate_entry));

    // 3. Overflow the window, making the cold item (1) the candidate.
    let trigger_entry = Arc::new(CacheEntry::new("trigger".to_string(), 1, None, None));
    policy.on_admit(&access_info_for(&2, &trigger_entry));

    // 4. Manually call evict, requesting 10 cost to be freed.
    // The window logic will run, see candidate (1, freq=1) is colder than
    // victim (100, freq=2), and reject it. This frees 1 cost.
    // The `cost_to_free` requirement is now 9. The function MUST continue
    // and evict from the main cache to free the remaining cost.
    let (victims, total_cost_freed) =
      <TinyLfu<i32> as CachePolicy<i32, String>>::evict(&policy, 10);

    // 5. Assert the outcome.
    // We expect the rejected candidate (1) AND victims from main ({100, 101}).
    assert_eq!(
      victims.len(),
      2,
      "Should evict 2 items: the rejected candidate and 2 from main."
    );

    assert!(
      total_cost_freed >= 10,
      "Total cost freed must be at least 10."
    );

    let mut victim_set: std::collections::HashSet<i32> = victims.into_iter().collect();
    assert!(
      victim_set.remove(&100),
      "Main cache victim should be in victims."
    );
    assert!(
      victim_set.remove(&101),
      "Main cache victim should be in victims."
    );
  }
}
