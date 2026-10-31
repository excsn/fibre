use crate::entry::CacheEntry;
use crate::hpolicy::{ReadRing, ShardPolicy, Table, Victim};
use hashbrown::hash_table::Entry;
use hashbrown::HashTable;
use crate::select::BuiltinPolicy;
use crate::rng::FastRng;
use crate::sync::{HybridMutex, HybridRwLock};
use crate::task::read_log::ReadLog;
use crate::task::timer::TimerWheel;

use core::fmt;
use std::borrow::Borrow;
use equivalent::Equivalent;

use rand::Rng;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::policy::{AdmissionDecision, CachePolicy};
use std::hash::{BuildHasher, Hash, Hasher};
use std::sync::Arc;
use std::time::Duration;

use crossbeam_utils::CachePadded;

/// A helper function to hash a key using a `BuildHasher`.
#[inline]
pub(crate) fn hash_key<K: Hash + ?Sized, H: BuildHasher>(hasher: &H, key: &K) -> u64 {
  let mut state = hasher.build_hasher();
  key.hash(&mut state);
  state.finish()
}

/// A single, independently locked partition of the cache.
pub(crate) struct Shard<K: Send, V, H> {
  pub(crate) map: HybridRwLock<ShardMap<K, V, H>>,
  /// When the shard's earliest TTL timer is due; `ShardMap::expire_due` keeps it current.
  pub(crate) ttl_due: Arc<AtomicU64>,
  /// Reads for a key-based policy, applied under the write lock.
  pub(crate) read_log: Arc<ReadLog<K>>,
  /// Reads for the built-in policy, by node id.
  pub(crate) read_ring: Arc<ReadRing>,
  pub(crate) counters: Arc<ShardCounters>,
  // A lock to ensure only one thread performs maintenance at a time.
  pub(crate) maintenance_lock: HybridMutex<()>,
  // This holds the RNG state for this thread.
  pub(crate) rng: FastRng,
}

impl<K: Send, V, H> Shard<K, V, H> {
  /// Whether a TTL timer is due, read without taking the shard lock.
  pub(crate) fn ttl_is_due(&self, now: u64) -> bool {
    now >= self.ttl_due.load(Ordering::Relaxed)
  }
}

/// A cache store that is partitioned into multiple, independently locked shards.
///
/// This design allows for high concurrency by ensuring that operations on
/// different keys are unlikely to contend for the same lock.
pub(crate) struct ShardedStore<K: Send, V, H> {
  pub(crate) shards: Box<[CachePadded<Shard<K, V, H>>]>,
  pub(crate) hasher: H,
}

impl<K: Send, V, H> fmt::Debug for ShardedStore<K, V, H> {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("ShardedStore")
      .field("num_shards", &self.shards.len())
      .finish()
  }
}

impl<K, V, H> ShardedStore<K, V, H>
where
  K: Eq + Hash + Clone + Send,
  V: Send,
  H: BuildHasher + Clone,
{
  /// Creates a new `ShardedStore` with the specified number of shards and hasher.
  pub(crate) fn new(
    num_shards: usize,
    hasher: H,
    timer_wheel_size: usize,
    timer_tick_duration: Duration,
    capacity: u64,
    policies: &[Arc<dyn CachePolicy<K, V>>],
    builtin: Option<(crate::select::BuiltinChoice, u64)>,
    ctx: Arc<EvictCtx<K, V>>,
  ) -> Self {
    let budget = if capacity == u64::MAX { u64::MAX } else { (capacity / num_shards as u64).max(1) };
    let mut shards = Vec::with_capacity(num_shards);
    for i in 0..num_shards {
      let reads = Arc::new(ReadLog::new());
      let ring = Arc::new(ReadRing::new());
      let counters = Arc::new(ShardCounters::new());
      let ttl_due = Arc::new(AtomicU64::new(u64::MAX));
      let wheel = TimerWheel::new(timer_wheel_size, timer_tick_duration, ttl_due.clone());
      let shard_map = ShardMap {
        wheel,
        local: counters.clone(),
        builtin: builtin.map(|(choice, cap)| crate::select::builtin(choice, cap)),
        ring: ring.clone(),
        victims: Vec::new(),
        reads: reads.clone(),
        table: HashTable::new(),
        hasher: hasher.clone(),
        cost: 0,
        budget,
        policy: policies[i].clone(),
        ctx: ctx.clone(),
      };
      let lock = HybridRwLock::new(shard_map);

      let initial_seed = rand::rng().random();
      let shard = Shard {
        map: lock,
        ttl_due,
        read_log: reads,
        read_ring: ring,
        counters,
        maintenance_lock: HybridMutex::new(()),
        rng: FastRng::new(initial_seed),
      };

      shards.push(CachePadded::new(shard));
    }

    Self {
      shards: shards.into_boxed_slice(),
      hasher,
    }
  }
}

