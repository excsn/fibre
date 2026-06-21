//! Single-producer, single-consumer **rendezvous** channel.
//!
//! A rendezvous channel has zero capacity: a `send` completes only once the
//! receiver takes the value as part of the same direct handoff. It is a
//! distinct channel family from the buffered SPSC ring buffer — there is no
//! item buffer and no early deposit, so cancelling a pending send can never
//! ghost-deliver its payload.
//!
//! There is exactly one sender and one receiver; neither is `Clone`. Both
//! blocking and async ends are provided and interoperate.
//!
//! # Implementation note
//!
//! This SPSC rendezvous is currently backed by the shared, mutex-based
//! [`crate::internal::rendezvous`] core (instantiated with a single-slot
//! receiver store). That core is fully cancel-safe and allocation-free on the
//! steady-state parked path. `plan.md` §6 envisions a *lock-free* SPSC phase
//! machine; that is a tracked follow-up optimization. Correctness and API are
//! identical — only the uncontended fast-path cost differs (an uncontended
//! `parking_lot` lock, a couple of atomics).

use crate::error::{CloseError, RecvError, RecvErrorTimeout, SendError, TryRecvError, TrySendError};
use crate::internal::rendezvous::{MpscRvShared as RvShared, WAITING};

use std::future::Future;
use std::marker::PhantomPinned;
use std::mem;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

// --- Constructors ---------------------------------------------------------

