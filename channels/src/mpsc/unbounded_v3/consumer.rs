//! Receiver handles for the unbounded v3 MPSC channel.
//!
//! The consumer cursor lives in the shared state behind an `UnsafeCell`;
//! exclusivity is enforced at the type level: `Receiver` is `!Sync` (and
//! `!Clone`), and `AsyncReceiver` additionally takes `&mut self` on every
//! receive method so two futures can never race the cursor.

use super::shared::MpscShared;
use crate::error::{CloseError, RecvError, RecvErrorTimeout, TryRecvError};
use crate::sync_util;

use std::fmt;
use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};
use std::thread;
use std::time::{Duration, Instant};

use futures_core::Stream;

pub struct Receiver<T: Send> {
  pub(crate) shared: Arc<MpscShared<T>>,
  pub(crate) closed: AtomicBool,
  // Makes the handle !Sync (and !Send by default; Send is restored below):
  // the consumer cursor in `MpscShared` requires single-threaded access.
  _phantom: PhantomData<*mut ()>,
}

unsafe impl<T: Send> Send for Receiver<T> {}

pub struct AsyncReceiver<T: Send> {
  pub(crate) shared: Arc<MpscShared<T>>,
  pub(crate) closed: AtomicBool,
  is_registered: bool,
  _phantom: PhantomData<*mut ()>,
}

unsafe impl<T: Send> Send for AsyncReceiver<T> {}

impl<T: Send> fmt::Debug for Receiver<T> {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("Receiver")
      .field("len", &self.len())
      .field("closed", &self.closed.load(Ordering::Relaxed))
      .finish()
  }
}

impl<T: Send> fmt::Debug for AsyncReceiver<T> {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("AsyncReceiver")
      .field("len", &self.len())
      .field("closed", &self.closed.load(Ordering::Relaxed))
      .finish()
  }
}

// --- Sync Receiver ------------------------------------------------------------

impl<T: Send> Receiver<T> {
  pub(crate) fn from_shared(shared: Arc<MpscShared<T>>) -> Self {
    Receiver {
      shared,
      closed: AtomicBool::new(false),
      _phantom: PhantomData,
    }
  }

  pub fn try_recv(&self) -> Result<T, TryRecvError> {
    if self.closed.load(Ordering::Relaxed) {
      return Err(TryRecvError::Disconnected);
    }
    self.shared.try_recv_internal()
  }

  pub fn recv(&self) -> Result<T, RecvError> {
    if self.closed.load(Ordering::Relaxed) {
      return Err(RecvError::Disconnected);
    }
    let notified = AtomicBool::new(false);
    let notified_ptr = &notified as *const AtomicBool;
    let mut is_registered = false;

    loop {
      match self.shared.try_recv_internal() {
        Ok(v) => {
          if is_registered {
            self.shared.unregister_sync_recv();
          }
          return Ok(v);
        }
        Err(TryRecvError::Disconnected) => {
          if is_registered {
            self.shared.unregister_sync_recv();
          }
          return Err(RecvError::Disconnected);
        }
        Err(TryRecvError::Empty) => {}
      }

      if is_registered {
        sync_util::park_thread();
        if notified.swap(false, Ordering::Acquire) {
          is_registered = false;
        }
        continue;
      }

      self
        .shared
        .register_sync_recv(thread::current(), notified_ptr);
      is_registered = true;
      self.shared.pre_park_fence();
    }
  }

  pub fn recv_timeout(&self, timeout: Duration) -> Result<T, RecvErrorTimeout> {
    if self.closed.load(Ordering::Relaxed) {
      return Err(RecvErrorTimeout::Disconnected);
    }
    let deadline = Instant::now().checked_add(timeout);
    let notified = AtomicBool::new(false);
    let notified_ptr = &notified as *const AtomicBool;
    let mut is_registered = false;

    loop {
      match self.shared.try_recv_internal() {
        Ok(v) => {
          if is_registered {
            self.shared.unregister_sync_recv();
          }
          return Ok(v);
        }
        Err(TryRecvError::Disconnected) => {
          if is_registered {
            self.shared.unregister_sync_recv();
          }
          return Err(RecvErrorTimeout::Disconnected);
        }
        Err(TryRecvError::Empty) => {}
      }

      let now = Instant::now();
      let remaining = match deadline {
        Some(d) if d > now => d - now,
        _ => {
          if is_registered {
            self.shared.unregister_sync_recv();
          }
          return Err(RecvErrorTimeout::Timeout);
        }
      };

      if is_registered {
        sync_util::park_thread_timeout(remaining);
        if notified.swap(false, Ordering::Acquire) {
          is_registered = false;
        }
        continue;
      }

      self
        .shared
        .register_sync_recv(thread::current(), notified_ptr);
      is_registered = true;
      self.shared.pre_park_fence();
    }
  }

