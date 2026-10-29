//! Built-in policies that live inside a shard. The shard owns the policy state
//! and calls it under its write lock, so a policy takes `&mut self` and needs no
//! lock of its own. A resident entry is named by the node id the policy returned
//! from `admit`, stored in its `CacheEntry`; the policy keeps only the entry's
//! hash and names victims by hash and node id, so it holds no key.
//! A node id is `generation << 32 | slot`; a stale id names a freed slot whose
//! generation has moved on and is ignored.

use crate::entry::CacheEntry;
use hashbrown::HashTable;
use std::collections::{HashSet, VecDeque};
use std::sync::atomic::{AtomicU64, AtomicU8, AtomicUsize, Ordering};

pub(crate) const NO_NODE: u64 = u64::MAX;
const NIL: u32 = u32::MAX;

/// A shard's entries, keyed by hash.
pub(crate) type Table<K, V> = HashTable<(K, CacheEntry<V>)>;

/// An entry the policy wants removed: its hash and its node id.
pub(crate) type Victim = (u64, u64);

pub(crate) trait ShardPolicy<K, V> {
  /// Admits an entry not resident in the shard and returns its node id.
  /// Entries the policy rejects on the way, possibly this one, go to `out`.
  fn admit(&mut self, hash: u64, cost: u64, out: &mut Vec<Victim>) -> u64;
  /// A resident entry's value was replaced with one of `cost`.
  fn update(&mut self, node: u64, cost: u64);
  fn remove(&mut self, node: u64);
  /// Evicts until `cost_to_free` is released or the policy is empty.
  fn evict(&mut self, cost_to_free: u64, out: &mut Vec<Victim>);
  /// Applies the reads recorded in `ring` since the last call.
  fn apply_reads(&mut self, ring: &ReadRing);
  /// A read of `node`, under the shard's read lock.
  fn touch(&self, _node: u64) {}
  /// A read of `node`, under the shard's write lock.
  fn apply_read(&mut self, node: u64);
  fn clear(&mut self);
}

/// Reads recorded as node ids, lossy once full until the next drain.
pub(crate) struct ReadRing {
  slots: Box<[AtomicU64]>,
  cursor: AtomicUsize,
}

const RING: usize = 256;

impl ReadRing {
  pub(crate) fn new() -> Self {
    Self { slots: (0..RING).map(|_| AtomicU64::new(0)).collect(), cursor: AtomicUsize::new(0) }
  }

  /// Records a read of `node`. Returns false when the ring is full.
  #[inline]
  pub(crate) fn push(&self, node: u64) -> bool {
    if node == NO_NODE {
      return true;
    }
    if self.cursor.load(Ordering::Relaxed) >= RING {
      return false;
    }
    let i = self.cursor.fetch_add(1, Ordering::Relaxed);
    if i < RING {
      self.slots[i].store(node + 1, Ordering::Release);
      true
    } else {
      false
    }
  }

  fn drain(&self, mut f: impl FnMut(u64)) {
    let n = self.cursor.swap(0, Ordering::Acquire).min(RING);
    for slot in &self.slots[..n] {
      let v = slot.swap(0, Ordering::Acquire);
      if v != 0 {
        f(v - 1);
      }
    }
  }
}

#[inline]
fn split(node: u64) -> (u32, u32) {
  ((node >> 32) as u32, node as u32)
}

#[inline]
fn join(generation: u32, slot: u32) -> u64 {
  (generation as u64) << 32 | slot as u64
}

struct Link {
  prev: u32,
  next: u32,
  seg: u8,
  generation: u32,
}

#[derive(Clone, Copy)]
struct List {
  head: u32,
  tail: u32,
  cost: u64,
}

const EMPTY: List = List { head: NIL, tail: NIL, cost: 0 };
const FREE: u8 = u8::MAX;

/// Intrusive doubly linked lists over one slab. `P` is the per-node payload.
struct Slab<P> {
  links: Vec<Link>,
  payload: Vec<Option<P>>,
  costs: Vec<u64>,
  free: Vec<u32>,
  lists: [List; 3],
}

impl<P> Slab<P> {
  fn new() -> Self {
    Self { links: Vec::new(), payload: Vec::new(), costs: Vec::new(), free: Vec::new(), lists: [EMPTY; 3] }
  }

  fn alloc(&mut self, p: P, cost: u64) -> u32 {
    if let Some(i) = self.free.pop() {
      self.payload[i as usize] = Some(p);
      self.costs[i as usize] = cost;
      i
    } else {
      self.links.push(Link { prev: NIL, next: NIL, seg: FREE, generation: 0 });
      self.payload.push(Some(p));
      self.costs.push(cost);
      (self.links.len() - 1) as u32
    }
  }

