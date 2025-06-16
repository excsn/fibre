use crate::error::{
  CloseError, RecvError, RecvErrorTimeout, SendError, TryRecvError, TrySendError,
};
use crate::spsc::shared::SpscShared;
use crate::sync_util;

use std::marker::PhantomData;
use std::sync::atomic::{self, AtomicBool, Ordering};
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
  pub(crate) closed: AtomicBool,
  // This PhantomData makes BoundedSyncSender<T> !Sync, which is appropriate
  // as only one thread should use the producer.
  pub(crate) _phantom: PhantomData<*mut ()>,
}

/// The synchronous receiving end of a bounded SPSC channel.
#[derive(Debug)]
pub struct BoundedSyncReceiver<T> {
  pub(crate) shared: Arc<SpscShared<T>>,
  pub(crate) closed: AtomicBool,
  // This PhantomData makes BoundedSyncReceiver<T> !Sync.
  pub(crate) _phantom: PhantomData<*mut ()>,
}

unsafe impl<T: Send> Send for BoundedSyncSender<T> {}
unsafe impl<T: Send> Send for BoundedSyncReceiver<T> {}

// Methods that do not require T: Send
impl<T> BoundedSyncSender<T> {
  fn close_internal(&self) {
    self.shared.producer_dropped.store(true, Ordering::Release);
    atomic::fence(Ordering::SeqCst);
    self.shared.wake_consumer();
  }
}

// Methods that require T: Send for the channel to be usable
impl<T: Send> BoundedSyncSender<T> {
  /// Crate-internal constructor, used for creating from a shared core, e.g., in tests or by async part.
  pub(crate) fn from_shared(shared: Arc<SpscShared<T>>) -> Self {
    Self {
      shared,
      closed: AtomicBool::new(false),
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
  pub fn try_send(&self, item: T) -> Result<(), TrySendError<T>> {
    if self.closed.load(Ordering::Relaxed) {
      return Err(TrySendError::Closed(item));
    }
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
    self
      .shared
      .head
      .store(head.wrapping_add(1), Ordering::Release);

    self.shared.wake_consumer();
    Ok(())
  }

  /// Sends an item into the channel, blocking the current thread if the channel is full.
  ///
  /// # Errors
  ///
  /// - `Err(SendError::Closed)` if the consumer has been dropped.
  pub fn send(&self, mut item: T) -> Result<(), SendError> {
    if self.closed.load(Ordering::Relaxed) {
      return Err(SendError::Closed);
    }
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
          self
            .shared
            .producer_parked_sync_flag
            .store(true, Ordering::Release);

          let head_after_flag_set = self.shared.head.load(Ordering::Relaxed);
          let tail_after_flag_set = self.shared.tail.load(Ordering::Acquire);

          if !self
            .shared
            .is_full(head_after_flag_set, tail_after_flag_set)
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
        }
        Err(TrySendError::Closed(_returned_item)) => {
          return Err(SendError::Closed);
        }
        Err(TrySendError::Sent(_)) => unreachable!("SPSC try_send should not return Sent variant"),
      }
    }
  }

  /// Closes the sending end of the channel.
  /// This is an explicit alternative to `drop`. If the channel is not already
  /// closed, this will signal to the receiver that no more messages will be sent.
  pub fn close(&self) -> Result<(), CloseError> {
    if self
      .closed
      .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
      .is_ok()
    {
      self.close_internal();
      Ok(())
    } else {
      Err(CloseError)
    }
  }

  /// Returns `true` if the receiver has been dropped.
  pub fn is_closed(&self) -> bool {
    self.closed.load(Ordering::Relaxed) || self.shared.consumer_dropped.load(Ordering::Acquire)
  }

  /// Returns the total capacity of the channel.
  pub fn capacity(&self) -> usize {
    self.shared.capacity
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
    if !self.closed.swap(true, Ordering::AcqRel) {
      self.close_internal();
    }
  }
}

// Methods that do not require T: Send
impl<T> BoundedSyncReceiver<T> {
  fn close_internal(&self) {
    self.shared.consumer_dropped.store(true, Ordering::Release);
    atomic::fence(Ordering::SeqCst);
    self.shared.wake_producer();
  }
}

// Methods that require T: Send for the channel to be usable
impl<T: Send> BoundedSyncReceiver<T> {
  /// Crate-internal constructor.
  pub(crate) fn from_shared(shared: Arc<SpscShared<T>>) -> Self {
    Self {
      shared,
      closed: AtomicBool::new(false),
      _phantom: PhantomData,
    }
  }

