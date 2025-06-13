use bench_matrix::{
  criterion_runner::sync_suite::SyncBenchmarkSuite, AbstractCombination, MatrixCellValue,
};
use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};
use fibre_cache::{builder::CacheBuilder, Cache};
use rand::prelude::{SliceRandom, StdRng};
use rand::SeedableRng;
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::{Duration, Instant};

// --- Config, State, Context ---

#[derive(Debug, Clone)]
struct BenchConfig {
  op_type: String,
  capacity: u64,
  num_items: usize,
  concurrency: usize,
}

struct BenchState {
  cache: Arc<Cache<u64, u64>>, // Cache must be in an Arc to be shared across threads
  // Each thread gets its own set of keys to operate on
  keys_by_thread: Vec<Vec<u64>>,
}

type BenchContext = ();

// --- Extractor Function ---

fn extract_config(combo: &AbstractCombination) -> Result<BenchConfig, String> {
  Ok(BenchConfig {
    op_type: combo.get_string(0)?.to_string(),
    capacity: combo.get_u64(1)?,
    num_items: combo.get_u64(2)? as usize,
    concurrency: combo.get_u64(3)? as usize,
  })
}

// --- Benchmark Functions ---

fn setup_fn(cfg: &BenchConfig) -> Result<(BenchContext, BenchState), String> {
  let cache = Arc::new(CacheBuilder::default().capacity(cfg.capacity).build().unwrap());

  // 1. Pre-populate the cache with num_items *in a single thread* for a consistent start.
  for i in 0..cfg.num_items {
    cache.insert(i as u64, i as u64, 1);
  }

  // 2. Prepare the keys for the actual benchmark workload.
  let mut workload_keys: Vec<u64> = match cfg.op_type.as_str() {
    "GetHit" => (0..cfg.num_items as u64).collect(),
    "GetMiss" => (cfg.num_items as u64..2 * cfg.num_items as u64).collect(),
    "Insert" => (cfg.num_items as u64..2 * cfg.num_items as u64).collect(),
    _ => return Err("Invalid operation type".to_string()),
  };

  // 3. Shuffle and distribute the keys among the threads.
  let mut rng = StdRng::from_seed([0; 32]);
  workload_keys.shuffle(&mut rng);

  let mut keys_by_thread = vec![Vec::new(); cfg.concurrency];
  for (i, key) in workload_keys.into_iter().enumerate() {
    keys_by_thread[i % cfg.concurrency].push(key);
  }

  Ok((
    (),
    BenchState {
      cache,
      keys_by_thread,
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
    for thread_keys in &state.keys_by_thread {
      let barrier_clone = barrier.clone();
      let cache_clone = state.cache.clone();
      let op_type = cfg.op_type.clone();

      s.spawn(move || {
        barrier_clone.wait(); // Synchronize all threads to start at once

        match op_type.as_str() {
          "GetHit" | "GetMiss" => {
            for key in thread_keys {
              black_box(cache_clone.get(key));
            }
          }
          "Insert" => {
            for key in thread_keys {
              cache_clone.insert(*key, *key, 1);
            }
          }
          _ => unreachable!(),
        }
      });
    }
  }); // Scope waits for all spawned threads to finish

  let duration = start_time.elapsed();
  ((), state, duration)
}

fn sync_benches(c: &mut Criterion) {
  let parameter_axes = vec![
    vec![
      MatrixCellValue::String("GetHit".to_string()),
      MatrixCellValue::String("GetMiss".to_string()),
      MatrixCellValue::String("Insert".to_string()),
    ], // Operation Type
    vec![MatrixCellValue::Unsigned(10_000), MatrixCellValue::Unsigned(100_000)], // Cache Capacity
    vec![
      MatrixCellValue::Unsigned(10_000),
      MatrixCellValue::Unsigned(100_000),
    ], // Number of Operations
    vec![
      MatrixCellValue::Unsigned(1), // Concurrency
      MatrixCellValue::Unsigned(4),
      MatrixCellValue::Unsigned(8),
    ],
  ];
  let parameter_names = vec![
    "Op".to_string(),
    "Cap".to_string(),
    "Items".to_string(),
    "Threads".to_string(), // New parameter name
  ];

  SyncBenchmarkSuite::new(
    c,
    "SyncBasicOps".to_string(),
    Some(parameter_names),
    parameter_axes,
    Box::new(extract_config),
    setup_fn,
    benchmark_logic,
    |_, _, _| {}, // Teardown
  )
  .throughput(|cfg: &BenchConfig| Throughput::Elements(cfg.num_items as u64))
  .run();
}

criterion_group!(benches, sync_benches);
criterion_main!(benches);
