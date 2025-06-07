use crate::error::{RecvError, RecvErrorTimeout, SendError, TryRecvError, TrySendError};
use crate::spsc::shared::SpscShared;
use crate::sync_util;

use std::marker::PhantomData;
use std::sync::atomic::{self, Ordering};
use std::sync::Arc;
use std::thread::{self};
use std::time::{Duration, Instant};

// Import async types for conversion methods
use super::bounded_async::{AsyncBoundedSpscReceiver, AsyncBoundedSpscSender};
use std::mem; // For mem::forget

/// The synchronous sending end of a bounded SPSC channel.
#[derive(Debug)]
pub struct BoundedSyncSender<T> {
  pub(crate) shared: Arc<SpscShared<T>>,
  // This PhantomData makes BoundedSyncSender<T> !Sync, which is appropriate
  // as only one thread should use the producer.
  pub(crate) _phantom: PhantomData<*mut ()>,
}

/// The synchronous receiving end of a bounded SPSC channel.
#[derive(Debug)]
pub struct BoundedSyncReceiver<T> {
  pub(crate) shared: Arc<SpscShared<T>>,
  // This PhantomData makes BoundedSyncReceiver<T> !Sync.
  pub(crate) _phantom: PhantomData<*mut ()>,
}

unsafe impl<T: Send> Send for BoundedSyncSender<T> {}
unsafe impl<T: Send> Send for BoundedSyncReceiver<T> {}

impl<T: Send> BoundedSyncSender<T> {
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
  /// `BoundedSyncSender` will not be called.
  pub fn to_async(self) -> AsyncBoundedSpscSender<T> {
    let shared = unsafe { std::ptr::read(&self.shared) };
    mem::forget(self); // Prevent Drop of BoundedSyncSender
    AsyncBoundedSpscSender::from_shared(shared) // Use async's pub(crate) constructor
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

    let head = self.shared.head.load(Ordering::Relaxed); // Sender owns head updates
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

  /// Returns the number of items currently in the channel.
  #[inline]
  pub fn len(&self) -> usize {
    let head = self.shared.head.load(Ordering::Acquire);
    let tail = self.shared.tail.load(Ordering::Acquire);
    self.shared.current_len(head, tail)
  }

  /// Returns `true` if the channel is currently empty.
  #[inline]
  pub fn is_empty(&self) -> bool {
    let head = self.shared.head.load(Ordering::Acquire);
    let tail = self.shared.tail.load(Ordering::Acquire);
    self.shared.is_empty(head, tail)
  }

  /// Returns `true` if the channel is currently full.
  #[inline]
  pub fn is_full(&self) -> bool {
    let head = self.shared.head.load(Ordering::Acquire);
    let tail = self.shared.tail.load(Ordering::Acquire);
    self.shared.is_full(head, tail)
  }
}

impl<T> Drop for BoundedSyncSender<T> {
  fn drop(&mut self) {
    self.shared.producer_dropped.store(true, Ordering::Release);
    // Ensure the store to producer_dropped is globally visible
    // before the wake can cause the consumer to read it.
    atomic::fence(Ordering::SeqCst);
    self.shared.wake_consumer();
  }
}

impl<T: Send> BoundedSyncReceiver<T> {
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
  /// `BoundedSyncReceiver` will not be called.
  pub fn to_async(self) -> AsyncBoundedSpscReceiver<T> {
    let shared = unsafe { std::ptr::read(&self.shared) };
    mem::forget(self); // Prevent Drop of BoundedSyncReceiver
    AsyncBoundedSpscReceiver::from_shared(shared) // Use async's pub(crate) constructor
  }

