//! Consumer handles for `bounded_v3`: `Receiver` (sync) and `AsyncReceiver`,
//! plus their recv futures and the `Stream` impl. The single-consumer drain core
//! is `Shared::deq_once` + `flush_progress`; the async side uses a
//! proper `RecvFuture`/`Stream` (register → fence → recheck, Drop-unregister)
//! instead of `poll_fn` so cancellation is clean.

use std::cell::Cell;
use std::fmt;
use std::marker::PhantomData;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use futures_core::Stream;

use crate::internal::sync::{fence, thread, Arc, AtomicBool, Ordering};

use crate::error::{CloseError, RecvError, RecvErrorTimeout, TryRecvError};
use crate::sync_util;

use super::shared::{spin_before_park_cap1, Deq, Shared, SYNC_SPIN_LIMIT};

pub struct Receiver<T: Send> {
  pub(crate) shared: Arc<Shared<T>>,
  pub(crate) closed: AtomicBool,
  // Keep the single-consumer contract: `Receiver` is `Send` but not `Sync`
  // (mirrors `bounded_queue`'s `RefCell`-enforced restriction).
  pub(crate) _not_sync: PhantomData<Cell<()>>,
}

pub struct AsyncReceiver<T: Send> {
  pub(crate) shared: Arc<Shared<T>>,
  pub(crate) closed: AtomicBool,
  pub(crate) is_registered: bool,
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
  /// Blocking recv: drain, flush-before-
  /// wait, an unconditional pre-register spin, then register-fence-recheck-park.
  pub fn recv(&self) -> Result<T, RecvError> {
    if self.closed.load(Ordering::Relaxed) {
      return Err(RecvError::Disconnected);
    }

    let mut spins = 0usize;
    let mut is_registered = false;
    let notified = AtomicBool::new(false);
    let notified_ptr = &notified as *const AtomicBool;

    loop {
      match self.shared.deq_once() {
        Deq::Got(v) => {
          if is_registered {
            self.shared.finish_sync_recv(&notified);
          }
          return Ok(v);
        }
        Deq::Empty if !self.shared.senders_alive() => {
          if let Some(v) = self.shared.drain_straggler() {
            if is_registered {
              self.shared.finish_sync_recv(&notified);
            }
            return Ok(v);
          }
          self.shared.flush_progress();
          if is_registered {
            self.shared.finish_sync_recv(&notified);
          }
          return Err(RecvError::Disconnected);
        }
        _ => {
          self.shared.flush_progress();
          if is_registered {
            spin_before_park_cap1(self.shared.cap(), &notified);
            if !notified.load(Ordering::Relaxed) {
              sync_util::park_thread();
            }
            // Barrier this registration before looping: `finish_sync_recv`
            // guarantees the notifier is done storing through `notified`'s raw
            // pointer (it stores under the recv-waiter lock, which finish
            // synchronizes with), so `notified` is never reused on re-register
            // or dropped on return while a notifier can still touch it. Reset
            // the flag for the next register.
            self.shared.finish_sync_recv(&notified);
            is_registered = false;
            notified.store(false, Ordering::Relaxed);
            continue;
          }
          if spins < SYNC_SPIN_LIMIT {
            spins += 1;
            if self.shared.cap() <= 4 {
              std::hint::spin_loop();
            } else {
              thread::yield_now();
            }
            continue;
          }
          self.shared.register_sync_recv(thread::current(), notified_ptr);
          is_registered = true;
          fence(Ordering::SeqCst);
        }
      }
    }
  }

