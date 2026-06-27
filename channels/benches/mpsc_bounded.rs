use bench_matrix::{
  AbstractCombination, MatrixCellValue,
  criterion_runner::{async_suite::AsyncBenchmarkSuite, sync_suite::SyncBenchmarkSuite},
};
use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use fibre::mpsc;
use std::{
  future::Future,
  pin::Pin,
  thread::{self, available_parallelism},
  time::{Duration, Instant},
};
use tokio::runtime::Runtime;

const ITEM_VALUE: u64 = 42;

// ==========================================
// --- SECTION 1: Shared Configuration & Setup ---
// ==========================================

#[derive(Debug, Clone)]
struct MpscBoundedBenchConfig {
  capacity: usize,
  num_producers: usize,
  total_items: usize,
}

#[derive(Default, Debug)]
struct BenchContext {
  actual_items_processed: usize,
}

#[derive(Clone)]
struct MpscBoundedBenchState;

// Extractor function used by both Sync and Async suites
fn extract_mpsc_config(combo: &AbstractCombination) -> Result<MpscBoundedBenchConfig, String> {
  let capacity = combo.get_u64(0)? as usize;
  let num_producers = combo.get_u64(1)? as usize;
  let total_items = combo.get_u64(2)? as usize;

  if capacity == 0 {
    return Err("Capacity must be at least 1 for bounded benchmarks.".to_string());
  }
  if num_producers == 0 {
    return Err("Number of producers must be at least 1.".to_string());
  }

  Ok(MpscBoundedBenchConfig {
    capacity,
    num_producers,
    total_items,
  })
}

// ==========================================
// --- SECTION 2: Synchronous Implementation ---
// ==========================================

fn setup_fn_mpsc_sync(
  _cfg: &MpscBoundedBenchConfig,
) -> Result<(BenchContext, MpscBoundedBenchState), String> {
  Ok((BenchContext::default(), MpscBoundedBenchState))
}

fn benchmark_logic_mpsc_sync(
  mut ctx: BenchContext,
  state: MpscBoundedBenchState,
  cfg: &MpscBoundedBenchConfig,
) -> (BenchContext, MpscBoundedBenchState, Duration) {
  // Create a fresh synchronous bounded channel
  let (tx, rx) = mpsc::bounded(cfg.capacity);

  let start_time = Instant::now();

  thread::scope(|s| {
    let total_items = cfg.total_items;
    let num_producers = cfg.num_producers;
    let base_items = total_items / num_producers;
    let remainder = total_items % num_producers;

    // Spawn producer threads
    for p_idx in 0..num_producers {
      let items_this_producer = base_items + if p_idx < remainder { 1 } else { 0 };
      if items_this_producer > 0 {
        let tx_clone = tx.clone();
        s.spawn(move || {
          for _ in 0..items_this_producer {
            tx_clone.send(ITEM_VALUE).unwrap();
          }
        });
      }
    }
    drop(tx); // Drop original sender handle so receiver sees disconnect

    // Receiver runs in the current scoped thread
    for _ in 0..cfg.total_items {
      rx.recv().unwrap();
    }
  });

  let duration = start_time.elapsed();
  ctx.actual_items_processed += cfg.total_items;
  (ctx, state, duration)
}

// ==========================================
// --- SECTION 3: Asynchronous Implementation ---
// ==========================================

fn setup_fn_mpsc_async(
  _rt: &Runtime,
  _cfg: &MpscBoundedBenchConfig,
) -> Pin<Box<dyn Future<Output = Result<(BenchContext, MpscBoundedBenchState), String>> + Send>> {
  Box::pin(async move { Ok((BenchContext::default(), MpscBoundedBenchState)) })
}