/// Creates a synchronous SPSC rendezvous channel.
pub fn rendezvous<T: Send>() -> (Sender<T>, Receiver<T>) {
  let shared = Arc::new(RvShared::new());
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

/// Creates an asynchronous SPSC rendezvous channel.
pub fn rendezvous_async<T: Send>() -> (AsyncSender<T>, AsyncReceiver<T>) {
  let shared = Arc::new(RvShared::new());
  (
    AsyncSender {
      shared: Arc::clone(&shared),
      closed: AtomicBool::new(false),
    },
    AsyncReceiver {
      shared,
      closed: AtomicBool::new(false),
    },
  )
}

// --- Handles --------------------------------------------------------------

/// The synchronous sending half of an SPSC rendezvous channel. Not `Clone`.
#[derive(Debug)]
pub struct Sender<T: Send> {
  shared: Arc<RvShared<T>>,
  closed: AtomicBool,
}

/// The synchronous receiving half of an SPSC rendezvous channel. Not `Clone`.
#[derive(Debug)]
pub struct Receiver<T: Send> {
  shared: Arc<RvShared<T>>,
  closed: AtomicBool,
}

/// The asynchronous sending half of an SPSC rendezvous channel. Not `Clone`.
#[derive(Debug)]
pub struct AsyncSender<T: Send> {
  shared: Arc<RvShared<T>>,
  closed: AtomicBool,
}

/// The asynchronous receiving half of an SPSC rendezvous channel. Not `Clone`.
#[derive(Debug)]
pub struct AsyncReceiver<T: Send> {
  shared: Arc<RvShared<T>>,
  closed: AtomicBool,
}

// --- Sender (sync) --------------------------------------------------------

impl<T: Send> Sender<T> {
  /// Sends a value, blocking until the receiver takes it or the channel closes.
  pub fn send(&self, item: T) -> Result<(), SendError> {
    if self.closed.load(Ordering::Relaxed) {
      return Err(SendError::Closed);
    }
    self.shared.send_blocking(item)
  }

  /// Attempts to hand off to an already-waiting receiver without blocking.
  pub fn try_send(&self, item: T) -> Result<(), TrySendError<T>> {
    if self.closed.load(Ordering::Relaxed) {
      return Err(TrySendError::Closed(item));
    }
    self.shared.try_send(item)
  }

  /// Closes this handle; a parked receiver observes disconnection.
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

  /// Returns `true` if the receiver has been dropped.
  pub fn is_closed(&self) -> bool {
    self.shared.receiver_count() == 0
  }

  /// Always `0`.
  #[inline]
  pub fn len(&self) -> usize {
    0
  }
  /// Always `true`.
  #[inline]
  pub fn is_empty(&self) -> bool {
    true
  }
  /// Always `true`.
  #[inline]
  pub fn is_full(&self) -> bool {
    true
  }
  /// Always `Some(0)`.
  #[inline]
  pub fn capacity(&self) -> Option<usize> {
    Some(0)
  }

  /// Converts this handle into an asynchronous [`AsyncSender`]. Zero-cost.
  pub fn to_async(self) -> AsyncSender<T> {
    let shared = unsafe { std::ptr::read(&self.shared) };
    mem::forget(self);
    AsyncSender {
      shared,
      closed: AtomicBool::new(false),
    }
  }
}

impl<T: Send> Drop for Sender<T> {
  fn drop(&mut self) {
    let _ = self.close();
  }
}

// --- Receiver (sync) ------------------------------------------------------

impl<T: Send> Receiver<T> {
  /// Receives a value, blocking until the sender hands one off or the channel
  /// disconnects.
  pub fn recv(&self) -> Result<T, RecvError> {
    if self.closed.load(Ordering::Relaxed) {
      return Err(RecvError::Disconnected);
    }
    self.shared.recv_blocking()
  }

  /// Attempts to take from an already-waiting sender without blocking.
  pub fn try_recv(&self) -> Result<T, TryRecvError> {
    if self.closed.load(Ordering::Relaxed) {
      return Err(TryRecvError::Disconnected);
    }
    self.shared.try_recv()
  }

  /// Receives a value, blocking for at most `timeout`.
  pub fn recv_timeout(&self, timeout: Duration) -> Result<T, RecvErrorTimeout> {
    if self.closed.load(Ordering::Relaxed) {
      return Err(RecvErrorTimeout::Disconnected);
    }
    self.shared.recv_timeout(timeout)
  }

  /// Closes this handle; a parked sender observes closure.
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

  /// Returns `true` if the sender has been dropped.
  pub fn is_closed(&self) -> bool {
    self.shared.sender_count() == 0
  }

  /// Always `0`.
  #[inline]
  pub fn len(&self) -> usize {
    0
  }
  /// Always `true`.
  #[inline]
  pub fn is_empty(&self) -> bool {
    true
  }
  /// Always `true`.
  #[inline]
  pub fn is_full(&self) -> bool {
    true
  }
  /// Always `Some(0)`.
  #[inline]
  pub fn capacity(&self) -> Option<usize> {
    Some(0)
  }

  /// Converts this handle into an asynchronous [`AsyncReceiver`]. Zero-cost.
  pub fn to_async(self) -> AsyncReceiver<T> {
    let shared = unsafe { std::ptr::read(&self.shared) };
    mem::forget(self);
    AsyncReceiver {
      shared,
      closed: AtomicBool::new(false),
    }
  }
}

impl<T: Send> Drop for Receiver<T> {
  fn drop(&mut self) {
    let _ = self.close();
  }
}

// --- AsyncSender ----------------------------------------------------------

impl<T: Send> AsyncSender<T> {
  /// Sends a value, resolving once the receiver takes it or the channel closes.
  pub fn send(&self, item: T) -> SendFuture<'_, T> {
    SendFuture::new(&self.shared, item)
  }

  /// Attempts to hand off to an already-waiting receiver without awaiting.
  pub fn try_send(&self, item: T) -> Result<(), TrySendError<T>> {
    if self.closed.load(Ordering::Relaxed) {
      return Err(TrySendError::Closed(item));
    }
    self.shared.try_send(item)
  }

  /// See [`Sender::close`].
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

  /// Returns `true` if the receiver has been dropped.
  pub fn is_closed(&self) -> bool {
    self.shared.receiver_count() == 0
  }

  /// Always `0`.
  #[inline]
  pub fn len(&self) -> usize {
    0
  }
  /// Always `true`.
  #[inline]
  pub fn is_empty(&self) -> bool {
    true
  }
  /// Always `true`.
  #[inline]
  pub fn is_full(&self) -> bool {
    true
  }
  /// Always `Some(0)`.
  #[inline]
  pub fn capacity(&self) -> Option<usize> {
    Some(0)
  }

  /// Converts this handle into a synchronous [`Sender`]. Zero-cost.
  pub fn to_sync(self) -> Sender<T> {
    let shared = unsafe { std::ptr::read(&self.shared) };
    mem::forget(self);
    Sender {
      shared,
      closed: AtomicBool::new(false),
    }
  }
}