  fn release(&mut self, i: u32) -> P {
    let l = &mut self.links[i as usize];
    l.seg = FREE;
    l.generation = l.generation.wrapping_add(1);
    self.free.push(i);
    self.payload[i as usize].take().expect("live slot")
  }

  fn resolve(&self, node: u64) -> Option<u32> {
    let (generation, slot) = split(node);
    let l = self.links.get(slot as usize)?;
    (l.seg != FREE && l.generation == generation).then_some(slot)
  }

  fn id(&self, i: u32) -> u64 {
    join(self.links[i as usize].generation, i)
  }

  fn push_front(&mut self, seg: u8, i: u32) {
    let list = &mut self.lists[seg as usize];
    let old = list.head;
    list.head = i;
    if list.tail == NIL {
      list.tail = i;
    }
    list.cost += self.costs[i as usize];
    let l = &mut self.links[i as usize];
    l.prev = NIL;
    l.next = old;
    l.seg = seg;
    if old != NIL {
      self.links[old as usize].prev = i;
    }
  }

  fn unlink(&mut self, i: u32) {
    let (prev, next, seg) = {
      let l = &self.links[i as usize];
      (l.prev, l.next, l.seg)
    };
    if prev != NIL {
      self.links[prev as usize].next = next;
    } else {
      self.lists[seg as usize].head = next;
    }
    if next != NIL {
      self.links[next as usize].prev = prev;
    } else {
      self.lists[seg as usize].tail = prev;
    }
    self.lists[seg as usize].cost -= self.costs[i as usize];
  }

  fn move_front(&mut self, seg: u8, i: u32) {
    if self.lists[seg as usize].head != i || self.links[i as usize].seg != seg {
      self.unlink(i);
      self.push_front(seg, i);
    }
  }

  fn set_cost(&mut self, i: u32, cost: u64) {
    let seg = self.links[i as usize].seg as usize;
    let old = std::mem::replace(&mut self.costs[i as usize], cost);
    self.lists[seg].cost = self.lists[seg].cost - old + cost;
  }

  fn clear(&mut self) {
    self.links.clear();
    self.payload.clear();
    self.costs.clear();
    self.free.clear();
    self.lists = [EMPTY; 3];
  }
}

const WINDOW: u8 = 0;
const PROBATION: u8 = 1;
const PROTECTED: u8 = 2;

/// W-TinyLFU on node handles: the same window, probation and protected
/// segments and admission test as `policy::tinylfu::TinyLfuPolicy`.
pub(crate) struct HTinyLfu {
  slab: Slab<u64>,
  sketch: Sketch,
  window_target: u64,
  main_capacity: u64,
  prot_capacity: u64,
}

impl HTinyLfu {
  pub(crate) fn new(capacity: u64) -> Self {
    let window_target = if capacity == 0 { 0 } else { ((capacity as f64 * 0.01).round() as u64).max(1) };
    let main = capacity.saturating_sub(window_target);
    let prot_capacity = if main == 0 { 0 } else { main.saturating_sub(((main as f64 * 0.20).round() as u64).max(1)) };
    Self { slab: Slab::new(), sketch: Sketch::new((capacity * 10).max(100) as usize), window_target, main_capacity: main, prot_capacity }
  }

  #[inline]
  fn hash_of(&self, i: u32) -> u64 {
    self.slab.payload[i as usize].expect("live slot")
  }

  fn maintain(&mut self) {
    while self.slab.lists[PROTECTED as usize].cost > self.prot_capacity {
      let t = self.slab.lists[PROTECTED as usize].tail;
      if t == NIL {
        break;
      }
      self.slab.unlink(t);
      self.slab.push_front(PROBATION, t);
    }
  }

  fn touch(&mut self, i: u32) {
    match self.slab.links[i as usize].seg {
      WINDOW => self.slab.move_front(WINDOW, i),
      PROBATION => {
        self.slab.unlink(i);
        self.slab.push_front(PROTECTED, i);
        self.maintain();
      }
      _ => self.slab.move_front(PROTECTED, i),
    }
  }

  fn touch_node(&mut self, node: u64) {
    if let Some(i) = self.slab.resolve(node) {
      self.sketch.increment(self.hash_of(i));
      self.touch(i);
    }
  }

  fn drop_node(&mut self, i: u32, out: &mut Vec<Victim>) {
    let id = self.slab.id(i);
    let hash = self.slab.release(i);
    out.push((hash, id));
  }