impl<K, V, H> ShardedStore<K, V, H>
where
  K: Hash + Send,
  H: BuildHasher,
{
  /// Returns a reference to the `Shard` for a given key.
  #[inline]
  pub(crate) fn get_shard<Q>(&self, key: &Q) -> &Shard<K, V, H>
  where
    K: Borrow<Q>,
    Q: Eq + Hash + Equivalent<K> + ?Sized,
  {
    let hash = hash_key(&self.hasher, key);
    let index = hash as usize & (self.shards.len() - 1);
    &self.shards[index]
  }

  /// Returns an iterator over all the shards.
  pub(crate) fn iter_shards(&self) -> impl Iterator<Item = &Shard<K, V, H>> {
    self.shards.iter().map(|padded_shard| &**padded_shard)
  }
}

/// Counters a shard bumps under its write lock; `Metrics::snapshot` sums them
/// with the striped counters the other paths still use.
#[derive(Debug)]
pub(crate) struct ShardCounters {
  pub(crate) inserts: AtomicU64,
  pub(crate) keys_admitted: AtomicU64,
  pub(crate) total_cost_added: AtomicU64,
  pub(crate) evicted_by_capacity: AtomicU64,
  /// Net change to the shard's cost from inserts, wrapping.
  pub(crate) cost_delta: AtomicU64,
}

impl ShardCounters {
  fn new() -> Self {
    Self {
      inserts: AtomicU64::new(0),
      keys_admitted: AtomicU64::new(0),
      total_cost_added: AtomicU64::new(0),
      evicted_by_capacity: AtomicU64::new(0),
      cost_delta: AtomicU64::new(0),
    }
  }

  /// The caller holds the shard's write lock, so a load and a store suffice.
  #[inline]
  pub(crate) fn add(c: &AtomicU64, n: u64) {
    c.store(c.load(Ordering::Relaxed).wrapping_add(n), Ordering::Relaxed);
  }
}

/// A read whose shard log or ring was full; the hit path applies it under the
/// write lock through `ShardMap::apply_deferred`.
pub(crate) enum DeferredRead<K> {
  Key(K, u64),
  Node(u64),
}

/// What a shard needs to account for the entries it evicts on insert.
pub(crate) struct EvictCtx<K, V> {
  pub(crate) notify: Option<Box<dyn Fn(K, Arc<V>) + Send + Sync>>,
  /// Nanos past `expires_at` an entry stays for `stale_while_revalidate`.
  pub(crate) grace_ns: u64,
  /// Nanos an entry may stay unread, 0 when the cache has no time-to-idle.
  pub(crate) tti_ns: u64,
}

/// Which limit removed an entry.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ExpiredBy {
  Ttl,
  Tti,
}

#[inline]
fn is_idle<V>(e: &CacheEntry<V>, now: u64, tti_ns: u64) -> bool {
  tti_ns > 0 && now >= e.last_accessed.load(Ordering::Relaxed).saturating_add(tti_ns)
}

#[inline]
fn expired_by<V>(e: &CacheEntry<V>, now: u64, grace_ns: u64, tti_ns: u64) -> Option<ExpiredBy> {
  let expires_at = e.expires_at.load(Ordering::Relaxed);
  if expires_at > 0 && now >= expires_at.saturating_add(grace_ns) {
    return Some(ExpiredBy::Ttl);
  }
  if is_idle(e, now, tti_ns) {
    // Past its TTL and waiting on the grace period, the TTL is what ended it.
    return Some(if expires_at > 0 && now >= expires_at { ExpiredBy::Ttl } else { ExpiredBy::Tti });
  }
  None
}

