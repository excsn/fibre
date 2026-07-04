use futures_core::Stream;

use super::bounded_sync::{BoundedSyncReceiver, BoundedSyncSender};
use crate::error::{
  BatchSendErrorReason, CloseError, RecvError, SendBatchError, SendError, TryRecvError,
  TrySendBatchError, TrySendError,
};
use crate::spsc::shared::{Role, SpscShared, WakeRef};

use core::marker::PhantomPinned;
use std::future::Future;
use std::mem;
use std::pin::Pin;
use std::task::{Context, Poll};

// Sync primitives via the loom facade (see `internal/sync.rs`).
use crate::internal::sync::{Arc, AtomicBool, Ordering};

// --- Async Sender ---
#[derive(Debug)]
pub struct BoundedAsyncSender<T> {
  pub(crate) shared: Arc<SpscShared<T>>,
  pub(crate) closed: AtomicBool,
}

// --- Async Receiver ---
#[derive(Debug)]
pub struct BoundedAsyncReceiver<T> {
  pub(crate) shared: Arc<SpscShared<T>>,
  pub(crate) closed: AtomicBool,
  is_registered: bool,
}

unsafe impl<T: Send> Send for BoundedAsyncSender<T> {}
unsafe impl<T: Send> Send for BoundedAsyncReceiver<T> {}

impl<T> BoundedAsyncSender<T> {
  fn close_internal(&self) {
    self.shared.producer_dropped.store(true, Ordering::Release);
    self.shared.drop_sender();
  }
}

impl<T: Send> BoundedAsyncSender<T> {
  pub(crate) fn from_shared(shared: Arc<SpscShared<T>>) -> Self {
    Self {
      shared,
      closed: AtomicBool::new(false),
    }
  }

  pub fn to_sync(self) -> BoundedSyncSender<T> {
    let shared = unsafe { std::ptr::read(&self.shared) };
    mem::forget(self);
    BoundedSyncSender::from_shared(shared)
  }

