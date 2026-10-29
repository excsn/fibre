//! Each shipped policy, run through the cache, makes the decisions it makes
//! when run alone: an oracle holds a plain map per shard and calls the policy
//! on every operation, with nothing deferred and nothing else evicting. The
//! cache built with the same policy over the same trace must hold the same
//! keys after every step.

use crate::policy::arc::ArcPolicy;
use crate::policy::clock::ClockPolicy;
use crate::policy::fifo::Fifo;
use crate::policy::lru::LruPolicy;
use crate::policy::random::RandomPolicy;
use crate::policy::s3fifo::S3FifoPolicy;
use crate::policy::sieve::SievePolicy;
use crate::policy::slru::SlruPolicy;
use crate::policy::tinylfu::TinyLfuPolicy;
use crate::policy::{AdmissionDecision, CachePolicy};
use crate::store::hash_key;
use crate::{Cache, CacheBuilder};

use parking_lot::Mutex;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

type Factory = Arc<dyn Fn() -> Box<dyn CachePolicy<u64, u64>> + Send + Sync>;

const CAPACITY: u64 = 2_000;
const OPS: usize = 300_000;

fn hasher() -> ahash::RandomState {
  ahash::RandomState::with_seeds(11, 12, 13, 14)
}

fn xorshift(x: &mut u64) -> u64 {
  *x ^= *x << 13;
  *x ^= *x >> 7;
  *x ^= *x << 17;
  *x
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
  Get(u64),
  Insert(u64),
  Remove(u64),
}

/// Zipf-skewed keys over ten times the capacity: 70% gets, 25% inserts (some
/// of resident keys), 5% removes.
fn trace(seed: u64) -> Vec<Op> {
  let universe = CAPACITY * 10;
  let mut cdf = Vec::with_capacity(universe as usize);
  let mut acc = 0.0;
  for r in 1..=universe {
    acc += 1.0 / (r as f64).powf(0.99);
    cdf.push(acc);
  }
  let mut x = seed | 1;
  (0..OPS)
    .map(|_| {
      let u = (xorshift(&mut x) >> 11) as f64 / (1u64 << 53) as f64 * acc;
      let key = cdf.partition_point(|&c| c < u).min(universe as usize - 1) as u64;
      match xorshift(&mut x) % 100 {
        0..=69 => Op::Get(key),
        70..=94 => Op::Insert(key),
        _ => Op::Remove(key),
      }
    })
    .collect()
}

struct OracleShard {
  map: HashMap<u64, u64>,
  cost: u64,
  policy: Box<dyn CachePolicy<u64, u64>>,
  dump: Option<Box<dyn Fn() -> (Vec<(u64, u8)>, Vec<(u64, u8)>, Vec<u64>, Vec<u64>)>>,
}

/// The reference: `ShardedStore::new` splits the capacity the same way, and
/// `ShardMap::insert_policy` calls the policy in the same order.
struct Oracle {
  shards: Vec<OracleShard>,
  budget: u64,
  hasher: ahash::RandomState,
}

impl Oracle {
  fn new(shards: usize, factory: &Factory) -> Self {
    Oracle {
      shards: (0..shards).map(|_| OracleShard { map: HashMap::new(), cost: 0, policy: factory(), dump: None }).collect(),
      budget: (CAPACITY / shards as u64).max(1),
      hasher: hasher(),
    }
  }

  fn shard(&mut self, key: u64) -> &mut OracleShard {
    let n = self.shards.len();
    &mut self.shards[hash_key(&self.hasher, &key) as usize & (n - 1)]
  }

  fn get(&mut self, key: u64) -> bool {
    let s = self.shard(key);
    match s.map.get(&key) {
      Some(&cost) => {
        s.policy.on_access(&key, cost);
        true
      }
      None => false,
    }
  }

  fn insert(&mut self, key: u64, cost: u64) {
    let budget = self.budget;
    let s = self.shard(key);
    let decision = s.policy.on_admit(&key, cost);
    if let Some(old) = s.map.insert(key, cost) {
      s.cost -= old;
    }
    s.cost += cost;
    if let AdmissionDecision::AdmitAndEvict(victims) = decision {
      for v in victims {
        if let Some(c) = s.map.remove(&v) {
          s.policy.on_remove(&v);
          s.cost -= c;
        }
      }
    }
    if s.cost > budget {
      let (victims, _) = s.policy.evict(s.cost - budget);
      for v in victims {
        if let Some(c) = s.map.remove(&v) {
          s.cost -= c;
        }
      }
    }
  }

