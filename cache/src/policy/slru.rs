use super::CachePolicy;
use crate::policy::lru_list::LruList;
use crate::policy::AdmissionDecision;

use parking_lot::Mutex;
use std::hash::Hash;

/// An eviction policy based on the Segmented LRU algorithm.
/// It maintains a probationary and a protected segment to resist cache scans.
#[derive(Debug)]
pub struct SlruPolicy<K: Eq + Hash + Clone> {
  pub(super) probationary: Mutex<LruList<K>>,
  pub(super) protected: Mutex<LruList<K>>,
  pub(crate) prob_capacity: u64,
  pub(crate) prot_capacity: u64,
}

impl<K: Eq + Hash + Clone> SlruPolicy<K> {
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
    if let Some(tail_idx) = prob_guard.tail {
      return Some(prob_guard.nodes[tail_idx].key.clone());
    }

    // If probationary is empty, the victim is from the protected segment's LRU end.
    let prot_guard = self.protected.lock();
    if let Some(tail_idx) = prot_guard.tail {
      return Some(prot_guard.nodes[tail_idx].key.clone());
    }

    None
  }

  //// Internal method for admitting a key with a known cost,
  /// used when the full CacheEntry<V> isn't available or needed.
  pub(crate) fn admit_internal(&self, key: K, cost: u64) {
    let mut prob_guard = self.probationary.lock();
    let prot_guard = self.protected.lock(); // Keep lock to ensure consistency with contains check

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

    // If it's in protected, refresh it and update its cost.
    if prot_guard.contains(key) {
      prot_guard.push_front(key.clone(), cost);
      return;
    }
    // If it's in probationary, promote it to protected with the new cost.
    if prob_guard.remove(key).is_some() {
      prot_guard.push_front(key.clone(), cost);
      self.maintain_capacities(&mut prob_guard, &mut prot_guard);
    }
    // If not in either, it's a new admission, which should go through admit_internal.
  }
}

