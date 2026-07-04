//! Implementation of the asynchronous Future-based send and receive logic.

use futures_core::Stream;

use super::core::{AsyncWaiter, STATE_CANCELLED, STATE_WAITING};
use super::{AsyncReceiver, AsyncSender};
use crate::error::{BatchSendErrorReason, SendBatchError, SendError, TrySendError};
use crate::RecvError;

use core::marker::PhantomPinned;
use std::future::Future;
use std::pin::Pin;
use crate::internal::sync::{AtomicU8, Ordering};
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
    if self.is_registered {
      let _ = self.state.compare_exchange(
        STATE_WAITING,
        STATE_CANCELLED,
        Ordering::SeqCst,
        Ordering::SeqCst,
      );
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
    let state_ptr = &this.state as *const AtomicU8;

    'poll_loop: loop {
      if this.is_registered {
        let st = this.state.load(Ordering::SeqCst);

        if (st & 0x01) != 0 {
          this.is_registered = false;

          let mut guard = this.sender.shared.internal.lock();
          if let Some(pos) = guard
            .waiting_async_senders
            .iter()
            .position(|w| w.state == state_ptr)
          {
            let _ = guard.waiting_async_senders.remove(pos).unwrap();
          }
          drop(guard);

          if (st & 0x02) == 0 {
            return Poll::Ready(Err(SendError::Closed));
          }
          // STATE_SUCCESS_SPACE: item is still in this.item, loop back to retry
        } else {
          let mut guard = this.sender.shared.internal.lock();
          if let Some(w) = guard
            .waiting_async_senders
            .iter_mut()
            .find(|w| w.state == state_ptr)
          {
            w.waker = cx.waker().clone();
            return Poll::Pending;
          }
          drop(guard);
          cx.waker().wake_by_ref();
          return Poll::Pending;
        }
      }

      if this.item.is_none() {
        return Poll::Ready(Ok(()));
      }

      let item_to_send = this.item.take().unwrap();
      match this.sender.shared.try_send_core(item_to_send) {
        Ok(()) => {
          this.is_registered = false;
          return Poll::Ready(Ok(()));
        }
        Err(TrySendError::Full(returned_item)) => {
          this.item = Some(returned_item);
        }
        Err(TrySendError::Closed(returned_item)) => {
          this.item = Some(returned_item);
          this.is_registered = false;
          return Poll::Ready(Err(SendError::Closed));
        }
        Err(TrySendError::Sent(_)) => unreachable!(),
      }

      {
        let mut guard = this.sender.shared.internal.lock();

        if !guard.is_full(this.sender.shared.capacity)
          && (!guard.waiting_async_receivers.is_empty()
            || !guard.waiting_sync_receivers.is_empty()
            || (this.sender.shared.capacity > 0 && guard.len() < this.sender.shared.capacity))
        {
          drop(guard);
          continue 'poll_loop;
        }

        if guard.receiver_count == 0 {
          this.is_registered = false;
          return Poll::Ready(Err(SendError::Closed));
        }

        this.is_registered = true;
        this.state.store(STATE_WAITING, Ordering::SeqCst);

        guard.waiting_async_senders.push_back(AsyncWaiter {
          waker: cx.waker().clone(),
          state: state_ptr,
        });

        return Poll::Pending;
      }
    }
  }
}

// --- SendBatchFuture ---

/// A future that completes once an entire batch has been sent to the MPMC
/// channel (or the channel closes).
///
/// If dropped before completion, the unsent remainder is dropped (including a
/// rendezvous payload parked in the waiter queue). Use
/// [`AsyncSender::send_batch_mut`] for the cancel-safe variant.
#[must_use = "futures do nothing unless you .await or poll them"]
pub struct SendBatchFuture<'a, T: Send> {
  sender: &'a AsyncSender<T>,
  iter: std::vec::IntoIter<T>,
  total: usize,
  sent: usize,
  pending: Option<T>,
  state: AtomicU8,
  is_registered: bool,
  _phantom: PhantomPinned,
}

