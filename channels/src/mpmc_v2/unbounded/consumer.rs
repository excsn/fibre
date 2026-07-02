//! Receiver handles for the unbounded MPMC channel. Receivers are `Clone`;
//! waiting receives take `&mut self` so each handle can cache its waiter
//! cell/ctx and the hot path never allocates. Clone the receiver for each
//! thread or task that receives; non-waiting calls (`try_recv*`) stay `&self`.

use super::shared::{AsyncWaitCtx, RecvTimeoutOutcome, UnboundedShared, WaiterCell};
use crate::error::{CloseError, RecvError, RecvErrorTimeout, TryRecvError};

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use futures_core::Stream;

pub struct UnboundedSyncReceiver<T: Send> {
  pub(crate) shared: Arc<UnboundedShared<T>>,
  pub(crate) closed: AtomicBool,
  /// Cached landing pad for blocking waits, reused across `recv` calls.
  /// `&mut self` on the waiting methods guarantees one wait at a time.
  cell: Arc<WaiterCell<T>>,
}

pub struct UnboundedAsyncReceiver<T: Send> {
  pub(crate) shared: Arc<UnboundedShared<T>>,
  pub(crate) closed: AtomicBool,
  /// Registration state shared by the `Stream` impl and the `recv*` futures
  /// (which borrow it via `&mut self`), so polling never allocates.
  stream_ctx: AsyncWaitCtx<T>,
}

impl<T: Send> fmt::Debug for UnboundedSyncReceiver<T> {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("UnboundedSyncReceiver")
      .field("len", &self.len())
      .field("closed", &self.closed.load(Ordering::Relaxed))
      .finish()
  }
}

impl<T: Send> fmt::Debug for UnboundedAsyncReceiver<T> {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("UnboundedAsyncReceiver")
      .field("len", &self.len())
      .field("closed", &self.closed.load(Ordering::Relaxed))
      .finish()
  }
}

