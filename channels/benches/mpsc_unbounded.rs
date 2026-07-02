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
struct MpscUnboundedBenchConfig {
  num_producers: usize,
  total_items: usize,
}

#[derive(Default, Debug)]
struct BenchContext {
  actual_items_processed: usize,
}

#[derive(Clone)]
struct MpscUnboundedBenchState;

// Extractor function used by both Sync and Async suites
fn extract_mpsc_config(combo: &AbstractCombination) -> Result<MpscUnboundedBenchConfig, String> {
  let num_producers = combo.get_u64(0)? as usize;
  let total_items = combo.get_u64(1)? as usize;

  if num_producers == 0 {
    return Err("Number of producers must be at least 1.".to_string());
  }

  Ok(MpscUnboundedBenchConfig {
    num_producers,
    total_items,
  })
}

// ==========================================
// --- SECTION 2: Synchronous Implementation ---
// ==========================================

fn setup_fn_mpsc_sync(
  _cfg: &MpscUnboundedBenchConfig,
) -> Result<(BenchContext, MpscUnboundedBenchState), String> {
  Ok((BenchContext::default(), MpscUnboundedBenchState))
}

fn benchmark_logic_mpsc_sync(
  mut ctx: BenchContext,
  state: MpscUnboundedBenchState,
  cfg: &MpscUnboundedBenchConfig,
) -> (BenchContext, MpscUnboundedBenchState, Duration) {
  // Create a fresh synchronous unbounded channel
  let (tx, rx) = mpsc::unbounded();

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
        let mut tx_clone = tx.clone();
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
  _cfg: &MpscUnboundedBenchConfig,
) -> Pin<Box<dyn Future<Output = Result<(BenchContext, MpscUnboundedBenchState), String>> + Send>> {
  Box::pin(async move { Ok((BenchContext::default(), MpscUnboundedBenchState)) })
}

fn benchmark_logic_mpsc_async(
  mut ctx: BenchContext,
  state: MpscUnboundedBenchState,
  cfg: &MpscUnboundedBenchConfig,
) -> Pin<Box<dyn Future<Output = (BenchContext, MpscUnboundedBenchState, Duration)> + Send>> {
  let cfg_clone = cfg.clone();
  Box::pin(async move {
    // Create a fresh asynchronous unbounded channel
    let (tx, mut rx) = mpsc::unbounded_async();

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
        let mut tx_clone = tx.clone();
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

fn mpsc_unbounded_sync_benches(c: &mut Criterion) {
  let core_count = usize::from(available_parallelism().unwrap()) as u64;
  let parameter_axes = vec![
    vec![
      // Axis 0: Num Senders
      MatrixCellValue::Unsigned(1),
      MatrixCellValue::Unsigned(4),
      MatrixCellValue::Unsigned(core_count),
    ],
    vec![
      // Axis 1: Total Items
      MatrixCellValue::Unsigned(100_000),
      MatrixCellValue::Unsigned(1_000_000),
      MatrixCellValue::Unsigned(10_000_000),
    ],
  ];
  let parameter_names = vec!["Prod".to_string(), "Items".to_string()];

  SyncBenchmarkSuite::new(
    c,
    "MpscUnboundedSync".to_string(),
    Some(parameter_names),
    parameter_axes,
    Box::new(extract_mpsc_config),
    setup_fn_mpsc_sync,
    benchmark_logic_mpsc_sync,
    |_, _, _| {}, // Teardown
  )
  .throughput(|cfg: &MpscUnboundedBenchConfig| Throughput::Elements(cfg.total_items as u64))
  .run();
}

fn mpsc_unbounded_async_benches(c: &mut Criterion) {
  let rt = Runtime::new().unwrap();
  let core_count = usize::from(available_parallelism().unwrap()) as u64;
  let parameter_axes = vec![
    vec![
      // Axis 0: Num Senders
      MatrixCellValue::Unsigned(1),
      MatrixCellValue::Unsigned(4),
      MatrixCellValue::Unsigned(core_count),
    ],
    vec![
      // Axis 1: Total Items
      MatrixCellValue::Unsigned(100_000),
      MatrixCellValue::Unsigned(1_000_000),
      MatrixCellValue::Unsigned(10_000_000),
    ],
  ];
  let parameter_names = vec!["Prod".to_string(), "Items".to_string()];

  AsyncBenchmarkSuite::new(
    c,
    &rt,
    "MpscUnboundedAsync".to_string(),
    Some(parameter_names),
    parameter_axes,
    Box::new(extract_mpsc_config),
    setup_fn_mpsc_async,
    benchmark_logic_mpsc_async,
    |_, _, _, _| Box::pin(async {}), // Teardown
  )
  .throughput(|cfg: &MpscUnboundedBenchConfig| Throughput::Elements(cfg.total_items as u64))
  .run();
}

criterion_group!(
  benches,
  mpsc_unbounded_sync_benches,
  mpsc_unbounded_async_benches
);
criterion_main!(benches);
