//! Producer handles for the unbounded v3 MPSC channel.
//!
//! A send bump-allocates a node from the handle's private slab (zero shared
//! traffic), publishes it with the single Vyukov `swap`, and never blocks.
//!
//! Sending takes `&mut self`: the bump slab is producer-private state, so
//! exclusive access comes from the borrow checker instead of a per-handle
//! lock. Clone the sender for each thread or task that sends.

use super::shared::MpscShared;
use crate::error::{
  BatchSendErrorReason, CloseError, SendBatchError, SendError, TrySendBatchError, TrySendError,
};
use crate::internal::slab_chain::ProducerSlab;

use core::marker::PhantomPinned;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::ptr;
use std::sync::Arc;
use std::task::{Context, Poll};

/// Core send: bump + write into the handle-private slab, publish + notify.
pub(crate) fn send_internal<T: Send>(
  shared: &Arc<MpscShared<T>>,
  slab: &mut ProducerSlab<T>,
  shard: usize,
  value: T,
) -> Result<(), T> {
  if !shared.receivers_alive() {
    return Err(value);
  }
  let node = slab.bump();
  unsafe { *(*node).val.get() = Some(value) };
  shared.publish(node, node);
  shared.record_sent(shard, 1);
  shared.notify_receiver();
  Ok(())
}

/// Batch counterpart: bumps and pre-links `count` nodes, then publishes the
/// whole run with ONE swap and wakes the consumer once. The caller performs
/// the closed checks before consuming any items. The iterator must yield at
/// least `count` items.
pub(crate) fn send_batch_internal<T: Send>(
  shared: &Arc<MpscShared<T>>,
  slab: &mut ProducerSlab<T>,
  shard: usize,
  iter: &mut impl Iterator<Item = T>,
  count: usize,
) {
  debug_assert!(count > 0);
  let (first, last) = slab.bump_batch(iter, count);
  shared.publish(first, last);
  shared.record_sent(shard, count);
  shared.notify_receiver();
}

// --- Handles ------------------------------------------------------------------

pub struct Sender<T: Send> {
  pub(crate) shared: Arc<MpscShared<T>>,
  pub(crate) closed: bool,
  pub(crate) slab: ProducerSlab<T>,
  pub(crate) shard: usize,
}

pub struct AsyncSender<T: Send> {
  pub(crate) shared: Arc<MpscShared<T>>,
  pub(crate) closed: bool,
  pub(crate) slab: ProducerSlab<T>,
  pub(crate) shard: usize,
}

unsafe impl<T: Send> Send for Sender<T> {}
unsafe impl<T: Send> Sync for Sender<T> {}
unsafe impl<T: Send> Send for AsyncSender<T> {}
unsafe impl<T: Send> Sync for AsyncSender<T> {}

impl<T: Send> fmt::Debug for Sender<T> {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("Sender")
      .field("len", &self.len())
      .field("closed", &self.closed)
      .finish()
  }
}

impl<T: Send> fmt::Debug for AsyncSender<T> {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("AsyncSender")
      .field("len", &self.len())
      .field("closed", &self.closed)
      .finish()
  }
}

impl<T: Send> Sender<T> {
  pub(crate) fn from_shared(shared: Arc<MpscShared<T>>) -> Self {
    let shard = shared.next_shard();
    let slab = ProducerSlab::new(shared.slab_pool());
    Sender {
      shared,
      closed: false,
      slab,
      shard,
    }
  }

  pub fn send(&mut self, value: T) -> Result<(), SendError> {
    if self.closed {
      return Err(SendError::Closed);
    }
    send_internal(&self.shared, &mut self.slab, self.shard, value).map_err(|_| SendError::Closed)
  }

  /// Never fails with `Full`: the channel is unbounded.
  pub fn try_send(&mut self, value: T) -> Result<(), TrySendError<T>> {
    if self.closed {
      return Err(TrySendError::Closed(value));
    }
    send_internal(&self.shared, &mut self.slab, self.shard, value).map_err(TrySendError::Closed)
  }

  /// Sends a whole batch with a single publish and a single consumer wake.
  /// Unbounded, so this never blocks: `Ok(n)` means every item was sent. The
  /// only failure is a closed channel, reported before any item is consumed.
  pub fn send_batch(&mut self, items: Vec<T>) -> Result<usize, SendBatchError<T>> {
    self.try_send_batch(items).map_err(|e| SendBatchError {
      sent: e.sent,
      unsent: e.unsent,
    })
  }

