//! The asynchronous API for the bounded MPSC channel.

use super::bounded_sync::{
  unwrap_batch_messages, BoundedMessage, BoundedMpscShared, Permit, Receiver, Sender,
};
use crate::error::{
  BatchSendErrorReason, RecvError, SendBatchError, SendError, TrySendBatchError, TrySendError,
};
use crate::mpsc::unbounded_v2;
use crate::{CloseError, TryRecvError};
use futures_core::Stream;

use std::future::Future;
use std::marker::PhantomPinned;
use std::mem;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};

// --- Public Channel Handles (Async) ---

#[derive(Debug)]
pub struct AsyncSender<T: Send> {
  pub(crate) shared: Arc<BoundedMpscShared<T>>,
  pub(crate) closed: AtomicBool,
}

#[derive(Debug)]
pub struct AsyncReceiver<T: Send> {
  pub(crate) shared: Arc<BoundedMpscShared<T>>,
  pub(crate) closed: AtomicBool,
}

// --- Sender Implementation (Async) ---

impl<T: Send> AsyncSender<T> {
  /// Sends a value asynchronously.
  /// The returned future completes when the value has been sent. It will wait
  /// if the channel is currently full.
  pub fn send(&self, value: T) -> SendFuture<'_, T> {
    SendFuture {
      // Eagerly create the future for acquiring a permit.
      acquire: self.shared.gate.acquire_async(),
      sender: self,
      value: Some(value),
      is_rendezvous: self.capacity() == 0, // Pass rendezvous info to future
      _phantom: PhantomPinned,
    }
  }
  /// Attempts to send a value into the channel without blocking.
  pub fn try_send(&self, value: T) -> Result<(), TrySendError<T>> {
    // We can just use the sync try_send logic directly.
    if self.is_closed() {
      return Err(TrySendError::Closed(value));
    }
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
      return Err(TrySendError::Closed(msg.value));
    }
    Ok(())
  }

  /// Sends a batch asynchronously, taking ownership of the vector. Permits
  /// are acquired in bulk; the future re-arms its permit acquisition for the
  /// remainder whenever the channel fills mid-batch.
  ///
  /// Resolves with `Ok(n)` once every item is sent, or [`SendBatchError`]
  /// (count sent + unsent remainder) if the receiver drops. If the future is
  /// dropped after partial progress, the already-sent prefix stays in the
  /// channel and the remainder is dropped; use
  /// [`send_batch_mut`](Self::send_batch_mut) for cancel safety.
  pub fn send_batch(&self, items: Vec<T>) -> SendBatchFuture<'_, T> {
    let total = items.len();
    SendBatchFuture {
      acquire: self.shared.gate.acquire_many_async(total),
      sender: self,
      iter: items.into_iter(),
      total,
      sent: 0,
      is_rendezvous: self.capacity() == 0,
      _phantom: PhantomPinned,
    }
  }

  /// Sends a batch asynchronously in place, draining sent items from the
  /// front of `items`. Cancel-safe: on drop or `Err(SendError::Closed)`,
  /// every unsent item remains in `items`.
  pub fn send_batch_mut<'a>(&'a self, items: &'a mut Vec<T>) -> SendBatchMutFuture<'a, T> {
    let len = items.len();
    SendBatchMutFuture {
      acquire: self.shared.gate.acquire_many_async(len),
      sender: self,
      items,
      sent: 0,
      is_rendezvous: self.capacity() == 0,
      _phantom: PhantomPinned,
    }
  }

  /// Attempts to send a batch without blocking, taking ownership of the
  /// vector. Same semantics as the sync [`Sender::try_send_batch`].
  pub fn try_send_batch(&self, items: Vec<T>) -> Result<usize, TrySendBatchError<T>> {
    let total = items.len();
    if total == 0 {
      return Ok(0);
    }
    if self.closed.load(Ordering::Relaxed) || self.is_closed() {
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
    if self.is_closed() {
      return Err(TrySendBatchError {
        sent: 0,
        unsent: items,
        reason: BatchSendErrorReason::Closed,
      });
    }
    let mut iter = items.into_iter();
    push_batch_messages_async(self, &mut iter, k);
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

  /// Attempts to send a batch in place without blocking, draining sent items
  /// from the front of `items`. Returns `Ok(k)` with the count sent (`0` if
  /// no capacity); `Err(SendError::Closed)` only if closed with zero sent by
  /// this call.
  pub fn try_send_batch_mut(&self, items: &mut Vec<T>) -> Result<usize, SendError> {
    if items.is_empty() {
      return Ok(0);
    }
    if self.closed.load(Ordering::Relaxed) || self.is_closed() {
      return Err(SendError::Closed);
    }
    let k = self.shared.gate.try_acquire_many(items.len());
    if k == 0 {
      return Ok(0);
    }
    if self.is_closed() {
      return Err(SendError::Closed);
    }
    let mut drain = items.drain(..k);
    push_batch_messages_async(self, &mut drain, k);
    drop(drain);
    Ok(k)
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
      self.shared.channel.wake_consumer();
      // Also wake the gate in case the receiver is waiting on a rendezvous
      self.shared.gate.release();
    }
  }

  /// Returns `true` if the receiver has been dropped.
  pub fn is_closed(&self) -> bool {
    self.shared.channel.receiver_dropped.load(Ordering::Acquire)
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

  /// Converts this `AsyncSender` into a synchronous `Sender`.
  pub fn to_sync(self) -> Sender<T> {
    let shared = unsafe { std::ptr::read(&self.shared) };
    mem::forget(self);
    Sender {
      shared,
      closed: AtomicBool::new(false),
    }
  }
}

impl<T: Send> Clone for AsyncSender<T> {
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

impl<T: Send> Drop for AsyncSender<T> {
  fn drop(&mut self) {
    if !self.closed.swap(true, Ordering::AcqRel) {
      self.close_internal();
    }
  }
}

/// Wraps the next `k` items from `iter` in `BoundedMessage`s (each with its
/// own RAII permit) and pushes them through the underlying unbounded channel
/// with one length update and one consumer wake.
fn push_batch_messages_async<T: Send>(
  sender: &AsyncSender<T>,
  iter: &mut impl Iterator<Item = T>,
  k: usize,
) {
  let is_rendezvous = sender.capacity() == 0;
  let shared = &sender.shared;
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

// --- Receiver Implementation (Async) ---

impl<T: Send> AsyncReceiver<T> {
  /// Receives a value asynchronously.
  pub fn recv(&self) -> RecvFuture<'_, T> {
    RecvFuture {
      receiver: self,
      rendezvous_permit_released: false,
    }
  }

  /// Receives up to `max` items asynchronously. Resolves with between 1 and
  /// `max` items (FIFO order) once at least one is available. Capacity
  /// permits are returned to the gate in one bulk release. Cancel-safe.
  pub fn recv_batch(&self, max: usize) -> RecvBatchFuture<'_, T> {
    RecvBatchFuture {
      receiver: self,
      max,
      rendezvous_permit_released: false,
    }
  }

  /// Receives up to `max` items asynchronously, appending them to the end of
  /// `out`. Resolves with the number appended. Cancel-safe.
  pub fn recv_batch_mut<'a>(
    &'a self,
    out: &'a mut Vec<T>,
    max: usize,
  ) -> RecvBatchMutFuture<'a, T> {
    RecvBatchMutFuture {
      receiver: self,
      out,
      max,
      rendezvous_permit_released: false,
    }
  }

  /// Attempts to receive up to `max` items without blocking. Same semantics
  /// as the sync [`Receiver::try_recv_batch`].
  pub fn try_recv_batch(&self, max: usize) -> Result<Vec<T>, TryRecvError> {
    let mut out = Vec::new();
    self.try_recv_batch_mut(&mut out, max)?;
    Ok(out)
  }

  /// Attempts to receive up to `max` items without blocking, appending them
  /// to the end of `out`. Returns the number appended.
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
    let mut msgs = Vec::new();
    let k = self
      .shared
      .channel
      .try_recv_batch_internal(&mut msgs, max)?;
    debug_assert_eq!(k, msgs.len());
    Ok(unwrap_batch_messages(&self.shared.gate, msgs, out))
  }

  /// Attempts to receive a value without blocking.
  pub fn try_recv(&self) -> Result<T, TryRecvError> {
    if self.closed.load(Ordering::Relaxed) {
      return Err(TryRecvError::Disconnected);
    }
    // For rendezvous, release a permit to signal readiness for a try_send.
    if self.capacity() == 0 {
      self.shared.gate.release();
    }
    self.shared.channel.try_recv_internal().map(|msg| msg.value)
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

  fn close_internal(&self) {
    self
      .shared
      .channel
      .receiver_dropped
      .store(true, Ordering::Release);
    while self.shared.channel.try_recv_internal().is_ok() {}
    self.shared.gate.close();
  }

  /// Returns `true` if all senders have been dropped and the channel is empty.
  pub fn is_closed(&self) -> bool {
    let chan = &self.shared.channel;
    chan.sender_count.load(Ordering::Acquire) == 0 && self.is_empty()
  }

  /// Returns the number of messages currently in the channel.
  pub fn len(&self) -> usize {
    self.shared.channel.current_len.load(Ordering::Relaxed)
  }

  pub fn sender_count(&self) -> usize {
    self.shared.channel.sender_count.load(Ordering::Relaxed)
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

  /// Converts this `AsyncReceiver` into a synchronous `Receiver`.
  pub fn to_sync(self) -> Receiver<T> {
    let shared = unsafe { std::ptr::read(&self.shared) };
    mem::forget(self);
    Receiver {
      shared,
      closed: AtomicBool::new(false),
    }
  }
}

impl<T: Send> Drop for AsyncReceiver<T> {
  fn drop(&mut self) {
    if !self.closed.swap(true, Ordering::AcqRel) {
      self.close_internal();
    }
  }
}

// --- Future Implementations ---

#[must_use = "futures do nothing unless you .await or poll them"]
pub struct SendFuture<'a, T: Send> {
  // The future that acquires the permit.
  acquire: crate::coord::AcquireFuture<'a>,
  // The rest of the state, to be used once `acquire` is Ready.
  sender: &'a AsyncSender<T>,
  value: Option<T>,
  is_rendezvous: bool,
  // Make this future `!Unpin`.
  _phantom: PhantomPinned,
}

