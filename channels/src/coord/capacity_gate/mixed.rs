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
use std::marker::PhantomPinned;
use std::pin::Pin;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll, Waker};
use std::thread::{self, Thread};

use parking_lot::Mutex;

pub(crate) const STATE_WAITING: u8 = 0;
pub(crate) const STATE_SUCCESS: u8 = 1;
pub(crate) const STATE_CANCELLED: u8 = 2;

/// An enum representing either a sync or async waiter.
#[derive(Debug)]
enum Waiter {
  Sync(Thread),
  Async {
    waker: Waker,
    state: *const AtomicU8,
  },
}

// Safety: the raw pointer points into a pinned AcquireFuture; its Drop impl
// removes the entry before the future is destroyed, so the pointer is valid
// for the lifetime of the Waiter in the queue.
unsafe impl Send for Waiter {}

impl Waiter {
  fn wake(self) {
    match self {
      Waiter::Sync(thread) => thread.unpark(),
      Waiter::Async { waker, .. } => waker.wake(),
    }
  }

  fn will_wake(&self, waker: &Waker) -> bool {
    match self {
      Waiter::Async { waker: self_waker, .. } => self_waker.will_wake(waker),
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
  /// Set to true when the gate is closed; subsequent acquisitions return immediately.
  is_closed: bool,
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
        is_closed: false,
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
      if internal.is_closed {
        return;
      }
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
    AcquireFuture {
      gate: self,
      state: AtomicU8::new(STATE_WAITING),
      is_registered: false,
      _phantom: PhantomPinned,
    }
  }

  /// Attempts to acquire a permit without blocking.
  ///
  /// The key to preventing deadlock: a permit can only be taken if no one
  /// is waiting. This gives waiters priority and prevents permit stealing.
  pub fn try_acquire(&self) -> bool {
    let mut internal = self.internal.lock();
    if internal.is_closed {
      return true;
    }
    if internal.waiters.is_empty() && internal.permits > 0 {
      internal.permits -= 1;
      true
    } else {
      false
    }
  }

  /// Closes the gate and unparks all waiting producers.
  ///
  /// Called when the consumer drops. Wakes every waiter in a single O(N) pass
  /// so they can observe the closed state and return an error, rather than
  /// relying on a fragile per-permit daisy-chain.
  pub fn close(&self) {
    let mut internal = self.internal.lock();
    if !internal.is_closed {
      internal.is_closed = true;
      while let Some(waiter) = internal.waiters.pop_front() {
        waiter.wake();
      }
    }
  }

  /// Releases a permit back to the gate.
  pub fn release(&self) {
    let mut internal = self.internal.lock();
    internal.permits += 1;

    while let Some(waiter) = internal.waiters.pop_front() {
      match waiter {
        Waiter::Sync(thread) => {
          thread.unpark();
          return;
        }
        Waiter::Async { waker, state } => {
          let state_ref = unsafe { &*state };
          if state_ref
            .compare_exchange(
              STATE_WAITING,
              STATE_SUCCESS,
              Ordering::SeqCst,
              Ordering::SeqCst,
            )
            .is_ok()
          {
            waker.wake();
            return;
          }
          // STATE_CANCELLED: discard this waiter and try the next one.
        }
      }
    }

    internal.permits = internal.permits.min(self.capacity);
  }
}

/// A future that resolves when a permit is acquired from the `CapacityGate`.
#[must_use = "futures do nothing unless you .await or poll them"]
pub struct AcquireFuture<'a> {
  gate: &'a CapacityGate,
  state: AtomicU8,
  is_registered: bool,
  _phantom: PhantomPinned,
}

impl<'a> Future for AcquireFuture<'a> {
  type Output = ();

  fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    let this = unsafe { self.as_mut().get_unchecked_mut() };

    // Fast path: release() already claimed a permit on our behalf.
    if this.state.load(Ordering::Acquire) == STATE_SUCCESS {
      this.is_registered = false;
      return Poll::Ready(());
    }

