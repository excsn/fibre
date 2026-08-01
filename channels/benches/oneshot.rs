use bench_matrix::{
  criterion_runner::async_suite::AsyncBenchmarkSuite, AbstractCombination, MatrixCellValue,
};
use criterion::{criterion_group, criterion_main, Criterion, Throughput};
use std::{
  future::Future,
  pin::Pin,
  time::{Duration, Instant},
};
use tokio::runtime::Runtime;

use fibre::oneshot;

const ITEM_VALUE: u64 = 42;

#[derive(Debug, Clone)]
struct OneshotBenchConfig {
  num_items: usize,
}

#[derive(Default, Debug)]
struct BenchContext {
  items_processed_total: usize,
}

struct OneshotAsyncState {
  _marker: (),
}

fn extract_oneshot_config(combo: &AbstractCombination) -> Result<OneshotBenchConfig, String> {
  Ok(OneshotBenchConfig {
    num_items: combo.get_u64(0)? as usize,
  })
}

fn setup_fn_oneshot_async(
  _runtime: &Runtime,
  _cfg: &OneshotBenchConfig,
) -> Pin<Box<dyn Future<Output = Result<(BenchContext, OneshotAsyncState), String>> + Send>> {
  Box::pin(async move { Ok((BenchContext::default(), OneshotAsyncState { _marker: () })) })
}

fn teardown_oneshot_async(
  _ctx: BenchContext,
  _state: OneshotAsyncState,
  _runtime: &Runtime,
  _cfg: &OneshotBenchConfig,
) -> Pin<Box<dyn Future<Output = ()> + Send>> {
  Box::pin(async move {})
}

type LogicFuture = Pin<Box<dyn Future<Output = (BenchContext, OneshotAsyncState, Duration)> + Send>>;

// --- Clonable oneshot() ---

fn logic_clonable_full(
  mut ctx: BenchContext,
  state: OneshotAsyncState,
  cfg: &OneshotBenchConfig,
) -> LogicFuture {
  let cfg = cfg.clone();
  Box::pin(async move {
    let start = Instant::now();
    for _ in 0..cfg.num_items {
      let (tx, rx) = oneshot::oneshot();
      tx.send(ITEM_VALUE).expect("send failed");
      let _ = rx.recv().await.unwrap();
    }
    let duration = start.elapsed();
    ctx.items_processed_total += cfg.num_items;
    (ctx, state, duration)
  })
}

fn logic_clonable_xfer(
  mut ctx: BenchContext,
  state: OneshotAsyncState,
  cfg: &OneshotBenchConfig,
) -> LogicFuture {
  let cfg = cfg.clone();
  Box::pin(async move {
    let mut pairs = Vec::with_capacity(cfg.num_items);
    for _ in 0..cfg.num_items {
      pairs.push(oneshot::oneshot());
    }
    let start = Instant::now();
    for (tx, rx) in pairs {
      tx.send(ITEM_VALUE).expect("send failed");
      let _ = rx.recv().await.unwrap();
    }
    let duration = start.elapsed();
    ctx.items_processed_total += cfg.num_items;
    (ctx, state, duration)
  })
}

// --- exclusive() ---

fn logic_exclusive_full(
  mut ctx: BenchContext,
  state: OneshotAsyncState,
  cfg: &OneshotBenchConfig,
) -> LogicFuture {
  let cfg = cfg.clone();
  Box::pin(async move {
    let start = Instant::now();
    for _ in 0..cfg.num_items {
      let (tx, mut rx) = oneshot::exclusive();
      tx.send(ITEM_VALUE).expect("send failed");
      let _ = rx.recv().await.unwrap();
    }
    let duration = start.elapsed();
    ctx.items_processed_total += cfg.num_items;
    (ctx, state, duration)
  })
}

fn logic_exclusive_xfer(
  mut ctx: BenchContext,
  state: OneshotAsyncState,
  cfg: &OneshotBenchConfig,
) -> LogicFuture {
  let cfg = cfg.clone();
  Box::pin(async move {
    let mut pairs = Vec::with_capacity(cfg.num_items);
    for _ in 0..cfg.num_items {
      pairs.push(oneshot::exclusive());
    }
    let start = Instant::now();
    for (tx, mut rx) in pairs {
      tx.send(ITEM_VALUE).expect("send failed");
      let _ = rx.recv().await.unwrap();
    }
    let duration = start.elapsed();
    ctx.items_processed_total += cfg.num_items;
    (ctx, state, duration)
  })
}

// --- tokio::sync::oneshot ---

fn logic_tokio_full(
  mut ctx: BenchContext,
  state: OneshotAsyncState,
  cfg: &OneshotBenchConfig,
) -> LogicFuture {
  let cfg = cfg.clone();
  Box::pin(async move {
    let start = Instant::now();
    for _ in 0..cfg.num_items {
      let (tx, rx) = tokio::sync::oneshot::channel();
      tx.send(ITEM_VALUE).expect("send failed");
      let _ = rx.await.unwrap();
    }
    let duration = start.elapsed();
    ctx.items_processed_total += cfg.num_items;
    (ctx, state, duration)
  })
}

fn logic_tokio_xfer(
  mut ctx: BenchContext,
  state: OneshotAsyncState,
  cfg: &OneshotBenchConfig,
) -> LogicFuture {
  let cfg = cfg.clone();
  Box::pin(async move {
    let mut pairs = Vec::with_capacity(cfg.num_items);
    for _ in 0..cfg.num_items {
      pairs.push(tokio::sync::oneshot::channel());
    }
    let start = Instant::now();
    for (tx, rx) in pairs {
      tx.send(ITEM_VALUE).expect("send failed");
      let _ = rx.await.unwrap();
    }
    let duration = start.elapsed();
    ctx.items_processed_total += cfg.num_items;
    (ctx, state, duration)
  })
}

fn run_suite(
  c: &mut Criterion,
  rt: &Runtime,
  name: &str,
  logic: fn(BenchContext, OneshotAsyncState, &OneshotBenchConfig) -> LogicFuture,
) {
  let parameter_axes = vec![vec![
    MatrixCellValue::Unsigned(100),
    MatrixCellValue::Unsigned(1000),
  ]];
  let parameter_names = vec!["Ops".to_string()];

  AsyncBenchmarkSuite::new(
    c,
    rt,
    name.to_string(),
    Some(parameter_names),
    parameter_axes,
    Box::new(extract_oneshot_config),
    setup_fn_oneshot_async,
    logic,
    teardown_oneshot_async,
  )
  .throughput(|cfg: &OneshotBenchConfig| Throughput::Elements(cfg.num_items as u64))
  .run();
}

fn oneshot_async_benches(c: &mut Criterion) {
  let rt = Runtime::new().unwrap();
  run_suite(c, &rt, "OneshotAsync", logic_clonable_full);
  run_suite(c, &rt, "OneshotAsyncXfer", logic_clonable_xfer);
  run_suite(c, &rt, "OneshotExclusiveAsync", logic_exclusive_full);
  run_suite(c, &rt, "OneshotExclusiveAsyncXfer", logic_exclusive_xfer);
  run_suite(c, &rt, "OneshotTokioAsync", logic_tokio_full);
  run_suite(c, &rt, "OneshotTokioAsyncXfer", logic_tokio_xfer);
}

criterion_group!(benches, oneshot_async_benches);
criterion_main!(benches);
