//! Async send/recv futures for `mpmc_v3`.
//!
//! Cancel-safety: each future suspends only while parked for space/data, holding
//! no slot. Its `Drop` just unregisters the parked waiter; a pending `SendFuture`
//! still owns its item inline, so dropping it neither delivers nor loses it
//! beyond the normal "the value is dropped with the cancelled future" semantics.
//!
//! The pre-`Pending` recheck re-runs the real `push`/`pop` (not a cursor-only
//! `is_full`/`is_empty` peek): the two-fence lost-wakeup handshake in
//! [`super::core`] pairs against each slot's `seq` stamp, which a cursor peek does
//! not observe (a producer mid-publish has advanced the cursor but not yet stamped
//! the slot).

use std::collections::VecDeque;
use std::future::Future;
use std::mem;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use crate::error::{RecvError, SendBatchError, SendError};

use super::core::{Role, Shared, WakeRef};

// --- Send -----------------------------------------------------------------

pub struct SendFuture<'a, T: Send> {
  shared: &'a Arc<Shared<T>>,
  item: Option<T>,
  my_id: Option<u64>,
}

impl<'a, T: Send> SendFuture<'a, T> {
  pub(crate) fn new(shared: &'a Arc<Shared<T>>, item: T) -> Self {
    SendFuture {
      shared,
      item: Some(item),
      my_id: None,
    }
  }

  #[inline]
  fn unregister(&mut self) {
    if let Some(id) = self.my_id.take() {
      self.shared.unregister(Role::Send, id);
    }
  }
}

impl<'a, T: Send> Future for SendFuture<'a, T> {
  type Output = Result<(), SendError>;

  fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    // Safety: `SendFuture` is never relied upon to stay pinned — it holds only an
    // `Option<T>`, a shared-ref, and an id, none self-referential, so moving its
    // fields is sound.
    let this = unsafe { self.get_unchecked_mut() };

    // Already completed (item taken on a prior successful poll).
    let mut item = match this.item.take() {
      Some(it) => it,
      None => return Poll::Ready(Ok(())),
    };

    if !this.shared.receivers_alive() {
      this.unregister();
      return Poll::Ready(Err(SendError::Closed));
    }
    match this.shared.ring.push(item) {
      Ok(()) => {
        this.unregister();
        this.shared.notify_receivers();
        return Poll::Ready(Ok(()));
      }
      Err(returned) => item = returned,
    }

    // Full: register/refresh waker, fence, then recheck with the real op.
    let id = this
      .shared
      .register(Role::Send, this.my_id, WakeRef::Waker(cx.waker().clone()), std::ptr::null());
    this.my_id = Some(id);
    this.shared.pre_park_fence();

    if !this.shared.receivers_alive() {
      this.unregister();
      return Poll::Ready(Err(SendError::Closed));
    }
    match this.shared.ring.push(item) {
      Ok(()) => {
        this.unregister();
        this.shared.notify_receivers();
        Poll::Ready(Ok(()))
      }
      Err(returned) => {
        this.item = Some(returned);
        Poll::Pending
      }
    }
  }
}

impl<'a, T: Send> Drop for SendFuture<'a, T> {
  fn drop(&mut self) {
    self.unregister();
  }
}

// --- Recv -----------------------------------------------------------------

pub struct RecvFuture<'a, T: Send> {
  shared: &'a Arc<Shared<T>>,
  my_id: Option<u64>,
}

impl<'a, T: Send> RecvFuture<'a, T> {
  pub(crate) fn new(shared: &'a Arc<Shared<T>>) -> Self {
    RecvFuture {
      shared,
      my_id: None,
    }
  }

  #[inline]
  fn unregister(&mut self) {
    if let Some(id) = self.my_id.take() {
      self.shared.unregister(Role::Recv, id);
    }
  }
}

impl<'a, T: Send> Future for RecvFuture<'a, T> {
  type Output = Result<T, RecvError>;

  fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    let this = self.get_mut();

    if let Some(item) = this.shared.ring.pop() {
      this.unregister();
      this.shared.notify_senders();
      return Poll::Ready(Ok(item));
    }
    if !this.shared.senders_alive() {
      // A sender may have published then dropped: drain once more.
      if let Some(item) = this.shared.ring.pop() {
        this.unregister();
        this.shared.notify_senders();
        return Poll::Ready(Ok(item));
      }
      this.unregister();
      return Poll::Ready(Err(RecvError::Disconnected));
    }

