//! Accounting checks for the workload driver.
//!
//! These are not benchmarks: each case moves a few thousand items and asserts
//! only that every item sent is received exactly once. `run_sync`/`run_async`
//! return `Miscounted` when the totals disagree, which is the failure these
//! guard against, particularly on the batch paths where an adapter has to drain
//! accepted items from the caller's vector and report an accurate count.

use channels_arena::adapters::{fibre_ch, flume_ch, oneshot_ch, tokio_ch};
use channels_arena::driver::{run_async, run_oneshot_async, run_oneshot_sync, run_sync};
use channels_arena::spec::{Api, Capacity, Cell, Flavor, Mode, Pairing, Stage};

const ITEMS: u64 = 4_096;

fn cell(flavor: Flavor, mode: Mode, capacity: Capacity, producers: usize, consumers: usize, api: Api) -> Cell {
  Cell {
    flavor,
    mode,
    capacity,
    pairing: Pairing {
      producers,
      consumers,
    },
    api,
    stage: Stage::Stream,
  }
}

fn oneshot_cell(mode: Mode, stage: Stage) -> Cell {
  Cell {
    flavor: Flavor::Oneshot,
    mode,
    capacity: Capacity::Bounded(1),
    pairing: Pairing {
      producers: 1,
      consumers: 1,
    },
    api: Api::Single,
    stage,
  }
}

fn runtime() -> tokio::runtime::Runtime {
  tokio::runtime::Builder::new_multi_thread()
    .worker_threads(4)
    .enable_all()
    .build()
    .unwrap()
}

#[test]
fn fibre_mpsc_sync_batch_and_single_agree() {
  for api in [Api::Single, Api::Batch(512)] {
    for capacity in [Capacity::Bounded(128), Capacity::Bounded(1024)] {
      let cell = cell(Flavor::Mpsc, Mode::Sync, capacity, 4, 1, api);
      run_sync::<fibre_ch::MpscSync>(&cell, ITEMS)
        .unwrap_or_else(|e| panic!("{cell} failed: {e:?}"));
    }
  }
}

#[test]
fn fibre_mpsc_unbounded_batch() {
  for api in [Api::Single, Api::Batch(512)] {
    let cell = cell(Flavor::Mpsc, Mode::Sync, Capacity::Unbounded, 4, 1, api);
    run_sync::<fibre_ch::MpscUnboundedSync>(&cell, ITEMS)
      .unwrap_or_else(|e| panic!("{cell} failed: {e:?}"));
  }
}

#[test]
fn fibre_mpmc_sync_batch_asymmetric() {
  for (producers, consumers) in [(1, 1), (4, 1), (1, 4), (4, 4)] {
    let cell = cell(
      Flavor::Mpmc,
      Mode::Sync,
      Capacity::Bounded(1024),
      producers,
      consumers,
      Api::Batch(512),
    );
    run_sync::<fibre_ch::MpmcSync>(&cell, ITEMS)
      .unwrap_or_else(|e| panic!("{cell} failed: {e:?}"));
  }
}

/// Broadcast: every consumer must receive every item, so the driver expects
/// `sent * consumers` rather than `sent`.
#[test]
fn fibre_spmc_broadcast_batch_accounting() {
  for api in [Api::Single, Api::Batch(512)] {
    let cell = cell(Flavor::Spmc, Mode::Sync, Capacity::Bounded(1024), 1, 4, api);
    run_sync::<fibre_ch::SpmcSync>(&cell, ITEMS)
      .unwrap_or_else(|e| panic!("{cell} failed: {e:?}"));
  }
}

#[test]
fn fibre_spsc_sync_batch() {
  for api in [Api::Single, Api::Batch(512)] {
    let cell = cell(Flavor::Spsc, Mode::Sync, Capacity::Bounded(1024), 1, 1, api);
    run_sync::<fibre_ch::SpscSync>(&cell, ITEMS)
      .unwrap_or_else(|e| panic!("{cell} failed: {e:?}"));
  }
}

#[test]
fn fibre_async_batch_paths() {
  let rt = runtime();
  for api in [Api::Single, Api::Batch(512)] {
    let mpsc = cell(Flavor::Mpsc, Mode::Async, Capacity::Bounded(1024), 4, 1, api);
    run_async::<fibre_ch::MpscAsync>(&rt, &mpsc, ITEMS)
      .unwrap_or_else(|e| panic!("{mpsc} failed: {e:?}"));

    let mpmc = cell(Flavor::Mpmc, Mode::Async, Capacity::Unbounded, 4, 4, api);
    run_async::<fibre_ch::MpmcUnboundedAsync>(&rt, &mpmc, ITEMS)
      .unwrap_or_else(|e| panic!("{mpmc} failed: {e:?}"));
  }
}