  /// Attempts to receive an item from the channel without blocking.
  ///
  /// # Errors
  ///
  /// - `Ok(T)` if an item was received.
  /// - `Err(TryRecvError::Empty)` if the channel is empty but the producer is alive.
  /// - `Err(TryRecvError::Disconnected)` if the producer has been dropped and the channel is empty.
  pub fn try_recv(&self) -> Result<T, TryRecvError> {
    let tail = self.shared.tail.load(Ordering::Relaxed); // Receiver owns tail updates
    let head = self.shared.head.load(Ordering::Acquire); // Must see latest producer writes

    if self.shared.is_empty(head, tail) {
      if self.shared.producer_dropped.load(Ordering::Acquire) {
        // Sender is gone. Re-check head to see if an item was sent just before drop.
        let final_head = self.shared.head.load(Ordering::Acquire);
        if final_head == tail {
          // Still empty even after seeing producer_dropped
          return Err(TryRecvError::Disconnected);
        }
        // An item was sent right before producer dropped. Fall through to read it.
        // Update `head` for the read logic below to use this `final_head`.
        // let head_for_read = final_head; // Not needed, head is already final_head in this path
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
  pub fn recv(&self) -> Result<T, RecvError> {
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

  /// Returns the number of items currently in the channel.
  #[inline]
  pub fn len(&self) -> usize {
    let head = self.shared.head.load(Ordering::Acquire);
    let tail = self.shared.tail.load(Ordering::Acquire);
    self.shared.current_len(head, tail)
  }

  /// Returns `true` if the channel is currently empty.
  #[inline]
  pub fn is_empty(&self) -> bool {
    let head = self.shared.head.load(Ordering::Acquire);
    let tail = self.shared.tail.load(Ordering::Acquire);
    self.shared.is_empty(head, tail)
  }

  /// Returns `true` if the channel is currently full.
  #[inline]
  pub fn is_full(&self) -> bool {
    let head = self.shared.head.load(Ordering::Acquire);
    let tail = self.shared.tail.load(Ordering::Acquire);
    self.shared.is_full(head, tail)
  }
}

impl<T> Drop for BoundedSyncReceiver<T> {
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
pub fn bounded_sync<T: Send>(capacity: usize) -> (BoundedSyncSender<T>, BoundedSyncReceiver<T>) {
  let shared_core = SpscShared::new_internal(capacity);
  let shared_arc = Arc::new(shared_core);

  (
    BoundedSyncSender {
      shared: Arc::clone(&shared_arc),
      _phantom: PhantomData,
    },
    BoundedSyncReceiver {
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
    time::Duration,
  };

  #[test]
  fn create_channel() {
    let (p, c) = bounded_sync::<i32>(1);
    assert_eq!(p.shared.capacity, 1);
    assert_eq!(c.shared.capacity, 1);
    assert!(p.is_empty());
    assert!(c.is_empty());
    assert!(!p.is_full());
    assert!(!c.is_full());
    assert_eq!(p.len(), 0);
    assert_eq!(c.len(), 0);
  }

  #[test]
  #[should_panic]
  fn create_channel_zero_capacity() {
    let (_, _) = bounded_sync::<i32>(0);
  }

  #[test]
  fn send_recv_single_item() {
    let (mut p, mut c) = bounded_sync(1);
    assert!(p.is_empty());
    p.send(42i32).unwrap();
    assert!(p.is_full());
    assert!(!p.is_empty());
    assert_eq!(p.len(), 1);
    assert!(c.is_full());
    assert!(!c.is_empty());
    assert_eq!(c.len(), 1);

    assert_eq!(c.recv().unwrap(), 42);
    assert!(p.is_empty());
    assert!(c.is_empty());
    assert_eq!(p.len(), 0);
    assert_eq!(c.len(), 0);
  }

  #[test]
  fn try_send_full_try_recv_empty() {
    let (mut p, mut c) = bounded_sync::<i32>(1);
    p.try_send(10).unwrap();
    assert!(p.is_full());
    assert_eq!(p.len(), 1);

    match p.try_send(20) {
      Err(TrySendError::Full(val)) => assert_eq!(val, 20),
      res => panic!("Expected Full error, got {:?}", res),
    }
    assert!(p.is_full()); // Still full

    assert_eq!(c.try_recv().unwrap(), 10);
    assert!(c.is_empty());
    assert_eq!(c.len(), 0);

    match c.try_recv() {
      Err(TryRecvError::Empty) => {}
      res => panic!("Expected Empty error, got {:?}", res),
    }
    assert!(c.is_empty());
  }

  #[test]
  fn send_blocks_until_recv() {
    let (mut p, mut c) = bounded_sync::<i32>(1);
    p.send(1).unwrap(); // Fill channel
    assert_eq!(p.len(), 1);

    let producer_thread = thread::spawn(move || {
      p.send(2).unwrap(); // This should block
      assert_eq!(p.len(), 1); // After send, it's full again before consumer gets it
      p // Return producer to drop it in main thread, ensuring its Drop runs
    });

    thread::sleep(Duration::from_millis(100)); // Give producer time to block

    assert_eq!(c.recv().unwrap(), 1); // Unblock producer
    assert_eq!(c.len(), 0); // Locally empty, producer might be sending
    let _p_returned = producer_thread.join().unwrap(); // Sender should now complete
    assert_eq!(c.recv().unwrap(), 2);
    assert_eq!(c.len(), 0);
  }

  #[test]
  fn recv_blocks_until_send() {
    let (mut p, mut c) = bounded_sync::<i32>(1);

    let consumer_thread = thread::spawn(move || {
      let val = c.recv().unwrap(); // This should block
      assert_eq!(val, 100);
      assert_eq!(c.len(), 0);
      c // Return consumer
    });

    thread::sleep(Duration::from_millis(100)); // Give consumer time to block

    p.send(100).unwrap(); // Unblock consumer
    assert_eq!(p.len(), 1); // Item is in
    let _c_returned = consumer_thread.join().unwrap();
  }

  #[test]
  fn producer_drop_signals_consumer() {
    let (mut p, mut c) = bounded_sync::<i32>(1); // p needs to be mut to call send
    p.send(7).unwrap(); // CORRECTED: Call send on p
    assert_eq!(p.len(), 1);
    drop(p); // len is no longer accessible on p
    assert_eq!(c.len(), 1);
    assert_eq!(c.recv().unwrap(), 7);
    assert_eq!(c.len(), 0);
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
    assert_eq!(p.len(), 0);
    drop(p);
    assert_eq!(c.len(), 0);
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
    assert_eq!(p.len(), 0);
    drop(c);
    // After consumer drop, len might be stale or 0.
    // The key is that send fails.
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
      // After consuming all items, the channel should be empty from consumer's perspective,
      // assuming the producer has also finished and not sent more.
      assert!(c.is_empty());
    });

    producer_handle.join().unwrap();
    consumer_handle.join().unwrap();
  }

  #[test]
  fn try_send_closed_by_consumer_drop() {
    let (mut p, c) = bounded_sync::<i32>(5);
    p.try_send(1).unwrap();
    assert_eq!(p.len(), 1);
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
    assert_eq!(p.len(), 1);
    drop(p);
    assert_eq!(c.len(), 1);
    assert_eq!(c.try_recv().unwrap(), 10);
    assert_eq!(c.len(), 0);
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
      assert_eq!(p.len(), 2);
      drop(p);
      assert_eq!(c.len(), 2);

      let d1 = c.recv().unwrap();
      assert_eq!(drop_counter_arc.load(AtomicOrdering::Relaxed), 0);
      assert_eq!(c.len(), 1);
      drop(d1);
      assert_eq!(drop_counter_arc.load(AtomicOrdering::Relaxed), 1);

      let d2 = c.recv().unwrap();
      assert_eq!(drop_counter_arc.load(AtomicOrdering::Relaxed), 1);
      assert_eq!(c.len(), 0);
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
      drop(p); // Sender drops
      drop(c); // Receiver drops, items 3 & 4 should be dropped by SpscShared::drop
    }
    assert_eq!(drop_counter_arc.load(AtomicOrdering::Relaxed), 2); // Items 3 and 4 dropped

    // Test 3: Sender sends, then drops. Receiver drops without consuming.
    drop_counter_arc.store(0, AtomicOrdering::Relaxed);
    {
      let (mut p, c) = bounded_sync::<Droppable>(1);
      p.send(Droppable(5, drop_counter_arc.clone())).unwrap();
      drop(p); // Item 5 is in channel.
      drop(c); // Receiver drops. Item 5 should be dropped by SpscShared.
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
