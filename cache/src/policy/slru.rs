use parking_lot::Mutex;
use std::collections::{HashMap, VecDeque};
use std::hash::Hash;

use super::{AccessInfo, CachePolicy};
use crate::policy::AdmissionDecision;

// A self-contained, cost-based LRU list helper.
#[derive(Debug)]
pub(crate) struct LruList<K: Eq + Hash + Clone> {
  pub(crate) keys: VecDeque<K>,
  pub(crate) lookup: HashMap<K, u64>,
  current_cost: u64,
}

impl<K: Eq + Hash + Clone> LruList<K> {
  pub fn new() -> Self {
    Self {
      keys: VecDeque::new(),
      lookup: HashMap::new(),
      current_cost: 0,
    }
  }

  pub fn contains(&self, key: &K) -> bool {
    self.lookup.contains_key(key)
  }

  pub fn current_total_cost(&self) -> u64 {
    self.current_cost
  }

  pub fn push_front(&mut self, key: K, cost: u64) {
    if let Some(old_cost) = self.lookup.insert(key.clone(), cost) {
      self.current_cost = self.current_cost.saturating_sub(old_cost) + cost;
      if let Some(pos) = self.keys.iter().position(|k| k == &key) {
        self.keys.remove(pos);
      }
    } else {
      self.current_cost += cost;
    }
    self.keys.push_front(key);
  }

  pub fn move_to_front(&mut self, key: &K) {
    if let Some(pos) = self.keys.iter().position(|k| k == key) {
      if let Some(k_val) = self.keys.remove(pos) {
        self.keys.push_front(k_val);
      }
    }
  }

  pub fn pop_back(&mut self) -> Option<(K, u64)> {
    if let Some(key) = self.keys.pop_back() {
      if let Some(cost) = self.lookup.remove(&key) {
        self.current_cost = self.current_cost.saturating_sub(cost);
        return Some((key, cost));
      }
    }
    None
  }

  pub fn remove(&mut self, key: &K) -> Option<u64> {
    if let Some(cost) = self.lookup.remove(key) {
      self.keys.retain(|k| k != key);
      self.current_cost = self.current_cost.saturating_sub(cost);
      return Some(cost);
    }
    None
  }

  pub fn clear(&mut self) {
    self.keys.clear();
    self.lookup.clear();
    self.current_cost = 0;
  }
}

/// An eviction policy based on the Segmented LRU algorithm.
/// It maintains a probationary and a protected segment to resist cache scans.
#[derive(Debug)]
pub struct Slru<K: Eq + Hash + Clone> {
  pub(crate) probationary: Mutex<LruList<K>>,
  pub(crate) protected: Mutex<LruList<K>>,
  pub(crate) prob_capacity: u64,
  pub(crate) prot_capacity: u64,
}

impl<K: Eq + Hash + Clone> Slru<K> {
  pub fn new(capacity: u64) -> Self {
    let prob_capacity = if capacity == 0 {
      0
    } else {
      ((capacity as f64 * 0.20).round() as u64).max(1)
    };
    let prot_capacity = capacity.saturating_sub(prob_capacity);

    Self {
      probationary: Mutex::new(LruList::new()),
      protected: Mutex::new(LruList::new()),
      prob_capacity,
      prot_capacity,
    }
  }

  // A helper function to maintain segment capacities.
  // It handles demotion from protected to probationary.
  fn maintain_capacities(&self, prob_guard: &mut LruList<K>, prot_guard: &mut LruList<K>) {
    // Demote from protected if it's over capacity
    while prot_guard.current_total_cost() > self.prot_capacity {
      if let Some((demoted_key, demoted_cost)) = prot_guard.pop_back() {
        prob_guard.push_front(demoted_key, demoted_cost);
      } else {
        break;
      }
    }
  }

