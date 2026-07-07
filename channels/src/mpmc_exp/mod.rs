//! A lock-free bounded MPMC channel.
//!
//! The data path (`send`/`recv` fast path) is a lock-free Vyukov ring: FIFO,
//! no per-call scan, no lock. A lock is taken only to park/unpark when the ring
//! is transiently full (senders) or empty (receivers). Sync and async handles
//! interoperate over the same channel.
//!
//! Capacity is rounded up to the next power of two; [`capacity`](Sender::capacity)
//! reports the rounded value.
//!
//! Cancellation is trivial: a send/recv future suspends only while parked,
//! holding no slot, so dropping it merely unlinks the parked waiter - there is no
//! in-buffer tombstone to resolve and no risk of "ghost delivery".

use std::mem;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::error::{
  BatchSendErrorReason, CloseError, RecvError, RecvErrorTimeout, SendBatchError, SendError,
  TryRecvError, TrySendBatchError, TrySendError,
};

use self::imp::Shared;

pub use self::imp::{
  RecvBatchFuture, RecvBatchMutFuture, RecvFuture, SendBatchFuture, SendBatchMutFuture, SendFuture,
};

// Rendezvous (capacity-0) is a separate channel family and is reused verbatim
// from `mpmc_v2`; the unbounded constructors likewise delegate to `mpmc_v2`
// (a fixed lock-free ring cannot be unbounded). Both are orthogonal to the
// bounded lock-free ring this module implements.
pub use crate::mpmc_v2::rendezvous;
pub use crate::mpmc_v2::{unbounded, unbounded_async};


/// Facade exposing the Vyukov backend under the common surface the public
/// handles use. A second backend (`tombstone_handoff`) will expose the same
/// names so the two cores can be swapped for benching/testing.
mod vyukov;

/// THE BACKEND SWITCH - flip this alias to bench/test the other MPMC core.
/// Both backends expose an identical surface (`Shared`, the sync ops, the
/// futures, `poll_stream_next`).
use self::vyukov as imp;

// --- Public handles -------------------------------------------------------

/// A synchronous sending handle. Cloneable for multiple producers.
#[derive(Debug)]
pub struct Sender<T: Send> {
  shared: Arc<Shared<T>>,
  closed: AtomicBool,
}

/// A synchronous receiving handle. Cloneable for multiple consumers.
#[derive(Debug)]
pub struct Receiver<T: Send> {
  shared: Arc<Shared<T>>,
  closed: AtomicBool,
}

/// An asynchronous sending handle. Cloneable for multiple producers.
#[derive(Debug)]
pub struct AsyncSender<T: Send> {
  pub(crate) shared: Arc<Shared<T>>,
  closed: AtomicBool,
}

/// An asynchronous receiving handle. Cloneable for multiple consumers.
#[derive(Debug)]
pub struct AsyncReceiver<T: Send> {
  pub(crate) shared: Arc<Shared<T>>,
  closed: AtomicBool,
  /// Waiter registration id for the `Stream` impl (independent of `recv()`'s own
  /// per-future registration). `None` until parked via `poll_next`.
  stream_id: Option<u64>,
}

// --- Constructors ---------------------------------------------------------

/// Creates a synchronous bounded MPMC channel. `capacity` is rounded up to the
/// next power of two. Panics if `capacity == 0`.
pub fn bounded<T: Send>(capacity: usize) -> (Sender<T>, Receiver<T>) {
  assert!(capacity != 0, "mpmc_v3::bounded(0) is not supported");
  let shared = Arc::new(Shared::new(capacity));
  (
    Sender {
      shared: Arc::clone(&shared),
      closed: AtomicBool::new(false),
    },
    Receiver {
      shared,
      closed: AtomicBool::new(false),
    },
  )
}

