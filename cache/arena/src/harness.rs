//! Fixed-duration driver. Every cell runs for `secs` and reports how many
//! operations its workers completed. Workers stay alive for the whole run, keys
//! are shuffled and dealt round-robin and each worker checks the stop flag
//! every 256 operations. Sync workers are OS threads; async workers are tasks on
//! a multi-thread tokio runtime with one worker thread per task; mixed puts half
//! the workers on each side of the same instance. A sampler thread reads the
//! entry count every millisecond to record the peak.

use crate::{AsyncCache, Engine, SyncCache};
use std::hint::black_box;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Barrier};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
  /// Every key is resident; each operation is a get.
  Hit,
  /// Each operation inserts a key not yet seen, after a prefill to capacity.
  Insert,
  /// Keys drawn from a Zipf distribution (exponent 0.99) over ten times the
  /// capacity, starting from an empty cache; a miss inserts the key.
  Zipf,
  /// Every worker gets the same key on every operation: the last one prefilled, so it is resident.
  Hot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
  Sync,
  Async,
  Mixed,
}

#[derive(Debug, Clone, Copy)]
pub struct Cell {
  pub op: Op,
  pub mode: Mode,
  pub workers: usize,
}

impl Cell {
  pub fn parse(s: &str) -> Result<Cell, String> {
    let parts: Vec<&str> = s.split_whitespace().collect();
    let [op, mode, workers] = parts.as_slice() else {
      return Err(format!("cell `{s}`: expected `<op> <mode> <workers>`"));
    };
    let op = match *op {
      "hit" => Op::Hit,
      "insert" => Op::Insert,
      "zipf" => Op::Zipf,
      "hot" => Op::Hot,
      o => return Err(format!("unknown op `{o}`")),
    };
    let mode = match *mode {
      "sync" => Mode::Sync,
      "async" => Mode::Async,
      "mixed" => Mode::Mixed,
      m => return Err(format!("unknown mode `{m}`")),
    };
    let workers: usize = workers.parse().map_err(|_| format!("bad worker count `{workers}`"))?;
    if workers == 0 || (mode == Mode::Mixed && workers < 2) {
      return Err(format!("cell `{s}`: too few workers"));
    }
    Ok(Cell { op, mode, workers })
  }
}

#[derive(Debug, Clone, Copy)]
pub struct Cfg {
  pub cell: Cell,
  pub capacity: u64,
  pub secs: f64,
}

#[derive(Debug, Clone)]
pub struct Report {
  pub total: u64,
  pub hits: u64,
  pub elapsed: f64,
  /// Highest entry count the sampler saw during the timed run.
  pub peak_len: u64,
  /// Entry count when the workers stopped.
  pub end_len: u64,
  /// Entry count after the implementation's pending work was run.
  pub settled_len: u64,
}

impl Report {
  pub fn mops(&self) -> f64 {
    self.total as f64 / self.elapsed / 1e6
  }
  pub fn hit_ratio(&self) -> f64 {
    self.hits as f64 / self.total.max(1) as f64
  }
}

const CHUNK: usize = 256;
const ZIPF_SAMPLES: usize = 1 << 20;
const ZIPF_EXPONENT: f64 = 0.99;

fn xorshift(x: &mut u64) -> u64 {
  *x ^= *x << 13;
  *x ^= *x >> 7;
  *x ^= *x << 17;
  *x
}

fn shuffled(n: u64, seed: u64) -> Vec<u64> {
  let mut v: Vec<u64> = (0..n).collect();
  let mut x = seed | 1;
  for i in (1..v.len()).rev() {
    v.swap(i, (xorshift(&mut x) % (i as u64 + 1)) as usize);
  }
  v
}

fn zipf_keys(universe: u64, seed: u64) -> Vec<u64> {
  let mut cdf = Vec::with_capacity(universe as usize);
  let mut acc = 0.0;
  for r in 1..=universe {
    acc += 1.0 / (r as f64).powf(ZIPF_EXPONENT);
    cdf.push(acc);
  }
  let perm = shuffled(universe, 11);
  let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
  (0..ZIPF_SAMPLES)
    .map(|_| {
      let u = (xorshift(&mut x) >> 11) as f64 / (1u64 << 53) as f64 * acc;
      let rank = cdf.partition_point(|&c| c < u).min(universe as usize - 1);
      perm[rank]
    })
    .collect()
}

/// The hit ratio of a cache that always holds the `capacity` most frequent keys
/// of the `zipf` op's distribution, the most any policy can expect on it.
pub fn zipf_ceiling(capacity: u64) -> f64 {
  let weight = |r: u64| 1.0 / (r as f64).powf(ZIPF_EXPONENT);
  let top: f64 = (1..=capacity).map(weight).sum();
  let all: f64 = top + (capacity + 1..=capacity * 10).map(weight).sum::<f64>();
  top / all
}

fn keys_for(cfg: &Cfg) -> Vec<Vec<u64>> {
  let workers = cfg.cell.workers;
  if cfg.cell.op == Op::Zipf {
    return (0..workers).map(|w| zipf_keys(cfg.capacity * 10, w as u64 + 1)).collect();
  }
  if cfg.cell.op == Op::Hot {
    return (0..workers).map(|_| vec![cfg.capacity - 1; CHUNK]).collect();
  }
  let base = shuffled(cfg.capacity, 7);
  let keys: Vec<u64> = match cfg.cell.op {
    Op::Hit => base,
    Op::Insert => base.into_iter().map(|k| k + cfg.capacity).collect(),
    Op::Zipf | Op::Hot => unreachable!(),
  };
  (0..workers).map(|w| keys.iter().copied().skip(w).step_by(workers).collect()).collect()
}

