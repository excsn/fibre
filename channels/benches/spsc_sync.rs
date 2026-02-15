use bench_matrix::{
  criterion_runner::sync_suite::SyncBenchmarkSuite, AbstractCombination, MatrixCellValue,
};
use criterion::{criterion_group, criterion_main, Criterion, Throughput};
use std::time::{Duration, Instant};

use fibre::spsc;

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

// Sync SPSC
trait SyncSenderSpsc: Send {
  fn send(&self, item: u64);
}
trait SyncReceiverSpsc: Send {
  fn recv(&self) -> u64;
}
struct SpscSyncState {
  producer: Box<dyn SyncSenderSpsc>,
  consumer: Box<dyn SyncReceiverSpsc>,
}
struct SpscSyncProdImpl(spsc::BoundedSyncSender<u64>);
impl SyncSenderSpsc for SpscSyncProdImpl {
  fn send(&self, item: u64) {
    self.0.send(item).unwrap();
  }
}
struct SpscSyncConsImpl(spsc::BoundedSyncReceiver<u64>);
impl SyncReceiverSpsc for SpscSyncConsImpl {
  fn recv(&self) -> u64 {
    self.0.recv().unwrap()
  }
}

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

  Ok(SpscBenchConfig {
    capacity,
    num_items,
  })
}

// Setup for Sync SPSC
fn setup_fn_spsc_sync(cfg: &SpscBenchConfig) -> Result<(BenchContext, SpscSyncState), String> {
  let (p, r) = spsc::bounded_sync(cfg.capacity);
  Ok((
    BenchContext::default(),
    SpscSyncState {
      producer: Box::new(SpscSyncProdImpl(p)),
      consumer: Box::new(SpscSyncConsImpl(r)),
    },
  ))
}

// Benchmark Logic for Sync SPSC
fn benchmark_logic_spsc_sync(
  mut ctx: BenchContext,
  state: SpscSyncState,
  cfg: &SpscBenchConfig,
) -> (BenchContext, SpscSyncState, Duration) {
  let start_time = Instant::now();
  for _ in 0..cfg.num_items {
    state.producer.send(ITEM_VALUE);
    let _ = state.consumer.recv();
  }
  let duration = start_time.elapsed();
  ctx.items_processed_total += cfg.num_items;
  (ctx, state, duration)
}

// Teardown (common for SPSC)
fn teardown_spsc_sync(_ctx: BenchContext, _state: SpscSyncState, _cfg: &SpscBenchConfig) {}

// Suites
fn spsc_sync_benches(c: &mut Criterion) {
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

  SyncBenchmarkSuite::new(
    c,
    "SpscSync".to_string(),
    Some(parameter_names),
    parameter_axes,
    Box::new(extract_spsc_config), // Extractor now handles filtering
    setup_fn_spsc_sync,
    benchmark_logic_spsc_sync,
    teardown_spsc_sync,
  )
  .throughput(|cfg: &SpscBenchConfig| Throughput::Elements(cfg.num_items as u64))
  .run();
}

criterion_group!(benches, spsc_sync_benches);
criterion_main!(benches);
