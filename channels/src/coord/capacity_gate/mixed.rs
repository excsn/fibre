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
      Waiter::Async {
        waker: self_waker, ..
      } => self_waker.will_wake(waker),
      Waiter::Sync(_) => false,
    }
  }
}

/// The internal state of the `CapacityGate`, protected by a `Mutex`.
#[derive(Debug)]
struct GateInternal {
  /// The number of currently available permits.
  permits: usize,
  /// Permits committed to woken *sync* waiters that have not yet re-acquired
  /// the lock to claim them. Kept out of `permits` so the capacity clamp in
  /// `release()` can never discard an in-flight handoff — for a capacity-0
  /// (rendezvous) gate that steal would re-park the woken sender forever.
  reserved: usize,
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
      .field("reserved", &internal.reserved)
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
        reserved: 0,
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
    let mut was_parked = false;
    loop {
      if internal.is_closed {
        return;
      }
      // A permit reserved by release() for a woken waiter. Only claim it
      // after we have actually parked, so arriving threads cannot jump the
      // queue ahead of the waiter the permit was committed to.
      if was_parked && internal.reserved > 0 {
        internal.reserved -= 1;
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
      was_parked = true;
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
          // Move the permit out of the general pool and reserve it for the
          // woken thread. If it stayed in the pool, a concurrent release()
          // hitting the empty-queue clamp below could discard it before the
          // thread wakes up — a permanent livelock on capacity-0 gates.
          internal.permits -= 1;
          internal.reserved += 1;
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
            // The woken future's STATE_SUCCESS fast path returns Ready
            // without touching the pool, so the permit is consumed here.
            // (This also stops the pool inflating past capacity on every
            // async wake.)
            internal.permits -= 1;
            waker.wake();
            return;
          }
          // STATE_CANCELLED: discard this waiter and try the next one.
        }
      }
    }

    internal.permits = internal.permits.min(self.capacity);
  }

  /// Attempts to acquire up to `n` permits without blocking.
  ///
  /// Returns the number of permits acquired (`0..=n`). Returns `0` if any
  /// waiters are queued (preserving the no-stealing rule of `try_acquire`)
  /// or if no permits are available. Returns `n` immediately if the gate is
  /// closed, matching `try_acquire`'s closed-returns-true convention so the
  /// caller can observe the closed state and bail out.
  pub fn try_acquire_many(&self, n: usize) -> usize {
    if n == 0 {
      return 0;
    }
    let mut internal = self.internal.lock();
    if internal.is_closed {
      return n;
    }
    if internal.waiters.is_empty() && internal.permits > 0 {
      let k = internal.permits.min(n);
      internal.permits -= k;
      k
    } else {
      0
    }
  }

  /// Acquires between 1 and `max` permits, blocking the current thread until
  /// at least one permit is available.
  ///
  /// Returns the number of permits acquired. If the gate is closed (or closes
  /// while waiting), returns `max` immediately so the caller can observe the
  /// closed state and bail out, matching `acquire_sync`'s behavior.
  pub fn acquire_many_sync(&self, max: usize) -> usize {
    if max == 0 {
      return 0;
    }
    // Optimistic fast path.
    let k = self.try_acquire_many(max);
    if k > 0 {
      return k;
    }

    // Slow path, must lock and wait.
    let mut internal = self.internal.lock();
    let mut was_parked = false;
    loop {
      if internal.is_closed {
        return max;
      }
      // A permit reserved for a woken waiter (see `acquire_sync`); claim it
      // plus any extra pool permits up to `max`.
      if was_parked && internal.reserved > 0 {
        internal.reserved -= 1;
        let extra = internal.permits.min(max - 1);
        internal.permits -= extra;
        return 1 + extra;
      }
      // Safe from stealing for the same reason as `acquire_sync`: new
      // arrivals fail `try_acquire_many` while we sit in the waiter queue.
      if internal.permits > 0 {
        let k = internal.permits.min(max);
        internal.permits -= k;
        return k;
      }

      internal.waiters.push_back(Waiter::Sync(thread::current()));
      drop(internal);
      thread::park();
      was_parked = true;
      internal = self.internal.lock();
    }
  }

  /// Acquires between 1 and `max` permits asynchronously.
  ///
  /// The returned future resolves with the number of permits acquired
  /// (or `max` if the gate is closed).
  pub fn acquire_many_async(&self, max: usize) -> AcquireManyFuture<'_> {
    AcquireManyFuture {
      gate: self,
      max,
      state: AtomicU8::new(STATE_WAITING),
      is_registered: false,
      _phantom: PhantomPinned,
    }
  }

  /// Releases `n` permits back to the gate in a single lock acquisition,
  /// waking up to `n` waiters on the stack in one coalesced pass.
  pub fn release_many(&self, n: usize) {
    if n == 0 {
      return;
    }
    const DUMMY: Option<Waiter> = None;
    let mut inline_wake = [DUMMY; 8]; // lightweight register-friendly size
    let mut inline_count = 0;
    let mut heap_wake = Vec::new(); // Fallback for very large bulk releases

    {
      let mut internal = self.internal.lock();
      internal.permits += n;

      while inline_count + heap_wake.len() < n {
        match internal.waiters.pop_front() {
          None => break,
          Some(Waiter::Sync(thread)) => {
            internal.permits -= 1;
            internal.reserved += 1;
            let waiter = Waiter::Sync(thread);
            if inline_count < 8 {
              inline_wake[inline_count] = Some(waiter);
              inline_count += 1;
            } else {
              heap_wake.push(waiter);
            }
          }
          Some(Waiter::Async { waker, state }) => {
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
              internal.permits -= 1;
              let waiter = Waiter::Async { waker, state };
              if inline_count < 8 {
                inline_wake[inline_count] = Some(waiter);
                inline_count += 1;
              } else {
                heap_wake.push(waiter);
              }
            }
          }
        }
      }

      internal.permits = internal.permits.min(self.capacity);
    }

    // Wake outside the lock to prevent immediate contention
    for opt in &mut inline_wake[..inline_count] {
      if let Some(waiter) = opt.take() {
        waiter.wake();
      }
    }
    for waiter in heap_wake {
      waiter.wake();
    }
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
    let state_ptr = &this.state as *const AtomicU8;

    if internal.is_closed {
      this.is_registered = false;
      return Poll::Ready(());
    }

    // Re-check under lock in case release() raced with our fast-path check.
    if this.state.load(Ordering::Acquire) == STATE_SUCCESS {
      this.is_registered = false;
      return Poll::Ready(());
    }

    if internal.permits > 0 {
      internal.permits -= 1;
      // If we were registered, our waiter entry is still queued (we claimed
      // a pool permit instead of being woken). Unlink it so no stale state
      // pointer survives this future.
      if this.is_registered {
        internal.waiters.retain(|w| match w {
          Waiter::Async { state, .. } => *state != state_ptr,
          _ => true,
        });
        this.is_registered = false;
      }
      return Poll::Ready(());
    }

    let new_waker = cx.waker();

    // Update waker in-place if already registered (waker may have changed).
    let mut found = false;
    for waiter in internal.waiters.iter_mut() {
      if let Waiter::Async {
        state,
        waker: existing_waker,
      } = waiter
      {
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
        .compare_exchange(
          STATE_WAITING,
          STATE_CANCELLED,
          Ordering::SeqCst,
          Ordering::SeqCst,
        )
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

/// A future that resolves with the number of permits (1..=max) acquired from
/// the `CapacityGate`, or `max` if the gate is closed.
#[must_use = "futures do nothing unless you .await or poll them"]
pub struct AcquireManyFuture<'a> {
  gate: &'a CapacityGate,
  max: usize,
  state: AtomicU8,
  is_registered: bool,
  _phantom: PhantomPinned,
}

impl<'a> Future for AcquireManyFuture<'a> {
  type Output = usize;

  fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    let this = unsafe { self.as_mut().get_unchecked_mut() };

    if this.max == 0 {
      this.is_registered = false;
      return Poll::Ready(0);
    }

    // Fast path: release()/release_many() consumed one permit from the pool
    // on our behalf when it flagged us STATE_SUCCESS. Grab up to `max - 1`
    // extra permits under the lock.
    if this.state.load(Ordering::Acquire) == STATE_SUCCESS {
      let mut internal = this.gate.internal.lock();
      let extra = internal.permits.min(this.max - 1);
      internal.permits -= extra;
      this.is_registered = false;
      return Poll::Ready(1 + extra);
    }

    let mut internal = this.gate.internal.lock();
    let state_ptr = &this.state as *const AtomicU8;

    if internal.is_closed {
      this.is_registered = false;
      return Poll::Ready(this.max);
    }

    // Re-check under lock in case a release raced with our fast-path check.
    if this.state.load(Ordering::Acquire) == STATE_SUCCESS {
      let extra = internal.permits.min(this.max - 1);
      internal.permits -= extra;
      this.is_registered = false;
      return Poll::Ready(1 + extra);
    }

    // Mirrors the permit-taking branch of `AcquireFuture::poll`.
    if internal.permits > 0 {
      let k = internal.permits.min(this.max);
      internal.permits -= k;
      // Unlink a still-queued waiter entry so no stale state pointer
      // survives this future (see `AcquireFuture::poll`).
      if this.is_registered {
        internal.waiters.retain(|w| match w {
          Waiter::Async { state, .. } => *state != state_ptr,
          _ => true,
        });
        this.is_registered = false;
      }
      return Poll::Ready(k);
    }

    let new_waker = cx.waker();

    // Update waker in-place if already registered (waker may have changed).
    let mut found = false;
    for waiter in internal.waiters.iter_mut() {
      if let Waiter::Async {
        state,
        waker: existing_waker,
      } = waiter
      {
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

impl<'a> Drop for AcquireManyFuture<'a> {
  fn drop(&mut self) {
    if self.is_registered
      && self
        .state
        .compare_exchange(
          STATE_WAITING,
          STATE_CANCELLED,
          Ordering::SeqCst,
          Ordering::SeqCst,
        )
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
    gate.release();
  }

  #[test]
  fn acquire_sync_blocks_and_unblocks() {
    let gate = Arc::new(CapacityGate::new(1));
    gate.acquire_sync();

    let gate_clone = gate.clone();
    let handle = thread::spawn(move || {
      gate_clone.acquire_sync();
    });

    thread::sleep(Duration::from_millis(100));
    assert!(!handle.is_finished(), "Thread should have blocked");

    gate.release();
    handle.join().expect("Thread panicked");
  }

  #[cfg(not(miri))]
  #[tokio::test]
  async fn acquire_async_waits_and_completes() {
    use tokio::time::timeout;

    let gate = Arc::new(CapacityGate::new(1));
    gate.acquire_sync();

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

    for handle in task_handles {
      handle.await.unwrap();
    }

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
    assert!(gate.try_acquire());

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

    let mut fut = Box::pin(gate.acquire_async());
    assert!(fut.as_mut().poll(&mut cx).is_pending());

    {
      let internal = gate.internal.lock();
      assert_eq!(internal.waiters.len(), 1);
    }

    drop(fut);

    let leaked_count = {
      let internal = gate.internal.lock();
      internal.waiters.len()
    };

    assert_eq!(
      leaked_count, 0,
      "Waker leak detected: dropping left a stale waker in the queue!"
    );
  }

  #[test]
  fn try_acquire_many_basic() {
    let gate = CapacityGate::new(5);
    assert_eq!(gate.try_acquire_many(0), 0);
    assert_eq!(gate.try_acquire_many(3), 3);
    assert_eq!(gate.try_acquire_many(5), 2);
    assert_eq!(gate.try_acquire_many(1), 0);
  }

  #[test]
  fn release_many_restores_and_clamps() {
    let gate = CapacityGate::new(5);
    assert_eq!(gate.try_acquire_many(5), 5);
    gate.release_many(5);
    assert_eq!(gate.try_acquire_many(5), 5);
    gate.release_many(10);
    assert_eq!(gate.try_acquire_many(10), 5);
  }

  #[test]
  fn many_apis_on_closed_gate() {
    let gate = CapacityGate::new(2);
    assert_eq!(gate.try_acquire_many(2), 2);
    gate.close();
    assert_eq!(gate.try_acquire_many(7), 7);
    assert_eq!(gate.acquire_many_sync(4), 4);
  }

  #[test]
  fn acquire_many_sync_blocks_and_gets_batch() {
    let gate = Arc::new(CapacityGate::new(4));
    assert_eq!(gate.try_acquire_many(4), 4);

    let gate_clone = gate.clone();
    let handle = thread::spawn(move || gate_clone.acquire_many_sync(3));

    thread::sleep(Duration::from_millis(100));
    assert!(!handle.is_finished(), "Thread should have blocked");

    gate.release_many(3);
    let acquired = handle.join().expect("Thread panicked");
    assert!(
      (1..=3).contains(&acquired),
      "expected 1..=3 permits, got {acquired}"
    );
  }

  #[test]
  fn release_many_wakes_multiple_sync_waiters() {
    let gate = Arc::new(CapacityGate::new(3));
    assert_eq!(gate.try_acquire_many(3), 3);

    let mut handles = Vec::new();
    for _ in 0..3 {
      let gate = gate.clone();
      handles.push(thread::spawn(move || gate.acquire_many_sync(1)));
    }
    thread::sleep(Duration::from_millis(100));

    gate.release_many(3);
    let mut total = 0;
    for h in handles {
      total += h.join().expect("waiter panicked");
    }
    assert_eq!(total, 3);
  }

  #[cfg(not(miri))]
  #[tokio::test]
  async fn acquire_many_async_waits_and_completes() {
    use tokio::time::timeout;

    let gate = Arc::new(CapacityGate::new(4));
    assert_eq!(gate.try_acquire_many(4), 4);

    let acquire_fut = gate.acquire_many_async(3);

    let gate_for_spawn = gate.clone();
    tokio::spawn(async move {
      tokio::time::sleep(Duration::from_millis(100)).await;
      gate_for_spawn.release_many(3);
    });

    let acquired = timeout(Duration::from_millis(500), acquire_fut)
      .await
      .expect("Future did not complete after release_many");
    assert!(
      (1..=3).contains(&acquired),
      "expected 1..=3 permits, got {acquired}"
    );
  }

  #[test]
  #[cfg(not(miri))]
  fn release_does_not_steal_inflight_handoff_permit_cap0() {
    let gate = Arc::new(CapacityGate::new(0));

    let gate_clone = gate.clone();
    let waiter = thread::spawn(move || gate_clone.acquire_sync());

    thread::sleep(Duration::from_millis(100));
    assert!(!waiter.is_finished(), "waiter should be parked");

    gate.release();
    gate.release();

    thread::sleep(Duration::from_millis(200));
    assert!(
      waiter.is_finished(),
      "REGRESSION: clamp stole the in-flight handoff permit"
    );
    waiter.join().unwrap();
  }

  #[test]
  #[cfg(not(miri))]
  fn release_does_not_steal_inflight_handoff_permit_cap0_many() {
    let gate = Arc::new(CapacityGate::new(0));

    let gate_clone = gate.clone();
    let waiter = thread::spawn(move || gate_clone.acquire_many_sync(4));

    thread::sleep(Duration::from_millis(100));
    assert!(!waiter.is_finished(), "waiter should be parked");

    gate.release();
    gate.release();

    thread::sleep(Duration::from_millis(200));
    assert!(
      waiter.is_finished(),
      "REGRESSION: clamp stole the in-flight handoff permit from acquire_many_sync"
    );
    let acquired = waiter.join().unwrap();
    assert!(acquired >= 1);
  }

  #[cfg(not(miri))]
  #[tokio::test]
  async fn async_wake_consumes_permit_no_pool_inflation() {
    let gate = Arc::new(CapacityGate::new(1));
    gate.acquire_sync();

    let gate_for_task = gate.clone();
    let task = tokio::spawn(async move { gate_for_task.acquire_async().await });

    tokio::time::sleep(Duration::from_millis(100)).await;

    gate.release();
    task.await.unwrap();

    {
      let internal = gate.internal.lock();
      assert_eq!(
        internal.permits, 0,
        "async wake must consume the permit, not inflate the pool"
      );
      assert_eq!(internal.reserved, 0);
    }
    assert!(!gate.try_acquire());
  }

  #[test]
  fn test_acquire_many_async_drop_leak() {
    let gate = CapacityGate::new(1);
    assert!(gate.try_acquire());

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

    let mut fut = Box::pin(gate.acquire_many_async(3));
    assert!(fut.as_mut().poll(&mut cx).is_pending());
    {
      let internal = gate.internal.lock();
      assert_eq!(internal.waiters.len(), 1);
    }

    drop(fut);

    let leaked_count = {
      let internal = gate.internal.lock();
      internal.waiters.len()
    };
    assert_eq!(
      leaked_count, 0,
      "Waker leak: dropping left a stale waker in the queue!"
    );
  }
}