/// A shard's entries, its cost and its policy. An insert hands the write to
/// the policy under the shard's write lock and removes the victims there, so
/// the policy sees every write and capacity holds when the insert returns.
/// Entries live inline in the table; a policy victim is found by its hash and
/// node id, so no key is cloned on the write path.
pub(crate) struct ShardMap<K, V, H> {
  table: Table<K, V>,
  hasher: H,
  cost: u64,
  budget: u64,
  policy: Arc<dyn CachePolicy<K, V>>,
  reads: Arc<ReadLog<K>>,
  ctx: Arc<EvictCtx<K, V>>,
  /// Set when no `cache_policy_factory` was given; `policy` is then unused.
  builtin: Option<BuiltinPolicy>,
  ring: Arc<ReadRing>,
  victims: Vec<Victim>,
  local: Arc<ShardCounters>,
  wheel: TimerWheel,
}

impl<K, V, H> ShardMap<K, V, H>
where
  K: Eq + Hash,
  H: BuildHasher,
{
  #[inline]
  fn hash<Q: Hash + ?Sized>(&self, k: &Q) -> u64 {
    self.hasher.hash_one(k)
  }

  pub(crate) fn len(&self) -> usize {
    self.table.len()
  }

  pub(crate) fn is_empty(&self) -> bool {
    self.table.is_empty()
  }

  pub(crate) fn iter(&self) -> impl Iterator<Item = (&K, &CacheEntry<V>)> {
    self.table.iter().map(|(k, e)| (k, e))
  }

  pub(crate) fn keys(&self) -> impl Iterator<Item = &K> {
    self.table.iter().map(|(k, _)| k)
  }

  pub(crate) fn get<Q>(&self, k: &Q) -> Option<&CacheEntry<V>>
  where
    Q: Hash + Equivalent<K> + ?Sized,
  {
    self.get_key_value(k).map(|(_, e)| e)
  }

  pub(crate) fn get_key_value<Q>(&self, k: &Q) -> Option<(&K, &CacheEntry<V>)>
  where
    Q: Hash + Equivalent<K> + ?Sized,
  {
    self.get_key_value_hashed(k, self.hash(k))
  }

  /// `hash` is `k`'s hash under the cache's hasher, computed by the caller.
  pub(crate) fn get_key_value_hashed<Q>(&self, k: &Q, hash: u64) -> Option<(&K, &CacheEntry<V>)>
  where
    Q: Equivalent<K> + ?Sized,
  {
    self.table.find(hash, |(kk, _)| k.equivalent(kk)).map(|(kk, e)| (kk, e))
  }

  #[cfg(test)]
  pub(crate) fn timer_count(&self) -> usize {
    self.wheel.len()
  }

  #[cfg(test)]
  pub(crate) fn s3fifo_state(&self) -> Option<(Vec<(u64, u8)>, Vec<(u64, u8)>, Vec<u64>, Vec<u64>)> {
    self.builtin.as_ref().and_then(|b| b.s3fifo_state())
  }

  /// Records a read of the entry whose policy node is `node`.
  #[inline]
  pub(crate) fn touch_read(&self, node: u64) {
    if let Some(b) = &self.builtin {
      ShardPolicy::<K, V>::touch(b, node);
    }
  }

  pub(crate) fn contains_key<Q>(&self, k: &Q) -> bool
  where
    Q: Hash + Equivalent<K> + ?Sized,
  {
    self.get_key_value(k).is_some()
  }

  pub(crate) fn get_mut<Q>(&mut self, k: &Q) -> Option<&mut CacheEntry<V>>
  where
    Q: Hash + Equivalent<K> + ?Sized,
  {
    let hash = self.hash(k);
    self.table.find_mut(hash, |(kk, _)| k.equivalent(kk)).map(|(_, e)| e)
  }

}

