//! Producer handles for `bounded_v3`: `Sender` (sync) and `AsyncSender`, plus
//! their send futures. The send core is the credit-before-claim
//! (`Shared::try_send_now` + the ticketless waiter registry); a parked producer
//! holds NO ticket, so cancellation is clean by construction - there is no
//! ticket-holding Drop path to wedge or lose anything.

use std::fmt;
use std::marker::PhantomPinned;
use std::pin::Pin;
use std::task::{Context, Poll};

use crate::internal::sync::{fence, thread, Arc, AtomicBool, Ordering};

use crate::error::{
  BatchSendErrorReason, CloseError, SendBatchError, SendError, TrySendBatchError, TrySendError,
};
use crate::sync_util;

use super::shared::{spin_before_park_cap1, Shared, SYNC_SPIN_LIMIT};

pub struct Sender<T: Send> {
  pub(crate) shared: Arc<Shared<T>>,
  pub(crate) closed: AtomicBool,
}

pub struct AsyncSender<T: Send> {
  pub(crate) shared: Arc<Shared<T>>,
  pub(crate) closed: AtomicBool,
}

impl<T: Send> fmt::Debug for Sender<T> {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("Sender")
      .field("capacity", &self.capacity())
      .field("len", &self.len())
      .field("closed", &self.closed.load(Ordering::Relaxed))
      .finish()
  }
}

impl<T: Send> fmt::Debug for AsyncSender<T> {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("AsyncSender")
      .field("capacity", &self.capacity())
      .field("len", &self.len())
      .field("closed", &self.closed.load(Ordering::Relaxed))
      .finish()
  }
}

unsafe impl<T: Send> Send for Sender<T> {}
unsafe impl<T: Send> Send for AsyncSender<T> {}
unsafe impl<T: Send> Sync for AsyncSender<T> {}

impl<T: Send> Sender<T> {
  /// Blocking send: try the credit window,
  /// then a small-cap pre-register spin, then register-fence-recheck-park. A
  /// parked producer holds no ticket, so the wait can be abandoned freely.
  pub fn send(&self, item: T) -> Result<(), SendError> {
    self.send_inner(item).map_err(|_| SendError::Closed)
  }

  /// Blocking send that returns the item back on `Err` (channel closed) so
  /// batch callers never drop an un-delivered value. `send` is a thin wrapper.
  fn send_inner(&self, item: T) -> Result<(), T> {
    if self.closed.load(Ordering::Relaxed) || !self.shared.receivers_alive() {
      return Err(item);
    }

    let mut item = match self.shared.try_send_now(item) {
      Ok(()) => return Ok(()),
      Err(v) => v,
    };

    // Small-cap pre-register spin (cap<=4 probe economics).
    if self.shared.cap() <= 4 {
      for _ in 0..SYNC_SPIN_LIMIT {
        thread::yield_now();
        item = match self.shared.try_send_now(item) {
          Ok(()) => return Ok(()),
          Err(v) => v,
        };
      }
    }

    let mut is_registered = false;
    let mut my_id = None;
    let notified = AtomicBool::new(false);
    let notified_ptr = &notified as *const AtomicBool;

    loop {
      if self.closed.load(Ordering::Relaxed) || !self.shared.receivers_alive() {
        if let Some(id) = my_id {
          self.shared.finish_sync_send(id, &notified);
        }
        return Err(item); // no claim held - nothing to tombstone
      }

      item = match self.shared.try_send_now(item) {
        Ok(()) => {
          if let Some(id) = my_id {
            self.shared.finish_sync_send(id, &notified);
          }
          return Ok(());
        }
        Err(v) => v,
      };

      if is_registered {
        spin_before_park_cap1(self.shared.cap(), &notified);
        if !notified.load(Ordering::Relaxed) {
          sync_util::park_thread();
        }
        if notified.swap(false, Ordering::Acquire) {
          is_registered = false;
          my_id = None;
        }
        continue;
      }

      let id = self
        .shared
        .register_sync_send(thread::current(), notified_ptr);
      is_registered = true;
      my_id = Some(id);
      fence(Ordering::SeqCst);
    }
  }

  pub fn try_send(&self, item: T) -> Result<(), TrySendError<T>> {
    if self.closed.load(Ordering::Relaxed) || !self.shared.receivers_alive() {
      return Err(TrySendError::Closed(item));
    }
    match self.shared.try_send_now(item) {
      Ok(()) => Ok(()),
      Err(v) => Err(TrySendError::Full(v)),
    }
  }

