// src/spmc/ring_buffer.rs

use crate::async_util::AtomicWaker;
use crate::error::{RecvError, SendError, TryRecvError};
use crate::internal::cache_padded::CachePadded;
use crate::sync_util;

use std::cell::UnsafeCell;
use std::fmt;
use std::future::Future;
use std::marker::PhantomData;
use std::mem::MaybeUninit;
use std::pin::Pin;
use std::sync::atomic::{self, AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::thread::{self, Thread};

// --- Core Structs (No changes here) ---

pub(crate) struct Slot<T> {
  sequence: AtomicUsize,
  value: UnsafeCell<MaybeUninit<T>>,
  waker: AtomicWaker,
}

impl<T> Drop for Slot<T> {
  fn drop(&mut self) {
    if *self.sequence.get_mut() % 2 == 1 {
      unsafe { self.value.get_mut().assume_init_drop() };
    }
  }
}

#[derive(Debug)]
struct ConsumerCursors {
  list: Vec<Arc<AtomicUsize>>,
}

pub(crate) struct SpmcShared<T: Send + Clone> {
  buffer: Box<[Slot<T>]>,
  capacity: usize,
  head: CachePadded<UnsafeCell<usize>>,
  tails: Mutex<ConsumerCursors>,
  producer_parked: AtomicBool,
  producer_thread: UnsafeCell<Option<Thread>>,
  producer_waker: AtomicWaker,
}

impl<T: Send + Clone> fmt::Debug for SpmcShared<T> {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("SpmcShared")
      .field("capacity", &self.capacity)
      .field("head", &unsafe { *self.head.get() })
      .field("tails", &self.tails.lock().unwrap())
      .finish_non_exhaustive()
  }
}

unsafe impl<T: Send + Clone> Send for SpmcShared<T> {}
unsafe impl<T: Send + Clone> Sync for SpmcShared<T> {}

impl<T: Send + Clone> SpmcShared<T> {
  fn new(capacity: usize) -> Self {
    let mut buffer = Vec::with_capacity(capacity);
    for i in 0..capacity {
      buffer.push(Slot {
        sequence: AtomicUsize::new(2 * i),
        value: UnsafeCell::new(MaybeUninit::uninit()),
        waker: AtomicWaker::new(),
      });
    }
    SpmcShared {
      buffer: buffer.into_boxed_slice(),
      capacity,
      head: CachePadded::new(UnsafeCell::new(0)),
      tails: Mutex::new(ConsumerCursors { list: Vec::new() }),
      producer_parked: AtomicBool::new(false),
      producer_thread: UnsafeCell::new(None),
      producer_waker: AtomicWaker::new(),
    }
  }

  fn wake_producer(&self) {
    if self.producer_parked.load(Ordering::Relaxed) {
      atomic::fence(Ordering::Acquire);
      if self
        .producer_parked
        .compare_exchange(true, false, Ordering::AcqRel, Ordering::Relaxed)
        .is_ok()
      {
        if let Some(thread) = unsafe { (*self.producer_thread.get()).take() } {
          thread.unpark();
        }
      }
    }
    self.producer_waker.wake();
  }
}

// --- Handles (No changes here) ---
#[derive(Debug)]
pub struct Producer<T: Send + Clone> {
  pub(crate) shared: Arc<SpmcShared<T>>,
  pub(crate) _phantom: PhantomData<*const ()>,
}
unsafe impl<T: Send + Clone> Send for Producer<T> {}
#[derive(Debug)]
pub struct AsyncProducer<T: Send + Clone> {
  pub(crate) shared: Arc<SpmcShared<T>>,
  pub(crate) _phantom: PhantomData<*const ()>,
}
unsafe impl<T: Send + Clone> Send for AsyncProducer<T> {}
#[derive(Debug)]
pub struct Receiver<T: Send + Clone> {
  pub(crate) shared: Arc<SpmcShared<T>>,
  pub(crate) tail: Arc<AtomicUsize>,
}
#[derive(Debug)]
pub struct AsyncReceiver<T: Send + Clone> {
  pub(crate) shared: Arc<SpmcShared<T>>,
  pub(crate) tail: Arc<AtomicUsize>,
}

// --- Channel Constructor (No changes here) ---
pub(crate) fn new_channel<T: Send + Clone>(capacity: usize) -> (Producer<T>, Receiver<T>) {
  assert!(capacity > 0, "SPMC channel capacity must be > 0");
  let shared = Arc::new(SpmcShared::new(capacity));
  let initial_tail = Arc::new(AtomicUsize::new(0));
  shared.tails.lock().unwrap().list.push(initial_tail.clone());
  (
    Producer {
      shared: Arc::clone(&shared),
      _phantom: PhantomData,
    },
    Receiver {
      shared,
      tail: initial_tail,
    },
  )
}

