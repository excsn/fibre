//! Producer handles for the unbounded v3 MPSC channel.
//!
//! A send bump-allocates a node from the handle's private slab (zero shared
//! traffic), publishes it with the single Vyukov `swap`, and never blocks.

use super::shared::{alloc_slab, seal_slab, MpscShared, Node, Slab, SLAB_NODES};
use crate::error::{
  BatchSendErrorReason, CloseError, SendBatchError, SendError, TrySendBatchError, TrySendError,
};

use core::marker::PhantomPinned;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};

use parking_lot::Mutex;

/// Producer-private bump state. Lives behind each handle's `Mutex` — the lock
/// keeps `Sender: Sync` sound and preserves per-handle FIFO if one clone
/// really is shared across threads; in the intended clone-per-thread usage it
/// is never contended.
pub(crate) struct ProducerSlab<T> {
  slab: *mut Slab<T>,
  base: *mut Node<T>,
  pos: usize,
}

unsafe impl<T: Send> Send for ProducerSlab<T> {}

impl<T: Send> ProducerSlab<T> {
  pub(crate) fn new() -> Self {
    ProducerSlab {
      slab: ptr::null_mut(),
      base: ptr::null_mut(),
      pos: 0,
    }
  }

  /// Hands out the next free node (fresh: `next == null`, `val == None`,
  /// slab back-pointer set), sealing/allocating slabs as needed.
  fn bump(&mut self) -> *mut Node<T> {
    if self.slab.is_null() || self.pos == SLAB_NODES {
      if !self.slab.is_null() {
        // Exhausted: release only the producer hold.
        unsafe { seal_slab(self.slab, SLAB_NODES) };
      }
      let (slab, base) = alloc_slab::<T>();
      self.slab = slab;
      self.base = base;
      self.pos = 0;
    }
    let node = unsafe { self.base.add(self.pos) };
    self.pos += 1;
    node
  }

  /// Handle close/drop: seals the partial slab exactly once. Safe to call on
  /// an already-sealed (null) state.
  pub(crate) fn seal(&mut self) {
    if !self.slab.is_null() {
      unsafe { seal_slab(self.slab, self.pos) };
      self.slab = ptr::null_mut();
      self.base = ptr::null_mut();
      self.pos = 0;
    }
  }
}

/// Core send: bump + write under the handle lock, publish + notify outside it.
pub(crate) fn send_internal<T: Send>(
  shared: &Arc<MpscShared<T>>,
  slab: &Mutex<ProducerSlab<T>>,
  shard: usize,
  value: T,
) -> Result<(), T> {
  if !shared.receivers_alive() {
    return Err(value);
  }
  let node = {
    let mut g = slab.lock();
    let node = g.bump();
    unsafe { *(*node).val.get() = Some(value) };
    node
  };
  shared.publish(node, node);
  shared.record_sent(shard, 1);
  shared.notify_receiver();
  Ok(())
}

/// Batch counterpart: bumps and pre-links `count` nodes (Relaxed stores; runs
/// may span slabs), then publishes the whole run with ONE swap and wakes the
/// consumer once. The caller performs the closed checks before consuming any
/// items. The iterator must yield at least `count` items.
pub(crate) fn send_batch_internal<T: Send>(
  shared: &Arc<MpscShared<T>>,
  slab: &Mutex<ProducerSlab<T>>,
  shard: usize,
  iter: &mut impl Iterator<Item = T>,
  count: usize,
) {
  debug_assert!(count > 0);
  let expect_msg = "send_batch: iterator yielded fewer than `count` items";
  let (first, last) = {
    let mut g = slab.lock();
    let first = g.bump();
    unsafe { *(*first).val.get() = Some(iter.next().expect(expect_msg)) };
    let mut prev = first;
    for _ in 1..count {
      let node = g.bump();
      unsafe {
        *(*node).val.get() = Some(iter.next().expect(expect_msg));
        (*prev).next.store(node, Ordering::Relaxed);
      }
      prev = node;
    }
    (first, prev)
  };
  shared.publish(first, last);
  shared.record_sent(shard, count);
  shared.notify_receiver();
}