impl<T: Send> Drop for AsyncSender<T> {
  fn drop(&mut self) {
    let _ = self.close();
  }
}

// --- AsyncReceiver --------------------------------------------------------

impl<T: Send> AsyncReceiver<T> {
  /// Receives a value, resolving once the sender hands one off or the channel
  /// disconnects.
  pub fn recv(&self) -> RecvFuture<'_, T> {
    RecvFuture::new(&self.shared)
  }

  /// Attempts to take from an already-waiting sender without awaiting.
  pub fn try_recv(&self) -> Result<T, TryRecvError> {
    if self.closed.load(Ordering::Relaxed) {
      return Err(TryRecvError::Disconnected);
    }
    self.shared.try_recv()
  }

  /// See [`Receiver::close`].
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

  /// Returns `true` if the sender has been dropped.
  pub fn is_closed(&self) -> bool {
    self.shared.sender_count() == 0
  }

  /// Always `0`.
  #[inline]
  pub fn len(&self) -> usize {
    0
  }
  /// Always `true`.
  #[inline]
  pub fn is_empty(&self) -> bool {
    true
  }
  /// Always `true`.
  #[inline]
  pub fn is_full(&self) -> bool {
    true
  }
  /// Always `Some(0)`.
  #[inline]
  pub fn capacity(&self) -> Option<usize> {
    Some(0)
  }

  /// Converts this handle into a synchronous [`Receiver`]. Zero-cost.
  pub fn to_sync(self) -> Receiver<T> {
    let shared = unsafe { std::ptr::read(&self.shared) };
    mem::forget(self);
    Receiver {
      shared,
      closed: AtomicBool::new(false),
    }
  }
}

impl<T: Send> Drop for AsyncReceiver<T> {
  fn drop(&mut self) {
    let _ = self.close();
  }
}

// --- Futures --------------------------------------------------------------

/// Future returned by [`AsyncSender::send`].
#[must_use = "futures do nothing unless you .await or poll them"]
pub struct SendFuture<'a, T: Send> {
  shared: &'a Arc<RvShared<T>>,
  slot: Option<T>,
  state: AtomicU8,
  registered: bool,
  _pin: PhantomPinned,
}

impl<'a, T: Send> SendFuture<'a, T> {
  fn new(shared: &'a Arc<RvShared<T>>, item: T) -> Self {
    Self {
      shared,
      slot: Some(item),
      state: AtomicU8::new(WAITING),
      registered: false,
      _pin: PhantomPinned,
    }
  }
}

impl<'a, T: Send> Future for SendFuture<'a, T> {
  type Output = Result<(), SendError>;

  fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    let this = unsafe { self.get_unchecked_mut() };
    if this.slot.is_none() && !this.registered {
      return Poll::Ready(Ok(()));
    }
    this
      .shared
      .poll_send(cx, &this.state, &mut this.slot, &mut this.registered)
  }
}

impl<'a, T: Send> Drop for SendFuture<'a, T> {
  fn drop(&mut self) {
    if self.registered {
      self
        .shared
        .cancel_sender(&self.state as *const AtomicU8, &self.state);
    }
  }
}

/// Future returned by [`AsyncReceiver::recv`].
#[must_use = "futures do nothing unless you .await or poll them"]
pub struct RecvFuture<'a, T: Send> {
  shared: &'a Arc<RvShared<T>>,
  dest: Option<T>,
  state: AtomicU8,
  registered: bool,
  _pin: PhantomPinned,
}

impl<'a, T: Send> RecvFuture<'a, T> {
  fn new(shared: &'a Arc<RvShared<T>>) -> Self {
    Self {
      shared,
      dest: None,
      state: AtomicU8::new(WAITING),
      registered: false,
      _pin: PhantomPinned,
    }
  }
}

impl<'a, T: Send> Future for RecvFuture<'a, T> {
  type Output = Result<T, RecvError>;

  fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    let this = unsafe { self.get_unchecked_mut() };
    this
      .shared
      .poll_recv(cx, &this.state, &mut this.dest, &mut this.registered)
  }
}

impl<'a, T: Send> Drop for RecvFuture<'a, T> {
  fn drop(&mut self) {
    if self.registered {
      self
        .shared
        .cancel_receiver(&self.state as *const AtomicU8, &self.state);
    }
  }
}