  pub fn try_send_batch(&mut self, items: Vec<T>) -> Result<usize, TrySendBatchError<T>> {
    let total = items.len();
    if total == 0 {
      return Ok(0);
    }
    if self.closed || !self.shared.receivers_alive() {
      return Err(TrySendBatchError {
        sent: 0,
        unsent: items,
        reason: BatchSendErrorReason::Closed,
      });
    }
    let mut iter = items.into_iter();
    send_batch_internal(&self.shared, &mut self.slab, self.shard, &mut iter, total);
    Ok(total)
  }

  /// In-place batch send, draining all items from `items`. Never blocks. On
  /// `Err(SendError::Closed)` all items remain in `items`.
  pub fn send_batch_mut(&mut self, items: &mut Vec<T>) -> Result<usize, SendError> {
    if items.is_empty() {
      return Ok(0);
    }
    if self.closed || !self.shared.receivers_alive() {
      return Err(SendError::Closed);
    }
    let total = items.len();
    let mut drain = items.drain(..);
    send_batch_internal(&self.shared, &mut self.slab, self.shard, &mut drain, total);
    drop(drain);
    Ok(total)
  }

  pub fn try_send_batch_mut(&mut self, items: &mut Vec<T>) -> Result<usize, SendError> {
    self.send_batch_mut(items)
  }

  pub fn close(&mut self) -> Result<(), CloseError> {
    if self.closed {
      return Err(CloseError);
    }
    self.closed = true;
    self.close_internal();
    Ok(())
  }

  fn close_internal(&mut self) {
    self.slab.seal();
    self.shared.drop_sender();
  }

  pub fn is_closed(&self) -> bool {
    self.closed || !self.shared.receivers_alive()
  }

  pub fn sender_count(&self) -> usize {
    self.shared.sender_count()
  }

  pub fn len(&self) -> usize {
    self.shared.len()
  }

  /// Approximate: another handle may send, or the receiver consume,
  /// concurrently with this check. For an accurate check use the receiver's
  /// `is_empty()`.
  pub fn is_empty(&self) -> bool {
    self.len() == 0
  }

  /// Converts this synchronous `Sender` into an `AsyncSender`, keeping its
  /// bump slab and closed state.
  pub fn to_async(self) -> AsyncSender<T> {
    let shared = unsafe { ptr::read(&self.shared) };
    let slab = unsafe { ptr::read(&self.slab) };
    let shard = self.shard;
    let closed = self.closed;
    std::mem::forget(self);
    AsyncSender {
      shared,
      closed,
      slab,
      shard,
    }
  }
}

impl<T: Send> AsyncSender<T> {
  pub(crate) fn from_shared(shared: Arc<MpscShared<T>>) -> Self {
    let shard = shared.next_shard();
    let slab = ProducerSlab::new(shared.slab_pool());
    AsyncSender {
      shared,
      closed: false,
      slab,
      shard,
    }
  }

  /// Sends never block on an unbounded channel; the returned future resolves
  /// on first poll.
  pub fn send(&mut self, value: T) -> SendFuture<'_, T> {
    SendFuture {
      producer: self,
      value: Some(value),
      _phantom: PhantomPinned,
    }
  }

  pub fn try_send(&mut self, value: T) -> Result<(), TrySendError<T>> {
    if self.closed {
      return Err(TrySendError::Closed(value));
    }
    send_internal(&self.shared, &mut self.slab, self.shard, value).map_err(TrySendError::Closed)
  }

  pub fn send_batch(&mut self, items: Vec<T>) -> SendBatchFuture<'_, T> {
    SendBatchFuture {
      producer: self,
      items: Some(items),
      _phantom: PhantomPinned,
    }
  }

  pub fn try_send_batch(&mut self, items: Vec<T>) -> Result<usize, TrySendBatchError<T>> {
    let total = items.len();
    if total == 0 {
      return Ok(0);
    }
    if self.closed || !self.shared.receivers_alive() {
      return Err(TrySendBatchError {
        sent: 0,
        unsent: items,
        reason: BatchSendErrorReason::Closed,
      });
    }
    let mut iter = items.into_iter();
    send_batch_internal(&self.shared, &mut self.slab, self.shard, &mut iter, total);
    Ok(total)
  }

  pub fn send_batch_mut<'a>(&'a mut self, items: &'a mut Vec<T>) -> SendBatchMutFuture<'a, T> {
    SendBatchMutFuture {
      producer: self,
      items,
      _phantom: PhantomPinned,
    }
  }

  pub fn try_send_batch_mut(&mut self, items: &mut Vec<T>) -> Result<usize, SendError> {
    if items.is_empty() {
      return Ok(0);
    }
    if self.closed || !self.shared.receivers_alive() {
      return Err(SendError::Closed);
    }
    let total = items.len();
    let mut drain = items.drain(..);
    send_batch_internal(&self.shared, &mut self.slab, self.shard, &mut drain, total);
    drop(drain);
    Ok(total)
  }

  pub fn close(&mut self) -> Result<(), CloseError> {
    if self.closed {
      return Err(CloseError);
    }
    self.closed = true;
    self.close_internal();
    Ok(())
  }

  fn close_internal(&mut self) {
    self.slab.seal();
    self.shared.drop_sender();
  }

  pub fn is_closed(&self) -> bool {
    self.closed || !self.shared.receivers_alive()
  }

  pub fn sender_count(&self) -> usize {
    self.shared.sender_count()
  }

  pub fn len(&self) -> usize {
    self.shared.len()
  }

  /// Approximate; see [`Sender::is_empty`].
  pub fn is_empty(&self) -> bool {
    self.len() == 0
  }

  /// Converts this `AsyncSender` into a synchronous `Sender`, keeping its
  /// bump slab and closed state.
  pub fn to_sync(self) -> Sender<T> {
    let shared = unsafe { ptr::read(&self.shared) };
    let slab = unsafe { ptr::read(&self.slab) };
    let shard = self.shard;
    let closed = self.closed;
    std::mem::forget(self);
    Sender {
      shared,
      closed,
      slab,
      shard,
    }
  }
}