  fn remove(&mut self, key: u64) {
    let s = self.shard(key);
    if let Some(c) = s.map.remove(&key) {
      s.policy.on_remove(&key);
      s.cost -= c;
    }
  }

  fn cost(&self) -> u64 {
    self.shards.iter().map(|s| s.cost).sum()
  }

  /// The oracle S3-FIFO state of shard `idx` with keys hashed, for comparison
  /// with the in-shard policy.
  fn s3fifo_state(&self, idx: usize) -> Option<(Vec<(u64, u8)>, Vec<(u64, u8)>, Vec<u64>, Vec<u64>)> {
    let d = self.shards[idx].dump.as_ref()?;
    let (small, main, mut set, order) = d();
    let h = &self.hasher;
    let hk = |k: &u64| hash_key(h, k);
    set.sort_unstable_by_key(|k| hk(k));
    Some((
      small.iter().map(|(k, f)| (hk(k), *f)).collect(),
      main.iter().map(|(k, f)| (hk(k), *f)).collect(),
      set.iter().map(hk).collect(),
      order.iter().map(hk).collect(),
    ))
  }

  fn keys(&self) -> Vec<u64> {
    let mut v: Vec<u64> = self.shards.iter().flat_map(|s| s.map.keys().copied()).collect();
    v.sort_unstable();
    v
  }
}

fn cache_keys(cache: &Cache<u64, u64, ahash::RandomState>) -> Vec<u64> {
  let mut v: Vec<u64> = cache.iter().map(|(k, _)| k).collect();
  v.sort_unstable();
  v
}

/// Runs the trace through both and compares after every operation.
fn lockstep(name: &str, shards: usize, cache: Cache<u64, u64, ahash::RandomState>, factory: &Factory) {
  let oracle = Oracle::new(shards, factory);
  run_lockstep(name, shards, cache, oracle);
}

fn run_lockstep(name: &str, shards: usize, cache: Cache<u64, u64, ahash::RandomState>, mut oracle: Oracle) {
  let exact = true;
  let ops = trace(shards as u64 + 1);
  let check_every: usize = std::env::var("LOCKSTEP_CHECK_EVERY").ok().and_then(|v| v.parse().ok()).unwrap_or(499);
  let (mut hits, mut checks) = (0u64, 0u64);
  for (i, op) in ops.iter().enumerate() {
    match *op {
      Op::Get(k) => {
        let a = oracle.get(k);
        let b = cache.fetch(&k).is_some();
        if exact {
          assert_eq!(a, b, "{name} shards={shards} op {i} get {k}: oracle hit {a}, cache hit {b}");
        }
        hits += a as u64;
      }
      Op::Insert(k) => {
        oracle.insert(k, 1);
        cache.insert(k, k, 1);
      }
      Op::Remove(k) => {
        oracle.remove(k);
        cache.remove(&k);
      }
    }
    let (oc, cc) = (oracle.cost(), cache.metrics().current_cost);
    assert_eq!(oc, cc, "{name} shards={shards} op {i} {op:?}: oracle cost {oc}, cache cost {cc}");
    if i % check_every == 0 {
      let key = match *op {
        Op::Get(k) | Op::Insert(k) | Op::Remove(k) => k,
      };
      let idx = hash_key(&hasher(), &key) as usize & (shards - 1);
      if let Some(expected) = oracle.s3fifo_state(idx) {
        let actual = cache.shared.store.shards[idx].map.read().s3fifo_state().expect("in-shard s3fifo");
        if expected != actual {
          println!("prior op: {:?}", ops.get(i.wrapping_sub(1)));
          println!("oracle small {:?}\n       main {:?}\n       ghost_set {:?}\n       ghost_order {:?}", expected.0, expected.1, expected.2, expected.3);
          println!("cache  small {:?}\n       main {:?}\n       ghost_set {:?}\n       ghost_order {:?}", actual.0, actual.1, actual.2, actual.3);
          panic!("{name} shards={shards} op {i} {op:?} shard {idx}: policy state differs");
        }
      }
    }
    if exact && (i % check_every == 0 || i + 1 == ops.len()) {
      let (a, b) = (oracle.keys(), cache_keys(&cache));
      if a != b {
        let only_oracle: Vec<u64> = a.iter().filter(|k| !b.contains(k)).copied().collect();
        let only_cache: Vec<u64> = b.iter().filter(|k| !a.contains(k)).copied().collect();
        let key = match *op {
          Op::Get(k) | Op::Insert(k) | Op::Remove(k) => k,
        };
        let idx = hash_key(&hasher(), &key) as usize & (shards - 1);
        let h = hasher();
        let hashes: Vec<(u64, u64)> = only_oracle.iter().chain(only_cache.iter()).map(|k| (*k, hash_key(&h, k))).collect();
        println!("key hashes {hashes:?}; op key {key} hash {}", hash_key(&h, &key));
        println!("prior op: {:?}", ops.get(i.wrapping_sub(1)));
        panic!("{name} shards={shards} op {i} {op:?}: resident sets differ; only oracle {only_oracle:?}, only cache {only_cache:?}");
      }
      checks += 1;
    }
  }
  println!("{name:8} shards={shards:>3}: {OPS} ops, {checks} set checks, oracle hit ratio {:.4}", hits as f64 / OPS as f64);
}