impl<'a, T: Send> SendBatchFuture<'a, T> {
  pub(super) fn new(sender: &'a AsyncSender<T>, items: Vec<T>) -> Self {
    let total = items.len();
    Self {
      sender,
      iter: items.into_iter(),
      total,
      sent: 0,
      pending: None,
      state: AtomicU8::new(STATE_WAITING),
      is_registered: false,
      _phantom: PhantomPinned,
    }
  }
}

impl<T: Send> Drop for SendBatchFuture<'_, T> {
  fn drop(&mut self) {
    if self.is_registered {
      let _ = self.state.compare_exchange(
        STATE_WAITING,
        STATE_CANCELLED,
        Ordering::SeqCst,
        Ordering::SeqCst,
      );
      let mut guard = self.sender.shared.internal.lock();
      let state_ptr = &self.state as *const AtomicU8;
      guard.waiting_async_senders.retain(|w| w.state != state_ptr);
    }
  }
}

impl<'a, T: Send> Future for SendBatchFuture<'a, T> {
  type Output = Result<usize, SendBatchError<T>>;

  fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    let this = unsafe { self.as_mut().get_unchecked_mut() };
    let state_ptr = &this.state as *const AtomicU8;

    'poll_loop: loop {
      if this.is_registered {
        let st = this.state.load(Ordering::SeqCst);

        // Bit 0 / 0x01 is the FINISHED bit
        if (st & 0x01) != 0 {
          this.is_registered = false;

          // Check if we closed (Bit 1 / 0x02 is 0)
          if (st & 0x02) == 0 {
            let mut guard = this.sender.shared.internal.lock();
            if let Some(pos) = guard
              .waiting_async_senders
              .iter()
              .position(|w| w.state == state_ptr)
            {
              let _ = guard.waiting_async_senders.remove(pos).unwrap();
            }
            drop(guard);
            return Poll::Ready(Err(SendBatchError {
              sent: this.sent,
              unsent: this.pending
                .take()
                .into_iter()
                .chain(this.iter.by_ref())
                .collect(),
            }));
          }

          // It was a success!
          // If rendezvous (Bit 2 / 0x04 is set), the receiver took the item.
          if (st & 0x04) != 0 {
            this.sent += 1;
            if this.sent == this.total {
              return Poll::Ready(Ok(this.total));
            }
          } else {
            // Space retry: remove the waiter, item stays in this.pending
            let mut guard = this.sender.shared.internal.lock();
            if let Some(pos) = guard
              .waiting_async_senders
              .iter()
              .position(|w| w.state == state_ptr)
            {
              let _ = guard.waiting_async_senders.remove(pos).unwrap();
            }
            drop(guard);
          }
        } else {
          // Still parked: a spurious poll — update the waker in place.
          let mut guard = this.sender.shared.internal.lock();
          if let Some(w) = guard
            .waiting_async_senders
            .iter_mut()
            .find(|w| w.state == state_ptr)
          {
            w.waker = cx.waker().clone();
            return Poll::Pending;
          }
          // Popped but our state not yet flipped (a close path holds the
          // waiter outside the lock): yield back to the executor and re-poll.
          drop(guard);
          cx.waker().wake_by_ref();
          return Poll::Pending;
        }
      }

      if let Some(item) = this.pending.take() {
        let mut once = std::iter::once(item);
        let (k, reason) = this.sender.shared.try_send_batch_core(&mut once, 1);
        this.sent += k;
        if k == 0 {
          this.pending = once.next();
          if matches!(reason, Some(BatchSendErrorReason::Closed)) {
            let mut unsent: Vec<T> = this.pending.take().into_iter().collect();
            unsent.extend(this.iter.by_ref());
            return Poll::Ready(Err(SendBatchError {
              sent: this.sent,
              unsent,
            }));
          }
        }
      }

      if this.pending.is_none() {
        if this.sent == this.total {
          return Poll::Ready(Ok(this.total));
        }

        let (k, reason) = this
          .sender
          .shared
          .try_send_batch_core(&mut this.iter, this.total - this.sent);
        this.sent += k;
        if this.sent == this.total {
          return Poll::Ready(Ok(this.total));
        }
        if matches!(reason, Some(BatchSendErrorReason::Closed)) {
          return Poll::Ready(Err(SendBatchError {
            sent: this.sent,
            unsent: this.iter.by_ref().collect(),
          }));
        }
      }

