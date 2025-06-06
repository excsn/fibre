// benches/mpsc_sync.rs

use bench_matrix::{
  criterion_runner::sync_suite::SyncBenchmarkSuite, AbstractCombination, MatrixCellValue,
};
use criterion::{criterion_group, criterion_main, Criterion, Throughput};
use fibre::mpsc;
use std::{
  thread::{self, available_parallelism},
  time::{Duration, Instant},
};

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

// State for Sync benchmark. The channel must be re-created each time
// because `thread::scope` consumes the handles. So state is minimal.
#[derive(Clone)]
struct MpscBenchState;

// --- Extractor Function ---
fn extract_mpsc_config(combo: &AbstractCombination) -> Result<MpscBenchConfig, String> {
  Ok(MpscBenchConfig {
    num_producers: combo.get_u64(0)? as usize,
    total_items: combo.get_u64(1)? as usize,
  })
}

// --- Sync Benchmark ---

fn setup_fn_mpsc_sync(_cfg: &MpscBenchConfig) -> Result<(BenchContext, MpscBenchState), String> {
  Ok((BenchContext::default(), MpscBenchState))
}

fn benchmark_logic_mpsc_sync(
  mut ctx: BenchContext,
  state: MpscBenchState,
  cfg: &MpscBenchConfig,
) -> (BenchContext, MpscBenchState, Duration) {
  // Create a fresh channel for each iteration of the benchmark.
  let (tx, mut rx) = mpsc::channel();

  let items_per_producer = cfg.total_items / cfg.num_producers;
  let start_time = Instant::now();

  thread::scope(|s| {
    // Spawn producers
    for _ in 0..cfg.num_producers {
      let tx_clone = tx.clone();
      s.spawn(move || {
        for _ in 0..items_per_producer {
          tx_clone.send(ITEM_VALUE).unwrap();
        }
      });
    }
    drop(tx); // Drop the original sender handle

    // Consumer runs in the current scoped thread
    for _ in 0..cfg.total_items {
      rx.recv().unwrap();
    }
  });

  let duration = start_time.elapsed();
  ctx.actual_items_processed += cfg.total_items;
  (ctx, state, duration)
}

fn mpsc_sync_benches(c: &mut Criterion) {
  let core_count = usize::from(available_parallelism().unwrap()) as u64;
  let parameter_axes = vec![
    vec![
      MatrixCellValue::Unsigned(1),          // SPSC-like scenario
      MatrixCellValue::Unsigned(4),          // MPSC scenario
      MatrixCellValue::Unsigned(core_count),
    ], // Num Producers
    vec![
      MatrixCellValue::Unsigned(100_000),
      MatrixCellValue::Unsigned(1_000_000),
    ], // Total Items
  ];
  let parameter_names = vec!["Prod".to_string(), "Items".to_string()];

  SyncBenchmarkSuite::new(
    c,
    "MpscSync".to_string(),
    Some(parameter_names),
    parameter_axes,
    Box::new(extract_mpsc_config),
    setup_fn_mpsc_sync,
    benchmark_logic_mpsc_sync,
    |_, _, _| {}, // Teardown
  )
  .throughput(|cfg: &MpscBenchConfig| Throughput::Elements(cfg.total_items as u64))
  .run();
}

criterion_group!(benches, mpsc_sync_benches);
criterion_main!(benches);