  // New method to peek at the next eviction victim without removing it.
  // This is required by the W-TinyLFU composition logic.
  pub(crate) fn peek_lru(&self) -> Option<K> {
    let prob_guard = self.probationary.lock();
    // Prioritize evicting from the probationary segment's LRU end.
    if let Some(key) = prob_guard.keys.back() {
      return Some(key.clone());
    }

    // If probationary is empty, the victim is from the protected segment's LRU end.
    let prot_guard = self.protected.lock();
    if let Some(key) = prot_guard.keys.back() {
      return Some(key.clone());
    }

    None
  }

  /// Internal method for admitting a key with a known cost,
  /// used when the full CacheEntry<V> isn't available or needed.
  pub(crate) fn admit_internal(&self, key: K, cost: u64) {
    let mut prob_guard = self.probationary.lock();
    let prot_guard = self.protected.lock(); // Keep lock to ensure consistency with contains check

    // If item is already in the cache (prob or prot), this is effectively a
    // "touch" or "cost update" if costs were dynamic, but SLRU on_admit
    // is simple. Here, we just ensure it's tracked.
    // Original SLRU on_admit:
    // if !prot_guard.contains(&key) && !prob_guard.contains(&key) {
    //    prob_guard.push_front(key.clone(), cost);
    // }
    // For an internal promotion, it's less likely to already be in probationary.
    // If it *is* already in protected, `on_access` should have handled it.
    // If it was in probationary and now being promoted, it would have been removed
    // from probationary by `TinyLfu` before this call, and now added to protected.
    // This `admit_internal` is for adding to *probationary* when promoted to main by TinyLfu.

    // The key was a candidate from TinyLfu's window. It's now being admitted to SLRU's
    // probationary segment.
    if !prot_guard.contains(&key) && !prob_guard.contains(&key) {
      // Ensure it's not already somewhere in SLRU
      prob_guard.push_front(key, cost);
    } else if prob_guard.contains(&key) {
      // If it somehow ended up in probationary (e.g. demoted then re-promoted quickly)
      // update its cost and move to front if push_front does that.
      // LruList::push_front handles existing items by updating cost & moving to front.
      prob_guard.push_front(key, cost);
    }
    // If it's already in prot_guard, on_access on TinyLFU should have handled the SLRU update.
    // This path is for when TinyLFU decides to move something *into* SLRU.
  }

  /// Internal method for handling access to a key already in SLRU,
  /// used when the full CacheEntry<V> isn't available or needed.
  pub(crate) fn access_internal(&self, key: &K, cost: u64) {
    let mut prob_guard = self.probationary.lock();
    let mut prot_guard = self.protected.lock();

    if prot_guard.contains(key) {
      prot_guard.move_to_front(key);
      return;
    }
    if let Some(c) = prob_guard.remove(key) {
      // Use cost from prob_guard if exists
      prot_guard.push_front(key.clone(), c);
      self.maintain_capacities(&mut prob_guard, &mut prot_guard);
    }
    // If not in either, it's a new admission, which should go through admit_internal.
  }
}