fn benchmark_logic_mpsc_async(
  mut ctx: BenchContext,
  state: MpscBoundedBenchState,
  cfg: &MpscBoundedBenchConfig,
) -> Pin<Box<dyn Future<Output = (BenchContext, MpscBoundedBenchState, Duration)> + Send>> {
  let cfg_clone = cfg.clone();
  Box::pin(async move {
    // Create a fresh asynchronous bounded channel
    let (tx, rx) = mpsc::bounded_async(cfg_clone.capacity);

    let start_time = Instant::now();

    let consumer_handle = tokio::spawn(async move {
      for _ in 0..cfg_clone.total_items {
        rx.recv().await.unwrap();
      }
    });

    let mut producer_handles = Vec::new();
    let total_items = cfg_clone.total_items;
    let num_producers = cfg_clone.num_producers;
    let base_items = total_items / num_producers;
    let remainder = total_items % num_producers;

    for p_idx in 0..num_producers {
      let items_this_producer = base_items + if p_idx < remainder { 1 } else { 0 };
      if items_this_producer > 0 {
        let tx_clone = tx.clone();
        producer_handles.push(tokio::spawn(async move {
          for _ in 0..items_this_producer {
            tx_clone.send(ITEM_VALUE).await.unwrap();
          }
        }));
      }
    }
    drop(tx);

    for handle in producer_handles {
      handle.await.unwrap();
    }
    consumer_handle.await.unwrap();

    let duration = start_time.elapsed();
    ctx.actual_items_processed += cfg_clone.total_items;

    (ctx, state, duration)
  })
}

// ==========================================
// --- SECTION 4: Criterion Group Suite Setup ---
// ==========================================

fn mpsc_bounded_sync_benches(c: &mut Criterion) {
  let core_count = usize::from(available_parallelism().unwrap()) as u64;
  let parameter_axes = vec![
    vec![
      // Axis 0: Capacity
      MatrixCellValue::Unsigned(1),
      MatrixCellValue::Unsigned(4),
      MatrixCellValue::Unsigned(128),
    ],
    vec![
      // Axis 1: Num Senders
      MatrixCellValue::Unsigned(1),
      MatrixCellValue::Unsigned(4),
      MatrixCellValue::Unsigned(core_count),
    ],
    vec![
      // Axis 2: Total Items
      MatrixCellValue::Unsigned(100_000),
      MatrixCellValue::Unsigned(1_000_000),
      MatrixCellValue::Unsigned(10_000_000),
    ],
  ];
  let parameter_names = vec!["Cap".to_string(), "Prod".to_string(), "Items".to_string()];

  SyncBenchmarkSuite::new(
    c,
    "MpscBoundedSync".to_string(),
    Some(parameter_names),
    parameter_axes,
    Box::new(extract_mpsc_config),
    setup_fn_mpsc_sync,
    benchmark_logic_mpsc_sync,
    |_, _, _| {}, // Teardown
  )
  .throughput(|cfg: &MpscBoundedBenchConfig| Throughput::Elements(cfg.total_items as u64))
  .run();
}

fn mpsc_bounded_async_benches(c: &mut Criterion) {
  let rt = Runtime::new().unwrap();
  let core_count = usize::from(available_parallelism().unwrap()) as u64;
  let parameter_axes = vec![
    vec![
      // Axis 0: Capacity
      MatrixCellValue::Unsigned(1),
      MatrixCellValue::Unsigned(4),
      MatrixCellValue::Unsigned(128),
    ],
    vec![
      // Axis 1: Num Senders
      MatrixCellValue::Unsigned(1),
      MatrixCellValue::Unsigned(4),
      MatrixCellValue::Unsigned(core_count),
    ],
    vec![
      // Axis 2: Total Items
      MatrixCellValue::Unsigned(100_000),
      MatrixCellValue::Unsigned(1_000_000),
      MatrixCellValue::Unsigned(10_000_000),
    ],
  ];
  let parameter_names = vec!["Cap".to_string(), "Prod".to_string(), "Items".to_string()];

  AsyncBenchmarkSuite::new(
    c,
    &rt,
    "MpscBoundedAsync".to_string(),
    Some(parameter_names),
    parameter_axes,
    Box::new(extract_mpsc_config),
    setup_fn_mpsc_async,
    benchmark_logic_mpsc_async,
    |_, _, _, _| Box::pin(async {}), // Teardown
  )
  .throughput(|cfg: &MpscBoundedBenchConfig| Throughput::Elements(cfg.total_items as u64))
  .run();
}

criterion_group!(
  benches,
  mpsc_bounded_sync_benches,
  mpsc_bounded_async_benches
);
criterion_main!(benches);
