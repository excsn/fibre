use std::{collections::HashMap, hash::Hash};

use generational_arena::{Arena, Index};

#[derive(Debug)]
pub(super) struct Node<K> {
  pub(crate) key: K,
  pub(crate) cost: u64,
  pub(crate) next: Option<Index>,
  pub(crate) prev: Option<Index>,
}

// A self-contained, cost-based LRU list helper.
// The internal implementation is completely replaced.
#[derive(Debug)]
pub(super) struct LruList<K: Eq + Hash + Clone> {
  // Arena stores all nodes contiguously.
  pub(crate) nodes: Arena<Node<K>>,
  // HashMap for O(1) lookup of a key to its node index in the arena.
  pub(crate) lookup: HashMap<K, Index>,
  // Head is the most-recently-used item.
  pub(crate) head: Option<Index>,
  // Tail is the least-recently-used item.
  pub(crate) tail: Option<Index>,
  // Total cost of all items in the list.
  pub(crate) current_cost: u64,
}

impl<K: Eq + Hash + Clone> LruList<K> {
  pub fn new() -> Self {
    Self {
      nodes: Arena::new(),
      lookup: HashMap::new(),
      head: None,
      tail: None,
      current_cost: 0,
    }
  }

  // Helper to unlink a node from the list.
  // This is a private method as it doesn't handle arena/map removal.
  fn unlink(&mut self, index: Index) {
    let node = &self.nodes[index];
    let prev_node_idx = node.prev;
    let next_node_idx = node.next;

    // Update the 'next' pointer of the previous node.
    if let Some(prev_idx) = prev_node_idx {
      self.nodes[prev_idx].next = next_node_idx;
    } else {
      // We are unlinking the head of the list.
      self.head = next_node_idx;
    }

    // Update the 'prev' pointer of the next node.
    if let Some(next_idx) = next_node_idx {
      self.nodes[next_idx].prev = prev_node_idx;
    } else {
      // We are unlinking the tail of the list.
      self.tail = prev_node_idx;
    }
  }

  // Helper to push a node to the front (making it the new head).
  // This is a private method as it assumes the node is already in the arena.
  fn push_front_node(&mut self, index: Index) {
    let old_head_idx = self.head;
    self.nodes[index].next = old_head_idx;
    self.nodes[index].prev = None;
    self.head = Some(index);

    if let Some(old_head) = old_head_idx {
      self.nodes[old_head].prev = Some(index);
    }

    if self.tail.is_none() {
      self.tail = Some(index);
    }
  }

  pub fn contains(&self, key: &K) -> bool {
    self.lookup.contains_key(key)
  }

  pub fn current_total_cost(&self) -> u64 {
    self.current_cost
  }

  pub fn push_front(&mut self, key: K, cost: u64) {
    if let Some(&index) = self.lookup.get(&key) {
      // Item exists, update its cost and move to front.
      let old_cost = self.nodes[index].cost;
      self.current_cost = self.current_cost.saturating_sub(old_cost) + cost;
      self.nodes[index].cost = cost;
      self.move_to_front(&key);
    } else {
      // New item, insert it.
      let new_node = Node {
        key: key.clone(),
        cost,
        next: None,
        prev: None,
      };
      let index = self.nodes.insert(new_node);
      self.lookup.insert(key, index);
      self.current_cost += cost;
      self.push_front_node(index);
    }
  }

  pub fn move_to_front(&mut self, key: &K) {
    if let Some(&index) = self.lookup.get(key) {
      // Only move if it's not already the head.
      if self.head != Some(index) {
        self.unlink(index);
        self.push_front_node(index);
      }
    }
  }

  pub fn pop_back(&mut self) -> Option<(K, u64)> {
    if let Some(tail_index) = self.tail {
      let tail_node = self.nodes.get(tail_index).unwrap();
      let key = tail_node.key.clone();
      let cost = tail_node.cost;
      self.remove(&key);
      Some((key, cost))
    } else {
      None
    }
  }

  pub fn remove(&mut self, key: &K) -> Option<u64> {
    if let Some(index) = self.lookup.remove(key) {
      self.unlink(index);
      let node = self.nodes.remove(index).unwrap();
      self.current_cost = self.current_cost.saturating_sub(node.cost);
      Some(node.cost)
    } else {
      None
    }
  }

  pub fn clear(&mut self) {
    self.nodes.clear();
    self.lookup.clear();
    self.head = None;
    self.tail = None;
    self.current_cost = 0;
  }

  // A helper for tests, to get the order of keys from head to tail.
  #[cfg(test)]
  pub(crate) fn keys_as_vec(&self) -> Vec<K> {
    let mut keys = Vec::new();
    let mut current = self.head;
    while let Some(index) = current {
      keys.push(self.nodes[index].key.clone());
      current = self.nodes[index].next;
    }
    keys
  }
}

#[cfg(test)]
mod test {
  use super::*;

  #[test]
  fn new_list_is_empty() {
    let list = LruList::<i32>::new();
    assert!(list.keys_as_vec().is_empty(), "New list keys should be empty");
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
    assert_eq!(list.lookup.len(), 1);
    assert_eq!(list.keys_as_vec(), vec![10]);
    assert_eq!(list.lookup.get(&10).map(|&idx| list.nodes[idx].cost), Some(5));

    // 2. Push second item
    list.push_front(20, 2);
    assert!(list.contains(&20));
    assert_eq!(list.current_total_cost(), 7, "Cost should be 5 + 2");
    assert_eq!(list.lookup.len(), 2);
    assert_eq!(
      list.keys_as_vec(),
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
    assert_eq!(list.keys_as_vec(), vec![3, 2, 1]);
    assert_eq!(list.current_total_cost(), 3);

    // Re-push '1' (the LRU item). It should move to the front. Cost is unchanged.
    list.push_front(1, 1);
    assert_eq!(list.current_total_cost(), 3, "Cost should not change");
    assert_eq!(list.lookup.len(), 3, "Length should not change");
    assert_eq!(
      list.keys_as_vec(),
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
      list.lookup.get(&1).map(|&idx| list.nodes[idx].cost),
      Some(5),
      "Lookup cost should be new cost"
    );
    assert_eq!(
      list.keys_as_vec(),
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
    assert_eq!(list.lookup.len(), 2);
    assert_eq!(
      list.keys_as_vec(),
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
    assert!(list.keys_as_vec().is_empty());
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
    assert_eq!(list.lookup.len(), 2);
    assert_eq!(list.keys_as_vec(), vec![3, 1]);
  }

  #[test]
  fn remove_non_existent_item() {
    let mut list = LruList::new();
    list.push_front(1, 1);
    list.push_front(2, 2);

    let removed_cost = list.remove(&99);
    assert_eq!(removed_cost, None);
    assert_eq!(list.current_total_cost(), 3, "Cost should not change");
    assert_eq!(list.lookup.len(), 2, "Length should not change");
  }

  #[test]
  fn clear_resets_list() {
    let mut list = LruList::new();
    list.push_front(1, 10);
    list.push_front(2, 20);
    list.push_front(3, 30);
    assert_eq!(list.current_total_cost(), 60);
    assert!(!list.keys_as_vec().is_empty());

    list.clear();

    assert!(list.keys_as_vec().is_empty());
    assert!(list.lookup.is_empty());
    assert_eq!(list.current_total_cost(), 0);
    assert!(!list.contains(&1));
  }
}