use futures_core::Stream;

use super::bounded_sync::{BoundedSyncReceiver, BoundedSyncSender};
use crate::error::{
  BatchSendErrorReason, CloseError, RecvError, SendBatchError, SendError, TryRecvError,
  TrySendBatchError, TrySendError,
};
use crate::spsc::shared::SpscShared;

use core::marker::PhantomPinned;
use std::future::Future;
use std::mem;
use std::pin::Pin;
use std::sync::atomic::{self, AtomicBool, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};

// --- Async Sender ---
#[derive(Debug)]
pub struct AsyncBoundedSpscSender<T> {
  pub(crate) shared: Arc<SpscShared<T>>,
  pub(crate) closed: AtomicBool,
}

// --- Async Receiver ---
#[derive(Debug)]
pub struct AsyncBoundedSpscReceiver<T> {
  pub(crate) shared: Arc<SpscShared<T>>,
  pub(crate) closed: AtomicBool,
}

unsafe impl<T: Send> Send for AsyncBoundedSpscSender<T> {}
unsafe impl<T: Send> Send for AsyncBoundedSpscReceiver<T> {}

// Methods that do not require T: Send (e.g., for Drop)
impl<T> AsyncBoundedSpscSender<T> {
  fn close_internal(&self) {
    self.shared.producer_dropped.store(true, Ordering::Release);
    atomic::fence(Ordering::SeqCst);
    self.shared.wake_consumer();
  }
}

// Methods that require T: Send
impl<T: Send> AsyncBoundedSpscSender<T> {
  /// Crate-internal constructor for tests or other channel types.
  pub(crate) fn from_shared(shared: Arc<SpscShared<T>>) -> Self {
    Self {
      shared,
      closed: AtomicBool::new(false),
    }
  }

  /// Converts this asynchronous SPSC producer into a synchronous one.
  pub fn to_sync(self) -> BoundedSyncSender<T> {
    let shared = unsafe { std::ptr::read(&self.shared) };
    mem::forget(self);
    BoundedSyncSender::from_shared(shared)
  }

