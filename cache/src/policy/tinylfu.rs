use super::{AccessInfo, CachePolicy};
use parking_lot::Mutex;
use std::collections::{HashMap, VecDeque};
use std::hash::{BuildHasher, Hash, Hasher};
use std::sync::atomic::{AtomicUsize, Ordering};

// --- CountMinSketch for Frequency Estimation ---

/// A probabilistic data structure for estimating item frequency.
struct CountMinSketch {
  // A 2D array of counters. `depth` rows, `width` columns.
  counters: Vec<Vec<AtomicUsize>>,
  // Hash builders for each row to map items to columns.
  hashers: Vec<ahash::RandomState>,
  // Total number of increments, used for resetting counters.
  increments: AtomicUsize,
  // The capacity of the sketch. When `increments` reaches `capacity`,
  // all counters are halved to decay old frequencies.
  capacity: usize,
}

impl CountMinSketch {
  fn new(capacity: usize) -> Self {
    // These are typical values from the Ristretto paper.
    const DEPTH: usize = 4;
    let width = (capacity * 4).next_power_of_two();
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
      capacity,
    }
  }

  /// Increments the frequency count for a given key.
  fn increment<K: Hash>(&self, key: &K) {
    for i in 0..self.counters.len() {
      let mut hasher = self.hashers[i].build_hasher();
      key.hash(&mut hasher);
      let index = hasher.finish() as usize % self.counters[i].len();
      self.counters[i][index].fetch_add(1, Ordering::Relaxed);
    }

    if self.increments.fetch_add(1, Ordering::Relaxed) == self.capacity {
      self.reset();
    }
  }

  /// Estimates the frequency of a key by taking the minimum count across all rows.
  fn estimate<K: Hash>(&self, key: &K) -> usize {
    let mut min = usize::MAX;
    for i in 0..self.counters.len() {
      let mut hasher = self.hashers[i].build_hasher();
      key.hash(&mut hasher);
      let index = hasher.finish() as usize % self.counters[i].len();
      min = min.min(self.counters[i][index].load(Ordering::Relaxed));
    }
    min
  }

  /// Halves all counters to decay old frequencies.
  fn reset(&self) {
    self.increments.store(0, Ordering::Relaxed);
    for row in &self.counters {
      for counter in row {
        let current = counter.load(Ordering::Relaxed);
        counter.store(current / 2, Ordering::Relaxed);
      }
    }
  }

  fn clear(&self) {
    self.increments.store(0, Ordering::Relaxed);
    for row in &self.counters {
      for counter in row {
        counter.store(0, Ordering::Relaxed);
      }
    }
  }
}

// --- TinyLfu Eviction Policy ---

/// A list of keys ordered by recent usage (Least Recently Used).
struct LruList<K: Eq + Hash + Clone> {
  keys: VecDeque<K>,
  lookup: HashMap<K, u64>,
}

impl<K: Eq + Hash + Clone> LruList<K> {
  fn new() -> Self {
    Self {
      keys: VecDeque::new(),
      lookup: HashMap::new(),
    }
  }
  fn contains(&self, key: &K) -> bool {
    self.lookup.contains_key(key)
  }

  fn push_front(&mut self, key: K, cost: u64) {
    if self.lookup.insert(key.clone(), cost).is_none() {
      self.keys.push_front(key);
    }
  }

  fn move_to_front(&mut self, key: &K) {
    if let Some(pos) = self.keys.iter().position(|k| k == key) {
      if let Some(k) = self.keys.remove(pos) {
        self.keys.push_front(k);
      }
    }
  }

  fn pop_back(&mut self) -> Option<(K, u64)> {
    if let Some(key) = self.keys.pop_back() {
      if let Some(cost) = self.lookup.remove(&key) {
        return Some((key, cost));
      }
    }
    None
  }

  fn remove(&mut self, key: &K) {
    if self.lookup.remove(key).is_some() {
      self.keys.retain(|k| k != key);
    }
  }
  fn clear(&mut self) {
    self.keys.clear();
    self.lookup.clear();
  }
}

/// The TinyLFU eviction policy, balancing frequency and recency.
pub(crate) struct TinyLfu<K: Eq + Hash + Clone> {
  // Frequency estimator.
  sketch: CountMinSketch,
  // The main cache area, behaving like an LRU.
  main: Mutex<LruList<K>>,
  // A smaller, protected area for new items.
  // Items in `main` are protected from eviction by new, one-hit-wonder items.
  protected: Mutex<LruList<K>>,
  // The total capacity of the cache (used to size the protected area).
  capacity: u64,
}

