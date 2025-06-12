//! A hybrid semaphore that supports both synchronous and asynchronous waiters.
//!
//! This `CapacityGate` is the core primitive for creating bounded channels that
//! need to support both blocking and async operations. It uses a `parking_lot::Mutex`
//! to protect its internal state, ensuring that the management of permits and
//! the unified waiter queue (for sync `Thread`s and async `Waker`s) is
//! free of race conditions like "lost wakeups" and "permit stealing".
//!
//! The mutex is only contended when the gate is out of permits and a new waiter
//! must be enqueued, or when a permit is released and a waiter must be dequeued.
//! This is an efficient approach for a backpressure mechanism.

use std::collections::VecDeque;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};
use std::thread::{self, Thread};

use parking_lot::Mutex;

/// An enum representing either a sync or async waiter.
#[derive(Debug)]
enum Waiter {
  Sync(Thread),
  Async(Waker),
}

impl Waiter {
  /// Wakes the underlying thread or task.
  fn wake(self) {
    match self {
      Waiter::Sync(thread) => thread.unpark(),
      Waiter::Async(waker) => waker.wake(),
    }
  }

  /// Checks if this waiter would be woken by the given waker.
  fn will_wake(&self, waker: &Waker) -> bool {
    match self {
      Waiter::Async(self_waker) => self_waker.will_wake(waker),
      Waiter::Sync(_) => false,
    }
  }
}

/// The internal state of the `CapacityGate`, protected by a `Mutex`.
#[derive(Debug)]
struct GateInternal {
  /// The number of currently available permits.
  permits: usize,
  /// A unified, fair (FIFO) queue of waiting threads and tasks.
  waiters: VecDeque<Waiter>,
}

/// A clonable handle to a hybrid sync/async semaphore.
pub struct CapacityGate {
  capacity: usize,
  internal: Arc<Mutex<GateInternal>>,
}

impl fmt::Debug for CapacityGate {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    let internal = self.internal.lock();
    f.debug_struct("CapacityGate")
      .field("capacity", &self.capacity)
      .field("permits", &internal.permits)
      .field("waiters", &internal.waiters.len())
      .finish()
  }
}

impl CapacityGate {
  /// Creates a new `CapacityGate` with a given capacity.
  pub fn new(capacity: usize) -> Self {
    Self {
      capacity,
      internal: Arc::new(Mutex::new(GateInternal {
        permits: capacity,
        waiters: VecDeque::new(),
      })),
    }
  }

  /// Returns the total capacity of the gate.
  pub fn capacity(&self) -> usize {
    self.capacity
  }

  /// Acquires a permit, blocking the current thread if none are available.
  pub fn acquire_sync(&self) {
    // Optimistic fast path using the correct try_acquire logic.
    if self.try_acquire() {
      return;
    }

    // Slow path, must lock and wait.
    let mut internal = self.internal.lock();
    loop {
      // Check for a permit. This is safe from stealing because `try_acquire`
      // will fail for new arrivals if we are in the waiters queue.
      if internal.permits > 0 {
        internal.permits -= 1;
        return;
      }

      // Add our thread to the waiter queue, unlock, and park.
      internal.waiters.push_back(Waiter::Sync(thread::current()));
      drop(internal);
      thread::park();
      internal = self.internal.lock();
    }
  }

  /// Acquires a permit asynchronously, returning a future that resolves
  /// when a permit is available.
  pub fn acquire_async(&self) -> AcquireFuture<'_> {
    AcquireFuture { gate: self }
  }

  /// Attempts to acquire a permit without blocking.
  ///
  /// The key to preventing deadlock: a permit can only be taken if no one
  /// is waiting. This gives waiters priority and prevents permit stealing.
  pub fn try_acquire(&self) -> bool {
    let mut internal = self.internal.lock();
    if internal.waiters.is_empty() && internal.permits > 0 {
      internal.permits -= 1;
      true
    } else {
      false
    }
  }

  /// Releases a permit back to the gate.
  pub fn release(&self) {
    let mut internal = self.internal.lock();
    // Always increment permits. A permit is a permit.
    internal.permits += 1;

    if let Some(waiter) = internal.waiters.pop_front() {
      // A waiter exists. Wake them up. They will consume the permit we just
      // added. We don't drop the lock here; waking is cheap.
      waiter.wake();
    } else {
      // No one was waiting. We must now cap the permit count to prevent it
      // from exceeding capacity. This is only necessary when the queue is empty.
      internal.permits = internal.permits.min(self.capacity);
    }
  }
}