  pub(crate) fn admit_key(&mut self, hash: u64, cost: u64, out: &mut Vec<Victim>) -> u64 {
    self.sketch.increment(hash);
    let i = self.slab.alloc(hash, cost);
    self.slab.push_front(WINDOW, i);
    let id = self.slab.id(i);
    while self.slab.lists[WINDOW as usize].cost > self.window_target {
      let cand = self.slab.lists[WINDOW as usize].tail;
      if cand == NIL {
        break;
      }
      self.slab.unlink(cand);
      let main_cost = self.slab.lists[PROBATION as usize].cost + self.slab.lists[PROTECTED as usize].cost;
      if main_cost + self.slab.costs[cand as usize] <= self.main_capacity {
        self.slab.push_front(PROBATION, cand);
        continue;
      }
      let victim = match self.slab.lists[PROBATION as usize].tail {
        NIL => self.slab.lists[PROTECTED as usize].tail,
        t => t,
      };
      let admit = victim == NIL || self.sketch.estimate(self.hash_of(cand)) >= self.sketch.estimate(self.hash_of(victim));
      if admit {
        self.slab.push_front(PROBATION, cand);
      } else {
        self.drop_node(cand, out);
      }
    }
    id
  }

  pub(crate) fn evict_nodes(&mut self, mut cost_to_free: u64, out: &mut Vec<Victim>) {
    self.maintain();
    for seg in [PROBATION, PROTECTED] {
      while cost_to_free > 0 {
        let t = self.slab.lists[seg as usize].tail;
        if t == NIL {
          break;
        }
        cost_to_free = cost_to_free.saturating_sub(self.slab.costs[t as usize]);
        self.slab.unlink(t);
        self.drop_node(t, out);
      }
    }
  }
}

impl<K, V> ShardPolicy<K, V> for HTinyLfu {
  fn admit(&mut self, hash: u64, cost: u64, out: &mut Vec<Victim>) -> u64 {
    self.admit_key(hash, cost, out)
  }

  fn update(&mut self, node: u64, cost: u64) {
    if let Some(i) = self.slab.resolve(node) {
      self.sketch.increment(self.hash_of(i));
      self.slab.set_cost(i, cost);
      self.touch(i);
    }
  }

  fn remove(&mut self, node: u64) {
    if let Some(i) = self.slab.resolve(node) {
      self.slab.unlink(i);
      self.slab.release(i);
    }
  }

  fn evict(&mut self, cost_to_free: u64, out: &mut Vec<Victim>) {
    self.evict_nodes(cost_to_free, out)
  }

  fn apply_reads(&mut self, ring: &ReadRing) {
    let mut nodes = [0u64; RING];
    let mut n = 0;
    ring.drain(|v| {
      nodes[n] = v;
      n += 1;
    });
    for &node in &nodes[..n] {
      self.touch_node(node);
    }
  }

  fn apply_read(&mut self, node: u64) {
    self.touch_node(node);
  }

  fn clear(&mut self) {
    self.slab.clear();
    self.sketch.clear();
  }
}

/// S3-FIFO on node handles, making the decisions of `policy::s3fifo::S3FifoPolicy`.
/// Reads bump the node's `freq` (capped at 3) under the shard read lock; `freq`
/// grows only under the write lock.
pub(crate) struct HS3Fifo {
  slab: Slab<u64>,
  freq: Vec<AtomicU8>,
  small_target: u64,
  ghost: HashSet<u64>,
  ghost_order: VecDeque<u64>,
  ghost_cap: usize,
}

const SMALL: u8 = 0;
const MAIN: u8 = 1;

impl HS3Fifo {
  pub(crate) fn new(capacity: u64) -> Self {
    Self {
      slab: Slab::new(),
      freq: Vec::new(),
      small_target: (capacity / 10).max(1),
      ghost: HashSet::new(),
      ghost_order: VecDeque::new(),
      ghost_cap: capacity.max(1) as usize,
    }
  }

  #[inline]
  fn hash_of(&self, i: u32) -> u64 {
    self.slab.payload[i as usize].expect("live slot")
  }

  #[inline]
  fn freq(&self, i: u32) -> u8 {
    self.freq[i as usize].load(Ordering::Relaxed)
  }

  #[inline]
  fn set_freq(&self, i: u32, f: u8) {
    self.freq[i as usize].store(f, Ordering::Relaxed);
  }

  #[inline]
  fn bump(&self, i: u32) {
    let f = self.freq(i);
    if f < 3 {
      self.set_freq(i, f + 1);
    }
  }