  // --- batch send: run-claim ---

  /// Block (ticketless) until the send window looks open again, `Ok(())`, or the
  /// channel closes, `Err(())`. A FRESH `notified` per call, finished on the way
  /// out - never reused across a `finish_sync_send` (bounded_queue's
  /// `allocate_node` lifetime discipline; reusing one `notified` across the whole
  /// batch loop is a stack-lifetime UAF the miri suite caught).
  fn wait_for_window(&self) -> Result<(), ()> {
    let mut is_registered = false;
    let mut my_id: Option<u64> = None;
    let notified = AtomicBool::new(false);
    let notified_ptr = &notified as *const AtomicBool;

    loop {
      if self.closed.load(Ordering::Relaxed) || !self.shared.receivers_alive() {
        if let Some(id) = my_id {
          self.shared.finish_sync_send(id, &notified);
        }
        return Err(());
      }
      if self.shared.window_open() {
        if let Some(id) = my_id {
          self.shared.finish_sync_send(id, &notified);
        }
        return Ok(());
      }
      if is_registered {
        spin_before_park_cap1(self.shared.cap(), &notified);
        if !notified.load(Ordering::Relaxed) {
          sync_util::park_thread();
        }
        if notified.swap(false, Ordering::Acquire) {
          is_registered = false;
          my_id = None;
        }
        continue;
      }
      let id = self
        .shared
        .register_sync_send(thread::current(), notified_ptr);
      is_registered = true;
      my_id = Some(id);
      fence(Ordering::SeqCst);
    }
  }

  /// Blocking batch send via contiguous ticket-run claims: each attempt claims
  /// `min(remaining, window, K)`, fills it in one ascending pass with a single
  /// receiver wake, and blocks in `wait_for_window` when the window is closed.
  pub fn send_batch(&self, items: Vec<T>) -> Result<usize, SendBatchError<T>> {
    let total = items.len();
    if total == 0 {
      return Ok(0);
    }
    if self.closed.load(Ordering::Relaxed) || !self.shared.receivers_alive() {
      return Err(SendBatchError {
        sent: 0,
        unsent: items,
      });
    }

    let mut iter = items.into_iter();
    let mut sent = 0;

    while sent < total {
      if self.closed.load(Ordering::Relaxed) || !self.shared.receivers_alive() {
        return Err(SendBatchError {
          sent,
          unsent: iter.collect(),
        });
      }
      let (t, valid, m) = self.shared.claim_run(total - sent);
      if m > 0 {
        self
          .shared
          .resolve_run(t, valid, m, &mut iter.by_ref().take(valid));
        sent += valid;
        continue;
      }
      // Window closed - block until it reopens (fresh notified, finish-on-exit).
      if self.wait_for_window().is_err() {
        return Err(SendBatchError {
          sent,
          unsent: iter.collect(),
        });
      }
    }
    Ok(total)
  }

  pub fn try_send_batch(&self, items: Vec<T>) -> Result<usize, TrySendBatchError<T>> {
    if items.is_empty() {
      return Ok(0);
    }
    if self.closed.load(Ordering::Relaxed) || !self.shared.receivers_alive() {
      return Err(TrySendBatchError {
        sent: 0,
        unsent: items,
        reason: BatchSendErrorReason::Closed,
      });
    }
    try_send_run_batch(&self.shared, items)
  }

  pub fn send_batch_mut(&self, items: &mut Vec<T>) -> Result<usize, SendError> {
    if items.is_empty() {
      return Ok(0);
    }
    if self.closed.load(Ordering::Relaxed) || !self.shared.receivers_alive() {
      return Err(SendError::Closed);
    }

    // Drain in place so the caller keeps the buffer's allocation (unlike a
    // `mem::take`, which hands it off and leaves an empty zero-capacity Vec).
    // One `drain(..)` held for the whole call: `by_ref().take(valid)` advances
    // its cursor without shifting per run, and on drop only the unconsumed tail
    // moves back into `items` (empty on full success) — capacity retained either
    // way, and a mid-batch close leaves exactly the untried tail in `items`.
    let total = items.len();
    let mut drain = items.drain(..);
    let mut sent = 0;
    while sent < total {
      if self.closed.load(Ordering::Relaxed) || !self.shared.receivers_alive() {
        break;
      }
      let (t, valid, m) = self.shared.claim_run(total - sent);
      if m > 0 {
        self
          .shared
          .resolve_run(t, valid, m, &mut drain.by_ref().take(valid));
        sent += valid;
        continue;
      }
      // Window closed - block until it reopens.
      if self.wait_for_window().is_err() {
        break;
      }
    }
    if sent == total {
      Ok(sent)
    } else {
      Err(SendError::Closed)
    }
  }

