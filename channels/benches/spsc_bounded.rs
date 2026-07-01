use bench_matrix::{
  criterion_runner::{async_suite::AsyncBenchmarkSuite, sync_suite::SyncBenchmarkSuite},
  AbstractCombination, MatrixCellValue,
};
use criterion::{criterion_group, criterion_main, Criterion, Throughput};
use std::{
  future::Future,
  pin::Pin,
  thread,
  time::{Duration, Instant},
};
use tokio::runtime::Runtime;

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

#[derive(Clone)]
struct SpscBenchState;

fn extract_spsc_config(combo: &AbstractCombination) -> Result<SpscBenchConfig, String> {
  let capacity = (combo.get_u64(0)? as usize).max(1);
  let num_items = (combo.get_u64(1)? as usize).max(1);

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

fn parameter_axes() -> Vec<Vec<MatrixCellValue>> {
  vec![
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
  ]
}

fn parameter_names() -> Vec<String> {
  vec!["Cap".to_string(), "Items".to_string()]
}

// =============================================================================
// Sync
// =============================================================================

fn setup_fn_spsc_sync(_cfg: &SpscBenchConfig) -> Result<(BenchContext, SpscBenchState), String> {
  Ok((BenchContext::default(), SpscBenchState))
}

fn benchmark_logic_spsc_sync(
  mut ctx: BenchContext,
  state: SpscBenchState,
  cfg: &SpscBenchConfig,
) -> (BenchContext, SpscBenchState, Duration) {
  let total = cfg.num_items;
  let (producer, consumer) = spsc::bounded_sync(cfg.capacity);

  let start_time = Instant::now();

  let producer_handle = thread::spawn(move || {
    for _ in 0..total {
      producer.send(ITEM_VALUE).unwrap();
    }
  });
  let consumer_handle = thread::spawn(move || {
    for _ in 0..total {
      let _ = consumer.recv().unwrap();
    }
  });

  producer_handle
    .join()
    .expect("SPSC sync producer thread panicked");
  consumer_handle
    .join()
    .expect("SPSC sync consumer thread panicked");

  let duration = start_time.elapsed();
  ctx.items_processed_total += total;
  (ctx, state, duration)
}

fn teardown_spsc_sync(_ctx: BenchContext, _state: SpscBenchState, _cfg: &SpscBenchConfig) {}

fn spsc_sync_benches(c: &mut Criterion) {
  SyncBenchmarkSuite::new(
    c,
    "SpscSync".to_string(),
    Some(parameter_names()),
    parameter_axes(),
    Box::new(extract_spsc_config),
    setup_fn_spsc_sync,
    benchmark_logic_spsc_sync,
    teardown_spsc_sync,
  )
  .throughput(|cfg: &SpscBenchConfig| Throughput::Elements(cfg.num_items as u64))
  .run();
}

// =============================================================================
// Async
// =============================================================================

fn setup_fn_spsc_async_minimal(
  _runtime: &Runtime,
  _cfg: &SpscBenchConfig,
) -> Pin<Box<dyn Future<Output = Result<(BenchContext, SpscBenchState), String>> + Send>> {
  Box::pin(async move { Ok((BenchContext::default(), SpscBenchState)) })
}

fn benchmark_logic_spsc_async_minimal(
  mut ctx: BenchContext,
  state: SpscBenchState,
  cfg: &SpscBenchConfig,
) -> Pin<Box<dyn Future<Output = (BenchContext, SpscBenchState, Duration)> + Send>> {
  let cfg_clone = cfg.clone();
  Box::pin(async move {
    let total = cfg_clone.num_items;
    let (mut p, mut c) = spsc::bounded_async(cfg_clone.capacity);

    let start_time = Instant::now();

    let producer_handle = tokio::spawn(async move {
      for _ in 0..total {
        p.send(ITEM_VALUE).await.unwrap();
      }
    });
    let consumer_handle = tokio::spawn(async move {
      for _ in 0..total {
        let _ = c.recv().await.unwrap();
      }
    });

    producer_handle
      .await
      .expect("SPSC async producer task panicked");
    consumer_handle
      .await
      .expect("SPSC async consumer task panicked");

    let duration = start_time.elapsed();
    ctx.items_processed_total += total;
    (ctx, state, duration)
  })
}

fn teardown_spsc_async_minimal(
  _ctx: BenchContext,
  _state: SpscBenchState,
  _runtime: &Runtime,
  _cfg: &SpscBenchConfig,
) -> Pin<Box<dyn Future<Output = ()> + Send>> {
  Box::pin(async move {})
}

fn spsc_async_benches(c: &mut Criterion) {
  let rt = Runtime::new().expect("Failed to create Tokio runtime for SPSC async benchmarks");

  AsyncBenchmarkSuite::new(
    c,
    &rt,
    "SpscAsync".to_string(),
    Some(parameter_names()),
    parameter_axes(),
    Box::new(extract_spsc_config),
    setup_fn_spsc_async_minimal,
    benchmark_logic_spsc_async_minimal,
    teardown_spsc_async_minimal,
  )
  .throughput(|cfg: &SpscBenchConfig| Throughput::Elements(cfg.num_items as u64))
  .run();
}

// =============================================================================
// Batch config + helpers
// =============================================================================

#[derive(Debug, Clone)]
struct SpscBatchBenchConfig {
  capacity: usize,
  num_items: usize,
  batch_size: usize,
}

fn extract_spsc_batch_config(combo: &AbstractCombination) -> Result<SpscBatchBenchConfig, String> {
  let capacity = (combo.get_u64(0)? as usize).max(1);
  let num_items = (combo.get_u64(1)? as usize).max(1);
  let batch_size = combo.get_u64(2)? as usize;

  if capacity == 0 {
    return Err(format!(
      "Skipping SPSC combination: capacity cannot be 0. Cap: {}, Items: {}",
      capacity, num_items
    ));
  }
  if batch_size == 0 {
    return Err("Batch size must be at least 1.".to_string());
  }

  Ok(SpscBatchBenchConfig {
    capacity,
    num_items,
    batch_size,
  })
}

fn batch_parameter_axes() -> Vec<Vec<MatrixCellValue>> {
  let mut axes = parameter_axes();
  axes.push(vec![
    // Axis 2: Batch Size
    MatrixCellValue::Unsigned(8),
    MatrixCellValue::Unsigned(64),
    MatrixCellValue::Unsigned(512),
  ]);
  axes
}

fn batch_parameter_names() -> Vec<String> {
  vec!["Cap".to_string(), "Items".to_string(), "Batch".to_string()]
}

// =============================================================================
// Sync Batch
// =============================================================================

fn setup_fn_spsc_sync_batch(
  _cfg: &SpscBatchBenchConfig,
) -> Result<(BenchContext, SpscBenchState), String> {
  Ok((BenchContext::default(), SpscBenchState))
}

fn benchmark_logic_spsc_sync_batch(
  mut ctx: BenchContext,
  state: SpscBenchState,
  cfg: &SpscBatchBenchConfig,
) -> (BenchContext, SpscBenchState, Duration) {
  let total = cfg.num_items;
  if total == 0 {
    return (ctx, state, Duration::from_nanos(0));
  }

  let (producer, consumer) = spsc::bounded_sync(cfg.capacity);
  let batch_size = cfg.batch_size;

  let start_time = Instant::now();

  let producer_handle = thread::spawn(move || {
    let mut remaining = total;
    while remaining > 0 {
      let chunk_size = remaining.min(batch_size);
      let chunk = vec![ITEM_VALUE; chunk_size];
      producer.send_batch(chunk).unwrap();
      remaining -= chunk_size;
    }
  });

  let consumer_handle = thread::spawn(move || loop {
    match consumer.recv_batch(batch_size) {
      Ok(_) => {}
      Err(_) => break,
    }
  });

  producer_handle
    .join()
    .expect("SPSC sync batch producer thread panicked");
  consumer_handle
    .join()
    .expect("SPSC sync batch consumer thread panicked");

  let duration = start_time.elapsed();
  ctx.items_processed_total += total;
  (ctx, state, duration)
}

fn teardown_spsc_sync_batch(_ctx: BenchContext, _state: SpscBenchState, _cfg: &SpscBatchBenchConfig) {}

fn spsc_sync_batch_benches(c: &mut Criterion) {
  SyncBenchmarkSuite::new(
    c,
    "SpscSyncBatch".to_string(),
    Some(batch_parameter_names()),
    batch_parameter_axes(),
    Box::new(extract_spsc_batch_config),
    setup_fn_spsc_sync_batch,
    benchmark_logic_spsc_sync_batch,
    teardown_spsc_sync_batch,
  )
  .throughput(|cfg: &SpscBatchBenchConfig| Throughput::Elements(cfg.num_items as u64))
  .run();
}

// =============================================================================
// Async Batch
// =============================================================================

fn setup_fn_spsc_async_batch(
  _runtime: &Runtime,
  _cfg: &SpscBatchBenchConfig,
) -> Pin<Box<dyn Future<Output = Result<(BenchContext, SpscBenchState), String>> + Send>> {
  Box::pin(async move { Ok((BenchContext::default(), SpscBenchState)) })
}

fn benchmark_logic_spsc_async_batch(
  mut ctx: BenchContext,
  state: SpscBenchState,
  cfg: &SpscBatchBenchConfig,
) -> Pin<Box<dyn Future<Output = (BenchContext, SpscBenchState, Duration)> + Send>> {
  let cfg_clone = cfg.clone();

  Box::pin(async move {
    let total = cfg_clone.num_items;
    if total == 0 {
      return (ctx, state, Duration::from_nanos(0));
    }

    let (mut producer, mut consumer) = spsc::bounded_async(cfg_clone.capacity);
    let batch_size = cfg_clone.batch_size;

    let start_time = Instant::now();

    let producer_handle = tokio::spawn(async move {
      let mut remaining = total;
      while remaining > 0 {
        let chunk_size = remaining.min(batch_size);
        let chunk = vec![ITEM_VALUE; chunk_size];
        producer.send_batch(chunk).await.unwrap();
        remaining -= chunk_size;
      }
    });

    let consumer_handle = tokio::spawn(async move {
      loop {
        match consumer.recv_batch(batch_size).await {
          Ok(_) => {}
          Err(_) => break,
        }
      }
    });

    producer_handle
      .await
      .expect("SPSC async batch producer task panicked");
    consumer_handle
      .await
      .expect("SPSC async batch consumer task panicked");

    let duration = start_time.elapsed();
    ctx.items_processed_total += total;
    (ctx, state, duration)
  })
}

fn teardown_spsc_async_batch(
  _ctx: BenchContext,
  _state: SpscBenchState,
  _runtime: &Runtime,
  _cfg: &SpscBatchBenchConfig,
) -> Pin<Box<dyn Future<Output = ()> + Send>> {
  Box::pin(async move {})
}

fn spsc_async_batch_benches(c: &mut Criterion) {
  let rt = Runtime::new().expect("Failed to create Tokio runtime for SPSC async batch benchmarks");

  AsyncBenchmarkSuite::new(
    c,
    &rt,
    "SpscAsyncBatch".to_string(),
    Some(batch_parameter_names()),
    batch_parameter_axes(),
    Box::new(extract_spsc_batch_config),
    setup_fn_spsc_async_batch,
    benchmark_logic_spsc_async_batch,
    teardown_spsc_async_batch,
  )
  .throughput(|cfg: &SpscBatchBenchConfig| Throughput::Elements(cfg.num_items as u64))
  .run();
}

criterion_group!(
  benches,
  spsc_sync_benches,
  spsc_async_benches,
  spsc_sync_batch_benches,
  spsc_async_batch_benches,
);
criterion_main!(benches);
