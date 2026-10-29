//! S3-FIFO: a small FIFO for new keys, a main FIFO for keys read again, and a
//! ghost list of keys recently evicted from the small FIFO. Reads only raise a
//! per-key frequency; eviction works from the queue heads.

use crate::policy::{AdmissionDecision, CachePolicy};
use parking_lot::Mutex;
use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::Hash;

const FREQ_MAX: u8 = 3;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Queue {
  Small,
  Main,
}

struct Meta {
  cost: u64,
  freq: u8,
  queue: Queue,
  /// Taken from `State::next_stamp` each time the key is queued, so a stale
  /// queue entry never matches a later one, even after a remove and re-insert.
  stamp: u32,
}

struct State<K> {
  meta: HashMap<K, Meta>,
  small: VecDeque<(K, u32)>,
  main: VecDeque<(K, u32)>,
  small_cost: u64,
  total_cost: u64,
  ghost: VecDeque<K>,
  ghost_set: HashSet<K>,
  next_stamp: u32,
}

/// S3-FIFO eviction for one shard of `capacity` cost units.
pub struct S3FifoPolicy<K> {
  state: Mutex<State<K>>,
  small_target: u64,
  ghost_cap: usize,
}

impl<K: Eq + Hash + Clone> S3FifoPolicy<K> {
  pub fn new(capacity: u64) -> Self {
    Self {
      state: Mutex::new(State {
        meta: HashMap::new(),
        small: VecDeque::new(),
        main: VecDeque::new(),
        small_cost: 0,
        total_cost: 0,
        ghost: VecDeque::new(),
        ghost_set: HashSet::new(),
        next_stamp: 0,
      }),
      small_target: (capacity / 10).max(1),
      ghost_cap: capacity.max(1) as usize,
    }
  }
}

impl<K: Eq + Hash + Clone> State<K> {
  fn push(&mut self, key: K, queue: Queue) {
    self.next_stamp = self.next_stamp.wrapping_add(1);
    let stamp = self.next_stamp;
    let m = self.meta.get_mut(&key).expect("tracked key");
    m.stamp = stamp;
    m.queue = queue;
    let entry = (key, m.stamp);
    match queue {
      Queue::Small => {
        self.small_cost += m.cost;
        self.small.push_back(entry);
      }
      Queue::Main => self.main.push_back(entry),
    }
  }

  fn remember_ghost(&mut self, key: K, cap: usize) {
    if self.ghost.len() >= cap {
      if let Some(old) = self.ghost.pop_front() {
        self.ghost_set.remove(&old);
      }
    }
    if self.ghost_set.insert(key.clone()) {
      self.ghost.push_back(key);
    }
  }

  fn compact(&mut self) {
    if self.small.len() + self.main.len() <= 2 * self.meta.len() + 64 {
      return;
    }
    let meta = &self.meta;
    let live = |q: Queue| move |(k, s): &(K, u32)| meta.get(k).is_some_and(|m| m.stamp == *s && m.queue == q);
    self.small.retain(live(Queue::Small));
    self.main.retain(live(Queue::Main));
  }
}

impl<K: Eq + Hash + Clone + std::fmt::Debug> S3FifoPolicy<K> {
  /// Live small and main entries oldest first as (key, freq), the ghost set
  /// and the ghost order.
  #[cfg(test)]
  pub(crate) fn debug_state(&self) -> (Vec<(K, u8)>, Vec<(K, u8)>, Vec<K>, Vec<K>) {
    let s = self.state.lock();
    let live = |q: &VecDeque<(K, u32)>, which: Queue| -> Vec<(K, u8)> {
      q.iter()
        .filter(|(k, st)| s.meta.get(k).is_some_and(|m| m.stamp == *st && m.queue == which))
        .map(|(k, _)| (k.clone(), s.meta[k].freq))
        .collect()
    };
    (live(&s.small, Queue::Small), live(&s.main, Queue::Main), s.ghost_set.iter().cloned().collect(), s.ghost.iter().cloned().collect())
  }
}

impl<K, V> CachePolicy<K, V> for S3FifoPolicy<K>
where
  K: Eq + Hash + Clone + Send + Sync,
{
  fn on_access(&self, key: &K, _cost: u64) {
    if let Some(m) = self.state.lock().meta.get_mut(key) {
      m.freq = (m.freq + 1).min(FREQ_MAX);
    }
  }

  fn on_admit(&self, key: &K, cost: u64) -> AdmissionDecision<K> {
    let mut s = self.state.lock();
    if let Some(m) = s.meta.get_mut(key) {
      let old = m.cost;
      m.cost = cost;
      let in_small = m.queue == Queue::Small;
      s.total_cost = s.total_cost - old + cost;
      if in_small {
        s.small_cost = s.small_cost - old + cost;
      }
      return AdmissionDecision::Admit;
    }
    let to_main = s.ghost_set.remove(key);
    s.meta.insert(key.clone(), Meta { cost, freq: 0, queue: Queue::Small, stamp: 0 });
    s.total_cost += cost;
    s.push(key.clone(), if to_main { Queue::Main } else { Queue::Small });
    s.compact();
    AdmissionDecision::Admit
  }

  fn on_remove(&self, key: &K) {
    let mut s = self.state.lock();
    if let Some(m) = s.meta.remove(key) {
      s.total_cost -= m.cost;
      if m.queue == Queue::Small {
        s.small_cost -= m.cost;
      }
    }
  }

  fn evict(&self, cost_to_free: u64) -> (Vec<K>, u64) {
    let mut s = self.state.lock();
    let mut victims = Vec::new();
    let mut freed = 0;
    while freed < cost_to_free {
      let from_small = !s.small.is_empty() && (s.small_cost > self.small_target || s.main.is_empty());
      let popped = if from_small { s.small.pop_front() } else { s.main.pop_front() };
      let Some((key, stamp)) = popped else { break };
      let q = if from_small { Queue::Small } else { Queue::Main };
      let Some(m) = s.meta.get(&key).filter(|m| m.stamp == stamp && m.queue == q) else { continue };
      let (cost, freq) = (m.cost, m.freq);
      if from_small {
        s.small_cost -= cost;
        if freq > 1 {
          s.meta.get_mut(&key).unwrap().freq = 0;
          s.push(key, Queue::Main);
          continue;
        }
        s.meta.remove(&key);
        s.total_cost -= cost;
        s.remember_ghost(key.clone(), self.ghost_cap);
      } else {
        if freq > 0 {
          s.meta.get_mut(&key).unwrap().freq = freq - 1;
          s.push(key, Queue::Main);
          continue;
        }
        s.meta.remove(&key);
        s.total_cost -= cost;
      }
      freed += cost;
      victims.push(key);
    }
    (victims, freed)
  }

  fn clear(&self) {
    let mut s = self.state.lock();
    s.meta.clear();
    s.small.clear();
    s.main.clear();
    s.ghost.clear();
    s.ghost_set.clear();
    s.small_cost = 0;
    s.total_cost = 0;
  }
}