  pub fn try_send_batch_mut(&self, items: &mut Vec<T>) -> Result<usize, SendError> {
    if items.is_empty() {
      return Ok(0);
    }
    let batch = std::mem::take(items);
    match self.try_send_batch(batch) {
      Ok(n) => Ok(n),
      Err(e) => {
        let (sent, reason) = (e.sent, e.reason);
        *items = e.unsent;
        if sent == 0 && matches!(reason, BatchSendErrorReason::Closed) {
          Err(SendError::Closed)
        } else {
          Ok(sent)
        }
      }
    }
  }

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

  pub fn is_closed(&self) -> bool {
    self.closed.load(Ordering::Relaxed) || !self.shared.receivers_alive()
  }

  pub fn capacity(&self) -> usize {
    self.shared.capacity()
  }

  pub fn len(&self) -> usize {
    self.shared.len()
  }

  pub fn is_empty(&self) -> bool {
    self.shared.is_empty()
  }

  pub fn is_full(&self) -> bool {
    self.shared.is_full()
  }

  pub fn to_async(self) -> AsyncSender<T> {
    let shared = unsafe { std::ptr::read(&self.shared) };
    let closed = self.closed.load(Ordering::Relaxed);
    std::mem::forget(self);
    AsyncSender {
      shared,
      closed: AtomicBool::new(closed),
    }
  }
}

impl<T: Send> Clone for Sender<T> {
  fn clone(&self) -> Self {
    self.shared.add_sender();
    Sender {
      shared: Arc::clone(&self.shared),
      closed: AtomicBool::new(false),
    }
  }
}

impl<T: Send> Drop for Sender<T> {
  fn drop(&mut self) {
    let _ = self.close();
  }
}

impl<T: Send> AsyncSender<T> {
  pub fn send(&self, item: T) -> SendFuture<'_, T> {
    SendFuture::new(self, item)
  }

  pub fn try_send(&self, item: T) -> Result<(), TrySendError<T>> {
    if self.closed.load(Ordering::Relaxed) || !self.shared.receivers_alive() {
      return Err(TrySendError::Closed(item));
    }
    match self.shared.try_send_now(item) {
      Ok(()) => Ok(()),
      Err(v) => Err(TrySendError::Full(v)),
    }
  }

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

  pub fn is_closed(&self) -> bool {
    self.closed.load(Ordering::Relaxed) || !self.shared.receivers_alive()
  }

  pub fn capacity(&self) -> usize {
    self.shared.capacity()
  }

  pub fn len(&self) -> usize {
    self.shared.len()
  }

  pub fn is_empty(&self) -> bool {
    self.shared.is_empty()
  }

  pub fn is_full(&self) -> bool {
    self.shared.is_full()
  }

  pub fn to_sync(self) -> Sender<T> {
    let shared = unsafe { std::ptr::read(&self.shared) };
    let closed = self.closed.load(Ordering::Relaxed);
    std::mem::forget(self);
    Sender {
      shared,
      closed: AtomicBool::new(closed),
    }
  }

  pub fn try_send_batch(&self, items: Vec<T>) -> Result<usize, TrySendBatchError<T>> {
    if items.is_empty() {
      return Ok(0);
    }
    if self.closed.load(Ordering::Relaxed) || !self.shared.receivers_alive() {
      return Err(TrySendBatchError {
        sent: 0,
        unsent: items,
        reason: BatchSendErrorReason::Closed,
      });
    }
    try_send_run_batch(&self.shared, items)
  }

  pub fn try_send_batch_mut(&self, items: &mut Vec<T>) -> Result<usize, SendError> {
    if items.is_empty() {
      return Ok(0);
    }
    let batch = std::mem::take(items);
    match self.try_send_batch(batch) {
      Ok(n) => Ok(n),
      Err(e) => {
        let (sent, reason) = (e.sent, e.reason);
        *items = e.unsent;
        if sent == 0 && matches!(reason, BatchSendErrorReason::Closed) {
          Err(SendError::Closed)
        } else {
          Ok(sent)
        }
      }
    }
  }

  pub fn send_batch(&self, items: Vec<T>) -> BoundedSendBatchFuture<'_, T> {
    let total = items.len();
    BoundedSendBatchFuture {
      sender: self,
      iter: items.into_iter(),
      total,
      sent: 0,
      my_id: None,
      _phantom: PhantomPinned,
    }
  }

  pub fn send_batch_mut<'a>(&'a self, items: &'a mut Vec<T>) -> BoundedSendBatchMutFuture<'a, T> {
    BoundedSendBatchMutFuture {
      sender: self,
      items,
      sent: 0,
      my_id: None,
      _phantom: PhantomPinned,
    }
  }
}