      {
        let mut guard = this.sender.shared.internal.lock();

        if !guard.is_full(this.sender.shared.capacity)
          && (!guard.waiting_async_receivers.is_empty()
            || !guard.waiting_sync_receivers.is_empty()
            || (this.sender.shared.capacity > 0 && guard.len() < this.sender.shared.capacity))
        {
          drop(guard);
          continue 'poll_loop;
        }

        if guard.receiver_count == 0 {
          return Poll::Ready(Err(SendBatchError {
            sent: this.sent,
            unsent: this.iter.by_ref().collect(),
          }));
        }

        this.is_registered = true;
        this.state.store(STATE_WAITING, Ordering::SeqCst);

        guard.waiting_async_senders.push_back(AsyncWaiter {
          waker: cx.waker().clone(),
          state: state_ptr,
        });
        return Poll::Pending;
      }
    }
  }
}

// --- SendBatchMutFuture ---

/// A future that sends a batch in place, draining sent items from the
/// caller's vector.
///
/// Cancel-safe: if dropped before completion, every unsent item — including a
/// rendezvous payload parked in the waiter queue, which is recovered on drop —
/// remains in the caller's vector.
#[must_use = "futures do nothing unless you .await or poll them"]
pub struct SendBatchMutFuture<'a, T: Send> {
  sender: &'a AsyncSender<T>,
  items: &'a mut Vec<T>,
  sent: usize,
  pending: Option<T>,
  state: AtomicU8,
  is_registered: bool,
  _phantom: PhantomPinned,
}

impl<'a, T: Send> SendBatchMutFuture<'a, T> {
  pub(super) fn new(sender: &'a AsyncSender<T>, items: &'a mut Vec<T>) -> Self {
    Self {
      sender,
      items,
      sent: 0,
      pending: None,
      state: AtomicU8::new(STATE_WAITING),
      is_registered: false,
      _phantom: PhantomPinned,
    }
  }
}

impl<T: Send> Drop for SendBatchMutFuture<'_, T> {
  fn drop(&mut self) {
    if self.is_registered {
      let _ = self.state.compare_exchange(
        STATE_WAITING,
        STATE_CANCELLED,
        Ordering::SeqCst,
        Ordering::SeqCst,
      );
      let mut guard = self.sender.shared.internal.lock();
      let state_ptr = &self.state as *const AtomicU8;
      guard.waiting_async_senders.retain(|w| w.state != state_ptr);
    }
    if let Some(item) = self.pending.take() {
      self.items.insert(0, item);
    }
  }
}

impl<'a, T: Send> Future for SendBatchMutFuture<'a, T> {
  type Output = Result<usize, SendError>;

  fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    let this = unsafe { self.as_mut().get_unchecked_mut() };
    let state_ptr = &this.state as *const AtomicU8;

    'poll_loop: loop {
      if this.is_registered {
        let st = this.state.load(Ordering::SeqCst);

        // Bit 0 / 0x01 is the FINISHED bit
        if (st & 0x01) != 0 {
          this.is_registered = false;

          // Check if we closed (Bit 1 / 0x02 is 0)
          if (st & 0x02) == 0 {
            let mut guard = this.sender.shared.internal.lock();
            if let Some(pos) = guard
              .waiting_async_senders
              .iter()
              .position(|w| w.state == state_ptr)
            {
              let _ = guard.waiting_async_senders.remove(pos).unwrap();
            }
            drop(guard);
            return Poll::Ready(Err(SendError::Closed));
          }

          // It was a success!
          if (st & 0x04) != 0 {
            this.sent += 1;
          } else {
            // Space retry: remove the waiter, item stays in this.pending
            let mut guard = this.sender.shared.internal.lock();
            if let Some(pos) = guard
              .waiting_async_senders
              .iter()
              .position(|w| w.state == state_ptr)
            {
              let _ = guard.waiting_async_senders.remove(pos).unwrap();
            }
            drop(guard);
          }
        } else {
          let mut guard = this.sender.shared.internal.lock();
          if let Some(w) = guard
            .waiting_async_senders
            .iter_mut()
            .find(|w| w.state == state_ptr)
          {
            w.waker = cx.waker().clone();
            return Poll::Pending;
          }
          drop(guard);
          cx.waker().wake_by_ref();
          return Poll::Pending;
        }
      }

