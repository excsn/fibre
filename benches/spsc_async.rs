// benches/spsc_benches.rs

use bench_matrix::{
  criterion_runner::{
    async_suite::{AsyncBenchmarkSuite, AsyncTeardownFn},
    ExtractorFn,
  },
  AbstractCombination, MatrixCellValue,
};
use criterion::{criterion_group, criterion_main, Criterion, Throughput};
use std::{
  future::Future,
  pin::Pin,
  time::{Duration, Instant},
};
use tokio::runtime::Runtime;

use fibre::spsc; // Use your actual library import

const ITEM_VALUE: u64 = 42;

// --- Config, State, Context for SPSC ---
#[derive(Debug, Clone)]
struct SpscBenchConfig {
  capacity: usize,
  num_items: usize,
}

#[derive(Default, Debug)]
struct BenchContext {
  items_processed_total: usize,
}

// Async SPSC - State can be minimal if P/C are created per logic call
struct SpscAsyncStateMinimal {
  _marker: (),
}

// Extractor for SPSC - NOW INCLUDES FILTERING
fn extract_spsc_config(combo: &AbstractCombination) -> Result<SpscBenchConfig, String> {
  let capacity = (combo.get_u64(0)? as usize).max(1);
  let num_items = (combo.get_u64(1)? as usize).max(1);

  // Perform filtering here:
  // fibre::spsc's bounded_sync/async panics if capacity is 0.
  if capacity == 0 {
    return Err(format!(
      "Skipping SPSC combination: capacity cannot be 0. Cap: {}, Items: {}",
      capacity, num_items
    ));
  }

  Ok(SpscBenchConfig { capacity, num_items })
}

// Setup for Async SPSC (minimal state)
fn setup_fn_spsc_async_minimal(
  _runtime: &Runtime,
  _cfg: &SpscBenchConfig, // cfg is passed but capacity check is now in extractor
) -> Pin<Box<dyn Future<Output = Result<(BenchContext, SpscAsyncStateMinimal), String>> + Send>> {
  Box::pin(async move {
    // Capacity check done in extractor.
    Ok((BenchContext::default(), SpscAsyncStateMinimal { _marker: () }))
  })
}

// Benchmark Logic for Async SPSC (creating P/C inside)
fn benchmark_logic_spsc_async_minimal(
  mut ctx: BenchContext,
  state: SpscAsyncStateMinimal, // state is minimal
  cfg: &SpscBenchConfig,
) -> Pin<Box<dyn Future<Output = (BenchContext, SpscAsyncStateMinimal, Duration)> + Send>> {
  let cfg_clone = cfg.clone();
  Box::pin(async move {
    let start_time = Instant::now();
    // Create producer and consumer for the scope of this benchmark logic call
    let (p, c) = spsc::bounded_async(cfg_clone.capacity);
    for _ in 0..cfg_clone.num_items {
      p.send(ITEM_VALUE).await.unwrap();
      let _ = c.recv().await.unwrap();
    }
    let duration = start_time.elapsed();
    ctx.items_processed_total += cfg_clone.num_items;
    (ctx, state, duration) // Return the minimal state
  })
}

// Teardown (common for SPSC)
fn teardown_spsc_async_minimal(
  _ctx: BenchContext,
  _state: SpscAsyncStateMinimal,
  _runtime: &Runtime,
  _cfg: &SpscBenchConfig,
) -> Pin<Box<dyn Future<Output = ()> + Send>> {
  Box::pin(async move {})
}

// Suites
fn spsc_async_benches(c: &mut Criterion) {
  let rt = Runtime::new().expect("Failed to create Tokio runtime for SPSC async benchmarks");
  let parameter_axes = vec![
    vec![
      MatrixCellValue::Unsigned(1),
      MatrixCellValue::Unsigned(128),
      MatrixCellValue::Unsigned(1024),
    ], // Capacity
    vec![
      MatrixCellValue::Unsigned(1_000),
      MatrixCellValue::Unsigned(100_000),
      MatrixCellValue::Unsigned(1_000_000),
    ], // NumItems
  ];
  let parameter_names = vec!["Cap".to_string(), "Items".to_string()];

  AsyncBenchmarkSuite::new(
    c,
    &rt,
    "SpscAsync".to_string(),
    Some(parameter_names),
    parameter_axes,
    Box::new(extract_spsc_config), // Extractor now handles filtering
    setup_fn_spsc_async_minimal,
    benchmark_logic_spsc_async_minimal,
    teardown_spsc_async_minimal,
  )
  .throughput(|cfg: &SpscBenchConfig| Throughput::Elements(cfg.num_items as u64))
  .run();
}

criterion_group!(benches, spsc_async_benches);
criterion_main!(benches);
