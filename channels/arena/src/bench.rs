use crate::channel::{AsyncChannel, AsyncOneshotChannel, OneshotChannel, SyncChannel};
use crate::driver::{RunError, RunResult, run_async, run_oneshot_async, run_oneshot_sync, run_sync};
use crate::measure::{Budget, calibrate};
use crate::spec::{BatchSupport, Cell, Flavor, Mode};

use std::marker::PhantomData;
use tokio::runtime::Runtime;

/// Type-erased entry so the registry can hold every adapter in one list.
///
/// Calibration and sampling are separate calls so the runner can interleave
/// sample rounds across implementations instead of finishing one before
/// starting the next.
pub trait Bench: Send + Sync {
  fn library(&self) -> &'static str;
  fn flavor(&self) -> Flavor;
  fn mode(&self) -> Mode;
  fn batch_support(&self) -> BatchSupport;
  fn calibrate(&self, rt: &Runtime, cell: &Cell, budget: &Budget) -> Result<u64, RunError>;
  fn run_once(&self, rt: &Runtime, cell: &Cell, items: u64) -> RunResult;
}

pub struct SyncEntry<C: SyncChannel> {
  library: &'static str,
  flavor: Flavor,
  _channel: PhantomData<fn() -> C>,
}

impl<C: SyncChannel> SyncEntry<C> {
  pub fn boxed(library: &'static str, flavor: Flavor) -> Box<dyn Bench> {
    Box::new(SyncEntry::<C> {
      library,
      flavor,
      _channel: PhantomData,
    })
  }
}

impl<C: SyncChannel> Bench for SyncEntry<C> {
  fn library(&self) -> &'static str {
    self.library
  }

  fn flavor(&self) -> Flavor {
    self.flavor
  }

  fn mode(&self) -> Mode {
    Mode::Sync
  }

  fn batch_support(&self) -> BatchSupport {
    C::BATCH
  }

  fn calibrate(&self, _rt: &Runtime, cell: &Cell, budget: &Budget) -> Result<u64, RunError> {
    calibrate(budget, cell, |items| run_sync::<C>(cell, items))
  }

  fn run_once(&self, _rt: &Runtime, cell: &Cell, items: u64) -> RunResult {
    run_sync::<C>(cell, items)
  }
}

pub struct AsyncEntry<C: AsyncChannel> {
  library: &'static str,
  flavor: Flavor,
  _channel: PhantomData<fn() -> C>,
}

impl<C: AsyncChannel> AsyncEntry<C> {
  pub fn boxed(library: &'static str, flavor: Flavor) -> Box<dyn Bench> {
    Box::new(AsyncEntry::<C> {
      library,
      flavor,
      _channel: PhantomData,
    })
  }
}

impl<C: AsyncChannel> Bench for AsyncEntry<C> {
  fn library(&self) -> &'static str {
    self.library
  }

  fn flavor(&self) -> Flavor {
    self.flavor
  }

  fn mode(&self) -> Mode {
    Mode::Async
  }

  fn batch_support(&self) -> BatchSupport {
    C::BATCH
  }

  fn calibrate(&self, rt: &Runtime, cell: &Cell, budget: &Budget) -> Result<u64, RunError> {
    calibrate(budget, cell, |items| run_async::<C>(rt, cell, items))
  }

  fn run_once(&self, rt: &Runtime, cell: &Cell, items: u64) -> RunResult {
    run_async::<C>(rt, cell, items)
  }
}

pub struct OneshotSyncEntry<C: OneshotChannel> {
  library: &'static str,
  _channel: PhantomData<fn() -> C>,
}

impl<C: OneshotChannel> OneshotSyncEntry<C> {
  pub fn boxed(library: &'static str) -> Box<dyn Bench> {
    Box::new(OneshotSyncEntry::<C> {
      library,
      _channel: PhantomData,
    })
  }
}

impl<C: OneshotChannel> Bench for OneshotSyncEntry<C> {
  fn library(&self) -> &'static str {
    self.library
  }

  fn flavor(&self) -> Flavor {
    Flavor::Oneshot
  }

  fn mode(&self) -> Mode {
    Mode::Sync
  }

  fn batch_support(&self) -> BatchSupport {
    BatchSupport::None
  }

  fn calibrate(&self, _rt: &Runtime, cell: &Cell, budget: &Budget) -> Result<u64, RunError> {
    calibrate(budget, cell, |items| run_oneshot_sync::<C>(cell, items))
  }

  fn run_once(&self, _rt: &Runtime, cell: &Cell, items: u64) -> RunResult {
    run_oneshot_sync::<C>(cell, items)
  }
}

pub struct OneshotAsyncEntry<C: AsyncOneshotChannel> {
  library: &'static str,
  _channel: PhantomData<fn() -> C>,
}

impl<C: AsyncOneshotChannel> OneshotAsyncEntry<C> {
  pub fn boxed(library: &'static str) -> Box<dyn Bench> {
    Box::new(OneshotAsyncEntry::<C> {
      library,
      _channel: PhantomData,
    })
  }
}

impl<C: AsyncOneshotChannel> Bench for OneshotAsyncEntry<C> {
  fn library(&self) -> &'static str {
    self.library
  }

  fn flavor(&self) -> Flavor {
    Flavor::Oneshot
  }

  fn mode(&self) -> Mode {
    Mode::Async
  }

  fn batch_support(&self) -> BatchSupport {
    BatchSupport::None
  }

  fn calibrate(&self, rt: &Runtime, cell: &Cell, budget: &Budget) -> Result<u64, RunError> {
    calibrate(budget, cell, |items| run_oneshot_async::<C>(rt, cell, items))
  }

  fn run_once(&self, rt: &Runtime, cell: &Cell, items: u64) -> RunResult {
    run_oneshot_async::<C>(rt, cell, items)
  }
}