impl<'a, T: Send> Future for SendFuture<'a, T> {
  type Output = Result<(), SendError>;

  fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    // This is the key change. We get a `&mut SendFuture` from the `Pin`
    // by promising the compiler we won't move out of it.
    let this = unsafe { self.as_mut().get_unchecked_mut() };

    // Check if the receiver has been dropped.
    if this.sender.is_closed() {
      // Drop the value before returning
      this.value = None;
      return Poll::Ready(Err(SendError::Closed));
    }

    // Poll the gate first to acquire a permit. This is a "projection".
    // We get a `Pin<&mut SendFuture>` and poll its `acquire` field.
    // Safety: `SendFuture` is `!Unpin`; `this` was obtained via get_unchecked_mut
    // on a `Pin<&mut SendFuture>`, so pinning the `acquire` sub-field is sound.
    match unsafe { Pin::new_unchecked(&mut this.acquire) }.poll(cx) {
      Poll::Ready(()) => {
        // We have a permit. Now we can send.
        let value = this
          .value
          .take()
          .expect("SendFuture polled after completion");
        let permit = Permit {
          gate: this.sender.shared.gate.clone(),
          is_rendezvous: this.is_rendezvous,
        };
        let message = BoundedMessage {
          value,
          _permit: permit,
        };

        let mut cache = None;
        match unbounded_v2::send_internal(&this.sender.shared.channel, message, &mut cache) {
          Ok(()) => Poll::Ready(Ok(())),
          Err(_) => Poll::Ready(Err(SendError::Closed)),
        }
      }
      Poll::Pending => Poll::Pending,
    }
  }
}

