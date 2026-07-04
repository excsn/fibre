//! Consumer handles for the bounded MPSC: `Receiver` (sync) and
//! `AsyncReceiver`, plus their recv futures and the `Stream` impl. The receiver
//! recycles consumed nodes through its single-threaded `LocalCache` and flushes
//! them back to `super::shared`'s chunk stack.

use std::cell::RefCell;
use std::fmt;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use futures_core::Stream;

use crate::internal::sync::{fence, thread, Arc, AtomicBool, Mutex, Ordering};

use crate::error::{CloseError, RecvError, RecvErrorTimeout, TryRecvError};
use crate::sync_util;

use super::shared::*;

// Deliberately `RefCell`, not `Mutex`: Receiver is intentionally restricted to
// single-thread access (see the `Send`-only impl below, no `Sync`). Sharing a
// `Receiver` across threads is a compile error now (RefCell is never Sync),
// rather than a silent footgun protected only by a lock nothing contends.
pub struct Receiver<T: Send> {
  pub(crate) shared: Arc<BoundedQueue<T>>,
  pub(crate) closed: AtomicBool,
  pub(crate) cache: RefCell<LocalCache<T>>,
}

pub struct AsyncReceiver<T: Send> {
  pub(crate) shared: Arc<BoundedQueue<T>>,
  pub(crate) closed: AtomicBool,
  pub(crate) is_registered: bool,
  pub(crate) cache: Mutex<LocalCache<T>>,
}

impl<T: Send> fmt::Debug for Receiver<T> {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("Receiver")
      .field("capacity", &self.capacity())
      .field("len", &self.len())
      .field("closed", &self.closed.load(Ordering::Relaxed))
      .finish()
  }
}

impl<T: Send> fmt::Debug for AsyncReceiver<T> {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("AsyncReceiver")
      .field("capacity", &self.capacity())
      .field("len", &self.len())
      .field("closed", &self.closed.load(Ordering::Relaxed))
      .finish()
  }
}

unsafe impl<T: Send> Send for Receiver<T> {}
unsafe impl<T: Send> Send for AsyncReceiver<T> {}
unsafe impl<T: Send> Sync for AsyncReceiver<T> {}

impl<T: Send> Receiver<T> {
  #[inline]
  fn recycle_node(&self, node_ptr: *mut Node<T>, pool: &mut LocalCache<T>) {
    pool.push_node(&self.shared, node_ptr);
    if pool.count >= self.shared.capacity().min(CACHE_FLUSH_CHUNK) {
      pool.flush(&self.shared);
    }
  }

  pub fn recv(&self) -> Result<T, RecvError> {
    let mut is_registered = false;
    let notified = AtomicBool::new(false);
    let notified_ptr = &notified as *const AtomicBool;

    if self.closed.load(Ordering::Relaxed) {
      return Err(RecvError::Disconnected);
    }

    let mut pool = self.cache.borrow_mut();

    loop {
      let tail = unsafe { *self.shared.tail.get() };

      if let Some(next) = unsafe { self.shared.follow_link(tail) } {
        if is_registered {
          self.shared.finish_sync_recv(&notified);
        }
        // see poll_pop: must leave the slot None, or the queue's drop glue
        // double-drops a stale value left in a never-resent free-list node
        let value = unsafe { (&mut *(*next).val.get()).take().unwrap() };
        unsafe {
          *self.shared.tail.get() = next;
        }
        self.shared.current_len.fetch_sub(1, Ordering::Relaxed);
        self.recycle_node(tail, &mut pool);
        return Ok(value);
      }

      pool.flush(&self.shared);

      if !self.shared.senders_alive() {
        if unsafe { self.shared.link_is_fresh(tail) } {
          continue;
        }
        if is_registered {
          self.shared.finish_sync_recv(&notified);
        }
        return Err(RecvError::Disconnected);
      }

      if is_registered {
        spin_before_park_cap1(self.shared.cap, &notified);
        if !notified.load(Ordering::Relaxed) {
          sync_util::park_thread();
        }
        if notified.swap(false, Ordering::Acquire) {
          is_registered = false;
        }
        continue;
      }

      // Probe the link BEFORE registering, at every capacity — the
      // pre-f35e867 consumer spin was unconditional. Primitive choice is
      // producer-count-dispatched; see `probe_for_publish`.
      if unsafe { self.shared.probe_for_publish(tail) } {
        continue;
      }

      self
        .shared
        .register_sync_recv(thread::current(), notified_ptr);
      is_registered = true;
      fence(Ordering::SeqCst);
    }
  }