      if let Some(item) = this.pending.take() {
        let mut once = std::iter::once(item);
        let (k, reason) = this.sender.shared.try_send_batch_core(&mut once, 1);
        this.sent += k;
        if k == 0 {
          this.pending = once.next();
          if matches!(reason, Some(BatchSendErrorReason::Closed)) {
            if let Some(it) = this.pending.take() {
              this.items.insert(0, it);
            }
            return Poll::Ready(Err(SendError::Closed));
          }
        }
      }

      if this.pending.is_none() {
        if this.items.is_empty() {
          return Poll::Ready(Ok(this.sent));
        }

        let limit = this.items.len();
        let mut drain = this.items.drain(..);
        let (k, reason) = this.sender.shared.try_send_batch_core(&mut drain, limit);
        let rest: Vec<T> = drain.collect();
        *this.items = rest;
        this.sent += k;
        if this.items.is_empty() {
          return Poll::Ready(Ok(this.sent));
        }
        if matches!(reason, Some(BatchSendErrorReason::Closed)) {
          return Poll::Ready(Err(SendError::Closed));
        }
      }

      // --- Phase 2/3: lock, re-check, commit to parking ---
      {
        let mut guard = this.sender.shared.internal.lock();

        if !guard.is_full(this.sender.shared.capacity)
          && (!guard.waiting_async_receivers.is_empty()
            || !guard.waiting_sync_receivers.is_empty()
            || (this.sender.shared.capacity > 0 && guard.len() < this.sender.shared.capacity))
        {
          drop(guard);
          continue 'poll_loop;
        }

        if guard.receiver_count == 0 {
          return Poll::Ready(Err(SendError::Closed));
        }

        this.is_registered = true;
        this.state.store(STATE_WAITING, Ordering::SeqCst);

        guard.waiting_async_senders.push_back(AsyncWaiter {
          waker: cx.waker().clone(),
          state: state_ptr,
        });
        return Poll::Pending;
      }
    }
  }
}

// --- RecvBatchFuture ---

/// A future that completes with between 1 and `max` items from the MPMC
/// channel.
///
/// Cancel-safe: items are only removed in the poll that resolves the future.
#[must_use = "futures do nothing unless you .await or poll them"]
#[derive(Debug)]
pub struct RecvBatchFuture<'a, T: Send> {
  receiver: &'a AsyncReceiver<T>,
  max: usize,
  state: AtomicU8,
  is_registered: bool,
  _phantom: PhantomPinned,
}

impl<'a, T: Send> RecvBatchFuture<'a, T> {
  pub(super) fn new(receiver: &'a AsyncReceiver<T>, max: usize) -> Self {
    Self {
      receiver,
      max,
      state: AtomicU8::new(STATE_WAITING),
      is_registered: false,
      _phantom: PhantomPinned,
    }
  }
}

impl<'a, T: Send> Future for RecvBatchFuture<'a, T> {
  type Output = Result<Vec<T>, RecvError>;

  fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    let this = unsafe { self.as_mut().get_unchecked_mut() };
    let state_ptr = &this.state as *const AtomicU8;
    let mut out = Vec::new();

    if this.is_registered {
      let st = this.state.load(Ordering::SeqCst);
      if (st & 0x01) != 0 {
        this.is_registered = false;
        if (st & 0x02) == 0 {
          let mut guard = this.receiver.shared.internal.lock();
          guard
            .waiting_async_receivers
            .retain(|w| w.state != state_ptr);
          drop(guard);
          return Poll::Ready(Err(RecvError::Disconnected));
        }
      }
    }

    this.is_registered = true;
    match this
      .receiver
      .shared
      .poll_recv_batch_internal(cx, state_ptr, &mut out, this.max)
    {
      Poll::Ready(Ok(_)) => {
        this.is_registered = false;
        Poll::Ready(Ok(out))
      }
      Poll::Ready(Err(e)) => {
        this.is_registered = false;
        Poll::Ready(Err(e))
      }
      Poll::Pending => Poll::Pending,
    }
  }
}

