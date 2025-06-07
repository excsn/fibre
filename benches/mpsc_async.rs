// benches/mpsc_async.rs

use bench_matrix::{
  criterion_runner::async_suite::AsyncBenchmarkSuite, AbstractCombination, MatrixCellValue,
};
use criterion::{criterion_group, criterion_main, Criterion, Throughput};
use fibre::mpsc;
use std::{
  future::Future,
  pin::Pin,
  thread::available_parallelism,
  time::{Duration, Instant},
};
use tokio::runtime::Runtime;

const ITEM_VALUE: u64 = 42;

// --- Config, State, Context ---
#[derive(Debug, Clone)]
struct MpscBenchConfig {
  num_producers: usize,
  total_items: usize,
}

#[derive(Default, Debug)]
struct BenchContext {
  actual_items_processed: usize,
}

// State is minimal as we create a new channel each time to avoid interference.
#[derive(Clone)]
struct MpscBenchState;

// --- Extractor Function ---
fn extract_mpsc_config(combo: &AbstractCombination) -> Result<MpscBenchConfig, String> {
  Ok(MpscBenchConfig {
    num_producers: combo.get_u64(0)? as usize,
    total_items: combo.get_u64(1)? as usize,
  })
}

// --- Async Benchmark ---

fn setup_fn_mpsc_async(
  _rt: &Runtime,
  _cfg: &MpscBenchConfig,
) -> Pin<Box<dyn Future<Output = Result<(BenchContext, MpscBenchState), String>> + Send>> {
  Box::pin(async move { Ok((BenchContext::default(), MpscBenchState)) })
}

fn benchmark_logic_mpsc_async(
  mut ctx: BenchContext,
  state: MpscBenchState,
  cfg: &MpscBenchConfig,
) -> Pin<Box<dyn Future<Output = (BenchContext, MpscBenchState, Duration)> + Send>> {
  let cfg_clone = cfg.clone();
  Box::pin(async move {
    // Create a fresh channel for each iteration.
    let (tx, mut rx) = mpsc::unbounded_async();

    let start_time = Instant::now();

    let consumer_handle = tokio::spawn(async move {
      for _ in 0..cfg_clone.total_items {
        rx.recv().await.unwrap();
      }
    });

    let mut producer_handles = Vec::new();
    // Correctly distribute items, including remainder, across producers.
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

fn mpsc_async_benches(c: &mut Criterion) {
  let rt = Runtime::new().unwrap();
  let core_count = usize::from(available_parallelism().unwrap()) as u64;
  let parameter_axes = vec![
    vec![
      MatrixCellValue::Unsigned(1),          // SPSC-like scenario
      MatrixCellValue::Unsigned(4),          // MPSC scenario
      MatrixCellValue::Unsigned(core_count),
    ], // Num Senders
    vec![
      MatrixCellValue::Unsigned(100_000),
      MatrixCellValue::Unsigned(1_000_000),
    ], // Total Items
  ];
  let parameter_names = vec!["Prod".to_string(), "Items".to_string()];

  AsyncBenchmarkSuite::new(
    c,
    &rt,
    "MpscAsync".to_string(),
    Some(parameter_names),
    parameter_axes,
    Box::new(extract_mpsc_config),
    setup_fn_mpsc_async,
    benchmark_logic_mpsc_async,
    |_, _, _, _| Box::pin(async {}), // Teardown
  )
  .throughput(|cfg: &MpscBenchConfig| Throughput::Elements(cfg.total_items as u64))
  .run();
}

criterion_group!(benches, mpsc_async_benches);
criterion_main!(benches);