  pub fn recv_timeout(&self, timeout: Duration) -> Result<T, RecvErrorTimeout> {
    let deadline = Instant::now().checked_add(timeout);
    let mut is_registered = false;
    let notified = AtomicBool::new(false);
    let notified_ptr = &notified as *const AtomicBool;

    let mut pool = self.cache.borrow_mut();

    loop {
      let tail = unsafe { *self.shared.tail.get() };

      if let Some(next) = unsafe { self.shared.follow_link(tail) } {
        if is_registered {
          self.shared.finish_sync_recv(&notified);
        }
        // see poll_pop: must leave the slot None, or the queue's drop glue
        // double-drops a stale value left in a never-resent free-list node
        let value = unsafe { (&mut *(*next).val.get()).take().unwrap() };
        unsafe {
          *self.shared.tail.get() = next;
        }
        self.shared.current_len.fetch_sub(1, Ordering::Relaxed);
        self.recycle_node(tail, &mut pool);
        return Ok(value);
      }

      pool.flush(&self.shared);

      if !self.shared.senders_alive() {
        if unsafe { self.shared.link_is_fresh(tail) } {
          continue;
        }
        if is_registered {
          self.shared.finish_sync_recv(&notified);
        }
        return Err(RecvErrorTimeout::Disconnected);
      }

      let now = Instant::now();
      let remaining = match deadline {
        Some(d) if d > now => d - now,
        _ => {
          if is_registered {
            self.shared.finish_sync_recv(&notified);
          }
          return Err(RecvErrorTimeout::Timeout);
        }
      };

      if is_registered {
        spin_before_park_cap1(self.shared.cap, &notified);
        if !notified.load(Ordering::Relaxed) {
          sync_util::park_thread_timeout(remaining);
        }
        if notified.swap(false, Ordering::Acquire) {
          is_registered = false;
        }
        continue;
      }

      self
        .shared
        .register_sync_recv(thread::current(), notified_ptr);
      is_registered = true;
      fence(Ordering::SeqCst);
    }
  }

  pub fn try_recv(&self) -> Result<T, TryRecvError> {
    if self.closed.load(Ordering::Relaxed) {
      return Err(TryRecvError::Disconnected);
    }

    let mut pool = self.cache.borrow_mut();
    let tail = unsafe { *self.shared.tail.get() };

    if let Some(next) = unsafe { self.shared.follow_link(tail) } {
      // see poll_pop: must leave the slot None, or the queue's drop glue
      // double-drops a stale value left in a never-resent free-list node
      let value = unsafe { (&mut *(*next).val.get()).take().unwrap() };
      unsafe {
        *self.shared.tail.get() = next;
      }
      self.shared.current_len.fetch_sub(1, Ordering::Relaxed);
      self.recycle_node(tail, &mut *pool);
      Ok(value)
    } else {
      pool.flush(&self.shared);
      if !self.shared.senders_alive() {
        Err(TryRecvError::Disconnected)
      } else {
        Err(TryRecvError::Empty)
      }
    }
  }

  fn pop_batch_lock_free(&self, out: &mut Vec<T>, max: usize, pool: &mut LocalCache<T>) -> usize {
    let mut popped = 0;
    unsafe {
      let mut tail = *self.shared.tail.get();
      while popped < max {
        let Some(next) = self.shared.follow_link(tail) else {
          break;
        };
        // see poll_pop: must leave the slot None, or the queue's drop glue
        // double-drops a stale value left in a never-resent free-list node
        let value = (&mut *(*next).val.get()).take().unwrap();
        *self.shared.tail.get() = next;
        self.recycle_node(tail, pool);
        tail = next;
        out.push(value);
        popped += 1;
      }
    }
    if popped > 0 {
      self.shared.current_len.fetch_sub(popped, Ordering::Relaxed);
    }
    popped
  }

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

    let mut is_registered = false;
    let notified = AtomicBool::new(false);
    let notified_ptr = &notified as *const AtomicBool;
    let mut pool = self.cache.borrow_mut();

