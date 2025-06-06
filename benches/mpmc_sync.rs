// benches/mpmc_sync.rs

use bench_matrix::{criterion_runner::sync_suite::SyncBenchmarkSuite, AbstractCombination, MatrixCellValue};
use criterion::{criterion_group, criterion_main, Criterion, Throughput};
use std::{
  sync::{
    atomic::{AtomicUsize, Ordering as AtomicOrdering},
    Arc,
  },
  thread,
  time::{Duration, Instant},
};

// Use the new v2 module
use fibre::mpmc as mpmc;

const ITEM_VALUE: u64 = 42;

// --- Config, State, Context ---
#[derive(Debug, Clone)]
struct MpmcBenchConfig {
  capacity: usize,
  num_producers: usize,
  num_consumers: usize,
  total_items: usize,
}

#[derive(Default, Debug)]
struct BenchContext {
  actual_items_processed_total: usize,
}

#[derive(Clone)]
struct MpmcBenchState;

// --- Extractor Function ---
fn extract_mpmc_config(combo: &AbstractCombination) -> Result<MpmcBenchConfig, String> {
  let capacity = combo.get_u64(0)? as usize;
  let num_producers = combo.get_u64(1)? as usize;
  let num_consumers = combo.get_u64(2)? as usize;
  let total_items = combo.get_u64(3)? as usize;

  // v2 does not require power-of-two capacity
  if num_producers == 0 || num_consumers == 0 {
    return Err("Number of producers and consumers must be at least 1.".to_string());
  }
  if total_items > 0 && num_producers > 0 && total_items < num_producers {
    return Err(format!(
      "Total items ({}) must be >= num_producers ({})",
      total_items, num_producers
    ));
  }

  Ok(MpmcBenchConfig {
    capacity,
    num_producers,
    num_consumers,
    total_items,
  })
}

// --- Setup Function ---
fn setup_fn_mpmc_sync(_cfg: &MpmcBenchConfig) -> Result<(BenchContext, MpmcBenchState), String> {
  Ok((BenchContext::default(), MpmcBenchState))
}

// --- Benchmark Logic ---
fn benchmark_logic_mpmc_sync(
  mut ctx: BenchContext,
  state: MpmcBenchState,
  cfg: &MpmcBenchConfig,
) -> (BenchContext, MpmcBenchState, Duration) {
  let items_to_send_total = cfg.total_items;
  if items_to_send_total == 0 {
    return (ctx, state, Duration::from_nanos(0));
  }

  // Create a fresh channel for this iteration.
  let (main_producer, main_consumer) = mpmc::channel(cfg.capacity);

  let mut producer_handles = Vec::with_capacity(cfg.num_producers);
  let start_time = Instant::now();

  for p_idx in 0..cfg.num_producers {
    let producer_clone = main_producer.clone();
    let items_this_producer = {
      let base = items_to_send_total / cfg.num_producers;
      let remainder = items_to_send_total % cfg.num_producers;
      base + if p_idx < remainder { 1 } else { 0 }
    };

    if items_this_producer > 0 {
      let handle = thread::spawn(move || {
        for _ in 0..items_this_producer {
          producer_clone.send(ITEM_VALUE).unwrap();
        }
      });
      producer_handles.push(handle);
    }
  }
  drop(main_producer);

  let mut consumer_handles = Vec::with_capacity(cfg.num_consumers);
  for _ in 0..cfg.num_consumers {
    let consumer_clone = main_consumer.clone();
    let handle = thread::spawn(move || {
      // Each consumer runs until the channel is disconnected.
      while let Ok(_) = consumer_clone.recv() {
        // work is done inside recv
      }
    });
    consumer_handles.push(handle);
  }
  drop(main_consumer);

  // Wait for producers first
  for handle in producer_handles {
    handle.join().expect("MPMC v2 sync producer thread panicked");
  }

  // Then wait for consumers to finish draining
  for handle in consumer_handles {
    handle.join().expect("MPMC v2 sync consumer thread panicked");
  }

  let duration = start_time.elapsed();
  ctx.actual_items_processed_total += items_to_send_total;
  (ctx, state, duration)
}

// --- Teardown Function ---
fn teardown_mpmc_sync(_ctx: BenchContext, _state: MpmcBenchState, _cfg: &MpmcBenchConfig) {}

// --- Main Benchmark Suite ---
fn mpmc_sync_benches(c: &mut Criterion) {
  let core_count = u64::from(available_parallelism().unwrap());
  let parameter_axes = vec![
    vec![
      // Axis 0: Capacity
      MatrixCellValue::Unsigned(0), // Rendezvous
      MatrixCellValue::Unsigned(4),
      MatrixCellValue::Unsigned(128),
    ],
    vec![
      // Axis 1: Num Producers
      MatrixCellValue::Unsigned(1),
      MatrixCellValue::Unsigned(4),
      MatrixCellValue::Unsigned(core_count),
    ],
    vec![
      // Axis 2: Num Consumers
      MatrixCellValue::Unsigned(1),
      MatrixCellValue::Unsigned(4),
      MatrixCellValue::Unsigned(core_count),
    ],
    vec![
      // Axis 3: Total Items
      MatrixCellValue::Unsigned(100_000),
      MatrixCellValue::Unsigned(1_000_000),
    ],
  ];
  let parameter_names = vec!["Cap", "Prod", "Cons", "Items"]
    .into_iter()
    .map(String::from)
    .collect();

  SyncBenchmarkSuite::new(
    c,
    "MpmcV2Sync".to_string(), // New benchmark group name
    Some(parameter_names),
    parameter_axes,
    Box::new(extract_mpmc_config),
    setup_fn_mpmc_sync,
    benchmark_logic_mpmc_sync,
    teardown_mpmc_sync,
  )
  .throughput(|cfg: &MpmcBenchConfig| Throughput::Elements(cfg.total_items as u64))
  .run();
}

criterion_group!(benches, mpmc_sync_benches);
criterion_main!(benches);