impl<K, V> CachePolicy<K, V> for Slru<K>
where
  K: Eq + Hash + Clone + Send + Sync,
  V: Send + Sync,
{
  fn on_access(&self, info: &AccessInfo<K, V>) {
    let mut prob_guard = self.probationary.lock();
    let mut prot_guard = self.protected.lock();

    // If it's in protected, just refresh it and update its cost.
    if prot_guard.contains(info.key) {
      // Use push_front, which handles cost updates for existing items.
      prot_guard.push_front(info.key.clone(), info.entry.cost());
      return;
    }

    // If it's in probationary, promote it to protected with the new cost.
    if prob_guard.remove(info.key).is_some() {
      prot_guard.push_front(info.key.clone(), info.entry.cost());
      // Promoting might cause overflow in protected, which triggers demotion.
      self.maintain_capacities(&mut prob_guard, &mut prot_guard);
    }
  }

  fn on_admit(&self, info: &AccessInfo<K, V>) -> AdmissionDecision<K> {
    let mut prob_guard = self.probationary.lock();
    let prot_guard = self.protected.lock();

    // If item is already in the cache, on_access will handle it.
    // Here we only handle new items.
    if !prot_guard.contains(info.key) && !prob_guard.contains(info.key) {
      prob_guard.push_front(info.key.clone(), info.entry.cost());
    }

    AdmissionDecision::Admit // SLRU always admits. Eviction is handled separately.
  }

  fn on_remove(&self, key: &K) {
    if self.probationary.lock().remove(key).is_some() {
      return;
    }
    self.protected.lock().remove(key);
  }

  fn evict(&self, mut cost_to_free: u64) -> (Vec<K>, u64) {
    let mut victims = Vec::new();
    let mut total_cost_freed = 0;
    let mut prob_guard = self.probationary.lock();
    let mut prot_guard = self.protected.lock();

    self.maintain_capacities(&mut prob_guard, &mut prot_guard);

    while cost_to_free > 0 {
      if let Some((key, cost)) = prob_guard.pop_back() {
        cost_to_free = cost_to_free.saturating_sub(cost);
        total_cost_freed += cost;
        victims.push(key);
      } else {
        break;
      }
    }

    while cost_to_free > 0 {
      if let Some((key, cost)) = prot_guard.pop_back() {
        cost_to_free = cost_to_free.saturating_sub(cost);
        total_cost_freed += cost;
        victims.push(key);
      } else {
        break;
      }
    }

    (victims, total_cost_freed)
  }

  fn clear(&self) {
    self.probationary.lock().clear();
    self.protected.lock().clear();
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::entry::CacheEntry;

  use std::sync::Arc;
  // Helper to create a dummy AccessInfo for a given key.
  // The `entry` Arc needs to live as long as the AccessInfo, so we pass it in.
  fn access_info_for<'a>(
    key: &'a i32,
    entry: &'a Arc<CacheEntry<String>>,
  ) -> AccessInfo<'a, i32, String> {
    AccessInfo { key, entry }
  }

  #[test]
  fn new_item_goes_to_probationary() {
    let policy: Slru<i32> = Slru::new(10);
    let entry = Arc::new(CacheEntry::new("v".to_string(), 1, None, None));
    policy.on_admit(&access_info_for(&1, &entry));

    assert!(policy.probationary.lock().contains(&1));
    assert!(!policy.protected.lock().contains(&1));
    assert_eq!(policy.probationary.lock().current_total_cost(), 1);
  }

  #[test]
  fn access_promotes_from_probationary_to_protected() {
    let policy: Slru<i32> = Slru::new(10);
    let entry = Arc::new(CacheEntry::new("v".to_string(), 1, None, None));
    policy.on_admit(&access_info_for(&1, &entry));

    policy.on_access(&access_info_for(&1, &entry));

    assert!(!policy.probationary.lock().contains(&1));
    assert!(policy.protected.lock().contains(&1));
    assert_eq!(policy.protected.lock().current_total_cost(), 1);
  }

  #[test]
  fn access_refreshes_item_in_protected() {
    let policy: Slru<i32> = Slru::new(10); // prob=2, prot=8
    let entry1 = Arc::new(CacheEntry::new("v".to_string(), 1, None, None));
    policy.on_admit(&access_info_for(&1, &entry1));
    policy.on_access(&access_info_for(&1, &entry1)); // 1 is now in protected

    let entry2 = Arc::new(CacheEntry::new("v".to_string(), 1, None, None));
    policy.on_admit(&access_info_for(&2, &entry2));
    policy.on_access(&access_info_for(&2, &entry2)); // 2 is now in protected, LRU of protected is 1

    assert_eq!(policy.protected.lock().keys.back(), Some(&1));

    policy.on_access(&access_info_for(&1, &entry1)); // Access 1 again

    // Now 2 should be the LRU of protected
    assert_eq!(policy.protected.lock().keys.back(), Some(&2));
  }

  #[test]
  fn promotion_causes_demotion_when_protected_is_full() {
    let policy: Slru<i32> = Slru::new(5); // prob=1, prot=4

    // 1. Fill protected segment
    let entries: Vec<_> = (1..=5)
      .map(|_| Arc::new(CacheEntry::new("v".to_string(), 1, None, None)))
      .collect();
    for i in 1..=4 {
      policy.on_admit(&access_info_for(&i, &entries[(i - 1) as usize]));
      policy.on_access(&access_info_for(&i, &entries[(i - 1) as usize]));
    }
    assert_eq!(policy.protected.lock().current_total_cost(), 4);
    assert_eq!(policy.probationary.lock().current_total_cost(), 0);

    // 2. Insert an item into probationary
    policy.on_admit(&access_info_for(&5, &entries[4]));
    assert_eq!(policy.probationary.lock().current_total_cost(), 1);

    // 3. Access item 5 to promote it. This should cause item 1 (LRU of protected) to be demoted.
    policy.on_access(&access_info_for(&5, &entries[4]));

    assert!(policy.protected.lock().contains(&5), "5 should be promoted");
    assert!(
      !policy.protected.lock().contains(&1),
      "1 should no longer be in protected"
    );
    assert!(
      policy.probationary.lock().contains(&1),
      "1 should be demoted to probationary"
    );
    assert_eq!(policy.protected.lock().current_total_cost(), 4);
    assert_eq!(policy.probationary.lock().current_total_cost(), 1);
  }

  #[test]
  fn evict_from_probationary_first() {
    let policy: Slru<i32> = Slru::new(5); // prob=1, prot=4
    let entries: Vec<_> = (1..=5)
      .map(|_| Arc::new(CacheEntry::new("v".to_string(), 1, None, None)))
      .collect();
    for i in 1..=5 {
      policy.on_admit(&access_info_for(&i, &entries[(i - 1) as usize]));
    }
    // Prob has [5, 4, 3, 2, 1], but capacity is 1. Protected is empty.

    let (victims, _) = <Slru<i32> as CachePolicy<i32, String>>::evict(&policy, 1);

    // The `evict` call first calls `maintain_capacities`, then evicts from probationary.
    // The LRU item is 1.
    assert_eq!(victims, vec![1]);
    assert!(!policy.probationary.lock().contains(&1));
  }

  #[test]
  fn evict_from_protected_if_probationary_is_empty() {
    let policy: Slru<i32> = Slru::new(5); // prob=1, prot=4
    let entries: Vec<_> = (1..=4)
      .map(|_| Arc::new(CacheEntry::new("v".to_string(), 1, None, None)))
      .collect();
    for i in 1..=4 {
      policy.on_admit(&access_info_for(&i, &entries[(i - 1) as usize]));
      policy.on_access(&access_info_for(&i, &entries[(i - 1) as usize]));
    }
    // Now protected has [4, 3, 2, 1], probationary is empty.

    let (victims, _) = <Slru<i32> as CachePolicy<i32, String>>::evict(&policy, 1);

    // LRU of protected is 1.
    assert_eq!(victims, vec![1]);
    assert!(!policy.protected.lock().contains(&1));
  }

  #[test]
  fn on_remove_cleans_up_state() {
    let policy: Slru<i32> = Slru::new(10);
    let entry1 = Arc::new(CacheEntry::new("v".to_string(), 1, None, None));
    let entry2 = Arc::new(CacheEntry::new("v".to_string(), 1, None, None));
    policy.on_admit(&access_info_for(&1, &entry1)); // in prob
    policy.on_admit(&access_info_for(&2, &entry2));
    policy.on_access(&access_info_for(&2, &entry2)); // in prot

    <Slru<i32> as CachePolicy<i32, String>>::on_remove(&policy, &1);
    assert!(!policy.probationary.lock().contains(&1));

    <Slru<i32> as CachePolicy<i32, String>>::on_remove(&policy, &2);
    assert!(!policy.protected.lock().contains(&2));

    assert_eq!(policy.probationary.lock().current_total_cost(), 0);
    assert_eq!(policy.protected.lock().current_total_cost(), 0);
  }

  #[test]
  fn peek_lru_finds_correct_victim() {
    let policy: Slru<i32> = Slru::new(5); // prob=1, prot=4
    let entries: Vec<_> = (1..=2)
      .map(|_| Arc::new(CacheEntry::new("v".to_string(), 1, None, None)))
      .collect();

    // 1. Victim in probationary
    policy.on_admit(&access_info_for(&1, &entries[0]));
    policy.on_admit(&access_info_for(&2, &entries[1]));
    assert_eq!(
      policy.peek_lru(),
      Some(1),
      "LRU of probationary should be the victim"
    );

    // 2. Victim in protected (after probationary is empty)
    policy.on_access(&access_info_for(&1, &entries[0]));
    policy.on_access(&access_info_for(&2, &entries[1]));
    assert_eq!(
      policy.peek_lru(),
      Some(1),
      "LRU of protected should be the victim"
    );

    // 3. Re-accessing changes the victim
    policy.on_access(&access_info_for(&1, &entries[0]));
    assert_eq!(
      policy.peek_lru(),
      Some(2),
      "After re-access, new LRU of protected is the victim"
    );
  }

  #[test]
  fn slru_with_small_capacity_allocates_correctly() {
    // Capacity 5 -> 20% is 1. prob_cap=1, prot_cap=4.
    let policy_5 = Slru::<i32>::new(5);
    assert_eq!(policy_5.prob_capacity, 1);
    assert_eq!(policy_5.prot_capacity, 4);

    // Capacity 4 -> 20% is 0.8, rounds to 1. prob_cap=1, prot_cap=3.
    let policy_4 = Slru::<i32>::new(4);
    assert_eq!(policy_4.prob_capacity, 1);
    assert_eq!(policy_4.prot_capacity, 3);

    // Capacity 1 -> 20% is 0.2, rounds to 0, but is max(1). prob_cap=1, prot_cap=0.
    let policy_1 = Slru::<i32>::new(1);
    assert_eq!(policy_1.prob_capacity, 1);
    assert_eq!(policy_1.prot_capacity, 0);

    // Capacity 0 -> prob_cap=0, prot_cap=0.
    let policy_0 = Slru::<i32>::new(0);
    assert_eq!(policy_0.prob_capacity, 0);
    assert_eq!(policy_0.prot_capacity, 0);
  }

  #[test]
  fn reinsert_of_existing_key_is_noop() {
    let policy: Slru<i32> = Slru::new(10);
    let entry = Arc::new(CacheEntry::new("v".to_string(), 1, None, None));

    // 1. Insert into probationary
    policy.on_admit(&access_info_for(&1, &entry));
    let prob_keys_before = policy.probationary.lock().keys.clone();

    // Re-inserting should do nothing
    policy.on_admit(&access_info_for(&1, &entry));
    let prob_keys_after = policy.probationary.lock().keys.clone();
    assert_eq!(
      prob_keys_before, prob_keys_after,
      "Re-inserting into probationary should not change order"
    );

    // 2. Promote to protected
    policy.on_access(&access_info_for(&1, &entry));
    let prot_keys_before = policy.protected.lock().keys.clone();

    // Re-inserting should still do nothing
    policy.on_admit(&access_info_for(&1, &entry));
    let prot_keys_after = policy.protected.lock().keys.clone();
    assert_eq!(
      prot_keys_before, prot_keys_after,
      "Re-inserting into protected should not change order"
    );
  }

  #[test]
  fn evict_empties_probationary_then_protected() {
    let policy: Slru<i32> = Slru::new(4); // prob=1, prot=3
    let entries: Vec<_> = (1..=4)
      .map(|_| Arc::new(CacheEntry::new("v".to_string(), 1, None, None)))
      .collect();

    // Fill protected
    for i in 1..=3 {
      policy.on_admit(&access_info_for(&i, &entries[(i - 1) as usize]));
      policy.on_access(&access_info_for(&i, &entries[(i - 1) as usize]));
    }
    // Fill probationary
    policy.on_admit(&access_info_for(&4, &entries[3]));

    assert_eq!(policy.probationary.lock().current_total_cost(), 1);
    assert_eq!(policy.protected.lock().current_total_cost(), 3);

    // Evict 3 items. Should take 1 from prob, and 2 from prot.
    // Prob victim: 4. Prot victims: 1, 2.
    let (victims, cost_freed) = <Slru<i32> as CachePolicy<i32, String>>::evict(&policy, 3);

    assert_eq!(cost_freed, 3);
    assert_eq!(
      victims,
      vec![4, 1, 2],
      "Eviction order should be prob-lru then prot-lru"
    );

    assert_eq!(policy.probationary.lock().current_total_cost(), 0);
    assert_eq!(policy.protected.lock().current_total_cost(), 1);
    assert!(policy.protected.lock().contains(&3));
  }

  #[test]
  fn evict_more_than_total_cost_clears_everything() {
    let policy: Slru<i32> = Slru::new(5);
    let entries: Vec<_> = (1..=5)
      .map(|_| Arc::new(CacheEntry::new("v".to_string(), 1, None, None)))
      .collect();
    for i in 1..=5 {
      policy.on_admit(&access_info_for(&i, &entries[(i - 1) as usize]));
    }
    policy.on_access(&access_info_for(&5, &entries[4])); // Promote 5

    // Request to free a huge amount of cost
    let (victims, cost_freed) = <Slru<i32> as CachePolicy<i32, String>>::evict(&policy, 100);

    assert_eq!(cost_freed, 5, "Should free the cost of all items");
    assert_eq!(victims.len(), 5, "Should evict all 5 items");

    assert_eq!(policy.probationary.lock().current_total_cost(), 0);
    assert_eq!(policy.protected.lock().current_total_cost(), 0);
    assert!(policy.peek_lru().is_none());
  }

  #[test]
  fn evict_with_zero_cost_triggers_demotion() {
    let policy: Slru<i32> = Slru::new(5); // prob=1, prot=4

    // 1. Fill protected segment to its capacity of 4
    let entries: Vec<_> = (1..=5)
      .map(|_| Arc::new(CacheEntry::new("v".to_string(), 1, None, None)))
      .collect();
    for i in 1..=4 {
      policy.on_admit(&access_info_for(&i, &entries[(i - 1) as usize]));
      policy.on_access(&access_info_for(&i, &entries[(i - 1) as usize]));
    }
    assert_eq!(policy.protected.lock().current_total_cost(), 4);
    assert_eq!(policy.probationary.lock().current_total_cost(), 0);
    assert_eq!(
      policy.protected.lock().keys.back(),
      Some(&1),
      "LRU of protected is 1 initially"
    );

    // 2. Insert a new item into probationary
    policy.on_admit(&access_info_for(&5, &entries[4]));
    assert_eq!(policy.probationary.lock().current_total_cost(), 1);

    // 3. Act: Access the probationary item to promote it.
    // This action should synchronously:
    //    a) Move item 5 to protected (cost becomes 5, over capacity).
    //    b) Trigger `maintain_capacities`, demoting item 1 to probationary.
    policy.on_access(&access_info_for(&5, &entries[4]));

    // 4. Assert: Check the state *immediately* after the access.
    assert_eq!(
      policy.protected.lock().current_total_cost(),
      4,
      "Protected cost should be brought back to capacity by demotion"
    );
    assert_eq!(
      policy.probationary.lock().current_total_cost(),
      1,
      "Probationary should now have the demoted item"
    );

    assert!(
      policy.protected.lock().contains(&5),
      "Item 5 should have been promoted"
    );
    assert!(
      !policy.protected.lock().contains(&1),
      "Item 1 should no longer be in protected"
    );
    assert!(
      policy.probationary.lock().contains(&1),
      "Item 1 should have been demoted to probationary"
    );
    assert_eq!(
      policy.protected.lock().keys.back(),
      Some(&2),
      "New LRU of protected should be 2"
    );

    // 5. Calling evict(0) now should be a no-op as capacities are correct.
    let (victims, cost_freed) = <Slru<i32> as CachePolicy<i32, String>>::evict(&policy, 0);
    assert!(victims.is_empty());
    assert_eq!(cost_freed, 0);
  }

  #[test]
  fn peek_lru_on_empty_returns_none() {
    let policy: Slru<i32> = Slru::new(10);
    assert!(policy.peek_lru().is_none());
  }

  #[test]
  fn clear_removes_all_items_from_both_segments() {
    let policy: Slru<i32> = Slru::new(10);
    let entry1 = Arc::new(CacheEntry::new("v".to_string(), 1, None, None));
    let entry2 = Arc::new(CacheEntry::new("v".to_string(), 1, None, None));

    policy.on_admit(&access_info_for(&1, &entry1)); // in prob
    policy.on_admit(&access_info_for(&2, &entry2));
    policy.on_access(&access_info_for(&2, &entry2)); // in prot

    assert_eq!(policy.probationary.lock().current_total_cost(), 1);
    assert_eq!(policy.protected.lock().current_total_cost(), 1);

    <Slru<i32> as CachePolicy<i32, String>>::clear(&policy);

    assert_eq!(policy.probationary.lock().current_total_cost(), 0);
    assert_eq!(policy.protected.lock().current_total_cost(), 0);
    assert!(policy.probationary.lock().keys.is_empty());
    assert!(policy.protected.lock().keys.is_empty());
    assert!(!policy.probationary.lock().contains(&1));
    assert!(!policy.protected.lock().contains(&2));
  }
}