    loop {
      let k = self.pop_batch_lock_free(out, max, &mut *pool);
      if k > 0 {
        if is_registered {
          self.shared.finish_sync_recv(&notified);
        }
        pool.flush(&self.shared);
        return Ok(k);
      }

      pool.flush(&self.shared);

      if !self.shared.senders_alive() {
        let k_end = self.pop_batch_lock_free(out, max, &mut pool);
        if k_end > 0 {
          if is_registered {
            self.shared.finish_sync_recv(&notified);
          }
          pool.flush(&self.shared);
          return Ok(k_end);
        }
        if is_registered {
          self.shared.finish_sync_recv(&notified);
        }
        return Err(RecvError::Disconnected);
      }

      if is_registered {
        spin_before_park_cap1(self.shared.cap, &notified);
        if !notified.load(Ordering::Relaxed) {
          sync_util::park_thread();
        }
        if notified.swap(false, Ordering::Acquire) {
          is_registered = false;
        }
        continue;
      }

      // Probe the link BEFORE registering, at every capacity — the
      // pre-f35e867 consumer spin was unconditional. Primitive choice is
      // producer-count-dispatched; see `probe_for_publish`.
      {
        let tail = unsafe { *self.shared.tail.get() };
        if unsafe { self.shared.probe_for_publish(tail) } {
          continue;
        }
      }

      self
        .shared
        .register_sync_recv(thread::current(), notified_ptr);
      is_registered = true;
      fence(Ordering::SeqCst);
    }
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

    let mut pool = self.cache.borrow_mut();
    let k = self.pop_batch_lock_free(out, max, &mut *pool);
    if k > 0 {
      pool.flush(&self.shared);
      return Ok(k);
    }
    pool.flush(&self.shared);

    if !self.shared.senders_alive() {
      let k_end = self.pop_batch_lock_free(out, max, &mut *pool);
      if k_end > 0 {
        pool.flush(&self.shared);
        return Ok(k_end);
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
      self.shared.drop_receiver();
      Ok(())
    } else {
      Err(CloseError)
    }
  }

  pub fn is_closed(&self) -> bool {
    self.closed.load(Ordering::Relaxed) || (!self.shared.senders_alive() && self.shared.is_empty())
  }

  pub fn capacity(&self) -> usize {
    self.shared.capacity()
  }

  pub fn len(&self) -> usize {
    self.shared.len()
  }

  pub fn is_empty(&self) -> bool {
    self.shared.is_empty()
  }

  pub fn is_full(&self) -> bool {
    self.shared.is_full()
  }

  pub fn to_async(mut self) -> AsyncReceiver<T> {
    let shared = unsafe { std::ptr::read(&self.shared) };
    let closed = self.closed.load(Ordering::Relaxed);
    let cache_val = std::mem::take(self.cache.get_mut());
    std::mem::forget(self);
    AsyncReceiver {
      shared,
      closed: AtomicBool::new(closed),
      is_registered: false,
      cache: Mutex::new(cache_val),
    }
  }
}

impl<T: Send> Drop for Receiver<T> {
  fn drop(&mut self) {
    self.cache.get_mut().flush(&self.shared);
    let _ = self.close();
  }
}


impl<T: Send> AsyncReceiver<T> {
  #[inline]
  fn recycle_node(&self, node_ptr: *mut Node<T>, pool: &mut LocalCache<T>) {
    pool.push_node(&self.shared, node_ptr);
    if pool.count >= self.shared.capacity() {
      pool.flush(&self.shared);
    }
  }