  pub fn recv_timeout(&self, timeout: Duration) -> Result<T, RecvErrorTimeout> {
    let deadline = Instant::now().checked_add(timeout);
    let mut is_registered = false;
    let notified = AtomicBool::new(false);
    let notified_ptr = &notified as *const AtomicBool;

    loop {
      match self.shared.deq_once() {
        Deq::Got(v) => {
          if is_registered {
            self.shared.finish_sync_recv(&notified);
          }
          return Ok(v);
        }
        Deq::Empty if !self.shared.senders_alive() => {
          if let Some(v) = self.shared.drain_straggler() {
            if is_registered {
              self.shared.finish_sync_recv(&notified);
            }
            return Ok(v);
          }
          self.shared.flush_progress();
          if is_registered {
            self.shared.finish_sync_recv(&notified);
          }
          return Err(RecvErrorTimeout::Disconnected);
        }
        _ => {
          self.shared.flush_progress();
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
            spin_before_park_cap1(self.shared.cap(), &notified);
            if !notified.load(Ordering::Relaxed) {
              sync_util::park_thread_timeout(remaining);
            }
            if notified.swap(false, Ordering::Acquire) {
              is_registered = false;
            }
            continue;
          }
          self.shared.register_sync_recv(thread::current(), notified_ptr);
          is_registered = true;
          fence(Ordering::SeqCst);
        }
      }
    }
  }

  pub fn try_recv(&self) -> Result<T, TryRecvError> {
    if self.closed.load(Ordering::Relaxed) {
      return Err(TryRecvError::Disconnected);
    }
    match self.shared.deq_once() {
      Deq::Got(v) => Ok(v),
      Deq::Empty if !self.shared.senders_alive() => {
        if let Some(v) = self.shared.drain_straggler() {
          return Ok(v);
        }
        self.shared.flush_progress();
        Err(TryRecvError::Disconnected)
      }
      _ => {
        self.shared.flush_progress();
        Err(TryRecvError::Empty)
      }
    }
  }

  // --- batch recv: single-lock run drains ---

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

    let mut spins = 0usize;
    let mut is_registered = false;
    let notified = AtomicBool::new(false);
    let notified_ptr = &notified as *const AtomicBool;

    loop {
      let k = self.shared.deq_run(out, max);
      if k > 0 {
        if is_registered {
          self.shared.finish_sync_recv(&notified);
        }
        self.shared.flush_progress();
        return Ok(k);
      }

      self.shared.flush_progress();

      if !self.shared.senders_alive() {
        // Straggler re-drain (see drain_straggler): the last producer's run may
        // be ordered-after the sender_count==0 we just read.
        let k2 = self.shared.deq_run(out, max);
        if is_registered {
          self.shared.finish_sync_recv(&notified);
        }
        if k2 > 0 {
          self.shared.flush_progress();
          return Ok(k2);
        }
        return Err(RecvError::Disconnected);
      }

      if is_registered {
        spin_before_park_cap1(self.shared.cap(), &notified);
        if !notified.load(Ordering::Relaxed) {
          sync_util::park_thread();
        }
        if notified.swap(false, Ordering::Acquire) {
          is_registered = false;
        }
        continue;
      }
      if spins < SYNC_SPIN_LIMIT {
        spins += 1;
        if self.shared.cap() <= 4 {
          std::hint::spin_loop();
        } else {
          thread::yield_now();
        }
        continue;
      }
      self.shared.register_sync_recv(thread::current(), notified_ptr);
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
    try_recv_run(&self.shared, out, max)
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

  pub fn to_async(self) -> AsyncReceiver<T> {
    // Async receiver flush cadence = full cap (hoard-then-dump, arms the drip).
    self.shared.set_publish_chunk(self.shared.cap());
    let shared = unsafe { std::ptr::read(&self.shared) };
    let closed = self.closed.load(Ordering::Relaxed);
    std::mem::forget(self);
    AsyncReceiver {
      shared,
      closed: AtomicBool::new(closed),
      is_registered: false,
    }
  }
}

impl<T: Send> Drop for Receiver<T> {
  fn drop(&mut self) {
    let _ = self.close();
  }
}