#[cfg(test)]
mod lru_list_tests {
  use super::*;

  #[test]
  fn new_list_is_empty() {
    let list = LruList::<i32>::new();
    assert!(list.keys.is_empty(), "New list keys should be empty");
    assert!(
      list.lookup.is_empty(),
      "New list lookup map should be empty"
    );
    assert_eq!(list.current_total_cost(), 0, "New list cost should be zero");
    assert!(!list.contains(&123), "New list should not contain any key");
  }

  #[test]
  fn push_front_new_items() {
    let mut list = LruList::new();

    // 1. Push first item
    list.push_front(10, 5);
    assert!(list.contains(&10));
    assert_eq!(list.current_total_cost(), 5);
    assert_eq!(list.keys.len(), 1);
    assert_eq!(list.lookup.len(), 1);
    assert_eq!(list.keys.front(), Some(&10));
    assert_eq!(list.lookup.get(&10), Some(&5));

    // 2. Push second item
    list.push_front(20, 2);
    assert!(list.contains(&20));
    assert_eq!(list.current_total_cost(), 7, "Cost should be 5 + 2");
    assert_eq!(list.keys.len(), 2);
    assert_eq!(list.lookup.len(), 2);
    assert_eq!(
      list.keys.iter().cloned().collect::<Vec<_>>(),
      vec![20, 10],
      "Newest item should be at the front"
    );
  }

