use parking_lot::{RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::collections::VecDeque;
use std::future::Future;
use std::ops::{Deref, DerefMut};
use std::pin::Pin;
use std::task::{Context, Poll, Waker};

/// A hybrid reader-writer lock that uses a fast, blocking `parking_lot::RwLock`
/// internally but provides a non-thread-blocking `async` interface for acquiring
/// a write lock under contention.
#[derive(Debug, Default)]
pub(crate) struct HybridRwLock<T> {
  inner: RwLock<T>,
  writer_waiters: parking_lot::Mutex<VecDeque<Waker>>,
}

impl<T> HybridRwLock<T> {
  pub fn new(data: T) -> Self {
    Self {
      inner: RwLock::new(data),
      writer_waiters: parking_lot::Mutex::new(VecDeque::new()),
    }
  }
  
  /// Acquires a write lock synchronously (blocking).
  ///
  /// Returns our custom hybrid guard.
  pub fn read(&self) -> RwLockReadGuard<'_, T> {
    self.inner.read()
  }

  pub fn write_sync(&self) -> HybridRwLockWriteGuard<'_, T> {
    HybridRwLockWriteGuard {
      waiters: &self.writer_waiters,
      // The guard is created by the blocking call here.
      guard: self.inner.write(),
    }
  }

  /// Acquires a write lock asynchronously.
  ///
  /// This will not block the OS thread. If the lock is contended, the future
  /// will yield until it is woken up.
  pub fn write_async(&self) -> WriteFuture<'_, T> {
    WriteFuture { lock: self }
  }
}

/// A custom RAII guard for our hybrid write lock.
/// Its `Drop` implementation is crucial for waking up the next pending task.
#[derive(Debug)]
pub(crate) struct HybridRwLockWriteGuard<'a, T> {
  // We hold a reference to the waiters queue for the Drop impl.
  waiters: &'a parking_lot::Mutex<VecDeque<Waker>>,
  // The actual guard from the inner lock.
  guard: RwLockWriteGuard<'a, T>,
}

impl<'a, T> Drop for HybridRwLockWriteGuard<'a, T> {
  fn drop(&mut self) {
    // The `RwLockWriteGuard` is dropped first, releasing the lock.

    // Now, check if there are any waiting async tasks and wake one up.
    let mut waiters_guard = self.waiters.lock();
    if let Some(waker) = waiters_guard.pop_front() {
      waker.wake();
    }
  }
}

// Allow the guard to be used like a normal `&mut T`.
impl<'a, T> Deref for HybridRwLockWriteGuard<'a, T> {
  type Target = T;
  fn deref(&self) -> &Self::Target {
    &self.guard
  }
}

impl<'a, T> DerefMut for HybridRwLockWriteGuard<'a, T> {
  fn deref_mut(&mut self) -> &mut Self::Target {
    &mut self.guard
  }
}

/// The `Future` returned by `HybridRwLock::write()`.
#[must_use = "futures do nothing unless you .await or poll them"]
pub(crate) struct WriteFuture<'a, T> {
  lock: &'a HybridRwLock<T>,
}

impl<'a, T> Future for WriteFuture<'a, T> {
  type Output = HybridRwLockWriteGuard<'a, T>;

  fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    // Fast path: Try to acquire the lock without parking.
    if let Some(guard) = self.lock.inner.try_write() {
      return Poll::Ready(HybridRwLockWriteGuard {
        waiters: &self.lock.writer_waiters,
        guard,
      });
    }

    // Slow path: The lock is contended. Park this task.
    let mut waiters_guard = self.lock.writer_waiters.lock();

    // It's possible the lock was released between the `try_write` call and
    // us acquiring the waiters lock. We must re-check here to avoid a
    // lost wakeup.
    if let Some(guard) = self.lock.inner.try_write() {
      return Poll::Ready(HybridRwLockWriteGuard {
        waiters: &self.lock.writer_waiters,
        guard,
      });
    }

    // Still contended. Push our waker to the queue if it's not already there.
    if !waiters_guard.iter().any(|w| w.will_wake(cx.waker())) {
      waiters_guard.push_back(cx.waker().clone());
    }

    Poll::Pending
  }
}