  /// Converts this synchronous SPSC consumer into an asynchronous one.
  ///
  /// This is a zero-cost conversion. The `Drop` implementation of the
  /// `BoundedSyncReceiver` will not be called.
  pub fn to_async(self) -> AsyncBoundedSpscReceiver<T> {
    let shared = unsafe { std::ptr::read(&self.shared) };
    mem::forget(self);
    AsyncBoundedSpscReceiver::from_shared(shared)
  }

  /// Attempts to receive an item from the channel without blocking.
  ///
  /// # Errors
  ///
  /// - `Ok(T)` if an item was received.
  /// - `Err(TryRecvError::Empty)` if the channel is empty but the producer is alive.
  /// - `Err(TryRecvError::Disconnected)` if the producer has been dropped and the channel is empty.
  pub fn try_recv(&self) -> Result<T, TryRecvError> {
    if self.closed.load(Ordering::Relaxed) {
      return Err(TryRecvError::Disconnected);
    }
    let tail = self.shared.tail.load(Ordering::Relaxed);
    let head = self.shared.head.load(Ordering::Acquire);

    if self.shared.is_empty(head, tail) {
      if self.shared.producer_dropped.load(Ordering::Acquire) {
        let final_head = self.shared.head.load(Ordering::Acquire);
        if final_head == tail {
          return Err(TryRecvError::Disconnected);
        }
        let slot_idx = tail % self.shared.capacity;
        let item = unsafe { (*self.shared.buffer[slot_idx].get()).assume_init_read() };
        self
          .shared
          .tail
          .store(tail.wrapping_add(1), Ordering::Release);
        self.shared.wake_producer();
        return Ok(item);
      } else {
        return Err(TryRecvError::Empty);
      }
    }

    let slot_idx = tail % self.shared.capacity;
    let item = unsafe { (*self.shared.buffer[slot_idx].get()).assume_init_read() };
    self
      .shared
      .tail
      .store(tail.wrapping_add(1), Ordering::Release);
    self.shared.wake_producer();
    Ok(item)
  }