  /// Sends an item into the channel asynchronously.
  ///
  /// The returned future will complete with `Ok(())` if the send was successful,
  /// or `Err(SendError::Closed)` if the consumer has been dropped.
  pub fn send(&self, item: T) -> SendFuture<'_, T> {
    SendFuture::new(self, item)
  }

  /// Attempts to send an item into the channel without blocking (asynchronously).
  ///
  /// This is a non-blocking operation that returns immediately.
  ///
  /// # Errors
  ///
  /// - `Err(TrySendError::Full(item))` if the channel is full.
  /// - `Err(TrySendError::Closed(item))` if the consumer has been dropped.
  pub fn try_send(&self, item: T) -> Result<(), TrySendError<T>> {
    if self.closed.load(Ordering::Relaxed) {
      return Err(TrySendError::Closed(item));
    }
    let shared = &self.shared;
    if shared.consumer_dropped.load(Ordering::Acquire) {
      return Err(TrySendError::Closed(item));
    }
    let head = shared.head.load(Ordering::Relaxed);
    let tail = shared.tail.load(Ordering::Acquire);
    if shared.is_full(head, tail) {
      return Err(TrySendError::Full(item));
    }
    let slot_idx = head % shared.capacity;
    unsafe {
      (*shared.buffer[slot_idx].get()).write(item);
    }
    shared.head.store(head.wrapping_add(1), Ordering::Release);
    shared.wake_consumer();
    Ok(())
  }

  /// Sends a batch of items asynchronously, taking ownership of the vector.
  ///
  /// The returned future resolves with `Ok(n)` once every item has been sent,
  /// or [`SendBatchError`] (carrying the count sent and the unsent remainder)
  /// if the consumer drops mid-batch.
  ///
  /// Note: if the future is dropped (cancelled) after partial progress, the
  /// already-sent prefix stays in the channel and the unsent remainder is
  /// dropped. Use [`send_batch_mut`](Self::send_batch_mut) for a cancel-safe
  /// variant.
  pub fn send_batch(&self, items: Vec<T>) -> SendBatchFuture<'_, T> {
    let total = items.len();
    SendBatchFuture {
      sender: self,
      iter: items.into_iter(),
      total,
      sent: 0,
      _phantom: PhantomPinned,
    }
  }

  /// Sends a batch of items asynchronously in place, draining sent items from
  /// the front of `items`.
  ///
  /// Cancel-safe: if the future is dropped, every unsent item remains in
  /// `items`. The future resolves with the number of items sent, or
  /// `Err(SendError::Closed)` if the consumer drops (unsent items remain in
  /// `items`).
  pub fn send_batch_mut<'a>(&'a self, items: &'a mut Vec<T>) -> SendBatchMutFuture<'a, T> {
    SendBatchMutFuture {
      sender: self,
      items,
      sent: 0,
      _phantom: PhantomPinned,
    }
  }

  /// Attempts to send a batch of items without blocking, taking ownership of
  /// the input vector. See [`BoundedSyncSender::try_send_batch`] for the full
  /// semantics; this is the same non-blocking operation.
  pub fn try_send_batch(&self, items: Vec<T>) -> Result<usize, TrySendBatchError<T>> {
    let total = items.len();
    if total == 0 {
      return Ok(0);
    }
    let mut iter = items.into_iter();
    if self.closed.load(Ordering::Relaxed) || self.shared.consumer_dropped.load(Ordering::Acquire)
    {
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

  /// Attempts to send a batch in place without blocking, draining sent items
  /// from the front of `items`. Returns `Ok(k)` with the count sent (`0` if
  /// the channel was full); `Err(SendError::Closed)` only if the channel is
  /// closed and zero items were sent by this call.
  pub fn try_send_batch_mut(&self, items: &mut Vec<T>) -> Result<usize, SendError> {
    if items.is_empty() {
      return Ok(0);
    }
    if self.closed.load(Ordering::Relaxed) || self.shared.consumer_dropped.load(Ordering::Acquire)
    {
      return Err(SendError::Closed);
    }
    let k = self.shared.available_space().min(items.len());
    if k == 0 {
      return Ok(0);
    }
    let mut drain = items.drain(..k);
    let written = self.shared.write_batch(&mut drain, k);
    debug_assert_eq!(written, k);
    Ok(written)
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

// Methods that do not require T: Send (e.g., for Drop)
impl<T> AsyncBoundedSpscReceiver<T> {
  fn close_internal(&self) {
    self.shared.consumer_dropped.store(true, Ordering::Release);
    atomic::fence(Ordering::SeqCst);
    self.shared.wake_producer();
  }
}

// Methods that require T: Send
impl<T: Send> AsyncBoundedSpscReceiver<T> {
  /// Crate-internal constructor for tests or other channel types.
  pub(crate) fn from_shared(shared: Arc<SpscShared<T>>) -> Self {
    Self {
      shared,
      closed: AtomicBool::new(false),
    }
  }

  /// Converts this asynchronous SPSC consumer into a synchronous one.
  pub fn to_sync(self) -> BoundedSyncReceiver<T> {
    let shared = unsafe { std::ptr::read(&self.shared) };
    mem::forget(self);
    BoundedSyncReceiver::from_shared(shared)
  }

  /// Receives an item from the channel asynchronously.
  ///
  /// The returned future will complete with `Ok(T)` when an item is received,
  /// or `Err(RecvError::Disconnected)` if the producer has been dropped and
  /// the channel is empty.
  pub fn recv(&self) -> ReceiveFuture<'_, T> {
    ReceiveFuture::new(self)
  }

  /// Attempts to receive an item from the channel without blocking (asynchronously).
  ///
  /// This is a non-blocking operation that returns immediately.
  ///
  /// # Errors
  ///
  /// - `Ok(T)` if an item was successfully received.
  /// - `Err(TryRecvError::Empty)` if the channel is currently empty but the producer is alive.
  /// - `Err(TryRecvError::Disconnected)` if the producer has been dropped and the channel is empty.
  pub fn try_recv(&self) -> Result<T, TryRecvError> {
    if self.closed.load(Ordering::Relaxed) {
      return Err(TryRecvError::Disconnected);
    }
    let shared = &self.shared;
    let tail = shared.tail.load(Ordering::Relaxed);
    let head = shared.head.load(Ordering::Acquire);

    if shared.is_empty(head, tail) {
      if shared.producer_dropped.load(Ordering::Acquire) {
        let final_head = shared.head.load(Ordering::Acquire);
        if final_head == tail {
          return Err(TryRecvError::Disconnected);
        }
        let slot_idx = tail % shared.capacity;
        let item = unsafe { (*shared.buffer[slot_idx].get()).assume_init_read() };
        shared.tail.store(tail.wrapping_add(1), Ordering::Release);
        shared.wake_producer();
        return Ok(item);
      } else {
        return Err(TryRecvError::Empty);
      }
    }

    let slot_idx = tail % shared.capacity;
    let item = unsafe { (*shared.buffer[slot_idx].get()).assume_init_read() };
    shared.tail.store(tail.wrapping_add(1), Ordering::Release);
    shared.wake_producer();
    Ok(item)
  }

  /// Receives up to `max` items asynchronously. The returned future resolves
  /// once at least one item is available, draining up to `max` items without
  /// further waiting (returns between 1 and `max` items in FIFO order), or
  /// with `Err(RecvError::Disconnected)` if the channel is empty and the
  /// producer has been dropped.
  ///
  /// Cancel-safe: items are only removed from the channel in the poll that
  /// resolves the future.
  pub fn recv_batch(&self, max: usize) -> RecvBatchFuture<'_, T> {
    RecvBatchFuture {
      receiver: self,
      max,
    }
  }

  /// Receives up to `max` items asynchronously, appending them to the end of
  /// `out`. The future resolves with the number of items appended once at
  /// least one is available. Cancel-safe.
  pub fn recv_batch_mut<'a>(&'a self, out: &'a mut Vec<T>, max: usize) -> RecvBatchMutFuture<'a, T> {
    RecvBatchMutFuture {
      receiver: self,
      out,
      max,
    }
  }

  /// Attempts to receive up to `max` items without blocking. Returns 1..=max
  /// items in FIFO order, `Err(TryRecvError::Empty)` if no items are
  /// available, or `Err(TryRecvError::Disconnected)` if the channel is empty
  /// and the producer has been dropped.
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

  /// Closes the receiving end of the channel.
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

/// Creates a new asynchronous bounded SPSC channel with the given capacity.
///
/// `capacity` must be greater than 0. Panics if capacity is 0.
pub fn bounded_async<T: Send>(
  capacity: usize,
) -> (AsyncBoundedSpscSender<T>, AsyncBoundedSpscReceiver<T>) {
  let shared_core = SpscShared::new_internal(capacity);
  let shared_arc = Arc::new(shared_core);

  (
    AsyncBoundedSpscSender {
      shared: Arc::clone(&shared_arc),
      closed: AtomicBool::new(false),
    },
    AsyncBoundedSpscReceiver {
      shared: shared_arc,
      closed: AtomicBool::new(false),
    },
  )
}

impl<T> Drop for AsyncBoundedSpscSender<T> {
  fn drop(&mut self) {
    if !self.closed.swap(true, Ordering::AcqRel) {
      self.close_internal();
    }
  }
}

#[must_use = "futures do nothing unless you .await or poll them"]
pub struct SendFuture<'a, T> {
  sender: &'a AsyncBoundedSpscSender<T>,
  item: Option<T>,
  _phantom: PhantomPinned,
}

impl<'a, T> SendFuture<'a, T> {
  fn new(sender: &'a AsyncBoundedSpscSender<T>, item: T) -> Self {
    SendFuture {
      sender,
      item: Some(item),
      _phantom: PhantomPinned,
    }
  }
}

impl<'a, T: Unpin + Send> Future for SendFuture<'a, T> {
  type Output = Result<(), SendError>;

  fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    loop {
      let this = unsafe { self.as_mut().get_unchecked_mut() };
      let shared = &this.sender.shared;

      if this.item.is_none() {
        return Poll::Ready(Ok(()));
      }

      if this.sender.closed.load(Ordering::Relaxed)
        || shared.consumer_dropped.load(Ordering::Acquire)
      {
        this.item = None;
        return Poll::Ready(Err(SendError::Closed));
      }

      let head = shared.head.load(Ordering::Relaxed);
      let tail = shared.tail.load(Ordering::Acquire);

      if !shared.is_full(head, tail) {
        let item_to_send = this.item.take().unwrap();
        let slot_idx = head % shared.capacity;
        unsafe {
          (*shared.buffer[slot_idx].get()).write(item_to_send);
        }
        shared.head.store(head.wrapping_add(1), Ordering::Release);
        shared.wake_consumer();
        return Poll::Ready(Ok(()));
      }

      shared.producer_waker_async.register(cx.waker());

      // Re-check after registration
      if shared.consumer_dropped.load(Ordering::Acquire) {
        continue;
      }
      let head_after_register = shared.head.load(Ordering::Relaxed);
      let tail_after_register = shared.tail.load(Ordering::Acquire);
      if !shared.is_full(head_after_register, tail_after_register) {
        continue;
      }

      return Poll::Pending;
    }
  }
}