  /// Pops the oldest ghost before adding this one, as `S3FifoPolicy` does, so a
  /// stale copy of the same hash at the front cannot cancel the new membership.
  fn remember(&mut self, hash: u64) {
    if self.ghost_order.len() >= self.ghost_cap {
      if let Some(old) = self.ghost_order.pop_front() {
        self.ghost.remove(&old);
      }
    }
    if self.ghost.insert(hash) {
      self.ghost_order.push_back(hash);
    }
  }

  fn drop_node(&mut self, i: u32, out: &mut Vec<Victim>) -> u64 {
    let cost = self.slab.costs[i as usize];
    let id = self.slab.id(i);
    let hash = self.slab.release(i);
    out.push((hash, id));
    cost
  }

  /// Evicts one entry from the small queue or promotes it; returns the cost freed.
  fn evict_small(&mut self, out: &mut Vec<Victim>) -> u64 {
    let t = self.slab.lists[SMALL as usize].tail;
    self.slab.unlink(t);
    if self.freq(t) > 1 {
      self.set_freq(t, 0);
      self.slab.push_front(MAIN, t);
      return 0;
    }
    let hash = self.hash_of(t);
    let cost = self.drop_node(t, out);
    self.remember(hash);
    cost
  }

  fn evict_main(&mut self, out: &mut Vec<Victim>) -> u64 {
    let t = self.slab.lists[MAIN as usize].tail;
    self.slab.unlink(t);
    let f = self.freq(t);
    if f > 0 {
      self.set_freq(t, f - 1);
      self.slab.push_front(MAIN, t);
      return 0;
    }
    self.drop_node(t, out)
  }

  /// Small and main queues oldest first as (hash, freq), the ghost set and
  /// the ghost order.
  #[cfg(test)]
  pub(crate) fn debug_state(&self) -> (Vec<(u64, u8)>, Vec<(u64, u8)>, Vec<u64>, Vec<u64>) {
    let list = |seg: u8| {
      let mut v = Vec::new();
      let mut i = self.slab.lists[seg as usize].tail;
      while i != NIL {
        v.push((self.hash_of(i), self.freq(i)));
        i = self.slab.links[i as usize].prev;
      }
      v
    };
    let mut set: Vec<u64> = self.ghost.iter().copied().collect();
    set.sort_unstable();
    (list(SMALL), list(MAIN), set, self.ghost_order.iter().copied().collect())
  }

  pub(crate) fn admit_key(&mut self, hash: u64, cost: u64) -> u64 {
    let seg = if self.ghost.remove(&hash) { MAIN } else { SMALL };
    let i = self.slab.alloc(hash, cost);
    if i as usize >= self.freq.len() {
      self.freq.push(AtomicU8::new(0));
    } else {
      self.set_freq(i, 0);
    }
    self.slab.push_front(seg, i);
    self.slab.id(i)
  }
}

impl<K, V> ShardPolicy<K, V> for HS3Fifo {
  fn admit(&mut self, hash: u64, cost: u64, _out: &mut Vec<Victim>) -> u64 {
    self.admit_key(hash, cost)
  }

  fn update(&mut self, node: u64, cost: u64) {
    if let Some(i) = self.slab.resolve(node) {
      self.slab.set_cost(i, cost);
    }
  }

  fn remove(&mut self, node: u64) {
    if let Some(i) = self.slab.resolve(node) {
      self.slab.unlink(i);
      self.slab.release(i);
    }
  }

  fn evict(&mut self, cost_to_free: u64, out: &mut Vec<Victim>) {
    let mut freed = 0;
    while freed < cost_to_free {
      let small = self.slab.lists[SMALL as usize];
      let main = self.slab.lists[MAIN as usize];
      freed += if small.tail != NIL && (small.cost > self.small_target || main.tail == NIL) {
        self.evict_small(out)
      } else if main.tail != NIL {
        self.evict_main(out)
      } else {
        break;
      };
    }
  }

  fn apply_reads(&mut self, _ring: &ReadRing) {}

  fn touch(&self, node: u64) {
    if let Some(i) = self.slab.resolve(node) {
      self.bump(i);
    }
  }

  fn apply_read(&mut self, node: u64) {
    if let Some(i) = self.slab.resolve(node) {
      self.bump(i);
    }
  }

  fn clear(&mut self) {
    self.slab.clear();
    self.ghost.clear();
    self.ghost_order.clear();
  }
}

/// Count-min sketch, four rows of `width` counters, halved every
/// `reset_threshold` increments. Rows index `h1 + i * h2` from one key hash,
/// mixed first because every key in a shard shares the hash's low bits.
struct Sketch {
  counters: Vec<Vec<u32>>,
  mask: usize,
  increments: usize,
  reset_threshold: usize,
}

