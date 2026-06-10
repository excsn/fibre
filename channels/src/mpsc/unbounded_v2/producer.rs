use super::shared::MpscShared;
use crate::error::{
  BatchSendErrorReason, CloseError, SendBatchError, SendError, TrySendBatchError, TrySendError,
};
use crate::mpsc::block_queue::Block;

use core::marker::PhantomPinned;
use parking_lot::Mutex;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};

// Common logic for both producer types.
// We take an explicit cache argument.
pub(crate) fn send_internal<T: Send>(
  shared: &Arc<MpscShared<T>>,
  value: T,
  cache: &mut Option<Arc<Block<T>>>,
) -> Result<(), T> {
  if shared.receiver_dropped.load(Ordering::Acquire) {
    return Err(value);
  }

  shared.queue.push(value, cache);
  shared.current_len.fetch_add(1, Ordering::Relaxed);
  shared.wake_consumer();
  Ok(())
}

// Batch counterpart of `send_internal`. The caller is responsible for the
// closed/receiver_dropped checks *before* consuming any items, so that no
// item is lost when the channel is closed. The queue length is incremented
// once, after the entire batch is written, and the consumer is woken once.
pub(crate) fn send_batch_internal<T: Send>(
  shared: &Arc<MpscShared<T>>,
  iter: &mut impl Iterator<Item = T>,
  count: usize,
  cache: &mut Option<Arc<Block<T>>>,
) {
  shared.queue.push_batch(iter, count, cache);
  shared.current_len.fetch_add(count, Ordering::Relaxed);
  shared.wake_consumer();
}

#[derive(Debug)]
pub struct Sender<T: Send> {
  pub(crate) shared: Arc<MpscShared<T>>,
  pub(crate) closed: AtomicBool,
  // Thread-local-ish cache for the block queue.
  // Protected by a Mutex to allow Sync, but intended to be uncontended (owned by this Sender).
  pub(crate) cache: Mutex<Option<Arc<Block<T>>>>,
}

#[derive(Debug)]
pub struct AsyncSender<T: Send> {
  pub(crate) shared: Arc<MpscShared<T>>,
  pub(crate) closed: AtomicBool,
  pub(crate) cache: Mutex<Option<Arc<Block<T>>>>,
}

impl<T: Send> Sender<T> {
  pub fn send(&self, value: T) -> Result<(), SendError> {
    if self.closed.load(Ordering::Relaxed) {
      return Err(SendError::Closed);
    }
    // Lock the local cache. This is fast if the Sender is not shared across threads.
    let mut cache_guard = self.cache.lock();
    send_internal(&self.shared, value, &mut *cache_guard).map_err(|_| SendError::Closed)
  }

  pub fn try_send(&self, value: T) -> Result<(), TrySendError<T>> {
    if self.closed.load(Ordering::Relaxed) {
      return Err(TrySendError::Closed(value));
    }
    let mut cache_guard = self.cache.lock();
    send_internal(&self.shared, value, &mut *cache_guard).map_err(TrySendError::Closed)
  }

  /// Sends a batch of items, taking ownership of the vector. The channel is
  /// unbounded, so this never blocks: `Ok(n)` means every item was sent.
  /// The only failure is a closed channel, reported before any item is sent
  /// (`sent: 0`, all items returned in `unsent`).
  ///
  /// The whole batch is written with one queue-length update and one
  /// consumer wake.
  pub fn send_batch(&self, items: Vec<T>) -> Result<usize, SendBatchError<T>> {
    self.try_send_batch(items).map_err(|e| SendBatchError {
      sent: e.sent,
      unsent: e.unsent,
    })
  }

  /// Non-blocking batch send. Identical to [`send_batch`](Self::send_batch)
  /// for this unbounded channel, but reports closure as [`TrySendBatchError`].
  pub fn try_send_batch(&self, items: Vec<T>) -> Result<usize, TrySendBatchError<T>> {
    let total = items.len();
    if total == 0 {
      return Ok(0);
    }
    if self.closed.load(Ordering::Relaxed) || self.shared.receiver_dropped.load(Ordering::Acquire)
    {
      return Err(TrySendBatchError {
        sent: 0,
        unsent: items,
        reason: BatchSendErrorReason::Closed,
      });
    }
    let mut iter = items.into_iter();
    let mut cache_guard = self.cache.lock();
    send_batch_internal(&self.shared, &mut iter, total, &mut *cache_guard);
    Ok(total)
  }