  #[test]
  fn push_front_existing_item_moves_to_front() {
    let mut list = LruList::new();
    list.push_front(1, 1);
    list.push_front(2, 1);
    list.push_front(3, 1);
    assert_eq!(list.keys.iter().cloned().collect::<Vec<_>>(), vec![3, 2, 1]);
    assert_eq!(list.current_total_cost(), 3);

    // Re-push '1' (the LRU item). It should move to the front. Cost is unchanged.
    list.push_front(1, 1);
    assert_eq!(list.current_total_cost(), 3, "Cost should not change");
    assert_eq!(list.keys.len(), 3, "Length should not change");
    assert_eq!(
      list.keys.iter().cloned().collect::<Vec<_>>(),
      vec![1, 3, 2],
      "Existing item should move to front"
    );
  }

  #[test]
  fn push_front_existing_item_updates_cost() {
    let mut list = LruList::new();
    list.push_front(1, 10);
    list.push_front(2, 20);
    assert_eq!(list.current_total_cost(), 30);

    // Re-push '1' with a new cost. It should move to the front and cost should be updated.
    // New cost = (30 - 10) + 5 = 25
    list.push_front(1, 5);
    assert_eq!(list.current_total_cost(), 25, "Cost should be updated");
    assert_eq!(
      list.lookup.get(&1),
      Some(&5),
      "Lookup cost should be new cost"
    );
    assert_eq!(
      list.keys.iter().cloned().collect::<Vec<_>>(),
      vec![1, 2],
      "Order should be updated"
    );
  }