/// flume batches on the receive side only, so the send side exercises the
/// trait's looping default.
#[test]
fn flume_recv_only_batch() {
  let cell = cell(Flavor::Mpmc, Mode::Sync, Capacity::Bounded(1024), 4, 4, Api::Batch(512));
  run_sync::<flume_ch::Sync_>(&cell, ITEMS).unwrap_or_else(|e| panic!("{cell} failed: {e:?}"));

  let rt = runtime();
  let cell = cell_async_flume();
  run_async::<flume_ch::Async_>(&rt, &cell, ITEMS)
    .unwrap_or_else(|e| panic!("{cell} failed: {e:?}"));
}

fn cell_async_flume() -> Cell {
  cell(
    Flavor::Mpmc,
    Mode::Async,
    Capacity::Bounded(1024),
    4,
    4,
    Api::Batch(512),
  )
}

/// tokio batches via `recv_many`, again receive side only.
#[test]
fn tokio_recv_many_batch() {
  let rt = runtime();
  for capacity in [Capacity::Bounded(1024), Capacity::Unbounded] {
    let cell = cell(Flavor::Mpsc, Mode::Async, capacity, 4, 1, Api::Batch(512));
    match capacity {
      Capacity::Unbounded => run_async::<tokio_ch::UnboundedAsync>(&rt, &cell, ITEMS)
        .unwrap_or_else(|e| panic!("{cell} failed: {e:?}")),
      _ => run_async::<tokio_ch::Async_>(&rt, &cell, ITEMS)
        .unwrap_or_else(|e| panic!("{cell} failed: {e:?}")),
    };
  }
}

/// A shape refuses handle counts it cannot serve rather than miscounting.
#[test]
fn unsupported_cells_are_reported_not_run() {
  let cell = cell(Flavor::Spsc, Mode::Sync, Capacity::Bounded(128), 4, 1, Api::Single);
  assert!(run_sync::<fibre_ch::SpscSync>(&cell, ITEMS).is_err());
}

macro_rules! oneshot_sync_cases {
  ($($name:ident => $adapter:ty),* $(,)?) => {
    $(
      #[test]
      fn $name() {
        for stage in Stage::ONESHOT {
          let cell = oneshot_cell(Mode::Sync, stage);
          run_oneshot_sync::<$adapter>(&cell, ITEMS)
            .unwrap_or_else(|e| panic!("{cell} failed: {e:?}"));
        }
      }
    )*
  };
}

macro_rules! oneshot_async_cases {
  ($($name:ident => $adapter:ty),* $(,)?) => {
    $(
      #[test]
      fn $name() {
        let rt = runtime();
        for stage in Stage::ONESHOT {
          let cell = oneshot_cell(Mode::Async, stage);
          run_oneshot_async::<$adapter>(&rt, &cell, ITEMS)
            .unwrap_or_else(|e| panic!("{cell} failed: {e:?}"));
        }
      }
    )*
  };
}

oneshot_sync_cases! {
  oneshot_fibre_sync => oneshot_ch::FibreSync,
  oneshot_fibre_exclusive_sync => oneshot_ch::FibreExclusiveSync,
  oneshot_fibre_pool_sync => oneshot_ch::FibrePoolSync,
  oneshot_fibre_pool_host_sync => oneshot_ch::FibrePoolHostSync,
  oneshot_tokio_sync => oneshot_ch::TokioSync,
  oneshot_crate_sync => oneshot_ch::OneshotCrateSync,
  oneshot_lite_sync => oneshot_ch::LiteSyncSync,
  oneshot_sync_oneshot_crate => oneshot_ch::SyncOneshotSync,
}

oneshot_async_cases! {
  oneshot_fibre_async => oneshot_ch::FibreAsync,
  oneshot_fibre_exclusive_async => oneshot_ch::FibreExclusiveAsync,
  oneshot_fibre_pool_async => oneshot_ch::FibrePoolAsync,
  oneshot_fibre_pool_host_async => oneshot_ch::FibrePoolHostAsync,
  oneshot_tokio_async => oneshot_ch::TokioAsync,
  oneshot_futures_async => oneshot_ch::FuturesAsync,
  oneshot_crate_async => oneshot_ch::OneshotCrateAsync,
  oneshot_async_oneshot_crate => oneshot_ch::AsyncOneshotAsync,
  oneshot_lite_sync_async => oneshot_ch::LiteSyncAsync,
}

/// A pool sized under the pairs a handoff holds open has to refuse rather than
/// hand out a slot that is still in use.
#[test]
fn oneshot_pool_refuses_past_its_capacity() {
  let pool = fibre::oneshot::pair_pool::<u64>(2);
  let held: Vec<_> = (0..2).map(|_| pool.pair().unwrap()).collect();
  assert!(pool.pair().is_none());
  drop(held);
  assert!(pool.pair().is_some());
}
