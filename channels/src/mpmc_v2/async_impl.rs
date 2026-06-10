//! Implementation of the asynchronous Future-based send and receive logic.

use futures_core::Stream;

use super::core::{AsyncWaiter, WaiterData, STATE_CANCELLED, STATE_SUCCESS, STATE_WAITING};
use super::{AsyncReceiver, AsyncSender};
use crate::error::{BatchSendErrorReason, SendBatchError, SendError, TrySendError};
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
  /// True while one rendezvous item is parked in our queued waiter's slot.
  in_flight: bool,
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
      in_flight: false,
      state: AtomicU8::new(STATE_WAITING),
      is_registered: false,
      _phantom: PhantomPinned,
    }
  }
}

impl<T: Send> Drop for SendBatchFuture<'_, T> {
  fn drop(&mut self) {
    if self.is_registered
      && self
        .state
        .compare_exchange(STATE_WAITING, STATE_CANCELLED, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
    {
      // Eagerly unlink the waiter so the future's memory can be safely freed.
      // A rendezvous payload in the waiter slot is dropped with it.
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
        if this.state.load(Ordering::SeqCst) == STATE_SUCCESS {
          // We were woken and popped from the waiter queue.
          this.is_registered = false;
          if this.in_flight {
            // The rendezvous payload left with the waiter (taken by a
            // receiver, or dropped by a close path). Mirror the single-item
            // future's optimism and count it as sent; a closed channel is
            // reported for the remainder on the next iteration.
            this.in_flight = false;
            this.sent += 1;
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
          // waiter outside the lock): spin briefly and re-check.
          drop(guard);
          std::hint::spin_loop();
          continue 'poll_loop;
        }
      }

      if this.sent == this.total {
        return Poll::Ready(Ok(this.total));
      }

      // --- Phase 1: non-blocking batch send ---
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

      // --- Phase 2/3: lock, re-check, commit to parking ---
      let is_rendezvous = this.sender.shared.capacity == 0;
      {
        let mut guard = this.sender.shared.internal.lock();

        if !guard.waiting_async_receivers.is_empty()
          || !guard.waiting_sync_receivers.is_empty()
          || (this.sender.shared.capacity > 0 && guard.queue.len() < this.sender.shared.capacity)
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
        let data = if is_rendezvous {
          let item = this.iter.next();
          debug_assert!(item.is_some(), "sent < total implies items remain");
          this.in_flight = true;
          Some(WaiterData::SenderItem(item))
        } else {
          None
        };
        guard.waiting_async_senders.push_back(AsyncWaiter {
          waker: cx.waker().clone(),
          data,
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
  /// True while one rendezvous item is parked in our queued waiter's slot.
  in_flight: bool,
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
      in_flight: false,
      state: AtomicU8::new(STATE_WAITING),
      is_registered: false,
      _phantom: PhantomPinned,
    }
  }
}

impl<T: Send> Drop for SendBatchMutFuture<'_, T> {
  fn drop(&mut self) {
    if self.is_registered
      && self
        .state
        .compare_exchange(STATE_WAITING, STATE_CANCELLED, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
    {
      let mut guard = self.sender.shared.internal.lock();
      let state_ptr = &self.state as *const AtomicU8;
      // Recover an in-flight rendezvous payload back into the caller's vector
      // before unlinking, preserving cancel safety.
      if let Some(w) = guard
        .waiting_async_senders
        .iter_mut()
        .find(|w| w.state == state_ptr)
      {
        if let Some(item) = w.take_item_from_sender_slot() {
          self.items.insert(0, item);
        }
      }
      guard.waiting_async_senders.retain(|w| w.state != state_ptr);
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
        if this.state.load(Ordering::SeqCst) == STATE_SUCCESS {
          this.is_registered = false;
          if this.in_flight {
            this.in_flight = false;
            this.sent += 1;
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
          std::hint::spin_loop();
          continue 'poll_loop;
        }
      }

      if this.items.is_empty() {
        return Poll::Ready(Ok(this.sent));
      }

      // --- Phase 1: non-blocking batch send straight out of the vector ---
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

      // --- Phase 2/3: lock, re-check, commit to parking ---
      let is_rendezvous = this.sender.shared.capacity == 0;
      {
        let mut guard = this.sender.shared.internal.lock();

        if !guard.waiting_async_receivers.is_empty()
          || !guard.waiting_sync_receivers.is_empty()
          || (this.sender.shared.capacity > 0 && guard.queue.len() < this.sender.shared.capacity)
        {
          drop(guard);
          continue 'poll_loop;
        }

        if guard.receiver_count == 0 {
          return Poll::Ready(Err(SendError::Closed));
        }

        this.is_registered = true;
        this.state.store(STATE_WAITING, Ordering::SeqCst);
        let data = if is_rendezvous {
          this.in_flight = true;
          Some(WaiterData::SenderItem(Some(this.items.remove(0))))
        } else {
          None
        };
        guard.waiting_async_senders.push_back(AsyncWaiter {
          waker: cx.waker().clone(),
          data,
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
    if self.is_registered
      && self
        .state
        .compare_exchange(STATE_WAITING, STATE_CANCELLED, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
    {
      let mut guard = self.receiver.shared.internal.lock();
      let state_ptr = &self.state as *const AtomicU8;
      guard.waiting_async_receivers.retain(|w| w.state != state_ptr);
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
    if self.is_registered
      && self
        .state
        .compare_exchange(STATE_WAITING, STATE_CANCELLED, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
    {
      let mut guard = self.receiver.shared.internal.lock();
      let state_ptr = &self.state as *const AtomicU8;
      guard.waiting_async_receivers.retain(|w| w.state != state_ptr);
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
