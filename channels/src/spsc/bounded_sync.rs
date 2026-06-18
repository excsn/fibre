use crate::error::{
  BatchSendErrorReason, CloseError, RecvError, RecvErrorTimeout, SendBatchError, SendError,
  TryRecvError, TrySendBatchError, TrySendError,
};
use crate::spsc::shared::{SpscShared, PARK_CONSUMING, PARK_IDLE, PARK_PARKED};
use crate::sync_util;

use std::marker::PhantomData;
use std::sync::atomic::{self, AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self};
use std::time::{Duration, Instant};

use super::bounded_async::{AsyncBoundedSpscReceiver, AsyncBoundedSpscSender};
use std::mem;

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
          self.park_until_not_full();
        }
        Err(TrySendError::Closed(_returned_item)) => {
          return Err(SendError::Closed);
        }
        Err(TrySendError::Sent(_)) => unreachable!("SPSC try_send should not return Sent variant"),
      }
    }
  }

  /// Parks the producer thread until the channel is no longer full or the
  /// consumer drops. Returns without parking if either condition is already
  /// true after arming the park state machine.
  fn park_until_not_full(&self) {
    unsafe {
      *self.shared.producer_thread_sync.get() = Some(thread::current());
    }
    self
      .shared
      .producer_parked_sync_flag
      .store(PARK_PARKED, Ordering::Release);
    atomic::fence(Ordering::SeqCst);

    let head_after_flag_set = self.shared.head.load(Ordering::Relaxed);
    let tail_after_flag_set = self.shared.tail.load(Ordering::Acquire);

    if !self
      .shared
      .is_full(head_after_flag_set, tail_after_flag_set)
      || self.shared.consumer_dropped.load(Ordering::Acquire)
    {
      // Skip-park: reclaim cell if consumer hasn't started consuming yet.
      if self
        .shared
        .producer_parked_sync_flag
        .compare_exchange(PARK_PARKED, PARK_IDLE, Ordering::AcqRel, Ordering::Relaxed)
        .is_ok()
      {
        unsafe {
          *self.shared.producer_thread_sync.get() = None;
        }
      } else {
        // Consumer won (PARKED→CONSUMING): wait until take() finishes (IDLE).
        // The consumer will call unpark(), depositing a token we absorb next park().
        while self
          .shared
          .producer_parked_sync_flag
          .load(Ordering::Acquire)
          == PARK_CONSUMING
        {
          thread::yield_now();
        }
      }
      return;
    }

    sync_util::park_thread();

    // Post-wakeup: resolve state to IDLE before writing to the cell in the next iteration.
    // Uses Acquire so the consumer's store(IDLE, Release) is visible before we proceed.
    loop {
      match self
        .shared
        .producer_parked_sync_flag
        .load(Ordering::Acquire)
      {
        PARK_IDLE => break,
        PARK_PARKED => {
          // Spurious wakeup: consumer hasn't started. Reclaim the cell.
          if self
            .shared
            .producer_parked_sync_flag
            .compare_exchange(PARK_PARKED, PARK_IDLE, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
          {
            unsafe {
              *self.shared.producer_thread_sync.get() = None;
            }
            break;
          }
          // CAS lost: consumer just transitioned PARKED→CONSUMING; fall to CONSUMING arm.
        }
        PARK_CONSUMING => thread::yield_now(),
        _ => unreachable!(),
      }
    }
  }

  /// Attempts to send a batch of items without blocking, taking ownership of
  /// the input vector.
  ///
  /// `Ok(n)` means every item was sent (`n == items.len()`). If the channel
  /// fills up or closes mid-batch, returns [`TrySendBatchError`] carrying the
  /// number of items sent and the unsent remainder, so no item is silently
  /// dropped. The entire batch is published with a single index update and a
  /// single consumer wake.
  pub fn try_send_batch(&self, items: Vec<T>) -> Result<usize, TrySendBatchError<T>> {
    let total = items.len();
    if total == 0 {
      return Ok(0);
    }
    let mut iter = items.into_iter();
    if self.closed.load(Ordering::Relaxed) || self.shared.consumer_dropped.load(Ordering::Acquire) {
      return Err(TrySendBatchError {
        sent: 0,
        unsent: iter.collect(),
        reason: BatchSendErrorReason::Closed,
      });
    }
    let sent = self.shared.write_batch(&mut iter, total);
    if sent == total {
      Ok(total)
    } else {
      let reason = if self.shared.consumer_dropped.load(Ordering::Acquire) {
        BatchSendErrorReason::Closed
      } else {
        BatchSendErrorReason::Full
      };
      Err(TrySendBatchError {
        sent,
        unsent: iter.collect(),
        reason,
      })
    }
  }

  /// Sends a batch of items, blocking the current thread whenever the channel
  /// is full, until every item is sent or the consumer drops.
  ///
  /// `Ok(n)` means every item was sent. On closure, returns
  /// [`SendBatchError`] carrying the count sent and the unsent remainder.
  pub fn send_batch(&self, items: Vec<T>) -> Result<usize, SendBatchError<T>> {
    let total = items.len();
    if total == 0 {
      return Ok(0);
    }
    let mut iter = items.into_iter();
    if self.closed.load(Ordering::Relaxed) {
      return Err(SendBatchError {
        sent: 0,
        unsent: iter.collect(),
      });
    }
    let mut sent = 0;
    loop {
      if self.shared.consumer_dropped.load(Ordering::Acquire) {
        return Err(SendBatchError {
          sent,
          unsent: iter.collect(),
        });
      }
      sent += self.shared.write_batch(&mut iter, total - sent);
      if sent == total {
        return Ok(total);
      }
      self.park_until_not_full();
    }
  }

  /// Attempts to send a batch in place without blocking, draining successfully
  /// sent items from the front of `items`.
  ///
  /// Returns `Ok(k)` with the number of items sent; `Ok(0)` means the channel
  /// was full (or `items` was empty). Returns `Err(SendError::Closed)` only
  /// if the channel is closed and zero items were sent by this call; unsent
  /// items always remain in `items`.
  pub fn try_send_batch_mut(&self, items: &mut Vec<T>) -> Result<usize, SendError> {
    if items.is_empty() {
      return Ok(0);
    }
    if self.closed.load(Ordering::Relaxed) || self.shared.consumer_dropped.load(Ordering::Acquire) {
      return Err(SendError::Closed);
    }
    let k = self.shared.available_space().min(items.len());
    if k == 0 {
      return Ok(0);
    }
    // The producer is single-threaded, so space can only have grown since the
    // snapshot: write_batch is guaranteed to consume the entire drain.
    let mut drain = items.drain(..k);
    let written = self.shared.write_batch(&mut drain, k);
    debug_assert_eq!(written, k);
    Ok(written)
  }

  /// Sends a batch in place, blocking whenever the channel is full, draining
  /// sent items from the front of `items` until all are sent or the consumer
  /// drops.
  ///
  /// On `Err(SendError::Closed)`, the unsent items remain in `items` (the
  /// count already sent is `original_len - items.len()`). This is the
  /// cancel-safe counterpart of [`send_batch`](Self::send_batch).
  pub fn send_batch_mut(&self, items: &mut Vec<T>) -> Result<usize, SendError> {
    if items.is_empty() {
      return Ok(0);
    }
    if self.closed.load(Ordering::Relaxed) {
      return Err(SendError::Closed);
    }
    let mut sent = 0;
    loop {
      if self.shared.consumer_dropped.load(Ordering::Acquire) {
        return Err(SendError::Closed);
      }
      let k = self.shared.available_space().min(items.len());
      if k > 0 {
        let mut drain = items.drain(..k);
        let written = self.shared.write_batch(&mut drain, k);
        debug_assert_eq!(written, k);
        sent += k;
      }
      if items.is_empty() {
        return Ok(sent);
      }
      self.park_until_not_full();
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
        Err(TryRecvError::Empty) => self.park_until_not_empty(),
        Err(TryRecvError::Disconnected) => {
          return Err(RecvError::Disconnected);
        }
      }
    }
  }

  /// Parks the consumer thread until the channel is no longer empty or the
  /// producer drops. Returns without parking if either condition is already
  /// true after arming the park state machine.
  fn park_until_not_empty(&self) {
    unsafe {
      *self.shared.consumer_thread_sync.get() = Some(thread::current());
    }
    self
      .shared
      .consumer_parked_sync_flag
      .store(PARK_PARKED, Ordering::Release);
    atomic::fence(Ordering::SeqCst);

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
        .compare_exchange(PARK_PARKED, PARK_IDLE, Ordering::AcqRel, Ordering::Relaxed)
        .is_ok()
      {
        unsafe {
          *self.shared.consumer_thread_sync.get() = None;
        }
      } else {
        while self
          .shared
          .consumer_parked_sync_flag
          .load(Ordering::Acquire)
          == PARK_CONSUMING
        {
          thread::yield_now();
        }
      }
      return;
    }
    sync_util::park_thread();
    loop {
      match self
        .shared
        .consumer_parked_sync_flag
        .load(Ordering::Acquire)
      {
        PARK_IDLE => break,
        PARK_PARKED => {
          if self
            .shared
            .consumer_parked_sync_flag
            .compare_exchange(PARK_PARKED, PARK_IDLE, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
          {
            unsafe {
              *self.shared.consumer_thread_sync.get() = None;
            }
            break;
          }
        }
        PARK_CONSUMING => thread::yield_now(),
        _ => unreachable!(),
      }
    }
  }

  /// Attempts to receive up to `max` items without blocking.
  ///
  /// Returns a vector of 1..=max items in FIFO order, `Err(TryRecvError::Empty)`
  /// if no items are available, or `Err(TryRecvError::Disconnected)` if the
  /// channel is empty and the producer has been dropped. The entire batch is
  /// consumed with a single index update and a single producer wake.
  pub fn try_recv_batch(&self, max: usize) -> Result<Vec<T>, TryRecvError> {
    let mut out = Vec::new();
    self.try_recv_batch_mut(&mut out, max)?;
    Ok(out)
  }

  /// Attempts to receive up to `max` items without blocking, appending them
  /// to the end of `out`. Returns the number of items appended (>= 1 on `Ok`,
  /// unless `max == 0`).
  pub fn try_recv_batch_mut(&self, out: &mut Vec<T>, max: usize) -> Result<usize, TryRecvError> {
    if max == 0 {
      return Ok(0);
    }
    if self.closed.load(Ordering::Relaxed) {
      return Err(TryRecvError::Disconnected);
    }
    let k = self.shared.read_batch(out, max);
    if k > 0 {
      return Ok(k);
    }
    if self.shared.producer_dropped.load(Ordering::Acquire) {
      // The producer may have written items right before dropping; re-check
      // so we drain the channel before reporting disconnection.
      let k = self.shared.read_batch(out, max);
      if k > 0 {
        return Ok(k);
      }
      return Err(TryRecvError::Disconnected);
    }
    Err(TryRecvError::Empty)
  }

  /// Receives up to `max` items, blocking the current thread until at least
  /// one item is available, then draining up to `max` without further waiting.
  ///
  /// Returns between 1 and `max` items in FIFO order, or
  /// `Err(RecvError::Disconnected)` if the channel is empty and the producer
  /// has been dropped.
  pub fn recv_batch(&self, max: usize) -> Result<Vec<T>, RecvError> {
    let mut out = Vec::new();
    self.recv_batch_mut(&mut out, max)?;
    Ok(out)
  }

  /// Receives up to `max` items, blocking until at least one is available,
  /// appending them to the end of `out`. Returns the number of items appended.
  pub fn recv_batch_mut(&self, out: &mut Vec<T>, max: usize) -> Result<usize, RecvError> {
    if max == 0 {
      return Ok(0);
    }
    if self.closed.load(Ordering::Relaxed) {
      return Err(RecvError::Disconnected);
    }
    loop {
      match self.try_recv_batch_mut(out, max) {
        Ok(k) => return Ok(k),
        Err(TryRecvError::Empty) => self.park_until_not_empty(),
        Err(TryRecvError::Disconnected) => return Err(RecvError::Disconnected),
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
            .store(PARK_PARKED, Ordering::Release);
          atomic::fence(Ordering::SeqCst);

          let head_after_flag = self.shared.head.load(Ordering::Acquire);
          let tail_after_flag = self.shared.tail.load(Ordering::Relaxed);

          if !self.shared.is_empty(head_after_flag, tail_after_flag)
            || self.shared.producer_dropped.load(Ordering::Acquire)
          {
            if self
              .shared
              .consumer_parked_sync_flag
              .compare_exchange(PARK_PARKED, PARK_IDLE, Ordering::AcqRel, Ordering::Relaxed)
              .is_ok()
            {
              unsafe {
                *self.shared.consumer_thread_sync.get() = None;
              }
            } else {
              while self
                .shared
                .consumer_parked_sync_flag
                .load(Ordering::Acquire)
                == PARK_CONSUMING
              {
                thread::yield_now();
              }
            }
            continue;
          }

          sync_util::park_thread_timeout(remaining_timeout);

          // Resolve state machine before checking elapsed or retrying.
          // If CONSUMING, we spin briefly — recv_timeout's deadline is a lower bound.
          loop {
            match self
              .shared
              .consumer_parked_sync_flag
              .load(Ordering::Acquire)
            {
              PARK_IDLE => break,
              PARK_PARKED => {
                if self
                  .shared
                  .consumer_parked_sync_flag
                  .compare_exchange(PARK_PARKED, PARK_IDLE, Ordering::AcqRel, Ordering::Relaxed)
                  .is_ok()
                {
                  unsafe {
                    *self.shared.consumer_thread_sync.get() = None;
                  }
                  break;
                }
              }
              PARK_CONSUMING => thread::yield_now(),
              _ => unreachable!(),
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
    let (p, c) = bounded_sync(1);
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
    let (p, c) = bounded_sync::<i32>(1);
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
    let (p, c) = bounded_sync::<i32>(1);
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
    let (p, c) = bounded_sync::<i32>(1);

    let consumer_thread = thread::spawn(move || {
      let val = c.recv().unwrap(); // This should block
      assert_eq!(val, 100);
      assert_eq!(c.len(), 0);
      c // Return consumer
    });

    thread::sleep(Duration::from_millis(100)); // Give consumer time to block

    p.send(100).unwrap(); // Unblock consumer
    let _c_returned = consumer_thread.join().unwrap();

    // NOW we know the consumer received it, so the channel is empty
    assert_eq!(p.len(), 0);
  }

  #[test]
  fn producer_drop_signals_consumer() {
    let (p, c) = bounded_sync::<i32>(1);
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
    let (p, c) = bounded_sync::<i32>(1);
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
    let (p, c) = bounded_sync::<i32>(1);
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
    #[cfg(not(miri))]
    const ITEMS: usize = 100_000;
    #[cfg(miri)]
    const ITEMS: usize = 10000;
    const CAPACITY: usize = 128;
    let (p, c) = bounded_sync(CAPACITY);

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
    let (p, c) = bounded_sync::<i32>(5);
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
    let (p, c) = bounded_sync::<i32>(5);
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
    let (p, c) = bounded_sync::<i32>(5);
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
    struct Droppable(Arc<AtomicUsize>);
    impl Drop for Droppable {
      fn drop(&mut self) {
        self.0.fetch_add(1, AtomicOrdering::Relaxed);
      }
    }

    let drop_counter_arc = Arc::new(AtomicUsize::new(0));
    drop_counter_arc.store(0, AtomicOrdering::Relaxed);
    {
      let (p, c) = bounded_sync::<Droppable>(2);
      p.send(Droppable(drop_counter_arc.clone())).unwrap();
      p.send(Droppable(drop_counter_arc.clone())).unwrap();
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
      let (p, c) = bounded_sync::<Droppable>(2);
      p.send(Droppable(drop_counter_arc.clone())).unwrap();
      p.send(Droppable(drop_counter_arc.clone())).unwrap();
      drop(p);
      drop(c);
    }
    assert_eq!(drop_counter_arc.load(AtomicOrdering::Relaxed), 2);

    drop_counter_arc.store(0, AtomicOrdering::Relaxed);
    {
      let (p, c) = bounded_sync::<Droppable>(1);
      p.send(Droppable(drop_counter_arc.clone())).unwrap();
      drop(p);
      drop(c);
    }
    assert_eq!(drop_counter_arc.load(AtomicOrdering::Relaxed), 1);
  }

  #[test]
  #[cfg(not(miri))]
  fn recv_timeout_empty_times_out() {
    let (_p, mut c) = bounded_sync::<i32>(1);
    let res = c.recv_timeout(Duration::from_millis(50));
    assert!(matches!(res, Err(RecvErrorTimeout::Timeout)));
  }

  #[test]
  #[cfg(not(miri))]
  fn recv_timeout_item_arrives() {
    let (p, mut c) = bounded_sync::<i32>(1);
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
  #[cfg(not(miri))]
  fn recv_timeout_producer_drops_with_item() {
    let (p, mut c) = bounded_sync::<i32>(1);
    p.send(99).unwrap();
    drop(p);
    assert_eq!(c.recv_timeout(Duration::from_millis(50)).unwrap(), 99);
    assert!(matches!(
      c.recv_timeout(Duration::from_millis(50)),
      Err(RecvErrorTimeout::Disconnected)
    ));
  }

  #[test]
  #[cfg(not(miri))]
  fn new_spsc_apis_capacity_close_is_closed() {
    let (p, c) = bounded_sync::<i32>(5);
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

  #[test]
  fn sync_sender_unblocks_on_consumer_drop() {
    let (p, c) = bounded_sync(1);
    // Fill the channel capacity
    p.send(1).unwrap();

    let handle = thread::spawn(move || {
      // This send should block
      p.send(2)
    });

    // Give the sender thread some time to park
    thread::sleep(Duration::from_millis(50));
    assert!(!handle.is_finished(), "Sender thread should be blocked");

    // Drop the consumer to trigger unblocking
    drop(c);

    let res = handle.join().expect("Sender thread panicked");
    assert!(
      matches!(res, Err(SendError::Closed)),
      "Expected Err(SendError::Closed), got {:?}",
      res
    );
  }
}
