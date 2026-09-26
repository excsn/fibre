//! Producer handles for `bounded_v3`: `Sender` (sync) and `AsyncSender`, plus
//! their send futures. The send core is the credit-before-claim
//! (`Shared::try_send_now` + the ticketless waiter registry); a parked producer
//! holds NO ticket, so cancellation is clean by construction - there is no
//! ticket-holding Drop path to wedge or lose anything.

use std::fmt;
use std::marker::PhantomPinned;
use std::pin::Pin;
use std::task::{Context, Poll};

use crate::internal::sync::{fence, hint, thread, Arc, AtomicBool, Ordering};

use crate::error::{
  BatchSendErrorReason, CloseError, SendBatchError, SendError, TrySendBatchError, TrySendError,
};
use crate::sync_util;

use super::shared::{spin_before_park_cap1, Shared, Step, ELECT, SYNC_SPIN_LIMIT};

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
    let mut token = false;
    let mut cold_recheck = false;
    let mut spins = 0u32;
    let mut last = u32::MAX;
    let mut bo = 0u32;

    loop {
      if self.closed.load(Ordering::Relaxed) || !self.shared.receivers_alive() {
        self.leave(my_id, &notified, token);
        return Err(item); // no claim held - nothing to tombstone
      }

      item = match self.shared.try_send_now(item) {
        Ok(()) => {
          self.leave(my_id, &notified, token);
          return Ok(());
        }
        Err(v) => v,
      };

      if cold_recheck {
        item = match self.shared.try_send_now_cold(item) {
          Ok(()) => {
            self.leave(my_id, &notified, token);
            return Ok(());
          }
          Err(v) => v,
        };
      }

      if ELECT && !token && !is_registered && self.shared.awake_acquire() {
        token = true;
        spins = 0;
        last = u32::MAX;
        bo = 0;
      }

      if token {
        match self.shared.awake_step(item, &mut spins, &mut last) {
          Step::Sent => {
            self.leave(my_id, &notified, token);
            return Ok(());
          }
          Step::Yield(v) => {
            item = v;
            let n = 1usize;
            spins += n as u32;
            for _ in 0..n {
              if self.shared.cap() <= 4 {
                hint::spin_loop();
              } else {
                thread::yield_now();
              }
            }
            continue;
          }
          Step::Stalled(v) => {
            item = v;
            token = false;
            self.shared.awake_release();
            cold_recheck = true;
          }
        }
      }

      if is_registered {
        spin_before_park_cap1(self.shared.cap(), &notified);
        cold_recheck = false;
        if !notified.load(Ordering::Relaxed) {
          sync_util::park_thread();
        }
        if notified.swap(false, Ordering::Acquire) {
          is_registered = false;
          my_id = None;
          token = true;
          spins = 0;
          last = u32::MAX;
          bo = 0;
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

  /// Leave the wait: settle the registration and give back the awake count
  /// this sender holds (a token, or a wake the consumer counted).
  #[inline]
  fn leave(&self, my_id: Option<u64>, notified: &AtomicBool, token: bool) {
    if let Some(id) = my_id {
      self.shared.finish_sync_send(id, notified);
      if notified.load(Ordering::Acquire) {
        self.shared.awake_release();
      }
    }
    if token {
      self.shared.awake_release();
    }
  }

  pub fn try_send(&self, item: T) -> Result<(), TrySendError<T>> {
    if self.closed.load(Ordering::Relaxed) || !self.shared.receivers_alive() {
      return Err(TrySendError::Closed(item));
    }
    match self.shared.try_send_now(item) {
      Ok(()) => Ok(()),
      // The hot window is up-to-K stale; verify Full against the fresh
      // split counter before reporting it.
      Err(v) => match self.shared.try_send_now_cold(v) {
        Ok(()) => Ok(()),
        Err(v) => Err(TrySendError::Full(v)),
      },
    }
  }

  // --- batch send: run-claim ---

  /// Block (ticketless) until the send window looks open again, `Ok(())`, or the
  /// channel closes, `Err(())`. A FRESH `notified` per call, finished on the way
  /// out - never reused across a `finish_sync_send` (bounded_queue's
  /// `allocate_node` lifetime discipline; reusing one `notified` across the whole
  /// batch loop is a stack-lifetime UAF the miri suite caught).
  fn wait_for_window(&self) -> Result<bool, ()> {
    let mut held = false;
    let mut is_registered = false;
    let mut my_id: Option<u64> = None;
    let notified = AtomicBool::new(false);
    let notified_ptr = &notified as *const AtomicBool;

    loop {
      if self.closed.load(Ordering::Relaxed) || !self.shared.receivers_alive() {
        self.leave(my_id, &notified, false);
        if held {
          self.shared.awake_release();
        }
        return Err(());
      }
      if !self.shared.window_open() && self.shared.window_open_cold() {
        self.shared.publish_from_drained();
      }
      if self.shared.window_open() {
        self.leave(my_id, &notified, false);
        return Ok(held);
      }
      if held {
        self.shared.awake_release();
        held = false;
      }
      if is_registered {
        spin_before_park_cap1(self.shared.cap(), &notified);
        if !notified.load(Ordering::Relaxed) {
          sync_util::park_thread();
        }
        if notified.swap(false, Ordering::Acquire) {
          is_registered = false;
          my_id = None;
          held = true;
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
    let mut held = false;

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
        if held {
          self.shared.awake_release();
          held = false;
        }
        continue;
      }
      if held {
        self.shared.awake_release();
        held = false;
      }
      // Window closed - block until it reopens (fresh notified, finish-on-exit).
      let waited = self.wait_for_window();
      if let Ok(h) = waited {
        held = h;
      }
      if waited.is_err() {
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

    // Drain in place so on success the caller keeps the buffer's allocation
    // (unlike a `mem::take`, which hands it off and leaves an empty
    // zero-capacity Vec). One `drain(..)` held for the whole call:
    // `by_ref().take(valid)` advances its cursor without shifting per run.
    // Drain's Drop destroys any unconsumed elements, so a mid-batch close must
    // collect the untried tail back into `items` before returning.
    let total = items.len();
    let mut drain = items.drain(..);
    let mut sent = 0;
    let mut held = false;
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
        if held {
          self.shared.awake_release();
          held = false;
        }
        continue;
      }
      if held {
        self.shared.awake_release();
        held = false;
      }
      // Window closed - block until it reopens.
      let waited = self.wait_for_window();
      if let Ok(h) = waited {
        held = h;
      }
      if waited.is_err() {
        break;
      }
    }
    if sent == total {
      Ok(sent)
    } else {
      let rest: Vec<T> = drain.collect();
      *items = rest;
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
      // The hot window is up-to-K stale; verify Full against the fresh
      // split counter before reporting it.
      Err(v) => match self.shared.try_send_now_cold(v) {
        Ok(()) => Ok(()),
        Err(v) => Err(TrySendError::Full(v)),
      },
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
      notified: AtomicBool::new(false),
      woken: false,
      _phantom: PhantomPinned,
    }
  }

  pub fn send_batch_mut<'a>(&'a self, items: &'a mut Vec<T>) -> BoundedSendBatchMutFuture<'a, T> {
    BoundedSendBatchMutFuture {
      sender: self,
      items,
      sent: 0,
      my_id: None,
      notified: AtomicBool::new(false),
      woken: false,
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
/// only has to clean up the waiter registration and the awake count.
#[must_use = "futures do nothing unless you .await or poll them"]
pub struct SendFuture<'a, T: Send> {
  sender: &'a AsyncSender<T>,
  item: Option<T>,
  my_id: Option<u64>,
  notified: AtomicBool,
  _phantom: PhantomPinned,
}

impl<'a, T: Send> SendFuture<'a, T> {
  fn new(sender: &'a AsyncSender<T>, item: T) -> Self {
    SendFuture {
      sender,
      item: Some(item),
      my_id: None,
      notified: AtomicBool::new(false),
      _phantom: PhantomPinned,
    }
  }

  #[inline(always)]
  fn leave(&mut self) {
    if self.my_id.is_some() {
      self.leave_slow();
    }
  }

  #[inline(never)]
  fn leave_slow(&mut self) {
    let sender = self.sender;
    let shared = &sender.shared;
    if let Some(id) = self.my_id.take() {
      if !shared.unregister_async_send(id) {
        while !self.notified.load(Ordering::Acquire) {
          hint::spin_loop();
        }
        self.notified.store(false, Ordering::Relaxed);
        shared.awake_release();
      }
    }
  }
}

impl<'a, T: Send> Future for SendFuture<'a, T> {
  type Output = Result<(), SendError>;

  fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    let this = unsafe { self.as_mut().get_unchecked_mut() };
    let sender = this.sender;
    let shared = &sender.shared;
    let notified_ptr = &this.notified as *const AtomicBool;

    let mut woken = this.my_id.is_some() && this.notified.swap(false, Ordering::Acquire);
    if woken {
      this.my_id = None;
    }

    loop {
      if sender.closed.load(Ordering::Relaxed) || !shared.receivers_alive() {
        if woken {
          shared.awake_release();
        }
        this.leave();
        return Poll::Ready(Err(SendError::Closed));
      }

      let item = match this.item.take() {
        Some(it) => it,
        None => return Poll::Ready(Ok(())),
      };
      let item = match shared.try_send_now(item) {
        Ok(()) => Ok(()),
        Err(v) if woken || shared.cold_on_miss() => shared.try_send_now_cold(v),
        Err(v) => Err(v),
      };
      match item {
        Ok(()) => {
          if woken {
            shared.awake_release();
          }
          this.leave();
          return Poll::Ready(Ok(()));
        }
        Err(v) => this.item = Some(v),
      }
      if woken {
        woken = false;
        shared.awake_release();
      }

      match shared.register_async_send(this.my_id, cx.waker().clone(), notified_ptr) {
        Some(id) => this.my_id = Some(id),
        None => {
          while !this.notified.load(Ordering::Acquire) {
            hint::spin_loop();
          }
          this.notified.store(false, Ordering::Relaxed);
          this.my_id = None;
          woken = true;
          continue;
        }
      }
      fence(Ordering::SeqCst);

      if shared.window_open()
        || (shared.cold_on_miss() && shared.window_open_cold())
        || !shared.receivers_alive()
      {
        continue;
      }
      return Poll::Pending;
    }
  }
}

impl<'a, T: Send> Drop for SendFuture<'a, T> {
  fn drop(&mut self) {
    self.leave();
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
  notified: AtomicBool,
  woken: bool,
  _phantom: PhantomPinned,
}

impl<'a, T: Send> Future for BoundedSendBatchFuture<'a, T> {
  type Output = Result<usize, SendBatchError<T>>;

  fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    let this = unsafe { self.as_mut().get_unchecked_mut() };
    let shared = &this.sender.shared;
    if this.my_id.is_some() && this.notified.swap(false, Ordering::Acquire) {
      this.my_id = None;
      this.woken = true;
    }

    loop {
      if this.sent == this.total {
        if let Some(id) = this.my_id.take() {
          if !shared.unregister_async_send(id) {
            while !this.notified.load(Ordering::Acquire) {
              hint::spin_loop();
            }
            this.notified.store(false, Ordering::Relaxed);
            shared.awake_release();
          }
        }
        if this.woken {
          this.woken = false;
          shared.awake_release();
        }
        return Poll::Ready(Ok(this.total));
      }

      if this.sender.closed.load(Ordering::Relaxed) || !shared.receivers_alive() {
        if let Some(id) = this.my_id.take() {
          if !shared.unregister_async_send(id) {
            while !this.notified.load(Ordering::Acquire) {
              hint::spin_loop();
            }
            this.notified.store(false, Ordering::Relaxed);
            shared.awake_release();
          }
        }
        if this.woken {
          this.woken = false;
          shared.awake_release();
        }
        return Poll::Ready(Err(SendBatchError {
          sent: this.sent,
          unsent: this.iter.by_ref().collect(),
        }));
      }

      if !shared.window_open() && shared.window_open_cold() {
        shared.publish_from_drained();
      }
      let (t, valid, m) = shared.claim_run(this.total - this.sent);
      if m > 0 {
        shared.resolve_run(t, valid, m, &mut this.iter.by_ref().take(valid));
        this.sent += valid;
        if let Some(id) = this.my_id.take() {
          if !shared.unregister_async_send(id) {
            while !this.notified.load(Ordering::Acquire) {
              hint::spin_loop();
            }
            this.notified.store(false, Ordering::Relaxed);
            shared.awake_release();
          }
        }
        if this.woken {
          this.woken = false;
          shared.awake_release();
        }
        continue;
      }

      if this.woken {
        this.woken = false;
        shared.awake_release();
      }
      match shared.register_async_send(this.my_id, cx.waker().clone(), &this.notified as *const AtomicBool) {
        Some(id) => this.my_id = Some(id),
        None => {
          while !this.notified.load(Ordering::Acquire) {
            hint::spin_loop();
          }
          this.notified.store(false, Ordering::Relaxed);
          this.my_id = None;
          this.woken = true;
          continue;
        }
      }
      fence(Ordering::SeqCst);

      if shared.window_open() || shared.window_open_cold() || !shared.receivers_alive() {
        continue;
      }
      return Poll::Pending;
    }
  }
}

impl<'a, T: Send> Drop for BoundedSendBatchFuture<'a, T> {
  fn drop(&mut self) {
    if let Some(id) = self.my_id.take() {
      if !self.sender.shared.unregister_async_send(id) {
        while !self.notified.load(Ordering::Acquire) {
          hint::spin_loop();
        }
        self.notified.store(false, Ordering::Relaxed);
        self.sender.shared.awake_release();
      }
    }
    if self.woken {
      self.woken = false;
      self.sender.shared.awake_release();
    }
  }
}

#[must_use = "futures do nothing unless you .await or poll them"]
pub struct BoundedSendBatchMutFuture<'a, T: Send> {
  sender: &'a AsyncSender<T>,
  items: &'a mut Vec<T>,
  sent: usize,
  my_id: Option<u64>,
  notified: AtomicBool,
  woken: bool,
  _phantom: PhantomPinned,
}

impl<'a, T: Send> Future for BoundedSendBatchMutFuture<'a, T> {
  type Output = Result<usize, SendError>;

  fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    let this = unsafe { self.as_mut().get_unchecked_mut() };
    let shared = &this.sender.shared;
    if this.my_id.is_some() && this.notified.swap(false, Ordering::Acquire) {
      this.my_id = None;
      this.woken = true;
    }

    loop {
      if this.items.is_empty() {
        if let Some(id) = this.my_id.take() {
          if !shared.unregister_async_send(id) {
            while !this.notified.load(Ordering::Acquire) {
              hint::spin_loop();
            }
            this.notified.store(false, Ordering::Relaxed);
            shared.awake_release();
          }
        }
        if this.woken {
          this.woken = false;
          shared.awake_release();
        }
        return Poll::Ready(Ok(this.sent));
      }

      if this.sender.closed.load(Ordering::Relaxed) || !shared.receivers_alive() {
        if let Some(id) = this.my_id.take() {
          if !shared.unregister_async_send(id) {
            while !this.notified.load(Ordering::Acquire) {
              hint::spin_loop();
            }
            this.notified.store(false, Ordering::Relaxed);
            shared.awake_release();
          }
        }
        if this.woken {
          this.woken = false;
          shared.awake_release();
        }
        return Poll::Ready(Err(SendError::Closed));
      }

      let remaining = this.items.len();
      if !shared.window_open() && shared.window_open_cold() {
        shared.publish_from_drained();
      }
      let (t, valid, m) = shared.claim_run(remaining);
      if m > 0 {
        {
          let mut drain = this.items.drain(..valid);
          shared.resolve_run(t, valid, m, &mut drain);
        }
        this.sent += valid;
        if let Some(id) = this.my_id.take() {
          if !shared.unregister_async_send(id) {
            while !this.notified.load(Ordering::Acquire) {
              hint::spin_loop();
            }
            this.notified.store(false, Ordering::Relaxed);
            shared.awake_release();
          }
        }
        if this.woken {
          this.woken = false;
          shared.awake_release();
        }
        continue;
      }

      if this.woken {
        this.woken = false;
        shared.awake_release();
      }
      match shared.register_async_send(this.my_id, cx.waker().clone(), &this.notified as *const AtomicBool) {
        Some(id) => this.my_id = Some(id),
        None => {
          while !this.notified.load(Ordering::Acquire) {
            hint::spin_loop();
          }
          this.notified.store(false, Ordering::Relaxed);
          this.my_id = None;
          this.woken = true;
          continue;
        }
      }
      fence(Ordering::SeqCst);

      if shared.window_open() || shared.window_open_cold() || !shared.receivers_alive() {
        continue;
      }
      return Poll::Pending;
    }
  }
}

impl<'a, T: Send> Drop for BoundedSendBatchMutFuture<'a, T> {
  fn drop(&mut self) {
    if let Some(id) = self.my_id.take() {
      if !self.sender.shared.unregister_async_send(id) {
        while !self.notified.load(Ordering::Acquire) {
          hint::spin_loop();
        }
        self.notified.store(false, Ordering::Relaxed);
        self.sender.shared.awake_release();
      }
    }
    if self.woken {
      self.woken = false;
      self.sender.shared.awake_release();
    }
  }
}

/// Non-blocking batch send via run-claims (shared by sync + async `try_send_batch`).
/// Callers must have already handled the up-front closed check. The first
/// full-signal (closed window or overshoot) escalates from the K-stale hot
/// window to the fresh split-counter claim (`claim_run_cold`); a full-signal
/// while already cold is a verified `Full`, returning the untouched tail.
fn try_send_run_batch<T: Send>(
  shared: &Shared<T>,
  items: Vec<T>,
) -> Result<usize, TrySendBatchError<T>> {
  let total = items.len();
  let mut iter = items.into_iter();
  let mut sent = 0;
  let mut cold = false;
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
    let (t, valid, m) = if cold {
      shared.claim_run_cold(total - sent)
    } else {
      shared.claim_run(total - sent)
    };
    if m == 0 {
      if !cold {
        cold = true;
        continue;
      }
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
      if !cold {
        cold = true;
        continue;
      }
      return Err(TrySendBatchError {
        sent,
        unsent: iter.collect(),
        reason: BatchSendErrorReason::Full,
      });
    }
  }
}