fn builder(shards: usize) -> CacheBuilder<u64, u64, ahash::RandomState> {
  CacheBuilder::<u64, u64, ahash::RandomState>::new().hasher(hasher()).capacity(CAPACITY).shards(shards)
}

fn shard_capacity(shards: usize) -> u64 {
  (CAPACITY as f64 / shards as f64).ceil() as u64
}

fn policies(shards: usize) -> Vec<(&'static str, Factory)> {
  let cap = shard_capacity(shards);
  let h = hasher();
  vec![
    ("tinylfu", Arc::new(move || Box::new(TinyLfuPolicy::with_sketch_hasher(cap, h.clone())) as Box<dyn CachePolicy<u64, u64>>)),
    ("s3fifo", Arc::new(move || Box::new(S3FifoPolicy::new(cap)) as Box<dyn CachePolicy<u64, u64>>)),
    ("lru", Arc::new(|| Box::new(LruPolicy::new()) as Box<dyn CachePolicy<u64, u64>>)),
    ("slru", Arc::new(move || Box::new(SlruPolicy::new(cap)) as Box<dyn CachePolicy<u64, u64>>)),
    ("arc", Arc::new(move || Box::new(ArcPolicy::new(cap as usize)) as Box<dyn CachePolicy<u64, u64>>)),
    ("fifo", Arc::new(|| Box::new(Fifo::new()) as Box<dyn CachePolicy<u64, u64>>)),
    ("clock", Arc::new(|| Box::new(ClockPolicy::new()) as Box<dyn CachePolicy<u64, u64>>)),
    ("sieve", Arc::new(|| Box::new(SievePolicy::new()) as Box<dyn CachePolicy<u64, u64>>)),
  ]
}

/// Every shipped policy through `cache_policy_factory`, at 1, 4 and 64 shards.
#[test]
fn every_policy_through_the_factory_matches_the_oracle() {
  for shards in [1usize, 4, 64] {
    for (name, factory) in policies(shards) {
      let f = factory.clone();
      let cache = builder(shards).cache_policy_factory(move || f()).build().expect("build");
      lockstep(name, shards, cache, &factory);
    }
  }
}

/// The in-shard TinyLFU (`tinylfu()` on the builder) against the key-based policy with the
/// same sketch seeds: the cache hashes keys with `hasher()`, so the sketch
/// built on that hasher indexes as the shard's does.
#[test]
fn builtin_tinylfu_matches_the_oracle() {
  for shards in [1usize, 4, 64] {
    let cap = shard_capacity(shards);
    let h = hasher();
    let factory: Factory = Arc::new(move || Box::new(TinyLfuPolicy::with_sketch_hasher(cap, h.clone())));
    let cache = builder(shards).tinylfu().build().expect("build");
    lockstep("tinylfu*", shards, cache, &factory);
  }
}

/// The in-shard S3-FIFO, the default for a bounded cache, against the key-based policy.
#[test]
fn builtin_s3fifo_matches_the_oracle() {
  let only: Option<usize> = std::env::var("LOCKSTEP_SHARDS").ok().and_then(|v| v.parse().ok());
  for shards in [1usize, 4, 64].into_iter().filter(|s| only.map_or(true, |o| o == *s)) {
    let cap = shard_capacity(shards);
    let factory: Factory = Arc::new(move || Box::new(S3FifoPolicy::new(cap)));
    let cache = builder(shards).build().expect("build");
    lockstep_with_dump("s3fifo*", shards, cache, &factory, cap);
  }
}

/// Wraps a policy and keeps the set of keys the policy considers resident:
/// admitted, less what it evicted or was told was removed.
struct Recorded<P> {
  inner: P,
  model: Arc<Mutex<HashSet<u64>>>,
}

