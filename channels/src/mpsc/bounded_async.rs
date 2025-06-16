//! The asynchronous API for the bounded MPSC channel.

use super::bounded_sync::{BoundedMessage, BoundedMpscShared, Permit, Receiver, Sender};
use crate::error::{RecvError, SendError, TrySendError};
use crate::mpsc::unbounded;
use crate::{CloseError, TryRecvError};
use futures_core::Stream;

use std::future::Future;
use std::marker::PhantomPinned;
use std::mem;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};

// --- Public Channel Handles (Async) ---

#[derive(Debug)]
pub struct AsyncSender<T: Send> {
  pub(crate) shared: Arc<BoundedMpscShared<T>>,
  pub(crate) closed: AtomicBool,
}

#[derive(Debug)]
pub struct AsyncReceiver<T: Send> {
  pub(crate) shared: Arc<BoundedMpscShared<T>>,
  pub(crate) closed: AtomicBool,
}

// --- Sender Implementation (Async) ---

impl<T: Send> AsyncSender<T> {
  /// Sends a value asynchronously.
  /// The returned future completes when the value has been sent. It will wait
  /// if the channel is currently full.
  pub fn send(&self, value: T) -> SendFuture<'_, T> {
    SendFuture {
      // Eagerly create the future for acquiring a permit.
      acquire: self.shared.gate.acquire_async(),
      sender: self,
      value: Some(value),
      is_rendezvous: self.capacity() == 0, // Pass rendezvous info to future
      _phantom: PhantomPinned,
    }
  }
  /// Attempts to send a value into the channel without blocking.
  pub fn try_send(&self, value: T) -> Result<(), TrySendError<T>> {
    // We can just use the sync try_send logic directly.
    if self.is_closed() {
      return Err(TrySendError::Closed(value));
    }
    if !self.shared.gate.try_acquire() {
      return Err(TrySendError::Full(value));
    }
    let permit = Permit {
      gate: self.shared.gate.clone(),
      is_rendezvous: self.capacity() == 0,
    };
    let message = BoundedMessage {
      value,
      _permit: permit,
    };
    if let Err(msg) = unbounded::send_internal(&self.shared.channel, message) {
      return Err(TrySendError::Closed(msg.value));
    }
    Ok(())
  }

  /// Closes this sender handle.
  ///
  /// This is an explicit alternative to `drop`. If this is the last sender handle,
  /// the channel will become disconnected from the receiver's perspective.
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
    if self
      .shared
      .channel
      .sender_count
      .fetch_sub(1, Ordering::AcqRel)
      == 1
    {
      self.shared.channel.wake_consumer();
      // Also wake the gate in case the receiver is waiting on a rendezvous
      self.shared.gate.release();
    }
  }

  /// Returns `true` if the receiver has been dropped.
  pub fn is_closed(&self) -> bool {
    self.shared.channel.receiver_dropped.load(Ordering::Acquire)
  }

  /// Returns the number of messages currently in the channel.
  pub fn len(&self) -> usize {
    self.shared.channel.current_len.load(Ordering::Relaxed)
  }

  /// Returns `true` if the channel is empty.
  pub fn is_empty(&self) -> bool {
    self.len() == 0
  }

  /// Returns the total capacity of the channel.
  pub fn capacity(&self) -> usize {
    self.shared.gate.capacity()
  }

  /// Returns `true` if the channel is full.
  pub fn is_full(&self) -> bool {
    self.len() == self.capacity()
  }

  /// Converts this `AsyncSender` into a synchronous `Sender`.
  pub fn to_sync(self) -> Sender<T> {
    let shared = unsafe { std::ptr::read(&self.shared) };
    mem::forget(self);
    Sender {
      shared,
      closed: AtomicBool::new(false),
    }
  }
}