impl Sketch {
  fn new(reset_threshold: usize) -> Self {
    const DEPTH: usize = 4;
    let width = (reset_threshold * 2 / DEPTH).max(256).next_power_of_two();
    Self { counters: vec![vec![0; width]; DEPTH], mask: width - 1, increments: 0, reset_threshold }
  }

  #[inline]
  fn index(&self, hash: u64, row: usize) -> usize {
    let mixed = fmix64(hash);
    let (h1, h2) = (mixed as usize, ((mixed >> 32) as usize) | 1);
    h1.wrapping_add(row.wrapping_mul(h2)) & self.mask
  }

  fn increment(&mut self, hash: u64) {
    for row in 0..self.counters.len() {
      let i = self.index(hash, row);
      self.counters[row][i] = self.counters[row][i].saturating_add(1);
    }
    self.increments += 1;
    if self.increments >= self.reset_threshold {
      self.increments = 0;
      for row in &mut self.counters {
        for c in row {
          *c /= 2;
        }
      }
    }
  }

  fn estimate(&self, hash: u64) -> u32 {
    (0..self.counters.len()).map(|row| self.counters[row][self.index(hash, row)]).min().unwrap_or(0)
  }

  fn clear(&mut self) {
    self.increments = 0;
    for row in &mut self.counters {
      row.fill(0);
    }
  }
}

/// MurmurHash3's 64-bit finalizer.
#[inline]
pub(crate) fn fmix64(mut h: u64) -> u64 {
  h ^= h >> 33;
  h = h.wrapping_mul(0xff51_afd7_ed55_8ccd);
  h ^= h >> 33;
  h = h.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
  h ^ (h >> 33)
}

/// The built-in policies, dispatched by `match`.
pub(crate) enum Builtin {
  TinyLfu(HTinyLfu),
  S3Fifo(HS3Fifo),
}

impl Builtin {
  #[cfg(test)]
  pub(crate) fn s3fifo_state(&self) -> Option<(Vec<(u64, u8)>, Vec<(u64, u8)>, Vec<u64>, Vec<u64>)> {
    match self {
      Builtin::TinyLfu(_) => None,
      Builtin::S3Fifo(p) => Some(p.debug_state()),
    }
  }
}

impl<K, V> ShardPolicy<K, V> for Builtin {
  #[inline]
  fn admit(&mut self, hash: u64, cost: u64, out: &mut Vec<Victim>) -> u64 {
    match self {
      Builtin::TinyLfu(p) => p.admit_key(hash, cost, out),
      Builtin::S3Fifo(p) => p.admit_key(hash, cost),
    }
  }
  #[inline]
  fn update(&mut self, node: u64, cost: u64) {
    match self {
      Builtin::TinyLfu(p) => ShardPolicy::<K, V>::update(p, node, cost),
      Builtin::S3Fifo(p) => ShardPolicy::<K, V>::update(p, node, cost),
    }
  }
  #[inline]
  fn remove(&mut self, node: u64) {
    match self {
      Builtin::TinyLfu(p) => ShardPolicy::<K, V>::remove(p, node),
      Builtin::S3Fifo(p) => ShardPolicy::<K, V>::remove(p, node),
    }
  }
  #[inline]
  fn evict(&mut self, cost_to_free: u64, out: &mut Vec<Victim>) {
    match self {
      Builtin::TinyLfu(p) => p.evict_nodes(cost_to_free, out),
      Builtin::S3Fifo(p) => ShardPolicy::<K, V>::evict(p, cost_to_free, out),
    }
  }
  #[inline]
  fn apply_reads(&mut self, ring: &ReadRing) {
    match self {
      Builtin::TinyLfu(p) => ShardPolicy::<K, V>::apply_reads(p, ring),
      Builtin::S3Fifo(p) => ShardPolicy::<K, V>::apply_reads(p, ring),
    }
  }
  #[inline]
  fn touch(&self, node: u64) {
    match self {
      Builtin::TinyLfu(p) => ShardPolicy::<K, V>::touch(p, node),
      Builtin::S3Fifo(p) => ShardPolicy::<K, V>::touch(p, node),
    }
  }
  #[inline]
  fn apply_read(&mut self, node: u64) {
    match self {
      Builtin::TinyLfu(p) => ShardPolicy::<K, V>::apply_read(p, node),
      Builtin::S3Fifo(p) => ShardPolicy::<K, V>::apply_read(p, node),
    }
  }
  #[inline]
  fn clear(&mut self) {
    match self {
      Builtin::TinyLfu(p) => ShardPolicy::<K, V>::clear(p),
      Builtin::S3Fifo(p) => ShardPolicy::<K, V>::clear(p),
    }
  }
}