  /// Receives up to `max` items without blocking; see
  /// [`try_recv_batch_mut`](Self::try_recv_batch_mut).
  pub fn try_recv_batch(&self, max: usize) -> Result<Vec<T>, TryRecvError> {
    let mut out = Vec::new();
    self.try_recv_batch_mut(&mut out, max)?;
    Ok(out)
  }

  /// Appends up to `max` items to `out` without blocking, returning the count.
  pub fn try_recv_batch_mut(&self, out: &mut Vec<T>, max: usize) -> Result<usize, TryRecvError> {
    if max == 0 {
      return Ok(0);
    }
    if self.closed.load(Ordering::Relaxed) {
      return Err(TryRecvError::Disconnected);
    }
    self.shared.try_recv_batch_internal(out, max)
  }

  /// Blocks until at least one item is available, then drains up to `max`
  /// without further waiting.
  pub fn recv_batch(&self, max: usize) -> Result<Vec<T>, RecvError> {
    let mut out = Vec::new();
    self.recv_batch_mut(&mut out, max)?;
    Ok(out)
  }

  pub fn recv_batch_mut(&self, out: &mut Vec<T>, max: usize) -> Result<usize, RecvError> {
    if max == 0 {
      return Ok(0);
    }
    if self.closed.load(Ordering::Relaxed) {
      return Err(RecvError::Disconnected);
    }
    let notified = AtomicBool::new(false);
    let notified_ptr = &notified as *const AtomicBool;
    let mut is_registered = false;

    loop {
      match self.shared.try_recv_batch_internal(out, max) {
        Ok(k) => {
          if is_registered {
            self.shared.unregister_sync_recv();
          }
          return Ok(k);
        }
        Err(TryRecvError::Disconnected) => {
          if is_registered {
            self.shared.unregister_sync_recv();
          }
          return Err(RecvError::Disconnected);
        }
        Err(TryRecvError::Empty) => {}
      }

      if is_registered {
        sync_util::park_thread();
        if notified.swap(false, Ordering::Acquire) {
          is_registered = false;
        }
        continue;
      }

      self
        .shared
        .register_sync_recv(thread::current(), notified_ptr);
      is_registered = true;
      self.shared.pre_park_fence();
    }
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
    self.shared.drop_receiver();
    // Drain what was already published so buffered values (and their slabs)
    // are released promptly rather than at shared-state drop.
    while self.shared.try_recv_internal().is_ok() {}
  }

  pub fn is_closed(&self) -> bool {
    !self.shared.senders_alive() && self.is_empty()
  }

  pub fn sender_count(&self) -> usize {
    self.shared.sender_count()
  }

  pub fn len(&self) -> usize {
    self.shared.len()
  }

  /// Accurate from the consumer's side.
  pub fn is_empty(&self) -> bool {
    self.shared.consumer_is_empty()
  }

  pub fn to_async(self) -> AsyncReceiver<T> {
    let shared = unsafe { std::ptr::read(&self.shared) };
    let closed = AtomicBool::new(self.closed.load(Ordering::Relaxed));
    std::mem::forget(self);
    AsyncReceiver {
      shared,
      closed,
      is_registered: false,
      _phantom: PhantomData,
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

// --- Async Receiver -------------------------------------------------------------

impl<T: Send> AsyncReceiver<T> {
  pub(crate) fn from_shared(shared: Arc<MpscShared<T>>) -> Self {
    AsyncReceiver {
      shared,
      closed: AtomicBool::new(false),
      is_registered: false,
      _phantom: PhantomData,
    }
  }

  pub fn recv(&mut self) -> RecvFuture<'_, T> {
    RecvFuture {
      receiver: self,
      is_registered: false,
    }
  }

  pub fn try_recv(&mut self) -> Result<T, TryRecvError> {
    if self.closed.load(Ordering::Relaxed) {
      return Err(TryRecvError::Disconnected);
    }
    self.shared.try_recv_internal()
  }

  /// Resolves with 1..=max items once at least one is available.
  pub fn recv_batch(&mut self, max: usize) -> RecvBatchFuture<'_, T> {
    RecvBatchFuture {
      receiver: self,
      max,
      is_registered: false,
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
      is_registered: false,
    }
  }

  pub fn try_recv_batch(&mut self, max: usize) -> Result<Vec<T>, TryRecvError> {
    let mut out = Vec::new();
    self.try_recv_batch_mut(&mut out, max)?;
    Ok(out)
  }

  pub fn try_recv_batch_mut(
    &mut self,
    out: &mut Vec<T>,
    max: usize,
  ) -> Result<usize, TryRecvError> {
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
      self.close_internal();
      Ok(())
    } else {
      Err(CloseError)
    }
  }

