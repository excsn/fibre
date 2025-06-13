use std::sync::{
  atomic::{AtomicUsize, Ordering},
  Arc, Barrier,
};
use std::thread;
use std::time::{Duration, Instant};

use bench_matrix::{
  criterion_runner::sync_suite::SyncBenchmarkSuite, AbstractCombination, MatrixCellValue,
};
use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};
use fibre_cache::{builder::CacheBuilder, BuildError as CacheBuilderError, Cache};
use rand::prelude::*;
use rand_distr::Distribution;
use rand_pcg::Pcg64;

// --- Enums for Operations ---
#[derive(Clone)]
enum Op {
  Read(u64),
  Write(u64, u64),
  Compute(u64),
}

// --- Config, State, Context ---
#[derive(Debug, Clone)]
struct BenchConfig {
  workload: String,
  capacity: usize,
  num_ops: usize,
  concurrency: usize,
}

struct BenchState {
  cache: Arc<Cache<u64, u64>>,
  ops_by_thread: Vec<Vec<Op>>,
}

type BenchContext = ();

// --- Extractor Function ---
fn extract_config(combo: &AbstractCombination) -> Result<BenchConfig, String> {
  Ok(BenchConfig {
    workload: combo.get_string(0)?.to_string(),
    capacity: combo.get_u64(1)? as usize,
    num_ops: combo.get_u64(2)? as usize,
    concurrency: combo.get_u64(3)? as usize,
  })
}

// --- Benchmark Functions ---
fn setup_fn(cfg: &BenchConfig) -> Result<(BenchContext, BenchState), String> {
  let load_counter = Arc::new(AtomicUsize::new(0));
  let cache = Arc::new(
    CacheBuilder::default()
      .capacity(cfg.capacity as u64)
      .loader(move |key: u64| {
        load_counter.fetch_add(1, Ordering::Relaxed);
        (key, 1)
      })
      .build()
      .map_err(|e: CacheBuilderError| e.to_string())?,
  );

  // Pre-fill the cache to its capacity to simulate a warm state.
  for i in 0..(cfg.capacity) {
    cache.insert(i as u64, i as u64, 1);
  }

  let mut rng = Pcg64::from_rng(&mut rand::rng());
  // The new Zipf distribution requires a f64 for the capacity.
  let zipf = rand_distr::Zipf::new(cfg.capacity as f64, 1.01).unwrap();

  let mut ops_by_thread = vec![Vec::with_capacity(cfg.num_ops / cfg.concurrency); cfg.concurrency];
  let (reads, _) = match cfg.workload.as_str() {
    "Read100_Zipf" | "Read100_Uniform" | "Compute_Zipf" | "Compute_SameKey" => (100, 0),
    "Read75Write25_Zipf" => (75, 25),
    "Write100_Zipf" => (0, 100),
    _ => return Err(format!("Unknown workload: {}", cfg.workload)),
  };

  for i in 0..cfg.num_ops {
    let key = match cfg.workload.as_str() {
      "Read100_Zipf" | "Read75Write25_Zipf" | "Write100_Zipf" | "Compute_Zipf" => {
        zipf.sample(&mut rng) as u64
      }
      "Read100_Uniform" => rng.random_range(0..cfg.capacity) as u64,
      "Compute_SameKey" => 0,
      _ => 0, // Should be unreachable
    };

    let op = if cfg.workload.starts_with("Compute") {
      Op::Compute(key)
    } else if i % 100 < reads {
      Op::Read(key)
    } else {
      Op::Write(key, i as u64)
    };
    ops_by_thread[i % cfg.concurrency].push(op);
  }

  Ok((
    (),
    BenchState {
      cache,
      ops_by_thread,
    },
  ))
}

fn benchmark_logic(
  _ctx: BenchContext,
  state: BenchState,
  cfg: &BenchConfig,
) -> (BenchContext, BenchState, Duration) {
  let barrier = Arc::new(Barrier::new(cfg.concurrency));

  let start_time = Instant::now();

  thread::scope(|s| {
    for thread_ops in &state.ops_by_thread {
      let barrier_clone = barrier.clone();
      let cache_clone = state.cache.clone();

      s.spawn(move || {
        barrier_clone.wait();
        for op in thread_ops {
          match op {
            Op::Read(key) => {
              black_box(cache_clone.get(key));
            }
            Op::Write(key, value) => {
              cache_clone.insert(*key, *value, 1);
            }
            Op::Compute(key) => {
              black_box(cache_clone.get_with(key));
            }
          }
        }
      });
    }
  });

  let duration = start_time.elapsed();
  ((), state, duration)
}

fn sync_general_benches(c: &mut Criterion) {
  let parameter_axes = vec![
    vec![
      MatrixCellValue::String("Read100_Zipf".to_string()),
      MatrixCellValue::String("Read75Write25_Zipf".to_string()),
      MatrixCellValue::String("Write100_Zipf".to_string()),
      MatrixCellValue::String("Compute_SameKey".to_string()),
      MatrixCellValue::String("Compute_Zipf".to_string()),
    ],
    vec![MatrixCellValue::Unsigned(1_000_000)], // Capacity
    vec![MatrixCellValue::Unsigned(1_000_000)], // Num Ops
    vec![
      MatrixCellValue::Unsigned(1), // Concurrency
      MatrixCellValue::Unsigned(4),
      MatrixCellValue::Unsigned(8),
    ],
  ];
  let parameter_names = vec![
    "Workload".to_string(),
    "Cap".to_string(),
    "Ops".to_string(),
    "Threads".to_string(),
  ];

  SyncBenchmarkSuite::new(
    c,
    "SyncGeneral".to_string(),
    Some(parameter_names),
    parameter_axes,
    Box::new(extract_config),
    setup_fn,
    benchmark_logic,
    |_, _, _| {}, // Teardown
  )
  .throughput(|cfg: &BenchConfig| Throughput::Elements(cfg.num_ops as u64))
  .run();
}

criterion_group!(benches, sync_general_benches);
criterion_main!(benches);