macro_rules! common_receiver_methods {
  () => {
    pub fn try_recv(&self) -> Result<T, TryRecvError> {
      if self.closed.load(Ordering::Relaxed) {
        return Err(TryRecvError::Disconnected);
      }
      self.shared.try_recv_internal()
    }

    pub fn try_recv_batch(&self, max: usize) -> Result<Vec<T>, TryRecvError> {
      let mut out = Vec::new();
      self.try_recv_batch_mut(&mut out, max)?;
      Ok(out)
    }

    pub fn try_recv_batch_mut(&self, out: &mut Vec<T>, max: usize) -> Result<usize, TryRecvError> {
      if max == 0 {
        return Ok(0);
      }
      if self.closed.load(Ordering::Relaxed) {
        return Err(TryRecvError::Disconnected);
      }
      self.shared.try_recv_batch_internal(out, max)
    }

    pub fn close(&self) -> Result<(), CloseError> {
      if self
        .closed
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
        .is_ok()
      {
        self.shared.drop_receiver();
        Ok(())
      } else {
        Err(CloseError)
      }
    }

    pub fn is_closed(&self) -> bool {
      self.closed.load(Ordering::Relaxed)
        || (!self.shared.senders_alive() && self.is_empty())
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

    /// Approximate: producers and other receivers act concurrently.
    pub fn is_empty(&self) -> bool {
      self.len() == 0
    }

    pub fn is_full(&self) -> bool {
      false
    }
  };
}

impl<T: Send> UnboundedSyncReceiver<T> {
  pub(crate) fn from_shared(shared: Arc<UnboundedShared<T>>) -> Self {
    UnboundedSyncReceiver {
      shared,
      closed: AtomicBool::new(false),
      cell: WaiterCell::new(),
    }
  }

  common_receiver_methods!();

  pub fn recv(&mut self) -> Result<T, RecvError> {
    if self.closed.load(Ordering::Relaxed) {
      return Err(RecvError::Disconnected);
    }
    match self.shared.recv_sync_internal(&self.cell, None) {
      Ok(v) => Ok(v),
      Err(RecvTimeoutOutcome::Disconnected) => Err(RecvError::Disconnected),
      Err(RecvTimeoutOutcome::Timeout) => unreachable!("no deadline was set"),
    }
  }

  pub fn recv_timeout(&mut self, timeout: Duration) -> Result<T, RecvErrorTimeout> {
    if self.closed.load(Ordering::Relaxed) {
      return Err(RecvErrorTimeout::Disconnected);
    }
    let deadline = Instant::now().checked_add(timeout);
    match self.shared.recv_sync_internal(&self.cell, deadline) {
      Ok(v) => Ok(v),
      Err(RecvTimeoutOutcome::Disconnected) => Err(RecvErrorTimeout::Disconnected),
      Err(RecvTimeoutOutcome::Timeout) => Err(RecvErrorTimeout::Timeout),
    }
  }

  /// Blocks until at least one item is available, then drains up to `max`.
  pub fn recv_batch(&mut self, max: usize) -> Result<Vec<T>, RecvError> {
    let mut out = Vec::new();
    self.recv_batch_mut(&mut out, max)?;
    Ok(out)
  }

  pub fn recv_batch_mut(&mut self, out: &mut Vec<T>, max: usize) -> Result<usize, RecvError> {
    if max == 0 {
      return Ok(0);
    }
    let first = self.recv()?;
    let _ = out.try_reserve_exact(max);
    out.push(first);
    let mut got = 1;
    if got < max {
      if let Ok(k) = self.shared.try_recv_batch_internal(out, max - got) {
        got += k;
      }
    }
    Ok(got)
  }

  pub fn to_async(self) -> UnboundedAsyncReceiver<T> {
    let shared = unsafe { std::ptr::read(&self.shared) };
    let cell = unsafe { std::ptr::read(&self.cell) };
    let closed = AtomicBool::new(self.closed.load(Ordering::Relaxed));
    std::mem::forget(self);
    // The cell may carry a consumed terminal state from a past wait; the
    // async ctx reads state before registering, so reset it here.
    cell.rearm();
    UnboundedAsyncReceiver {
      shared,
      closed,
      stream_ctx: AsyncWaitCtx {
        cell,
        registered: None,
      },
    }
  }
}

impl<T: Send> UnboundedAsyncReceiver<T> {
  pub(crate) fn from_shared(shared: Arc<UnboundedShared<T>>) -> Self {
    UnboundedAsyncReceiver {
      shared,
      closed: AtomicBool::new(false),
      stream_ctx: AsyncWaitCtx::new(),
    }
  }

  common_receiver_methods!();

  pub fn recv(&mut self) -> RecvFuture<'_, T> {
    RecvFuture { receiver: self }
  }

  /// Resolves with 1..=max items once at least one is available.
  pub fn recv_batch(&mut self, max: usize) -> RecvBatchFuture<'_, T> {
    RecvBatchFuture {
      receiver: self,
      max,
    }
  }

  pub fn recv_batch_mut<'a>(
    &'a mut self,
    out: &'a mut Vec<T>,
    max: usize,
  ) -> RecvBatchMutFuture<'a, T> {
    RecvBatchMutFuture {
      receiver: self,
      out,
      max,
    }
  }

  pub fn to_sync(self) -> UnboundedSyncReceiver<T> {
    let mut this = self;
    this.shared.cancel_wait(&mut this.stream_ctx);
    let shared = unsafe { std::ptr::read(&this.shared) };
    let ctx = unsafe { std::ptr::read(&this.stream_ctx) };
    let closed = AtomicBool::new(this.closed.load(Ordering::Relaxed));
    std::mem::forget(this);
    UnboundedSyncReceiver {
      shared,
      closed,
      cell: ctx.cell,
    }
  }
}

impl<T: Send> Clone for UnboundedSyncReceiver<T> {
  fn clone(&self) -> Self {
    self.shared.add_receiver();
    UnboundedSyncReceiver::from_shared(Arc::clone(&self.shared))
  }
}

impl<T: Send> Clone for UnboundedAsyncReceiver<T> {
  fn clone(&self) -> Self {
    self.shared.add_receiver();
    UnboundedAsyncReceiver::from_shared(Arc::clone(&self.shared))
  }
}

impl<T: Send> Drop for UnboundedSyncReceiver<T> {
  fn drop(&mut self) {
    if !self.closed.swap(true, Ordering::AcqRel) {
      self.shared.drop_receiver();
    }
  }
}

impl<T: Send> Drop for UnboundedAsyncReceiver<T> {
  fn drop(&mut self) {
    self.shared.cancel_wait(&mut self.stream_ctx);
    if !self.closed.swap(true, Ordering::AcqRel) {
      self.shared.drop_receiver();
    }
  }
}

impl<T: Send> Stream for UnboundedAsyncReceiver<T> {
  type Item = T;

  fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
    let this = self.get_mut();
    if this.closed.load(Ordering::Relaxed) {
      return Poll::Ready(None);
    }
    match this.shared.poll_recv_internal(cx.waker(), &mut this.stream_ctx) {
      Poll::Ready(Ok(v)) => Poll::Ready(Some(v)),
      Poll::Ready(Err(_)) => Poll::Ready(None),
      Poll::Pending => Poll::Pending,
    }
  }
}

// --- Futures --------------------------------------------------------------------
//
// Each future borrows the receiver mutably and polls through the handle's
// cached `stream_ctx`, so creating and resolving one allocates nothing. A
// registration left behind by a dropped-while-pending future (or the Stream
// impl) is picked up or cancelled by whichever wait uses the ctx next.

#[must_use = "futures do nothing unless you .await or poll them"]
pub struct RecvFuture<'a, T: Send> {
  receiver: &'a mut UnboundedAsyncReceiver<T>,
}

impl<'a, T: Send> Future for RecvFuture<'a, T> {
  type Output = Result<T, RecvError>;

  fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    let this = self.get_mut();
    if this.receiver.closed.load(Ordering::Relaxed) {
      return Poll::Ready(Err(RecvError::Disconnected));
    }
    this
      .receiver
      .shared
      .poll_recv_internal(cx.waker(), &mut this.receiver.stream_ctx)
  }
}

impl<'a, T: Send> Drop for RecvFuture<'a, T> {
  fn drop(&mut self) {
    self
      .receiver
      .shared
      .cancel_wait(&mut self.receiver.stream_ctx);
  }
}

#[must_use = "futures do nothing unless you .await or poll them"]
pub struct RecvBatchFuture<'a, T: Send> {
  receiver: &'a mut UnboundedAsyncReceiver<T>,
  max: usize,
}

impl<'a, T: Send> Future for RecvBatchFuture<'a, T> {
  type Output = Result<Vec<T>, RecvError>;

  fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    let this = self.get_mut();
    if this.max == 0 {
      return Poll::Ready(Ok(Vec::new()));
    }
    if this.receiver.closed.load(Ordering::Relaxed) {
      return Poll::Ready(Err(RecvError::Disconnected));
    }
    match this
      .receiver
      .shared
      .poll_recv_internal(cx.waker(), &mut this.receiver.stream_ctx)
    {
      Poll::Ready(Ok(first)) => {
        let mut out = Vec::new();
        let _ = out.try_reserve_exact(this.max);
        out.push(first);
        if out.len() < this.max {
          let _ = this
            .receiver
            .shared
            .try_recv_batch_internal(&mut out, this.max - 1);
        }
        Poll::Ready(Ok(out))
      }
      Poll::Ready(Err(e)) => Poll::Ready(Err(e)),
      Poll::Pending => Poll::Pending,
    }
  }
}

impl<'a, T: Send> Drop for RecvBatchFuture<'a, T> {
  fn drop(&mut self) {
    self
      .receiver
      .shared
      .cancel_wait(&mut self.receiver.stream_ctx);
  }
}

#[must_use = "futures do nothing unless you .await or poll them"]
pub struct RecvBatchMutFuture<'a, T: Send> {
  receiver: &'a mut UnboundedAsyncReceiver<T>,
  out: &'a mut Vec<T>,
  max: usize,
}

impl<'a, T: Send> Future for RecvBatchMutFuture<'a, T> {
  type Output = Result<usize, RecvError>;

  fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    let this = self.get_mut();
    if this.max == 0 {
      return Poll::Ready(Ok(0));
    }
    if this.receiver.closed.load(Ordering::Relaxed) {
      return Poll::Ready(Err(RecvError::Disconnected));
    }
    match this
      .receiver
      .shared
      .poll_recv_internal(cx.waker(), &mut this.receiver.stream_ctx)
    {
      Poll::Ready(Ok(first)) => {
        let _ = this.out.try_reserve_exact(this.max);
        this.out.push(first);
        let mut got = 1;
        if got < this.max {
          if let Ok(k) = this
            .receiver
            .shared
            .try_recv_batch_internal(this.out, this.max - got)
          {
            got += k;
          }
        }
        Poll::Ready(Ok(got))
      }
      Poll::Ready(Err(e)) => Poll::Ready(Err(e)),
      Poll::Pending => Poll::Pending,
    }
  }
}

impl<'a, T: Send> Drop for RecvBatchMutFuture<'a, T> {
  fn drop(&mut self) {
    self
      .receiver
      .shared
      .cancel_wait(&mut self.receiver.stream_ctx);
  }
}
