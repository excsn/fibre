use bench_matrix::{
  criterion_runner::{async_suite::AsyncBenchmarkSuite, sync_suite::SyncBenchmarkSuite},
  AbstractCombination, MatrixCellValue,
};
use criterion::{criterion_group, criterion_main, Criterion, Throughput};
use fibre::mpmc;
use std::{
  future::Future,
  pin::Pin,
  thread::{self, available_parallelism},
  time::{Duration, Instant},
};
use tokio::{runtime::Runtime, task::JoinHandle};

const ITEM_VALUE: u64 = 42;

// ==========================================
// --- SECTION 1: Shared Configuration ---
// ==========================================

#[derive(Debug, Clone)]
struct MpmcRendezvousBenchConfig {
  num_producers: usize,
  num_consumers: usize,
  total_items: usize,
}

#[derive(Default, Debug)]
struct BenchContext {
  actual_items_processed: usize,
}

#[derive(Clone)]
struct MpmcRendezvousBenchState;

fn extract_mpmc_rendezvous_config(
  combo: &AbstractCombination,
) -> Result<MpmcRendezvousBenchConfig, String> {
  let num_producers = combo.get_u64(0)? as usize;
  let num_consumers = combo.get_u64(1)? as usize;
  let total_items = combo.get_u64(2)? as usize;

  if num_producers == 0 || num_consumers == 0 {
    return Err("Number of producers and consumers must be at least 1.".to_string());
  }
  if total_items > 0 && total_items < num_producers {
    return Err(format!(
      "Total items ({}) must be >= num_producers ({})",
      total_items, num_producers
    ));
  }

  Ok(MpmcRendezvousBenchConfig {
    num_producers,
    num_consumers,
    total_items,
  })
}

// ==========================================
// --- SECTION 2: Synchronous Implementation ---
// ==========================================

fn setup_fn_mpmc_rendezvous_sync(
  _cfg: &MpmcRendezvousBenchConfig,
) -> Result<(BenchContext, MpmcRendezvousBenchState), String> {
  Ok((BenchContext::default(), MpmcRendezvousBenchState))
}

