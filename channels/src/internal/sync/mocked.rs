//! Loom-build primitives: loom's modeled types, shimmed where their API
//! differs from what the channels use. Keep the export list in lockstep with
//! `real.rs`.

pub(crate) use loom::sync::atomic::{
  fence, AtomicBool, AtomicPtr, AtomicU8, AtomicU32, AtomicU64, AtomicUsize, Ordering,
};

// loom's `hint::spin_loop()` is a scheduler hint (yield-equivalent), so
// wait-for-progress spins become explorable instead of branch explosions.
pub(crate) use loom::hint;

/// See `real.rs` - collapses spin budgets inside loom models.
pub(crate) const IS_LOOM: bool = true;

pub(crate) use loom::sync::Arc;

pub(crate) use self::thread::Thread;

/// `loom::thread` plus panicking stubs for the time-based APIs loom doesn't
/// model. The stubs exist so `#[cfg(test)]` std-thread tests and timeout code
/// paths still COMPILE under `--cfg loom` (they are never run there - loom.sh
/// filters to `loom_tests`); if a loom model actually reaches one, failing
/// loudly at the call site beats degrading to an untimed park and reporting a
/// deadlock miles from the cause.
pub(crate) mod thread {
  pub use loom::thread::*;

  use std::time::Duration;

  pub fn park_timeout(_duration: Duration) {
    panic!("thread::park_timeout is not modeled under loom - keep timeout paths out of loom tests");
  }

  pub fn sleep(_duration: Duration) {
    panic!("thread::sleep is not modeled under loom - keep sleeping tests out of loom runs");
  }
}

/// Loom-modeled stand-in for `futures_util::task::AtomicWaker`, so waker-driven
/// protocols can run under `loom::future::block_on`. Ordering-stronger than the
/// real primitive (a loom Mutex instead of AtomicWaker's internal handshake):
/// it verifies the CALLER's protocol orderings, not AtomicWaker's.
pub(crate) struct AtomicWaker {
  inner: Mutex<Option<std::task::Waker>>,
}

impl AtomicWaker {
  pub(crate) fn new() -> Self {
    AtomicWaker {
      inner: Mutex::new(None),
    }
  }

  pub(crate) fn register(&self, waker: &std::task::Waker) {
    *self.inner.lock() = Some(waker.clone());
  }

  pub(crate) fn wake(&self) {
    if let Some(waker) = self.inner.lock().take() {
      waker.wake();
    }
  }

  pub(crate) fn take(&self) -> Option<std::task::Waker> {
    self.inner.lock().take()
  }
}

/// Loom `Mutex` wearing parking_lot's API: guard-returning `lock`,
/// `Option`-returning `try_lock`, no poison `Result`s.
#[derive(Debug)]
pub(crate) struct Mutex<T>(loom::sync::Mutex<T>);

impl<T> Mutex<T> {
  #[inline]
  pub(crate) fn new(value: T) -> Self {
    Self(loom::sync::Mutex::new(value))
  }

  #[inline]
  pub(crate) fn lock(&self) -> loom::sync::MutexGuard<'_, T> {
    // Loom mirrors std's poisoning API; poison here means a panic already
    // happened inside the model - propagate it.
    self.0.lock().unwrap()
  }

  #[inline]
  pub(crate) fn try_lock(&self) -> Option<loom::sync::MutexGuard<'_, T>> {
    self.0.try_lock().ok()
  }

  #[inline]
  pub(crate) fn get_mut(&mut self) -> &mut T {
    self.0.get_mut().unwrap()
  }
}
