use bench_matrix::{
  criterion_runner::{async_suite::AsyncBenchmarkSuite, sync_suite::SyncBenchmarkSuite},
  AbstractCombination, MatrixCellValue,
};
use criterion::{criterion_group, criterion_main, Criterion, Throughput};
use std::{
  future::Future,
  pin::Pin,
  thread::{self, available_parallelism},
  time::{Duration, Instant},
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

fn parameter_axes(core_count: u64) -> Vec<Vec<MatrixCellValue>> {
  vec![
    vec![
      // Axis 0: Capacity
      // MatrixCellValue::Unsigned(0), // Rendezvous
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
  ]
}

fn parameter_names() -> Vec<String> {
  vec!["Cap", "Prod", "Cons", "Items"]
    .into_iter()
    .map(String::from)
    .collect()
}

// =============================================================================
// Sync
// =============================================================================

fn setup_fn_mpmc_sync(_cfg: &MpmcBenchConfig) -> Result<(BenchContext, MpmcBenchState), String> {
  Ok((BenchContext::default(), MpmcBenchState))
}

fn benchmark_logic_mpmc_sync(
  mut ctx: BenchContext,
  state: MpmcBenchState,
  cfg: &MpmcBenchConfig,
) -> (BenchContext, MpmcBenchState, Duration) {
  let items_to_send_total = cfg.total_items;
  if items_to_send_total == 0 {
    return (ctx, state, Duration::from_nanos(0));
  }

  let (main_producer, main_consumer) = mpmc::bounded(cfg.capacity);

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
      while let Ok(_) = consumer_clone.recv() {}
    });
    consumer_handles.push(handle);
  }
  drop(main_consumer);

  for handle in producer_handles {
    handle.join().expect("MPMC sync producer thread panicked");
  }
  for handle in consumer_handles {
    handle.join().expect("MPMC sync consumer thread panicked");
  }

  let duration = start_time.elapsed();
  ctx.actual_items_processed_total += items_to_send_total;
  (ctx, state, duration)
}

fn teardown_mpmc_sync(_ctx: BenchContext, _state: MpmcBenchState, _cfg: &MpmcBenchConfig) {}

fn mpmc_sync_benches(c: &mut Criterion) {
  let core_count = usize::from(available_parallelism().unwrap()) as u64;

  SyncBenchmarkSuite::new(
    c,
    "MpmcSync".to_string(),
    Some(parameter_names()),
    parameter_axes(core_count),
    Box::new(extract_mpmc_config),
    setup_fn_mpmc_sync,
    benchmark_logic_mpmc_sync,
    teardown_mpmc_sync,
  )
  .throughput(|cfg: &MpmcBenchConfig| Throughput::Elements(cfg.total_items as u64))
  .run();
}

// =============================================================================
// Async
// =============================================================================

fn setup_fn_mpmc_async(
  _runtime: &Runtime,
  _cfg: &MpmcBenchConfig,
) -> Pin<Box<dyn Future<Output = Result<(BenchContext, MpmcBenchState), String>> + Send>> {
  Box::pin(async move { Ok((BenchContext::default(), MpmcBenchState)) })
}

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
        while let Ok(_) = consumer_clone.recv().await {}
      });
      consumer_handles.push(handle);
    }
    drop(main_consumer);

    for handle in producer_handles {
      handle.await.expect("MPMC async producer task panicked");
    }
    for handle in consumer_handles {
      handle.await.expect("MPMC async consumer task panicked");
    }

    let duration = start_time.elapsed();
    ctx.actual_items_processed_total += items_to_send_total;
    (ctx, state, duration)
  })
}

fn teardown_mpmc_async(
  _ctx: BenchContext,
  _state: MpmcBenchState,
  _runtime: &Runtime,
  _cfg: &MpmcBenchConfig,
) -> Pin<Box<dyn Future<Output = ()> + Send>> {
  Box::pin(async move {})
}

fn mpmc_async_benches(c: &mut Criterion) {
  let core_count = usize::from(available_parallelism().unwrap()) as u64;
  let rt = Runtime::new().expect("Failed to create Tokio runtime for MPMC async benchmarks");

  AsyncBenchmarkSuite::new(
    c,
    &rt,
    "MpmcAsync".to_string(),
    Some(parameter_names()),
    parameter_axes(core_count),
    Box::new(extract_mpmc_config),
    setup_fn_mpmc_async,
    benchmark_logic_mpmc_async,
    teardown_mpmc_async,
  )
  .throughput(|cfg: &MpmcBenchConfig| Throughput::Elements(cfg.total_items as u64))
  .run();
}