fn benchmark_logic_mpmc_rendezvous_sync(
  mut ctx: BenchContext,
  state: MpmcRendezvousBenchState,
  cfg: &MpmcRendezvousBenchConfig,
) -> (BenchContext, MpmcRendezvousBenchState, Duration) {
  let total = cfg.total_items;
  if total == 0 {
    return (ctx, state, Duration::from_nanos(0));
  }

  let (main_producer, main_consumer) = mpmc::rendezvous::rendezvous();

  let mut producer_handles = Vec::with_capacity(cfg.num_producers);
  let start_time = Instant::now();

  for p_idx in 0..cfg.num_producers {
    let producer_clone = main_producer.clone();
    let items_this_producer = {
      let base = total / cfg.num_producers;
      let remainder = total % cfg.num_producers;
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
    handle.join().expect("MPMC rendezvous sync producer thread panicked");
  }
  for handle in consumer_handles {
    handle.join().expect("MPMC rendezvous sync consumer thread panicked");
  }

  let duration = start_time.elapsed();
  ctx.actual_items_processed += total;
  (ctx, state, duration)
}

// ==========================================
// --- SECTION 3: Asynchronous Implementation ---
// ==========================================

fn setup_fn_mpmc_rendezvous_async(
  _rt: &Runtime,
  _cfg: &MpmcRendezvousBenchConfig,
) -> Pin<Box<dyn Future<Output = Result<(BenchContext, MpmcRendezvousBenchState), String>> + Send>>
{
  Box::pin(async move { Ok((BenchContext::default(), MpmcRendezvousBenchState)) })
}

fn benchmark_logic_mpmc_rendezvous_async(
  mut ctx: BenchContext,
  state: MpmcRendezvousBenchState,
  cfg: &MpmcRendezvousBenchConfig,
) -> Pin<Box<dyn Future<Output = (BenchContext, MpmcRendezvousBenchState, Duration)> + Send>> {
  let cfg_clone = cfg.clone();

  Box::pin(async move {
    let total = cfg_clone.total_items;
    if total == 0 {
      return (ctx, state, Duration::from_nanos(0));
    }

    let (main_producer, main_consumer) = mpmc::rendezvous::rendezvous_async();

    let mut producer_handles: Vec<JoinHandle<()>> = Vec::with_capacity(cfg_clone.num_producers);
    let start_time = Instant::now();

    for p_idx in 0..cfg_clone.num_producers {
      let producer_clone = main_producer.clone();
      let items_this_producer = {
        let base = total / cfg_clone.num_producers;
        let remainder = total % cfg_clone.num_producers;
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
      handle.await.expect("MPMC rendezvous async producer task panicked");
    }
    for handle in consumer_handles {
      handle.await.expect("MPMC rendezvous async consumer task panicked");
    }

    let duration = start_time.elapsed();
    ctx.actual_items_processed += total;
    (ctx, state, duration)
  })
}

// ==========================================
// --- SECTION 4: Criterion Group Suite Setup ---
// ==========================================

fn mpmc_rendezvous_sync_benches(c: &mut Criterion) {
  let core_count = usize::from(available_parallelism().unwrap()) as u64;
  let parameter_axes = vec![
    vec![
      // Axis 0: Num Producers
      MatrixCellValue::Unsigned(1),
      MatrixCellValue::Unsigned(4),
      MatrixCellValue::Unsigned(core_count),
    ],
    vec![
      // Axis 1: Num Consumers
      MatrixCellValue::Unsigned(1),
      MatrixCellValue::Unsigned(4),
      MatrixCellValue::Unsigned(core_count),
    ],
    vec![
      // Axis 2: Total Items
      MatrixCellValue::Unsigned(100_000),
      MatrixCellValue::Unsigned(1_000_000),
    ],
  ];
  let parameter_names = vec!["Prod".to_string(), "Cons".to_string(), "Items".to_string()];

  SyncBenchmarkSuite::new(
    c,
    "MpmcRendezvousSync".to_string(),
    Some(parameter_names),
    parameter_axes,
    Box::new(extract_mpmc_rendezvous_config),
    setup_fn_mpmc_rendezvous_sync,
    benchmark_logic_mpmc_rendezvous_sync,
    |_, _, _| {},
  )
  .throughput(|cfg: &MpmcRendezvousBenchConfig| Throughput::Elements(cfg.total_items as u64))
  .run();
}

fn mpmc_rendezvous_async_benches(c: &mut Criterion) {
  let rt = Runtime::new().unwrap();
  let core_count = usize::from(available_parallelism().unwrap()) as u64;
  let parameter_axes = vec![
    vec![
      // Axis 0: Num Producers
      MatrixCellValue::Unsigned(1),
      MatrixCellValue::Unsigned(4),
      MatrixCellValue::Unsigned(core_count),
    ],
    vec![
      // Axis 1: Num Consumers
      MatrixCellValue::Unsigned(1),
      MatrixCellValue::Unsigned(4),
      MatrixCellValue::Unsigned(core_count),
    ],
    vec![
      // Axis 2: Total Items
      MatrixCellValue::Unsigned(100_000),
      MatrixCellValue::Unsigned(1_000_000),
    ],
  ];
  let parameter_names = vec!["Prod".to_string(), "Cons".to_string(), "Items".to_string()];

  AsyncBenchmarkSuite::new(
    c,
    &rt,
    "MpmcRendezvousAsync".to_string(),
    Some(parameter_names),
    parameter_axes,
    Box::new(extract_mpmc_rendezvous_config),
    setup_fn_mpmc_rendezvous_async,
    benchmark_logic_mpmc_rendezvous_async,
    |_, _, _, _| Box::pin(async {}),
  )
  .throughput(|cfg: &MpmcRendezvousBenchConfig| Throughput::Elements(cfg.total_items as u64))
  .run();
}

criterion_group!(benches, mpmc_rendezvous_sync_benches, mpmc_rendezvous_async_benches);
criterion_main!(benches);