impl<T: Send> AsyncReceiver<T> {
  pub fn recv(&self) -> RecvFuture<'_, T> {
    RecvFuture {
      receiver: self,
      is_registered: false,
    }
  }

  pub fn try_recv(&self) -> Result<T, TryRecvError> {
    if self.closed.load(Ordering::Relaxed) {
      return Err(TryRecvError::Disconnected);
    }
    match self.shared.deq_once() {
      Deq::Got(v) => Ok(v),
      Deq::Empty if !self.shared.senders_alive() => {
        if let Some(v) = self.shared.drain_straggler() {
          return Ok(v);
        }
        self.shared.flush_progress();
        Err(TryRecvError::Disconnected)
      }
      _ => {
        self.shared.flush_progress();
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

  pub fn to_sync(self) -> Receiver<T> {
    if self.is_registered {
      self.shared.unregister_async_recv();
    }
    // Sync receiver flush cadence = min(cap, 64).
    self
      .shared
      .set_publish_chunk(self.shared.cap().min(super::shared::CACHE_FLUSH_CHUNK));
    let shared = unsafe { std::ptr::read(&self.shared) };
    let closed = self.closed.load(Ordering::Relaxed);
    std::mem::forget(self);
    Receiver {
      shared,
      closed: AtomicBool::new(closed),
      _not_sync: PhantomData,
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
    try_recv_run(&self.shared, out, max)
  }

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

impl<T: Send> Drop for AsyncReceiver<T> {
  fn drop(&mut self) {
    if self.is_registered {
      self.shared.unregister_async_recv();
    }
    let _ = self.close();
  }
}

/// Shared async drain core for `RecvFuture` and `Stream`:
/// drain → flush → register → fence → recheck.
fn poll_recv<T: Send>(
  shared: &Shared<T>,
  cx: &mut Context<'_>,
  is_registered: &mut bool,
) -> Poll<Result<T, RecvError>> {
  match shared.deq_once() {
    Deq::Got(v) => {
      if *is_registered {
        shared.unregister_async_recv();
        *is_registered = false;
      }
      return Poll::Ready(Ok(v));
    }
    Deq::Empty if !shared.senders_alive() => {
      if let Some(v) = shared.drain_straggler() {
        if *is_registered {
          shared.unregister_async_recv();
          *is_registered = false;
        }
        return Poll::Ready(Ok(v));
      }
      shared.flush_progress();
      if *is_registered {
        shared.unregister_async_recv();
        *is_registered = false;
      }
      return Poll::Ready(Err(RecvError::Disconnected));
    }
    _ => {}
  }

  shared.flush_progress();
  shared.register_async_recv(cx.waker().clone());
  *is_registered = true;
  fence(Ordering::SeqCst);

  match shared.deq_once() {
    Deq::Got(v) => {
      shared.unregister_async_recv();
      *is_registered = false;
      Poll::Ready(Ok(v))
    }
    Deq::Empty if !shared.senders_alive() => {
      if let Some(v) = shared.drain_straggler() {
        shared.unregister_async_recv();
        *is_registered = false;
        return Poll::Ready(Ok(v));
      }
      shared.unregister_async_recv();
      *is_registered = false;
      Poll::Ready(Err(RecvError::Disconnected))
    }
    _ => Poll::Pending,
  }
}

#[must_use = "futures do nothing unless you .await or poll them"]
pub struct RecvFuture<'a, T: Send> {
  receiver: &'a AsyncReceiver<T>,
  is_registered: bool,
}

impl<'a, T: Send> Future for RecvFuture<'a, T> {
  type Output = Result<T, RecvError>;

  fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    let this = unsafe { self.get_unchecked_mut() };
    if this.receiver.closed.load(Ordering::Relaxed) {
      return Poll::Ready(Err(RecvError::Disconnected));
    }
    poll_recv(&this.receiver.shared, cx, &mut this.is_registered)
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
    if this.closed.load(Ordering::Relaxed) {
      return Poll::Ready(None);
    }
    match poll_recv(&this.shared, cx, &mut this.is_registered) {
      Poll::Ready(Ok(v)) => Poll::Ready(Some(v)),
      Poll::Ready(Err(_)) => Poll::Ready(None),
      Poll::Pending => Poll::Pending,
    }
  }
}

/// Non-blocking batch drain (shared by sync + async `try_recv_batch_mut`).
/// Callers have handled the up-front closed check.
fn try_recv_run<T: Send>(
  shared: &Shared<T>,
  out: &mut Vec<T>,
  max: usize,
) -> Result<usize, TryRecvError> {
  let k = shared.deq_run(out, max);
  if k > 0 {
    shared.flush_progress();
    return Ok(k);
  }
  shared.flush_progress();
  if !shared.senders_alive() {
    // Straggler re-drain after observing the disconnect (see drain_straggler).
    let k2 = shared.deq_run(out, max);
    if k2 > 0 {
      shared.flush_progress();
      return Ok(k2);
    }
    return Err(TryRecvError::Disconnected);
  }
  Err(TryRecvError::Empty)
}

/// Shared async batch drain core: one `deq_run`, else register + Pending.
fn poll_recv_batch<T: Send>(
  shared: &Shared<T>,
  cx: &mut Context<'_>,
  out: &mut Vec<T>,
  max: usize,
  is_registered: &mut bool,
) -> Poll<Result<usize, RecvError>> {
  let k = shared.deq_run(out, max);
  if k > 0 {
    if *is_registered {
      shared.unregister_async_recv();
      *is_registered = false;
    }
    shared.flush_progress();
    return Poll::Ready(Ok(k));
  }

  shared.flush_progress();
  if !shared.senders_alive() {
    // Straggler re-drain after observing the disconnect (see drain_straggler).
    let k2 = shared.deq_run(out, max);
    if *is_registered {
      shared.unregister_async_recv();
      *is_registered = false;
    }
    if k2 > 0 {
      shared.flush_progress();
      return Poll::Ready(Ok(k2));
    }
    return Poll::Ready(Err(RecvError::Disconnected));
  }

  shared.register_async_recv(cx.waker().clone());
  *is_registered = true;
  fence(Ordering::SeqCst);

  let k3 = shared.deq_run(out, max);
  if k3 > 0 {
    shared.unregister_async_recv();
    *is_registered = false;
    shared.flush_progress();
    return Poll::Ready(Ok(k3));
  }
  if !shared.senders_alive() {
    let k4 = shared.deq_run(out, max);
    shared.unregister_async_recv();
    *is_registered = false;
    if k4 > 0 {
      shared.flush_progress();
      return Poll::Ready(Ok(k4));
    }
    return Poll::Ready(Err(RecvError::Disconnected));
  }
  Poll::Pending
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
    if this.receiver.closed.load(Ordering::Relaxed) {
      return Poll::Ready(Err(RecvError::Disconnected));
    }
    let mut out = Vec::new();
    match poll_recv_batch(
      &this.receiver.shared,
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
    if this.receiver.closed.load(Ordering::Relaxed) {
      return Poll::Ready(Err(RecvError::Disconnected));
    }
    let max = this.max;
    poll_recv_batch(
      &this.receiver.shared,
      cx,
      this.out,
      max,
      &mut this.is_registered,
    )
  }
}

impl<'a, T: Send> Drop for BoundedRecvBatchMutFuture<'a, T> {
  fn drop(&mut self) {
    if self.is_registered {
      self.receiver.shared.unregister_async_recv();
    }
  }
}