  #[test]
  fn pop_back_from_non_empty_list() {
    let mut list = LruList::new();
    list.push_front(1, 1); // This will be the LRU item
    list.push_front(2, 2);
    list.push_front(3, 3);
    assert_eq!(list.current_total_cost(), 6);

    // Pop the LRU item (1)
    let popped = list.pop_back();
    assert_eq!(
      popped,
      Some((1, 1)),
      "pop_back should return the LRU key and its cost"
    );

    // Verify internal state
    assert_eq!(
      list.current_total_cost(),
      5,
      "Cost should be reduced by popped item's cost"
    );
    assert!(!list.contains(&1), "Popped item should be removed");
    assert_eq!(list.keys.len(), 2);
    assert_eq!(
      list.keys.iter().cloned().collect::<Vec<_>>(),
      vec![3, 2],
      "Remaining items should be correct"
    );
  }

  #[test]
  fn pop_back_from_single_item_list() {
    let mut list = LruList::new();
    list.push_front(1, 10);

    let popped = list.pop_back();
    assert_eq!(popped, Some((1, 10)));
    assert_eq!(list.current_total_cost(), 0);
    assert!(list.keys.is_empty());
    assert!(list.lookup.is_empty());
  }

  #[test]
  fn pop_back_from_empty_list() {
    let mut list = LruList::<i32>::new();
    assert_eq!(list.pop_back(), None, "pop_back on empty list returns None");
  }

