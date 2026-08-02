use bench_matrix::{
  criterion_runner::async_suite::AsyncBenchmarkSuite, AbstractCombination, MatrixCellValue,
};
use criterion::{criterion_group, criterion_main, Criterion, Throughput};
use std::{
  future::Future,
  hint::black_box,
  pin::Pin,
  sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Barrier,
  },
  thread,
  time::{Duration, Instant},
};
use tokio::runtime::Runtime;

use fibre::oneshot::{pair_pool, OneshotHostPool, PoolSlot};

const ITEM_VALUE: u64 = 42;

/// Stand-in for an in-flight request record living inside a host pool cell.
struct Req {
  id: u64,
  reply: PoolSlot<u64>,
}

/// The same record for the plain-pool record arm, allocated per request since
/// that channel carries no record.
struct PlainReq {
  id: u64,
}

fn req_pool(capacity: usize) -> OneshotHostPool<Req, u64> {
  OneshotHostPool::new(
    capacity,
    || Req {
      id: 0,
      reply: PoolSlot::new(),
    },
    |r| &r.reply,
  )
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum PoolKind {
  Pair,
  Host,
}

#[derive(Debug, Clone)]
struct PoolBenchConfig {
  pool: PoolKind,
  num_items: usize,
}

#[derive(Default, Debug)]
struct BenchContext {
  items_processed_total: usize,
}

struct PoolAsyncState {
  _marker: (),
}

fn extract_pool_config(combo: &AbstractCombination) -> Result<PoolBenchConfig, String> {
  let pool = match combo.get_tag(0)? {
    "Pair" => PoolKind::Pair,
    "Host" => PoolKind::Host,
    other => return Err(format!("unknown pool axis value: {other}")),
  };
  Ok(PoolBenchConfig {
    pool,
    num_items: combo.get_u64(1)? as usize,
  })
}

fn setup_fn_pool_async(
  _runtime: &Runtime,
  _cfg: &PoolBenchConfig,
) -> Pin<Box<dyn Future<Output = Result<(BenchContext, PoolAsyncState), String>> + Send>> {
  Box::pin(async move { Ok((BenchContext::default(), PoolAsyncState { _marker: () })) })
}

fn teardown_pool_async(
  _ctx: BenchContext,
  _state: PoolAsyncState,
  _runtime: &Runtime,
  _cfg: &PoolBenchConfig,
) -> Pin<Box<dyn Future<Output = ()> + Send>> {
  Box::pin(async move {})
}

type LogicFuture = Pin<Box<dyn Future<Output = (BenchContext, PoolAsyncState, Duration)> + Send>>;

fn logic_full(
  mut ctx: BenchContext,
  state: PoolAsyncState,
  cfg: &PoolBenchConfig,
) -> LogicFuture {
  let cfg = cfg.clone();
  Box::pin(async move {
    let mut sum = 0u64;
    let duration = match cfg.pool {
      PoolKind::Pair => {
        let pool = pair_pool::<u64>(64);
        let start = Instant::now();
        for _ in 0..cfg.num_items {
          let (tx, mut rx) = pool.pair().expect("pool exhausted");
          tx.send(ITEM_VALUE).expect("send failed");
          sum += rx.recv().await.unwrap();
        }
        start.elapsed()
      }
      PoolKind::Host => {
        let pool = req_pool(64);
        let start = Instant::now();
        for _ in 0..cfg.num_items {
          let (tx, mut rx) = pool.pair_init(|_| {}).expect("pool exhausted");
          tx.send(ITEM_VALUE).expect("send failed");
          sum += rx.recv().await.unwrap();
        }
        start.elapsed()
      }
    };
    black_box(sum);
    ctx.items_processed_total += cfg.num_items;
    (ctx, state, duration)
  })
}

fn logic_xfer(
  mut ctx: BenchContext,
  state: PoolAsyncState,
  cfg: &PoolBenchConfig,
) -> LogicFuture {
  let cfg = cfg.clone();
  Box::pin(async move {
    let mut sum = 0u64;
    let duration = match cfg.pool {
      PoolKind::Pair => {
        let pool = pair_pool::<u64>(cfg.num_items + 4);
        let mut pairs = Vec::with_capacity(cfg.num_items);
        for _ in 0..cfg.num_items {
          pairs.push(pool.pair().expect("pool exhausted"));
        }
        let start = Instant::now();
        for (tx, mut rx) in pairs {
          tx.send(ITEM_VALUE).expect("send failed");
          sum += rx.recv().await.unwrap();
        }
        start.elapsed()
      }
      PoolKind::Host => {
        let pool = req_pool(cfg.num_items + 4);
        let mut pairs = Vec::with_capacity(cfg.num_items);
        for _ in 0..cfg.num_items {
          pairs.push(pool.pair_init(|_| {}).expect("pool exhausted"));
        }
        let start = Instant::now();
        for (tx, mut rx) in pairs {
          tx.send(ITEM_VALUE).expect("send failed");
          sum += rx.recv().await.unwrap();
        }
        start.elapsed()
      }
    };
    black_box(sum);
    ctx.items_processed_total += cfg.num_items;
    (ctx, state, duration)
  })
}

fn logic_batch(
  mut ctx: BenchContext,
  state: PoolAsyncState,
  cfg: &PoolBenchConfig,
) -> LogicFuture {
  let cfg = cfg.clone();
  Box::pin(async move {
    let mut sum = 0u64;
    let duration = match cfg.pool {
      PoolKind::Pair => {
        let pool = pair_pool::<u64>(cfg.num_items + 4);
        let start = Instant::now();
        let pairs = pool.pair_batch(cfg.num_items).expect("pool exhausted");
        for (tx, mut rx) in pairs {
          tx.send(ITEM_VALUE).expect("send failed");
          sum += rx.recv().await.unwrap();
        }
        start.elapsed()
      }
      PoolKind::Host => {
        let pool = req_pool(cfg.num_items + 4);
        let start = Instant::now();
        let pairs = pool
          .pair_init_batch(cfg.num_items, |_| {})
          .expect("pool exhausted");
        for (tx, mut rx) in pairs {
          tx.send(ITEM_VALUE).expect("send failed");
          sum += rx.recv().await.unwrap();
        }
        start.elapsed()
      }
    };
    black_box(sum);
    ctx.items_processed_total += cfg.num_items;
    (ctx, state, duration)
  })
}

fn logic_record(
  mut ctx: BenchContext,
  state: PoolAsyncState,
  cfg: &PoolBenchConfig,
) -> LogicFuture {
  let cfg = cfg.clone();
  Box::pin(async move {
    let mut sum = 0u64;
    let duration = match cfg.pool {
      PoolKind::Pair => {
        let pool = pair_pool::<u64>(64);
        let start = Instant::now();
        for i in 0..cfg.num_items {
          let core = Arc::new(PlainReq { id: i as u64 });
          let (tx, mut rx) = pool.pair().expect("pool exhausted");
          tx.send(core.id).expect("send failed");
          sum += rx.recv().await.unwrap();
          black_box(&core);
        }
        start.elapsed()
      }
      PoolKind::Host => {
        let pool = req_pool(64);
        let start = Instant::now();
        for i in 0..cfg.num_items {
          let (tx, mut rx) = pool.pair_init(|r| r.id = i as u64).expect("pool exhausted");
          tx.send(rx.host().id).expect("send failed");
          sum += rx.recv().await.unwrap();
        }
        start.elapsed()
      }
    };
    black_box(sum);
    ctx.items_processed_total += cfg.num_items;
    (ctx, state, duration)
  })
}

fn logic_handoff(
  mut ctx: BenchContext,
  state: PoolAsyncState,
  cfg: &PoolBenchConfig,
) -> LogicFuture {
  let cfg = cfg.clone();
  Box::pin(async move {
    let n = cfg.num_items;
    let mut senders = Vec::with_capacity(n);
    let mut receivers = Vec::with_capacity(n);
    let _pair_pool_guard;
    let _host_pool_guard;
    match cfg.pool {
      PoolKind::Pair => {
        let pool = pair_pool::<u64>(n + 4);
        for _ in 0..n {
          let (tx, rx) = pool.pair().expect("pool exhausted");
          senders.push(PoolSender::Pair(tx));
          receivers.push(PoolReceiver::Pair(rx));
        }
        _pair_pool_guard = Some(pool);
        _host_pool_guard = None;
      }
      PoolKind::Host => {
        let pool = req_pool(n + 4);
        for _ in 0..n {
          let (tx, rx) = pool.pair_init(|_| {}).expect("pool exhausted");
          senders.push(PoolSender::Host(tx));
          receivers.push(PoolReceiver::Host(rx));
        }
        _pair_pool_guard = None;
        _host_pool_guard = Some(pool);
      }
    }
    let barrier = Arc::new(Barrier::new(2));

    let sender = {
      let barrier = Arc::clone(&barrier);
      thread::spawn(move || {
        barrier.wait();
        for tx in senders {
          tx.send(ITEM_VALUE).expect("send failed");
        }
      })
    };

    barrier.wait();
    let start = Instant::now();
    let mut sum = 0u64;
    for mut rx in receivers {
      sum += rx.recv().await.unwrap();
    }
    let duration = start.elapsed();

    sender.join().expect("pool sender thread panicked");
    black_box(sum);
    ctx.items_processed_total += n;
    (ctx, state, duration)
  })
}

fn logic_pingpong(
  mut ctx: BenchContext,
  state: PoolAsyncState,
  cfg: &PoolBenchConfig,
) -> LogicFuture {
  let cfg = cfg.clone();
  Box::pin(async move {
    let n = cfg.num_items;
    let (mut in_tx, mut in_rx) = (Vec::with_capacity(n), Vec::with_capacity(n));
    let (mut out_tx, mut out_rx) = (Vec::with_capacity(n), Vec::with_capacity(n));
    let _pair_pool_guard;
    let _host_pool_guard;
    match cfg.pool {
      PoolKind::Pair => {
        let pool = pair_pool::<u64>(n * 2 + 4);
        for _ in 0..n {
          let (tx, rx) = pool.pair().expect("pool exhausted");
          in_tx.push(PoolSender::Pair(tx));
          in_rx.push(PoolReceiver::Pair(rx));
          let (tx, rx) = pool.pair().expect("pool exhausted");
          out_tx.push(PoolSender::Pair(tx));
          out_rx.push(PoolReceiver::Pair(rx));
        }
        _pair_pool_guard = Some(pool);
        _host_pool_guard = None;
      }
      PoolKind::Host => {
        let pool = req_pool(n * 2 + 4);
        for _ in 0..n {
          let (tx, rx) = pool.pair_init(|_| {}).expect("pool exhausted");
          in_tx.push(PoolSender::Host(tx));
          in_rx.push(PoolReceiver::Host(rx));
          let (tx, rx) = pool.pair_init(|_| {}).expect("pool exhausted");
          out_tx.push(PoolSender::Host(tx));
          out_rx.push(PoolReceiver::Host(rx));
        }
        _pair_pool_guard = None;
        _host_pool_guard = Some(pool);
      }
    }
    let barrier = Arc::new(Barrier::new(2));

    let peer = {
      let barrier = Arc::clone(&barrier);
      thread::spawn(move || {
        barrier.wait();
        for (tx, mut rx) in in_tx.into_iter().zip(out_rx) {
          tx.send(ITEM_VALUE).expect("send failed");
          futures_executor::block_on(rx.recv()).unwrap();
        }
      })
    };

    barrier.wait();
    let start = Instant::now();
    let mut sum = 0u64;
    for (mut rx, tx) in in_rx.into_iter().zip(out_tx) {
      sum += rx.recv().await.unwrap();
      tx.send(ITEM_VALUE).expect("send failed");
    }
    let duration = start.elapsed();

    peer.join().expect("pool ping-pong peer thread panicked");
    black_box(sum);
    ctx.items_processed_total += n;
    (ctx, state, duration)
  })
}

/// Handle wrappers so the cross-thread arms can hold either pool's handles in
/// one vector; the enum dispatch sits outside the measured per-op work for the
/// send side and adds one predictable branch per receive.
enum PoolSender {
  Pair(fibre::oneshot::PooledSender<u64>),
  Host(fibre::oneshot::HostSender<Req, u64>),
}

impl PoolSender {
  fn send(self, value: u64) -> Result<(), fibre::error::TrySendError<u64>> {
    match self {
      PoolSender::Pair(tx) => tx.send(value),
      PoolSender::Host(tx) => tx.send(value),
    }
  }
}

enum PoolReceiver {
  Pair(fibre::oneshot::PooledReceiver<u64>),
  Host(fibre::oneshot::HostReceiver<Req, u64>),
}

impl PoolReceiver {
  async fn recv(&mut self) -> Result<u64, fibre::error::RecvError> {
    match self {
      PoolReceiver::Pair(rx) => rx.recv().await,
      PoolReceiver::Host(rx) => rx.recv().await,
    }
  }
}

fn run_suite(
  c: &mut Criterion,
  rt: &Runtime,
  name: &str,
  logic: fn(BenchContext, PoolAsyncState, &PoolBenchConfig) -> LogicFuture,
  ops: &[u64],
) {
  let parameter_axes: Vec<Vec<MatrixCellValue>> = vec![
    vec![
      MatrixCellValue::Tag("Pair".to_string()),
      MatrixCellValue::Tag("Host".to_string()),
    ],
    ops.iter().copied().map(MatrixCellValue::Unsigned).collect(),
  ];
  let parameter_names = vec!["Pool".to_string(), "Ops".to_string()];

  AsyncBenchmarkSuite::new(
    c,
    rt,
    name.to_string(),
    Some(parameter_names),
    parameter_axes,
    Box::new(extract_pool_config),
    setup_fn_pool_async,
    logic,
    teardown_pool_async,
  )
  .throughput(|cfg: &PoolBenchConfig| Throughput::Elements(cfg.num_items as u64))
  .run();
}

/// Reports what fraction of the handoff and ping-pong receives actually parked,
/// so those numbers can be read. Gated on `ONESHOT_POOL_PARK_DIAG` because the
/// counting wrapper allocates per receive; it never runs inside a timed region.
fn report_parked_fraction(rt: &Runtime) {
  const OPS: usize = 10_000;

  async fn counting<F: Future>(fut: F, parks: &AtomicUsize) -> F::Output {
    let mut fut = Box::pin(fut);
    std::future::poll_fn(|cx| {
      let polled = fut.as_mut().poll(cx);
      if polled.is_pending() {
        parks.fetch_add(1, Ordering::Relaxed);
      }
      polled
    })
    .await
  }

  for kind in [PoolKind::Pair, PoolKind::Host] {
    let parks = AtomicUsize::new(0);
    rt.block_on(async {
      let mut senders = Vec::with_capacity(OPS);
      let mut receivers = Vec::with_capacity(OPS);
      let _pair_pool_guard;
      let _host_pool_guard;
      match kind {
        PoolKind::Pair => {
          let pool = pair_pool::<u64>(OPS + 4);
          for _ in 0..OPS {
            let (tx, rx) = pool.pair().expect("pool exhausted");
            senders.push(PoolSender::Pair(tx));
            receivers.push(PoolReceiver::Pair(rx));
          }
          _pair_pool_guard = Some(pool);
          _host_pool_guard = None;
        }
        PoolKind::Host => {
          let pool = req_pool(OPS + 4);
          for _ in 0..OPS {
            let (tx, rx) = pool.pair_init(|_| {}).expect("pool exhausted");
            senders.push(PoolSender::Host(tx));
            receivers.push(PoolReceiver::Host(rx));
          }
          _pair_pool_guard = None;
          _host_pool_guard = Some(pool);
        }
      }
      let barrier = Arc::new(Barrier::new(2));
      let sender = {
        let barrier = Arc::clone(&barrier);
        thread::spawn(move || {
          barrier.wait();
          for tx in senders {
            tx.send(ITEM_VALUE).expect("send failed");
          }
        })
      };
      barrier.wait();
      for mut rx in receivers {
        counting(rx.recv(), &parks).await.unwrap();
      }
      sender.join().unwrap();
    });
    eprintln!(
      "OneshotPoolHandoff/{kind:?}: {}/{} receives parked",
      parks.load(Ordering::Relaxed),
      OPS
    );

    let parks = AtomicUsize::new(0);
    rt.block_on(async {
      let (mut in_tx, mut in_rx) = (Vec::with_capacity(OPS), Vec::with_capacity(OPS));
      let (mut out_tx, mut out_rx) = (Vec::with_capacity(OPS), Vec::with_capacity(OPS));
      let _pair_pool_guard;
      let _host_pool_guard;
      match kind {
        PoolKind::Pair => {
          let pool = pair_pool::<u64>(OPS * 2 + 4);
          for _ in 0..OPS {
            let (tx, rx) = pool.pair().expect("pool exhausted");
            in_tx.push(PoolSender::Pair(tx));
            in_rx.push(PoolReceiver::Pair(rx));
            let (tx, rx) = pool.pair().expect("pool exhausted");
            out_tx.push(PoolSender::Pair(tx));
            out_rx.push(PoolReceiver::Pair(rx));
          }
          _pair_pool_guard = Some(pool);
          _host_pool_guard = None;
        }
        PoolKind::Host => {
          let pool = req_pool(OPS * 2 + 4);
          for _ in 0..OPS {
            let (tx, rx) = pool.pair_init(|_| {}).expect("pool exhausted");
            in_tx.push(PoolSender::Host(tx));
            in_rx.push(PoolReceiver::Host(rx));
            let (tx, rx) = pool.pair_init(|_| {}).expect("pool exhausted");
            out_tx.push(PoolSender::Host(tx));
            out_rx.push(PoolReceiver::Host(rx));
          }
          _pair_pool_guard = None;
          _host_pool_guard = Some(pool);
        }
      }
      let barrier = Arc::new(Barrier::new(2));
      let peer = {
        let barrier = Arc::clone(&barrier);
        thread::spawn(move || {
          barrier.wait();
          for (tx, mut rx) in in_tx.into_iter().zip(out_rx) {
            tx.send(ITEM_VALUE).expect("send failed");
            futures_executor::block_on(rx.recv()).unwrap();
          }
        })
      };
      barrier.wait();
      for (mut rx, tx) in in_rx.into_iter().zip(out_tx) {
        counting(rx.recv(), &parks).await.unwrap();
        tx.send(ITEM_VALUE).expect("send failed");
      }
      peer.join().unwrap();
    });
    eprintln!(
      "OneshotPoolPingPong/{kind:?}: {}/{} receives parked",
      parks.load(Ordering::Relaxed),
      OPS
    );
  }
}

fn oneshot_pool_async_benches(c: &mut Criterion) {
  let rt = Runtime::new().unwrap();

  if std::env::var_os("ONESHOT_POOL_PARK_DIAG").is_some() {
    report_parked_fraction(&rt);
  }

  const OPS: &[u64] = &[100, 1000];
  const HANDOFF_OPS: &[u64] = &[1000, 10_000];

  run_suite(c, &rt, "OneshotPoolAsync", logic_full, OPS);
  run_suite(c, &rt, "OneshotPoolAsyncXfer", logic_xfer, OPS);
  run_suite(c, &rt, "OneshotPoolBatchAsync", logic_batch, OPS);
  run_suite(c, &rt, "OneshotPoolRecordAsync", logic_record, OPS);
  run_suite(c, &rt, "OneshotPoolHandoff", logic_handoff, HANDOFF_OPS);
  run_suite(c, &rt, "OneshotPoolPingPong", logic_pingpong, HANDOFF_OPS);
}

criterion_group!(benches, oneshot_pool_async_benches);
criterion_main!(benches);
