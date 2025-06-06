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
use std::time::Duration;

/// Internal shared state for the bounded SPSC channel, supporting both sync and async waiters.
// This struct is pub so it can be constructed directly by bounded_async.rs and in tests.
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
      .field("producer_waker_async", &self.producer_waker_async)
      .field(
        "consumer_parked_sync_flag",
        &self.consumer_parked_sync_flag.load(Ordering::Relaxed),
      )
      .field("consumer_waker_async", &self.consumer_waker_async)
      .field("producer_dropped", &self.producer_dropped.load(Ordering::Relaxed))
      .field("consumer_dropped", &self.consumer_dropped.load(Ordering::Relaxed))
      .finish_non_exhaustive()
  }
}

impl<T> SpscShared<T> {
  /// Helper for direct construction, e.g., in bounded_async or tests.
  #[allow(dead_code)] // May not be used if bounded_sync is the only constructor
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
      producer_dropped: AtomicBool::new(false), // Caller will wrap this in Arc and manage present state
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
    if self.consumer_parked_sync_flag.load(Ordering::Relaxed) {
      atomic::fence(Ordering::Acquire);
      if self
        .consumer_parked_sync_flag
        .compare_exchange(true, false, Ordering::AcqRel, Ordering::Relaxed)
        .is_ok()
      {
        if let Some(thread_handle) = unsafe { (*self.consumer_thread_sync.get()).take() } {
          sync_util::unpark_thread(&thread_handle);
        }
      }
    }
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
    if self.producer_dropped.load(Ordering::Relaxed) && self.consumer_dropped.load(Ordering::Relaxed) {
      // Both sides explicitly dropped, this is the expected path for cleanup.
    } else {
      // If Arc count reached zero but flags aren't set, it's unusual for SPSC
      // (implies P/C wrappers didn't run their Drop, or were mem::forgotten).
      // Still, proceed to clean up buffer for safety.
      // eprintln!("Warning: SpscShared dropped without producer/consumer flags being fully set.");
    }

    let head = *self.head.get_mut(); // Safe in Drop with &mut self
    let mut tail = *self.tail.get_mut();

    while tail != head {
      let slot_idx = tail % self.capacity;
      unsafe {
        let slot_ptr = self.buffer[slot_idx].get();
        (*slot_ptr).assume_init_drop();
      }
      tail = tail.wrapping_add(1);
    }
  }
}

#[derive(Debug)]
pub struct BoundedSyncProducer<T> {
  pub(crate) shared: Arc<SpscShared<T>>,
  _phantom: PhantomData<*mut ()>,
}

#[derive(Debug)]
pub struct BoundedSyncConsumer<T> {
  pub(crate) shared: Arc<SpscShared<T>>,
  _phantom: PhantomData<*mut ()>,
}

unsafe impl<T: Send> Send for BoundedSyncProducer<T> {}
unsafe impl<T: Send> Send for BoundedSyncConsumer<T> {}

impl<T> BoundedSyncProducer<T> {
  pub(crate) fn from_shared(shared: Arc<SpscShared<T>>) -> Self {
    Self {
      shared,
      _phantom: PhantomData,
    }
  }
}
impl<T> BoundedSyncConsumer<T> {
  /// Crate-internal constructor for tests or other channel types.
  pub(crate) fn from_shared(shared: Arc<SpscShared<T>>) -> Self {
    Self {
      shared,
      _phantom: PhantomData,
    }
  }
}