  pub fn recv(&self) -> RecvFuture<'_, T> {
    RecvFuture::new(self)
  }

  pub fn try_recv(&self) -> Result<T, TryRecvError> {
    if self.closed.load(Ordering::Relaxed) {
      return Err(TryRecvError::Disconnected);
    }

    let mut pool = self.cache.lock();
    let tail = unsafe { *self.shared.tail.get() };

    if let Some(next) = unsafe { self.shared.follow_link(tail) } {
      // see poll_pop: must leave the slot None, or the queue's drop glue
      // double-drops a stale value left in a never-resent free-list node
      let value = unsafe { (&mut *(*next).val.get()).take().unwrap() };
      unsafe {
        *self.shared.tail.get() = next;
      }
      self.shared.current_len.fetch_sub(1, Ordering::Relaxed);
      self.recycle_node(tail, &mut *pool);
      Ok(value)
    } else {
      pool.flush(&self.shared);
      if !self.shared.senders_alive() {
        Err(TryRecvError::Disconnected)
      } else {
        Err(TryRecvError::Empty)
      }
    }
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
    self.closed.load(Ordering::Relaxed) || (!self.shared.senders_alive() && self.shared.is_empty())
  }

  pub fn capacity(&self) -> usize {
    self.shared.capacity()
  }

  pub fn len(&self) -> usize {
    self.shared.len()
  }

  pub fn is_empty(&self) -> bool {
    self.shared.is_empty()
  }

  pub fn is_full(&self) -> bool {
    self.shared.is_full()
  }

  pub fn to_sync(mut self) -> Receiver<T> {
    if self.is_registered {
      self.shared.unregister_async_recv();
    }
    let shared = unsafe { std::ptr::read(&self.shared) };
    let closed = self.closed.load(Ordering::Relaxed);
    let cache_val = std::mem::take(self.cache.get_mut());
    std::mem::forget(self);
    Receiver {
      shared,
      closed: AtomicBool::new(closed),
      cache: RefCell::new(cache_val),
    }
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
    let mut pool = self.cache.lock();
    let k = self.pop_batch_lock_free(out, max, &mut *pool);
    if k > 0 {
      pool.flush(&self.shared);
      return Ok(k);
    }
    pool.flush(&self.shared);
    if !self.shared.senders_alive() {
      let k_end = self.pop_batch_lock_free(out, max, &mut *pool);
      if k_end > 0 {
        pool.flush(&self.shared);
        return Ok(k_end);
      }
      return Err(TryRecvError::Disconnected);
    }
    Err(TryRecvError::Empty)
  }

  fn pop_batch_lock_free(&self, out: &mut Vec<T>, max: usize, pool: &mut LocalCache<T>) -> usize {
    let mut popped = 0;
    unsafe {
      let mut tail = *self.shared.tail.get();
      while popped < max {
        let Some(next) = self.shared.follow_link(tail) else {
          break;
        };
        // see poll_pop: must leave the slot None, or the queue's drop glue
        // double-drops a stale value left in a never-resent free-list node
        let value = (&mut *(*next).val.get()).take().unwrap();
        *self.shared.tail.get() = next;
        self.recycle_node(tail, pool);
        tail = next;
        out.push(value);
        popped += 1;
      }
    }
    if popped > 0 {
      self.shared.current_len.fetch_sub(popped, Ordering::Relaxed);
    }
    popped
  }
}

impl<T: Send> Drop for AsyncReceiver<T> {
  fn drop(&mut self) {
    if self.is_registered {
      self.shared.unregister_async_recv();
    }
    self.cache.get_mut().flush(&self.shared);
    let _ = self.close();
  }
}

#[must_use = "futures do nothing unless you .await or poll them"]
pub struct RecvFuture<'a, T: Send> {
  receiver: &'a AsyncReceiver<T>,
  is_registered: bool,
}

impl<'a, T: Send> RecvFuture<'a, T> {
  fn new(receiver: &'a AsyncReceiver<T>) -> Self {
    RecvFuture {
      receiver,
      is_registered: false,
    }
  }
}

impl<'a, T: Send> Future for RecvFuture<'a, T> {
  type Output = Result<T, RecvError>;

  fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    let this = unsafe { self.get_unchecked_mut() };
    let shared = &this.receiver.shared;

    if this.receiver.closed.load(Ordering::Relaxed) {
      return Poll::Ready(Err(RecvError::Disconnected));
    }

    loop {
      let was_registered = this.is_registered;
      match shared.poll_pop(cx, &mut this.is_registered) {
        Poll::Ready(Ok((tail, value))) => {
          let mut pool = this.receiver.cache.lock();
          this.receiver.recycle_node(tail, &mut *pool);
          return Poll::Ready(Ok(value));
        }
        Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
        Poll::Pending => {
          if !was_registered {
            this.receiver.cache.lock().flush(shared);
          }
          return Poll::Pending;
        }
      }
    }
  }
}

impl<'a, T: Send> Drop for RecvFuture<'a, T> {
  fn drop(&mut self) {
    if self.is_registered {
      self.receiver.shared.unregister_async_recv();
    }
  }
}

impl<T: Send> Stream for AsyncReceiver<T> {
  type Item = T;

  fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
    let this = unsafe { self.as_mut().get_unchecked_mut() };
    let shared = &this.shared;

    if this.closed.load(Ordering::Relaxed) {
      return Poll::Ready(None);
    }

    loop {
      let was_registered = this.is_registered;
      match shared.poll_pop(cx, &mut this.is_registered) {
        Poll::Ready(Ok((tail, value))) => {
          let mut pool = this.cache.lock();
          this.recycle_node(tail, &mut *pool);
          return Poll::Ready(Some(value));
        }
        Poll::Ready(Err(_)) => return Poll::Ready(None),
        Poll::Pending => {
          if !was_registered {
            this.cache.lock().flush(shared);
          }
          return Poll::Pending;
        }
      }
    }
  }
}