  /// Receives an item from the channel, blocking the current thread if the channel is empty.
  ///
  /// # Errors
  ///
  /// - `Err(RecvError::Disconnected)` if the producer has been dropped and the channel is empty.
  pub fn recv(&self) -> Result<T, RecvError> {
    if self.closed.load(Ordering::Relaxed) {
      return Err(RecvError::Disconnected);
    }
    loop {
      match self.try_recv() {
        Ok(item) => return Ok(item),
        Err(TryRecvError::Empty) => {
          unsafe {
            *self.shared.consumer_thread_sync.get() = Some(thread::current());
          }
          self
            .shared
            .consumer_parked_sync_flag
            .store(true, Ordering::Release);

          let head_after_flag_set = self.shared.head.load(Ordering::Acquire);
          let tail_after_flag_set = self.shared.tail.load(Ordering::Relaxed);

          if !self
            .shared
            .is_empty(head_after_flag_set, tail_after_flag_set)
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
  pub fn recv_timeout(&mut self, timeout: Duration) -> Result<T, RecvErrorTimeout> {
    if self.closed.load(Ordering::Relaxed) {
      return Err(RecvErrorTimeout::Disconnected);
    }
    let start_time = Instant::now();
    loop {
      match self.try_recv() {
        Ok(item) => return Ok(item),
        Err(TryRecvError::Empty) => {
          let elapsed = start_time.elapsed();
          if elapsed >= timeout {
            return Err(RecvErrorTimeout::Timeout);
          }
          let remaining_timeout = timeout - elapsed;

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
            continue;
          }

          sync_util::park_thread_timeout(remaining_timeout);

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
        Err(TryRecvError::Disconnected) => return Err(RecvErrorTimeout::Disconnected),
      }
    }
  }

  /// Closes the receiving end of the channel.
  /// This is an explicit alternative to `drop`. If the channel is not already
  /// closed, this will signal to the sender that the channel is closed.
  pub fn close(&self) -> Result<(), CloseError> {
    if self
      .closed
      .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
      .is_ok()
    {
      self.close_internal();
      Ok(())
    } else {
      Err(CloseError)
    }
  }

  /// Returns `true` if the producer has been dropped and the channel is empty.
  pub fn is_closed(&self) -> bool {
    self.closed.load(Ordering::Relaxed)
      || (self.shared.producer_dropped.load(Ordering::Acquire) && self.is_empty())
  }

  /// Returns the total capacity of the channel.
  pub fn capacity(&self) -> usize {
    self.shared.capacity
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
    if !self.closed.swap(true, Ordering::AcqRel) {
      self.close_internal();
    }
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
      closed: AtomicBool::new(false),
      _phantom: PhantomData,
    },
    BoundedSyncReceiver {
      shared: shared_arc,
      closed: AtomicBool::new(false),
      _phantom: PhantomData,
    },
  )
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::{thread, time::Duration};

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
    let (p, mut c) = bounded_sync::<i32>(1);
    p.send(1).unwrap(); // Fill channel
    assert_eq!(p.len(), 1);

    let producer_thread = thread::spawn(move || {
      p.send(2).unwrap(); // This should block
      assert_eq!(p.len(), 1); // After send, it's full again before consumer gets it
      p // Return producer to drop it in main thread, ensuring its Drop runs
    });

    thread::sleep(Duration::from_millis(100)); // Give producer time to block

    assert_eq!(c.recv().unwrap(), 1); // Unblock producer
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
    let (mut p, mut c) = bounded_sync::<i32>(1);
    p.send(7).unwrap();
    assert_eq!(p.len(), 1);
    drop(p);
    assert_eq!(c.len(), 1);
    assert_eq!(c.recv().unwrap(), 7);
    assert_eq!(c.len(), 0);
    match c.recv() {
      Err(RecvError::Disconnected) => {}
      Ok(v) => panic!("Expected Disconnected error, got Ok({:?})", v),
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
      Ok(v) => panic!("Expected Disconnected error, got Ok({:?})", v),
    }
  }

  #[test]
  fn consumer_drop_signals_producer() {
    let (mut p, c) = bounded_sync::<i32>(1);
    assert_eq!(p.len(), 0);
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
    let (mut p, mut c) = bounded_sync::<i32>(5);
    p.try_send(10).unwrap();
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
      other => panic!("Expected TryRecvError::Disconnected, got {:?}", other),
    }
  }

  #[test]
  fn values_are_dropped() {
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
    static DROP_COUNT: AtomicUsize = AtomicUsize::new(0);
    #[derive(Debug)]
    struct Droppable(usize, Arc<AtomicUsize>);
    impl Drop for Droppable {
      fn drop(&mut self) {
        self.1.fetch_add(1, AtomicOrdering::Relaxed);
      }
    }

    let drop_counter_arc = Arc::new(AtomicUsize::new(0));
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
    }
    assert_eq!(drop_counter_arc.load(AtomicOrdering::Relaxed), 2);

    drop_counter_arc.store(0, AtomicOrdering::Relaxed);
    {
      let (mut p, c) = bounded_sync::<Droppable>(2);
      p.send(Droppable(3, drop_counter_arc.clone())).unwrap();
      p.send(Droppable(4, drop_counter_arc.clone())).unwrap();
      drop(p);
      drop(c);
    }
    assert_eq!(drop_counter_arc.load(AtomicOrdering::Relaxed), 2);

    drop_counter_arc.store(0, AtomicOrdering::Relaxed);
    {
      let (mut p, c) = bounded_sync::<Droppable>(1);
      p.send(Droppable(5, drop_counter_arc.clone())).unwrap();
      drop(p);
      drop(c);
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

  #[test]
  fn new_spsc_apis_capacity_close_is_closed() {
    let (mut p, c) = bounded_sync::<i32>(5);
    assert_eq!(p.capacity(), 5);
    assert_eq!(c.capacity(), 5);
    assert!(!p.is_closed());
    assert!(!c.is_closed());

    // Close the sender
    p.close().unwrap();
    assert!(p.is_closed());
    assert_eq!(p.close(), Err(CloseError)); // Idempotent
    assert_eq!(p.send(1), Err(SendError::Closed)); // Cannot send on closed handle

    // Receiver should see the channel is now closed
    assert!(c.is_closed());
    assert_eq!(c.recv(), Err(RecvError::Disconnected));

    // Test receiver closing
    let (p, c) = bounded_sync::<i32>(5);
    c.close().unwrap();
    assert!(c.is_closed());
    assert_eq!(c.close(), Err(CloseError));
    assert_eq!(c.recv(), Err(RecvError::Disconnected));

    // Sender should see the channel is now closed
    assert!(p.is_closed());
    assert_eq!(p.send(1), Err(SendError::Closed));
  }
}
