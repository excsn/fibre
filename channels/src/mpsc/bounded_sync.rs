//! The core implementation and synchronous API for the bounded MPSC channel.

use crate::coord::CapacityGate;
use crate::error::{
  BatchSendErrorReason, CloseError, RecvError, SendBatchError, SendError, TryRecvError,
  TrySendBatchError, TrySendError,
};
use crate::mpsc::unbounded_v2;
use crate::{sync_util, RecvErrorTimeout};

use std::mem;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

// Import the async types for `to_async` conversions.
use super::bounded_async::{AsyncReceiver, AsyncSender};

// --- Internal Message & Permit ---

/// A private RAII guard for a capacity permit.
/// When this is dropped, a permit is released back to the gate.
#[derive(Debug)]
pub(crate) struct Permit {
  pub(crate) gate: Arc<CapacityGate>,
  // This flag prevents releasing a permit for zero-capacity (rendezvous) channels.
  pub(crate) is_rendezvous: bool,
}

impl Drop for Permit {
  fn drop(&mut self) {
    // Only release the permit if it's not a rendezvous channel.
    // In a rendezvous, the permit represents the receiver's readiness and is
    // consumed by the sender, not returned to the pool.
    if !self.is_rendezvous {
      self.gate.release();
    }
  }
}

impl Permit {
  /// Defuses the RAII drop and returns the parts, so batch receive can issue
  /// a single `gate.release_many(k)` instead of `k` individually locked
  /// `release()` calls.
  pub(crate) fn into_parts(self) -> (Arc<CapacityGate>, bool /* is_rendezvous */) {
    let this = std::mem::ManuallyDrop::new(self);
    // SAFETY: `this` is wrapped in ManuallyDrop, so `Permit::drop` never runs
    // and the field is read out exactly once.
    let gate = unsafe { std::ptr::read(&this.gate) };
    (gate, this.is_rendezvous)
  }
}

/// Unwraps a batch of received `BoundedMessage`s: appends the values to `out`
/// and returns all non-rendezvous permits to the gate in a single bulk
/// release. Returns the number of values appended.
pub(crate) fn unwrap_batch_messages<T: Send>(
  gate: &CapacityGate,
  msgs: Vec<BoundedMessage<T>>,
  out: &mut Vec<T>,
) -> usize {
  let k = msgs.len();
  let mut to_release = 0usize;
  out.reserve(k);
  for msg in msgs {
    out.push(msg.value);
    let (_gate, is_rendezvous) = msg._permit.into_parts();
    if !is_rendezvous {
      to_release += 1;
    }
  }
  if to_release > 0 {
    gate.release_many(to_release);
  }
  k
}

/// The internal message type that travels through the underlying unbounded channel.
/// It bundles the user's value with the capacity permit.
pub(crate) struct BoundedMessage<T> {
  pub(crate) value: T,
  pub(crate) _permit: Permit,
}

// --- Shared State ---

/// The shared state for the bounded MPSC channel.
/// This is wrapped in an Arc and shared between the sender(s) and receiver.
#[derive(Debug)]
pub(crate) struct BoundedMpscShared<T: Send> {
  pub(crate) gate: Arc<CapacityGate>,
  // The underlying lock-free channel that transports BoundedMessage<T>
  pub(crate) channel: Arc<unbounded_v2::MpscShared<BoundedMessage<T>>>,
}

// --- Public Channel Handles (Sync) ---

#[derive(Debug)]
pub struct Sender<T: Send> {
  pub(crate) shared: Arc<BoundedMpscShared<T>>,
  pub(crate) closed: AtomicBool,
}

#[derive(Debug)]
pub struct Receiver<T: Send> {
  pub(crate) shared: Arc<BoundedMpscShared<T>>,
  pub(crate) closed: AtomicBool,
}

// --- Sender Implementation (Sync) ---