// --- Handles ------------------------------------------------------------------

pub struct Sender<T: Send> {
  pub(crate) shared: Arc<MpscShared<T>>,
  pub(crate) closed: AtomicBool,
  pub(crate) slab: Mutex<ProducerSlab<T>>,
  pub(crate) shard: usize,
}

pub struct AsyncSender<T: Send> {
  pub(crate) shared: Arc<MpscShared<T>>,
  pub(crate) closed: AtomicBool,
  pub(crate) slab: Mutex<ProducerSlab<T>>,
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
      .field("closed", &self.closed.load(Ordering::Relaxed))
      .finish()
  }
}

impl<T: Send> fmt::Debug for AsyncSender<T> {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("AsyncSender")
      .field("len", &self.len())
      .field("closed", &self.closed.load(Ordering::Relaxed))
      .finish()
  }
}

impl<T: Send> Sender<T> {
  pub(crate) fn from_shared(shared: Arc<MpscShared<T>>) -> Self {
    let shard = shared.next_shard();
    Sender {
      shared,
      closed: AtomicBool::new(false),
      slab: Mutex::new(ProducerSlab::new()),
      shard,
    }
  }

  pub fn send(&self, value: T) -> Result<(), SendError> {
    if self.closed.load(Ordering::Relaxed) {
      return Err(SendError::Closed);
    }
    send_internal(&self.shared, &self.slab, self.shard, value).map_err(|_| SendError::Closed)
  }

  /// Never fails with `Full`: the channel is unbounded.
  pub fn try_send(&self, value: T) -> Result<(), TrySendError<T>> {
    if self.closed.load(Ordering::Relaxed) {
      return Err(TrySendError::Closed(value));
    }
    send_internal(&self.shared, &self.slab, self.shard, value).map_err(TrySendError::Closed)
  }

  /// Sends a whole batch with a single publish and a single consumer wake.
  /// Unbounded, so this never blocks: `Ok(n)` means every item was sent. The
  /// only failure is a closed channel, reported before any item is consumed.
  pub fn send_batch(&self, items: Vec<T>) -> Result<usize, SendBatchError<T>> {
    self.try_send_batch(items).map_err(|e| SendBatchError {
      sent: e.sent,
      unsent: e.unsent,
    })
  }

  pub fn try_send_batch(&self, items: Vec<T>) -> Result<usize, TrySendBatchError<T>> {
    let total = items.len();
    if total == 0 {
      return Ok(0);
    }
    if self.closed.load(Ordering::Relaxed) || !self.shared.receivers_alive() {
      return Err(TrySendBatchError {
        sent: 0,
        unsent: items,
        reason: BatchSendErrorReason::Closed,
      });
    }
    let mut iter = items.into_iter();
    send_batch_internal(&self.shared, &self.slab, self.shard, &mut iter, total);
    Ok(total)
  }

  /// In-place batch send, draining all items from `items`. Never blocks. On
  /// `Err(SendError::Closed)` all items remain in `items`.
  pub fn send_batch_mut(&self, items: &mut Vec<T>) -> Result<usize, SendError> {
    if items.is_empty() {
      return Ok(0);
    }
    if self.closed.load(Ordering::Relaxed) || !self.shared.receivers_alive() {
      return Err(SendError::Closed);
    }
    let total = items.len();
    let mut drain = items.drain(..);
    send_batch_internal(&self.shared, &self.slab, self.shard, &mut drain, total);
    drop(drain);
    Ok(total)
  }