// --- Clone / Drop ---------------------------------------------------------------

impl<T: Send> Clone for Sender<T> {
  fn clone(&self) -> Self {
    self.shared.add_sender();
    Sender::from_shared(Arc::clone(&self.shared))
  }
}

impl<T: Send> Clone for AsyncSender<T> {
  fn clone(&self) -> Self {
    self.shared.add_sender();
    AsyncSender::from_shared(Arc::clone(&self.shared))
  }
}

impl<T: Send> Drop for Sender<T> {
  fn drop(&mut self) {
    if !self.closed {
      self.closed = true;
      self.close_internal();
    }
  }
}

impl<T: Send> Drop for AsyncSender<T> {
  fn drop(&mut self) {
    if !self.closed {
      self.closed = true;
      self.close_internal();
    }
  }
}

// --- Futures ---------------------------------------------------------------------

#[must_use = "futures do nothing unless you .await or poll them"]
pub struct SendFuture<'a, T: Send> {
  producer: &'a mut AsyncSender<T>,
  value: Option<T>,
  _phantom: PhantomPinned,
}

impl<'a, T: Send> Future for SendFuture<'a, T> {
  type Output = Result<(), SendError>;

  fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
    let this = unsafe { self.as_mut().get_unchecked_mut() };
    if this.producer.closed {
      return Poll::Ready(Err(SendError::Closed));
    }
    let value = this
      .value
      .take()
      .expect("SendFuture polled after completion");
    Poll::Ready(
      send_internal(
        &this.producer.shared,
        &mut this.producer.slab,
        this.producer.shard,
        value,
      )
      .map_err(|_| SendError::Closed),
    )
  }
}

#[must_use = "futures do nothing unless you .await or poll them"]
pub struct SendBatchFuture<'a, T: Send> {
  producer: &'a mut AsyncSender<T>,
  items: Option<Vec<T>>,
  _phantom: PhantomPinned,
}

impl<'a, T: Send> Future for SendBatchFuture<'a, T> {
  type Output = Result<usize, SendBatchError<T>>;

  fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
    let this = unsafe { self.as_mut().get_unchecked_mut() };
    let items = this
      .items
      .take()
      .expect("SendBatchFuture polled after completion");
    Poll::Ready(this.producer.try_send_batch(items).map_err(|e| {
      SendBatchError {
        sent: e.sent,
        unsent: e.unsent,
      }
    }))
  }
}

#[must_use = "futures do nothing unless you .await or poll them"]
pub struct SendBatchMutFuture<'a, T: Send> {
  producer: &'a mut AsyncSender<T>,
  items: &'a mut Vec<T>,
  _phantom: PhantomPinned,
}

impl<'a, T: Send> Future for SendBatchMutFuture<'a, T> {
  type Output = Result<usize, SendError>;

  fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
    let this = unsafe { self.as_mut().get_unchecked_mut() };
    Poll::Ready(this.producer.try_send_batch_mut(this.items))
  }
}