impl<T: Send> Drop for RecvBatchFuture<'_, T> {
  fn drop(&mut self) {
    if self.is_registered {
      let _ = self.state.compare_exchange(
        STATE_WAITING,
        STATE_CANCELLED,
        Ordering::SeqCst,
        Ordering::SeqCst,
      );
      let mut guard = self.receiver.shared.internal.lock();
      let state_ptr = &self.state as *const AtomicU8;
      guard
        .waiting_async_receivers
        .retain(|w| w.state != state_ptr);
    }
  }
}

// --- RecvBatchMutFuture ---

/// A future that receives up to `max` items, appending them to the caller's
/// vector, and completes with the count appended.
///
/// Cancel-safe: items are only appended in the poll that resolves the future.
#[must_use = "futures do nothing unless you .await or poll them"]
pub struct RecvBatchMutFuture<'a, T: Send> {
  receiver: &'a AsyncReceiver<T>,
  out: &'a mut Vec<T>,
  max: usize,
  state: AtomicU8,
  is_registered: bool,
  _phantom: PhantomPinned,
}

impl<'a, T: Send> RecvBatchMutFuture<'a, T> {
  pub(super) fn new(receiver: &'a AsyncReceiver<T>, out: &'a mut Vec<T>, max: usize) -> Self {
    Self {
      receiver,
      out,
      max,
      state: AtomicU8::new(STATE_WAITING),
      is_registered: false,
      _phantom: PhantomPinned,
    }
  }
}

impl<'a, T: Send> Future for RecvBatchMutFuture<'a, T> {
  type Output = Result<usize, RecvError>;

  fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    let this = unsafe { self.as_mut().get_unchecked_mut() };
    let state_ptr = &this.state as *const AtomicU8;
    let max = this.max;

    if this.is_registered {
      let st = this.state.load(Ordering::SeqCst);
      if (st & 0x01) != 0 {
        this.is_registered = false;
        if (st & 0x02) == 0 {
          let mut guard = this.receiver.shared.internal.lock();
          guard
            .waiting_async_receivers
            .retain(|w| w.state != state_ptr);
          drop(guard);
          return Poll::Ready(Err(RecvError::Disconnected));
        }
      }
    }

    this.is_registered = true;
    match this
      .receiver
      .shared
      .poll_recv_batch_internal(cx, state_ptr, this.out, max)
    {
      Poll::Ready(res) => {
        this.is_registered = false;
        Poll::Ready(res)
      }
      Poll::Pending => Poll::Pending,
    }
  }
}

impl<T: Send> Drop for RecvBatchMutFuture<'_, T> {
  fn drop(&mut self) {
    if self.is_registered {
      let _ = self.state.compare_exchange(
        STATE_WAITING,
        STATE_CANCELLED,
        Ordering::SeqCst,
        Ordering::SeqCst,
      );
      let mut guard = self.receiver.shared.internal.lock();
      let state_ptr = &self.state as *const AtomicU8;
      guard
        .waiting_async_receivers
        .retain(|w| w.state != state_ptr);
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

    if this.is_registered {
      let st = this.state.load(Ordering::SeqCst);
      if (st & 0x01) != 0 {
        this.is_registered = false;
        if (st & 0x02) == 0 {
          let mut guard = this.receiver.shared.internal.lock();
          guard
            .waiting_async_receivers
            .retain(|w| w.state != state_ptr);
          drop(guard);
          return Poll::Ready(Err(RecvError::Disconnected));
        }
      }
    }

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
    if self.is_registered {
      let _ = self.state.compare_exchange(
        STATE_WAITING,
        STATE_CANCELLED,
        Ordering::SeqCst,
        Ordering::SeqCst,
      );
      // Eagerly unlink the waiter so the future's memory can be safely freed.
      let mut guard = self.receiver.shared.internal.lock();
      let state_ptr = &self.state as *const AtomicU8;
      guard
        .waiting_async_receivers
        .retain(|w| w.state != state_ptr);
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
