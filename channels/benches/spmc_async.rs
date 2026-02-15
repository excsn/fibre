use bench_matrix::{
  criterion_runner::async_suite::AsyncBenchmarkSuite, AbstractCombination, MatrixCellValue,
};
use criterion::{criterion_group, criterion_main, Criterion, Throughput};
use fibre::spmc;
use std::{
  future::Future,
  pin::Pin,
  sync::Arc,
  thread::available_parallelism,
  time::{Duration, Instant},
};
use tokio::{runtime::Runtime, sync::Barrier};

const ITEM_VALUE: u64 = 42;

#[derive(Debug, Clone)]
struct SpmcBenchConfig {
  num_consumers: usize,
  capacity: usize,
  total_items: usize,
}

#[derive(Default, Debug)]
struct BenchContext;

#[derive(Clone)]
struct SpmcBenchState;

fn extract_spmc_config(combo: &AbstractCombination) -> Result<SpmcBenchConfig, String> {
  Ok(SpmcBenchConfig {
    num_consumers: combo.get_u64(0)? as usize,
    capacity: combo.get_u64(1)? as usize,
    total_items: combo.get_u64(2)? as usize,
  })
}

fn setup_fn_spmc_async(
  _rt: &Runtime,
  _cfg: &SpmcBenchConfig,
) -> Pin<Box<dyn Future<Output = Result<(BenchContext, SpmcBenchState), String>> + Send>> {
  Box::pin(async { Ok((BenchContext::default(), SpmcBenchState)) })
}

fn benchmark_logic_spmc_async(
  ctx: BenchContext,
  state: SpmcBenchState,
  cfg: &SpmcBenchConfig,
) -> Pin<Box<dyn Future<Output = (BenchContext, SpmcBenchState, Duration)> + Send>> {
  let cfg_clone = cfg.clone();
  Box::pin(async move {
    let (tx, rx) = spmc::bounded_async(cfg_clone.capacity);
    let receivers: Vec<_> = (0..cfg_clone.num_consumers).map(|_| rx.clone()).collect();
    drop(rx);

    let barrier = Arc::new(Barrier::new(cfg_clone.num_consumers + 1));
    let mut consumer_handles = Vec::new();

    for receiver in receivers {
      let barrier = barrier.clone();
      consumer_handles.push(tokio::spawn(async move {
        barrier.wait().await;
        for _ in 0..cfg_clone.total_items {
          receiver.recv().await.unwrap();
        }
      }));
    }

    barrier.wait().await;
    let start_time = Instant::now();

    for _ in 0..cfg_clone.total_items {
      tx.send(ITEM_VALUE).await.unwrap();
    }

    for handle in consumer_handles {
      handle.await.unwrap();
    }

    let duration = start_time.elapsed();
    (ctx, state, duration)
  })
}

fn spmc_async_benches(c: &mut Criterion) {
  let rt = Runtime::new().unwrap();
  let core_count = usize::from(available_parallelism().unwrap()) as u64;
  AsyncBenchmarkSuite::new(
    c,
    &rt,
    "SpmcAsync".to_string(),
    Some(vec!["Cons".into(), "Cap".into(), "Items".into()]),
    vec![
      vec![
        MatrixCellValue::Unsigned(1),
        MatrixCellValue::Unsigned(4),
        MatrixCellValue::Unsigned(core_count),
      ], // Receivers
      vec![MatrixCellValue::Unsigned(1), MatrixCellValue::Unsigned(128)], // Capacity
      vec![MatrixCellValue::Unsigned(100_000)],                           // Total Items
    ],
    Box::new(extract_spmc_config),
    setup_fn_spmc_async,
    benchmark_logic_spmc_async,
    |_, _, _, _| Box::pin(async {}),
  )
  .throughput(|cfg| Throughput::Elements((cfg.total_items * cfg.num_consumers) as u64))
  .run();
}

criterion_group!(benches, spmc_async_benches);
criterion_main!(benches);
