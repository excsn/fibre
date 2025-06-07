use crate::async_util::AtomicWaker;
use crate::error::{RecvError, RecvErrorTimeout, SendError, TryRecvError, TrySendError};
use crate::internal::cache_padded::CachePadded;
use crate::sync_util;

use core::task::{Context, Poll};
use std::cell::UnsafeCell;
use std::fmt;
use std::marker::PhantomData;
use std::mem::MaybeUninit;
use std::sync::atomic::{self, AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread::{self, Thread};
use std::time::{Duration, Instant};

/// Internal shared state for the bounded SPSC channel, supporting both sync and async waiters.
// This struct is pub(crate) so it can be constructed directly by bounded_async.rs and in tests.
// Fields are pub(crate) for access within the fibre::spsc module.
pub struct SpscShared<T> {
  pub(crate) buffer: Box<[CachePadded<UnsafeCell<MaybeUninit<T>>>]>,
  pub(crate) capacity: usize,
  pub(crate) head: CachePadded<AtomicUsize>, // Write index (producer)
  pub(crate) tail: CachePadded<AtomicUsize>, // Read index (consumer)

  // --- Sender waiting state ---
  pub(crate) producer_parked_sync_flag: CachePadded<AtomicBool>,
  pub(crate) producer_thread_sync: CachePadded<UnsafeCell<Option<Thread>>>,
  pub(crate) producer_waker_async: CachePadded<AtomicWaker>,

  // --- Receiver waiting state ---
  pub(crate) consumer_parked_sync_flag: CachePadded<AtomicBool>,
  pub(crate) consumer_thread_sync: CachePadded<UnsafeCell<Option<Thread>>>,
  pub(crate) consumer_waker_async: CachePadded<AtomicWaker>,

  // These flags indicate if the authoritative producer/consumer handle has been dropped.
  pub(crate) producer_dropped: AtomicBool,
  pub(crate) consumer_dropped: AtomicBool,
}

// unsafe impl Send/Sync for SpscShared<T> are crucial for Arc<SpscShared<T>> to be Send
// and for futures holding &SpscShared<T> to be Send.
unsafe impl<T: Send> Send for SpscShared<T> {}
unsafe impl<T: Send> Sync for SpscShared<T> {} // Our SPSC logic + atomics ensure synchronized access

impl<T> fmt::Debug for SpscShared<T> {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("SpscShared")
      .field("capacity", &self.capacity)
      .field("head", &self.head.load(Ordering::Relaxed))
      .field("tail", &self.tail.load(Ordering::Relaxed))
      .field(
        "producer_parked_sync_flag",
        &self.producer_parked_sync_flag.load(Ordering::Relaxed),
      )
      .field("producer_waker_async", &self.producer_waker_async) // AtomicWaker is Debug
      .field(
        "consumer_parked_sync_flag",
        &self.consumer_parked_sync_flag.load(Ordering::Relaxed),
      )
      .field("consumer_waker_async", &self.consumer_waker_async) // AtomicWaker is Debug
      .field(
        "producer_dropped",
        &self.producer_dropped.load(Ordering::Relaxed),
      )
      .field(
        "consumer_dropped",
        &self.consumer_dropped.load(Ordering::Relaxed),
      )
      .finish_non_exhaustive()
  }
}

impl<T> SpscShared<T> {
  /// Pub(crate) constructor for SpscShared.
  /// Used by `bounded_sync` and `bounded_async` to create the shared core.
  pub(crate) fn new_internal(capacity: usize) -> Self {
    assert!(capacity > 0, "SPSC channel capacity must be greater than 0");
    let mut buffer_vec = Vec::with_capacity(capacity);
    for _ in 0..capacity {
      buffer_vec.push(CachePadded::new(UnsafeCell::new(MaybeUninit::uninit())));
    }
    SpscShared {
      buffer: buffer_vec.into_boxed_slice(),
      capacity,
      head: CachePadded::new(AtomicUsize::new(0)),
      tail: CachePadded::new(AtomicUsize::new(0)),
      producer_parked_sync_flag: CachePadded::new(AtomicBool::new(false)),
      producer_thread_sync: CachePadded::new(UnsafeCell::new(None)),
      producer_waker_async: CachePadded::new(AtomicWaker::new()),
      consumer_parked_sync_flag: CachePadded::new(AtomicBool::new(false)),
      consumer_thread_sync: CachePadded::new(UnsafeCell::new(None)),
      consumer_waker_async: CachePadded::new(AtomicWaker::new()),
      producer_dropped: AtomicBool::new(false),
      consumer_dropped: AtomicBool::new(false),
    }
  }