impl<T: Send> Clone for AsyncSender<T> {
  fn clone(&self) -> Self {
    self
      .shared
      .channel
      .sender_count
      .fetch_add(1, Ordering::Relaxed);
    Self {
      shared: self.shared.clone(),
      closed: AtomicBool::new(false),
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

// --- Receiver Implementation (Async) ---

impl<T: Send> AsyncReceiver<T> {
  /// Receives a value asynchronously.
  pub fn recv(&self) -> RecvFuture<'_, T> {
    RecvFuture {
      receiver: self,
      rendezvous_permit_released: false,
    }
  }

  /// Attempts to receive a value without blocking.
  pub fn try_recv(&self) -> Result<T, TryRecvError> {
    if self.closed.load(Ordering::Relaxed) {
      return Err(TryRecvError::Disconnected);
    }
    // For rendezvous, release a permit to signal readiness for a try_send.
    if self.capacity() == 0 {
      self.shared.gate.release();
    }
    self.shared.channel.try_recv_internal().map(|msg| msg.value)
  }

  /// Closes the receiving end of the channel.
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
    self
      .shared
      .channel
      .receiver_dropped
      .store(true, Ordering::Release);
    // Drain using the internal method to unblock senders.
    while self.shared.channel.try_recv_internal().is_ok() {}
    // Also wake the gate in case a sender is waiting on a rendezvous
    self.shared.gate.release();
  }

  /// Returns `true` if all senders have been dropped and the channel is empty.
  pub fn is_closed(&self) -> bool {
    let chan = &self.shared.channel;
    chan.sender_count.load(Ordering::Acquire) == 0 && self.is_empty()
  }

  /// Returns the number of messages currently in the channel.
  pub fn len(&self) -> usize {
    self.shared.channel.current_len.load(Ordering::Relaxed)
  }

  pub fn sender_count(&self) -> usize {
    self.shared.channel.sender_count.load(Ordering::Relaxed)
  }

  /// Returns `true` if the channel is empty.
  pub fn is_empty(&self) -> bool {
    self.len() == 0
  }

  /// Returns the total capacity of the channel.
  pub fn capacity(&self) -> usize {
    self.shared.gate.capacity()
  }

  /// Returns `true` if the channel is full.
  pub fn is_full(&self) -> bool {
    self.len() == self.capacity()
  }

  /// Converts this `AsyncReceiver` into a synchronous `Receiver`.
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
    if !self.closed.swap(true, Ordering::AcqRel) {
      self.close_internal();
    }
  }
}

// --- Future Implementations ---

#[must_use = "futures do nothing unless you .await or poll them"]
pub struct SendFuture<'a, T: Send> {
  // The future that acquires the permit.
  acquire: crate::coord::AcquireFuture<'a>,
  // The rest of the state, to be used once `acquire` is Ready.
  sender: &'a AsyncSender<T>,
  value: Option<T>,
  is_rendezvous: bool,
  // Make this future `!Unpin`.
  _phantom: PhantomPinned,
}

impl<'a, T: Send> Future for SendFuture<'a, T> {
  type Output = Result<(), SendError>;

  fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    // This is the key change. We get a `&mut SendFuture` from the `Pin`
    // by promising the compiler we won't move out of it.
    let this = unsafe { self.as_mut().get_unchecked_mut() };

    // Check if the receiver has been dropped.
    if this.sender.is_closed() {
      // Drop the value before returning
      this.value = None;
      return Poll::Ready(Err(SendError::Closed));
    }

    // Poll the gate first to acquire a permit. This is a "projection".
    // We get a `Pin<&mut SendFuture>` and poll its `acquire` field.
    match Pin::new(&mut this.acquire).poll(cx) {
      Poll::Ready(()) => {
        // We have a permit. Now we can send.
        let value = this
          .value
          .take()
          .expect("SendFuture polled after completion");
        let permit = Permit {
          gate: this.sender.shared.gate.clone(),
          is_rendezvous: this.is_rendezvous,
        };
        let message = BoundedMessage {
          value,
          _permit: permit,
        };

        match unbounded::send_internal(&this.sender.shared.channel, message) {
          Ok(()) => Poll::Ready(Ok(())),
          Err(_) => Poll::Ready(Err(SendError::Closed)),
        }
      }
      Poll::Pending => Poll::Pending,
    }
  }
}

#[must_use = "futures do nothing unless you .await or poll them"]
#[derive(Debug)]
pub struct RecvFuture<'a, T: Send> {
  receiver: &'a AsyncReceiver<T>,
  // State to ensure we only release one permit per recv() call for rendezvous.
  rendezvous_permit_released: bool,
}

impl<'a, T: Send> Future for RecvFuture<'a, T> {
  type Output = Result<T, RecvError>;
  fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    let this = self.get_mut(); // Pin allows getting a mutable ref

    if this.receiver.closed.load(Ordering::Relaxed) {
      return Poll::Ready(Err(RecvError::Disconnected));
    }

    // For rendezvous channels, release a permit on the first poll to
    // signal that the receiver is ready.
    if this.receiver.capacity() == 0 && !this.rendezvous_permit_released {
      this.receiver.shared.gate.release();
      this.rendezvous_permit_released = true;
    }

    match this.receiver.shared.channel.poll_recv_internal(cx) {
      Poll::Ready(Ok(msg)) => Poll::Ready(Ok(msg.value)),
      Poll::Ready(Err(e)) => Poll::Ready(Err(e)),
      Poll::Pending => Poll::Pending,
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

    // A Stream is a series of `recv` calls. For rendezvous, we must
    // release a permit before each poll.
    if this.capacity() == 0 {
      this.shared.gate.release();
    }

    match this.shared.channel.poll_recv_internal(cx) {
      Poll::Ready(Ok(msg)) => Poll::Ready(Some(msg.value)),
      Poll::Ready(Err(_)) => Poll::Ready(None),
      Poll::Pending => Poll::Pending,
    }
  }
}
