//! Implementation of the asynchronous Future-based send and receive logic.

use futures_core::Stream;

use super::core::{AsyncWaiter, WaiterData, STATE_CANCELLED, STATE_WAITING};
use super::{AsyncReceiver, AsyncSender};
use crate::error::{SendError, TrySendError};
use crate::RecvError;

use core::marker::PhantomPinned;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU8, Ordering};
use std::task::{Context, Poll};

// --- SendFuture ---

/// A future that completes when a value has been sent to the MPMC channel.
#[must_use = "futures do nothing unless you .await or poll them"]
#[derive(Debug)]
pub struct SendFuture<'a, T: Send> {
  sender: &'a AsyncSender<T>,
  item: Option<T>,
  /// Inline state flag; a raw pointer to this field is stored in the queued waiter.
  /// `PhantomPinned` ensures the future cannot be moved while registered.
  state: AtomicU8,
  is_registered: bool,
  _phantom: PhantomPinned,
}

impl<'a, T: Send> SendFuture<'a, T> {
  pub(super) fn new(sender: &'a AsyncSender<T>, item: T) -> Self {
    Self {
      sender,
      item: Some(item),
      state: AtomicU8::new(STATE_WAITING),
      is_registered: false,
      _phantom: PhantomPinned,
    }
  }
}

impl<T: Send> Drop for SendFuture<'_, T> {
  fn drop(&mut self) {
    if self.is_registered
      && self
        .state
        .compare_exchange(STATE_WAITING, STATE_CANCELLED, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
    {
      // Eagerly unlink the waiter so the future's memory can be safely freed.
      let mut guard = self.sender.shared.internal.lock();
      let state_ptr = &self.state as *const AtomicU8;
      guard.waiting_async_senders.retain(|w| w.state != state_ptr);
    }
  }
}

impl<'a, T: Send> Future for SendFuture<'a, T> {
  type Output = Result<(), SendError>;

  fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    let this = unsafe { self.as_mut().get_unchecked_mut() };
    'poll_loop: loop {
      if this.item.is_none() {
        return Poll::Ready(Ok(()));
      }

      // --- Phase 1: Try to send without parking ---
      let item_to_send = this.item.take().unwrap();
      match this.sender.shared.try_send_core(item_to_send) {
        Ok(()) => {
          this.is_registered = false;
          return Poll::Ready(Ok(())); // Success!
        }
        Err(TrySendError::Full(returned_item)) => {
          this.item = Some(returned_item);
        }
        Err(TrySendError::Closed(_)) => {
          this.is_registered = false;
          return Poll::Ready(Err(SendError::Closed));
        }
        Err(TrySendError::Sent(_)) => unreachable!(),
      }

      // --- Phase 2: Prepare to park ---
      let is_rendezvous = this.sender.shared.capacity == 0;

      // --- Phase 3: Lock, re-check, and commit to parking ---
      {
        let mut guard = this.sender.shared.internal.lock();

        if !guard.waiting_async_receivers.is_empty()
          || !guard.waiting_sync_receivers.is_empty()
          || (this.sender.shared.capacity > 0 && guard.queue.len() < this.sender.shared.capacity)
        {
          drop(guard);
          continue 'poll_loop; // Retry immediately.
        }

        if guard.receiver_count == 0 {
          this.item = None;
          this.is_registered = false;
          return Poll::Ready(Err(SendError::Closed));
        }

        let new_waker = cx.waker();
        let state_ptr = &this.state as *const AtomicU8;

        if let Some(existing_waiter) = guard
          .waiting_async_senders
          .iter_mut()
          .find(|w| w.state == state_ptr)
        {
          existing_waiter.waker = new_waker.clone();
          if is_rendezvous {
            existing_waiter.data = Some(WaiterData::SenderItem(this.item.take()));
          }
        } else {
          this.is_registered = true;
          // Reset state to WAITING before registering to clear any stale SUCCESS state
          // from a previous wake-up cycle. Done under lock for safety.
          this.state.store(STATE_WAITING, Ordering::SeqCst);
          let waiter = AsyncWaiter {
            waker: new_waker.clone(),
            data: if is_rendezvous {
              Some(WaiterData::SenderItem(this.item.take()))
            } else {
              None
            },
            state: state_ptr,
          };
          guard.waiting_async_senders.push_back(waiter);
        }
        return Poll::Pending;
      }
    }
  }
}

// --- RecvFuture ---

/// A future that completes when a value has been received from the MPMC channel.
#[must_use = "futures do nothing unless you .await or poll them"]
#[derive(Debug)]
pub struct RecvFuture<'a, T: Send> {
  receiver: &'a AsyncReceiver<T>,
  /// Inline state flag; a raw pointer to this field is stored in the queued waiter.
  /// `PhantomPinned` ensures the future cannot be moved while registered.
  state: AtomicU8,
  is_registered: bool,
  _phantom: PhantomPinned,
}

impl<'a, T: Send> RecvFuture<'a, T> {
  pub(super) fn new(receiver: &'a AsyncReceiver<T>) -> Self {
    Self {
      receiver,
      state: AtomicU8::new(STATE_WAITING),
      is_registered: false,
      _phantom: PhantomPinned,
    }
  }
}

impl<'a, T: Send> Future for RecvFuture<'a, T> {
  type Output = Result<T, RecvError>;

  fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    // PhantomPinned makes RecvFuture !Unpin, so get_unchecked_mut is required.
    let this = unsafe { self.as_mut().get_unchecked_mut() };
    let state_ptr = &this.state as *const AtomicU8;
    this.is_registered = true;
    match this.receiver.shared.poll_recv_internal(cx, state_ptr) {
      Poll::Ready(res) => {
        this.is_registered = false;
        Poll::Ready(res)
      }
      Poll::Pending => Poll::Pending,
    }
  }
}

impl<T: Send> Drop for RecvFuture<'_, T> {
  fn drop(&mut self) {
    if self.is_registered
      && self
        .state
        .compare_exchange(STATE_WAITING, STATE_CANCELLED, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
    {
      // Eagerly unlink the waiter so the future's memory can be safely freed.
      let mut guard = self.receiver.shared.internal.lock();
      let state_ptr = &self.state as *const AtomicU8;
      guard.waiting_async_receivers.retain(|w| w.state != state_ptr);
    }
  }
}

// --- Stream Implementation ---

impl<T: Send> Stream for AsyncReceiver<T> {
  type Item = T;

  fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
    // AsyncReceiver is Unpin (no PhantomPinned), so get_mut() is safe.
    let this = self.get_mut();

    if this.closed.load(Ordering::Relaxed) {
      return Poll::Ready(None);
    }

    let state_ptr = &this.state as *const AtomicU8;
    this.is_registered = true;

    match this.shared.poll_recv_internal(cx, state_ptr) {
      Poll::Ready(Ok(value)) => {
        this.is_registered = false;
        this.state.store(STATE_WAITING, Ordering::Relaxed); // Reset for next recv cycle.
        Poll::Ready(Some(value))
      }
      Poll::Ready(Err(_)) => {
        this.is_registered = false;
        this.state.store(STATE_WAITING, Ordering::Relaxed);
        Poll::Ready(None) // Disconnected
      }
      Poll::Pending => Poll::Pending,
    }
  }
}