impl<T: Send> Clone for AsyncSender<T> {
  fn clone(&self) -> Self {
    self.shared.add_sender();
    AsyncSender {
      shared: Arc::clone(&self.shared),
      closed: AtomicBool::new(false),
    }
  }
}

impl<T: Send> Drop for AsyncSender<T> {
  fn drop(&mut self) {
    let _ = self.close();
  }
}

// --- Futures ---

/// Cancel-safe by construction: a Pending send holds no ticket, so dropping it
/// only has to clean up the waiter registration.
#[must_use = "futures do nothing unless you .await or poll them"]
pub struct SendFuture<'a, T: Send> {
  sender: &'a AsyncSender<T>,
  item: Option<T>,
  my_id: Option<u64>,
  _phantom: PhantomPinned,
}

impl<'a, T: Send> SendFuture<'a, T> {
  fn new(sender: &'a AsyncSender<T>, item: T) -> Self {
    SendFuture {
      sender,
      item: Some(item),
      my_id: None,
      _phantom: PhantomPinned,
    }
  }
}

impl<'a, T: Send> Future for SendFuture<'a, T> {
  type Output = Result<(), SendError>;

  fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    let this = unsafe { self.as_mut().get_unchecked_mut() };
    let shared = &this.sender.shared;

    loop {
      if this.sender.closed.load(Ordering::Relaxed) || !shared.receivers_alive() {
        if let Some(id) = this.my_id.take() {
          shared.unregister_async_send(id);
        }
        return Poll::Ready(Err(SendError::Closed));
      }

      let item = match this.item.take() {
        Some(it) => it,
        None => return Poll::Ready(Ok(())),
      };
      match shared.try_send_now(item) {
        Ok(()) => {
          if let Some(id) = this.my_id.take() {
            shared.unregister_async_send(id);
          }
          return Poll::Ready(Ok(()));
        }
        Err(v) => this.item = Some(v),
      }

      let id = shared.register_async_send(this.my_id, cx.waker().clone());
      this.my_id = Some(id);
      fence(Ordering::SeqCst);

      if shared.window_open() || !shared.receivers_alive() {
        continue; // won the race after registering - resolve above
      }
      return Poll::Pending;
    }
  }
}

impl<'a, T: Send> Drop for SendFuture<'a, T> {
  fn drop(&mut self) {
    if let Some(id) = self.my_id.take() {
      self.sender.shared.unregister_async_send(id);
    }
  }
}

// --- Async batch futures: run-claim ---

#[must_use = "futures do nothing unless you .await or poll them"]
pub struct BoundedSendBatchFuture<'a, T: Send> {
  sender: &'a AsyncSender<T>,
  iter: std::vec::IntoIter<T>,
  total: usize,
  sent: usize,
  my_id: Option<u64>,
  _phantom: PhantomPinned,
}

impl<'a, T: Send> Future for BoundedSendBatchFuture<'a, T> {
  type Output = Result<usize, SendBatchError<T>>;

  fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    let this = unsafe { self.as_mut().get_unchecked_mut() };
    let shared = &this.sender.shared;