impl<T: Send> Sender<T> {
  /// Sends a value, blocking the current thread until a slot is available
  /// in the channel if it is full.
  pub fn send(&self, value: T) -> Result<(), SendError> {
    if self.closed.load(Ordering::Relaxed)
      || self.shared.channel.receiver_dropped.load(Ordering::Acquire)
    {
      return Err(SendError::Closed);
    }

    // Block until a permit is available. For capacity 0, this waits for a receiver.
    self.shared.gate.acquire_sync();

    let permit = Permit {
      gate: self.shared.gate.clone(),
      is_rendezvous: self.capacity() == 0,
    };
    let message = BoundedMessage {
      value,
      _permit: permit,
    };

    // The underlying send is lock-free and won't block.
    // It can only fail if the receiver was dropped.
    let mut cache = None;
    if unbounded_v2::send_internal(&self.shared.channel, message, &mut cache).is_err() {
      // The receiver was dropped. The `Permit` inside our `message` is dropped here.
      // For capacity > 0, this correctly releases the permit.
      // For capacity == 0, it does nothing, which is also correct.
      return Err(SendError::Closed);
    }

    Ok(())
  }

  /// Attempts to send a value into the channel without blocking.
  pub fn try_send(&self, value: T) -> Result<(), TrySendError<T>> {
    if self.closed.load(Ordering::Relaxed)
      || self.shared.channel.receiver_dropped.load(Ordering::Acquire)
    {
      return Err(TrySendError::Closed(value));
    }

    // Try to acquire a permit non-blockingly.
    if !self.shared.gate.try_acquire() {
      return Err(TrySendError::Full(value));
    }

    let permit = Permit {
      gate: self.shared.gate.clone(),
      is_rendezvous: self.capacity() == 0,
    };
    let message = BoundedMessage {
      value,
      _permit: permit,
    };

    let mut cache = None;
    if let Err(msg) = unbounded_v2::send_internal(&self.shared.channel, message, &mut cache) {
      // Receiver dropped, `msg._permit` is dropped, releasing the gate slot.
      return Err(TrySendError::Closed(msg.value));
    }

    Ok(())
  }

  /// Attempts to send a batch without blocking, taking ownership of the
  /// vector. Acquires up to `items.len()` permits from the capacity gate in
  /// one call, sends that many items, and reports the rest as unsent.
  ///
  /// `Ok(n)` means every item was sent. On a partial send, returns
  /// [`TrySendBatchError`] with the count sent and the unsent remainder
  /// (`reason: Full` if the channel ran out of capacity, `Closed` if the
  /// receiver dropped before anything was sent).
  pub fn try_send_batch(&self, items: Vec<T>) -> Result<usize, TrySendBatchError<T>> {
    let total = items.len();
    if total == 0 {
      return Ok(0);
    }
    if self.closed.load(Ordering::Relaxed)
      || self.shared.channel.receiver_dropped.load(Ordering::Acquire)
    {
      return Err(TrySendBatchError {
        sent: 0,
        unsent: items,
        reason: BatchSendErrorReason::Closed,
      });
    }

    let k = self.shared.gate.try_acquire_many(total);
    if k == 0 {
      return Err(TrySendBatchError {
        sent: 0,
        unsent: items,
        reason: BatchSendErrorReason::Full,
      });
    }
    // A closed gate grants permits immediately so callers can observe
    // closure; re-check before consuming any item.
    if self.shared.channel.receiver_dropped.load(Ordering::Acquire) {
      return Err(TrySendBatchError {
        sent: 0,
        unsent: items,
        reason: BatchSendErrorReason::Closed,
      });
    }

    let mut iter = items.into_iter();
    self.push_batch_messages(&mut iter, k);
    if k == total {
      Ok(total)
    } else {
      Err(TrySendBatchError {
        sent: k,
        unsent: iter.collect(),
        reason: BatchSendErrorReason::Full,
      })
    }
  }

  /// Sends a batch, blocking whenever the channel is full, until every item
  /// is sent or the receiver drops. Permits are acquired in bulk; each chunk
  /// is written with a single queue-length update and a single consumer wake.
  pub fn send_batch(&self, items: Vec<T>) -> Result<usize, SendBatchError<T>> {
    let total = items.len();
    if total == 0 {
      return Ok(0);
    }
    if self.closed.load(Ordering::Relaxed)
      || self.shared.channel.receiver_dropped.load(Ordering::Acquire)
    {
      return Err(SendBatchError {
        sent: 0,
        unsent: items,
      });
    }

    let mut iter = items.into_iter();
    let mut sent = 0;
    while sent < total {
      let k = self.shared.gate.acquire_many_sync(total - sent);
      // A closed gate grants `max` immediately; detect receiver drop here.
      if self.shared.channel.receiver_dropped.load(Ordering::Acquire) {
        return Err(SendBatchError {
          sent,
          unsent: iter.collect(),
        });
      }
      self.push_batch_messages(&mut iter, k);
      sent += k;
    }
    Ok(total)
  }