/// Creates an asynchronous bounded MPMC channel. `capacity` is rounded up to the
/// next power of two. Panics if `capacity == 0`.
pub fn bounded_async<T: Send>(capacity: usize) -> (AsyncSender<T>, AsyncReceiver<T>) {
  assert!(capacity != 0, "mpmc_v3::bounded_async(0) is not supported");
  let shared = Arc::new(Shared::new(capacity));
  (
    AsyncSender {
      shared: Arc::clone(&shared),
      closed: AtomicBool::new(false),
    },
    AsyncReceiver {
      shared,
      closed: AtomicBool::new(false),
      stream_id: None,
    },
  )
}

// --- Sync Sender ----------------------------------------------------------

impl<T: Send> Sender<T> {
  /// Sends a value, blocking until there is room or all receivers drop.
  pub fn send(&self, item: T) -> Result<(), SendError> {
    if self.closed.load(Ordering::Relaxed) {
      return Err(SendError::Closed);
    }
    imp::send_sync(&self.shared, item)
  }

  /// Attempts to send without blocking.
  pub fn try_send(&self, item: T) -> Result<(), TrySendError<T>> {
    if self.closed.load(Ordering::Relaxed) {
      return Err(TrySendError::Closed(item));
    }
    match imp::try_send(&self.shared, item) {
      Ok(()) => Ok(()),
      Err((item, true)) => Err(TrySendError::Closed(item)),
      Err((item, false)) => Err(TrySendError::Full(item)),
    }
  }

  /// Attempts to send a batch without blocking. `Ok(n)` means every item was
  /// sent; otherwise [`TrySendBatchError`] carries the count sent and the
  /// in-order remainder.
  pub fn try_send_batch(&self, items: Vec<T>) -> Result<usize, TrySendBatchError<T>> {
    if items.is_empty() {
      return Ok(0);
    }
    if self.closed.load(Ordering::Relaxed) {
      return Err(TrySendBatchError {
        sent: 0,
        unsent: items,
        reason: BatchSendErrorReason::Closed,
      });
    }
    let (sent, unsent, reason) = imp::try_send_batch_core(&self.shared, items);
    match reason {
      None => Ok(sent),
      Some(reason) => Err(TrySendBatchError { sent, unsent, reason }),
    }
  }

