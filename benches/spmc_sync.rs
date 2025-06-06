// benches/spmc_sync.rs

use bench_matrix::{
  criterion_runner::sync_suite::SyncBenchmarkSuite, AbstractCombination, MatrixCellValue,
};
use criterion::{criterion_group, criterion_main, Criterion, Throughput};
use fibre::spmc;
use std::{
  sync::Barrier,
  thread::{self, available_parallelism},
  time::{Duration, Instant},
};

const ITEM_VALUE: u64 = 42;

#[derive(Debug, Clone)]
struct SpmcBenchConfig {
  num_consumers: usize,
  capacity: usize,
  total_items: usize,
}

#[derive(Default, Debug)]
struct BenchContext {
  actual_items_processed: usize,
}

#[derive(Clone)]
struct SpmcBenchState;

fn extract_spmc_config(combo: &AbstractCombination) -> Result<SpmcBenchConfig, String> {
  Ok(SpmcBenchConfig {
    num_consumers: combo.get_u64(0)? as usize,
    capacity: combo.get_u64(1)? as usize,
    total_items: combo.get_u64(2)? as usize,
  })
}

fn setup_fn_spmc_sync(_cfg: &SpmcBenchConfig) -> Result<(BenchContext, SpmcBenchState), String> {
  Ok((BenchContext::default(), SpmcBenchState))
}

fn benchmark_logic_spmc_sync(
  mut ctx: BenchContext,
  state: SpmcBenchState,
  cfg: &SpmcBenchConfig,
) -> (BenchContext, SpmcBenchState, Duration) {
  let (mut tx, rx) = spmc::channel(cfg.capacity);
  let mut receivers: Vec<_> = (0..cfg.num_consumers).map(|_| rx.clone()).collect();
  drop(rx);

  let barrier = Barrier::new(cfg.num_consumers + 1);

  let start_time = Instant::now();

  thread::scope(|s| {
    for mut receiver in receivers {
      s.spawn(|| {
        barrier.wait();
        for _ in 0..cfg.total_items {
          receiver.recv().unwrap();
        }
      });
    }

    barrier.wait();
    for _ in 0..cfg.total_items {
      tx.send(ITEM_VALUE).unwrap();
    }
  });

  let duration = start_time.elapsed();
  ctx.actual_items_processed += cfg.total_items * cfg.num_consumers;
  (ctx, state, duration)
}

fn spmc_sync_benches(c: &mut Criterion) {
  let core_count = usize::from(available_parallelism().unwrap()) as u64;
  SyncBenchmarkSuite::new(
    c,
    "SpmcSync".to_string(),
    Some(vec!["Cons".into(), "Cap".into(), "Items".into()]),
    vec![
      vec![
        MatrixCellValue::Unsigned(1),
        MatrixCellValue::Unsigned(4),
        MatrixCellValue::Unsigned(core_count),
      ], // Consumers
      vec![MatrixCellValue::Unsigned(1), MatrixCellValue::Unsigned(128)], // Capacity
      vec![MatrixCellValue::Unsigned(100_000)],                           // Total Items
    ],
    Box::new(extract_spmc_config),
    setup_fn_spmc_sync,
    benchmark_logic_spmc_sync,
    |_, _, _| {},
  )
  .throughput(|cfg| Throughput::Elements((cfg.total_items * cfg.num_consumers) as u64))
  .run();
}

criterion_group!(benches, spmc_sync_benches);
criterion_main!(benches);