impl<'a, T> Drop for SendFuture<'a, T> {
  fn drop(&mut self) {
    // If self.item is Some, it means the send did not complete.
    // The item is dropped here when Option<T> is dropped.
  }
}

/// Future returned by [`AsyncBoundedSpscSender::send_batch`].
///
/// Resolves with `Ok(n)` once all items are sent, or `SendBatchError` on
/// closure. If dropped before completion, the unsent remainder is dropped.
#[must_use = "futures do nothing unless you .await or poll them"]
pub struct SendBatchFuture<'a, T> {
  sender: &'a AsyncBoundedSpscSender<T>,
  iter: std::vec::IntoIter<T>,
  total: usize,
  sent: usize,
  _phantom: PhantomPinned,
}

impl<'a, T: Send> Future for SendBatchFuture<'a, T> {
  type Output = Result<usize, SendBatchError<T>>;

  fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    let this = unsafe { self.as_mut().get_unchecked_mut() };
    let shared = &this.sender.shared;
    loop {
      if this.sent == this.total {
        return Poll::Ready(Ok(this.total));
      }
      if this.sender.closed.load(Ordering::Relaxed)
        || shared.consumer_dropped.load(Ordering::Acquire)
      {
        return Poll::Ready(Err(SendBatchError {
          sent: this.sent,
          unsent: this.iter.by_ref().collect(),
        }));
      }

      let written = shared.write_batch(&mut this.iter, this.total - this.sent);
      this.sent += written;
      if this.sent == this.total {
        return Poll::Ready(Ok(this.total));
      }

      shared.producer_waker_async.register(cx.waker());

      // Re-check after registration
      if shared.consumer_dropped.load(Ordering::Acquire) {
        continue;
      }
      let head_after_register = shared.head.load(Ordering::Relaxed);
      let tail_after_register = shared.tail.load(Ordering::Acquire);
      if !shared.is_full(head_after_register, tail_after_register) {
        continue;
      }

      return Poll::Pending;
    }
  }
}

