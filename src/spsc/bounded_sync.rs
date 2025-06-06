// src/spsc/bounded_sync.rs

use crate::async_util::AtomicWaker;
use crate::error::{RecvError, RecvErrorTimeout, SendError, TryRecvError, TrySendError};
use crate::internal::cache_padded::CachePadded;
use crate::sync_util;

use std::cell::UnsafeCell;
use std::fmt;
use std::marker::PhantomData;
use std::mem::MaybeUninit;
use std::sync::atomic::{self, AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread::{self, Thread};
use std::time::{Duration, Instant};

// Import async types for conversion methods
use super::bounded_async::{AsyncBoundedSpscConsumer, AsyncBoundedSpscProducer};
use std::mem; // For mem::forget

/// Internal shared state for the bounded SPSC channel, supporting both sync and async waiters.
// This struct is pub(crate) so it can be constructed directly by bounded_async.rs and in tests.
// Fields are pub(crate) for access within the fibre::spsc module.
pub struct SpscShared<T> {
  pub(crate) buffer: Box<[CachePadded<UnsafeCell<MaybeUninit<T>>>]>,
  pub(crate) capacity: usize,
  pub(crate) head: CachePadded<AtomicUsize>, // Write index (producer)
  pub(crate) tail: CachePadded<AtomicUsize>, // Read index (consumer)

  // --- Producer waiting state ---
  producer_parked_sync_flag: CachePadded<AtomicBool>,
  producer_thread_sync: CachePadded<UnsafeCell<Option<Thread>>>,
  pub(crate) producer_waker_async: CachePadded<AtomicWaker>,

  // --- Consumer waiting state ---
  consumer_parked_sync_flag: CachePadded<AtomicBool>,
  consumer_thread_sync: CachePadded<UnsafeCell<Option<Thread>>>,
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
  fn len(&self, head: usize, tail: usize) -> usize {
    head.wrapping_sub(tail)
  }

  #[inline]
  pub(crate) fn is_full(&self, head: usize, tail: usize) -> bool {
    self.len(head, tail) == self.capacity
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

/// The synchronous sending end of a bounded SPSC channel.
#[derive(Debug)]
pub struct BoundedSyncProducer<T> {
  pub(crate) shared: Arc<SpscShared<T>>,
  // This PhantomData makes BoundedSyncProducer<T> !Sync, which is appropriate
  // as only one thread should use the producer.
  pub(crate) _phantom: PhantomData<*mut ()>,
}

/// The synchronous receiving end of a bounded SPSC channel.
#[derive(Debug)]
pub struct BoundedSyncConsumer<T> {
  pub(crate) shared: Arc<SpscShared<T>>,
  // This PhantomData makes BoundedSyncConsumer<T> !Sync.
  pub(crate) _phantom: PhantomData<*mut ()>,
}

unsafe impl<T: Send> Send for BoundedSyncProducer<T> {}
unsafe impl<T: Send> Send for BoundedSyncConsumer<T> {}
// BoundedSyncProducer and BoundedSyncConsumer are NOT Sync due to _phantom: PhantomData<*mut ()>.

impl<T: Send> BoundedSyncProducer<T> {
  // T: Send is important here
  /// Crate-internal constructor, used for creating from a shared core, e.g., in tests or by async part.
  pub(crate) fn from_shared(shared: Arc<SpscShared<T>>) -> Self {
    Self {
      shared,
      _phantom: PhantomData,
    }
  }

  /// Converts this synchronous SPSC producer into an asynchronous one.
  ///
  /// This is a zero-cost conversion. The `Drop` implementation of the
  /// `BoundedSyncProducer` will not be called.
  pub fn to_async(self) -> AsyncBoundedSpscProducer<T> {
    let shared = unsafe { std::ptr::read(&self.shared) };
    mem::forget(self); // Prevent Drop of BoundedSyncProducer
    AsyncBoundedSpscProducer::from_shared(shared) // Use async's pub(crate) constructor
  }

  /// Attempts to send an item into the channel without blocking.
  ///
  /// # Errors
  ///
  /// - `Err(TrySendError::Full(item))` if the channel is full.
  /// - `Err(TrySendError::Closed(item))` if the consumer has been dropped.
  pub fn try_send(&mut self, item: T) -> Result<(), TrySendError<T>> {
    if self.shared.consumer_dropped.load(Ordering::Acquire) {
      return Err(TrySendError::Closed(item));
    }

    let head = self.shared.head.load(Ordering::Relaxed); // Producer owns head updates
    let tail = self.shared.tail.load(Ordering::Acquire); // Must see latest consumer progress

    if self.shared.is_full(head, tail) {
      return Err(TrySendError::Full(item));
    }

    // Write the item
    let slot_idx = head % self.shared.capacity;
    unsafe {
      // Get a mutable pointer to the UnsafeCell's content
      let slot_ptr = self.shared.buffer[slot_idx].get();
      // Write the item into the MaybeUninit memory
      (*slot_ptr).write(item);
    }
    // Publish the write by advancing head. Release ensures prior write is visible.
    self
      .shared
      .head
      .store(head.wrapping_add(1), Ordering::Release);

    // Notify the consumer
    self.shared.wake_consumer();
    Ok(())
  }

  /// Sends an item into the channel, blocking the current thread if the channel is full.
  ///
  /// # Errors
  ///
  /// - `Err(SendError::Closed)` if the consumer has been dropped.
  pub fn send(&mut self, mut item: T) -> Result<(), SendError> {
    loop {
      if self.shared.consumer_dropped.load(Ordering::Acquire) {
        return Err(SendError::Closed);
      }

      match self.try_send(item) {
        Ok(()) => return Ok(()),
        Err(TrySendError::Full(returned_item)) => {
          item = returned_item; // Keep the item for the next attempt

          // Prepare to park
          unsafe {
            // This is safe because producer is !Sync, so no other thread accesses this field.
            *self.shared.producer_thread_sync.get() = Some(thread::current());
          }
          // Release ensures that the store to producer_thread_sync is visible before
          // consumer checks producer_parked_sync_flag.
          self
            .shared
            .producer_parked_sync_flag
            .store(true, Ordering::Release);

          // Critical re-check to prevent lost wakeups
          // Must re-load head and tail *after* setting the parked flag.
          let head_after_flag_set = self.shared.head.load(Ordering::Relaxed);
          let tail_after_flag_set = self.shared.tail.load(Ordering::Acquire);

          if !self
            .shared
            .is_full(head_after_flag_set, tail_after_flag_set)
            || self.shared.consumer_dropped.load(Ordering::Acquire)
          // Also check if consumer dropped
          {
            // Channel became not full or consumer dropped while we were setting up to park.
            // Try to un-set the parked flag.
            if self
              .shared
              .producer_parked_sync_flag
              .compare_exchange(true, false, Ordering::AcqRel, Ordering::Relaxed)
              .is_ok()
            {
              // Successfully un-set, clear the thread handle.
              unsafe {
                *self.shared.producer_thread_sync.get() = None;
              }
            }
            // Loop again to retry send or handle closed.
            continue;
          }

          // Actually park
          sync_util::park_thread();

          // After unparking, ensure the flag is cleared if it was still set by us.
          // This handles spurious wakeups or cases where waker clears flag before we run.
          if self
            .shared
            .producer_parked_sync_flag
            .load(Ordering::Relaxed)
          {
            if self
              .shared
              .producer_parked_sync_flag
              .compare_exchange(true, false, Ordering::AcqRel, Ordering::Relaxed)
              .is_ok()
            {
              unsafe {
                *self.shared.producer_thread_sync.get() = None;
              }
            }
          }
          // Loop to retry send.
        }
        Err(TrySendError::Closed(_returned_item)) => {
          // try_send can return Closed if consumer dropped. Propagate as SendError::Closed.
          return Err(SendError::Closed);
        }
        Err(TrySendError::Sent(_)) => unreachable!("SPSC try_send should not return Sent variant"),
      }
    }
  }
}

impl<T> Drop for BoundedSyncProducer<T> {
  fn drop(&mut self) {
    self.shared.producer_dropped.store(true, Ordering::Release);
    // Ensure the store to producer_dropped is globally visible
    // before the wake can cause the consumer to read it.
    atomic::fence(Ordering::SeqCst);
    self.shared.wake_consumer();
  }
}

impl<T: Send> BoundedSyncConsumer<T> {
  // T: Send is important here
  /// Crate-internal constructor.
  pub(crate) fn from_shared(shared: Arc<SpscShared<T>>) -> Self {
    Self {
      shared,
      _phantom: PhantomData,
    }
  }

  /// Converts this synchronous SPSC consumer into an asynchronous one.
  ///
  /// This is a zero-cost conversion. The `Drop` implementation of the
  /// `BoundedSyncConsumer` will not be called.
  pub fn to_async(self) -> AsyncBoundedSpscConsumer<T> {
    let shared = unsafe { std::ptr::read(&self.shared) };
    mem::forget(self); // Prevent Drop of BoundedSyncConsumer
    AsyncBoundedSpscConsumer::from_shared(shared) // Use async's pub(crate) constructor
  }

  /// Attempts to receive an item from the channel without blocking.
  ///
  /// # Errors
  ///
  /// - `Ok(T)` if an item was received.
  /// - `Err(TryRecvError::Empty)` if the channel is empty but the producer is alive.
  /// - `Err(TryRecvError::Disconnected)` if the producer has been dropped and the channel is empty.
  pub fn try_recv(&mut self) -> Result<T, TryRecvError> {
    let tail = self.shared.tail.load(Ordering::Relaxed); // Consumer owns tail updates
    let head = self.shared.head.load(Ordering::Acquire); // Must see latest producer writes

    if self.shared.is_empty(head, tail) {
      if self.shared.producer_dropped.load(Ordering::Acquire) {
        // Producer is gone. Re-check head to see if an item was sent just before drop.
        let final_head = self.shared.head.load(Ordering::Acquire);
        if final_head == tail {
          // Still empty even after seeing producer_dropped
          return Err(TryRecvError::Disconnected);
        }
        // An item was sent right before producer dropped. Fall through to read it.
        // Update `head` for the read logic below to use this `final_head`.
        let head_for_read = final_head;
        let slot_idx = tail % self.shared.capacity;
        let item = unsafe { (*self.shared.buffer[slot_idx].get()).assume_init_read() };
        self
          .shared
          .tail
          .store(tail.wrapping_add(1), Ordering::Release);
        // wake_producer is less critical if producer_dropped, but good for consistency.
        self.shared.wake_producer();
        return Ok(item);
      } else {
        return Err(TryRecvError::Empty);
      }
    }

    // If not empty initially:
    let slot_idx = tail % self.shared.capacity;
    let item = unsafe { (*self.shared.buffer[slot_idx].get()).assume_init_read() };
    // Publish the read by advancing tail. Release ensures prior read is done.
    self
      .shared
      .tail
      .store(tail.wrapping_add(1), Ordering::Release);

    // Notify the producer that space is available
    self.shared.wake_producer();
    Ok(item)
  }

  /// Receives an item from the channel, blocking the current thread if the channel is empty.
  ///
  /// # Errors
  ///
  /// - `Err(RecvError::Disconnected)` if the producer has been dropped and the channel is empty.
  pub fn recv(&mut self) -> Result<T, RecvError> {
    loop {
      match self.try_recv() {
        Ok(item) => return Ok(item),
        Err(TryRecvError::Empty) => {
          // Prepare to park
          unsafe {
            *self.shared.consumer_thread_sync.get() = Some(thread::current());
          }
          self
            .shared
            .consumer_parked_sync_flag
            .store(true, Ordering::Release);

          // Critical re-check
          let head_after_flag_set = self.shared.head.load(Ordering::Acquire);
          let tail_after_flag_set = self.shared.tail.load(Ordering::Relaxed); // Tail doesn't change concurrently

          if !self
            .shared
            .is_empty(head_after_flag_set, tail_after_flag_set)
            || self.shared.producer_dropped.load(Ordering::Acquire)
          // Also check if producer dropped
          {
            if self
              .shared
              .consumer_parked_sync_flag
              .compare_exchange(true, false, Ordering::AcqRel, Ordering::Relaxed)
              .is_ok()
            {
              unsafe {
                *self.shared.consumer_thread_sync.get() = None;
              }
            }
            continue;
          }
          sync_util::park_thread();
          if self
            .shared
            .consumer_parked_sync_flag
            .load(Ordering::Relaxed)
          {
            if self
              .shared
              .consumer_parked_sync_flag
              .compare_exchange(true, false, Ordering::AcqRel, Ordering::Relaxed)
              .is_ok()
            {
              unsafe {
                *self.shared.consumer_thread_sync.get() = None;
              }
            }
          }
        }
        Err(TryRecvError::Disconnected) => {
          return Err(RecvError::Disconnected);
        }
      }
    }
  }

  /// Receives an item from the channel, blocking for at most `timeout` duration.
  /// (This is a placeholder, actual timeout logic needs implementation)
  pub fn recv_timeout(&mut self, timeout: Duration) -> Result<T, RecvErrorTimeout> {
    let start_time = Instant::now();
    loop {
      match self.try_recv() {
        Ok(item) => return Ok(item),
        Err(TryRecvError::Empty) => {
          if start_time.elapsed() >= timeout {
            return Err(RecvErrorTimeout::Timeout);
          }
          // Simplified parking for timeout example. Real implementation would be more nuanced.
          // This will park for the *remaining* duration or less.
          let remaining_timeout = timeout.saturating_sub(start_time.elapsed());
          if remaining_timeout.is_zero() {
            return Err(RecvErrorTimeout::Timeout);
          }

          unsafe {
            *self.shared.consumer_thread_sync.get() = Some(thread::current());
          }
          self
            .shared
            .consumer_parked_sync_flag
            .store(true, Ordering::Release);

          let head_after_flag = self.shared.head.load(Ordering::Acquire);
          let tail_after_flag = self.shared.tail.load(Ordering::Relaxed);

          if !self.shared.is_empty(head_after_flag, tail_after_flag)
            || self.shared.producer_dropped.load(Ordering::Acquire)
          {
            if self
              .shared
              .consumer_parked_sync_flag
              .compare_exchange(true, false, Ordering::AcqRel, Ordering::Relaxed)
              .is_ok()
            {
              unsafe {
                *self.shared.consumer_thread_sync.get() = None;
              }
            }
            continue; // Loop to retry or handle disconnect
          }

          sync_util::park_thread_timeout(remaining_timeout); // Park with timeout

          if self
            .shared
            .consumer_parked_sync_flag
            .load(Ordering::Relaxed)
          {
            if self
              .shared
              .consumer_parked_sync_flag
              .compare_exchange(true, false, Ordering::AcqRel, Ordering::Relaxed)
              .is_ok()
            {
              unsafe {
                *self.shared.consumer_thread_sync.get() = None;
              }
            }
          }
          // After park_timeout, loop to re-check or timeout.
        }
        Err(TryRecvError::Disconnected) => return Err(RecvErrorTimeout::Disconnected),
      }
    }
  }
}

impl<T> Drop for BoundedSyncConsumer<T> {
  fn drop(&mut self) {
    self.shared.consumer_dropped.store(true, Ordering::Release);
    atomic::fence(Ordering::SeqCst);
    self.shared.wake_producer();
  }
}

/// Creates a new bounded synchronous SPSC channel with the given capacity.
///
/// `capacity` must be greater than 0. Panics if capacity is 0.
///
/// The returned producer and consumer are the sole handles to their respective
/// ends of the channel.
pub fn bounded_sync<T: Send>(capacity: usize) -> (BoundedSyncProducer<T>, BoundedSyncConsumer<T>) {
  let shared_core = SpscShared::new_internal(capacity);
  let shared_arc = Arc::new(shared_core);

  (
    BoundedSyncProducer {
      shared: Arc::clone(&shared_arc),
      _phantom: PhantomData,
    },
    BoundedSyncConsumer {
      shared: shared_arc,
      _phantom: PhantomData,
    },
  )
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::{
    thread,
    time::{Duration, Instant},
  }; // Added Instant for timeout test

  #[test]
  fn create_channel() {
    let (p, c) = bounded_sync::<i32>(1);
    assert_eq!(p.shared.capacity, 1);
    assert_eq!(c.shared.capacity, 1);
  }

  #[test]
  #[should_panic]
  fn create_channel_zero_capacity() {
    let (_, _) = bounded_sync::<i32>(0);
  }

  #[test]
  fn send_recv_single_item() {
    let (mut p, mut c) = bounded_sync(1);
    p.send(42i32).unwrap();
    assert_eq!(c.recv().unwrap(), 42);
  }

  #[test]
  fn try_send_full_try_recv_empty() {
    let (mut p, mut c) = bounded_sync::<i32>(1);
    p.try_send(10).unwrap();
    match p.try_send(20) {
      Err(TrySendError::Full(val)) => assert_eq!(val, 20),
      res => panic!("Expected Full error, got {:?}", res),
    }

    assert_eq!(c.try_recv().unwrap(), 10);
    match c.try_recv() {
      Err(TryRecvError::Empty) => {}
      res => panic!("Expected Empty error, got {:?}", res),
    }
  }

  #[test]
  fn send_blocks_until_recv() {
    let (mut p, mut c) = bounded_sync::<i32>(1);
    p.send(1).unwrap(); // Fill channel

    let producer_thread = thread::spawn(move || {
      p.send(2).unwrap(); // This should block
      p // Return producer to drop it in main thread, ensuring its Drop runs
    });

    thread::sleep(Duration::from_millis(100)); // Give producer time to block

    assert_eq!(c.recv().unwrap(), 1); // Unblock producer
    let _p_returned = producer_thread.join().unwrap(); // Producer should now complete
    assert_eq!(c.recv().unwrap(), 2);
  }

  #[test]
  fn recv_blocks_until_send() {
    let (mut p, mut c) = bounded_sync::<i32>(1);

    let consumer_thread = thread::spawn(move || {
      let val = c.recv().unwrap(); // This should block
      assert_eq!(val, 100);
      c // Return consumer
    });

    thread::sleep(Duration::from_millis(100)); // Give consumer time to block

    p.send(100).unwrap(); // Unblock consumer
    let _c_returned = consumer_thread.join().unwrap();
  }

  #[test]
  fn producer_drop_signals_consumer() {
    let (mut p, mut c) = bounded_sync::<i32>(1); // p needs to be mut to call send
    p.send(7).unwrap(); // CORRECTED: Call send on p
    drop(p);
    assert_eq!(c.recv().unwrap(), 7);
    match c.recv() {
      Err(RecvError::Disconnected) => {}
      Ok(v) => panic!(
        "Expected Disconnected error after producer drop, got Ok({:?})",
        v
      ),
    }
  }

  #[test]
  fn producer_drop_empty_signals_consumer() {
    let (p, mut c) = bounded_sync::<i32>(1);
    drop(p);
    match c.recv() {
      Err(RecvError::Disconnected) => {}
      Ok(v) => panic!(
        "Expected Disconnected error for empty chan, got Ok({:?})",
        v
      ),
    }
  }

  #[test]
  fn consumer_drop_signals_producer() {
    let (mut p, c) = bounded_sync::<i32>(1);
    drop(c);
    match p.send(1) {
      Err(SendError::Closed) => {}
      Ok(_) => panic!("Expected Closed error after consumer drop"),
      Err(SendError::Sent) => panic!("SPSC should not produce Sent error"),
    }
  }

  #[test]
  fn stress_send_recv() {
    const ITEMS: usize = 100_000;
    const CAPACITY: usize = 128;
    let (mut p, mut c) = bounded_sync(CAPACITY);

    let producer_handle = thread::spawn(move || {
      for i in 0..ITEMS {
        p.send(i).unwrap();
      }
    });

    let consumer_handle = thread::spawn(move || {
      for i in 0..ITEMS {
        assert_eq!(c.recv().unwrap(), i);
      }
    });

    producer_handle.join().unwrap();
    consumer_handle.join().unwrap();
  }

  #[test]
  fn try_send_closed_by_consumer_drop() {
    let (mut p, c) = bounded_sync::<i32>(5);
    p.try_send(1).unwrap();
    drop(c);
    match p.try_send(2) {
      Err(TrySendError::Closed(val)) => assert_eq!(val, 2),
      other => panic!("Expected TrySendError::Closed, got {:?}", other),
    }
  }

  #[test]
  fn try_recv_disconnected_by_producer_drop() {
    let (mut p, mut c) = bounded_sync::<i32>(5); // p needs to be mut
    p.try_send(10).unwrap(); // CORRECTED: Call try_send on p
    drop(p);
    assert_eq!(c.try_recv().unwrap(), 10);
    match c.try_recv() {
      Err(TryRecvError::Disconnected) => {}
      other => panic!("Expected TryRecvError::Disconnected, got {:?}", other),
    }
  }
  #[test]
  fn try_recv_disconnected_by_producer_drop_empty() {
    let (p, mut c) = bounded_sync::<i32>(5);
    drop(p);
    match c.try_recv() {
      Err(TryRecvError::Disconnected) => {}
      other => panic!(
        "Expected TryRecvError::Disconnected for empty, got {:?}",
        other
      ),
    }
  }

  #[test]
  fn values_are_dropped() {
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
    static DROP_COUNT: AtomicUsize = AtomicUsize::new(0);
    #[derive(Debug)]
    struct Droppable(usize, Arc<AtomicUsize>); // Give it an ID and shared counter
    impl Drop for Droppable {
      fn drop(&mut self) {
        // println!("Dropping Droppable({})", self.0);
        self.1.fetch_add(1, AtomicOrdering::Relaxed);
      }
    }

    let drop_counter_arc = Arc::new(AtomicUsize::new(0));

    // Test 1: Items consumed
    drop_counter_arc.store(0, AtomicOrdering::Relaxed);
    {
      let (mut p, mut c) = bounded_sync::<Droppable>(2);
      p.send(Droppable(1, drop_counter_arc.clone())).unwrap();
      p.send(Droppable(2, drop_counter_arc.clone())).unwrap();
      assert_eq!(drop_counter_arc.load(AtomicOrdering::Relaxed), 0);
      drop(p);

      let d1 = c.recv().unwrap();
      assert_eq!(drop_counter_arc.load(AtomicOrdering::Relaxed), 0);
      drop(d1);
      assert_eq!(drop_counter_arc.load(AtomicOrdering::Relaxed), 1);

      let d2 = c.recv().unwrap();
      assert_eq!(drop_counter_arc.load(AtomicOrdering::Relaxed), 1);
      drop(d2);
      assert_eq!(drop_counter_arc.load(AtomicOrdering::Relaxed), 2);
    } // c drops here
    assert_eq!(drop_counter_arc.load(AtomicOrdering::Relaxed), 2); // Should not change after c drops

    // Test 2: Items left in channel when SpscShared drops
    drop_counter_arc.store(0, AtomicOrdering::Relaxed);
    {
      let (mut p, c) = bounded_sync::<Droppable>(2);
      p.send(Droppable(3, drop_counter_arc.clone())).unwrap();
      p.send(Droppable(4, drop_counter_arc.clone())).unwrap();
      drop(p); // Producer drops
      drop(c); // Consumer drops, items 3 & 4 should be dropped by SpscShared::drop
    }
    assert_eq!(drop_counter_arc.load(AtomicOrdering::Relaxed), 2); // Items 3 and 4 dropped

    // Test 3: Producer sends, then drops. Consumer drops without consuming.
    drop_counter_arc.store(0, AtomicOrdering::Relaxed);
    {
      let (mut p, c) = bounded_sync::<Droppable>(1);
      p.send(Droppable(5, drop_counter_arc.clone())).unwrap();
      drop(p); // Item 5 is in channel.
      drop(c); // Consumer drops. Item 5 should be dropped by SpscShared.
    }
    assert_eq!(drop_counter_arc.load(AtomicOrdering::Relaxed), 1);
  }

  #[test]
  fn recv_timeout_empty_times_out() {
    let (_p, mut c) = bounded_sync::<i32>(1);
    let res = c.recv_timeout(Duration::from_millis(50));
    assert!(matches!(res, Err(RecvErrorTimeout::Timeout)));
  }

  #[test]
  fn recv_timeout_item_arrives() {
    let (mut p, mut c) = bounded_sync::<i32>(1);
    let val_to_send = 123;

    let consumer_handle = thread::spawn(move || c.recv_timeout(Duration::from_secs(1)).unwrap());

    thread::sleep(Duration::from_millis(50)); // Ensure consumer blocks
    p.send(val_to_send).unwrap();

    assert_eq!(consumer_handle.join().unwrap(), val_to_send);
  }

  #[test]
  fn recv_timeout_producer_drops_empty() {
    let (p, mut c) = bounded_sync::<i32>(1);
    drop(p);
    let res = c.recv_timeout(Duration::from_millis(50));
    assert!(matches!(res, Err(RecvErrorTimeout::Disconnected)));
  }

  #[test]
  fn recv_timeout_producer_drops_with_item() {
    let (mut p, mut c) = bounded_sync::<i32>(1);
    p.send(99).unwrap();
    drop(p);
    assert_eq!(c.recv_timeout(Duration::from_millis(50)).unwrap(), 99);
    assert!(matches!(
      c.recv_timeout(Duration::from_millis(50)),
      Err(RecvErrorTimeout::Disconnected)
    ));
  }
}