#[cfg(test)]
mod policy_sim {
  use super::{HS3Fifo, HTinyLfu, ShardPolicy};
  use crate::policy::s3fifo::S3FifoPolicy;
  use crate::policy::tinylfu::TinyLfuPolicy;
  use crate::policy::{AdmissionDecision, CachePolicy};
  use crate::store::hash_key;
  use std::collections::HashMap;

  const SHARD_SEEDS: (u64, u64, u64, u64) = (1, 2, 3, 4);
  const SKETCH_SEEDS: (u64, u64, u64, u64) = (5, 6, 7, 8);

  fn sketch_hasher() -> ahash::RandomState {
    let s = SKETCH_SEEDS;
    ahash::RandomState::with_seeds(s.0, s.1, s.2, s.3)
  }

  fn xorshift(x: &mut u64) -> u64 {
    *x ^= *x << 13;
    *x ^= *x >> 7;
    *x ^= *x << 17;
    *x
  }

  fn zipf_trace(universe: u64, len: usize, exponent: f64, seed: u64) -> Vec<u64> {
    let mut cdf = Vec::with_capacity(universe as usize);
    let mut acc = 0.0;
    for r in 1..=universe {
      acc += 1.0 / (r as f64).powf(exponent);
      cdf.push(acc);
    }
    let mut x = seed | 1;
    (0..len)
      .map(|_| {
        let u = (xorshift(&mut x) >> 11) as f64 / (1u64 << 53) as f64 * acc;
        cdf.partition_point(|&c| c < u).min(universe as usize - 1) as u64
      })
      .collect()
  }

  #[derive(Clone, Copy, Debug)]
  enum Variant {
    KeyBased,
    Handle,
  }

  enum Shard {
    Key(TinyLfuPolicy<u64>, HashMap<u64, ()>),
    Handle(HTinyLfu, HashMap<u64, u64>, HashMap<u64, u64>),
  }

  struct Sim {
    shards: Vec<Shard>,
    per_shard: u64,
    shard_hasher: ahash::RandomState,
    sketch_hasher: ahash::RandomState,
  }

  impl Sim {
    fn new(variant: Variant, capacity: u64, shards: usize) -> Self {
      let per_shard = capacity.div_ceil(shards as u64);
      let s = SHARD_SEEDS;
      Sim {
        shards: (0..shards)
          .map(|_| match variant {
            Variant::KeyBased => Shard::Key(TinyLfuPolicy::with_sketch_hasher(per_shard, sketch_hasher()), HashMap::new()),
            Variant::Handle => Shard::Handle(HTinyLfu::new(per_shard), HashMap::new(), HashMap::new()),
          })
          .collect(),
        per_shard,
        shard_hasher: ahash::RandomState::with_seeds(s.0, s.1, s.2, s.3),
        sketch_hasher: sketch_hasher(),
      }
    }

    /// Returns whether `key` was resident; a miss inserts it. The handle
    /// policy gets the sketch hasher's hash, so its sketch indexes as the
    /// key-based policy's does.
    fn access(&mut self, key: u64) -> bool {
      let shard_hash = hash_key(&self.shard_hasher, &key);
      let hash = hash_key(&self.sketch_hasher, &key);
      let per_shard = self.per_shard;
      let n = self.shards.len();
      match &mut self.shards[shard_hash as usize & (n - 1)] {
        Shard::Key(p, map) => {
          if map.contains_key(&key) {
            CachePolicy::<u64, ()>::on_access(p, &key, 1);
            return true;
          }
          map.insert(key, ());
          if let AdmissionDecision::AdmitAndEvict(victims) = CachePolicy::<u64, ()>::on_admit(p, &key, 1) {
            for v in victims {
              map.remove(&v);
            }
          }
          if map.len() as u64 > per_shard {
            let (victims, _) = CachePolicy::<u64, ()>::evict(p, map.len() as u64 - per_shard);
            for v in victims {
              map.remove(&v);
            }
          }
          false
        }
        Shard::Handle(p, map, nodes) => {
          if let Some(&node) = map.get(&key) {
            p.touch_node(node);
            return true;
          }
          let mut out = Vec::new();
          let node = p.admit_key(hash, 1, &mut out);
          map.insert(key, node);
          nodes.insert(node, key);
          for (_, n) in out.drain(..) {
            if let Some(k) = nodes.remove(&n) {
              map.remove(&k);
            }
          }
          if map.len() as u64 > per_shard {
            p.evict_nodes(map.len() as u64 - per_shard, &mut out);
            for (_, n) in out.drain(..) {
              if let Some(k) = nodes.remove(&n) {
                map.remove(&k);
              }
            }
          }
          false
        }
      }
    }