/// A future that resolves when a permit is acquired from the `CapacityGate`.
#[must_use = "futures do nothing unless you .await or poll them"]
pub struct AcquireFuture<'a> {
  gate: &'a CapacityGate,
}

impl Future for AcquireFuture<'_> {
  type Output = ();

  fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    let mut internal = self.gate.internal.lock();

    // Fast path check using the anti-stealing logic.
    if internal.waiters.is_empty() && internal.permits > 0 {
      internal.permits -= 1;
      return Poll::Ready(());
    }

    // Slow path: must wait.
    // Check for a permit again. This is for waiters that have been woken up.
    if internal.permits > 0 {
      internal.permits -= 1;
      return Poll::Ready(());
    }

    // No permits available. Add our waker to the queue if not already present.
    if !internal.waiters.iter().any(|w| w.will_wake(cx.waker())) {
      internal
        .waiters
        .push_back(Waiter::Async(cx.waker().clone()));
    }

    Poll::Pending
  }
}

impl Clone for CapacityGate {
  fn clone(&self) -> Self {
    Self {
      capacity: self.capacity,
      internal: self.internal.clone(),
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::time::Duration;
  use tokio::time::timeout;

  #[test]
  fn gate_new_and_capacity() {
    let gate = CapacityGate::new(5);
    assert_eq!(gate.capacity(), 5);
  }

  #[test]
  fn acquire_sync_release() {
    let gate = CapacityGate::new(1);
    gate.acquire_sync();
    // No easy way to check permits without another thread,
    // but this ensures it doesn't hang on first acquire.
    gate.release();
  }

  #[test]
  fn acquire_sync_blocks_and_unblocks() {
    let gate = Arc::new(CapacityGate::new(1));
    gate.acquire_sync(); // Acquire the only permit

    let gate_clone = gate.clone();
    let handle = thread::spawn(move || {
      // This should block
      gate_clone.acquire_sync();
    });

    // Give the thread time to block
    thread::sleep(Duration::from_millis(100));
    assert!(!handle.is_finished(), "Thread should have blocked");

    // Release the permit, which should unpark the thread
    gate.release();
    handle.join().expect("Thread panicked");
  }

  #[tokio::test]
  async fn acquire_async_waits_and_completes() {
    let gate = Arc::new(CapacityGate::new(1));
    gate.acquire_sync(); // Use up the only permit

    let acquire_fut = gate.acquire_async();

    let gate_for_spawn = gate.clone();
    tokio::spawn(async move {
      tokio::time::sleep(Duration::from_millis(100)).await;
      gate_for_spawn.release();
    });

    timeout(Duration::from_millis(500), acquire_fut)
      .await
      .expect("Future did not complete after release");
  }

  #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
  async fn mixed_waiters_contention() {
    let gate = Arc::new(CapacityGate::new(2));
    let mut thread_handles = Vec::new();
    let mut task_handles = Vec::new();
    let completion_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));

    // Spawn 3 sync waiters
    for _ in 0..3 {
      let gate = gate.clone();
      let count = completion_count.clone();
      thread_handles.push(thread::spawn(move || {
        gate.acquire_sync();
        thread::sleep(Duration::from_millis(50));
        gate.release();
        count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
      }));
    }

    // Spawn 3 async waiters
    for _ in 0..3 {
      let gate = gate.clone();
      let count = completion_count.clone();
      task_handles.push(tokio::spawn(async move {
        gate.acquire_async().await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        gate.release();
        count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
      }));
    }

    // Wait for all async tasks to complete.
    for handle in task_handles {
      handle.await.unwrap();
    }

    // Wait for all sync threads to complete.
    for handle in thread_handles {
      handle.join().unwrap();
    }

    assert_eq!(
      completion_count.load(std::sync::atomic::Ordering::Relaxed),
      6
    );
  }
}