impl<K: Eq + Hash + Clone> TinyLfu<K> {
  pub(crate) fn new(capacity: u64) -> Self {
    Self {
      sketch: CountMinSketch::new(capacity as usize),
      main: Mutex::new(LruList::new()),
      protected: Mutex::new(LruList::new()),
      capacity,
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
    info.entry.increment_frequency();

    // Promote item from main to protected on access.
    let mut main_guard = self.main.lock();
    if main_guard.contains(info.key) {
      main_guard.remove(info.key);
      self.protected.lock().push_front(info.key.clone(), info.entry.cost());
    } else {
      self.protected.lock().move_to_front(info.key);
    }
  }

  fn on_insert(&self, info: &AccessInfo<K, V>) -> bool {
    // --- Admission Logic ---
    // A simplified version of the TinyLFU admission window.

    // 1. Estimate the frequency of the new item.
    let new_item_freq = self.sketch.estimate(info.key);

    // 2. Find a potential victim to compare against.
    //    The victim is the least-recently-used item in the main (probationary) area.
    let mut main_guard = self.main.lock();
    if let Some(victim_key) = main_guard.keys.back() {
      // 3. If the new item's frequency is lower than the victim's, reject it.
      //    This protects the cache from "one-hit wonders" that would evict
      //    an item that has proven to be more valuable.
      let victim_freq = self.sketch.estimate(victim_key);
      if new_item_freq < victim_freq {
        // The new item is less valuable than the thing we'd have to evict
        // soon. Don't let it in. We still increment its frequency so that if
        // we see it again soon, it will have a better chance.
        self.sketch.increment(info.key);
        return false; // REJECT
      }
    }

    // --- Tracking Logic ---
    // If we reach here, the item is admitted.
    // Add it to the main probationary LRU list.
    main_guard.push_front(info.key.clone(), info.entry.cost());
    self.sketch.increment(info.key);
    true // ADMIT
  }

  fn on_remove(&self, key: &K) {
    self.main.lock().remove(key);
    self.protected.lock().remove(key);
  }

  fn evict(&self, mut cost_to_free: u64) -> Vec<K> {
    let mut victims = Vec::new();
    let mut main_guard = self.main.lock();
    let mut protected_guard = self.protected.lock();

    // The protected area should be roughly 80% of the total capacity.
    // This logic is a simplification; a real cache might use item count.
    let protected_len = protected_guard.keys.len() as u64;
    let protected_cap = (self.capacity as f64 * 0.8) as u64;

    // Loop until we have freed enough cost.
    while cost_to_free > 0 {
      // Step 1: Demote from protected to main if protected is over capacity.
      // This ensures the main LRU has candidates to choose from.
      if protected_len > protected_cap {
        if let Some((demoted_key, demoted_cost)) = protected_guard.pop_back() {
          main_guard.push_front(demoted_key, demoted_cost);
          // Continue the loop to re-evaluate candidates.
          continue;
        }
      }

      // Step 2: Select the best victim candidate.
      let victim_key = match (main_guard.keys.back(), protected_guard.keys.back()) {
        // Case 1: Both main and protected have candidates.
        (Some(main_candidate), Some(protected_candidate)) => {
          // Compare the frequency of the two candidates.
          if self.sketch.estimate(main_candidate) > self.sketch.estimate(protected_candidate) {
            // The protected candidate is "colder" (less frequently used).
            // Choose it as the victim.
            protected_guard.pop_back()
          } else {
            // The main candidate is colder or they are equal; evict from main.
            main_guard.pop_back()
          }
        }
        // Case 2: Only the main area has a candidate.
        (Some(_), None) => main_guard.pop_back(),
        // Case 3: Only the protected area has a candidate.
        (None, Some(_)) => protected_guard.pop_back(),
        // Case 4: No candidates available in either list.
        (None, None) => break, // Nothing left to evict, exit the loop.
      };

      // Step 3: Process the chosen victim.
      let victim = match (main_guard.keys.back(), protected_guard.keys.back()) {
        (Some(main_cand_key), Some(prot_cand_key)) => {
          if self.sketch.estimate(main_cand_key) > self.sketch.estimate(prot_cand_key) {
            protected_guard.pop_back()
          } else {
            main_guard.pop_back()
          }
        }
        (Some(_), None) => main_guard.pop_back(),
        (None, Some(_)) => protected_guard.pop_back(),
        (None, None) => break,
      };

      if let Some((key, cost)) = victim {
        cost_to_free = cost_to_free.saturating_sub(cost);
        victims.push(key);
      } else {
        break;
      }
    }

    victims
  }

  fn clear(&self) {
    self.sketch.clear();
    self.main.lock().clear();
    self.protected.lock().clear();
  }
}