// --- Receiver Logic (No changes needed here, it was already correct) ---
fn try_recv_internal<T: Send + Clone>(
  shared: &SpmcShared<T>,
  tail_atomic: &AtomicUsize,
) -> Result<T, TryRecvError> {
  let tail = tail_atomic.load(Ordering::Relaxed);
  let slot_idx = tail % shared.capacity;
  let slot = &shared.buffer[slot_idx];
  let seq = slot.sequence.load(Ordering::Acquire);

  if seq == 2 * tail + 1 {
    let value = unsafe { (*slot.value.get()).assume_init_ref().clone() };
    tail_atomic.store(tail + 1, Ordering::Release);
    // CRITICAL: A consumer making progress must wake a potentially blocked producer.
    shared.wake_producer();
    Ok(value)
  } else {
    Err(TryRecvError::Empty)
  }
}

impl<T: Send + Clone> Receiver<T> {
  pub fn try_recv(&mut self) -> Result<T, TryRecvError> {
    try_recv_internal(&self.shared, &self.tail)
  }
  pub fn recv(&mut self) -> Result<T, RecvError> {
    loop {
      match self.try_recv() {
        Ok(value) => return Ok(value),
        Err(TryRecvError::Empty) => {
          // This spin-wait is inefficient but correct for a simple sync implementation.
          // A full-featured sync `recv` would park the thread here and be woken
          // by the producer's `slot.waker.wake()`. For this fix, we focus on the deadlock.
          thread::yield_now();
        }
        Err(TryRecvError::Disconnected) => unreachable!(),
      }
    }
  }
}
impl<T: Send + Clone> AsyncReceiver<T> {
  pub fn try_recv(&mut self) -> Result<T, TryRecvError> {
    try_recv_internal(&self.shared, &self.tail)
  }
  pub fn recv(&mut self) -> RecvFuture<'_, T> {
    RecvFuture { receiver: self }
  }
}

// --- Common Receiver Traits (No changes here) ---
fn clone_receiver<T: Send + Clone>(
  shared: &Arc<SpmcShared<T>>,
  tail: &Arc<AtomicUsize>,
) -> Arc<AtomicUsize> {
  let new_tail_val = tail.load(Ordering::Acquire);
  let new_tail = Arc::new(AtomicUsize::new(new_tail_val));
  shared.tails.lock().unwrap().list.push(new_tail.clone());
  new_tail
}
impl<T: Send + Clone> Clone for Receiver<T> {
  fn clone(&self) -> Self {
    Self {
      shared: Arc::clone(&self.shared),
      tail: clone_receiver(&self.shared, &self.tail),
    }
  }
}
impl<T: Send + Clone> Clone for AsyncReceiver<T> {
  fn clone(&self) -> Self {
    Self {
      shared: Arc::clone(&self.shared),
      tail: clone_receiver(&self.shared, &self.tail),
    }
  }
}
fn drop_receiver<T: Send + Clone>(shared: &SpmcShared<T>, tail: &Arc<AtomicUsize>) {
  let mut tails_guard = shared.tails.lock().unwrap();
  tails_guard.list.retain(|t| !Arc::ptr_eq(t, tail));
  // After a consumer drops, the producer might be unblocked.
  shared.wake_producer();
}
impl<T: Send + Clone> Drop for Receiver<T> {
  fn drop(&mut self) {
    drop_receiver(&self.shared, &self.tail)
  }
}
impl<T: Send + Clone> Drop for AsyncReceiver<T> {
  fn drop(&mut self) {
    drop_receiver(&self.shared, &self.tail)
  }
}

// --- Producer Logic (FIXED) ---

impl<T: Send + Clone> Producer<T> {
  pub fn send(&mut self, value: T) -> Result<(), SendError> {
    let head = unsafe { *self.shared.head.get() };

    // Wait until there is space in the buffer.
    loop {
      let tails_guard = self.shared.tails.lock().unwrap();
      // If all consumers have disconnected, the channel is closed.
      if tails_guard.list.is_empty() {
        return Err(SendError::Closed);
      }
      // The producer is blocked if its `head` would lap the slowest consumer's `tail`.
      // We find the minimum tail position to determine how far the producer can go.
      let min_tail = tails_guard
        .list
        .iter()
        .map(|t| t.load(Ordering::Acquire))
        .min()
        .unwrap();
      drop(tails_guard);

      // The space available is `capacity - (head - min_tail)`.
      // If `head - min_tail == capacity`, the buffer is full.
      if head - min_tail < self.shared.capacity {
        break; // Space is available, exit the blocking loop.
      }

      // Buffer is full, park the thread.
      unsafe { *self.shared.producer_thread.get() = Some(thread::current()) };
      self.shared.producer_parked.store(true, Ordering::Release);

      // Re-check condition after setting park flag to avoid lost wakeups.
      let tails_guard = self.shared.tails.lock().unwrap();
      if tails_guard.list.is_empty() {
        return Err(SendError::Closed);
      }
      let min_tail = tails_guard
        .list
        .iter()
        .map(|t| t.load(Ordering::Acquire))
        .min()
        .unwrap();
      drop(tails_guard);

      if head - min_tail < self.shared.capacity {
        // Space appeared while we were preparing to park. Abort the park.
        if self
          .shared
          .producer_parked
          .compare_exchange(true, false, Ordering::AcqRel, Ordering::Relaxed)
          .is_ok()
        {
          unsafe {
            *self.shared.producer_thread.get() = None;
          }
        }
        continue; // Retry the loop immediately.
      }

      sync_util::park_thread();
    }

    // --- Perform the write ---
    // At this point, we know there is space.
    unsafe {
      let slot_idx = head % self.shared.capacity;
      let slot = &self.shared.buffer[slot_idx];

      // Write the value and update the sequence number for consumers.
      (*slot.value.get()).write(value);
      slot.sequence.store(2 * head + 1, Ordering::Release);

      // Wake any consumers that were waiting on *this specific slot*.
      slot.waker.wake();

      // Advance the producer's head.
      *self.shared.head.get() = head + 1;
    }
    Ok(())
  }
}

