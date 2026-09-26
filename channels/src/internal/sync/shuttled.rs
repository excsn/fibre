//! Shuttle-build primitives: shuttle's modeled types, shimmed where their API
//! differs from what the channels use. Keep the export list in lockstep with
//! `real.rs`. Shuttle treats every atomic as sequentially consistent and does
//! not model time: `sleep` is a context switch and `park_timeout` is `park`.

pub(crate) use shuttle::sync::atomic::{
  fence, AtomicBool, AtomicPtr, AtomicU8, AtomicU32, AtomicU64, AtomicUsize, Ordering,
};

pub(crate) use shuttle::hint;

/// Collapses spin budgets to 1, as under loom: every spin is a scheduling
/// point, and a 200-yield budget buries the interleavings that matter under
/// hundreds of equivalent ones.
pub(crate) const IS_LOOM: bool = true;

pub(crate) use std::sync::Arc;

pub(crate) use shuttle::thread;
pub(crate) use shuttle::thread::Thread;

/// Shuttle-modeled stand-in for `futures_util::task::AtomicWaker`, backed by a
/// shuttle Mutex: it checks the caller's protocol, not AtomicWaker's own.
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

/// Shuttle `Mutex` wearing parking_lot's API: guard-returning `lock`,
/// `Option`-returning `try_lock`, no poison `Result`s.
#[derive(Debug)]
pub(crate) struct Mutex<T>(shuttle::sync::Mutex<T>);

impl<T> Mutex<T> {
  #[inline]
  pub(crate) fn new(value: T) -> Self {
    Self(shuttle::sync::Mutex::new(value))
  }

  #[inline]
  pub(crate) fn lock(&self) -> shuttle::sync::MutexGuard<'_, T> {
    // Poison means a panic already happened inside the test - propagate it.
    self.0.lock().unwrap()
  }

  #[inline]
  pub(crate) fn try_lock(&self) -> Option<shuttle::sync::MutexGuard<'_, T>> {
    self.0.try_lock().ok()
  }

  #[inline]
  pub(crate) fn get_mut(&mut self) -> &mut T {
    self.0.get_mut().unwrap()
  }
}