#[must_use = "futures do nothing unless you .await or poll them"]
pub struct BoundedRecvBatchFuture<'a, T: Send> {
  receiver: &'a AsyncReceiver<T>,
  max: usize,
  is_registered: bool,
}

impl<'a, T: Send> Future for BoundedRecvBatchFuture<'a, T> {
  type Output = Result<Vec<T>, RecvError>;

  fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    let this = unsafe { self.get_unchecked_mut() };
    if this.max == 0 {
      return Poll::Ready(Ok(Vec::new()));
    }
    let mut out = Vec::new();
    match poll_recv_batch_async_impl(
      this.receiver,
      cx,
      &mut out,
      this.max,
      &mut this.is_registered,
    ) {
      Poll::Ready(Ok(_)) => Poll::Ready(Ok(out)),
      Poll::Ready(Err(e)) => Poll::Ready(Err(e)),
      Poll::Pending => Poll::Pending,
    }
  }
}

impl<'a, T: Send> Drop for BoundedRecvBatchFuture<'a, T> {
  fn drop(&mut self) {
    if self.is_registered {
      self.receiver.shared.unregister_async_recv();
    }
  }
}

#[must_use = "futures do nothing unless you .await or poll them"]
pub struct BoundedRecvBatchMutFuture<'a, T: Send> {
  receiver: &'a AsyncReceiver<T>,
  out: &'a mut Vec<T>,
  max: usize,
  is_registered: bool,
}

impl<'a, T: Send> Future for BoundedRecvBatchMutFuture<'a, T> {
  type Output = Result<usize, RecvError>;

  fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    let this = unsafe { self.as_mut().get_unchecked_mut() };
    if this.max == 0 {
      return Poll::Ready(Ok(0));
    }
    let max = this.max;
    poll_recv_batch_async_impl(this.receiver, cx, this.out, max, &mut this.is_registered)
  }
}

impl<'a, T: Send> Drop for BoundedRecvBatchMutFuture<'a, T> {
  fn drop(&mut self) {
    if self.is_registered {
      self.receiver.shared.unregister_async_recv();
    }
  }
}

fn poll_recv_batch_async_impl<T: Send>(
  receiver: &AsyncReceiver<T>,
  cx: &mut Context<'_>,
  out: &mut Vec<T>,
  max: usize,
  is_registered: &mut bool,
) -> Poll<Result<usize, RecvError>> {
  if receiver.closed.load(Ordering::Relaxed) {
    return Poll::Ready(Err(RecvError::Disconnected));
  }
  let shared = &receiver.shared;

  let mut pool = receiver.cache.lock();

  loop {
    let k = receiver.pop_batch_lock_free(out, max, &mut *pool);
    if k > 0 {
      if *is_registered {
        shared.unregister_async_recv();
        *is_registered = false;
      }
      pool.flush(shared);
      return Poll::Ready(Ok(k));
    }

    pool.flush(shared);

    if !shared.senders_alive() {
      let k_end = receiver.pop_batch_lock_free(out, max, &mut *pool);
      if k_end > 0 {
        if *is_registered {
          shared.unregister_async_recv();
          *is_registered = false;
        }
        pool.flush(shared);
        return Poll::Ready(Ok(k_end));
      }
      if *is_registered {
        shared.unregister_async_recv();
        *is_registered = false;
      }
      return Poll::Ready(Err(RecvError::Disconnected));
    }

    shared.register_async_recv(cx.waker().clone(), std::ptr::null());
    *is_registered = true;
    shared.pre_park_fence();

    let k_after = receiver.pop_batch_lock_free(out, max, &mut pool);
    if k_after > 0 {
      shared.unregister_async_recv();
      *is_registered = false;
      pool.flush(shared);
      return Poll::Ready(Ok(k_after));
    }
    if !shared.senders_alive() {
      continue;
    }

    return Poll::Pending;
  }
}

impl<T: Send> AsyncReceiver<T> {
  pub fn recv_batch(&self, max: usize) -> BoundedRecvBatchFuture<'_, T> {
    BoundedRecvBatchFuture {
      receiver: self,
      max,
      is_registered: false,
    }
  }

  pub fn recv_batch_mut<'a>(
    &'a self,
    out: &'a mut Vec<T>,
    max: usize,
  ) -> BoundedRecvBatchMutFuture<'a, T> {
    BoundedRecvBatchMutFuture {
      receiver: self,
      out,
      max,
      is_registered: false,
    }
  }
}
