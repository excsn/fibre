use bench_matrix::{
  criterion_runner::sync_suite::SyncBenchmarkSuite, AbstractCombination, MatrixCellValue,
};
use criterion::{criterion_group, criterion_main, Criterion, Throughput};
use fibre::spmc::topic as spmc_topic;
use std::{
  sync::{Arc, Barrier},
  thread::{self, available_parallelism},
  time::{Duration, Instant},
};

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
fn setup_fn(_cfg: &TopicSpmcBenchConfig) -> Result<(BenchContext, TopicSpmcBenchState), String> {
  Ok((BenchContext::default(), TopicSpmcBenchState))
}

// --- Benchmark Logic ---
fn benchmark_logic(
  _ctx: BenchContext,
  state: TopicSpmcBenchState,
  cfg: &TopicSpmcBenchConfig,
) -> (BenchContext, TopicSpmcBenchState, Duration) {
  let (tx, rx) = spmc_topic::channel(cfg.mailbox_capacity);

  // Create all subscriber clones
  let subscribers: Vec<_> = (0..cfg.num_subscribers).map(|_| rx.clone()).collect();
  drop(rx); // Drop original

  // All subscribers listen to the same topic
  for sub in &subscribers {
    sub.subscribe(TOPIC_KEY);
  }

  let barrier = Arc::new(Barrier::new(cfg.num_subscribers + 1));

  // The thread::scope closure will return the measured duration.
  let duration = thread::scope(|s| {
    for subscriber in subscribers {
      let barrier_clone = Arc::clone(&barrier);
      s.spawn(move || {
        barrier_clone.wait(); // Wait for producer to be ready

        // Change from a fixed loop to a `while let` loop.
        while let Ok(_) = subscriber.recv() {
          // The work is simply receiving.
        }
      });
    }

    barrier.wait(); // Wait for all subscribers to be ready
    let start_time = Instant::now(); // Initialize start_time right before the work begins.

    // Producer sends all items
    for i in 0..cfg.total_items {
      tx.send(TOPIC_KEY, ITEM_VALUE + i as u64).unwrap();
    }
    // The scope will automatically join all threads.
    // We must drop the sender so receivers see the disconnect and terminate.
    drop(tx);

    // Return the duration from the scope.
    start_time.elapsed()
  });

  (BenchContext::default(), state, duration)
}

// --- Teardown Function ---
fn teardown(_ctx: BenchContext, _state: TopicSpmcBenchState, _cfg: &TopicSpmcBenchConfig) {}

// --- Main Benchmark Suite ---
fn topic_spmc_sync_benches(c: &mut Criterion) {
  let core_count = usize::from(available_parallelism().unwrap()) as u64;
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

  SyncBenchmarkSuite::new(
    c,
    "TopicSpmcSync".to_string(),
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

criterion_group!(benches, topic_spmc_sync_benches);
criterion_main!(benches);