impl<T: Send + Clone> AsyncProducer<T> {
  pub fn send(&mut self, value: T) -> SendFuture<'_, T> {
    SendFuture {
      producer: self,
      value: Some(value),
    }
  }
}

// --- Futures (FIXED) ---
#[must_use]
pub struct SendFuture<'a, T: Send + Clone> {
  producer: &'a mut AsyncProducer<T>,
  value: Option<T>,
}

impl<'a, T: Send + Clone + Unpin> Future for SendFuture<'a, T> {
  type Output = Result<(), SendError>;
  fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    let s = self.as_mut().get_mut();

    loop {
      let shared = &s.producer.shared;
      let head = unsafe { *shared.head.get() };

      // Check if the buffer is full relative to the slowest consumer.
      let tails_guard = shared.tails.lock().unwrap();
      if tails_guard.list.is_empty() {
        s.value = None; // Consume the value on error.
        return Poll::Ready(Err(SendError::Closed));
      }
      let min_tail = tails_guard
        .list
        .iter()
        .map(|t| t.load(Ordering::Acquire))
        .min()
        .unwrap();
      drop(tails_guard);

      if head - min_tail >= shared.capacity {
        // Buffer is full. Register our waker to be woken by a consumer.
        shared.producer_waker.register(cx.waker());

        // Re-check after registering to prevent lost wakeups.
        let tails_guard = shared.tails.lock().unwrap();
        if tails_guard.list.is_empty() {
          // Check again for drop
          s.value = None;
          return Poll::Ready(Err(SendError::Closed));
        }
        let new_min_tail = tails_guard
          .list
          .iter()
          .map(|t| t.load(Ordering::Acquire))
          .min()
          .unwrap();
        drop(tails_guard);

        // If space has opened up, we must loop to try again.
        if head - new_min_tail < shared.capacity {
          continue;
        }

        // Still full, waker is registered, so we can park.
        return Poll::Pending;
      }

      // --- Space is available, perform the write ---
      let value_to_write = s.value.take().expect("SendFuture polled after completion");
      let slot_idx = head % shared.capacity;
      let slot = &shared.buffer[slot_idx];

      unsafe {
        (*slot.value.get()).write(value_to_write);
        // This sequence number signals to consumers that the data is ready.
        slot.sequence.store(2 * head + 1, Ordering::Release);
        // Wake any consumers waiting for this specific slot.
        slot.waker.wake();
        // Advance our head.
        *shared.head.get() = head + 1;
      }
      return Poll::Ready(Ok(()));
    }
  }
}

#[must_use]
pub struct RecvFuture<'a, T: Send + Clone> {
  receiver: &'a mut AsyncReceiver<T>,
}

// No changes needed for RecvFuture, its logic was correct.
impl<'a, T: Send + Clone> Future for RecvFuture<'a, T> {
  type Output = Result<T, RecvError>;
  fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    let tail = self.receiver.tail.load(Ordering::Relaxed);
    let slot_idx = tail % self.receiver.shared.capacity;
    let slot = &self.receiver.shared.buffer[slot_idx];

    // Check if the slot we're waiting for has the correct sequence number.
    let seq = slot.sequence.load(Ordering::Acquire);
    if seq == 2 * tail + 1 {
      let value = unsafe { (*slot.value.get()).assume_init_ref().clone() };
      self.receiver.tail.store(tail + 1, Ordering::Release);
      self.receiver.shared.wake_producer();
      return Poll::Ready(Ok(value));
    }

    // Data not ready. Register waker on the slot we are waiting for.
    slot.waker.register(cx.waker());

    // Re-check after registering to prevent lost wakeups.
    if slot.sequence.load(Ordering::Acquire) == 2 * tail + 1 {
      let value = unsafe { (*slot.value.get()).assume_init_ref().clone() };
      self.receiver.tail.store(tail + 1, Ordering::Release);
      self.receiver.shared.wake_producer();
      return Poll::Ready(Ok(value));
    }

    Poll::Pending
  }
}