  #[test]
  fn remove_item_from_middle() {
    let mut list = LruList::new();
    list.push_front(1, 1);
    list.push_front(2, 2);
    list.push_front(3, 3);
    assert_eq!(list.current_total_cost(), 6);

    // Remove item from the middle
    let removed_cost = list.remove(&2);
    assert_eq!(removed_cost, Some(2));

    // Verify state
    assert_eq!(list.current_total_cost(), 4, "Cost should be 6 - 2");
    assert!(!list.contains(&2));
    assert_eq!(list.keys.len(), 2);
    assert_eq!(list.lookup.len(), 2);
    assert_eq!(list.keys.iter().cloned().collect::<Vec<_>>(), vec![3, 1]);
  }

  #[test]
  fn remove_non_existent_item() {
    let mut list = LruList::new();
    list.push_front(1, 1);
    list.push_front(2, 2);

    let removed_cost = list.remove(&99);
    assert_eq!(removed_cost, None);
    assert_eq!(list.current_total_cost(), 3, "Cost should not change");
    assert_eq!(list.keys.len(), 2, "Length should not change");
  }

  #[test]
  fn clear_resets_list() {
    let mut list = LruList::new();
    list.push_front(1, 10);
    list.push_front(2, 20);
    list.push_front(3, 30);
    assert_eq!(list.current_total_cost(), 60);
    assert!(!list.keys.is_empty());

    list.clear();

    assert!(list.keys.is_empty());
    assert!(list.lookup.is_empty());
    assert_eq!(list.current_total_cost(), 0);
    assert!(!list.contains(&1));
  }
}