// =============================================================================
// Batch config + helpers
// =============================================================================

#[derive(Debug, Clone)]
struct MpmcBatchBenchConfig {
  capacity: usize,
  num_producers: usize,
  num_consumers: usize,
  total_items: usize,
  batch_size: usize,
}

fn extract_mpmc_batch_config(combo: &AbstractCombination) -> Result<MpmcBatchBenchConfig, String> {
  let capacity = combo.get_u64(0)? as usize;
  let num_producers = combo.get_u64(1)? as usize;
  let num_consumers = combo.get_u64(2)? as usize;
  let total_items = combo.get_u64(3)? as usize;
  let batch_size = combo.get_u64(4)? as usize;

  if num_producers == 0 || num_consumers == 0 {
    return Err("Number of producers and consumers must be at least 1.".to_string());
  }
  if batch_size == 0 {
    return Err("Batch size must be at least 1.".to_string());
  }
  if total_items > 0 && num_producers > 0 && total_items < num_producers {
    return Err(format!(
      "Total items ({}) must be >= num_producers ({})",
      total_items, num_producers
    ));
  }

  Ok(MpmcBatchBenchConfig {
    capacity,
    num_producers,
    num_consumers,
    total_items,
    batch_size,
  })
}

fn batch_parameter_axes(core_count: u64) -> Vec<Vec<MatrixCellValue>> {
  let mut axes = parameter_axes(core_count);
  axes.push(vec![
    // Axis 4: Batch Size
    MatrixCellValue::Unsigned(8),
    MatrixCellValue::Unsigned(64),
    MatrixCellValue::Unsigned(512),
  ]);
  axes
}

fn batch_parameter_names() -> Vec<String> {
  vec!["Cap", "Prod", "Cons", "Items", "Batch"]
    .into_iter()
    .map(String::from)
    .collect()
}

// =============================================================================
// Sync Batch
// =============================================================================

fn setup_fn_mpmc_sync_batch(
  _cfg: &MpmcBatchBenchConfig,
) -> Result<(BenchContext, MpmcBenchState), String> {
  Ok((BenchContext::default(), MpmcBenchState))
}

fn benchmark_logic_mpmc_sync_batch(
  mut ctx: BenchContext,
  state: MpmcBenchState,
  cfg: &MpmcBatchBenchConfig,
) -> (BenchContext, MpmcBenchState, Duration) {
  let total = cfg.total_items;
  if total == 0 {
    return (ctx, state, Duration::from_nanos(0));
  }

  let (main_producer, main_consumer) = mpmc::bounded(cfg.capacity);

  let mut producer_handles = Vec::with_capacity(cfg.num_producers);
  let start_time = Instant::now();

  for p_idx in 0..cfg.num_producers {
    let producer_clone = main_producer.clone();
    let items_this_producer = {
      let base = total / cfg.num_producers;
      let remainder = total % cfg.num_producers;
      base + if p_idx < remainder { 1 } else { 0 }
    };
    let batch_size = cfg.batch_size;

    if items_this_producer > 0 {
      let handle = thread::spawn(move || {
        let mut remaining = items_this_producer;
        while remaining > 0 {
          let chunk_size = remaining.min(batch_size);
          let chunk = vec![ITEM_VALUE; chunk_size];
          producer_clone.send_batch(chunk).unwrap();
          remaining -= chunk_size;
        }
      });
      producer_handles.push(handle);
    }
  }
  drop(main_producer);

  let mut consumer_handles = Vec::with_capacity(cfg.num_consumers);
  for _ in 0..cfg.num_consumers {
    let consumer_clone = main_consumer.clone();
    let batch_size = cfg.batch_size;
    let handle = thread::spawn(move || {
      loop {
        match consumer_clone.recv_batch(batch_size) {
          Ok(_) => {}
          Err(_) => break,
        }
      }
    });
    consumer_handles.push(handle);
  }
  drop(main_consumer);

  for handle in producer_handles {
    handle.join().expect("MPMC sync batch producer thread panicked");
  }
  for handle in consumer_handles {
    handle.join().expect("MPMC sync batch consumer thread panicked");
  }

  let duration = start_time.elapsed();
  ctx.actual_items_processed_total += total;
  (ctx, state, duration)
}

fn teardown_mpmc_sync_batch(
  _ctx: BenchContext,
  _state: MpmcBenchState,
  _cfg: &MpmcBatchBenchConfig,
) {
}

