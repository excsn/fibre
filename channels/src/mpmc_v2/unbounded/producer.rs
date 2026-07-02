//! Producer handles for the unbounded MPMC channel: mpsc v3's lock-free
//! slab-chain publish with MPMC semantics. Sends never block.
//!
//! Sending takes `&mut self`: the bump slab is producer-private state, so
//! exclusive access comes from the borrow checker instead of a per-handle
//! lock. Clone the sender for each thread or task that sends.

use super::shared::UnboundedShared;
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

fn send_internal<T: Send>(
  shared: &Arc<UnboundedShared<T>>,
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
  shared.notify_receivers();
  Ok(())
}

fn send_batch_internal<T: Send>(
  shared: &Arc<UnboundedShared<T>>,
  slab: &mut ProducerSlab<T>,
  shard: usize,
  iter: &mut impl Iterator<Item = T>,
  count: usize,
) {
  debug_assert!(count > 0);
  let (first, last) = slab.bump_batch(iter, count);
  shared.publish(first, last);
  shared.record_sent(shard, count);
  shared.notify_receivers();
}

pub struct UnboundedSyncSender<T: Send> {
  pub(crate) shared: Arc<UnboundedShared<T>>,
  pub(crate) closed: bool,
  slab: ProducerSlab<T>,
  shard: usize,
}

pub struct UnboundedAsyncSender<T: Send> {
  pub(crate) shared: Arc<UnboundedShared<T>>,
  pub(crate) closed: bool,
  slab: ProducerSlab<T>,
  shard: usize,
}

unsafe impl<T: Send> Send for UnboundedSyncSender<T> {}
unsafe impl<T: Send> Sync for UnboundedSyncSender<T> {}
unsafe impl<T: Send> Send for UnboundedAsyncSender<T> {}
unsafe impl<T: Send> Sync for UnboundedAsyncSender<T> {}

impl<T: Send> fmt::Debug for UnboundedSyncSender<T> {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("UnboundedSyncSender")
      .field("len", &self.len())
      .field("closed", &self.closed)
      .finish()
  }
}

impl<T: Send> fmt::Debug for UnboundedAsyncSender<T> {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("UnboundedAsyncSender")
      .field("len", &self.len())
      .field("closed", &self.closed)
      .finish()
  }
}

macro_rules! common_sender_methods {
  () => {
    pub(crate) fn from_shared(shared: Arc<UnboundedShared<T>>) -> Self {
      let shard = shared.next_shard();
      let slab = ProducerSlab::new(shared.slab_pool());
      Self {
        shared,
        closed: false,
        slab,
        shard,
      }
    }

    /// Never fails with `Full`: the channel is unbounded.
    pub fn try_send(&mut self, value: T) -> Result<(), TrySendError<T>> {
      if self.closed {
        return Err(TrySendError::Closed(value));
      }
      send_internal(&self.shared, &mut self.slab, self.shard, value).map_err(TrySendError::Closed)
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

    pub fn capacity(&self) -> usize {
      usize::MAX
    }

    pub fn len(&self) -> usize {
      self.shared.len()
    }

    /// Approximate: another handle may send, or a receiver consume,
    /// concurrently with this check.
    pub fn is_empty(&self) -> bool {
      self.len() == 0
    }

    pub fn is_full(&self) -> bool {
      false
    }
  };
}

impl<T: Send> UnboundedSyncSender<T> {
  common_sender_methods!();

  pub fn send(&mut self, value: T) -> Result<(), SendError> {
    if self.closed {
      return Err(SendError::Closed);
    }
    send_internal(&self.shared, &mut self.slab, self.shard, value).map_err(|_| SendError::Closed)
  }

  /// Unbounded: never blocks; `Ok(n)` means every item was sent.
  pub fn send_batch(&mut self, items: Vec<T>) -> Result<usize, SendBatchError<T>> {
    self.try_send_batch(items).map_err(|e| SendBatchError {
      sent: e.sent,
      unsent: e.unsent,
    })
  }

  pub fn send_batch_mut(&mut self, items: &mut Vec<T>) -> Result<usize, SendError> {
    self.try_send_batch_mut(items)
  }

  pub fn to_async(self) -> UnboundedAsyncSender<T> {
    let shared = unsafe { ptr::read(&self.shared) };
    let slab = unsafe { ptr::read(&self.slab) };
    let shard = self.shard;
    let closed = self.closed;
    std::mem::forget(self);
    UnboundedAsyncSender {
      shared,
      closed,
      slab,
      shard,
    }
  }
}

impl<T: Send> UnboundedAsyncSender<T> {
  common_sender_methods!();

  /// Sends never block on an unbounded channel; the future resolves on first
  /// poll.
  pub fn send(&mut self, value: T) -> SendFuture<'_, T> {
    SendFuture {
      sender: self,
      value: Some(value),
      _phantom: PhantomPinned,
    }
  }

  pub fn send_batch(&mut self, items: Vec<T>) -> SendBatchFuture<'_, T> {
    SendBatchFuture {
      sender: self,
      items: Some(items),
      _phantom: PhantomPinned,
    }
  }

  pub fn send_batch_mut<'a>(&'a mut self, items: &'a mut Vec<T>) -> SendBatchMutFuture<'a, T> {
    SendBatchMutFuture {
      sender: self,
      items,
      _phantom: PhantomPinned,
    }
  }

  pub fn to_sync(self) -> UnboundedSyncSender<T> {
    let shared = unsafe { ptr::read(&self.shared) };
    let slab = unsafe { ptr::read(&self.slab) };
    let shard = self.shard;
    let closed = self.closed;
    std::mem::forget(self);
    UnboundedSyncSender {
      shared,
      closed,
      slab,
      shard,
    }
  }
}