    let mut internal = this.gate.internal.lock();

    if internal.is_closed {
      this.is_registered = false;
      return Poll::Ready(());
    }

    // Re-check under lock in case release() raced with our fast-path check.
    if this.state.load(Ordering::Acquire) == STATE_SUCCESS {
      this.is_registered = false;
      return Poll::Ready(());
    }

    if internal.waiters.is_empty() && internal.permits > 0 {
      internal.permits -= 1;
      this.is_registered = false;
      return Poll::Ready(());
    }

    if internal.permits > 0 {
      internal.permits -= 1;
      this.is_registered = false;
      return Poll::Ready(());
    }

    let new_waker = cx.waker();
    let state_ptr = &this.state as *const AtomicU8;

    // Update waker in-place if already registered (waker may have changed).
    let mut found = false;
    for waiter in internal.waiters.iter_mut() {
      if let Waiter::Async { state, waker: ref mut existing_waker } = waiter {
        if *state == state_ptr {
          *existing_waker = new_waker.clone();
          found = true;
          break;
        }
      }
    }

    if !found {
      this.is_registered = true;
      this.state.store(STATE_WAITING, Ordering::SeqCst);
      internal.waiters.push_back(Waiter::Async {
        waker: new_waker.clone(),
        state: state_ptr,
      });
    }

    Poll::Pending
  }
}

impl<'a> Drop for AcquireFuture<'a> {
  fn drop(&mut self) {
    if self.is_registered
      && self
        .state
        .compare_exchange(STATE_WAITING, STATE_CANCELLED, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
    {
      let mut internal = self.gate.internal.lock();
      let state_ptr = &self.state as *const AtomicU8;
      internal.waiters.retain(|w| match w {
        Waiter::Async { state, .. } => *state != state_ptr,
        _ => true,
      });
    }
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

  #[cfg(not(miri))]
  #[tokio::test]
  async fn acquire_async_waits_and_completes() {
    use tokio::time::timeout;

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

  #[cfg(not(miri))]
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

  #[test]
  fn test_acquire_async_drop_leak() {
    let gate = CapacityGate::new(1);

    // 1. Consume the only available permit so subsequent async acquires must wait
    assert!(gate.try_acquire());

    // 2. Create a self-contained dummy waker for the context
    fn dummy_waker() -> Waker {
      use std::task::{RawWaker, RawWakerVTable};
      unsafe fn clone(_: *const ()) -> RawWaker {
        RawWaker::new(std::ptr::null(), &VTABLE)
      }
      unsafe fn wake(_: *const ()) {}
      unsafe fn wake_by_ref(_: *const ()) {}
      unsafe fn drop_raw(_: *const ()) {}
      static VTABLE: RawWakerVTable = RawWakerVTable::new(clone, wake, wake_by_ref, drop_raw);
      unsafe { Waker::from_raw(RawWaker::new(std::ptr::null(), &VTABLE)) }
    }

    let waker = dummy_waker();
    let mut cx = Context::from_waker(&waker);

    // 3. Create the AcquireFuture and poll it once.
    // Since no permits are available, it will return Poll::Pending and register our waker.
    let mut fut = Box::pin(gate.acquire_async());
    assert!(fut.as_mut().poll(&mut cx).is_pending());

    // Verify that the waker was indeed registered in the internal queue
    {
      let internal = gate.internal.lock();
      assert_eq!(internal.waiters.len(), 1);
    }

    // 4. Drop (cancel) the future before it ever resolves to success
    drop(fut);

    // 5. Inspect the queue.
    // Under the unpatched code, this assertion will fail because the stale waker is still there.
    let leaked_count = {
      let internal = gate.internal.lock();
      internal.waiters.len()
    };

    assert_eq!(
      leaked_count, 0,
      "Waker leak detected: dropping AcquireFuture left a stale waker in the waiters queue!"
    );
  }
}