    fn per_shard_resident(&self) -> Vec<usize> {
      self
        .shards
        .iter()
        .map(|s| match s {
          Shard::Key(_, m) => m.len(),
          Shard::Handle(_, m, _) => m.len(),
        })
        .collect()
    }

    fn resident_keys(&self) -> Vec<u64> {
      let mut v: Vec<u64> = self
        .shards
        .iter()
        .flat_map(|s| match s {
          Shard::Key(_, m) => m.keys().copied().collect::<Vec<_>>(),
          Shard::Handle(_, m, _) => m.keys().copied().collect::<Vec<_>>(),
        })
        .collect();
      v.sort_unstable();
      v
    }

    fn resident(&self) -> usize {
      self
        .shards
        .iter()
        .map(|s| match s {
          Shard::Key(_, m) => m.len(),
          Shard::Handle(_, m, _) => m.len(),
        })
        .sum()
    }
  }

  const VARIANTS: [Variant; 2] = [Variant::KeyBased, Variant::Handle];

  #[test]
  fn tinylfu_hit_ratio_by_sketch_index() {
    let capacity = 10_000;
    let trace = zipf_trace(capacity * 10, 4_000_000, 0.99, 42);
    for shards in [128usize, 1] {
      for v in VARIANTS {
        let mut sim = Sim::new(v, capacity, shards);
        let half = trace.len() / 2;
        for &k in &trace[..half] {
          sim.access(k);
        }
        let hits = trace[half..].iter().filter(|&&k| sim.access(k)).count();
        println!(
          "zipf0.99 shards={shards:>3} {v:?}: hit_ratio={:.4} resident={}",
          hits as f64 / (trace.len() - half) as f64,
          sim.resident()
        );
        let r = sim.per_shard_resident();
        println!(
          "  per-shard resident min={} max={} below_cap={}",
          r.iter().min().unwrap(),
          r.iter().max().unwrap(),
          r.iter().filter(|&&x| (x as u64) < sim.per_shard).count()
        );
      }
    }
  }

  /// The handle policy and the key-based policy make the same decision on
  /// every operation of a Zipf trace, at one shard and at 128.
  #[test]
  fn tinylfu_handle_matches_key_based_in_lockstep() {
    let capacity = 10_000;
    let trace = zipf_trace(capacity * 10, 400_000, 0.99, 7);
    for shards in [128usize, 1] {
      let mut key_based = Sim::new(Variant::KeyBased, capacity, shards);
      let mut handle = Sim::new(Variant::Handle, capacity, shards);
      for (i, &k) in trace.iter().enumerate() {
        let a = key_based.access(k);
        let b = handle.access(k);
        assert_eq!(a, b, "shards={shards} op {i} key {k}: hit differs");
        if i % 997 == 0 || i + 1 == trace.len() {
          assert_eq!(key_based.resident_keys(), handle.resident_keys(), "shards={shards} op {i}: resident sets differ");
        }
      }
    }
  }

  struct S3Pair {
    key_based: Vec<(S3FifoPolicy<u64>, HashMap<u64, ()>)>,
    handle: Vec<(HS3Fifo, HashMap<u64, u64>, HashMap<u64, u64>)>,
    per_shard: u64,
    shard_hasher: ahash::RandomState,
  }

  impl S3Pair {
    fn new(capacity: u64, shards: usize) -> Self {
      let per_shard = capacity.div_ceil(shards as u64);
      let s = SHARD_SEEDS;
      S3Pair {
        key_based: (0..shards).map(|_| (S3FifoPolicy::new(per_shard), HashMap::new())).collect(),
        handle: (0..shards).map(|_| (HS3Fifo::new(per_shard), HashMap::new(), HashMap::new())).collect(),
        per_shard,
        shard_hasher: ahash::RandomState::with_seeds(s.0, s.1, s.2, s.3),
      }
    }

    /// Runs one access on both sides and returns their hit results.
    fn access(&mut self, key: u64) -> (bool, bool) {
      let hash = hash_key(&self.shard_hasher, &key);
      let per_shard = self.per_shard;
      let idx = hash as usize & (self.key_based.len() - 1);
      let a = {
        let (p, map) = &mut self.key_based[idx];
        if map.contains_key(&key) {
          CachePolicy::<u64, ()>::on_access(p, &key, 1);
          true
        } else {
          map.insert(key, ());
          CachePolicy::<u64, ()>::on_admit(p, &key, 1);
          if map.len() as u64 > per_shard {
            let (victims, _) = CachePolicy::<u64, ()>::evict(p, map.len() as u64 - per_shard);
            for v in victims {
              map.remove(&v);
            }
          }
          false
        }
      };
      let b = {
        let (p, map, nodes) = &mut self.handle[idx];
        if let Some(&node) = map.get(&key) {
          ShardPolicy::<u64, ()>::touch(p, node);
          true
        } else {
          let node = p.admit_key(hash, 1);
          map.insert(key, node);
          nodes.insert(node, key);
          if map.len() as u64 > per_shard {
            let mut out = Vec::new();
            ShardPolicy::<u64, ()>::evict(p, map.len() as u64 - per_shard, &mut out);
            for (_, n) in out {
              if let Some(k) = nodes.remove(&n) {
                map.remove(&k);
              }
            }
          }
          false
        }
      };
      (a, b)
    }

