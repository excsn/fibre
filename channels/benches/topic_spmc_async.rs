use bench_matrix::{
  criterion_runner::async_suite::AsyncBenchmarkSuite, AbstractCombination, MatrixCellValue,
};
use criterion::{criterion_group, criterion_main, Criterion, Throughput};
use fibre::spmc::topic as spmc_topic;
use std::{
  future::Future,
  pin::Pin,
  sync::Arc,
  thread::available_parallelism,
  time::{Duration, Instant},
};
use tokio::{runtime::Runtime, sync::Barrier, task::JoinHandle};

const ITEM_VALUE: u64 = 42;
const TOPIC_KEY: &str = "bench_topic";

// --- Config, State, Context ---
#[derive(Debug, Clone)]
struct TopicSpmcBenchConfig {
  num_subscribers: usize,
  mailbox_capacity: usize,
  total_items: usize,
}

#[derive(Default, Debug)]
struct BenchContext;

#[derive(Clone)]
struct TopicSpmcBenchState;

// --- Extractor Function ---
fn extract_config(combo: &AbstractCombination) -> Result<TopicSpmcBenchConfig, String> {
  let num_subscribers = combo.get_u64(0)? as usize;
  let mailbox_capacity = combo.get_u64(1)? as usize;
  let total_items = combo.get_u64(2)? as usize;

  if num_subscribers == 0 {
    return Err("Number of subscribers must be at least 1.".to_string());
  }
  if mailbox_capacity == 0 {
    return Err("Mailbox capacity must be at least 1 for this benchmark.".to_string());
  }

  Ok(TopicSpmcBenchConfig {
    num_subscribers,
    mailbox_capacity,
    total_items,
  })
}

// --- Setup Function ---
fn setup_fn(
  _runtime: &Runtime,
  _cfg: &TopicSpmcBenchConfig,
) -> Pin<Box<dyn Future<Output = Result<(BenchContext, TopicSpmcBenchState), String>> + Send>> {
  Box::pin(async move { Ok((BenchContext::default(), TopicSpmcBenchState)) })
}

// --- Benchmark Logic ---
fn benchmark_logic(
  ctx: BenchContext,
  state: TopicSpmcBenchState,
  cfg: &TopicSpmcBenchConfig,
) -> Pin<Box<dyn Future<Output = (BenchContext, TopicSpmcBenchState, Duration)> + Send>> {
  let cfg_clone = cfg.clone();

  Box::pin(async move {
    let (tx, rx) = spmc_topic::channel_async(cfg_clone.mailbox_capacity);
    let subscribers: Vec<_> = (0..cfg_clone.num_subscribers).map(|_| rx.clone()).collect();
    drop(rx);

    for sub in &subscribers {
      sub.subscribe(TOPIC_KEY);
    }

    let barrier = Arc::new(Barrier::new(cfg_clone.num_subscribers + 1));
    let mut consumer_handles: Vec<JoinHandle<()>> = Vec::with_capacity(cfg_clone.num_subscribers);

     for subscriber in subscribers {
      let barrier_clone = barrier.clone();
      let handle = tokio::spawn(async move {
        barrier_clone.wait().await;
        // Change from a fixed loop to a `while let` loop.
        // This correctly handles the case where messages are dropped and the
        // channel eventually disconnects.
        while let Ok(_) = subscriber.recv().await {
          // The work is simply receiving.
        }
      });
      consumer_handles.push(handle);
    }

    barrier.wait().await;
    let start_time = Instant::now();

    // Producer sends all items
    for i in 0..cfg_clone.total_items {
      tx.send(TOPIC_KEY, ITEM_VALUE + i as u64).unwrap();
    }
    drop(tx);

    for handle in consumer_handles {
      handle.await.unwrap();
    }

    let duration = start_time.elapsed();
    (ctx, state, duration)
  })
}

// --- Teardown Function ---
fn teardown(
  _ctx: BenchContext,
  _state: TopicSpmcBenchState,
  _runtime: &Runtime,
  _cfg: &TopicSpmcBenchConfig,
) -> Pin<Box<dyn Future<Output = ()> + Send>> {
  Box::pin(async move {})
}

// --- Main Benchmark Suite ---
fn topic_spmc_async_benches(c: &mut Criterion) {
  let core_count = usize::from(available_parallelism().unwrap()) as u64;
  let rt = Runtime::new().expect("Failed to create Tokio runtime for topic SPMC async benchmarks");
  let parameter_axes = vec![
    vec![
      // Axis 0: Num Subscribers
      MatrixCellValue::Unsigned(1),
      MatrixCellValue::Unsigned(4),
      MatrixCellValue::Unsigned(core_count),
    ],
    vec![
      // Axis 1: Mailbox Capacity
      MatrixCellValue::Unsigned(1), // High contention/dropping
      MatrixCellValue::Unsigned(128),
    ],
    vec![
      // Axis 2: Total Items
      MatrixCellValue::Unsigned(10_000),
      MatrixCellValue::Unsigned(100_000),
    ],
  ];
  let parameter_names = vec!["Subs", "MailboxCap", "Items"]
    .into_iter()
    .map(String::from)
    .collect();

  AsyncBenchmarkSuite::new(
    c,
    &rt,
    "TopicSpmcAsync".to_string(),
    Some(parameter_names),
    parameter_axes,
    Box::new(extract_config),
    setup_fn,
    benchmark_logic,
    teardown,
  )
  .throughput(|cfg: &TopicSpmcBenchConfig| {
    // Total work is every item being delivered to every subscriber
    Throughput::Elements((cfg.total_items * cfg.num_subscribers) as u64)
  })
  .run();
}

criterion_group!(benches, topic_spmc_async_benches);
criterion_main!(benches);