    // Empty: register/refresh waker, fence, then recheck with the real op.
    let id = this
      .shared
      .register(Role::Recv, this.my_id, WakeRef::Waker(cx.waker().clone()), std::ptr::null());
    this.my_id = Some(id);
    this.shared.pre_park_fence();

    if let Some(item) = this.shared.ring.pop() {
      this.unregister();
      this.shared.notify_senders();
      return Poll::Ready(Ok(item));
    }
    if !this.shared.senders_alive() {
      if let Some(item) = this.shared.ring.pop() {
        this.unregister();
        this.shared.notify_senders();
        return Poll::Ready(Ok(item));
      }
      this.unregister();
      return Poll::Ready(Err(RecvError::Disconnected));
    }
    Poll::Pending
  }
}

impl<'a, T: Send> Drop for RecvFuture<'a, T> {
  fn drop(&mut self) {
    self.unregister();
  }
}

// --- Batch send -----------------------------------------------------------

pub struct SendBatchFuture<'a, T: Send> {
  shared: &'a Arc<Shared<T>>,
  buf: VecDeque<T>,
  sent: usize,
  my_id: Option<u64>,
}

impl<'a, T: Send> SendBatchFuture<'a, T> {
  pub(crate) fn new(shared: &'a Arc<Shared<T>>, items: Vec<T>) -> Self {
    SendBatchFuture {
      shared,
      buf: items.into(),
      sent: 0,
      my_id: None,
    }
  }

  #[inline]
  fn unregister(&mut self) {
    if let Some(id) = self.my_id.take() {
      self.shared.unregister(Role::Send, id);
    }
  }
}

impl<'a, T: Send> Future for SendBatchFuture<'a, T> {
  type Output = Result<usize, SendBatchError<T>>;

  fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    // Safety: no self-referential state; safe to move.
    let this = unsafe { self.get_unchecked_mut() };

    loop {
      let item = match this.buf.pop_front() {
        Some(it) => it,
        None => {
          this.unregister();
          return Poll::Ready(Ok(this.sent));
        }
      };

      if !this.shared.receivers_alive() {
        this.buf.push_front(item);
        this.unregister();
        let unsent: Vec<T> = mem::take(&mut this.buf).into();
        return Poll::Ready(Err(SendBatchError { sent: this.sent, unsent }));
      }
      match this.shared.ring.push(item) {
        Ok(()) => {
          this.sent += 1;
          this.shared.notify_receivers();
          continue;
        }
        Err(returned) => this.buf.push_front(returned),
      }

      // Full: register/refresh, fence, recheck with the real op.
      let id = this
        .shared
        .register(Role::Send, this.my_id, WakeRef::Waker(cx.waker().clone()), std::ptr::null());
      this.my_id = Some(id);
      this.shared.pre_park_fence();

      let item = this.buf.pop_front().unwrap();
      if !this.shared.receivers_alive() {
        this.buf.push_front(item);
        this.unregister();
        let unsent: Vec<T> = mem::take(&mut this.buf).into();
        return Poll::Ready(Err(SendBatchError { sent: this.sent, unsent }));
      }
      match this.shared.ring.push(item) {
        Ok(()) => {
          this.sent += 1;
          this.shared.notify_receivers();
          continue;
        }
        Err(returned) => {
          this.buf.push_front(returned);
          return Poll::Pending;
        }
      }
    }
  }
}

impl<'a, T: Send> Drop for SendBatchFuture<'a, T> {
  fn drop(&mut self) {
    self.unregister();
  }
}

// --- Batch send (in place, cancel-safe) -----------------------------------

pub struct SendBatchMutFuture<'a, T: Send> {
  shared: &'a Arc<Shared<T>>,
  items: &'a mut Vec<T>,
  buf: VecDeque<T>,
  sent: usize,
  my_id: Option<u64>,
  done: bool,
}

impl<'a, T: Send> SendBatchMutFuture<'a, T> {
  pub(crate) fn new(shared: &'a Arc<Shared<T>>, items: &'a mut Vec<T>) -> Self {
    // Move the items inline; on completion or cancellation the unsent remainder
    // is written back into `items` (cancel-safety).
    let buf = mem::take(items).into();
    SendBatchMutFuture {
      shared,
      items,
      buf,
      sent: 0,
      my_id: None,
      done: false,
    }
  }