impl<K, V> CachePolicy<K, V> for SlruPolicy<K>
where
  K: Eq + Hash + Clone + Send + Sync,
  V: Send + Sync,
{
  fn on_access(&self, key: &K, cost: u64) {
    self.access_internal(key, cost);
  }

  fn on_admit(&self, key: &K, cost: u64) -> AdmissionDecision<K> {
    let mut prob_guard = self.probationary.lock();
    let prot_guard = self.protected.lock();

    // If item is already in the cache, on_access will handle it.
    // Here we only handle new items.
    if !prot_guard.contains(key) && !prob_guard.contains(key) {
      prob_guard.push_front(key.clone(), cost);
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

  #[test]
  fn new_item_goes_to_probationary() {
    let policy: SlruPolicy<i32> = SlruPolicy::new(10);
    let entry = Arc::new(CacheEntry::new("v".to_string(), 1, None, None));
    <SlruPolicy<i32> as CachePolicy<i32, String>>::on_admit(&policy, &1, entry.cost());

    assert!(policy.probationary.lock().contains(&1));
    assert!(!policy.protected.lock().contains(&1));
    assert_eq!(policy.probationary.lock().current_total_cost(), 1);
  }

  #[test]
  fn access_promotes_from_probationary_to_protected() {
    let policy: SlruPolicy<i32> = SlruPolicy::new(10);
    let entry = Arc::new(CacheEntry::new("v".to_string(), 1, None, None));
    <SlruPolicy<i32> as CachePolicy<i32, String>>::on_admit(&policy, &1, entry.cost());

    <SlruPolicy<i32> as CachePolicy<i32, String>>::on_access(&policy, &1, entry.cost());

    assert!(!policy.probationary.lock().contains(&1));
    assert!(policy.protected.lock().contains(&1));
    assert_eq!(policy.protected.lock().current_total_cost(), 1);
  }

  #[test]
  fn access_refreshes_item_in_protected() {
    let policy: SlruPolicy<i32> = SlruPolicy::new(10); // prob=2, prot=8
    let entry1 = Arc::new(CacheEntry::new("v".to_string(), 1, None, None));
    <SlruPolicy<i32> as CachePolicy<i32, String>>::on_admit(&policy, &1, entry1.cost());
    <SlruPolicy<i32> as CachePolicy<i32, String>>::on_access(&policy, &1, entry1.cost()); // 1 is now in protected

    let entry2 = Arc::new(CacheEntry::new("v".to_string(), 1, None, None));
    <SlruPolicy<i32> as CachePolicy<i32, String>>::on_admit(&policy, &2, entry2.cost());
    <SlruPolicy<i32> as CachePolicy<i32, String>>::on_access(&policy, &2, entry2.cost()); // 2 is now in protected, LRU of protected is 1

    let protected_keys = policy.protected.lock().keys_as_vec();
    assert_eq!(protected_keys.last(), Some(&1));

    <SlruPolicy<i32> as CachePolicy<i32, String>>::on_access(&policy, &1, entry1.cost()); // Access 1 again

    // Now 2 should be the LRU of protected
    let protected_keys_after = policy.protected.lock().keys_as_vec();
    assert_eq!(protected_keys_after.last(), Some(&2));
  }

  #[test]
  fn promotion_causes_demotion_when_protected_is_full() {
    let policy: SlruPolicy<i32> = SlruPolicy::new(5); // prob=1, prot=4

    // 1. Fill protected segment
    let entries: Vec<_> = (1..=5)
      .map(|_| Arc::new(CacheEntry::new("v".to_string(), 1, None, None)))
      .collect();
    for i in 1..=4 {
      <SlruPolicy<i32> as CachePolicy<i32, String>>::on_admit(
        &policy,
        &i,
        entries[(i - 1) as usize].cost(),
      );
      <SlruPolicy<i32> as CachePolicy<i32, String>>::on_access(&policy, &i, entries[(i - 1) as usize].cost());
    }
    assert_eq!(policy.protected.lock().current_total_cost(), 4);
    assert_eq!(policy.probationary.lock().current_total_cost(), 0);

    // 2. Insert an item into probationary
    <SlruPolicy<i32> as CachePolicy<i32, String>>::on_admit(&policy, &5, entries[4].cost());
    assert_eq!(policy.probationary.lock().current_total_cost(), 1);

    // 3. Access item 5 to promote it. This should cause item 1 (LRU of protected) to be demoted.
    <SlruPolicy<i32> as CachePolicy<i32, String>>::on_access(&policy, &5, entries[4].cost());

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
    let policy: SlruPolicy<i32> = SlruPolicy::new(5); // prob=1, prot=4
    let entries: Vec<_> = (1..=5)
      .map(|_| Arc::new(CacheEntry::new("v".to_string(), 1, None, None)))
      .collect();
    for i in 1..=5 {
      <SlruPolicy<i32> as CachePolicy<i32, String>>::on_admit(
        &policy,
        &i,
        entries[(i - 1) as usize].cost(),
      );
    }
    // Prob has [5, 4, 3, 2, 1], but capacity is 1. Protected is empty.

    let (victims, _) = <SlruPolicy<i32> as CachePolicy<i32, String>>::evict(&policy, 1);

    // The `evict` call first calls `maintain_capacities`, then evicts from probationary.
    // The LRU item is 1.
    assert_eq!(victims, vec![1]);
    assert!(!policy.probationary.lock().contains(&1));
  }

  #[test]
  fn evict_from_protected_if_probationary_is_empty() {
    let policy: SlruPolicy<i32> = SlruPolicy::new(5); // prob=1, prot=4
    let entries: Vec<_> = (1..=4)
      .map(|_| Arc::new(CacheEntry::new("v".to_string(), 1, None, None)))
      .collect();
    for i in 1..=4 {
      <SlruPolicy<i32> as CachePolicy<i32, String>>::on_admit(
        &policy,
        &i,
        entries[(i - 1) as usize].cost(),
      );
      <SlruPolicy<i32> as CachePolicy<i32, String>>::on_access(&policy, &i, entries[(i - 1) as usize].cost());
    }
    // Now protected has [4, 3, 2, 1], probationary is empty.

    let (victims, _) = <SlruPolicy<i32> as CachePolicy<i32, String>>::evict(&policy, 1);

    // LRU of protected is 1.
    assert_eq!(victims, vec![1]);
    assert!(!policy.protected.lock().contains(&1));
  }

  #[test]
  fn on_remove_cleans_up_state() {
    let policy: SlruPolicy<i32> = SlruPolicy::new(10);
    let entry1 = Arc::new(CacheEntry::new("v".to_string(), 1, None, None));
    let entry2 = Arc::new(CacheEntry::new("v".to_string(), 1, None, None));
    <SlruPolicy<i32> as CachePolicy<i32, String>>::on_admit(&policy, &1, entry1.cost()); // in prob
    <SlruPolicy<i32> as CachePolicy<i32, String>>::on_admit(&policy, &2, entry2.cost());
    <SlruPolicy<i32> as CachePolicy<i32, String>>::on_access(&policy, &2, entry2.cost()); // in prot

    <SlruPolicy<i32> as CachePolicy<i32, String>>::on_remove(&policy, &1);
    assert!(!policy.probationary.lock().contains(&1));

    <SlruPolicy<i32> as CachePolicy<i32, String>>::on_remove(&policy, &2);
    assert!(!policy.protected.lock().contains(&2));

    assert_eq!(policy.probationary.lock().current_total_cost(), 0);
    assert_eq!(policy.protected.lock().current_total_cost(), 0);
  }

  #[test]
  fn peek_lru_finds_correct_victim() {
    let policy: SlruPolicy<i32> = SlruPolicy::new(5); // prob=1, prot=4
    let entries: Vec<_> = (1..=2)
      .map(|_| Arc::new(CacheEntry::new("v".to_string(), 1, None, None)))
      .collect();

    // 1. Victim in probationary
    <SlruPolicy<i32> as CachePolicy<i32, String>>::on_admit(&policy, &1, entries[0].cost());
    <SlruPolicy<i32> as CachePolicy<i32, String>>::on_admit(&policy, &2, entries[1].cost());
    assert_eq!(
      policy.peek_lru(),
      Some(1),
      "LRU of probationary should be the victim"
    );

    // 2. Victim in protected (after probationary is empty)
    <SlruPolicy<i32> as CachePolicy<i32, String>>::on_access(&policy, &1, entries[0].cost());
    <SlruPolicy<i32> as CachePolicy<i32, String>>::on_access(&policy, &2, entries[1].cost());
    assert_eq!(
      policy.peek_lru(),
      Some(1),
      "LRU of protected should be the victim"
    );

    // 3. Re-accessing changes the victim
    <SlruPolicy<i32> as CachePolicy<i32, String>>::on_access(&policy, &1, entries[0].cost());
    assert_eq!(
      policy.peek_lru(),
      Some(2),
      "After re-access, new LRU of protected is the victim"
    );
  }

  #[test]
  fn slru_with_small_capacity_allocates_correctly() {
    // Capacity 5 -> 20% is 1. prob_cap=1, prot_cap=4.
    let policy_5 = SlruPolicy::<i32>::new(5);
    assert_eq!(policy_5.prob_capacity, 1);
    assert_eq!(policy_5.prot_capacity, 4);

    // Capacity 4 -> 20% is 0.8, rounds to 1. prob_cap=1, prot_cap=3.
    let policy_4 = SlruPolicy::<i32>::new(4);
    assert_eq!(policy_4.prob_capacity, 1);
    assert_eq!(policy_4.prot_capacity, 3);

    // Capacity 1 -> 20% is 0.2, rounds to 0, but is max(1). prob_cap=1, prot_cap=0.
    let policy_1 = SlruPolicy::<i32>::new(1);
    assert_eq!(policy_1.prob_capacity, 1);
    assert_eq!(policy_1.prot_capacity, 0);

    // Capacity 0 -> prob_cap=0, prot_cap=0.
    let policy_0 = SlruPolicy::<i32>::new(0);
    assert_eq!(policy_0.prob_capacity, 0);
    assert_eq!(policy_0.prot_capacity, 0);
  }

  #[test]
  fn reinsert_of_existing_key_is_noop() {
    let policy: SlruPolicy<i32> = SlruPolicy::new(10);
    let entry = Arc::new(CacheEntry::new("v".to_string(), 1, None, None));

    // 1. Insert into probationary
    <SlruPolicy<i32> as CachePolicy<i32, String>>::on_admit(&policy, &1, entry.cost());
    let prob_keys_before = policy.probationary.lock().keys_as_vec();

    // Re-inserting should do nothing
    <SlruPolicy<i32> as CachePolicy<i32, String>>::on_admit(&policy, &1, entry.cost());
    let prob_keys_after = policy.probationary.lock().keys_as_vec();
    assert_eq!(
      prob_keys_before, prob_keys_after,
      "Re-inserting into probationary should not change order"
    );

    // 2. Promote to protected
    <SlruPolicy<i32> as CachePolicy<i32, String>>::on_access(&policy, &1, entry.cost());
    let prot_keys_before = policy.protected.lock().keys_as_vec();

    // Re-inserting should still do nothing
    <SlruPolicy<i32> as CachePolicy<i32, String>>::on_admit(&policy, &1, entry.cost());
    let prot_keys_after = policy.protected.lock().keys_as_vec();
    assert_eq!(
      prot_keys_before, prot_keys_after,
      "Re-inserting into protected should not change order"
    );
  }

  #[test]
  fn evict_empties_probationary_then_protected() {
    let policy: SlruPolicy<i32> = SlruPolicy::new(4); // prob=1, prot=3
    let entries: Vec<_> = (1..=4)
      .map(|_| Arc::new(CacheEntry::new("v".to_string(), 1, None, None)))
      .collect();

    // Fill protected
    for i in 1..=3 {
      <SlruPolicy<i32> as CachePolicy<i32, String>>::on_admit(
        &policy,
        &i,
        entries[(i - 1) as usize].cost(),
      );
      <SlruPolicy<i32> as CachePolicy<i32, String>>::on_access(&policy, &i, entries[(i - 1) as usize].cost());
    }
    // Fill probationary
    <SlruPolicy<i32> as CachePolicy<i32, String>>::on_admit(&policy, &4, entries[3].cost());

    assert_eq!(policy.probationary.lock().current_total_cost(), 1);
    assert_eq!(policy.protected.lock().current_total_cost(), 3);

    // Evict 3 items. Should take 1 from prob, and 2 from prot.
    // Prob victim: 4. Prot victims: 1, 2.
    let (victims, cost_freed) = <SlruPolicy<i32> as CachePolicy<i32, String>>::evict(&policy, 3);

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
    let policy: SlruPolicy<i32> = SlruPolicy::new(5);
    let entries: Vec<_> = (1..=5)
      .map(|_| Arc::new(CacheEntry::new("v".to_string(), 1, None, None)))
      .collect();
    for i in 1..=5 {
      <SlruPolicy<i32> as CachePolicy<i32, String>>::on_admit(
        &policy,
        &i,
        entries[(i - 1) as usize].cost(),
      );
    }
    <SlruPolicy<i32> as CachePolicy<i32, String>>::on_access(&policy, &5, entries[4].cost()); // Promote 5

    // Request to free a huge amount of cost
    let (victims, cost_freed) = <SlruPolicy<i32> as CachePolicy<i32, String>>::evict(&policy, 100);

    assert_eq!(cost_freed, 5, "Should free the cost of all items");
    assert_eq!(victims.len(), 5, "Should evict all 5 items");

    assert_eq!(policy.probationary.lock().current_total_cost(), 0);
    assert_eq!(policy.protected.lock().current_total_cost(), 0);
    assert!(policy.peek_lru().is_none());
  }

  #[test]
  fn evict_with_zero_cost_triggers_demotion() {
    let policy: SlruPolicy<i32> = SlruPolicy::new(5); // prob=1, prot=4

    // 1. Fill protected segment to its capacity of 4
    let entries: Vec<_> = (1..=5)
      .map(|_| Arc::new(CacheEntry::new("v".to_string(), 1, None, None)))
      .collect();
    for i in 1..=4 {
      <SlruPolicy<i32> as CachePolicy<i32, String>>::on_admit(
        &policy,
        &i,
        entries[(i - 1) as usize].cost(),
      );
      <SlruPolicy<i32> as CachePolicy<i32, String>>::on_access(&policy, &i, entries[(i - 1) as usize].cost());
    }
    assert_eq!(policy.protected.lock().current_total_cost(), 4);
    assert_eq!(policy.probationary.lock().current_total_cost(), 0);
    let protected_keys = policy.protected.lock().keys_as_vec();
    assert_eq!(
      protected_keys.last(),
      Some(&1),
      "LRU of protected is 1 initially"
    );

    // 2. Insert a new item into probationary
    <SlruPolicy<i32> as CachePolicy<i32, String>>::on_admit(&policy, &5, entries[4].cost());
    assert_eq!(policy.probationary.lock().current_total_cost(), 1);

    // 3. Act: Access the probationary item to promote it.
    // This action should synchronously:
    //    a) Move item 5 to protected (cost becomes 5, over capacity).
    //    b) Trigger `maintain_capacities`, demoting item 1 to probationary.
    <SlruPolicy<i32> as CachePolicy<i32, String>>::on_access(&policy, &5, entries[4].cost());

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
    let protected_keys_after = policy.protected.lock().keys_as_vec();
    assert_eq!(
      protected_keys_after.last(),
      Some(&2),
      "New LRU of protected should be 2"
    );

    // 5. Calling evict(0) now should be a no-op as capacities are correct.
    let (victims, cost_freed) = <SlruPolicy<i32> as CachePolicy<i32, String>>::evict(&policy, 0);
    assert!(victims.is_empty());
    assert_eq!(cost_freed, 0);
  }

  #[test]
  fn peek_lru_on_empty_returns_none() {
    let policy: SlruPolicy<i32> = SlruPolicy::new(10);
    assert!(policy.peek_lru().is_none());
  }

  #[test]
  fn clear_removes_all_items_from_both_segments() {
    let policy: SlruPolicy<i32> = SlruPolicy::new(10);
    let entry1 = Arc::new(CacheEntry::new("v".to_string(), 1, None, None));
    let entry2 = Arc::new(CacheEntry::new("v".to_string(), 1, None, None));

    <SlruPolicy<i32> as CachePolicy<i32, String>>::on_admit(&policy, &1, entry1.cost()); // in prob
    <SlruPolicy<i32> as CachePolicy<i32, String>>::on_admit(&policy, &2, entry2.cost());
    <SlruPolicy<i32> as CachePolicy<i32, String>>::on_access(&policy, &2, entry2.cost()); // in prot

    assert_eq!(policy.probationary.lock().current_total_cost(), 1);
    assert_eq!(policy.protected.lock().current_total_cost(), 1);

    <SlruPolicy<i32> as CachePolicy<i32, String>>::clear(&policy);

    assert_eq!(policy.probationary.lock().current_total_cost(), 0);
    assert_eq!(policy.protected.lock().current_total_cost(), 0);
    assert!(policy.probationary.lock().keys_as_vec().is_empty());
    assert!(policy.protected.lock().keys_as_vec().is_empty());
    assert!(!policy.probationary.lock().contains(&1));
    assert!(!policy.protected.lock().contains(&2));
  }
}