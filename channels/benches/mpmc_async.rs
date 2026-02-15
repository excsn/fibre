use bench_matrix::{criterion_runner::async_suite::AsyncBenchmarkSuite, AbstractCombination, MatrixCellValue};
use criterion::{criterion_group, criterion_main, Criterion, Throughput};
use std::{
  future::Future, pin::Pin, thread::available_parallelism, time::{Duration, Instant}
};
use tokio::{runtime::Runtime, task::JoinHandle};

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
fn setup_fn_mpmc_async(
  _runtime: &Runtime,
  _cfg: &MpmcBenchConfig,
) -> Pin<Box<dyn Future<Output = Result<(BenchContext, MpmcBenchState), String>> + Send>> {
  Box::pin(async move { Ok((BenchContext::default(), MpmcBenchState)) })
}

// --- Benchmark Logic ---
fn benchmark_logic_mpmc_async(
  mut ctx: BenchContext,
  state: MpmcBenchState,
  cfg: &MpmcBenchConfig,
) -> Pin<Box<dyn Future<Output = (BenchContext, MpmcBenchState, Duration)> + Send>> {
  let cfg_clone = cfg.clone();

  Box::pin(async move {
    let items_to_send_total = cfg_clone.total_items;
    if items_to_send_total == 0 {
      return (ctx, state, Duration::from_nanos(0));
    }

    // Create a fresh channel for this specific iteration.
    let (main_producer, main_consumer) = mpmc::bounded_async(cfg_clone.capacity);

    let mut producer_handles: Vec<JoinHandle<()>> = Vec::with_capacity(cfg_clone.num_producers);
    let start_time = Instant::now();

    for p_idx in 0..cfg_clone.num_producers {
      let producer_clone = main_producer.clone();
      let items_this_producer = {
        let base = items_to_send_total / cfg_clone.num_producers;
        let remainder = items_to_send_total % cfg_clone.num_producers;
        base + if p_idx < remainder { 1 } else { 0 }
      };

      if items_this_producer > 0 {
        let handle = tokio::spawn(async move {
          for _ in 0..items_this_producer {
            producer_clone.send(ITEM_VALUE).await.unwrap();
          }
        });
        producer_handles.push(handle);
      }
    }
    drop(main_producer);

    let mut consumer_handles: Vec<JoinHandle<()>> = Vec::with_capacity(cfg_clone.num_consumers);
    for _ in 0..cfg_clone.num_consumers {
      let consumer_clone = main_consumer.clone();
      let handle = tokio::spawn(async move {
        while let Ok(_) = consumer_clone.recv().await {
          // work is done inside recv
        }
      });
      consumer_handles.push(handle);
    }
    drop(main_consumer);

    // Wait for producers first
    for handle in producer_handles {
      handle.await.expect("MPMC v2 async producer task panicked");
    }

    // Then wait for consumers to finish draining
    for handle in consumer_handles {
      handle.await.expect("MPMC v2 async consumer task panicked");
    }

    let duration = start_time.elapsed();
    ctx.actual_items_processed_total += items_to_send_total;
    (ctx, state, duration)
  })
}

// --- Teardown Function ---
fn teardown_mpmc_async(
  _ctx: BenchContext,
  _state: MpmcBenchState,
  _runtime: &Runtime,
  _cfg: &MpmcBenchConfig,
) -> Pin<Box<dyn Future<Output = ()> + Send>> {
  Box::pin(async move {})
}

// --- Main Benchmark Suite ---
fn mpmc_async_benches(c: &mut Criterion) {
  let core_count = usize::from(available_parallelism().unwrap()) as u64;
  let rt = Runtime::new().expect("Failed to create Tokio runtime for MPMC v2 async benchmarks");
  let parameter_axes = vec![
    vec![
      // Axis 0: Capacity
      MatrixCellValue::Unsigned(0), // Rendezvous
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
      // Axis 2: Num Receivers
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

  AsyncBenchmarkSuite::new(
    c,
    &rt,
    "MpmcV2Async".to_string(), // New benchmark group name
    Some(parameter_names),
    parameter_axes,
    Box::new(extract_mpmc_config),
    setup_fn_mpmc_async,
    benchmark_logic_mpmc_async,
    teardown_mpmc_async,
  )
  .throughput(|cfg: &MpmcBenchConfig| Throughput::Elements(cfg.total_items as u64))
  .run();
}

criterion_group!(benches, mpmc_async_benches);
criterion_main!(benches);