pub fn bounded_sync<T>(capacity: usize) -> (BoundedSyncProducer<T>, BoundedSyncConsumer<T>) {
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

impl<T> BoundedSyncProducer<T> {
  pub fn try_send(&mut self, item: T) -> Result<(), TrySendError<T>> {
    if self.shared.consumer_dropped.load(Ordering::Acquire) {
      return Err(TrySendError::Closed(item));
    }

    let head = self.shared.head.load(Ordering::Relaxed);
    let tail = self.shared.tail.load(Ordering::Acquire);

    if self.shared.is_full(head, tail) {
      return Err(TrySendError::Full(item));
    }

    let slot_idx = head % self.shared.capacity;
    unsafe {
      (*self.shared.buffer[slot_idx].get()).write(item);
    }
    self.shared.head.store(head.wrapping_add(1), Ordering::Release);

    self.shared.wake_consumer();
    Ok(())
  }

  pub fn send(&mut self, mut item: T) -> Result<(), SendError> {
    loop {
      if self.shared.consumer_dropped.load(Ordering::Acquire) {
        return Err(SendError::Closed);
      }

      match self.try_send(item) {
        Ok(()) => return Ok(()),
        Err(TrySendError::Full(returned_item)) => {
          item = returned_item;
          unsafe {
            *self.shared.producer_thread_sync.get() = Some(thread::current());
          }
          self.shared.producer_parked_sync_flag.store(true, Ordering::Release);

          let head_after_flag_set = self.shared.head.load(Ordering::Relaxed);
          let tail_after_flag_set = self.shared.tail.load(Ordering::Acquire);

          if !self.shared.is_full(head_after_flag_set, tail_after_flag_set)
            || self.shared.consumer_dropped.load(Ordering::Acquire)
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
            continue;
          }
          sync_util::park_thread();
          if self.shared.producer_parked_sync_flag.load(Ordering::Relaxed) {
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
        }
        Err(TrySendError::Closed(_returned_item)) => {
          return Err(SendError::Closed);
        }
        Err(TrySendError::Sent(_)) => unreachable!("SPSC try_send should not return Sent"),
      }
    }
  }
}

impl<T> Drop for BoundedSyncProducer<T> {
  fn drop(&mut self) {
    self.shared.producer_dropped.store(true, Ordering::Release);
    atomic::fence(Ordering::SeqCst); // Add SeqCst fence
    self.shared.wake_consumer();
  }
}

impl<T> BoundedSyncConsumer<T> {
  pub fn try_recv(&mut self) -> Result<T, TryRecvError> {
    // Load tail first for consumer's primary check
    let tail = self.shared.tail.load(Ordering::Relaxed);
    let head = self.shared.head.load(Ordering::Acquire); // Must see latest producer progress

    if self.shared.is_empty(head, tail) {
      if self.shared.producer_dropped.load(Ordering::Acquire) {
        // Producer is gone. Re-check head to see if an item was sent just before drop.
        let final_head = self.shared.head.load(Ordering::Acquire);
        if final_head == tail {
          // Still empty, definitely disconnected
          return Err(TryRecvError::Disconnected);
        }
        // An item was sent right before producer dropped. Fall through to read it.
        // `head` needs to be this `final_head` for the read.
        // For simplicity, let the main read logic use this updated head.
        // This code path means head_now (which is final_head) > tail.
        let slot_idx = tail % self.shared.capacity;
        let item = unsafe { (*self.shared.buffer[slot_idx].get()).assume_init_read() };
        self.shared.tail.store(tail.wrapping_add(1), Ordering::Release);
        self.shared.wake_producer(); // Wake just in case (though producer is dropped)
        return Ok(item);
      } else {
        return Err(TryRecvError::Empty);
      }
    }

    // If not empty initially:
    let slot_idx = tail % self.shared.capacity;
    let item = unsafe { (*self.shared.buffer[slot_idx].get()).assume_init_read() };
    self.shared.tail.store(tail.wrapping_add(1), Ordering::Release);

    self.shared.wake_producer();
    Ok(item)
  }

  pub fn recv(&mut self) -> Result<T, RecvError> {
    loop {
      match self.try_recv() {
        Ok(item) => return Ok(item),
        Err(TryRecvError::Empty) => {
          unsafe {
            *self.shared.consumer_thread_sync.get() = Some(thread::current());
          }
          self.shared.consumer_parked_sync_flag.store(true, Ordering::Release);

          let head_after_flag_set = self.shared.head.load(Ordering::Acquire);
          let tail_after_flag_set = self.shared.tail.load(Ordering::Relaxed);

          if !self.shared.is_empty(head_after_flag_set, tail_after_flag_set)
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
            continue;
          }
          sync_util::park_thread();
          if self.shared.consumer_parked_sync_flag.load(Ordering::Relaxed) {
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

  pub fn recv_timeout(&mut self, _timeout: Duration) -> Result<T, RecvErrorTimeout> {
    todo!("recv_timeout for SPSC sync");
  }
}

impl<T> Drop for BoundedSyncConsumer<T> {
  fn drop(&mut self) {
    self.shared.consumer_dropped.store(true, Ordering::Release);
    atomic::fence(Ordering::SeqCst); // Add SeqCst fence
    self.shared.wake_producer();
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::{thread, time::Duration};

  // Helper to create SpscShared for tests that mix P/C types
  fn create_test_shared_core<T>(capacity: usize) -> Arc<SpscShared<T>> {
    Arc::new(SpscShared::new_internal(capacity))
  }

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
    p.send(1).unwrap();

    let producer_thread = thread::spawn(move || {
      p.send(2).unwrap();
      p
    });

    thread::sleep(Duration::from_millis(100));

    assert_eq!(c.recv().unwrap(), 1);
    let _p = producer_thread.join().unwrap();
    assert_eq!(c.recv().unwrap(), 2);
  }

  #[test]
  fn recv_blocks_until_send() {
    let (mut p, mut c) = bounded_sync::<i32>(1);

    let consumer_thread = thread::spawn(move || {
      let val = c.recv().unwrap();
      assert_eq!(val, 100);
      c
    });

    thread::sleep(Duration::from_millis(100));

    p.send(100).unwrap();
    let _c = consumer_thread.join().unwrap();
  }

  #[test]
  fn producer_drop_signals_consumer() {
    let (p, mut c) = bounded_sync::<i32>(1);
    drop(p);
    match c.recv() {
      Err(RecvError::Disconnected) => {}
      Ok(v) => panic!("Expected Disconnected error after producer drop, got Ok({:?})", v),
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
  fn producer_drop_after_send_consumer_still_receives() {
    let (mut p, mut c) = bounded_sync::<i32>(1);
    p.send(77).unwrap();
    drop(p);
    assert_eq!(c.recv().unwrap(), 77);
    match c.recv() {
      Err(RecvError::Disconnected) => {}
      Ok(v) => panic!("Expected Disconnected error, got Ok({:?})", v),
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
    let (p, mut c) = bounded_sync::<i32>(5);
    drop(p);
    match c.try_recv() {
      Err(TryRecvError::Disconnected) => {}
      other => panic!("Expected TryRecvError::Disconnected, got {:?}", other),
    }
  }

  #[test]
  fn try_recv_empty_then_disconnected() {
    let (p, mut c) = bounded_sync::<i32>(1);
    assert!(matches!(c.try_recv(), Err(TryRecvError::Empty)));
    drop(p);
    assert!(matches!(c.try_recv(), Err(TryRecvError::Disconnected)));
  }

  #[test]
  fn values_are_dropped() {
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
    static DROP_COUNT: AtomicUsize = AtomicUsize::new(0);
    #[derive(Debug)]
    struct Droppable(usize);
    impl Drop for Droppable {
      fn drop(&mut self) {
        DROP_COUNT.fetch_add(1, AtomicOrdering::Relaxed);
      }
    }

    DROP_COUNT.store(0, AtomicOrdering::Relaxed);
    let (mut p, mut c) = bounded_sync::<Droppable>(2);

    p.send(Droppable(1)).unwrap();
    p.send(Droppable(2)).unwrap();
    assert_eq!(DROP_COUNT.load(AtomicOrdering::Relaxed), 0);

    drop(p); // Producer dropped, items still in channel

    let d1 = c.recv().unwrap();
    assert_eq!(DROP_COUNT.load(AtomicOrdering::Relaxed), 0);
    drop(d1);
    assert_eq!(DROP_COUNT.load(AtomicOrdering::Relaxed), 1);

    let d2 = c.recv().unwrap();
    assert_eq!(DROP_COUNT.load(AtomicOrdering::Relaxed), 1);
    drop(d2);
    assert_eq!(DROP_COUNT.load(AtomicOrdering::Relaxed), 2);

    DROP_COUNT.store(0, AtomicOrdering::Relaxed);
    let (mut p2, c2) = bounded_sync::<Droppable>(1);
    p2.send(Droppable(3)).unwrap(); // Item 3 created
    drop(p2); // Producer dropped, Item 3 still in channel
    drop(c2); // Consumer dropped, Item 3 in channel should be dropped by SpscShared::drop
    assert_eq!(DROP_COUNT.load(AtomicOrdering::Relaxed), 1);
  }
}