  pub fn send(&mut self, item: T) -> SendFuture<'_, T> {
    SendFuture::new(self, item)
  }

  pub fn try_send(&mut self, item: T) -> Result<(), TrySendError<T>> {
    if self.closed.load(Ordering::Relaxed) {
      return Err(TrySendError::Closed(item));
    }
    if self.shared.consumer_dropped.load(Ordering::Acquire) {
      return Err(TrySendError::Closed(item));
    }
    match self.shared.ring.push(item) {
      Ok(()) => {
        self.shared.notify_receivers();
        Ok(())
      }
      Err(returned) => Err(TrySendError::Full(returned)),
    }
  }

  pub fn send_batch(&mut self, items: Vec<T>) -> SendBatchFuture<'_, T> {
    let total = items.len();
    SendBatchFuture {
      sender: self,
      iter: items.into_iter(),
      total,
      sent: 0,
      is_registered: false,
      _phantom: PhantomPinned,
    }
  }

  pub fn send_batch_mut<'a>(&'a mut self, items: &'a mut Vec<T>) -> SendBatchMutFuture<'a, T> {
    SendBatchMutFuture {
      sender: self,
      items,
      sent: 0,
      is_registered: false,
      _phantom: PhantomPinned,
    }
  }

  pub fn try_send_batch(&mut self, items: Vec<T>) -> Result<usize, TrySendBatchError<T>> {
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

  pub fn try_send_batch_mut(&mut self, items: &mut Vec<T>) -> Result<usize, SendError> {
    if items.is_empty() {
      return Ok(0);
    }
    if self.closed.load(Ordering::Relaxed) || self.shared.consumer_dropped.load(Ordering::Acquire) {
      return Err(SendError::Closed);
    }
    let batch = std::mem::take(items);
    match self.try_send_batch(batch) {
      Ok(n) => Ok(n),
      Err(e) => {
        let (sent, reason) = (e.sent, e.reason);
        *items = e.unsent;
        if sent == 0 && matches!(reason, BatchSendErrorReason::Closed) {
          Err(SendError::Closed)
        } else {
          Ok(sent)
        }
      }
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

  pub fn is_closed(&self) -> bool {
    self.closed.load(Ordering::Relaxed) || self.shared.consumer_dropped.load(Ordering::Acquire)
  }

  pub fn capacity(&self) -> usize {
    self.shared.capacity()
  }

  #[inline]
  pub fn len(&self) -> usize {
    self.shared.len()
  }

  #[inline]
  pub fn is_empty(&self) -> bool {
    self.shared.is_empty()
  }

  #[inline]
  pub fn is_full(&self) -> bool {
    self.shared.is_full()
  }
}

// Methods that do not require T: Send (e.g., for Drop)
impl<T> BoundedAsyncReceiver<T> {
  fn close_internal(&self) {
    self.shared.consumer_dropped.store(true, Ordering::Release);
    self.shared.drop_receiver();
  }
}

// Methods that require T: Send
impl<T: Send> BoundedAsyncReceiver<T> {
  pub(crate) fn from_shared(shared: Arc<SpscShared<T>>) -> Self {
    Self {
      shared,
      closed: AtomicBool::new(false),
      is_registered: false,
    }
  }

  pub fn to_sync(self) -> BoundedSyncReceiver<T> {
    if self.is_registered {
      self.shared.unregister(Role::Recv);
    }
    let shared = unsafe { std::ptr::read(&self.shared) };
    mem::forget(self);
    BoundedSyncReceiver::from_shared(shared)
  }

  pub fn recv(&mut self) -> ReceiveFuture<'_, T> {
    ReceiveFuture::new(self)
  }

  pub fn try_recv(&mut self) -> Result<T, TryRecvError> {
    if self.closed.load(Ordering::Relaxed) {
      return Err(TryRecvError::Disconnected);
    }
    match self.shared.ring.pop() {
      Some(item) => {
        self.shared.notify_senders();
        Ok(item)
      }
      None => {
        if !self.shared.senders_alive() {
          if let Some(item) = self.shared.ring.pop() {
            self.shared.notify_senders();
            return Ok(item);
          }
          Err(TryRecvError::Disconnected)
        } else {
          Err(TryRecvError::Empty)
        }
      }
    }
  }

  pub fn recv_batch(&mut self, max: usize) -> RecvBatchFuture<'_, T> {
    RecvBatchFuture {
      receiver: self,
      max,
      is_registered: false,
    }
  }

  pub fn recv_batch_mut<'a>(&'a mut self, out: &'a mut Vec<T>, max: usize) -> RecvBatchMutFuture<'a, T> {
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

  pub fn try_recv_batch_mut(&mut self, out: &mut Vec<T>, max: usize) -> Result<usize, TryRecvError> {
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
    if !self.shared.senders_alive() {
      let k = self.shared.read_batch(out, max);
      if k > 0 {
        return Ok(k);
      }
      return Err(TryRecvError::Disconnected);
    }
    Err(TryRecvError::Empty)
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

  pub fn is_closed(&self) -> bool {
    self.closed.load(Ordering::Relaxed)
      || (!self.shared.senders_alive() && self.shared.is_empty())
  }

  pub fn capacity(&self) -> usize {
    self.shared.capacity()
  }

  #[inline]
  pub fn len(&self) -> usize {
    self.shared.len()
  }

  #[inline]
  pub fn is_empty(&self) -> bool {
    self.shared.is_empty()
  }

  #[inline]
  pub fn is_full(&self) -> bool {
    self.shared.is_full()
  }
}

pub fn bounded_async<T: Send>(
  capacity: usize,
) -> (BoundedAsyncSender<T>, BoundedAsyncReceiver<T>) {
  let shared_core = SpscShared::new_internal(capacity);
  let shared_arc = Arc::new(shared_core);

  (
    BoundedAsyncSender {
      shared: Arc::clone(&shared_arc),
      closed: AtomicBool::new(false),
    },
    BoundedAsyncReceiver {
      shared: shared_arc,
      closed: AtomicBool::new(false),
      is_registered: false,
    },
  )
}

impl<T> Drop for BoundedAsyncSender<T> {
  fn drop(&mut self) {
    if !self.closed.swap(true, Ordering::AcqRel) {
      self.close_internal();
    }
  }
}

impl<T> Drop for BoundedAsyncReceiver<T> {
  fn drop(&mut self) {
    if self.is_registered {
      self.shared.unregister(Role::Recv);
    }
    if !self.closed.swap(true, Ordering::AcqRel) {
      self.close_internal();
    }
  }
}

#[must_use = "futures do nothing unless you .await or poll them"]
pub struct SendFuture<'a, T> {
  sender: &'a mut BoundedAsyncSender<T>,
  item: Option<T>,
  is_registered: bool,
  _phantom: PhantomPinned,
}

impl<'a, T> SendFuture<'a, T> {
  fn new(sender: &'a mut BoundedAsyncSender<T>, item: T) -> Self {
    SendFuture {
      sender,
      item: Some(item),
      is_registered: false,
      _phantom: PhantomPinned,
    }
  }
}

impl<'a, T: Send> Future for SendFuture<'a, T> {
  type Output = Result<(), SendError>;

  fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    let this = unsafe { self.as_mut().get_unchecked_mut() };
    let shared = &this.sender.shared;

    loop {
      let mut item = match this.item.take() {
        Some(it) => it,
        None => return Poll::Ready(Ok(())),
      };

      if this.sender.closed.load(Ordering::Relaxed)
        || shared.consumer_dropped.load(Ordering::Acquire)
      {
        if this.is_registered {
          shared.unregister(Role::Send);
          this.is_registered = false;
        }
        return Poll::Ready(Err(SendError::Closed));
      }

      match shared.ring.push(item) {
        Ok(()) => {
          if this.is_registered {
            shared.unregister(Role::Send);
            this.is_registered = false;
          }
          shared.notify_receivers();
          return Poll::Ready(Ok(()));
        }
        Err(returned) => item = returned,
      }

      shared.register(Role::Send, WakeRef::Waker(cx.waker().clone()), std::ptr::null());
      this.is_registered = true;
      shared.pre_park_fence();

      if this.sender.closed.load(Ordering::Relaxed)
        || shared.consumer_dropped.load(Ordering::Acquire)
      {
        if this.is_registered {
          shared.unregister(Role::Send);
          this.is_registered = false;
        }
        this.item = Some(item);
        return Poll::Ready(Err(SendError::Closed));
      }

      match shared.ring.push(item) {
        Ok(()) => {
          if this.is_registered {
            shared.unregister(Role::Send);
            this.is_registered = false;
          }
          shared.notify_receivers();
          return Poll::Ready(Ok(()));
        }
        Err(returned) => {
          this.item = Some(returned);
          return Poll::Pending;
        }
      }
    }
  }
}

