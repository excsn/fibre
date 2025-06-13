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
pub(crate) struct LruList<K: Eq + Hash + Clone> {
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