  /// Sends a batch in place, draining all items from `items`. Never blocks
  /// (unbounded). On `Err(SendError::Closed)`, all items remain in `items`.
  pub fn send_batch_mut(&self, items: &mut Vec<T>) -> Result<usize, SendError> {
    if items.is_empty() {
      return Ok(0);
    }
    if self.closed.load(Ordering::Relaxed) || self.shared.receiver_dropped.load(Ordering::Acquire)
    {
      return Err(SendError::Closed);
    }
    let total = items.len();
    let mut cache_guard = self.cache.lock();
    let mut drain = items.drain(..);
    send_batch_internal(&self.shared, &mut drain, total, &mut *cache_guard);
    drop(drain);
    Ok(total)
  }

  /// Non-blocking in-place batch send. Identical to
  /// [`send_batch_mut`](Self::send_batch_mut) for this unbounded channel.
  pub fn try_send_batch_mut(&self, items: &mut Vec<T>) -> Result<usize, SendError> {
    self.send_batch_mut(items)
  }

  pub fn is_closed(&self) -> bool {
    self.shared.receiver_dropped.load(Ordering::Acquire)
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
    if self.shared.sender_count.fetch_sub(1, Ordering::AcqRel) == 1 {
      self.shared.wake_consumer();
    }
  }

  pub fn sender_count(&self) -> usize {
    self.shared.sender_count.load(Ordering::Relaxed)
  }

  pub fn len(&self) -> usize {
    self.shared.current_len.load(Ordering::Relaxed)
  }

  pub fn is_empty(&self) -> bool {
    self.shared.queue.is_empty()
  }

  pub fn to_async(self) -> AsyncSender<T> {
    // 1. Extract the shared state pointer.
    let shared = unsafe { std::ptr::read(&self.shared) };

    // 2. Safely drop the cache content.
    // We lock the cache and take the Option out.
    // When `_cached_block` goes out of scope at the end of this block, 
    // the Arc (if any) is dropped, decrementing the ref count.
    {
       let mut guard = self.cache.lock();
       let _cached_block = guard.take(); 
    }

    // 3. Forget self to prevent double-drop of `shared` (which we read above).
    // Note: `self.cache` itself (the Mutex) is also forgotten/leaked, 
    // but Mutexes generally don't have critical Drop logic other than 
    // OS resource cleanup (which is usually fine to skip for std/parking_lot mutexes 
    // that are just memory state). If using a Mutex that requires Drop, this leaks.
    // parking_lot::Mutex is just a byte in memory, so forgetting it is safe.
    std::mem::forget(self);

    AsyncSender {
      shared,
      closed: AtomicBool::new(false),
      // Start with a fresh, empty cache for the async sender
      cache: Mutex::new(None),
    }
  }
}

impl<T: Send> AsyncSender<T> {
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
    let mut cache_guard = self.cache.lock();
    send_internal(&self.shared, value, &mut *cache_guard).map_err(TrySendError::Closed)
  }

  /// Sends a batch of items asynchronously, taking ownership of the vector.
  /// The channel is unbounded, so the returned future completes on its first
  /// poll (it exists for API symmetry with the bounded channels).
  pub fn send_batch(&self, items: Vec<T>) -> SendBatchFuture<'_, T> {
    SendBatchFuture {
      producer: self,
      items: Some(items),
      _phantom: PhantomPinned,
    }
  }

  /// Sends a batch asynchronously in place, draining all items from `items`.
  /// Completes on first poll (unbounded). Cancel-safe: if never polled, all
  /// items remain in `items`.
  pub fn send_batch_mut<'a>(&'a self, items: &'a mut Vec<T>) -> SendBatchMutFuture<'a, T> {
    SendBatchMutFuture {
      producer: self,
      items,
      _phantom: PhantomPinned,
    }
  }

  /// Non-blocking batch send. `Ok(n)` means every item was sent; the only
  /// failure is a closed channel (`sent: 0`, all items returned in `unsent`).
  pub fn try_send_batch(&self, items: Vec<T>) -> Result<usize, TrySendBatchError<T>> {
    let total = items.len();
    if total == 0 {
      return Ok(0);
    }
    if self.closed.load(Ordering::Relaxed) || self.shared.receiver_dropped.load(Ordering::Acquire)
    {
      return Err(TrySendBatchError {
        sent: 0,
        unsent: items,
        reason: BatchSendErrorReason::Closed,
      });
    }
    let mut iter = items.into_iter();
    let mut cache_guard = self.cache.lock();
    send_batch_internal(&self.shared, &mut iter, total, &mut *cache_guard);
    Ok(total)
  }

  /// Non-blocking in-place batch send, draining all items from `items`.
  /// On `Err(SendError::Closed)`, all items remain in `items`.
  pub fn try_send_batch_mut(&self, items: &mut Vec<T>) -> Result<usize, SendError> {
    if items.is_empty() {
      return Ok(0);
    }
    if self.closed.load(Ordering::Relaxed) || self.shared.receiver_dropped.load(Ordering::Acquire)
    {
      return Err(SendError::Closed);
    }
    let total = items.len();
    let mut cache_guard = self.cache.lock();
    let mut drain = items.drain(..);
    send_batch_internal(&self.shared, &mut drain, total, &mut *cache_guard);
    drop(drain);
    Ok(total)
  }

  pub fn is_closed(&self) -> bool {
    self.shared.receiver_dropped.load(Ordering::Acquire)
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
    if self.shared.sender_count.fetch_sub(1, Ordering::AcqRel) == 1 {
      self.shared.wake_consumer();
    }
  }

  pub fn sender_count(&self) -> usize {
    self.shared.sender_count.load(Ordering::Relaxed)
  }

  pub fn len(&self) -> usize {
    self.shared.current_len.load(Ordering::Relaxed)
  }

  pub fn is_empty(&self) -> bool {
    self.shared.queue.is_empty()
  }

  pub fn to_sync(self) -> Sender<T> {
    let shared = unsafe { std::ptr::read(&self.shared) };
    {
      let mut c = self.cache.lock();
      *c = None;
    }
    std::mem::forget(self);
    Sender {
      shared,
      closed: AtomicBool::new(false),
      cache: Mutex::new(None),
    }
  }
}

