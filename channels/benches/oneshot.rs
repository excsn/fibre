use bench_matrix::{
  criterion_runner::
    async_suite::AsyncBenchmarkSuite
  ,
  AbstractCombination, MatrixCellValue,
};
use criterion::{criterion_group, criterion_main, Criterion, Throughput};
use std::{
  future::Future,
  pin::Pin,
  time::{Duration, Instant},
};
use tokio::runtime::Runtime;

use fibre::oneshot; // Use your actual library import

const ITEM_VALUE: u64 = 42;

// --- Config, State, Context for Oneshot ---
#[derive(Debug, Clone)]
struct OneshotBenchConfig {
  num_items: usize, // Number of separate oneshot operations
}

#[derive(Default, Debug)]
struct BenchContext {
  items_processed_total: usize,
}

// State for Oneshot (Async only)
// Since send() consumes the sender, we don't store a persistent producer/consumer in state.
// The benchmark logic will create them per iteration. So, State can be minimal or even ().
struct OneshotAsyncState {
  // Potentially hold a Tokio runtime handle if needed across iterations, but bench_matrix provides it.
  // Or a pre-allocated buffer if items were complex and allocation was to be excluded.
  // For u64, this is not necessary.
  _marker: (), // To make it a struct
}

// Extractor for Oneshot
fn extract_oneshot_config(combo: &AbstractCombination) -> Result<OneshotBenchConfig, String> {
  Ok(OneshotBenchConfig {
    num_items: combo.get_u64(0)? as usize,
  })
}

// Setup for Async Oneshot
fn setup_fn_oneshot_async(
  _runtime: &Runtime,
  _cfg: &OneshotBenchConfig, // cfg might be used if state needed initialization based on it
) -> Pin<Box<dyn Future<Output = Result<(BenchContext, OneshotAsyncState), String>> + Send>> {
  Box::pin(async move { Ok((BenchContext::default(), OneshotAsyncState { _marker: () })) })
}

// Benchmark Logic for Async Oneshot
fn benchmark_logic_oneshot_async(
  mut ctx: BenchContext,
  state: OneshotAsyncState, // State is passed but might not be used much if ops are self-contained
  cfg: &OneshotBenchConfig,
) -> Pin<Box<dyn Future<Output = (BenchContext, OneshotAsyncState, Duration)> + Send>> {
  let cfg_clone = cfg.clone();
  Box::pin(async move {
    let start_time = Instant::now();
    for _ in 0..cfg_clone.num_items {
      let (p_oneshot, r_oneshot) = oneshot::oneshot();
      p_oneshot.send(ITEM_VALUE).expect("Oneshot send failed"); // send consumes p_oneshot
      let _ = r_oneshot.recv().await.unwrap();
    }
    let duration = start_time.elapsed();
    ctx.items_processed_total += cfg_clone.num_items;
    (ctx, state, duration)
  })
}

// Teardown for Async Oneshot
fn teardown_oneshot_async(
  _ctx: BenchContext,
  _state: OneshotAsyncState,
  _runtime: &Runtime,
  _cfg: &OneshotBenchConfig,
) -> Pin<Box<dyn Future<Output = ()> + Send>> {
  Box::pin(async move {})
}

// Suite
fn oneshot_async_benches(c: &mut Criterion) {
  let rt = Runtime::new().unwrap();
  let parameter_axes = vec![
    vec![
      MatrixCellValue::Unsigned(100),
      MatrixCellValue::Unsigned(1000),
    ], // NumItems
  ];
  // Capacity is not really a parameter for oneshot channels in the same way.
  // If we wanted to vary something like "task spawn overhead", that'd be different.
  let parameter_names = vec!["Ops".to_string()];

  AsyncBenchmarkSuite::new(
    c,
    &rt,
    "OneshotAsync".to_string(),
    Some(parameter_names),
    parameter_axes,
    Box::new(extract_oneshot_config),
    setup_fn_oneshot_async,
    benchmark_logic_oneshot_async,
    teardown_oneshot_async,
  )
  .throughput(|cfg: &OneshotBenchConfig| Throughput::Elements(cfg.num_items as u64))
  .run();
}

criterion_group!(benches, oneshot_async_benches);
criterion_main!(benches);
