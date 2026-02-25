use super::CachePolicy;
use crate::policy::lru_list::LruList;
use crate::policy::AdmissionDecision;

use parking_lot::Mutex;
use std::hash::Hash;

/// The mutable state of the SLRU algorithm, held under a single lock.
#[derive(Debug)]
pub(crate) struct SlruState<K: Eq + Hash + Clone> {
  pub(crate) probationary: LruList<K>,
  pub(crate) protected: LruList<K>,
}

impl<K: Eq + Hash + Clone> SlruState<K> {
  fn new() -> Self {
    Self {
      probationary: LruList::new(),
      protected: LruList::new(),
    }
  }

  /// Construct a standalone SlruState whose capacity ratios mirror SlruPolicy.
  /// Used by TinyLfuPolicy, which embeds SlruState directly.
  pub(crate) fn new_with_capacity(_capacity: u64) -> Self {
    Self::new()
  }

  fn maintain_capacities(&mut self, prot_capacity: u64) {
    while self.protected.current_total_cost() > prot_capacity {
      if let Some((key, cost)) = self.protected.pop_back() {
        self.probationary.push_front(key, cost);
      } else {
        break;
      }
    }
  }

  pub(crate) fn peek_lru(&self) -> Option<K> {
    if let Some(tail_idx) = self.probationary.tail {
      return Some(self.probationary.nodes[tail_idx].key.clone());
    }
    if let Some(tail_idx) = self.protected.tail {
      return Some(self.protected.nodes[tail_idx].key.clone());
    }
    None
  }

  pub(crate) fn admit_internal(&mut self, key: K, cost: u64) {
    if !self.protected.contains(&key) && !self.probationary.contains(&key) {
      self.probationary.push_front(key, cost);
    } else if self.probationary.contains(&key) {
      self.probationary.push_front(key, cost);
    }
    // If already in protected, on_access handles it.
  }

  pub(crate) fn access_internal(&mut self, key: &K, cost: u64, prot_capacity: u64) {
    if self.protected.contains(key) {
      self.protected.push_front(key.clone(), cost);
      return;
    }
    if self.probationary.remove(key).is_some() {
      self.protected.push_front(key.clone(), cost);
      self.maintain_capacities(prot_capacity);
    }
  }

  pub(crate) fn evict_items(&mut self, mut cost_to_free: u64, prot_capacity: u64) -> (Vec<K>, u64) {
    let mut victims = Vec::new();
    let mut total_cost_freed = 0;

    self.maintain_capacities(prot_capacity);

    while cost_to_free > 0 {
      if let Some((key, cost)) = self.probationary.pop_back() {
        cost_to_free = cost_to_free.saturating_sub(cost);
        total_cost_freed += cost;
        victims.push(key);
      } else {
        break;
      }
    }

    while cost_to_free > 0 {
      if let Some((key, cost)) = self.protected.pop_back() {
        cost_to_free = cost_to_free.saturating_sub(cost);
        total_cost_freed += cost;
        victims.push(key);
      } else {
        break;
      }
    }

    (victims, total_cost_freed)
  }
}

/// An eviction policy based on the Segmented LRU algorithm.
/// It maintains a probationary and a protected segment to resist cache scans.
#[derive(Debug)]
pub struct SlruPolicy<K: Eq + Hash + Clone> {
  state: Mutex<SlruState<K>>,
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
      state: Mutex::new(SlruState::new()),
      prob_capacity,
      prot_capacity,
    }
  }

  pub(crate) fn peek_lru(&self) -> Option<K> {
    self.state.lock().peek_lru()
  }

  pub(crate) fn admit_internal(&self, key: K, cost: u64) {
    self.state.lock().admit_internal(key, cost);
  }

  pub(crate) fn access_internal(&self, key: &K, cost: u64) {
    self.state.lock().access_internal(key, cost, self.prot_capacity);
  }
}