  #[inline]
  fn unregister(&mut self) {
    if let Some(id) = self.my_id.take() {
      self.shared.unregister(Role::Send, id);
    }
  }

  #[inline]
  fn restore_unsent(&mut self) {
    *self.items = mem::take(&mut self.buf).into();
  }
}

impl<'a, T: Send> Future for SendBatchMutFuture<'a, T> {
  type Output = Result<usize, SendError>;

  fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    // Safety: no self-referential state; safe to move.
    let this = unsafe { self.get_unchecked_mut() };

    loop {
      let item = match this.buf.pop_front() {
        Some(it) => it,
        None => {
          this.unregister();
          this.done = true;
          return Poll::Ready(Ok(this.sent));
        }
      };

      if !this.shared.receivers_alive() {
        this.buf.push_front(item);
        this.unregister();
        this.restore_unsent();
        this.done = true;
        return Poll::Ready(Err(SendError::Closed));
      }
      match this.shared.ring.push(item) {
        Ok(()) => {
          this.sent += 1;
          this.shared.notify_receivers();
          continue;
        }
        Err(returned) => this.buf.push_front(returned),
      }

      let id = this
        .shared
        .register(Role::Send, this.my_id, WakeRef::Waker(cx.waker().clone()), std::ptr::null());
      this.my_id = Some(id);
      this.shared.pre_park_fence();

      let item = this.buf.pop_front().unwrap();
      if !this.shared.receivers_alive() {
        this.buf.push_front(item);
        this.unregister();
        this.restore_unsent();
        this.done = true;
        return Poll::Ready(Err(SendError::Closed));
      }
      match this.shared.ring.push(item) {
        Ok(()) => {
          this.sent += 1;
          this.shared.notify_receivers();
          continue;
        }
        Err(returned) => {
          this.buf.push_front(returned);
          return Poll::Pending;
        }
      }
    }
  }
}

impl<'a, T: Send> Drop for SendBatchMutFuture<'a, T> {
  fn drop(&mut self) {
    self.unregister();
    if !self.done {
      // Cancelled mid-flight: hand the unsent remainder back to the caller.
      self.restore_unsent();
    }
  }
}

// --- Batch recv -----------------------------------------------------------

pub struct RecvBatchFuture<'a, T: Send> {
  shared: &'a Arc<Shared<T>>,
  max: usize,
  my_id: Option<u64>,
}

impl<'a, T: Send> RecvBatchFuture<'a, T> {
  pub(crate) fn new(shared: &'a Arc<Shared<T>>, max: usize) -> Self {
    RecvBatchFuture {
      shared,
      max,
      my_id: None,
    }
  }

  #[inline]
  fn unregister(&mut self) {
    if let Some(id) = self.my_id.take() {
      self.shared.unregister(Role::Recv, id);
    }
  }

  fn drain(&self) -> Vec<T> {
    let mut out = Vec::new();
    while out.len() < self.max {
      match self.shared.ring.pop() {
        Some(item) => {
          out.push(item);
          self.shared.notify_senders();
        }
        None => break,
      }
    }
    out
  }
}

impl<'a, T: Send> Future for RecvBatchFuture<'a, T> {
  type Output = Result<Vec<T>, RecvError>;

  fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    let this = self.get_mut();

    if this.max == 0 {
      this.unregister();
      return Poll::Ready(Ok(Vec::new()));
    }

    let out = this.drain();
    if !out.is_empty() {
      this.unregister();
      return Poll::Ready(Ok(out));
    }
    if !this.shared.senders_alive() {
      let out = this.drain();
      this.unregister();
      return if out.is_empty() {
        Poll::Ready(Err(RecvError::Disconnected))
      } else {
        Poll::Ready(Ok(out))
      };
    }

    let id = this
      .shared
      .register(Role::Recv, this.my_id, WakeRef::Waker(cx.waker().clone()), std::ptr::null());
    this.my_id = Some(id);
    this.shared.pre_park_fence();

    let out = this.drain();
    if !out.is_empty() {
      this.unregister();
      return Poll::Ready(Ok(out));
    }
    if !this.shared.senders_alive() {
      let out = this.drain();
      this.unregister();
      return if out.is_empty() {
        Poll::Ready(Err(RecvError::Disconnected))
      } else {
        Poll::Ready(Ok(out))
      };
    }
    Poll::Pending
  }
}

