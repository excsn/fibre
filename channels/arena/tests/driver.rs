//! Accounting checks for the workload driver.
//!
//! These are not benchmarks: each case moves a few thousand items and asserts
//! only that every item sent is received exactly once. `run_sync`/`run_async`
//! return `Miscounted` when the totals disagree, which is the failure these
//! guard against, particularly on the batch paths where an adapter has to drain
//! accepted items from the caller's vector and report an accurate count.

use channels_arena::adapters::{fibre_ch, flume_ch, tokio_ch};
use channels_arena::driver::{run_async, run_sync};
use channels_arena::spec::{Api, Capacity, Cell, Flavor, Mode, Pairing};

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