/// Future returned by [`AsyncBoundedSpscSender::send_batch_mut`].
///
/// Cancel-safe: unsent items remain in the caller's vector.
#[must_use = "futures do nothing unless you .await or poll them"]
pub struct SendBatchMutFuture<'a, T> {
  sender: &'a AsyncBoundedSpscSender<T>,
  items: &'a mut Vec<T>,
  sent: usize,
  _phantom: PhantomPinned,
}

impl<'a, T: Send> Future for SendBatchMutFuture<'a, T> {
  type Output = Result<usize, SendError>;

  fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    let this = unsafe { self.as_mut().get_unchecked_mut() };
    let shared = &this.sender.shared;
    loop {
      if this.items.is_empty() {
        return Poll::Ready(Ok(this.sent));
      }
      if this.sender.closed.load(Ordering::Relaxed)
        || shared.consumer_dropped.load(Ordering::Acquire)
      {
        return Poll::Ready(Err(SendError::Closed));
      }

      let k = shared.available_space().min(this.items.len());
      if k > 0 {
        let mut drain = this.items.drain(..k);
        let written = shared.write_batch(&mut drain, k);
        debug_assert_eq!(written, k);
        drop(drain);
        this.sent += k;
        continue;
      }

      shared.producer_waker_async.register(cx.waker());

      // Re-check after registration
      if shared.consumer_dropped.load(Ordering::Acquire) {
        continue;
      }
      let head_after_register = shared.head.load(Ordering::Relaxed);
      let tail_after_register = shared.tail.load(Ordering::Acquire);
      if !shared.is_full(head_after_register, tail_after_register) {
        continue;
      }

      return Poll::Pending;
    }
  }
}

impl<T> Drop for AsyncBoundedSpscReceiver<T> {
  fn drop(&mut self) {
    if !self.closed.swap(true, Ordering::AcqRel) {
      self.close_internal();
    }
  }
}

#[must_use = "futures do nothing unless you .await or poll them"]
pub struct ReceiveFuture<'a, T> {
  receiver: &'a AsyncBoundedSpscReceiver<T>,
}

impl<'a, T> ReceiveFuture<'a, T> {
  fn new(receiver: &'a AsyncBoundedSpscReceiver<T>) -> Self {
    ReceiveFuture { receiver }
  }
}

impl<'a, T: Send> Future for ReceiveFuture<'a, T> {
  type Output = Result<T, RecvError>;

  fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    if self.receiver.closed.load(Ordering::Relaxed) {
      return Poll::Ready(Err(RecvError::Disconnected));
    }
    self.receiver.shared.poll_recv_internal(cx)
  }
}

/// Future returned by [`AsyncBoundedSpscReceiver::recv_batch`].
///
/// Cancel-safe: items are only removed in the poll that resolves the future.
#[must_use = "futures do nothing unless you .await or poll them"]
pub struct RecvBatchFuture<'a, T> {
  receiver: &'a AsyncBoundedSpscReceiver<T>,
  max: usize,
}

