//! Single-producer, single-consumer **rendezvous** channel.
//!
//! A rendezvous channel has zero capacity: a `send` completes only once the
//! receiver takes the value as part of the same direct handoff. It is a
//! distinct channel family from the buffered SPSC ring buffer - there is no
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
//! steady-state parked path. A *lock-free* SPSC phase machine is a tracked
//! follow-up optimization. Correctness and API are
//! identical - only the uncontended fast-path cost differs (an uncontended
//! `parking_lot` lock, a couple of atomics).

use crate::error::{CloseError, RecvError, RecvErrorTimeout, SendError, TryRecvError, TrySendError};
use crate::internal::rendezvous::{MpscRvShared as RvShared, WAITING};

use std::future::Future;
use std::marker::PhantomPinned;
use std::mem;
use std::pin::Pin;
use std::task::{Context, Poll};

use crate::internal::sync::{Arc, AtomicBool, AtomicU8, Ordering};
use std::time::Duration;

// --- Constructors ---------------------------------------------------------

/// Creates a synchronous SPSC rendezvous channel.
pub fn rendezvous<T: Send>() -> (RendezvousSyncSender<T>, RendezvousSyncReceiver<T>) {
  let shared = Arc::new(RvShared::new());
  (
    RendezvousSyncSender {
      shared: Arc::clone(&shared),
      closed: AtomicBool::new(false),
    },
    RendezvousSyncReceiver {
      shared,
      closed: AtomicBool::new(false),
    },
  )
}

/// Creates an asynchronous SPSC rendezvous channel.
pub fn rendezvous_async<T: Send>() -> (RendezvousAsyncSender<T>, RendezvousAsyncReceiver<T>) {
  let shared = Arc::new(RvShared::new());
  (
    RendezvousAsyncSender {
      shared: Arc::clone(&shared),
      closed: AtomicBool::new(false),
    },
    RendezvousAsyncReceiver {
      shared,
      closed: AtomicBool::new(false),
    },
  )
}

// --- Handles --------------------------------------------------------------

/// The synchronous sending half of an SPSC rendezvous channel. Not `Clone`.
#[derive(Debug)]
pub struct RendezvousSyncSender<T: Send> {
  shared: Arc<RvShared<T>>,
  closed: AtomicBool,
}

/// The synchronous receiving half of an SPSC rendezvous channel. Not `Clone`.
#[derive(Debug)]
pub struct RendezvousSyncReceiver<T: Send> {
  shared: Arc<RvShared<T>>,
  closed: AtomicBool,
}

/// The asynchronous sending half of an SPSC rendezvous channel. Not `Clone`.
#[derive(Debug)]
pub struct RendezvousAsyncSender<T: Send> {
  shared: Arc<RvShared<T>>,
  closed: AtomicBool,
}

/// The asynchronous receiving half of an SPSC rendezvous channel. Not `Clone`.
#[derive(Debug)]
pub struct RendezvousAsyncReceiver<T: Send> {
  shared: Arc<RvShared<T>>,
  closed: AtomicBool,
}

// --- RendezvousSyncSender (sync) --------------------------------------------------------

impl<T: Send> RendezvousSyncSender<T> {
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

  /// Converts this handle into an asynchronous [`RendezvousAsyncSender`]. Zero-cost.
  pub fn to_async(self) -> RendezvousAsyncSender<T> {
    let shared = unsafe { std::ptr::read(&self.shared) };
    mem::forget(self);
    RendezvousAsyncSender {
      shared,
      closed: AtomicBool::new(false),
    }
  }
}

impl<T: Send> Drop for RendezvousSyncSender<T> {
  fn drop(&mut self) {
    let _ = self.close();
  }
}

// --- RendezvousSyncReceiver (sync) ------------------------------------------------------

impl<T: Send> RendezvousSyncReceiver<T> {
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

  /// Converts this handle into an asynchronous [`RendezvousAsyncReceiver`]. Zero-cost.
  pub fn to_async(self) -> RendezvousAsyncReceiver<T> {
    let shared = unsafe { std::ptr::read(&self.shared) };
    mem::forget(self);
    RendezvousAsyncReceiver {
      shared,
      closed: AtomicBool::new(false),
    }
  }
}

impl<T: Send> Drop for RendezvousSyncReceiver<T> {
  fn drop(&mut self) {
    let _ = self.close();
  }
}

// --- RendezvousAsyncSender ----------------------------------------------------------

impl<T: Send> RendezvousAsyncSender<T> {
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

  /// See [`RendezvousSyncSender::close`].
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

  /// Converts this handle into a synchronous [`RendezvousSyncSender`]. Zero-cost.
  pub fn to_sync(self) -> RendezvousSyncSender<T> {
    let shared = unsafe { std::ptr::read(&self.shared) };
    mem::forget(self);
    RendezvousSyncSender {
      shared,
      closed: AtomicBool::new(false),
    }
  }
}

impl<T: Send> Drop for RendezvousAsyncSender<T> {
  fn drop(&mut self) {
    let _ = self.close();
  }
}

// --- RendezvousAsyncReceiver --------------------------------------------------------

impl<T: Send> RendezvousAsyncReceiver<T> {
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

  /// See [`RendezvousSyncReceiver::close`].
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

  /// Converts this handle into a synchronous [`RendezvousSyncReceiver`]. Zero-cost.
  pub fn to_sync(self) -> RendezvousSyncReceiver<T> {
    let shared = unsafe { std::ptr::read(&self.shared) };
    mem::forget(self);
    RendezvousSyncReceiver {
      shared,
      closed: AtomicBool::new(false),
    }
  }
}

impl<T: Send> Drop for RendezvousAsyncReceiver<T> {
  fn drop(&mut self) {
    let _ = self.close();
  }
}

// --- Futures --------------------------------------------------------------

/// Future returned by [`RendezvousAsyncSender::send`].
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

/// Future returned by [`RendezvousAsyncReceiver::recv`].
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