  fn close_internal(&self) {
    self.shared.drop_receiver();
    while self.shared.try_recv_internal().is_ok() {}
  }

  pub fn is_closed(&self) -> bool {
    !self.shared.senders_alive() && self.is_empty()
  }

  pub fn sender_count(&self) -> usize {
    self.shared.sender_count()
  }

  pub fn len(&self) -> usize {
    self.shared.len()
  }

  /// Accurate from the consumer's side.
  pub fn is_empty(&self) -> bool {
    self.shared.consumer_is_empty()
  }

  pub fn to_sync(self) -> Receiver<T> {
    if self.is_registered {
      self.shared.unregister_async_recv();
    }
    let shared = unsafe { std::ptr::read(&self.shared) };
    let closed = AtomicBool::new(self.closed.load(Ordering::Relaxed));
    std::mem::forget(self);
    Receiver {
      shared,
      closed,
      _phantom: PhantomData,
    }
  }
}

impl<T: Send> Drop for AsyncReceiver<T> {
  fn drop(&mut self) {
    if self.is_registered {
      self.shared.unregister_async_recv();
    }
    if !self.closed.swap(true, Ordering::AcqRel) {
      self.close_internal();
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
    match this.shared.poll_recv_internal(cx, &mut this.is_registered) {
      Poll::Ready(Ok(value)) => Poll::Ready(Some(value)),
      Poll::Ready(Err(_)) => Poll::Ready(None),
      Poll::Pending => Poll::Pending,
    }
  }
}

// --- Futures ----------------------------------------------------------------------

#[must_use = "futures do nothing unless you .await or poll them"]
pub struct RecvFuture<'a, T: Send> {
  receiver: &'a mut AsyncReceiver<T>,
  is_registered: bool,
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
      .poll_recv_internal(cx, &mut this.is_registered)
  }
}

impl<'a, T: Send> Drop for RecvFuture<'a, T> {
  fn drop(&mut self) {
    if self.is_registered {
      self.receiver.shared.unregister_async_recv();
    }
  }
}

#[must_use = "futures do nothing unless you .await or poll them"]
pub struct RecvBatchFuture<'a, T: Send> {
  receiver: &'a mut AsyncReceiver<T>,
  max: usize,
  is_registered: bool,
}

impl<'a, T: Send> Future for RecvBatchFuture<'a, T> {
  type Output = Result<Vec<T>, RecvError>;

  fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    let this = self.get_mut();
    if this.receiver.closed.load(Ordering::Relaxed) {
      return Poll::Ready(Err(RecvError::Disconnected));
    }
    let max = this.max;
    // Only returns Pending while `out` is untouched (k == 0), so building the
    // Vec inside poll is loss-free.
    let mut out = Vec::new();
    match this
      .receiver
      .shared
      .poll_recv_batch_internal(cx, &mut out, max, &mut this.is_registered)
    {
      Poll::Ready(Ok(_)) => Poll::Ready(Ok(out)),
      Poll::Ready(Err(e)) => Poll::Ready(Err(e)),
      Poll::Pending => Poll::Pending,
    }
  }
}

impl<'a, T: Send> Drop for RecvBatchFuture<'a, T> {
  fn drop(&mut self) {
    if self.is_registered {
      self.receiver.shared.unregister_async_recv();
    }
  }
}

#[must_use = "futures do nothing unless you .await or poll them"]
pub struct RecvBatchMutFuture<'a, T: Send> {
  receiver: &'a mut AsyncReceiver<T>,
  out: &'a mut Vec<T>,
  max: usize,
  is_registered: bool,
}

impl<'a, T: Send> Future for RecvBatchMutFuture<'a, T> {
  type Output = Result<usize, RecvError>;

  fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    let this = self.get_mut();
    if this.receiver.closed.load(Ordering::Relaxed) {
      return Poll::Ready(Err(RecvError::Disconnected));
    }
    let max = this.max;
    this
      .receiver
      .shared
      .poll_recv_batch_internal(cx, this.out, max, &mut this.is_registered)
  }
}

impl<'a, T: Send> Drop for RecvBatchMutFuture<'a, T> {
  fn drop(&mut self) {
    if self.is_registered {
      self.receiver.shared.unregister_async_recv();
    }
  }
}