    loop {
      if this.sent == this.total {
        if let Some(id) = this.my_id.take() {
          shared.unregister_async_send(id);
        }
        return Poll::Ready(Ok(this.total));
      }

      if this.sender.closed.load(Ordering::Relaxed) || !shared.receivers_alive() {
        if let Some(id) = this.my_id.take() {
          shared.unregister_async_send(id);
        }
        return Poll::Ready(Err(SendBatchError {
          sent: this.sent,
          unsent: this.iter.by_ref().collect(),
        }));
      }

      let (t, valid, m) = shared.claim_run(this.total - this.sent);
      if m > 0 {
        shared.resolve_run(t, valid, m, &mut this.iter.by_ref().take(valid));
        this.sent += valid;
        if let Some(id) = this.my_id.take() {
          shared.unregister_async_send(id);
        }
        continue;
      }

      let id = shared.register_async_send(this.my_id, cx.waker().clone());
      this.my_id = Some(id);
      fence(Ordering::SeqCst);

      if shared.window_open() || !shared.receivers_alive() {
        continue;
      }
      return Poll::Pending;
    }
  }
}

impl<'a, T: Send> Drop for BoundedSendBatchFuture<'a, T> {
  fn drop(&mut self) {
    if let Some(id) = self.my_id.take() {
      self.sender.shared.unregister_async_send(id);
    }
  }
}

#[must_use = "futures do nothing unless you .await or poll them"]
pub struct BoundedSendBatchMutFuture<'a, T: Send> {
  sender: &'a AsyncSender<T>,
  items: &'a mut Vec<T>,
  sent: usize,
  my_id: Option<u64>,
  _phantom: PhantomPinned,
}

impl<'a, T: Send> Future for BoundedSendBatchMutFuture<'a, T> {
  type Output = Result<usize, SendError>;

  fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    let this = unsafe { self.as_mut().get_unchecked_mut() };
    let shared = &this.sender.shared;

    loop {
      if this.items.is_empty() {
        if let Some(id) = this.my_id.take() {
          shared.unregister_async_send(id);
        }
        return Poll::Ready(Ok(this.sent));
      }

      if this.sender.closed.load(Ordering::Relaxed) || !shared.receivers_alive() {
        if let Some(id) = this.my_id.take() {
          shared.unregister_async_send(id);
        }
        return Poll::Ready(Err(SendError::Closed));
      }

      let remaining = this.items.len();
      let (t, valid, m) = shared.claim_run(remaining);
      if m > 0 {
        {
          let mut drain = this.items.drain(..valid);
          shared.resolve_run(t, valid, m, &mut drain);
        }
        this.sent += valid;
        if let Some(id) = this.my_id.take() {
          shared.unregister_async_send(id);
        }
        continue;
      }

      let id = shared.register_async_send(this.my_id, cx.waker().clone());
      this.my_id = Some(id);
      fence(Ordering::SeqCst);

      if shared.window_open() || !shared.receivers_alive() {
        continue;
      }
      return Poll::Pending;
    }
  }
}

impl<'a, T: Send> Drop for BoundedSendBatchMutFuture<'a, T> {
  fn drop(&mut self) {
    if let Some(id) = self.my_id.take() {
      self.sender.shared.unregister_async_send(id);
    }
  }
}

/// Non-blocking batch send via run-claims (shared by sync + async `try_send_batch`).
/// Callers must have already handled the up-front closed check. Stops as `Full`
/// the moment the window closes (or overshoots), returning the untouched tail.
fn try_send_run_batch<T: Send>(
  shared: &Shared<T>,
  items: Vec<T>,
) -> Result<usize, TrySendBatchError<T>> {
  let total = items.len();
  let mut iter = items.into_iter();
  let mut sent = 0;
  loop {
    if sent == total {
      return Ok(total);
    }
    if !shared.receivers_alive() {
      return Err(TrySendBatchError {
        sent,
        unsent: iter.collect(),
        reason: BatchSendErrorReason::Closed,
      });
    }
    let (t, valid, m) = shared.claim_run(total - sent);
    if m == 0 {
      return Err(TrySendBatchError {
        sent,
        unsent: iter.collect(),
        reason: BatchSendErrorReason::Full,
      });
    }
    shared.resolve_run(t, valid, m, &mut iter.by_ref().take(valid));
    sent += valid;
    if valid < m {
      // Overshoot tombstoned the rest of the claim; the window is closed for us.
      if sent == total {
        return Ok(total);
      }
      return Err(TrySendBatchError {
        sent,
        unsent: iter.collect(),
        reason: BatchSendErrorReason::Full,
      });
    }
  }
}