impl<T: Send> Clone for UnboundedSyncSender<T> {
  fn clone(&self) -> Self {
    self.shared.add_sender();
    Self::from_shared(Arc::clone(&self.shared))
  }
}

impl<T: Send> Clone for UnboundedAsyncSender<T> {
  fn clone(&self) -> Self {
    self.shared.add_sender();
    Self::from_shared(Arc::clone(&self.shared))
  }
}

impl<T: Send> Drop for UnboundedSyncSender<T> {
  fn drop(&mut self) {
    if !self.closed {
      self.closed = true;
      self.close_internal();
    }
  }
}

impl<T: Send> Drop for UnboundedAsyncSender<T> {
  fn drop(&mut self) {
    if !self.closed {
      self.closed = true;
      self.close_internal();
    }
  }
}

// --- Futures (immediate-resolve: unbounded sends cannot wait) --------------------

#[must_use = "futures do nothing unless you .await or poll them"]
pub struct SendFuture<'a, T: Send> {
  sender: &'a mut UnboundedAsyncSender<T>,
  value: Option<T>,
  _phantom: PhantomPinned,
}

impl<'a, T: Send> Future for SendFuture<'a, T> {
  type Output = Result<(), SendError>;

  fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
    let this = unsafe { self.as_mut().get_unchecked_mut() };
    if this.sender.closed {
      return Poll::Ready(Err(SendError::Closed));
    }
    let value = this
      .value
      .take()
      .expect("SendFuture polled after completion");
    Poll::Ready(
      send_internal(
        &this.sender.shared,
        &mut this.sender.slab,
        this.sender.shard,
        value,
      )
      .map_err(|_| SendError::Closed),
    )
  }
}

#[must_use = "futures do nothing unless you .await or poll them"]
pub struct SendBatchFuture<'a, T: Send> {
  sender: &'a mut UnboundedAsyncSender<T>,
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
    Poll::Ready(this.sender.try_send_batch(items).map_err(|e| SendBatchError {
      sent: e.sent,
      unsent: e.unsent,
    }))
  }
}

#[must_use = "futures do nothing unless you .await or poll them"]
pub struct SendBatchMutFuture<'a, T: Send> {
  sender: &'a mut UnboundedAsyncSender<T>,
  items: &'a mut Vec<T>,
  _phantom: PhantomPinned,
}

impl<'a, T: Send> Future for SendBatchMutFuture<'a, T> {
  type Output = Result<usize, SendError>;

  fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
    let this = unsafe { self.as_mut().get_unchecked_mut() };
    Poll::Ready(this.sender.try_send_batch_mut(this.items))
  }
}