  pub fn try_send_batch_mut(&self, items: &mut Vec<T>) -> Result<usize, SendError> {
    self.send_batch_mut(items)
  }

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
    self.slab.lock().seal();
    self.shared.drop_sender();
  }

  pub fn is_closed(&self) -> bool {
    self.closed.load(Ordering::Relaxed) || !self.shared.receivers_alive()
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
    let closed = AtomicBool::new(self.closed.load(Ordering::Relaxed));
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
    AsyncSender {
      shared,
      closed: AtomicBool::new(false),
      slab: Mutex::new(ProducerSlab::new()),
      shard,
    }
  }

  /// Sends never block on an unbounded channel; the returned future resolves
  /// on first poll.
  pub fn send(&self, value: T) -> SendFuture<'_, T> {
    SendFuture {
      producer: self,
      value: Some(value),
      _phantom: PhantomPinned,
    }
  }

  pub fn try_send(&self, value: T) -> Result<(), TrySendError<T>> {
    if self.closed.load(Ordering::Relaxed) {
      return Err(TrySendError::Closed(value));
    }
    send_internal(&self.shared, &self.slab, self.shard, value).map_err(TrySendError::Closed)
  }

  pub fn send_batch(&self, items: Vec<T>) -> SendBatchFuture<'_, T> {
    SendBatchFuture {
      producer: self,
      items: Some(items),
      _phantom: PhantomPinned,
    }
  }

  pub fn try_send_batch(&self, items: Vec<T>) -> Result<usize, TrySendBatchError<T>> {
    let total = items.len();
    if total == 0 {
      return Ok(0);
    }
    if self.closed.load(Ordering::Relaxed) || !self.shared.receivers_alive() {
      return Err(TrySendBatchError {
        sent: 0,
        unsent: items,
        reason: BatchSendErrorReason::Closed,
      });
    }
    let mut iter = items.into_iter();
    send_batch_internal(&self.shared, &self.slab, self.shard, &mut iter, total);
    Ok(total)
  }

  pub fn send_batch_mut<'a>(&'a self, items: &'a mut Vec<T>) -> SendBatchMutFuture<'a, T> {
    SendBatchMutFuture {
      producer: self,
      items,
      _phantom: PhantomPinned,
    }
  }

  pub fn try_send_batch_mut(&self, items: &mut Vec<T>) -> Result<usize, SendError> {
    if items.is_empty() {
      return Ok(0);
    }
    if self.closed.load(Ordering::Relaxed) || !self.shared.receivers_alive() {
      return Err(SendError::Closed);
    }
    let total = items.len();
    let mut drain = items.drain(..);
    send_batch_internal(&self.shared, &self.slab, self.shard, &mut drain, total);
    drop(drain);
    Ok(total)
  }

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
    self.slab.lock().seal();
    self.shared.drop_sender();
  }

  pub fn is_closed(&self) -> bool {
    self.closed.load(Ordering::Relaxed) || !self.shared.receivers_alive()
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
    let closed = AtomicBool::new(self.closed.load(Ordering::Relaxed));
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
    if !self.closed.swap(true, Ordering::AcqRel) {
      self.close_internal();
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

// --- Futures ---------------------------------------------------------------------

#[must_use = "futures do nothing unless you .await or poll them"]
pub struct SendFuture<'a, T: Send> {
  producer: &'a AsyncSender<T>,
  value: Option<T>,
  _phantom: PhantomPinned,
}

impl<'a, T: Send> Future for SendFuture<'a, T> {
  type Output = Result<(), SendError>;

  fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
    let this = unsafe { self.as_mut().get_unchecked_mut() };
    if this.producer.closed.load(Ordering::Relaxed) {
      return Poll::Ready(Err(SendError::Closed));
    }
    let value = this
      .value
      .take()
      .expect("SendFuture polled after completion");
    Poll::Ready(
      send_internal(
        &this.producer.shared,
        &this.producer.slab,
        this.producer.shard,
        value,
      )
      .map_err(|_| SendError::Closed),
    )
  }
}

#[must_use = "futures do nothing unless you .await or poll them"]
pub struct SendBatchFuture<'a, T: Send> {
  producer: &'a AsyncSender<T>,
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
  producer: &'a AsyncSender<T>,
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