  /// Attempts to send a batch in place without blocking, draining sent items
  /// from the front of `items`. Returns `Ok(k)` with the count sent (`0` if
  /// no capacity was available); `Err(SendError::Closed)` only if the channel
  /// is closed and zero items were sent by this call.
  pub fn try_send_batch_mut(&self, items: &mut Vec<T>) -> Result<usize, SendError> {
    if items.is_empty() {
      return Ok(0);
    }
    if self.closed.load(Ordering::Relaxed)
      || self.shared.channel.receiver_dropped.load(Ordering::Acquire)
    {
      return Err(SendError::Closed);
    }
    let k = self.shared.gate.try_acquire_many(items.len());
    if k == 0 {
      return Ok(0);
    }
    if self.shared.channel.receiver_dropped.load(Ordering::Acquire) {
      return Err(SendError::Closed);
    }
    let mut drain = items.drain(..k);
    self.push_batch_messages(&mut drain, k);
    drop(drain);
    Ok(k)
  }

  /// Sends a batch in place, blocking whenever the channel is full, draining
  /// sent items from the front of `items`. On `Err(SendError::Closed)`, the
  /// unsent items remain in `items`.
  pub fn send_batch_mut(&self, items: &mut Vec<T>) -> Result<usize, SendError> {
    if items.is_empty() {
      return Ok(0);
    }
    if self.closed.load(Ordering::Relaxed)
      || self.shared.channel.receiver_dropped.load(Ordering::Acquire)
    {
      return Err(SendError::Closed);
    }
    let mut sent = 0;
    while !items.is_empty() {
      let k = self.shared.gate.acquire_many_sync(items.len());
      if self.shared.channel.receiver_dropped.load(Ordering::Acquire) {
        return Err(SendError::Closed);
      }
      let mut drain = items.drain(..k);
      self.push_batch_messages(&mut drain, k);
      drop(drain);
      sent += k;
    }
    Ok(sent)
  }

  /// Wraps the next `k` items from `iter` in `BoundedMessage`s (each carrying
  /// its own RAII permit) and pushes them through the underlying unbounded
  /// channel with one length update and one consumer wake.
  fn push_batch_messages(&self, iter: &mut impl Iterator<Item = T>, k: usize) {
    let is_rendezvous = self.capacity() == 0;
    let shared = &self.shared;
    let mut msg_iter = iter.by_ref().map(|value| BoundedMessage {
      value,
      _permit: Permit {
        gate: shared.gate.clone(),
        is_rendezvous,
      },
    });
    let mut cache = None;
    unbounded_v2::send_batch_internal(&shared.channel, &mut msg_iter, k, &mut cache);
  }

  /// Closes this sender handle.
  ///
  /// This is an explicit alternative to `drop`. If this is the last sender handle,
  /// the channel will become disconnected from the receiver's perspective.
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

  fn close_internal(&self) {
    if self
      .shared
      .channel
      .sender_count
      .fetch_sub(1, Ordering::AcqRel)
      == 1
    {
      // This was the last sender, wake the consumer to signal disconnection.
      self.shared.channel.wake_consumer();
      // Also wake the gate in case the receiver is waiting on a rendezvous
      self.shared.gate.release();
    }
  }

  /// Returns `true` if the receiver has been dropped.
  pub fn is_closed(&self) -> bool {
    self.shared.channel.receiver_dropped.load(Ordering::Acquire)
  }

  pub fn sender_count(&self) -> usize {
    self.shared.channel.sender_count.load(Ordering::Relaxed)
  }

  /// Returns the number of messages currently in the channel.
  pub fn len(&self) -> usize {
    self.shared.channel.current_len.load(Ordering::Relaxed)
  }