impl<P: CachePolicy<u64, u64>> CachePolicy<u64, u64> for Recorded<P> {
  fn on_access(&self, key: &u64, cost: u64) {
    self.inner.on_access(key, cost);
  }
  fn on_admit(&self, key: &u64, cost: u64) -> AdmissionDecision<u64> {
    let decision = self.inner.on_admit(key, cost);
    let mut m = self.model.lock();
    m.insert(*key);
    if let AdmissionDecision::AdmitAndEvict(victims) = &decision {
      for v in victims {
        m.remove(v);
      }
    }
    decision
  }
  fn on_remove(&self, key: &u64) {
    self.inner.on_remove(key);
    self.model.lock().remove(key);
  }
  fn evict(&self, cost_to_free: u64) -> (Vec<u64>, u64) {
    let out = self.inner.evict(cost_to_free);
    let mut m = self.model.lock();
    for v in &out.0 {
      m.remove(v);
    }
    out
  }
  fn clear(&self) {
    self.inner.clear();
    self.model.lock().clear();
  }
}

/// A policy whose victims are random has no oracle, so the check is that the
/// cache holds exactly the keys the policy's own calls say it holds.
#[test]
fn random_policy_decisions_are_applied_exactly() {
  for shards in [1usize, 4, 64] {
    let models: Arc<Mutex<Vec<Arc<Mutex<HashSet<u64>>>>>> = Arc::new(Mutex::new(Vec::new()));
    let m2 = models.clone();
    let cache = builder(shards)
      .cache_policy_factory(move || {
        let model = Arc::new(Mutex::new(HashSet::new()));
        m2.lock().push(model.clone());
        Box::new(Recorded { inner: RandomPolicy::<u64>::new(), model })
      })
      .build()
      .expect("build");
    let ops = trace(shards as u64 + 1);
    for (i, op) in ops.iter().enumerate() {
      match *op {
        Op::Get(k) => {
          cache.fetch(&k);
        }
        Op::Insert(k) => cache.insert(k, k, 1),
        Op::Remove(k) => {
          cache.remove(&k);
        }
      }
      if i % 499 == 0 || i + 1 == ops.len() {
        let mut expected: Vec<u64> = models.lock().iter().flat_map(|m| m.lock().iter().copied().collect::<Vec<_>>()).collect();
        expected.sort_unstable();
        assert_eq!(expected, cache_keys(&cache), "random shards={shards} op {i} {op:?}: cache holds keys the policy does not");
        assert!(cache.metrics().current_cost <= CAPACITY, "random shards={shards} op {i}: over capacity");
      }
    }
    println!("random   shards={shards:>3}: {OPS} ops, policy decisions applied exactly");
  }
}

/// `lockstep` for S3-FIFO with the oracle policies kept reachable for a dump.
fn lockstep_with_dump(name: &str, shards: usize, cache: Cache<u64, u64, ahash::RandomState>, factory: &Factory, cap: u64) {
  let _ = factory;
  let policies: Vec<Arc<S3FifoPolicy<u64>>> = (0..shards).map(|_| Arc::new(S3FifoPolicy::new(cap))).collect();
  let ps = policies.clone();
  let shared: Factory = {
    let next = Arc::new(Mutex::new(0usize));
    Arc::new(move || {
      let mut n = next.lock();
      let p = ps[*n].clone();
      *n += 1;
      Box::new(SharedPolicy(p))
    })
  };
  let mut oracle = Oracle::new(shards, &shared);
  for (i, s) in oracle.shards.iter_mut().enumerate() {
    let p = policies[i].clone();
    s.dump = Some(Box::new(move || p.debug_state()));
  }
  run_lockstep(name, shards, cache, oracle);
}

struct SharedPolicy(Arc<S3FifoPolicy<u64>>);

impl CachePolicy<u64, u64> for SharedPolicy {
  fn on_access(&self, key: &u64, cost: u64) {
    CachePolicy::<u64, u64>::on_access(&*self.0, key, cost)
  }
  fn on_admit(&self, key: &u64, cost: u64) -> AdmissionDecision<u64> {
    CachePolicy::<u64, u64>::on_admit(&*self.0, key, cost)
  }
  fn on_remove(&self, key: &u64) {
    CachePolicy::<u64, u64>::on_remove(&*self.0, key)
  }
  fn evict(&self, cost_to_free: u64) -> (Vec<u64>, u64) {
    CachePolicy::<u64, u64>::evict(&*self.0, cost_to_free)
  }
  fn clear(&self) {
    CachePolicy::<u64, u64>::clear(&*self.0)
  }
}