  /// Attempts to send a batch in place without blocking, draining sent items
  /// from the front of `items`.
  pub fn try_send_batch_mut(&self, items: &mut Vec<T>) -> Result<usize, SendError> {
    if items.is_empty() {
      return Ok(0);
    }
    if self.closed.load(Ordering::Relaxed) {
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

  /// Sends a batch, blocking whenever the ring is full, until every item is
  /// sent or all receivers drop.
  pub fn send_batch(&self, items: Vec<T>) -> Result<usize, SendBatchError<T>> {
    if self.closed.load(Ordering::Relaxed) {
      return Err(SendBatchError { sent: 0, unsent: items });
    }
    imp::send_batch_sync(&self.shared, items)
  }

  /// Sends a batch in place, blocking whenever the ring is full. On closure the
  /// unsent items remain in `items`.
  pub fn send_batch_mut(&self, items: &mut Vec<T>) -> Result<usize, SendError> {
    if items.is_empty() {
      return Ok(0);
    }
    if self.closed.load(Ordering::Relaxed) {
      return Err(SendError::Closed);
    }
    let batch = std::mem::take(items);
    match imp::send_batch_sync(&self.shared, batch) {
      Ok(n) => Ok(n),
      Err(e) => {
        *items = e.unsent;
        Err(SendError::Closed)
      }
    }
  }

  /// Explicitly closes this handle. See [`Receiver::close`].
  pub fn close(&self) -> Result<(), CloseError> {
    if self
      .closed
      .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
      .is_ok()
    {
      self.shared.drop_sender();
      Ok(())
    } else {
      Err(CloseError)
    }
  }

  /// Returns `true` if all receivers have been dropped.
  pub fn is_closed(&self) -> bool {
    !self.shared.receivers_alive()
  }

  /// Returns the channel capacity (rounded up to a power of two).
  pub fn capacity(&self) -> Option<usize> {
    Some(self.shared.capacity())
  }

  /// Number of buffered items (best-effort snapshot).
  pub fn len(&self) -> usize {
    self.shared.len()
  }
  pub fn is_empty(&self) -> bool {
    self.shared.is_empty()
  }
  pub fn is_full(&self) -> bool {
    self.shared.is_full()
  }

  /// Zero-cost conversion into an [`AsyncSender`].
  pub fn to_async(self) -> AsyncSender<T> {
    let shared = unsafe { std::ptr::read(&self.shared) };
    let closed = self.closed.load(Ordering::Relaxed);
    mem::forget(self);
    AsyncSender {
      shared,
      closed: AtomicBool::new(closed),
    }
  }
}

impl<T: Send> Clone for Sender<T> {
  fn clone(&self) -> Self {
    self.shared.add_sender();
    Sender {
      shared: Arc::clone(&self.shared),
      closed: AtomicBool::new(false),
    }
  }
}

impl<T: Send> Drop for Sender<T> {
  fn drop(&mut self) {
    let _ = self.close();
  }
}

// --- Sync Receiver --------------------------------------------------------

impl<T: Send> Receiver<T> {
  /// Diagnostic snapshot: (buffered len, parked send waiters, parked recv
  /// waiters). For debugging deadlocks/lost wakeups.
  pub fn debug_state(&self) -> (usize, usize, usize) {
    let (sw, rw) = self.shared.debug_waiters();
    (self.shared.len(), sw, rw)
  }

  /// Receives a value, blocking until one is available or all senders drop.
  pub fn recv(&self) -> Result<T, RecvError> {
    if self.closed.load(Ordering::Relaxed) {
      return Err(RecvError::Disconnected);
    }
    imp::recv_sync(&self.shared)
  }

  /// Attempts to receive without blocking.
  pub fn try_recv(&self) -> Result<T, TryRecvError> {
    if self.closed.load(Ordering::Relaxed) {
      return Err(TryRecvError::Disconnected);
    }
    imp::try_recv(&self.shared)
  }

  /// Receives a value, blocking at most `timeout`.
  pub fn recv_timeout(&self, timeout: Duration) -> Result<T, RecvErrorTimeout> {
    if self.closed.load(Ordering::Relaxed) {
      return Err(RecvErrorTimeout::Disconnected);
    }
    imp::recv_timeout_sync(&self.shared, timeout)
  }

  /// Attempts to receive up to `max` items without blocking, in FIFO order.
  pub fn try_recv_batch(&self, max: usize) -> Result<Vec<T>, TryRecvError> {
    let mut out = Vec::new();
    self.try_recv_batch_mut(&mut out, max)?;
    Ok(out)
  }

  /// Attempts to receive up to `max` items without blocking, appending to `out`.
  pub fn try_recv_batch_mut(&self, out: &mut Vec<T>, max: usize) -> Result<usize, TryRecvError> {
    if max == 0 {
      return Ok(0);
    }
    if self.closed.load(Ordering::Relaxed) {
      return Err(TryRecvError::Disconnected);
    }
    imp::try_recv_batch_core(&self.shared, out, max)
  }

  /// Receives up to `max` items, blocking until at least one is available, then
  /// draining up to `max` without further waiting.
  pub fn recv_batch(&self, max: usize) -> Result<Vec<T>, RecvError> {
    let mut out = Vec::new();
    self.recv_batch_mut(&mut out, max)?;
    Ok(out)
  }

  /// Receives up to `max` items, blocking until at least one is available,
  /// appending to `out`.
  pub fn recv_batch_mut(&self, out: &mut Vec<T>, max: usize) -> Result<usize, RecvError> {
    if self.closed.load(Ordering::Relaxed) {
      return Err(RecvError::Disconnected);
    }
    imp::recv_batch_sync(&self.shared, out, max)
  }

  /// Explicitly closes this handle. If it is the last receiver, parked senders
  /// are woken to observe closure.
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

  /// Returns `true` if all senders have been dropped and the buffer is empty.
  pub fn is_closed(&self) -> bool {
    !self.shared.senders_alive() && self.shared.is_empty()
  }

  pub fn capacity(&self) -> Option<usize> {
    Some(self.shared.capacity())
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

  /// Zero-cost conversion into an [`AsyncReceiver`].
  pub fn to_async(self) -> AsyncReceiver<T> {
    let shared = unsafe { std::ptr::read(&self.shared) };
    let closed = self.closed.load(Ordering::Relaxed);
    mem::forget(self);
    AsyncReceiver {
      shared,
      closed: AtomicBool::new(closed),
      stream_id: None,
    }
  }
}

impl<T: Send> Clone for Receiver<T> {
  fn clone(&self) -> Self {
    self.shared.add_receiver();
    Receiver {
      shared: Arc::clone(&self.shared),
      closed: AtomicBool::new(false),
    }
  }
}

impl<T: Send> Drop for Receiver<T> {
  fn drop(&mut self) {
    let _ = self.close();
  }
}

// --- Async Sender ---------------------------------------------------------

impl<T: Send> AsyncSender<T> {
  /// Sends a value asynchronously. The returned future resolves once the value
  /// is in the buffer, or with `Err` if the channel closes. Cancel-safe.
  pub fn send(&self, item: T) -> SendFuture<'_, T> {
    imp::SendFuture::new(&self.shared, item)
  }

  pub fn try_send(&self, item: T) -> Result<(), TrySendError<T>> {
    if self.closed.load(Ordering::Relaxed) {
      return Err(TrySendError::Closed(item));
    }
    match imp::try_send(&self.shared, item) {
      Ok(()) => Ok(()),
      Err((item, true)) => Err(TrySendError::Closed(item)),
      Err((item, false)) => Err(TrySendError::Full(item)),
    }
  }

  /// Attempts to send a batch without blocking. Same semantics as
  /// [`Sender::try_send_batch`].
  pub fn try_send_batch(&self, items: Vec<T>) -> Result<usize, TrySendBatchError<T>> {
    if items.is_empty() {
      return Ok(0);
    }
    if self.closed.load(Ordering::Relaxed) {
      return Err(TrySendBatchError {
        sent: 0,
        unsent: items,
        reason: BatchSendErrorReason::Closed,
      });
    }
    let (sent, unsent, reason) = imp::try_send_batch_core(&self.shared, items);
    match reason {
      None => Ok(sent),
      Some(reason) => Err(TrySendBatchError { sent, unsent, reason }),
    }
  }

  /// Attempts to send a batch in place without blocking.
  pub fn try_send_batch_mut(&self, items: &mut Vec<T>) -> Result<usize, SendError> {
    if items.is_empty() {
      return Ok(0);
    }
    if self.closed.load(Ordering::Relaxed) {
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

  /// Sends a batch asynchronously. Resolves with `Ok(n)` once every item is
  /// sent, or [`SendBatchError`] if the channel closes mid-batch.
  pub fn send_batch(&self, items: Vec<T>) -> SendBatchFuture<'_, T> {
    imp::SendBatchFuture::new(&self.shared, items)
  }

  /// Sends a batch asynchronously in place. Cancel-safe: on drop or closure the
  /// unsent remainder stays in `items`.
  pub fn send_batch_mut<'a>(&'a self, items: &'a mut Vec<T>) -> SendBatchMutFuture<'a, T> {
    imp::SendBatchMutFuture::new(&self.shared, items)
  }

  pub fn close(&self) -> Result<(), CloseError> {
    if self
      .closed
      .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
      .is_ok()
    {
      self.shared.drop_sender();
      Ok(())
    } else {
      Err(CloseError)
    }
  }

  pub fn is_closed(&self) -> bool {
    !self.shared.receivers_alive()
  }
  pub fn capacity(&self) -> Option<usize> {
    Some(self.shared.capacity())
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

  /// Zero-cost conversion into a sync [`Sender`].
  pub fn to_sync(self) -> Sender<T> {
    let shared = unsafe { std::ptr::read(&self.shared) };
    let closed = self.closed.load(Ordering::Relaxed);
    mem::forget(self);
    Sender {
      shared,
      closed: AtomicBool::new(closed),
    }
  }
}

impl<T: Send> Clone for AsyncSender<T> {
  fn clone(&self) -> Self {
    self.shared.add_sender();
    AsyncSender {
      shared: Arc::clone(&self.shared),
      closed: AtomicBool::new(false),
    }
  }
}

impl<T: Send> Drop for AsyncSender<T> {
  fn drop(&mut self) {
    let _ = self.close();
  }
}

// --- Async Receiver -------------------------------------------------------

impl<T: Send> AsyncReceiver<T> {
  /// Receives a value asynchronously. Cancel-safe: dropping the future before it
  /// resolves only unlinks the parked waiter.
  pub fn recv(&self) -> RecvFuture<'_, T> {
    imp::RecvFuture::new(&self.shared)
  }

  pub fn try_recv(&self) -> Result<T, TryRecvError> {
    if self.closed.load(Ordering::Relaxed) {
      return Err(TryRecvError::Disconnected);
    }
    imp::try_recv(&self.shared)
  }

  /// Attempts to receive up to `max` items without blocking, in FIFO order.
  pub fn try_recv_batch(&self, max: usize) -> Result<Vec<T>, TryRecvError> {
    let mut out = Vec::new();
    self.try_recv_batch_mut(&mut out, max)?;
    Ok(out)
  }

  /// Attempts to receive up to `max` items without blocking, appending to `out`.
  pub fn try_recv_batch_mut(&self, out: &mut Vec<T>, max: usize) -> Result<usize, TryRecvError> {
    if max == 0 {
      return Ok(0);
    }
    if self.closed.load(Ordering::Relaxed) {
      return Err(TryRecvError::Disconnected);
    }
    imp::try_recv_batch_core(&self.shared, out, max)
  }

  /// Receives up to `max` items asynchronously (FIFO) once anything is
  /// available.
  pub fn recv_batch(&self, max: usize) -> RecvBatchFuture<'_, T> {
    imp::RecvBatchFuture::new(&self.shared, max)
  }

  /// Receives up to `max` items asynchronously, appending to `out`. Cancel-safe.
  pub fn recv_batch_mut<'a>(&'a self, out: &'a mut Vec<T>, max: usize) -> RecvBatchMutFuture<'a, T> {
    imp::RecvBatchMutFuture::new(&self.shared, out, max)
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
    !self.shared.senders_alive() && self.shared.is_empty()
  }
  pub fn capacity(&self) -> Option<usize> {
    Some(self.shared.capacity())
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

  /// Zero-cost conversion into a sync [`Receiver`].
  pub fn to_sync(self) -> Receiver<T> {
    let shared = unsafe { std::ptr::read(&self.shared) };
    let closed = self.closed.load(Ordering::Relaxed);
    mem::forget(self);
    Receiver {
      shared,
      closed: AtomicBool::new(closed),
    }
  }
}

impl<T: Send> Clone for AsyncReceiver<T> {
  fn clone(&self) -> Self {
    self.shared.add_receiver();
    AsyncReceiver {
      shared: Arc::clone(&self.shared),
      closed: AtomicBool::new(false),
      stream_id: None,
    }
  }
}

impl<T: Send> Drop for AsyncReceiver<T> {
  fn drop(&mut self) {
    if let Some(id) = self.stream_id.take() {
      self.shared.unregister_recv(id);
    }
    let _ = self.close();
  }
}

// --- Stream impl (parity with mpmc_v2) ------------------------------------

impl<T: Send> futures_core::Stream for AsyncReceiver<T> {
  type Item = T;

  fn poll_next(
    self: std::pin::Pin<&mut Self>,
    cx: &mut std::task::Context<'_>,
  ) -> std::task::Poll<Option<T>> {
    // `AsyncReceiver` is `Unpin`, so this is sound.
    let this = self.get_mut();

    if this.closed.load(Ordering::Relaxed) {
      return std::task::Poll::Ready(None);
    }

    // The register / recheck / park dance lives in the backend.
    imp::poll_stream_next(&this.shared, &mut this.stream_id, cx)
  }
}