  /// Returns `true` if the channel is empty.
  pub fn is_empty(&self) -> bool {
    self.len() == 0
  }

  /// Returns the total capacity of the channel.
  pub fn capacity(&self) -> usize {
    self.shared.gate.capacity()
  }

  /// Returns `true` if the channel is full.
  pub fn is_full(&self) -> bool {
    // For a zero-capacity channel, it's always "full" until a receiver is ready.
    self.len() == self.capacity()
  }

  /// Converts this synchronous `Sender` into an `AsyncSender`.
  pub fn to_async(self) -> AsyncSender<T> {
    let shared = unsafe { std::ptr::read(&self.shared) };
    mem::forget(self);
    AsyncSender {
      shared,
      closed: AtomicBool::new(false),
    }
  }
}

impl<T: Send> Clone for Sender<T> {
  /// Clones this sender.
  ///
  /// A bounded MPSC channel can have multiple producers.
  fn clone(&self) -> Self {
    self
      .shared
      .channel
      .sender_count
      .fetch_add(1, Ordering::Relaxed);
    Self {
      shared: self.shared.clone(),
      closed: AtomicBool::new(false),
    }
  }
}

impl<T: Send> Drop for Sender<T> {
  fn drop(&mut self) {
    if !self.closed.swap(true, Ordering::AcqRel) {
      self.close_internal();
    }
  }
}

// --- Receiver Implementation (Sync) ---