fn sync_worker<S: SyncCache>(c: &S, op: Op, keys: &[u64], stop: &AtomicBool) -> (u64, u64) {
  let (mut n, mut hits) = (0u64, 0u64);
  loop {
    for chunk in keys.chunks(CHUNK) {
      for &k in chunk {
        match op {
          Op::Hit | Op::Hot => hits += black_box(c.get(k)) as u64,
          Op::Insert => c.insert(k),
          Op::Zipf => {
            if c.get(k) {
              hits += 1;
            } else {
              c.insert(k);
            }
          }
        }
      }
      n += chunk.len() as u64;
      if stop.load(Ordering::Relaxed) {
        return (n, hits);
      }
    }
  }
}

async fn async_worker<A: AsyncCache>(c: A, op: Op, keys: Vec<u64>, stop: Arc<AtomicBool>) -> (u64, u64) {
  let (mut n, mut hits) = (0u64, 0u64);
  loop {
    for chunk in keys.chunks(CHUNK) {
      for &k in chunk {
        match op {
          Op::Hit | Op::Hot => hits += black_box(c.get(k).await) as u64,
          Op::Insert => c.insert(k).await,
          Op::Zipf => {
            if c.get(k).await {
              hits += 1;
            } else {
              c.insert(k).await;
            }
          }
        }
      }
      n += chunk.len() as u64;
      if stop.load(Ordering::Relaxed) {
        return (n, hits);
      }
    }
  }
}

/// Runs one cell against `E`. Returns `None` when `E` has no API for the
/// cell's mode.
pub fn run<E: Engine>(cfg: &Cfg) -> Option<Report> {
  let mode = cfg.cell.mode;
  let (n_sync, n_async) = match mode {
    Mode::Sync => (cfg.cell.workers, 0),
    Mode::Async => (0, cfg.cell.workers),
    Mode::Mixed => (cfg.cell.workers / 2, cfg.cell.workers - cfg.cell.workers / 2),
  };
  let rt = tokio::runtime::Builder::new_multi_thread()
    .worker_threads(n_async.max(1))
    .enable_all()
    .build()
    .expect("runtime");
  let (sync_c, async_c) = {
    let _enter = rt.enter();
    match mode {
      Mode::Sync => (Some(E::build_sync(cfg.capacity)?), None),
      Mode::Async => (None, Some(E::build_async(cfg.capacity)?)),
      Mode::Mixed => {
        let (s, a) = E::build_mixed(cfg.capacity)?;
        (Some(s), Some(a))
      }
    }
  };
  let len = || match (&sync_c, &async_c) {
    (Some(s), _) => s.len(),
    (None, Some(a)) => a.len(),
    (None, None) => unreachable!(),
  };
  let settle = || match (&sync_c, &async_c) {
    (Some(s), _) => s.settle(),
    (None, Some(a)) => rt.block_on(a.settle()),
    (None, None) => unreachable!(),
  };

  if cfg.cell.op != Op::Zipf {
    match (&sync_c, &async_c) {
      (Some(s), _) => (0..cfg.capacity).for_each(|k| s.insert(k)),
      (None, Some(a)) => rt.block_on(async {
        for k in 0..cfg.capacity {
          a.insert(k).await;
        }
      }),
      (None, None) => unreachable!(),
    }
    settle();
  }

  let per_worker = keys_for(cfg);
  let stop = Arc::new(AtomicBool::new(false));
  let sampling = AtomicBool::new(true);
  let peak = AtomicU64::new(0);
  let barrier = Arc::new(Barrier::new(n_sync + n_async + 1));

  std::thread::scope(|s| {
    s.spawn(|| {
      while sampling.load(Ordering::Relaxed) {
        peak.fetch_max(len(), Ordering::Relaxed);
        std::thread::sleep(Duration::from_millis(1));
      }
    });
    let mut sync_handles = Vec::new();
    if let Some(c) = &sync_c {
      for keys in per_worker.iter().take(n_sync) {
        let (c, stop, barrier, op) = (c.clone(), stop.clone(), barrier.clone(), cfg.cell.op);
        sync_handles.push(s.spawn(move || {
          barrier.wait();
          sync_worker(&c, op, keys, &stop)
        }));
      }
    }
    let mut async_handles = Vec::new();
    if let Some(c) = &async_c {
      for keys in per_worker.iter().skip(n_sync) {
        let (c, stop, barrier, op, keys) = (c.clone(), stop.clone(), barrier.clone(), cfg.cell.op, keys.clone());
        async_handles.push(rt.spawn(async move {
          tokio::task::block_in_place(|| barrier.wait());
          async_worker(c, op, keys, stop).await
        }));
      }
    }
    barrier.wait();
    let start = Instant::now();
    std::thread::sleep(Duration::from_secs_f64(cfg.secs));
    stop.store(true, Ordering::Relaxed);
    let (mut total, mut hits) = (0u64, 0u64);
    for h in sync_handles {
      let (n, hi) = h.join().expect("sync worker");
      total += n;
      hits += hi;
    }
    for h in async_handles {
      let (n, hi) = rt.block_on(h).expect("async worker");
      total += n;
      hits += hi;
    }
    let elapsed = start.elapsed().as_secs_f64();
    sampling.store(false, Ordering::Relaxed);
    let end_len = len();
    settle();
    let settled_len = len();
    Some(Report {
      total,
      hits,
      elapsed,
      peak_len: peak.load(Ordering::Relaxed).max(end_len),
      end_len,
      settled_len,
    })
  })
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn zipf_ceiling_at_ten_thousand() {
    let c = zipf_ceiling(10_000);
    assert!((c - 0.8001).abs() < 0.0005, "{c}");
  }
}
