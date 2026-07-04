//! Multi-producer, multi-consumer **rendezvous** channel.
//!
//! A rendezvous channel has zero capacity: a `send` completes only once a
//! `recv` takes the value as part of the same direct handoff, and vice versa.
//! It is a distinct channel family from the buffered [`mpmc`](crate::mpmc)
//! channel — there is no item buffer and no early deposit, so cancelling a
//! pending send can never ghost-deliver its payload.
//!
//! Senders and receivers can both be cloned. Both blocking and async ends are
//! provided and interoperate (a sync `RendezvousSyncSender` can hand off to an
//! `RendezvousAsyncReceiver`, etc.). See the module-level docs of
//! [`crate::internal::rendezvous`] for the transfer model and its safety
//! argument.

use crate::error::{CloseError, RecvError, RecvErrorTimeout, SendError, TryRecvError, TrySendError};
use crate::internal::rendezvous::MpmcRvShared;

use std::future::Future;
use std::marker::PhantomPinned;
use std::mem;
use std::pin::Pin;
use crate::internal::sync::{Arc, AtomicBool, AtomicU8, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;

use crate::internal::rendezvous::WAITING;

// --- Constructors ---------------------------------------------------------

/// Creates a synchronous MPMC rendezvous channel.
pub fn rendezvous<T: Send>() -> (RendezvousSyncSender<T>, RendezvousSyncReceiver<T>) {
  let shared = Arc::new(MpmcRvShared::new());
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

/// Creates an asynchronous MPMC rendezvous channel.
pub fn rendezvous_async<T: Send>() -> (RendezvousAsyncSender<T>, RendezvousAsyncReceiver<T>) {
  let shared = Arc::new(MpmcRvShared::new());
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

// --- Sync handles ---------------------------------------------------------

/// The synchronous sending half of an MPMC rendezvous channel.
#[derive(Debug)]
pub struct RendezvousSyncSender<T: Send> {
  shared: Arc<MpmcRvShared<T>>,
  closed: AtomicBool,
}

/// The synchronous receiving half of an MPMC rendezvous channel.
#[derive(Debug)]
pub struct RendezvousSyncReceiver<T: Send> {
  shared: Arc<MpmcRvShared<T>>,
  closed: AtomicBool,
}

// --- Async handles --------------------------------------------------------

/// The asynchronous sending half of an MPMC rendezvous channel.
#[derive(Debug)]
pub struct RendezvousAsyncSender<T: Send> {
  shared: Arc<MpmcRvShared<T>>,
  closed: AtomicBool,
}

/// The asynchronous receiving half of an MPMC rendezvous channel.
#[derive(Debug)]
pub struct RendezvousAsyncReceiver<T: Send> {
  shared: Arc<MpmcRvShared<T>>,
  closed: AtomicBool,
}

// --- RendezvousSyncSender (sync) --------------------------------------------------------

impl<T: Send> RendezvousSyncSender<T> {
  /// Sends a value, blocking until a receiver takes it or the channel closes.
  pub fn send(&self, item: T) -> Result<(), SendError> {
    if self.closed.load(Ordering::Relaxed) {
      return Err(SendError::Closed);
    }
    self.shared.send_blocking(item)
  }

  /// Attempts to hand off a value to an already-waiting receiver without
  /// blocking. Returns `TrySendError::Full` if no receiver is currently parked.
  pub fn try_send(&self, item: T) -> Result<(), TrySendError<T>> {
    if self.closed.load(Ordering::Relaxed) {
      return Err(TrySendError::Closed(item));
    }
    self.shared.try_send(item)
  }

  /// Closes this handle. If it is the last sender, parked receivers observe
  /// disconnection.
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
    self.shared.receiver_count() == 0
  }

  /// Always `0` — a rendezvous channel never buffers items.
  #[inline]
  pub fn len(&self) -> usize {
    0
  }

  /// Always `true` — a rendezvous channel never buffers items.
  #[inline]
  pub fn is_empty(&self) -> bool {
    true
  }

  /// Always `true` — a zero-capacity channel can never accept buffered items.
  #[inline]
  pub fn is_full(&self) -> bool {
    true
  }

  /// Always `Some(0)`.
  #[inline]
  pub fn capacity(&self) -> Option<usize> {
    Some(0)
  }

  /// Converts this handle into an asynchronous [`RendezvousAsyncSender`]. Zero-cost; the
  /// original handle's `Drop` does not run.
  pub fn to_async(self) -> RendezvousAsyncSender<T> {
    let shared = unsafe { std::ptr::read(&self.shared) };
    mem::forget(self);
    RendezvousAsyncSender {
      shared,
      closed: AtomicBool::new(false),
    }
  }
}

impl<T: Send> Clone for RendezvousSyncSender<T> {
  fn clone(&self) -> Self {
    self.shared.add_sender();
    RendezvousSyncSender {
      shared: Arc::clone(&self.shared),
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
  /// Receives a value, blocking until a sender hands one off or the channel
  /// disconnects.
  pub fn recv(&self) -> Result<T, RecvError> {
    if self.closed.load(Ordering::Relaxed) {
      return Err(RecvError::Disconnected);
    }
    self.shared.recv_blocking()
  }

  /// Attempts to take a value from an already-waiting sender without blocking.
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

  /// Closes this handle. If it is the last receiver, parked senders observe
  /// closure.
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

  /// Returns `true` if all senders have been dropped.
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

impl<T: Send> Clone for RendezvousSyncReceiver<T> {
  fn clone(&self) -> Self {
    self.shared.add_receiver();
    RendezvousSyncReceiver {
      shared: Arc::clone(&self.shared),
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
  /// Sends a value, resolving once a receiver takes it or the channel closes.
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

  /// Returns `true` if all receivers have been dropped.
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

impl<T: Send> Clone for RendezvousAsyncSender<T> {
  fn clone(&self) -> Self {
    self.shared.add_sender();
    RendezvousAsyncSender {
      shared: Arc::clone(&self.shared),
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
  /// Receives a value, resolving once a sender hands one off or the channel
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

  /// Returns `true` if all senders have been dropped.
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

impl<T: Send> Clone for RendezvousAsyncReceiver<T> {
  fn clone(&self) -> Self {
    self.shared.add_receiver();
    RendezvousAsyncReceiver {
      shared: Arc::clone(&self.shared),
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
//
// Concrete per-module futures (rather than a shared generic) so the public
// types don't leak the `pub(crate)` core. They are `!Unpin`: the core stores
// raw pointers to the inline `state`/`slot`, so the future must not move while
// registered.

/// Future returned by [`RendezvousAsyncSender::send`].
#[must_use = "futures do nothing unless you .await or poll them"]
pub struct SendFuture<'a, T: Send> {
  shared: &'a Arc<MpmcRvShared<T>>,
  slot: Option<T>,
  state: AtomicU8,
  registered: bool,
  _pin: PhantomPinned,
}

impl<'a, T: Send> SendFuture<'a, T> {
  fn new(shared: &'a Arc<MpmcRvShared<T>>, item: T) -> Self {
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
      // If the cancel wins, our inline `slot` still holds the item and is
      // dropped with the future — no ghost delivery. If it loses, the item was
      // already handed off under the lock and delivery stands.
      self
        .shared
        .cancel_sender(&self.state as *const AtomicU8, &self.state);
    }
  }
}

/// Future returned by [`RendezvousAsyncReceiver::recv`].
#[must_use = "futures do nothing unless you .await or poll them"]
pub struct RecvFuture<'a, T: Send> {
  shared: &'a Arc<MpmcRvShared<T>>,
  dest: Option<T>,
  state: AtomicU8,
  registered: bool,
  _pin: PhantomPinned,
}

impl<'a, T: Send> RecvFuture<'a, T> {
  fn new(shared: &'a Arc<MpmcRvShared<T>>) -> Self {
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
      // If the cancel loses the race, a sender already wrote the item into our
      // inline `dest`; it is dropped with the future (the receiver abandoned
      // it). Either way the queue node is removed before the future frees.
      self
        .shared
        .cancel_receiver(&self.state as *const AtomicU8, &self.state);
    }
  }
}