impl<'a, T: Send> Drop for RecvBatchFuture<'a, T> {
  fn drop(&mut self) {
    self.unregister();
  }
}

// --- Batch recv (in place, cancel-safe) -----------------------------------

pub struct RecvBatchMutFuture<'a, T: Send> {
  shared: &'a Arc<Shared<T>>,
  out: &'a mut Vec<T>,
  max: usize,
  my_id: Option<u64>,
}

impl<'a, T: Send> RecvBatchMutFuture<'a, T> {
  pub(crate) fn new(shared: &'a Arc<Shared<T>>, out: &'a mut Vec<T>, max: usize) -> Self {
    RecvBatchMutFuture {
      shared,
      out,
      max,
      my_id: None,
    }
  }

  #[inline]
  fn unregister(&mut self) {
    if let Some(id) = self.my_id.take() {
      self.shared.unregister(Role::Recv, id);
    }
  }

  /// Drains up to `max` into `out`, returning how many were appended this call.
  fn drain(&mut self) -> usize {
    let mut got = 0;
    while got < self.max {
      match self.shared.ring.pop() {
        Some(item) => {
          self.out.push(item);
          got += 1;
          self.shared.notify_senders();
        }
        None => break,
      }
    }
    got
  }
}

impl<'a, T: Send> Future for RecvBatchMutFuture<'a, T> {
  type Output = Result<usize, RecvError>;

  fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    let this = self.get_mut();

    if this.max == 0 {
      this.unregister();
      return Poll::Ready(Ok(0));
    }

    let got = this.drain();
    if got > 0 {
      this.unregister();
      return Poll::Ready(Ok(got));
    }
    if !this.shared.senders_alive() {
      let got = this.drain();
      this.unregister();
      return if got > 0 {
        Poll::Ready(Ok(got))
      } else {
        Poll::Ready(Err(RecvError::Disconnected))
      };
    }

    let id = this
      .shared
      .register(Role::Recv, this.my_id, WakeRef::Waker(cx.waker().clone()), std::ptr::null());
    this.my_id = Some(id);
    this.shared.pre_park_fence();

    let got = this.drain();
    if got > 0 {
      this.unregister();
      return Poll::Ready(Ok(got));
    }
    if !this.shared.senders_alive() {
      let got = this.drain();
      this.unregister();
      return if got > 0 {
        Poll::Ready(Ok(got))
      } else {
        Poll::Ready(Err(RecvError::Disconnected))
      };
    }
    Poll::Pending
  }
}

impl<'a, T: Send> Drop for RecvBatchMutFuture<'a, T> {
  fn drop(&mut self) {
    self.unregister();
  }
}

// --- Stream ---------------------------------------------------------------

/// `Stream::poll_next` body for `AsyncReceiver`, factored out of the public
/// handle so the backend owns the register / recheck / park dance. The caller
/// handles its own `closed` short-circuit before calling this.
pub(crate) fn poll_stream_next<T: Send>(
  shared: &Arc<Shared<T>>,
  stream_id: &mut Option<u64>,
  cx: &mut Context<'_>,
) -> Poll<Option<T>> {
  if let Some(item) = shared.ring.pop() {
    if let Some(id) = stream_id.take() {
      shared.unregister_recv(id);
    }
    shared.notify_senders();
    return Poll::Ready(Some(item));
  }
  if !shared.senders_alive() {
    if let Some(item) = shared.ring.pop() {
      if let Some(id) = stream_id.take() {
        shared.unregister_recv(id);
      }
      shared.notify_senders();
      return Poll::Ready(Some(item));
    }
    if let Some(id) = stream_id.take() {
      shared.unregister_recv(id);
    }
    return Poll::Ready(None);
  }

  // Empty: register/refresh waker, fence, then recheck with the real op.
  let id = shared.register(
    Role::Recv,
    *stream_id,
    WakeRef::Waker(cx.waker().clone()),
    std::ptr::null(),
  );
  *stream_id = Some(id);
  shared.pre_park_fence();

  if let Some(item) = shared.ring.pop() {
    shared.unregister_recv(id);
    *stream_id = None;
    shared.notify_senders();
    return Poll::Ready(Some(item));
  }
  if !shared.senders_alive() {
    shared.unregister_recv(id);
    *stream_id = None;
    if let Some(item) = shared.ring.pop() {
      shared.notify_senders();
      return Poll::Ready(Some(item));
    }
    return Poll::Ready(None);
  }
  Poll::Pending
}