fn mpmc_sync_batch_benches(c: &mut Criterion) {
  let core_count = usize::from(available_parallelism().unwrap()) as u64;

  SyncBenchmarkSuite::new(
    c,
    "MpmcSyncBatch".to_string(),
    Some(batch_parameter_names()),
    batch_parameter_axes(core_count),
    Box::new(extract_mpmc_batch_config),
    setup_fn_mpmc_sync_batch,
    benchmark_logic_mpmc_sync_batch,
    teardown_mpmc_sync_batch,
  )
  .throughput(|cfg: &MpmcBatchBenchConfig| Throughput::Elements(cfg.total_items as u64))
  .run();
}

// =============================================================================
// Async Batch
// =============================================================================

fn setup_fn_mpmc_async_batch(
  _runtime: &Runtime,
  _cfg: &MpmcBatchBenchConfig,
) -> Pin<Box<dyn Future<Output = Result<(BenchContext, MpmcBenchState), String>> + Send>> {
  Box::pin(async move { Ok((BenchContext::default(), MpmcBenchState)) })
}

fn benchmark_logic_mpmc_async_batch(
  mut ctx: BenchContext,
  state: MpmcBenchState,
  cfg: &MpmcBatchBenchConfig,
) -> Pin<Box<dyn Future<Output = (BenchContext, MpmcBenchState, Duration)> + Send>> {
  let cfg_clone = cfg.clone();

  Box::pin(async move {
    let total = cfg_clone.total_items;
    if total == 0 {
      return (ctx, state, Duration::from_nanos(0));
    }

    let (main_producer, main_consumer) = mpmc::bounded_async(cfg_clone.capacity);

    let mut producer_handles: Vec<JoinHandle<()>> = Vec::with_capacity(cfg_clone.num_producers);
    let start_time = Instant::now();

    for p_idx in 0..cfg_clone.num_producers {
      let producer_clone = main_producer.clone();
      let items_this_producer = {
        let base = total / cfg_clone.num_producers;
        let remainder = total % cfg_clone.num_producers;
        base + if p_idx < remainder { 1 } else { 0 }
      };
      let batch_size = cfg_clone.batch_size;

      if items_this_producer > 0 {
        let handle = tokio::spawn(async move {
          let mut remaining = items_this_producer;
          while remaining > 0 {
            let chunk_size = remaining.min(batch_size);
            let chunk = vec![ITEM_VALUE; chunk_size];
            producer_clone.send_batch(chunk).await.unwrap();
            remaining -= chunk_size;
          }
        });
        producer_handles.push(handle);
      }
    }
    drop(main_producer);

    let mut consumer_handles: Vec<JoinHandle<()>> = Vec::with_capacity(cfg_clone.num_consumers);
    for _ in 0..cfg_clone.num_consumers {
      let consumer_clone = main_consumer.clone();
      let batch_size = cfg_clone.batch_size;
      let handle = tokio::spawn(async move {
        loop {
          match consumer_clone.recv_batch(batch_size).await {
            Ok(_) => {}
            Err(_) => break,
          }
        }
      });
      consumer_handles.push(handle);
    }
    drop(main_consumer);

    for handle in producer_handles {
      handle.await.expect("MPMC async batch producer task panicked");
    }
    for handle in consumer_handles {
      handle.await.expect("MPMC async batch consumer task panicked");
    }

    let duration = start_time.elapsed();
    ctx.actual_items_processed_total += total;
    (ctx, state, duration)
  })
}

fn teardown_mpmc_async_batch(
  _ctx: BenchContext,
  _state: MpmcBenchState,
  _runtime: &Runtime,
  _cfg: &MpmcBatchBenchConfig,
) -> Pin<Box<dyn Future<Output = ()> + Send>> {
  Box::pin(async move {})
}

fn mpmc_async_batch_benches(c: &mut Criterion) {
  let core_count = usize::from(available_parallelism().unwrap()) as u64;
  let rt = Runtime::new().expect("Failed to create Tokio runtime for MPMC async batch benchmarks");

  AsyncBenchmarkSuite::new(
    c,
    &rt,
    "MpmcAsyncBatch".to_string(),
    Some(batch_parameter_names()),
    batch_parameter_axes(core_count),
    Box::new(extract_mpmc_batch_config),
    setup_fn_mpmc_async_batch,
    benchmark_logic_mpmc_async_batch,
    teardown_mpmc_async_batch,
  )
  .throughput(|cfg: &MpmcBatchBenchConfig| Throughput::Elements(cfg.total_items as u64))
  .run();
}

criterion_group!(
  benches,
  mpmc_sync_benches,
  mpmc_async_benches,
  mpmc_sync_batch_benches,
  mpmc_async_batch_benches,
);
criterion_main!(benches);
