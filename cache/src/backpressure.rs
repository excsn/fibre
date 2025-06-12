use parking_lot::Mutex;
use std::collections::VecDeque;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};
use std::thread::{self, Thread};

/// Represents a waiter in the queue, which can be either a synchronous
/// thread or an asynchronous waker.
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
  /// Used to prevent duplicate wakers for the same task in the queue.
  fn will_wake(&self, waker: &Waker) -> bool {
    match self {
      Waiter::Async(self_waker) => self_waker.will_wake(waker),
      Waiter::Sync(_) => false,
    }
  }
}

/// Internal state of the `CostGate`, protected by a `Mutex`.
struct GateInternal {
  /// The number of currently available permits (i.e., remaining capacity).
  permits: u64,
  /// A FIFO queue of waiting threads and tasks that want to acquire permits.
  waiters: VecDeque<(u64, Waiter)>, // (cost_needed, waiter)
}

/// A weighted, hybrid sync/async semaphore for managing cache capacity.
///
/// It allows `insert` operations to block or await until enough `cost` has
/// been freed up by evictions.
pub(crate) struct CostGate {
  internal: Arc<Mutex<GateInternal>>,
}

impl fmt::Debug for CostGate {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    let internal = self.internal.lock();
    f.debug_struct("CostGate")
      .field("permits", &internal.permits)
      .field("waiters", &internal.waiters.len())
      .finish()
  }
}

impl CostGate {
  /// Creates a new `CostGate` with a given initial capacity (number of permits).
  pub(crate) fn new(capacity: u64) -> Self {
    Self {
      internal: Arc::new(Mutex::new(GateInternal {
        permits: capacity,
        waiters: VecDeque::new(),
      })),
    }
  }

  /// Acquires `cost` permits, blocking the current thread if not enough are
  /// available.
  pub(crate) fn acquire_sync(&self, cost: u64) {
    let mut internal = self.internal.lock();
    loop {
      // If we have enough permits, take them and return.
      if internal.permits >= cost {
        internal.permits -= cost;
        return;
      }

      // Not enough permits. Add ourselves to the waiter queue, unlock, and park.
      internal
        .waiters
        .push_back((cost, Waiter::Sync(thread::current())));
      drop(internal); // Unlock before parking
      thread::park();
      internal = self.internal.lock(); // Re-lock after being unparked
    }
  }

  /// Acquires `cost` permits asynchronously.
  pub(crate) fn acquire_async(&self, cost: u64) -> AcquireFuture<'_> {
    AcquireFuture { gate: self, cost }
  }

  /// Releases `cost` permits back to the gate, potentially waking waiters.
  pub(crate) fn release(&self, cost: u64) {
    let mut internal = self.internal.lock();
    internal.permits += cost;

    // Wake up any waiters that can now be satisfied.
    while let Some((needed, _)) = internal.waiters.front() {
      if internal.permits >= *needed {
        // This waiter can be satisfied.
        let (needed, waiter) = internal.waiters.pop_front().unwrap();
        internal.permits -= needed;
        waiter.wake();
      } else {
        // The front waiter needs more permits than we have, so stop.
        break;
      }
    }
  }
}

/// A future that resolves when `cost` permits have been acquired.
#[must_use = "futures do nothing unless you .await or poll them"]
pub(crate) struct AcquireFuture<'a> {
  gate: &'a CostGate,
  cost: u64,
}

impl<'a> Future for AcquireFuture<'a> {
  type Output = ();

  fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    let mut internal = self.gate.internal.lock();

    // Check if we can acquire the permits now.
    if internal.permits >= self.cost {
      internal.permits -= self.cost;
      return Poll::Ready(());
    }

    // Not enough permits. Add our waker to the queue if not already present.
    if !internal
      .waiters
      .iter()
      .any(|(_, w)| w.will_wake(cx.waker()))
    {
      internal
        .waiters
        .push_back((self.cost, Waiter::Async(cx.waker().clone())));
    }

    Poll::Pending
  }
}