// --- Future ---
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

    let mut cache_guard = this.producer.cache.lock();
    Poll::Ready(
      send_internal(&this.producer.shared, value, &mut *cache_guard).map_err(|_| SendError::Closed),
    )
  }
}

/// Future returned by [`AsyncSender::send_batch`]. Completes on first poll
/// (the channel is unbounded). If dropped before being polled, the items are
/// dropped.
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
    let total = items.len();
    if total == 0 {
      return Poll::Ready(Ok(0));
    }
    if this.producer.closed.load(Ordering::Relaxed)
      || this.producer.shared.receiver_dropped.load(Ordering::Acquire)
    {
      return Poll::Ready(Err(SendBatchError {
        sent: 0,
        unsent: items,
      }));
    }
    let mut iter = items.into_iter();
    let mut cache_guard = this.producer.cache.lock();
    send_batch_internal(&this.producer.shared, &mut iter, total, &mut *cache_guard);
    Poll::Ready(Ok(total))
  }
}

/// Future returned by [`AsyncSender::send_batch_mut`]. Completes on first
/// poll (the channel is unbounded). Cancel-safe: unsent items remain in the
/// caller's vector.
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
    if this.items.is_empty() {
      return Poll::Ready(Ok(0));
    }
    if this.producer.closed.load(Ordering::Relaxed)
      || this.producer.shared.receiver_dropped.load(Ordering::Acquire)
    {
      return Poll::Ready(Err(SendError::Closed));
    }
    let total = this.items.len();
    let mut cache_guard = this.producer.cache.lock();
    let mut drain = this.items.drain(..);
    send_batch_internal(&this.producer.shared, &mut drain, total, &mut *cache_guard);
    drop(drain);
    Poll::Ready(Ok(total))
  }
}

// --- Cloning and Dropping ---

impl<T: Send> Clone for Sender<T> {
  fn clone(&self) -> Self {
    self.shared.sender_count.fetch_add(1, Ordering::Relaxed);
    // New sender starts with empty cache to avoid contention with original
    Sender {
      shared: Arc::clone(&self.shared),
      closed: AtomicBool::new(false),
      cache: Mutex::new(None),
    }
  }
}
impl<T: Send> Drop for Sender<T> {
  fn drop(&mut self) {
    if !self.closed.swap(true, Ordering::AcqRel) {
      self.close_internal();
    }
    // cache dropped implicitly
  }
}
impl<T: Send> Clone for AsyncSender<T> {
  fn clone(&self) -> Self {
    self.shared.sender_count.fetch_add(1, Ordering::Relaxed);
    AsyncSender {
      shared: Arc::clone(&self.shared),
      closed: AtomicBool::new(false),
      cache: Mutex::new(None),
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
