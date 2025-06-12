// src/mpmc/async_impl.rs
//! Implementation of the asynchronous Future-based send and receive logic.

use futures_core::Stream;

use super::core::{AsyncWaiter, WaiterData};
use super::{AsyncReceiver, AsyncSender};
use crate::error::{SendError, TryRecvError, TrySendError};
use crate::RecvError;

use core::marker::PhantomPinned;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

// --- SendFuture ---

/// A future that completes when a value has been sent to the MPMC channel.
#[must_use = "futures do nothing unless you .await or poll them"]
#[derive(Debug)]
pub struct SendFuture<'a, T: Send> {
  sender: &'a AsyncSender<T>,
  // The item is wrapped in an Option so it can be taken during the poll.
  item: Option<T>,
  _phantom: PhantomPinned,
}

impl<'a, T: Send> SendFuture<'a, T> {
  pub(super) fn new(sender: &'a AsyncSender<T>, item: T) -> Self {
    Self {
      sender,
      item: Some(item),
      _phantom: PhantomPinned,
    }
  }
}

impl<'a, T: Send> Future for SendFuture<'a, T> {
  type Output = Result<(), SendError>;

  fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    let this = unsafe { self.as_mut().get_unchecked_mut() };
    'poll_loop: loop {
      // If the item has already been sent, the future is complete.
      if this.item.is_none() {
        // This can happen if poll is called again after it has already completed.
        return Poll::Ready(Ok(()));
      }

      // --- Phase 1: Try to send without parking ---
      let item_to_send = this.item.take().unwrap();
      match this.sender.shared.try_send_core(item_to_send) {
        Ok(()) => {
          return Poll::Ready(Ok(())); // Success!
        }
        Err(TrySendError::Full(returned_item)) => {
          // Channel is full, must park. Put the item back.
          this.item = Some(returned_item);
        }
        Err(TrySendError::Closed(_)) => {
          return Poll::Ready(Err(SendError::Closed));
        }
        Err(TrySendError::Sent(_)) => unreachable!(),
      }

      // --- Phase 2: Prepare to park ---
      let is_rendezvous = this.sender.shared.capacity == 0;

      // --- Phase 3: Lock, re-check, and commit to parking ---
      {
        let mut guard = this.sender.shared.internal.lock();

        // Re-check under lock. State might have changed.
        if !guard.waiting_async_receivers.is_empty()
          || !guard.waiting_sync_receivers.is_empty()
          || (this.sender.shared.capacity > 0 && guard.queue.len() < this.sender.shared.capacity)
        {
          drop(guard);
          continue 'poll_loop; // Retry immediately.
        }

        if guard.receiver_count == 0 {
          this.item = None; // Drop the item.
          return Poll::Ready(Err(SendError::Closed));
        }

        // Safe to park. Create the waiter and add it to the async queue.
        let waiter = AsyncWaiter {
          waker: cx.waker().clone(),
          data: if is_rendezvous {
            Some(WaiterData::SenderItem(this.item.take()))
          } else {
            None
          },
        };
        guard.waiting_async_senders.push_back(waiter);
        return Poll::Pending;
      }
    }
  }
}

/// A future that completes when a value has been received from the MPMC channel.
#[must_use = "futures do nothing unless you .await or poll them"]
#[derive(Debug)]
pub struct RecvFuture<'a, T: Send> {
  receiver: &'a AsyncReceiver<T>,
}

impl<'a, T: Send> RecvFuture<'a, T> {
  pub(super) fn new(receiver: &'a AsyncReceiver<T>) -> Self {
    Self { receiver }
  }
}

impl<'a, T: Send> Future for RecvFuture<'a, T> {
  type Output = Result<T, RecvError>;

  fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    self.receiver.shared.poll_recv_internal(cx)
  }
}

impl<T: Send> Stream for AsyncReceiver<T> {
  type Item = T;

  fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
    // Delegate to the internal poll function and map the Result to an Option.
    match self.shared.poll_recv_internal(cx) {
      Poll::Ready(Ok(value)) => Poll::Ready(Some(value)),
      Poll::Ready(Err(_)) => Poll::Ready(None), // Disconnected
      Poll::Pending => Poll::Pending,
    }
  }
}