impl<'a, T> Drop for SendFuture<'a, T> {
  fn drop(&mut self) {
    if self.is_registered {
      self.sender.shared.unregister(Role::Send);
    }
  }
}

/// Future returned by [`BoundedAsyncSender::send_batch`].
#[must_use = "futures do nothing unless you .await or poll them"]
pub struct SendBatchFuture<'a, T> {
  sender: &'a mut BoundedAsyncSender<T>,
  iter: std::vec::IntoIter<T>,
  total: usize,
  sent: usize,
  is_registered: bool,
  _phantom: PhantomPinned,
}

impl<'a, T: Send> Future for SendBatchFuture<'a, T> {
  type Output = Result<usize, SendBatchError<T>>;

  fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    let this = unsafe { self.as_mut().get_unchecked_mut() };
    let shared = &this.sender.shared;
    loop {
      if this.sent == this.total {
        if this.is_registered {
          shared.unregister(Role::Send);
          this.is_registered = false;
        }
        return Poll::Ready(Ok(this.total));
      }
      if this.sender.closed.load(Ordering::Relaxed)
        || shared.consumer_dropped.load(Ordering::Acquire)
      {
        if this.is_registered {
          shared.unregister(Role::Send);
          this.is_registered = false;
        }
        return Poll::Ready(Err(SendBatchError {
          sent: this.sent,
          unsent: this.iter.by_ref().collect(),
        }));
      }

      let written = shared.write_batch(&mut this.iter, this.total - this.sent);
      this.sent += written;
      if this.sent == this.total {
        if this.is_registered {
          shared.unregister(Role::Send);
          this.is_registered = false;
        }
        return Poll::Ready(Ok(this.total));
      }

      shared.register(Role::Send, WakeRef::Waker(cx.waker().clone()), std::ptr::null());
      this.is_registered = true;
      shared.pre_park_fence();

      let written_after = shared.write_batch(&mut this.iter, this.total - this.sent);
      this.sent += written_after;
      if this.sent == this.total {
        shared.unregister(Role::Send);
        this.is_registered = false;
        return Poll::Ready(Ok(this.total));
      }
      if this.sender.closed.load(Ordering::Relaxed)
        || shared.consumer_dropped.load(Ordering::Acquire)
      {
        continue;
      }
      return Poll::Pending;
    }
  }
}