impl<K, V, H> ShardMap<K, V, H>
where
  K: Eq + Hash + Clone + Send,
  H: BuildHasher,
{
  fn take<Q>(&mut self, k: &Q) -> Option<(K, CacheEntry<V>)>
  where
    Q: Hash + Equivalent<K> + ?Sized,
  {
    let hash = self.hash(k);
    let (key, e) = match self.table.find_entry(hash, |(kk, _)| k.equivalent(kk)) {
      Ok(o) => o.remove().0,
      Err(_) => return None,
    };
    self.disarm(&e);
    Some((key, e))
  }

  fn take_victim(&mut self, (hash, node): Victim) -> Option<(K, CacheEntry<V>)> {
    let (key, e) = match self.table.find_entry(hash, |(_, e)| e.node.load(Ordering::Relaxed) == node) {
      Ok(o) => o.remove().0,
      Err(_) => return None,
    };
    self.disarm(&e);
    Some((key, e))
  }

  fn arm(&mut self, hash: u64, entry: &mut CacheEntry<V>) {
    entry.key_hash = hash;
    let expires_at = entry.expires_at.load(Ordering::Relaxed);
    if expires_at > 0 {
      entry.ttl_timer_handle = Some(self.wheel.schedule(hash, expires_at.saturating_add(self.ctx.grace_ns)));
    }
  }

  fn disarm(&mut self, entry: &CacheEntry<V>) {
    if let Some(h) = &entry.ttl_timer_handle {
      self.wheel.cancel(h);
    }
  }

  /// Advances the TTL timers to `now` (nanos since the cache epoch) and removes
  /// the entries past a limit.
  pub(crate) fn expire_due(&mut self, now: u64) -> Vec<(K, CacheEntry<V>, ExpiredBy)> {
    let fired = self.wheel.advance(now);
    if fired.is_empty() {
      return Vec::new();
    }
    self.apply_reads();
    let grace = self.ctx.grace_ns;
    let mut out = Vec::new();
    for hash in fired {
      self.remove_expired(hash, now, &mut out);
      // A timer can fire before its entry expires, from clock skew or a hash
      // shared with another key; that entry is armed again for the rest of its TTL.
      let w = &mut self.wheel;
      if let Some((_, e)) = self.table.find_mut(hash, |(_, e)| e.key_hash == hash && e.ttl_timer_handle.as_ref().is_some_and(|h| !w.is_armed(h))) {
        let expires_at = e.expires_at.load(Ordering::Relaxed);
        e.ttl_timer_handle = Some(w.schedule(hash, expires_at.saturating_add(grace)));
      }
    }
    out
  }

  /// Removes every entry past a limit at `now`, whatever the wheel's position.
  pub(crate) fn expire_all(&mut self, now: u64) -> Vec<(K, CacheEntry<V>, ExpiredBy)> {
    let (grace, tti) = (self.ctx.grace_ns, self.ctx.tti_ns);
    let hashes: Vec<u64> = self.table.iter().filter(|(_, e)| expired_by(e, now, grace, tti).is_some()).map(|(_, e)| e.key_hash).collect();
    self.remove_expired_hashes(hashes, now)
  }

  /// The hashes of the entries idle at `now`. Takes only a read lock.
  pub(crate) fn idle_hashes(&self, now: u64) -> Vec<u64> {
    let tti = self.ctx.tti_ns;
    self.table.iter().filter(|(_, e)| is_idle(e, now, tti)).map(|(_, e)| e.key_hash).collect()
  }

  /// Removes the entries under `hashes` still past a limit at `now`.
  pub(crate) fn remove_expired_hashes(&mut self, hashes: Vec<u64>, now: u64) -> Vec<(K, CacheEntry<V>, ExpiredBy)> {
    self.apply_reads();
    let mut out = Vec::with_capacity(hashes.len());
    for hash in hashes {
      self.remove_expired(hash, now, &mut out);
    }
    out
  }

  fn remove_expired(&mut self, hash: u64, now: u64, out: &mut Vec<(K, CacheEntry<V>, ExpiredBy)>) {
    let (grace, tti) = (self.ctx.grace_ns, self.ctx.tti_ns);
    while let Ok(o) = self.table.find_entry(hash, |(_, e)| e.key_hash == hash && expired_by(e, now, grace, tti).is_some()) {
      let by = expired_by(&o.get().1, now, grace, tti).expect("matched as expired");
      let (key, e) = o.remove().0;
      self.cost -= e.cost();
      self.disarm(&e);
      self.forget(&key, &e);
      out.push((key, e, by));
    }
  }

  /// Puts `key` in the table and returns the entry it replaced.
  fn put(&mut self, key: K, entry: CacheEntry<V>, hash: u64) -> Option<CacheEntry<V>> {
    let hasher = &self.hasher;
    match self.table.entry(hash, |(k, _)| *k == key, |(k, _)| hasher.hash_one(k)) {
      Entry::Occupied(mut o) => Some(std::mem::replace(&mut o.get_mut().1, entry)),
      Entry::Vacant(v) => {
        v.insert((key, entry));
        None
      }
    }
  }

  pub(crate) fn insert(&mut self, key: K, entry: CacheEntry<V>) -> Option<CacheEntry<V>> {
    let hash = self.hash(&key);
    self.insert_inner(key, entry, hash, true)
  }

  /// `hash` is `key`'s hash under the cache's hasher, computed by the caller.
  pub(crate) fn insert_hashed(&mut self, key: K, entry: CacheEntry<V>, hash: u64) -> Option<CacheEntry<V>> {
    self.insert_inner(key, entry, hash, true)
  }

  fn insert_inner(&mut self, key: K, mut entry: CacheEntry<V>, hash: u64, count: bool) -> Option<CacheEntry<V>> {
    let before = self.cost;
    let cost = entry.cost();
    self.arm(hash, &mut entry);
    let old = if self.builtin.is_some() { self.insert_builtin(key, entry, hash) } else { self.insert_policy(key, entry, hash) };
    if let Some(o) = &old {
      self.disarm(o);
    }
    let c = &self.local;
    if count {
      ShardCounters::add(&c.inserts, 1);
      ShardCounters::add(&c.keys_admitted, 1);
      ShardCounters::add(&c.total_cost_added, cost);
    }
    ShardCounters::add(&c.cost_delta, self.cost.wrapping_sub(before));
    old
  }

  fn insert_policy(&mut self, key: K, entry: CacheEntry<V>, hash: u64) -> Option<CacheEntry<V>> {
    let cost = entry.cost();
    self.apply_reads();
    let decision = self.policy.on_admit(&key, cost);
    let old = self.put(key, entry, hash);
    if let Some(o) = &old {
      self.cost -= o.cost();
    }
    self.cost += cost;
    let mut evicted = 0u64;
    let mut freed = 0u64;
    if let AdmissionDecision::AdmitAndEvict(victims) = decision {
      for v in victims {
        if let Some((k, e)) = self.take(&v) {
          self.policy.on_remove(&v);
          self.account_eviction(k, e, &mut evicted, &mut freed);
        }
      }
    }
    if self.cost > self.budget {
      let (victims, _) = self.policy.evict(self.cost - self.budget);
      for v in victims {
        if let Some((k, e)) = self.take(&v) {
          self.account_eviction(k, e, &mut evicted, &mut freed);
        }
      }
    }
    if evicted > 0 {
      ShardCounters::add(&self.local.evicted_by_capacity, evicted);
    }
    let _ = freed;
    old
  }

  fn insert_builtin(&mut self, key: K, entry: CacheEntry<V>, hash: u64) -> Option<CacheEntry<V>> {
    let cost = entry.cost();
    let mut victims = std::mem::take(&mut self.victims);
    let policy = self.builtin.as_mut().expect("builtin policy");
    ShardPolicy::<K, V>::apply_reads(policy, &self.ring);
    let hasher = &self.hasher;
    let old = match self.table.entry(hash, |(k, _)| *k == key, |(k, _)| hasher.hash_one(k)) {
      Entry::Occupied(mut o) => {
        let slot = &mut o.get_mut().1;
        let node = slot.node.load(Ordering::Relaxed);
        entry.node.store(node, Ordering::Relaxed);
        ShardPolicy::<K, V>::update(policy, node, cost);
        Some(std::mem::replace(slot, entry))
      }
      Entry::Vacant(v) => {
        let node = ShardPolicy::<K, V>::admit(policy, hash, cost, &mut victims);
        entry.node.store(node, Ordering::Relaxed);
        v.insert((key, entry));
        None
      }
    };
    if let Some(o) = &old {
      self.cost -= o.cost();
    }
    self.cost += cost;
    let (mut evicted, mut freed) = (0u64, 0u64);
    for v in victims.drain(..) {
      if let Some((k, e)) = self.take_victim(v) {
        self.account_eviction(k, e, &mut evicted, &mut freed);
      }
    }
    if self.cost > self.budget {
      let policy = self.builtin.as_mut().expect("builtin policy");
      ShardPolicy::<K, V>::evict(policy, self.cost - self.budget, &mut victims);
      for v in victims.drain(..) {
        if let Some((k, e)) = self.take_victim(v) {
          self.account_eviction(k, e, &mut evicted, &mut freed);
        }
      }
    }
    self.victims = victims;
    if evicted > 0 {
      ShardCounters::add(&self.local.evicted_by_capacity, evicted);
    }
    let _ = freed;
    old
  }

  #[inline]
  fn account_eviction(&mut self, k: K, e: CacheEntry<V>, evicted: &mut u64, freed: &mut u64) {
    self.cost -= e.cost();
    *evicted += 1;
    *freed += e.cost();
    if let Some(notify) = &self.ctx.notify {
      notify(k, e.value());
    }
  }

  pub(crate) fn remove<Q>(&mut self, k: &Q) -> Option<CacheEntry<V>>
  where
    Q: Hash + Equivalent<K> + ?Sized,
  {
    self.remove_entry(k).map(|(_, e)| e)
  }

  pub(crate) fn remove_entry<Q>(&mut self, k: &Q) -> Option<(K, CacheEntry<V>)>
  where
    Q: Hash + Equivalent<K> + ?Sized,
  {
    self.apply_reads();
    let (key, e) = self.take(k)?;
    self.cost -= e.cost();
    self.forget(&key, &e);
    Some((key, e))
  }

  pub(crate) fn clear(&mut self) {
    self.table.clear();
    self.wheel.clear();
    ShardCounters::add(&self.local.cost_delta, 0u64.wrapping_sub(self.cost));
    self.cost = 0;
    match &mut self.builtin {
      Some(b) => ShardPolicy::<K, V>::clear(b),
      None => self.policy.clear(),
    }
  }

  /// Applies this shard's recorded reads to its policy. Called with the
  /// shard's write lock held, like every other policy call.
  pub(crate) fn apply_reads(&mut self) {
    if let Some(b) = &mut self.builtin {
      ShardPolicy::<K, V>::apply_reads(b, &self.ring);
      return;
    }
    let (reads, policy) = (&self.reads, &self.policy);
    reads.drain(|k, c| policy.on_access(&k, c));
  }

  /// Applies a read a hit path could not log, after the reads logged before it.
  pub(crate) fn apply_deferred(&mut self, read: DeferredRead<K>) {
    self.apply_reads();
    match read {
      DeferredRead::Key(k, c) => self.policy.on_access(&k, c),
      DeferredRead::Node(n) => {
        if let Some(b) = &mut self.builtin {
          ShardPolicy::<K, V>::apply_read(b, n);
        }
      }
    }
  }

  fn forget(&mut self, key: &K, e: &CacheEntry<V>) {
    match &mut self.builtin {
      Some(b) => ShardPolicy::<K, V>::remove(b, e.node.load(Ordering::Relaxed)),
      None => self.policy.on_remove(key),
    }
  }

  /// Replaces the contents, as a snapshot restore does.
  pub(crate) fn replace_all(&mut self, entries: Vec<(K, CacheEntry<V>)>) {
    self.clear();
    for (k, e) in entries {
      let hash = self.hash(&k);
      self.insert_inner(k, e, hash, false);
    }
  }
}