impl<K, V> CachePolicy<K, V> for SlruPolicy<K>
where
  K: Eq + Hash + Clone + Send + Sync,
  V: Send + Sync,
{
  fn on_access(&self, key: &K, cost: u64) {
    self.state.lock().access_internal(key, cost, self.prot_capacity);
  }

  fn on_admit(&self, key: &K, cost: u64) -> AdmissionDecision<K> {
    let mut state = self.state.lock();
    if !state.protected.contains(key) && !state.probationary.contains(key) {
      state.probationary.push_front(key.clone(), cost);
    }
    AdmissionDecision::Admit
  }

  fn on_remove(&self, key: &K) {
    let mut state = self.state.lock();
    if state.probationary.remove(key).is_some() {
      return;
    }
    state.protected.remove(key);
  }

  fn evict(&self, cost_to_free: u64) -> (Vec<K>, u64) {
    self.state.lock().evict_items(cost_to_free, self.prot_capacity)
  }

  fn clear(&self) {
    let mut state = self.state.lock();
    state.probationary.clear();
    state.protected.clear();
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

    assert!(policy.state.lock().probationary.contains(&1));
    assert!(!policy.state.lock().protected.contains(&1));
    assert_eq!(policy.state.lock().probationary.current_total_cost(), 1);
  }

  #[test]
  fn access_promotes_from_probationary_to_protected() {
    let policy: SlruPolicy<i32> = SlruPolicy::new(10);
    let entry = Arc::new(CacheEntry::new("v".to_string(), 1, None, None));
    <SlruPolicy<i32> as CachePolicy<i32, String>>::on_admit(&policy, &1, entry.cost());

    <SlruPolicy<i32> as CachePolicy<i32, String>>::on_access(&policy, &1, entry.cost());

    assert!(!policy.state.lock().probationary.contains(&1));
    assert!(policy.state.lock().protected.contains(&1));
    assert_eq!(policy.state.lock().protected.current_total_cost(), 1);
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

    let protected_keys = policy.state.lock().protected.keys_as_vec();
    assert_eq!(protected_keys.last(), Some(&1));

    <SlruPolicy<i32> as CachePolicy<i32, String>>::on_access(&policy, &1, entry1.cost()); // Access 1 again

    // Now 2 should be the LRU of protected
    let protected_keys_after = policy.state.lock().protected.keys_as_vec();
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
    assert_eq!(policy.state.lock().protected.current_total_cost(), 4);
    assert_eq!(policy.state.lock().probationary.current_total_cost(), 0);

    // 2. Insert an item into probationary
    <SlruPolicy<i32> as CachePolicy<i32, String>>::on_admit(&policy, &5, entries[4].cost());
    assert_eq!(policy.state.lock().probationary.current_total_cost(), 1);

    // 3. Access item 5 to promote it. This should cause item 1 (LRU of protected) to be demoted.
    <SlruPolicy<i32> as CachePolicy<i32, String>>::on_access(&policy, &5, entries[4].cost());

    assert!(policy.state.lock().protected.contains(&5), "5 should be promoted");
    assert!(
      !policy.state.lock().protected.contains(&1),
      "1 should no longer be in protected"
    );
    assert!(
      policy.state.lock().probationary.contains(&1),
      "1 should be demoted to probationary"
    );
    assert_eq!(policy.state.lock().protected.current_total_cost(), 4);
    assert_eq!(policy.state.lock().probationary.current_total_cost(), 1);
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
    assert!(!policy.state.lock().probationary.contains(&1));
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
    assert!(!policy.state.lock().protected.contains(&1));
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
    assert!(!policy.state.lock().probationary.contains(&1));

    <SlruPolicy<i32> as CachePolicy<i32, String>>::on_remove(&policy, &2);
    assert!(!policy.state.lock().protected.contains(&2));

    assert_eq!(policy.state.lock().probationary.current_total_cost(), 0);
    assert_eq!(policy.state.lock().protected.current_total_cost(), 0);
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
    let prob_keys_before = policy.state.lock().probationary.keys_as_vec();

    // Re-inserting should do nothing
    <SlruPolicy<i32> as CachePolicy<i32, String>>::on_admit(&policy, &1, entry.cost());
    let prob_keys_after = policy.state.lock().probationary.keys_as_vec();
    assert_eq!(
      prob_keys_before, prob_keys_after,
      "Re-inserting into probationary should not change order"
    );

    // 2. Promote to protected
    <SlruPolicy<i32> as CachePolicy<i32, String>>::on_access(&policy, &1, entry.cost());
    let prot_keys_before = policy.state.lock().protected.keys_as_vec();

    // Re-inserting should still do nothing
    <SlruPolicy<i32> as CachePolicy<i32, String>>::on_admit(&policy, &1, entry.cost());
    let prot_keys_after = policy.state.lock().protected.keys_as_vec();
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

    assert_eq!(policy.state.lock().probationary.current_total_cost(), 1);
    assert_eq!(policy.state.lock().protected.current_total_cost(), 3);

    // Evict 3 items. Should take 1 from prob, and 2 from prot.
    // Prob victim: 4. Prot victims: 1, 2.
    let (victims, cost_freed) = <SlruPolicy<i32> as CachePolicy<i32, String>>::evict(&policy, 3);

    assert_eq!(cost_freed, 3);
    assert_eq!(
      victims,
      vec![4, 1, 2],
      "Eviction order should be prob-lru then prot-lru"
    );

    assert_eq!(policy.state.lock().probationary.current_total_cost(), 0);
    assert_eq!(policy.state.lock().protected.current_total_cost(), 1);
    assert!(policy.state.lock().protected.contains(&3));
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

    assert_eq!(policy.state.lock().probationary.current_total_cost(), 0);
    assert_eq!(policy.state.lock().protected.current_total_cost(), 0);
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
    assert_eq!(policy.state.lock().protected.current_total_cost(), 4);
    assert_eq!(policy.state.lock().probationary.current_total_cost(), 0);
    let protected_keys = policy.state.lock().protected.keys_as_vec();
    assert_eq!(
      protected_keys.last(),
      Some(&1),
      "LRU of protected is 1 initially"
    );

    // 2. Insert a new item into probationary
    <SlruPolicy<i32> as CachePolicy<i32, String>>::on_admit(&policy, &5, entries[4].cost());
    assert_eq!(policy.state.lock().probationary.current_total_cost(), 1);

    // 3. Act: Access the probationary item to promote it.
    <SlruPolicy<i32> as CachePolicy<i32, String>>::on_access(&policy, &5, entries[4].cost());

    // 4. Assert: Check the state *immediately* after the access.
    assert_eq!(
      policy.state.lock().protected.current_total_cost(),
      4,
      "Protected cost should be brought back to capacity by demotion"
    );
    assert_eq!(
      policy.state.lock().probationary.current_total_cost(),
      1,
      "Probationary should now have the demoted item"
    );

    assert!(
      policy.state.lock().protected.contains(&5),
      "Item 5 should have been promoted"
    );
    assert!(
      !policy.state.lock().protected.contains(&1),
      "Item 1 should no longer be in protected"
    );
    assert!(
      policy.state.lock().probationary.contains(&1),
      "Item 1 should have been demoted to probationary"
    );
    let protected_keys_after = policy.state.lock().protected.keys_as_vec();
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

    assert_eq!(policy.state.lock().probationary.current_total_cost(), 1);
    assert_eq!(policy.state.lock().protected.current_total_cost(), 1);

    <SlruPolicy<i32> as CachePolicy<i32, String>>::clear(&policy);

    assert_eq!(policy.state.lock().probationary.current_total_cost(), 0);
    assert_eq!(policy.state.lock().protected.current_total_cost(), 0);
    assert!(policy.state.lock().probationary.keys_as_vec().is_empty());
    assert!(policy.state.lock().protected.keys_as_vec().is_empty());
    assert!(!policy.state.lock().probationary.contains(&1));
    assert!(!policy.state.lock().protected.contains(&2));
  }
}
