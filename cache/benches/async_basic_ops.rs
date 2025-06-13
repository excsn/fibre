use bench_matrix::{
  criterion_runner::async_suite::AsyncBenchmarkSuite, AbstractCombination, MatrixCellValue,
};
use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};
use fibre_cache::{builder::CacheBuilder, AsyncCache};
use futures_util::future;
use rand::prelude::{SliceRandom, StdRng};
use rand::SeedableRng;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::runtime::Runtime;
use tokio::sync::Barrier;

// --- Config, State, Context ---

#[derive(Debug, Clone)]
struct BenchConfig {
  op_type: String,
  capacity: u64,
  num_items: usize,
  concurrency: usize,
}

struct BenchState {
  cache: Arc<AsyncCache<u64, u64>>,
  keys_by_task: Vec<Vec<u64>>,
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

fn setup_fn(
  _rt: &Runtime,
  cfg: &BenchConfig,
) -> Pin<Box<dyn Future<Output = Result<(BenchContext, BenchState), String>> + Send>> {
  let cfg = cfg.clone();
  Box::pin(async move {
    let cache = Arc::new(
      CacheBuilder::default()
        .capacity(cfg.capacity)
        .build_async()
        .unwrap(),
    );

    // Pre-populate the cache.
    for i in 0..cfg.num_items {
      cache.insert(i as u64, i as u64, 1).await;
    }

    let mut workload_keys: Vec<u64> = match cfg.op_type.as_str() {
      "GetHit" => (0..cfg.num_items as u64).collect(),
      "GetMiss" => (cfg.num_items as u64..2 * cfg.num_items as u64).collect(),
      "Insert" => (cfg.num_items as u64..2 * cfg.num_items as u64).collect(),
      _ => return Err("Invalid operation type".to_string()),
    };

    let mut rng = StdRng::from_seed([0; 32]);
    workload_keys.shuffle(&mut rng);

    let mut keys_by_task = vec![Vec::new(); cfg.concurrency];
    for (i, key) in workload_keys.into_iter().enumerate() {
      keys_by_task[i % cfg.concurrency].push(key);
    }

    Ok((
      (),
      BenchState {
        cache,
        keys_by_task,
      },
    ))
  })
}

fn benchmark_logic(
  _ctx: BenchContext,
  state: BenchState,
  cfg: &BenchConfig,
) -> Pin<Box<dyn Future<Output = (BenchContext, BenchState, Duration)> + Send>> {
  let op_type = cfg.op_type.clone();
  Box::pin(async move {
    let barrier = Arc::new(Barrier::new(state.keys_by_task.len()));
    let mut tasks = Vec::with_capacity(state.keys_by_task.len());

    let start_time = Instant::now();

    for task_keys in state.keys_by_task.iter() {
      // We need to clone the Arcs and other data for each spawned task.
      let barrier_clone = barrier.clone();
      let cache_clone = state.cache.clone();
      let op_type = op_type.clone();
      let task_keys: Vec<u64> = task_keys.clone(); // Clone keys for the task

      tasks.push(tokio::spawn(async move {
        barrier_clone.wait().await; // Synchronize tasks

        match op_type.as_str() {
          "GetHit" | "GetMiss" => {
            for key in &task_keys {
              black_box(cache_clone.get(key));
            }
          }
          "Insert" => {
            for key in &task_keys {
              cache_clone.insert(*key, *key, 1).await;
            }
          }
          _ => unreachable!(),
        }
      }));
    }

    future::join_all(tasks).await;

    let duration = start_time.elapsed();
    ((), state, duration)
  })
}

fn teardown_fn(
  _ctx: BenchContext,
  _state: BenchState,
  _rt: &Runtime,
  _cfg: &BenchConfig,
) -> Pin<Box<dyn Future<Output = ()> + Send>> {
  Box::pin(async move {})
}

fn async_benches(c: &mut Criterion) {
  let rt = Runtime::new().expect("Failed to create Tokio runtime");

  let parameter_axes = vec![
    vec![
      MatrixCellValue::String("GetHit".to_string()),
      MatrixCellValue::String("GetMiss".to_string()),
      MatrixCellValue::String("Insert".to_string()),
    ], // Operation Type
    vec![
      MatrixCellValue::Unsigned(10_000),
      MatrixCellValue::Unsigned(100_000),
    ], // Cache Capacity
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
    "Tasks".to_string(), // New parameter name
  ];

  AsyncBenchmarkSuite::new(
    c,
    &rt,
    "AsyncBasicOps".to_string(),
    Some(parameter_names),
    parameter_axes,
    Box::new(extract_config),
    setup_fn,
    benchmark_logic,
    teardown_fn,
  )
  .throughput(|cfg: &BenchConfig| Throughput::Elements(cfg.num_items as u64))
  .run();
}

criterion_group!(benches, async_benches);
criterion_main!(benches);