/// Future returned by [`AsyncSender::send_batch`].
///
/// Acquires permits in bulk via an embedded [`crate::coord::AcquireManyFuture`],
/// re-arming it in place for the remainder whenever the channel fills.
/// If dropped before completion, the unsent remainder is dropped.
#[must_use = "futures do nothing unless you .await or poll them"]
pub struct SendBatchFuture<'a, T: Send> {
  acquire: crate::coord::AcquireManyFuture<'a>,
  sender: &'a AsyncSender<T>,
  iter: std::vec::IntoIter<T>,
  total: usize,
  sent: usize,
  is_rendezvous: bool,
  _phantom: PhantomPinned,
}

impl<'a, T: Send> Future for SendBatchFuture<'a, T> {
  type Output = Result<usize, SendBatchError<T>>;

  fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    let this = unsafe { self.as_mut().get_unchecked_mut() };
    loop {
      if this.sent == this.total {
        return Poll::Ready(Ok(this.total));
      }
      if this.sender.is_closed() {
        return Poll::Ready(Err(SendBatchError {
          sent: this.sent,
          unsent: this.iter.by_ref().collect(),
        }));
      }

      // SAFETY: `SendBatchFuture` is `!Unpin` and `this` came from a pinned
      // reference, so pinning the `acquire` sub-field is sound.
      match unsafe { Pin::new_unchecked(&mut this.acquire) }.poll(cx) {
        Poll::Ready(k) => {
          // A closed gate grants `max` immediately; detect receiver drop.
          if this.sender.is_closed() {
            return Poll::Ready(Err(SendBatchError {
              sent: this.sent,
              unsent: this.iter.by_ref().collect(),
            }));
          }
          let k = k.min(this.total - this.sent);
          push_batch_messages_async_rendezvous(this.sender, &mut this.iter, k, this.is_rendezvous);
          this.sent += k;
          if this.sent == this.total {
            return Poll::Ready(Ok(this.total));
          }
          // Re-arm the permit acquisition in place for the remainder.
          // SAFETY: the completed `AcquireManyFuture` deregistered itself
          // before returning Ready (`is_registered == false`), so dropping it
          // in place and writing a fresh one respects the pin contract.
          this.acquire = this
            .sender
            .shared
            .gate
            .acquire_many_async(this.total - this.sent);
          // Loop: poll the new acquire future immediately.
        }
        Poll::Pending => return Poll::Pending,
      }
    }
  }
}