    fn resident(&self) -> (Vec<u64>, Vec<u64>) {
      let mut a: Vec<u64> = self.key_based.iter().flat_map(|(_, m)| m.keys().copied()).collect();
      let mut b: Vec<u64> = self.handle.iter().flat_map(|(_, m, _)| m.keys().copied()).collect();
      a.sort_unstable();
      b.sort_unstable();
      (a, b)
    }
  }

  /// The key-based `S3FifoPolicy` and `HS3Fifo` make the same decision on
  /// every operation of a Zipf trace, at one shard and at 128.
  #[test]
  fn s3fifo_handle_matches_key_based_in_lockstep() {
    let capacity = 10_000;
    let trace = zipf_trace(capacity * 10, 400_000, 0.99, 7);
    for shards in [128usize, 1] {
      let mut pair = S3Pair::new(capacity, shards);
      let mut hits = 0u64;
      for (i, &k) in trace.iter().enumerate() {
        let (a, b) = pair.access(k);
        assert_eq!(a, b, "shards={shards} op {i} key {k}: hit differs");
        hits += a as u64;
        if i % 997 == 0 || i + 1 == trace.len() {
          let (ra, rb) = pair.resident();
          assert_eq!(ra, rb, "shards={shards} op {i}: resident sets differ");
        }
      }
      println!("s3fifo zipf0.99 shards={shards}: hit_ratio={:.4}", hits as f64 / trace.len() as f64);
    }
  }

  #[test]
  fn htinylfu_admits_without_test_while_main_has_room() {
    let mut p = HTinyLfu::new(10);
    let mut out = Vec::new();
    let hot = p.admit_key(0, 1, &mut out);
    p.admit_key(1, 1, &mut out);
    for _ in 0..20 {
      p.touch_node(hot);
    }
    for k in 2..10u64 {
      p.admit_key(k, 1, &mut out);
    }
    assert!(out.is_empty(), "rejected while main had room: {out:?}");
  }

  #[test]
  fn key_based_tinylfu_admits_without_test_while_main_has_room() {
    let p: TinyLfuPolicy<u64> = TinyLfuPolicy::new(10);
    CachePolicy::<u64, ()>::on_admit(&p, &0, 1);
    CachePolicy::<u64, ()>::on_admit(&p, &1, 1);
    for _ in 0..20 {
      CachePolicy::<u64, ()>::on_access(&p, &0, 1);
    }
    for k in 2..10u64 {
      let d = CachePolicy::<u64, ()>::on_admit(&p, &k, 1);
      assert!(matches!(d, AdmissionDecision::Admit), "rejected {k} while main had room: {d:?}");
    }
  }

  #[test]
  fn zipf_trace_distinct_keys_per_shard() {
    let capacity = 10_000;
    let trace = zipf_trace(capacity * 10, 4_000_000, 0.99, 42);
    let hasher = ahash::RandomState::with_seeds(SHARD_SEEDS.0, SHARD_SEEDS.1, SHARD_SEEDS.2, SHARD_SEEDS.3);
    let mut seen = vec![std::collections::HashSet::new(); 128];
    for &k in &trace {
      seen[hash_key(&hasher, &k) as usize & 127].insert(k);
    }
    let mut d: Vec<usize> = seen.iter().map(|s| s.len()).collect();
    d.sort();
    let all: std::collections::HashSet<u64> = trace.iter().copied().collect();
    println!("distinct={} per-shard sorted head={:?} tail={:?}", all.len(), &d[..8], &d[120..]);
  }

  #[test]
  fn tinylfu_prefill_residency_by_sketch_index() {
    let capacity = 10_000;
    for shards in [128usize, 1] {
      for v in VARIANTS {
        let mut sim = Sim::new(v, capacity, shards);
        for k in 0..capacity {
          sim.access(k);
        }
        println!("prefill shards={shards:>3} {v:?}: resident={}", sim.resident());
      }
    }
  }
}