impl<'a, T> Drop for SendBatchFuture<'a, T> {
  fn drop(&mut self) {
    if self.is_registered {
      self.sender.shared.unregister(Role::Send);
    }
  }
}

/// Future returned by [`BoundedAsyncSender::send_batch_mut`].
#[must_use = "futures do nothing unless you .await or poll them"]
pub struct SendBatchMutFuture<'a, T> {
  sender: &'a mut BoundedAsyncSender<T>,
  items: &'a mut Vec<T>,
  sent: usize,
  is_registered: bool,
  _phantom: PhantomPinned,
}

impl<'a, T: Send> Future for SendBatchMutFuture<'a, T> {
  type Output = Result<usize, SendError>;

  fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    let this = unsafe { self.as_mut().get_unchecked_mut() };
    let shared = &this.sender.shared;
    loop {
      if this.items.is_empty() {
        if this.is_registered {
          shared.unregister(Role::Send);
          this.is_registered = false;
        }
        return Poll::Ready(Ok(this.sent));
      }
      if this.sender.closed.load(Ordering::Relaxed)
        || shared.consumer_dropped.load(Ordering::Acquire)
      {
        if this.is_registered {
          shared.unregister(Role::Send);
          this.is_registered = false;
        }
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

      shared.register(Role::Send, WakeRef::Waker(cx.waker().clone()), std::ptr::null());
      this.is_registered = true;
      shared.pre_park_fence();

      let k_after = shared.available_space().min(this.items.len());
      if k_after > 0 {
        let mut drain = this.items.drain(..k_after);
        let written = shared.write_batch(&mut drain, k_after);
        debug_assert_eq!(written, k_after);
        drop(drain);
        this.sent += k_after;
        continue;
      }
      if this.sender.closed.load(Ordering::Relaxed)
        || shared.consumer_dropped.load(Ordering::Acquire)
      {
        continue;
      }
      return Poll::Pending;
    }
  }
}

impl<'a, T> Drop for SendBatchMutFuture<'a, T> {
  fn drop(&mut self) {
    if self.is_registered {
      self.sender.shared.unregister(Role::Send);
    }
  }
}

#[must_use = "futures do nothing unless you .await or poll them"]
pub struct ReceiveFuture<'a, T> {
  receiver: &'a mut BoundedAsyncReceiver<T>,
  is_registered: bool,
}

impl<'a, T> ReceiveFuture<'a, T> {
  fn new(receiver: &'a mut BoundedAsyncReceiver<T>) -> Self {
    ReceiveFuture {
      receiver,
      is_registered: false,
    }
  }
}

impl<'a, T: Send> Future for ReceiveFuture<'a, T> {
  type Output = Result<T, RecvError>;

  fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    let this = unsafe { self.as_mut().get_unchecked_mut() };
    if this.receiver.closed.load(Ordering::Relaxed) {
      return Poll::Ready(Err(RecvError::Disconnected));
    }
    this.receiver.shared.poll_recv_internal(cx, &mut this.is_registered)
  }
}

impl<'a, T> Drop for ReceiveFuture<'a, T> {
  fn drop(&mut self) {
    if self.is_registered {
      self.receiver.shared.unregister(Role::Recv);
    }
  }
}

/// Future returned by [`BoundedAsyncReceiver::recv_batch`].
#[must_use = "futures do nothing unless you .await or poll them"]
pub struct RecvBatchFuture<'a, T> {
  receiver: &'a mut BoundedAsyncReceiver<T>,
  max: usize,
  is_registered: bool,
}

impl<'a, T: Send> Future for RecvBatchFuture<'a, T> {
  type Output = Result<Vec<T>, RecvError>;

  fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    let this = unsafe { self.as_mut().get_unchecked_mut() };
    if this.max == 0 {
      return Poll::Ready(Ok(Vec::new()));
    }
    let mut out = Vec::new();
    match poll_recv_batch_spsc(this.receiver, cx, &mut out, this.max, &mut this.is_registered) {
      Poll::Ready(Ok(_)) => Poll::Ready(Ok(out)),
      Poll::Ready(Err(e)) => Poll::Ready(Err(e)),
      Poll::Pending => Poll::Pending,
    }
  }
}

impl<'a, T> Drop for RecvBatchFuture<'a, T> {
  fn drop(&mut self) {
    if self.is_registered {
      self.receiver.shared.unregister(Role::Recv);
    }
  }
}

/// Future returned by [`BoundedAsyncReceiver::recv_batch_mut`].
#[must_use = "futures do nothing unless you .await or poll them"]
pub struct RecvBatchMutFuture<'a, T> {
  receiver: &'a mut BoundedAsyncReceiver<T>,
  out: &'a mut Vec<T>,
  max: usize,
  is_registered: bool,
}

impl<'a, T: Send> Future for RecvBatchMutFuture<'a, T> {
  type Output = Result<usize, RecvError>;

  fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    let this = unsafe { self.as_mut().get_unchecked_mut() };
    if this.max == 0 {
      return Poll::Ready(Ok(0));
    }
    let max = this.max;
    poll_recv_batch_spsc(this.receiver, cx, this.out, max, &mut this.is_registered)
  }
}

impl<'a, T> Drop for RecvBatchMutFuture<'a, T> {
  fn drop(&mut self) {
    if self.is_registered {
      self.receiver.shared.unregister(Role::Recv);
    }
  }
}

fn poll_recv_batch_spsc<T: Send>(
  receiver: &BoundedAsyncReceiver<T>,
  cx: &mut Context<'_>,
  out: &mut Vec<T>,
  max: usize,
  is_registered: &mut bool,
) -> Poll<Result<usize, RecvError>> {
  if receiver.closed.load(Ordering::Relaxed) {
    return Poll::Ready(Err(RecvError::Disconnected));
  }
  let shared = &receiver.shared;
  loop {
    let k = shared.read_batch(out, max);
    if k > 0 {
      if *is_registered {
        shared.unregister(Role::Recv);
        *is_registered = false;
      }
      return Poll::Ready(Ok(k));
    }

    if shared.producer_dropped.load(Ordering::Acquire) {
      let k = shared.read_batch(out, max);
      if k > 0 {
        if *is_registered {
          shared.unregister(Role::Recv);
          *is_registered = false;
        }
        return Poll::Ready(Ok(k));
      }
      if *is_registered {
        shared.unregister(Role::Recv);
        *is_registered = false;
      }
      return Poll::Ready(Err(RecvError::Disconnected));
    }

    shared.register(Role::Recv, WakeRef::Waker(cx.waker().clone()), std::ptr::null());
    *is_registered = true;
    shared.pre_park_fence();

    let k_after = shared.read_batch(out, max);
    if k_after > 0 {
      shared.unregister(Role::Recv);
      *is_registered = false;
      return Poll::Ready(Ok(k_after));
    }
    if !shared.senders_alive() {
      shared.unregister(Role::Recv);
      *is_registered = false;
      let k_end = shared.read_batch(out, max);
      if k_end > 0 {
        return Poll::Ready(Ok(k_end));
      }
      return Poll::Ready(Err(RecvError::Disconnected));
    }
    return Poll::Pending;
  }
}

impl<T: Send> Stream for BoundedAsyncReceiver<T> {
  type Item = T;

  fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
    if self.closed.load(Ordering::Relaxed) {
      return Poll::Ready(None);
    }
    let receiver = self.get_mut();
    match receiver.shared.poll_recv_internal(cx, &mut receiver.is_registered) {
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

  #[tokio::test]
  async fn async_send_recv_single_item() {
    let (mut p, mut c) = bounded_async(1);
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

  #[tokio::test]
  async fn async_try_send_full_try_recv_empty() {
    let (mut p, mut c) = bounded_async::<i32>(1);
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

  #[tokio::test]
  async fn async_send_blocks_then_completes() {
    let (mut p, mut c) = bounded_async::<i32>(1);

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

  #[tokio::test]
  async fn async_recv_blocks_then_completes() {
    let (mut p, mut c) = bounded_async::<i32>(1);
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

  #[tokio::test]
  async fn async_producer_drop_signals_consumer() {
    let (mut p, mut c) = bounded_async::<i32>(1);
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

  #[tokio::test]
  async fn async_producer_drop_empty_signals_consumer() {
    let (p, mut c) = bounded_async::<i32>(1);
    drop(p);
    assert_eq!(c.len(), 0);
    match c.recv().await {
      Err(RecvError::Disconnected) => {}
      Ok(v) => panic!("[DROP_P_EMPTY] Expected Disconnected, got Ok({:?})", v),
    }
  }

  #[tokio::test]
  async fn async_consumer_drop_signals_producer() {
    let (mut p, c) = bounded_async::<i32>(1);
    drop(c);
    match p.send(1).await {
      Err(SendError::Closed) => {}
      Ok(()) => panic!("[DROP_C] Expected Closed error"),
      Err(SendError::Sent) => panic!("[DROP_C] SPSC send should not error with Sent"),
    }
  }

  #[tokio::test]
  async fn async_select_recv_preference() {
    let (mut p1, mut c1) = bounded_async::<i32>(1);
    let (_p2, mut c2) = bounded_async::<i32>(1); // This consumer will never receive

    p1.send(10).await.unwrap();
    assert_eq!(c1.len(), 1);
    assert!(c2.is_empty());

    tokio::select! {
        biased;
        Ok(val) = c1.recv() => {
            assert_eq!(val, 10);
        }
        Ok(_val) = c2.recv() => {
            panic!("[SELECT_RECV] Should not have received from empty c2");
        }
        else => {}
    }
    assert!(c1.is_empty());
  }

  #[tokio::test]
  async fn async_select_send_blocks_other_completes() {
    let (mut p_full, _c_full) = bounded_async::<i32>(1);
    let (mut p_can_send, mut c_can_send) = bounded_async::<i32>(1);

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
        }
    }
    assert!(p_can_send.is_full());
    recv_task.await.unwrap();
  }

  // --- Mixed Sync/Async Tests ---
  #[tokio::test]
  async fn sync_producer_async_consumer() {
    const CAPACITY: usize = 2;
    let core_shared = create_test_shared_core::<String>(CAPACITY);

    let sync_p = BoundedSyncSender::from_shared(core_shared.clone());
    let mut async_c = BoundedAsyncReceiver::from_shared(core_shared);

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

    let mut async_p = BoundedAsyncSender::from_shared(core_shared.clone());
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

  #[tokio::test]
  async fn async_try_recv_disconnected() {
    let (mut p, mut c) = bounded_async::<i32>(1);
    p.try_send(1).unwrap();
    assert_eq!(c.try_recv().unwrap(), 1);
    assert!(c.is_empty());

    drop(p);

    assert_eq!(c.try_recv(), Err(TryRecvError::Disconnected));
  }

  #[tokio::test]
  async fn async_recv_future_disconnected_after_item() {
    let (mut p, mut c) = bounded_async::<i32>(1);
    p.send(1).await.unwrap();
    assert_eq!(c.recv().await.unwrap(), 1);
    assert!(c.is_empty());

    drop(p);

    assert_eq!(c.recv().await, Err(RecvError::Disconnected));
  }

  #[tokio::test]
  async fn new_spsc_apis_close_is_closed() {
    let (mut p, mut c) = bounded_async::<i32>(5);
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

    let (mut p, mut c) = bounded_async::<i32>(5);
    c.close().unwrap();
    assert!(c.is_closed());
    assert_eq!(c.close(), Err(CloseError));
    assert_eq!(c.recv().await, Err(RecvError::Disconnected));

    assert!(p.is_closed());
    assert_eq!(p.send(1).await, Err(SendError::Closed));
  }

  #[tokio::test]
  async fn async_sender_unblocks_on_consumer_drop() {
    let (mut p, c) = bounded_async(1);
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
