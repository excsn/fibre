//! Implementation of the asynchronous Future-based send and receive logic.

use super::core::{AsyncWaiter, WaiterData};
use super::{AsyncReceiver, AsyncSender};
use crate::error::{RecvError, SendError, TryRecvError, TrySendError};

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
}

impl<'a, T: Send> SendFuture<'a, T> {
  pub(super) fn new(sender: &'a AsyncSender<T>, item: T) -> Self {
    Self {
      sender,
      item: Some(item),
    }
  }
}

// The Unpin bound is required because we move the `item` out of the future.
impl<'a, T: Send + Unpin> Future for SendFuture<'a, T> {
  type Output = Result<(), SendError>;

  fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    'poll_loop: loop {
      // If the item has already been sent, the future is complete.
      if self.item.is_none() {
        // This can happen if poll is called again after it has already completed.
        return Poll::Ready(Ok(()));
      }

      // --- Phase 1: Try to send without parking ---
      let item_to_send = self.item.take().unwrap();
      match self.sender.shared.try_send_core(item_to_send) {
        Ok(()) => {
          return Poll::Ready(Ok(())); // Success!
        }
        Err(TrySendError::Full(returned_item)) => {
          // Channel is full, must park. Put the item back.
          self.item = Some(returned_item);
        }
        Err(TrySendError::Closed(_)) => {
          return Poll::Ready(Err(SendError::Closed));
        }
        Err(TrySendError::Sent(_)) => unreachable!(),
      }

      // --- Phase 2: Prepare to park ---
      let is_rendezvous = self.sender.shared.capacity == 0;

      // --- Phase 3: Lock, re-check, and commit to parking ---
      {
        let mut guard = self.sender.shared.internal.lock();

        // Re-check under lock. State might have changed.
        if !guard.waiting_async_receivers.is_empty()
          || !guard.waiting_sync_receivers.is_empty()
          || (self.sender.shared.capacity > 0 && guard.queue.len() < self.sender.shared.capacity)
        {
          drop(guard);
          continue 'poll_loop; // Retry immediately.
        }

        if guard.receiver_count == 0 {
          self.item = None; // Drop the item.
          return Poll::Ready(Err(SendError::Closed));
        }

        // Safe to park. Create the waiter and add it to the async queue.
        let waiter = AsyncWaiter {
          waker: cx.waker().clone(),
          data: if is_rendezvous {
            Some(WaiterData::SenderItem(self.item.take()))
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

// --- ReceiveFuture ---

/// A future that completes when a value has been received from the MPMC channel.
#[must_use = "futures do nothing unless you .await or poll them"]
#[derive(Debug)]
pub struct ReceiveFuture<'a, T: Send> {
  receiver: &'a AsyncReceiver<T>,
}

impl<'a, T: Send> ReceiveFuture<'a, T> {
  pub(super) fn new(receiver: &'a AsyncReceiver<T>) -> Self {
    Self { receiver }
  }
}

impl<'a, T: Send + Unpin> Future for ReceiveFuture<'a, T> {
  type Output = Result<T, RecvError>;

  fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    'poll_loop: loop {
      // --- Phase 1: Try to receive without parking ---
      match self.receiver.shared.try_recv_core() {
        Ok(item) => {
          return Poll::Ready(Ok(item));
        }
        Err(TryRecvError::Disconnected) => return Poll::Ready(Err(RecvError::Disconnected)),
        Err(TryRecvError::Empty) => { /* Proceed to park */ }
      }

      // --- Phase 2: Lock, re-check, and commit to parking ---
      {
        let mut guard = self.receiver.shared.internal.lock();

        // Re-check under lock. An item or a waiting sender might have appeared.
        if !guard.queue.is_empty() || !guard.waiting_sync_senders.is_empty() || !guard.waiting_async_senders.is_empty()
        {
          drop(guard);
          continue 'poll_loop; // Retry immediately.
        }

        if guard.sender_count == 0 {
          return Poll::Ready(Err(RecvError::Disconnected));
        }

        // Safe to park.
        let waiter = AsyncWaiter {
          waker: cx.waker().clone(),
          data: None, // Receivers never hold data.
        };
        guard.waiting_async_receivers.push_back(waiter);
        return Poll::Pending;
      }
    }
  }
}
