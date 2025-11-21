use super::shared::MpscShared;
use crate::error::{CloseError, SendError, TrySendError};
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