  #[inline]
  pub(crate) fn current_len(&self, head: usize, tail: usize) -> usize {
    head.wrapping_sub(tail)
  }

  #[inline]
  pub(crate) fn is_full(&self, head: usize, tail: usize) -> bool {
    self.current_len(head, tail) == self.capacity
  }

  #[inline]
  pub(crate) fn is_empty(&self, head: usize, tail: usize) -> bool {
    head == tail
  }

  #[inline]
  pub(crate) fn wake_consumer(&self) {
    // Try to wake sync consumer first if it's parked
    if self.consumer_parked_sync_flag.load(Ordering::Relaxed) {
      // Use Acquire fence to ensure that if we see `true`, the subsequent CAS operates
      // on a state that is at least as current as this read.
      atomic::fence(Ordering::Acquire);
      // Attempt to transition from true (parked) to false (unparked/woken)
      if self
        .consumer_parked_sync_flag
        .compare_exchange(true, false, Ordering::AcqRel, Ordering::Relaxed)
        .is_ok()
      {
        // Successfully set flag to false, meaning we are responsible for unparking.
        if let Some(thread_handle) = unsafe { (*self.consumer_thread_sync.get()).take() } {
          sync_util::unpark_thread(&thread_handle);
        }
      }
      // If CAS fails, another thread (or this one in a race) already unparked it.
    }
    // Always wake the async waker. It's idempotent and handles its own state.
    self.consumer_waker_async.wake();
  }

  #[inline]
  pub(crate) fn wake_producer(&self) {
    if self.producer_parked_sync_flag.load(Ordering::Relaxed) {
      atomic::fence(Ordering::Acquire);
      if self
        .producer_parked_sync_flag
        .compare_exchange(true, false, Ordering::AcqRel, Ordering::Relaxed)
        .is_ok()
      {
        if let Some(thread_handle) = unsafe { (*self.producer_thread_sync.get()).take() } {
          sync_util::unpark_thread(&thread_handle);
        }
      }
    }
    self.producer_waker_async.wake();
  }

  pub(crate) fn poll_recv_internal(&self, cx: &mut Context<'_>) -> Poll<Result<T, RecvError>> {
    loop {
      let tail = self.tail.load(Ordering::Relaxed);
      let head = self.head.load(Ordering::Acquire);

      if !self.is_empty(head, tail) {
        let slot_idx = tail % self.capacity;
        let item = unsafe { (*self.buffer[slot_idx].get()).assume_init_read() };
        self.tail.store(tail.wrapping_add(1), Ordering::Release);
        self.wake_producer();
        return Poll::Ready(Ok(item));
      }

      if self.producer_dropped.load(Ordering::Acquire) {
        let final_head = self.head.load(Ordering::Acquire);
        if final_head == tail {
          return Poll::Ready(Err(RecvError::Disconnected));
        }
        continue;
      }

      self.consumer_waker_async.register(cx.waker());

      // Critical re-check after registration
      let head_after_register = self.head.load(Ordering::Acquire);
      if !self.is_empty(head_after_register, tail) || self.producer_dropped.load(Ordering::Acquire)
      {
        continue;
      }

      return Poll::Pending;
    }
  }
}

impl<T> Drop for SpscShared<T> {
  fn drop(&mut self) {
    // This is called when the Arc<SpscShared<T>> is finally dropped.
    // Drop any items remaining in the buffer.
    if !(self.producer_dropped.load(Ordering::Relaxed)
      && self.consumer_dropped.load(Ordering::Relaxed))
    {
      // This might indicate Arc::leak or mem::forget was used on a P/C wrapper without proper drop.
      // For SPSC, typically both flags should be true if Arc count reaches zero through normal drops.
      // eprintln!("Warning: SpscShared dropped without producer AND consumer flags both being set.");
    }

    let head = *self.head.get_mut(); // Safe in Drop with &mut self due to exclusive access
    let mut tail = *self.tail.get_mut();

    while tail != head {
      let slot_idx = tail % self.capacity;
      unsafe {
        // Get a mutable pointer to the UnsafeCell's content.
        let slot_unsafe_cell_ptr = self.buffer[slot_idx].get();
        // Assume the value was initialized (since head > tail implies it was written).
        (*slot_unsafe_cell_ptr).assume_init_drop();
      }
      tail = tail.wrapping_add(1);
    }
  }
}