impl<'a, T: Send> Future for RecvBatchFuture<'a, T> {
  type Output = Result<Vec<T>, RecvError>;

  fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    if self.max == 0 {
      return Poll::Ready(Ok(Vec::new()));
    }
    let mut out = Vec::new();
    match poll_recv_batch_spsc(self.receiver, cx, &mut out, self.max) {
      Poll::Ready(Ok(_)) => Poll::Ready(Ok(out)),
      Poll::Ready(Err(e)) => Poll::Ready(Err(e)),
      Poll::Pending => Poll::Pending,
    }
  }
}

/// Future returned by [`AsyncBoundedSpscReceiver::recv_batch_mut`].
///
/// Cancel-safe: items are only removed in the poll that resolves the future.
#[must_use = "futures do nothing unless you .await or poll them"]
pub struct RecvBatchMutFuture<'a, T> {
  receiver: &'a AsyncBoundedSpscReceiver<T>,
  out: &'a mut Vec<T>,
  max: usize,
}

impl<'a, T: Send> Future for RecvBatchMutFuture<'a, T> {
  type Output = Result<usize, RecvError>;

  fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    let this = &mut *self;
    if this.max == 0 {
      return Poll::Ready(Ok(0));
    }
    let max = this.max;
    poll_recv_batch_spsc(this.receiver, cx, this.out, max)
  }
}

/// Shared poll logic for SPSC batch receive futures. Mirrors
/// `SpscShared::poll_recv_internal` (register waker, re-check, Pending) but
/// drains up to `max` items in one pass.
fn poll_recv_batch_spsc<T: Send>(
  receiver: &AsyncBoundedSpscReceiver<T>,
  cx: &mut Context<'_>,
  out: &mut Vec<T>,
  max: usize,
) -> Poll<Result<usize, RecvError>> {
  if receiver.closed.load(Ordering::Relaxed) {
    return Poll::Ready(Err(RecvError::Disconnected));
  }
  let shared = &receiver.shared;
  loop {
    let k = shared.read_batch(out, max);
    if k > 0 {
      return Poll::Ready(Ok(k));
    }

    if shared.producer_dropped.load(Ordering::Acquire) {
      // Drain anything written just before the producer dropped.
      let k = shared.read_batch(out, max);
      if k > 0 {
        return Poll::Ready(Ok(k));
      }
      return Poll::Ready(Err(RecvError::Disconnected));
    }

    shared.consumer_waker_async.register(cx.waker());

    // Critical re-check after registration
    let tail = shared.tail.load(Ordering::Relaxed);
    let head_after_register = shared.head.load(Ordering::Acquire);
    if !shared.is_empty(head_after_register, tail) || shared.producer_dropped.load(Ordering::Acquire)
    {
      continue;
    }

    return Poll::Pending;
  }
}

impl<T: Send> Stream for AsyncBoundedSpscReceiver<T> {
  type Item = T;

  fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
    if self.closed.load(Ordering::Relaxed) {
      return Poll::Ready(None);
    }
    match self.shared.poll_recv_internal(cx) {
      Poll::Ready(Ok(value)) => Poll::Ready(Some(value)),
      Poll::Ready(Err(_)) => Poll::Ready(None),
      Poll::Pending => Poll::Pending,
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::time::Duration;
  use tokio::time::timeout;

  const TEST_TIMEOUT: Duration = Duration::from_secs(2);

  // Helper to create SpscShared for tests that mix P/C types
  fn create_test_shared_core<T: Send>(capacity: usize) -> Arc<SpscShared<T>> {
    // T: Send
    SpscShared::new_internal(capacity).into()
  }

  #[cfg(not(miri))]
  #[tokio::test]
  async fn create_async_channel() {
    let (p, c) = bounded_async::<i32>(5);
    assert_eq!(p.len(), 0);
    assert!(p.is_empty());
    assert!(!p.is_full());
    assert_eq!(c.len(), 0);
    assert!(c.is_empty());
    assert!(!c.is_full());
    assert_eq!(p.capacity(), 5);
    assert_eq!(c.capacity(), 5);
    drop(p);
    drop(c);
  }

  #[cfg(not(miri))]
  #[tokio::test]
  async fn async_send_recv_single_item() {
    let (p, c) = bounded_async(1);
    p.send(42i32).await.unwrap();
    assert_eq!(p.len(), 1);
    assert!(!p.is_empty());
    assert!(p.is_full());
    assert_eq!(c.len(), 1);
    assert!(!c.is_empty());
    assert!(c.is_full());

    assert_eq!(c.recv().await.unwrap(), 42);
    assert_eq!(p.len(), 0); // After recv
    assert_eq!(c.len(), 0);
    assert!(c.is_empty());
  }

  #[cfg(not(miri))]
  #[tokio::test]
  async fn async_try_send_full_try_recv_empty() {
    let (p, c) = bounded_async::<i32>(1);
    assert_eq!(p.len(), 0);
    p.try_send(10).unwrap();
    assert_eq!(p.len(), 1);
    assert_eq!(c.len(), 1);
    assert!(p.is_full());

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

  #[cfg(not(miri))]
  #[tokio::test]
  async fn async_send_blocks_then_completes() {
    let (p, c) = bounded_async::<i32>(1);

    assert_eq!(p.len(), 0);
    p.send(1).await.unwrap(); // Fill the channel
    assert_eq!(p.len(), 1);

    let send_task = tokio::spawn(async move {
      println!("[ASYNC_SEND_BLOCKS] Send Task: Sending 2...");
      p.send(2).await.unwrap();
      println!("[ASYNC_SEND_BLOCKS] Send Task: Sent 2.");
    });

    tokio::time::sleep(Duration::from_millis(50)).await; // Give send_task time to block

    println!("[ASYNC_SEND_BLOCKS] Main Task: Receiving 1...");
    assert_eq!(c.len(), 1);
    assert_eq!(c.recv().await.unwrap(), 1); // Unblock producer
    println!("[ASYNC_SEND_BLOCKS] Main Task: Received 1.");

    match timeout(TEST_TIMEOUT, send_task).await {
      Ok(Ok(())) => println!("[ASYNC_SEND_BLOCKS] Main Task: Send task completed."),
      Ok(Err(e)) => panic!("[ASYNC_SEND_BLOCKS] Send task panicked: {:?}", e),
      Err(_) => panic!("[ASYNC_SEND_BLOCKS] Send task timed out"),
    }
    assert_eq!(c.len(), 1); // After send_task completes, one item should be in.

    println!("[ASYNC_SEND_BLOCKS] Main Task: Receiving 2...");
    assert_eq!(c.recv().await.unwrap(), 2);
    println!("[ASYNC_SEND_BLOCKS] Main Task: Received 2.");
    assert_eq!(c.len(), 0);
  }

  #[cfg(not(miri))]
  #[tokio::test]
  async fn async_recv_blocks_then_completes() {
    let (p, c) = bounded_async::<i32>(1);
    assert!(c.is_empty());

    let recv_task = tokio::spawn(async move {
      println!("[ASYNC_RECV_BLOCKS] Recv Task: Receiving...");
      let val = c.recv().await.unwrap();
      println!("[ASYNC_RECV_BLOCKS] Recv Task: Received {}.", val);
      assert_eq!(val, 100);
      assert!(c.is_empty()); // After receiving
    });

    tokio::time::sleep(Duration::from_millis(50)).await; // Give recv_task time to block

    println!("[ASYNC_RECV_BLOCKS] Main Task: Sending 100...");
    p.send(100).await.unwrap();
    println!("[ASYNC_RECV_BLOCKS] Main Task: Sent 100.");
    assert!(p.is_full()); // After send

    match timeout(TEST_TIMEOUT, recv_task).await {
      Ok(Ok(())) => println!("[ASYNC_RECV_BLOCKS] Main Task: Recv task completed."),
      Ok(Err(e)) => panic!("[ASYNC_RECV_BLOCKS] Recv task panicked: {:?}", e),
      Err(_) => panic!("[ASYNC_RECV_BLOCKS] Recv task timed out"),
    }
  }

  #[cfg(not(miri))]
  #[tokio::test]
  async fn async_producer_drop_signals_consumer() {
    let (p, c) = bounded_async::<i32>(1);
    p.send(10).await.unwrap(); // Send an item first
    assert_eq!(c.len(), 1);
    drop(p);
    assert_eq!(c.recv().await.unwrap(), 10); // Receiver gets the item
    assert_eq!(c.len(), 0);
    match c.recv().await {
      // Then gets disconnected
      Err(RecvError::Disconnected) => {}
      Ok(v) => panic!("[DROP_P] Expected Disconnected, got Ok({:?})", v),
    }
  }

  #[cfg(not(miri))]
  #[tokio::test]
  async fn async_producer_drop_empty_signals_consumer() {
    let (p, c) = bounded_async::<i32>(1);
    drop(p);
    assert_eq!(c.len(), 0);
    match c.recv().await {
      Err(RecvError::Disconnected) => {}
      Ok(v) => panic!("[DROP_P_EMPTY] Expected Disconnected, got Ok({:?})", v),
    }
  }

  #[cfg(not(miri))]
  #[tokio::test]
  async fn async_consumer_drop_signals_producer() {
    let (p, c) = bounded_async::<i32>(1);
    drop(c);
    match p.send(1).await {
      Err(SendError::Closed) => {}
      Ok(()) => panic!("[DROP_C] Expected Closed error"),
      Err(SendError::Sent) => panic!("[DROP_C] SPSC send should not error with Sent"),
    }
  }

  #[cfg(not(miri))]
  #[tokio::test]
  async fn async_select_recv_preference() {
    let (p1, c1) = bounded_async::<i32>(1);
    let (_p2, c2) = bounded_async::<i32>(1); // This consumer will never receive

    p1.send(10).await.unwrap();
    assert_eq!(c1.len(), 1);
    assert!(c2.is_empty());

    tokio::select! {
        biased;
        Ok(val) = c1.recv() => {
            assert_eq!(val, 10);
            assert!(c1.is_empty());
        }
        Ok(_val) = c2.recv() => {
            panic!("[SELECT_RECV] Should not have received from empty c2");
        }
        else => {}
    }
  }

  #[cfg(not(miri))]
  #[tokio::test]
  async fn async_select_send_blocks_other_completes() {
    let (p_full, _c_full) = bounded_async::<i32>(1);
    let (p_can_send, c_can_send) = bounded_async::<i32>(1);

    p_full.send(1).await.unwrap(); // Fill this channel
    assert!(p_full.is_full());
    assert!(p_can_send.is_empty());

    let recv_task = tokio::spawn(async move {
      tokio::time::sleep(Duration::from_millis(100)).await;
      assert_eq!(c_can_send.recv().await.unwrap(), 200);
    });

    tokio::select! {
        biased;
        res_full = p_full.send(100) => {
            panic!("[SELECT_SEND] Send to full channel should not have completed immediately, got {:?}", res_full);
        }
        res_can_send = p_can_send.send(200) => {
            assert!(res_can_send.is_ok(), "[SELECT_SEND] Send to available channel failed");
            assert!(p_can_send.is_full());
        }
    }
    recv_task.await.unwrap();
  }

  // --- Mixed Sync/Async Tests ---
  #[cfg(not(miri))]
  #[tokio::test]
  async fn sync_producer_async_consumer() {
    const CAPACITY: usize = 2;
    let core_shared = create_test_shared_core::<String>(CAPACITY);

    let sync_p = BoundedSyncSender::from_shared(core_shared.clone());
    let async_c = AsyncBoundedSpscReceiver::from_shared(core_shared);

    let val1 = "hello from sync".to_string();
    let val2 = "world from sync".to_string();

    println!("[MIXED_S2A] SyncP sending: {}", val1);
    assert!(sync_p.is_empty());
    sync_p.send(val1.clone()).unwrap();
    println!("[MIXED_S2A] SyncP sent: {}", val1);
    assert_eq!(sync_p.len(), 1);
    assert_eq!(async_c.len(), 1);

    println!("[MIXED_S2A] AsyncC awaiting val1...");
    assert_eq!(async_c.recv().await.unwrap(), val1);
    println!("[MIXED_S2A] AsyncC received val1.");
    assert_eq!(async_c.len(), 0);

    let send_task_val2 = val2.clone(); // Clone for the task
    let send_task = tokio::task::spawn_blocking(move || {
      println!("[MIXED_S2A_TASK] SyncP sending: {}", send_task_val2);
      sync_p.send(send_task_val2.clone()).unwrap(); // sync_p moved
      println!("[MIXED_S2A_TASK] SyncP sent: {}", send_task_val2);
      send_task_val2 // Return the value for assertion
    });

    println!("[MIXED_S2A] AsyncC awaiting val2...");
    assert_eq!(async_c.recv().await.unwrap(), send_task.await.unwrap());
    println!("[MIXED_S2A] AsyncC received val2.");
    assert!(async_c.is_empty());
  }

  #[test] // This test uses std::thread for consumer, tokio runtime for producer
  fn async_producer_sync_consumer() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    const CAPACITY: usize = 2;
    let rt = tokio::runtime::Builder::new_multi_thread()
      .worker_threads(1)
      .enable_time()
      .build()
      .unwrap();
    let core_shared = create_test_shared_core::<String>(CAPACITY);

    let async_p = AsyncBoundedSpscSender::from_shared(core_shared.clone());
    let sync_c = BoundedSyncReceiver::from_shared(core_shared);

    let val1_original = "hello from async".to_string();
    let val2_original = "world from async".to_string();

    let val1_for_async_task = val1_original.clone();
    let val2_for_async_task = val2_original.clone();

    // Atomic phase flag: producer stores 1 after asserting len==1,
    // consumer spins until it observes 1 before touching the queue.
    let phase = Arc::new(AtomicUsize::new(0));
    let phase_clone = Arc::clone(&phase);

    assert!(async_p.is_empty());
    assert!(sync_c.is_empty());

    let producer_handle = rt.spawn(async move {
      println!("[MIXED_A2S_TASK] AsyncP sending val1...");
      assert!(async_p.is_empty());
      async_p.send(val1_for_async_task).await.unwrap();
      println!("[MIXED_A2S_TASK] AsyncP sent val1.");

      // Assert while holding the item in the queue.
      assert_eq!(async_p.len(), 1);

      // Signal the sync consumer that the write and assertion are complete.
      phase_clone.store(1, Ordering::Release);

      tokio::time::sleep(Duration::from_millis(50)).await;

      println!("[MIXED_A2S_TASK] AsyncP sending val2...");
      async_p.send(val2_for_async_task).await.unwrap();
      println!("[MIXED_A2S_TASK] AsyncP sent val2.");
    });

    println!("[MIXED_A2S] SyncC waiting for phase 1...");
    // Spin-yield until the producer finishes its len assertion.
    while phase.load(Ordering::Acquire) == 0 {
      std::thread::yield_now();
    }

    assert_eq!(sync_c.len(), 1, "Length before first recv should be 1");

    assert_eq!(sync_c.recv().unwrap(), val1_original);
    println!("[MIXED_A2S] SyncC received val1.");
    assert_eq!(sync_c.len(), 0, "Length after first recv should be 0");

    println!("[MIXED_A2S] SyncC receiving val2...");

    rt.block_on(async { producer_handle.await.unwrap() });

    assert_eq!(
      sync_c.len(),
      1,
      "Length before second recv (after producer done) should be 1"
    );
    assert_eq!(sync_c.recv().unwrap(), val2_original);
    println!("[MIXED_A2S] SyncC received val2.");
    assert!(sync_c.is_empty(), "Channel should be empty after all recvs");
  }

  #[cfg(not(miri))]
  #[tokio::test]
  async fn async_try_recv_disconnected() {
    let (p, c) = bounded_async::<i32>(1);
    p.try_send(1).unwrap();
    assert_eq!(c.try_recv().unwrap(), 1);
    assert!(c.is_empty());

    drop(p);

    assert_eq!(c.try_recv(), Err(TryRecvError::Disconnected));
  }

  #[cfg(not(miri))]
  #[tokio::test]
  async fn async_recv_future_disconnected_after_item() {
    let (p, c) = bounded_async::<i32>(1);
    p.send(1).await.unwrap();
    assert_eq!(c.recv().await.unwrap(), 1);
    assert!(c.is_empty());

    drop(p);

    assert_eq!(c.recv().await, Err(RecvError::Disconnected));
  }

  #[cfg(not(miri))]
  #[tokio::test]
  async fn new_spsc_apis_close_is_closed() {
    let (p, c) = bounded_async::<i32>(5);
    assert_eq!(p.capacity(), 5);
    assert_eq!(c.capacity(), 5);
    assert!(!p.is_closed());
    assert!(!c.is_closed());

    p.close().unwrap();
    assert!(p.is_closed());
    assert_eq!(p.close(), Err(CloseError));
    assert_eq!(p.send(1).await, Err(SendError::Closed));

    assert!(c.is_closed());
    assert_eq!(c.recv().await, Err(RecvError::Disconnected));

    let (p, c) = bounded_async::<i32>(5);
    c.close().unwrap();
    assert!(c.is_closed());
    assert_eq!(c.close(), Err(CloseError));
    assert_eq!(c.recv().await, Err(RecvError::Disconnected));

    assert!(p.is_closed());
    assert_eq!(p.send(1).await, Err(SendError::Closed));
  }

  #[cfg(not(miri))]
  #[tokio::test]
  async fn async_sender_unblocks_on_consumer_drop() {
    let (p, c) = bounded_async(1);
    // Fill the channel capacity
    p.send(1).await.unwrap();

    let handle = tokio::spawn(async move {
      // This send should block (await)
      p.send(2).await
    });

    // Give the sender task some time to yield and register
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!handle.is_finished(), "Sender task should be blocked");

    // Drop the consumer to trigger unblocking
    drop(c);

    let res = handle.await.expect("Sender task panicked");
    assert!(
      matches!(res, Err(SendError::Closed)),
      "Expected Err(SendError::Closed), got {:?}",
      res
    );
  }
}