/// Future returned by [`AsyncSender::send_batch_mut`].
///
/// Cancel-safe: unsent items remain in the caller's vector. (A pending
/// embedded permit acquisition unlinks itself from the gate on drop.)
#[must_use = "futures do nothing unless you .await or poll them"]
pub struct SendBatchMutFuture<'a, T: Send> {
  acquire: crate::coord::AcquireManyFuture<'a>,
  sender: &'a AsyncSender<T>,
  items: &'a mut Vec<T>,
  sent: usize,
  is_rendezvous: bool,
  _phantom: PhantomPinned,
}

impl<'a, T: Send> Future for SendBatchMutFuture<'a, T> {
  type Output = Result<usize, SendError>;

  fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    let this = unsafe { self.as_mut().get_unchecked_mut() };
    loop {
      if this.items.is_empty() {
        return Poll::Ready(Ok(this.sent));
      }
      if this.sender.is_closed() {
        return Poll::Ready(Err(SendError::Closed));
      }

      // SAFETY: see `SendBatchFuture::poll`.
      match unsafe { Pin::new_unchecked(&mut this.acquire) }.poll(cx) {
        Poll::Ready(k) => {
          if this.sender.is_closed() {
            return Poll::Ready(Err(SendError::Closed));
          }
          let k = k.min(this.items.len());
          {
            let mut drain = this.items.drain(..k);
            push_batch_messages_async_rendezvous(this.sender, &mut drain, k, this.is_rendezvous);
          }
          this.sent += k;
          if this.items.is_empty() {
            return Poll::Ready(Ok(this.sent));
          }
          // SAFETY: see `SendBatchFuture::poll` for the re-arm justification.
          this.acquire = this.sender.shared.gate.acquire_many_async(this.items.len());
        }
        Poll::Pending => return Poll::Pending,
      }
    }
  }
}

/// Like `push_batch_messages_async` but with the rendezvous flag captured at
/// future-creation time (matching the single-item `SendFuture`).
fn push_batch_messages_async_rendezvous<T: Send>(
  sender: &AsyncSender<T>,
  iter: &mut impl Iterator<Item = T>,
  k: usize,
  is_rendezvous: bool,
) {
  let shared = &sender.shared;
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

/// Future returned by [`AsyncReceiver::recv_batch`].
///
/// Cancel-safe: items are only removed in the poll that resolves the future.
#[must_use = "futures do nothing unless you .await or poll them"]
pub struct RecvBatchFuture<'a, T: Send> {
  receiver: &'a AsyncReceiver<T>,
  max: usize,
  rendezvous_permit_released: bool,
}

impl<'a, T: Send> Future for RecvBatchFuture<'a, T> {
  type Output = Result<Vec<T>, RecvError>;

  fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    let this = self.get_mut();
    if this.max == 0 {
      return Poll::Ready(Ok(Vec::new()));
    }
    let mut out = Vec::new();
    match poll_recv_batch_bounded(
      this.receiver,
      cx,
      &mut out,
      this.max,
      &mut this.rendezvous_permit_released,
    ) {
      Poll::Ready(Ok(_)) => Poll::Ready(Ok(out)),
      Poll::Ready(Err(e)) => Poll::Ready(Err(e)),
      Poll::Pending => Poll::Pending,
    }
  }
}

