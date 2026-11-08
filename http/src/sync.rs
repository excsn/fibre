//! The crate's threads, locks and atomics. Under `RUSTFLAGS="--cfg shuttle"` they are shuttle's modelled types, so the shuttle models in `conn::model` can explore their interleavings. This module and the model transport are the only `cfg(shuttle)` sites.

#[cfg(not(shuttle))]
mod real {
  pub(crate) use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize, Ordering};
  pub(crate) use std::thread;
  pub(crate) use std::thread::Thread;

  pub(crate) use parking_lot::Mutex;
}

#[cfg(shuttle)]
mod shuttled {
  pub(crate) use shuttle::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize, Ordering};
  pub(crate) use shuttle::thread;
  pub(crate) use shuttle::thread::Thread;

  /// shuttle's `Mutex` with parking_lot's API: `lock` returns the guard, `try_lock` an `Option`.
  #[derive(Debug, Default)]
  pub(crate) struct Mutex<T>(shuttle::sync::Mutex<T>);

  impl<T> Mutex<T> {
    pub(crate) fn new(value: T) -> Self {
      Self(shuttle::sync::Mutex::new(value))
    }

    pub(crate) fn lock(&self) -> shuttle::sync::MutexGuard<'_, T> {
      self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[allow(dead_code)]
    pub(crate) fn try_lock(&self) -> Option<shuttle::sync::MutexGuard<'_, T>> {
      self.0.try_lock().ok()
    }
  }
}

#[cfg(not(shuttle))]
pub(crate) use real::*;
#[cfg(shuttle)]
pub(crate) use shuttled::*;