impl<T: Send> Receiver<T> {
  /// Receives a value, blocking the current thread until one is available.
  pub fn recv(&self) -> Result<T, RecvError> {
    if self.closed.load(Ordering::Relaxed) {
      return Err(RecvError::Disconnected);
    }

    // For rendezvous channels, the receiver must signal its readiness
    // by providing a permit for the sender to acquire.
    if self.capacity() == 0 {
      self.shared.gate.release();
    }

    loop {
      match self.try_recv_internal_no_release() {
        Ok(value) => return Ok(value),
        Err(TryRecvError::Disconnected) => return Err(RecvError::Disconnected),
        Err(TryRecvError::Empty) => {}
      }

      let lf_shared = &self.shared.channel;
      *lf_shared.consumer_thread.lock().unwrap() = Some(thread::current());
      lf_shared.consumer_parked.store(true, Ordering::Release);

      match self.try_recv_internal_no_release() {
        Ok(value) => {
          if lf_shared
            .consumer_parked
            .compare_exchange(true, false, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
          {
            *lf_shared.consumer_thread.lock().unwrap() = None;
          }
          return Ok(value);
        }
        Err(TryRecvError::Disconnected) => {
          if lf_shared
            .consumer_parked
            .compare_exchange(true, false, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
          {
            *lf_shared.consumer_thread.lock().unwrap() = None;
          }
          return Err(RecvError::Disconnected);
        }
        Err(TryRecvError::Empty) => {
          sync_util::park_thread();
          if lf_shared
            .consumer_parked
            .compare_exchange(true, false, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
          {
            *lf_shared.consumer_thread.lock().unwrap() = None;
          }
        }
      }
    }
  }

  /// Receives a value, blocking for at most `timeout` duration.
  pub fn recv_timeout(&self, timeout: Duration) -> Result<T, RecvErrorTimeout> {
    if self.closed.load(Ordering::Relaxed) {
      return Err(RecvErrorTimeout::Disconnected);
    }

    let start_time = Instant::now();

    // First, try a non-blocking receive.
    if self.capacity() == 0 {
      self.shared.gate.release();
    }
    match self.try_recv_internal_no_release() {
      Ok(value) => return Ok(value),
      Err(TryRecvError::Disconnected) => return Err(RecvErrorTimeout::Disconnected),
      Err(TryRecvError::Empty) => {} // Continue to blocking path.
    }

    loop {
      let elapsed = start_time.elapsed();
      if elapsed >= timeout {
        return Err(RecvErrorTimeout::Timeout);
      }
      let remaining_timeout = timeout - elapsed;

      if self.capacity() == 0 {
        self.shared.gate.release();
      }

      let lf_shared = &self.shared.channel;
      *lf_shared.consumer_thread.lock().unwrap() = Some(thread::current());
      lf_shared.consumer_parked.store(true, Ordering::Release);

      // Re-check state after arming the parker.
      match self.try_recv_internal_no_release() {
        Ok(value) => {
          if lf_shared
            .consumer_parked
            .compare_exchange(true, false, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
          {
            *lf_shared.consumer_thread.lock().unwrap() = None;
          }
          return Ok(value);
        }
        Err(TryRecvError::Disconnected) => {
          // Disarm parker before returning.
          if lf_shared
            .consumer_parked
            .compare_exchange(true, false, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
          {
            *lf_shared.consumer_thread.lock().unwrap() = None;
          }
          return Err(RecvErrorTimeout::Disconnected);
        }
        Err(TryRecvError::Empty) => {
          // Park with a timeout.
          sync_util::park_thread_timeout(remaining_timeout);
          if lf_shared
            .consumer_parked
            .compare_exchange(true, false, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
          {
            *lf_shared.consumer_thread.lock().unwrap() = None;
          }
        }
      }

      // After waking, try to receive again in the next loop iteration.
      match self.try_recv_internal_no_release() {
        Ok(value) => return Ok(value),
        Err(TryRecvError::Disconnected) => return Err(RecvErrorTimeout::Disconnected),
        Err(TryRecvError::Empty) => {} // Loop again to check timeout.
      }
    }
  }

  // Private helper to avoid calling release in the blocking `recv` loop.
  fn try_recv_internal_no_release(&self) -> Result<T, TryRecvError> {
    if self.closed.load(Ordering::Relaxed) {
      return Err(TryRecvError::Disconnected);
    }
    self.shared.channel.try_recv_internal().map(|msg| msg.value)
  }

  /// Attempts to receive a value without blocking.
  pub fn try_recv(&self) -> Result<T, TryRecvError> {
    if self.closed.load(Ordering::Relaxed) {
      return Err(TryRecvError::Disconnected);
    }
    // For rendezvous, release a permit to signal readiness for a try_send.
    if self.capacity() == 0 {
      // Note: This permit may be "leaked" if no sender is ready, but this is
      // acceptable for `try_recv` semantics. The alternative is much more complex.
      self.shared.gate.release();
    }
    self.shared.channel.try_recv_internal().map(|msg| msg.value)
  }

  // Batch counterpart of `try_recv_internal_no_release`: drains up to `max`
  // messages, unwraps the values into `out`, and returns the permits in one
  // bulk release.
  fn try_recv_batch_internal_no_release(
    &self,
    out: &mut Vec<T>,
    max: usize,
  ) -> Result<usize, TryRecvError> {
    if self.closed.load(Ordering::Relaxed) {
      return Err(TryRecvError::Disconnected);
    }
    let mut msgs = Vec::new();
    let k = self.shared.channel.try_recv_batch_internal(&mut msgs, max)?;
    debug_assert_eq!(k, msgs.len());
    Ok(unwrap_batch_messages(&self.shared.gate, msgs, out))
  }

  /// Attempts to receive up to `max` items without blocking. Returns 1..=max
  /// items in FIFO order, `Err(TryRecvError::Empty)` if none are available,
  /// or `Err(TryRecvError::Disconnected)` if the channel is empty and all
  /// senders dropped. Capacity permits are returned to the gate in a single
  /// bulk release for the whole batch.
  pub fn try_recv_batch(&self, max: usize) -> Result<Vec<T>, TryRecvError> {
    let mut out = Vec::new();
    self.try_recv_batch_mut(&mut out, max)?;
    Ok(out)
  }

  /// Attempts to receive up to `max` items without blocking, appending them
  /// to the end of `out`. Returns the number of items appended.
  pub fn try_recv_batch_mut(&self, out: &mut Vec<T>, max: usize) -> Result<usize, TryRecvError> {
    if max == 0 {
      return Ok(0);
    }
    if self.closed.load(Ordering::Relaxed) {
      return Err(TryRecvError::Disconnected);
    }
    // For rendezvous, release a permit to signal readiness (same as try_recv).
    if self.capacity() == 0 {
      self.shared.gate.release();
    }
    self.try_recv_batch_internal_no_release(out, max)
  }

  /// Receives up to `max` items, blocking until at least one is available,
  /// then draining up to `max` without further waiting. Returns between 1 and
  /// `max` items in FIFO order.
  pub fn recv_batch(&self, max: usize) -> Result<Vec<T>, RecvError> {
    let mut out = Vec::new();
    self.recv_batch_mut(&mut out, max)?;
    Ok(out)
  }

  /// Receives up to `max` items, blocking until at least one is available,
  /// appending them to the end of `out`. Returns the number appended.
  pub fn recv_batch_mut(&self, out: &mut Vec<T>, max: usize) -> Result<usize, RecvError> {
    if max == 0 {
      return Ok(0);
    }
    if self.closed.load(Ordering::Relaxed) {
      return Err(RecvError::Disconnected);
    }

    // For rendezvous channels, signal readiness once per call (same as `recv`).
    if self.capacity() == 0 {
      self.shared.gate.release();
    }

    loop {
      match self.try_recv_batch_internal_no_release(out, max) {
        Ok(k) => return Ok(k),
        Err(TryRecvError::Disconnected) => return Err(RecvError::Disconnected),
        Err(TryRecvError::Empty) => {}
      }

      // Same park protocol as `recv`.
      let lf_shared = &self.shared.channel;
      *lf_shared.consumer_thread.lock().unwrap() = Some(thread::current());
      lf_shared.consumer_parked.store(true, Ordering::Release);

      match self.try_recv_batch_internal_no_release(out, max) {
        Ok(k) => {
          if lf_shared
            .consumer_parked
            .compare_exchange(true, false, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
          {
            *lf_shared.consumer_thread.lock().unwrap() = None;
          }
          return Ok(k);
        }
        Err(TryRecvError::Disconnected) => {
          if lf_shared
            .consumer_parked
            .compare_exchange(true, false, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
          {
            *lf_shared.consumer_thread.lock().unwrap() = None;
          }
          return Err(RecvError::Disconnected);
        }
        Err(TryRecvError::Empty) => {
          sync_util::park_thread();
          if lf_shared
            .consumer_parked
            .compare_exchange(true, false, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
          {
            *lf_shared.consumer_thread.lock().unwrap() = None;
          }
        }
      }
    }
  }

  /// Closes the receiving end of the channel.
  ///
  /// This is an explicit alternative to `drop`. After this is called, any
  /// `send` attempts will fail. Any buffered items are drained and dropped.
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

  fn close_internal(&self) {
    self
      .shared
      .channel
      .receiver_dropped
      .store(true, Ordering::Release);
    while self.shared.channel.try_recv_internal().is_ok() {}
    // Wake every blocked sender (sync and async) in one pass so none are
    // stranded — async senders won't daisy-chain a permit release on their own.
    self.shared.gate.close();
  }

  /// Returns `true` if all senders have been dropped and the channel is empty.
  pub fn is_closed(&self) -> bool {
    let chan = &self.shared.channel;
    chan.sender_count.load(Ordering::Acquire) == 0 && self.is_empty()
  }

  pub fn sender_count(&self) -> usize {
    self.shared.channel.sender_count.load(Ordering::Relaxed)
  }

  /// Returns the number of messages currently in the channel.
  pub fn len(&self) -> usize {
    self.shared.channel.current_len.load(Ordering::Relaxed)
  }

  /// Returns `true` if the channel is empty.
  pub fn is_empty(&self) -> bool {
    self.len() == 0
  }

  /// Returns the total capacity of the channel.
  pub fn capacity(&self) -> usize {
    self.shared.gate.capacity()
  }

  /// Returns `true` if the channel is full.
  pub fn is_full(&self) -> bool {
    self.len() == self.capacity()
  }

  /// Converts this synchronous `Receiver` into an `AsyncReceiver`.
  pub fn to_async(self) -> AsyncReceiver<T> {
    let shared = unsafe { std::ptr::read(&self.shared) };
    mem::forget(self);
    AsyncReceiver {
      shared,
      closed: AtomicBool::new(false),
    }
  }
}

impl<T: Send> Drop for Receiver<T> {
  fn drop(&mut self) {
    if !self.closed.swap(true, Ordering::AcqRel) {
      self.close_internal();
    }
  }
}