/// Future returned by [`AsyncReceiver::recv_batch_mut`].
///
/// Cancel-safe: items are only removed in the poll that resolves the future.
#[must_use = "futures do nothing unless you .await or poll them"]
pub struct RecvBatchMutFuture<'a, T: Send> {
  receiver: &'a AsyncReceiver<T>,
  out: &'a mut Vec<T>,
  max: usize,
  rendezvous_permit_released: bool,
}

impl<'a, T: Send> Future for RecvBatchMutFuture<'a, T> {
  type Output = Result<usize, RecvError>;

  fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    let this = self.get_mut();
    if this.max == 0 {
      return Poll::Ready(Ok(0));
    }
    let max = this.max;
    poll_recv_batch_bounded(
      this.receiver,
      cx,
      this.out,
      max,
      &mut this.rendezvous_permit_released,
    )
  }
}

/// Shared poll logic for the bounded batch receive futures: drains up to
/// `max` messages via the unbounded batch internals, unwraps the values, and
/// bulk-releases the permits.
fn poll_recv_batch_bounded<T: Send>(
  receiver: &AsyncReceiver<T>,
  cx: &mut Context<'_>,
  out: &mut Vec<T>,
  max: usize,
  rendezvous_permit_released: &mut bool,
) -> Poll<Result<usize, RecvError>> {
  if receiver.closed.load(Ordering::Relaxed) {
    return Poll::Ready(Err(RecvError::Disconnected));
  }

  // For rendezvous channels, release a permit on the first poll to signal
  // that the receiver is ready (same as the single-item `RecvFuture`).
  if receiver.capacity() == 0 && !*rendezvous_permit_released {
    receiver.shared.gate.release();
    *rendezvous_permit_released = true;
  }

  let mut msgs = Vec::new();
  match receiver
    .shared
    .channel
    .poll_recv_batch_internal(cx, &mut msgs, max)
  {
    Poll::Ready(Ok(k)) => {
      debug_assert_eq!(k, msgs.len());
      Poll::Ready(Ok(unwrap_batch_messages(&receiver.shared.gate, msgs, out)))
    }
    Poll::Ready(Err(e)) => Poll::Ready(Err(e)),
    Poll::Pending => Poll::Pending,
  }
}

#[must_use = "futures do nothing unless you .await or poll them"]
#[derive(Debug)]
pub struct RecvFuture<'a, T: Send> {
  receiver: &'a AsyncReceiver<T>,
  // State to ensure we only release one permit per recv() call for rendezvous.
  rendezvous_permit_released: bool,
}

impl<'a, T: Send> Future for RecvFuture<'a, T> {
  type Output = Result<T, RecvError>;
  fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    let this = self.get_mut(); // Pin allows getting a mutable ref

    if this.receiver.closed.load(Ordering::Relaxed) {
      return Poll::Ready(Err(RecvError::Disconnected));
    }

    // For rendezvous channels, release a permit on the first poll to
    // signal that the receiver is ready.
    if this.receiver.capacity() == 0 && !this.rendezvous_permit_released {
      this.receiver.shared.gate.release();
      this.rendezvous_permit_released = true;
    }

    match this.receiver.shared.channel.poll_recv_internal(cx) {
      Poll::Ready(Ok(msg)) => Poll::Ready(Ok(msg.value)),
      Poll::Ready(Err(e)) => Poll::Ready(Err(e)),
      Poll::Pending => Poll::Pending,
    }
  }
}

impl<T: Send> Stream for AsyncReceiver<T> {
  type Item = T;

  fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
    let this = self.get_mut();

    if this.closed.load(Ordering::Relaxed) {
      return Poll::Ready(None);
    }

    // A Stream is a series of `recv` calls. For rendezvous, we must
    // release a permit before each poll.
    if this.capacity() == 0 {
      this.shared.gate.release();
    }

    match this.shared.channel.poll_recv_internal(cx) {
      Poll::Ready(Ok(msg)) => Poll::Ready(Some(msg.value)),
      Poll::Ready(Err(_)) => Poll::Ready(None),
      Poll::Pending => Poll::Pending,
    }
  }
}
