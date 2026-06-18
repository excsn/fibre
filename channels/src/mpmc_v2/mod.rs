//! A high-performance, flexible, lock-based MPMC (Multi-Sender, Multi-Receiver) channel.
//!
//! This MPMC channel implementation is designed for both high performance and flexibility.
//! It uses a `parking_lot::Mutex` for robust state management and supports adaptive
//! backoff for its synchronous variants to reduce context-switching overhead under contention.
//!
//! A key feature of this implementation is its ability to support mixed-paradigm usage.
//! You can create a synchronous `Sender` and an asynchronous `AsyncReceiver` (or any other
//! combination) from the same channel, and they will interoperate correctly. This is
//! achieved by maintaining separate queues for synchronous and asynchronous waiters internally.
//!
//! ### When to use MPMC
//!
//! - When you need to send messages from multiple threads or tasks to multiple other
//!   threads or tasks.
//! - As a general-purpose channel when the specific producer/consumer counts are unknown
//!   or variable.
//! - For work-stealing queue patterns where multiple workers both produce and consume tasks.
//!
//! For more specific use-cases like SPSC, MPSC, or SPMC, using the specialized channels from
//! this crate will offer better performance by avoiding unnecessary locking.

use crate::error::{
  BatchSendErrorReason, CloseError, RecvError, RecvErrorTimeout, SendBatchError, SendError,
  TryRecvError, TrySendBatchError, TrySendError,
};

use self::core::{
  MpmcShared, STATE_CANCELLED, STATE_CLOSED_BUFFERED, STATE_CLOSED_RENDEZVOUS, STATE_SUCCESS_SPACE,
  STATE_WAITING,
};

pub use async_impl::{
  RecvBatchFuture, RecvBatchMutFuture, RecvFuture, SendBatchFuture, SendBatchMutFuture, SendFuture,
};

mod async_impl;
mod backoff;
mod core;
mod sync_impl;

use core::mem;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::Arc;

// --- Public Structs (Sync) ---

/// A synchronous sending handle for the MPMC channel.
///
/// Senders can be cloned to create multiple producers. When all senders for a
/// channel are dropped (or explicitly closed), the channel becomes disconnected.
#[derive(Debug)]
pub struct Sender<T: Send> {
  shared: Arc<MpmcShared<T>>,
  closed: AtomicBool,
}

/// A synchronous receiving handle for the MPMC channel.
///
/// Receivers can be cloned to create multiple consumers. When all receivers for a
/// channel are dropped (or explicitly closed), the channel is considered closed.
#[derive(Debug)]
pub struct Receiver<T: Send> {
  shared: Arc<MpmcShared<T>>,
  closed: AtomicBool,
}

// --- Public Structs (Async) ---

/// An asynchronous sending handle for the MPMC channel.
///
/// Senders can be cloned to create multiple producers. When all senders for a
/// channel are dropped (or explicitly closed), the channel becomes disconnected.
#[derive(Debug)]
pub struct AsyncSender<T: Send> {
  shared: Arc<MpmcShared<T>>,
  closed: AtomicBool,
}

/// An asynchronous receiving handle for the MPMC channel.
///
/// Receivers can be cloned to create multiple consumers. When all receivers for a
/// channel are dropped (or explicitly closed), the channel is considered closed.
#[derive(Debug)]
pub struct AsyncReceiver<T: Send> {
  shared: Arc<MpmcShared<T>>,
  closed: AtomicBool,
  /// Inline state flag for the `Stream` impl. A raw pointer to this field is stored
  /// in `waiting_async_receivers` while the stream is parked. Eagerly unlinked on
  /// drop / `to_sync` before the struct is freed.
  pub(super) state: AtomicU8,
  pub(super) is_registered: bool,
}

// --- Channel Constructors ---

/// Creates a new synchronous bounded MPMC channel.
///
/// A capacity of `0` creates a "rendezvous" channel, where a `send` will block
/// until a `recv` is ready to take the value.
pub fn bounded<T: Send>(capacity: usize) -> (Sender<T>, Receiver<T>) {
  let shared = Arc::new(MpmcShared::new(capacity));
  (
    Sender {
      shared: Arc::clone(&shared),
      closed: AtomicBool::new(false),
    },
    Receiver {
      shared,
      closed: AtomicBool::new(false),
    },
  )
}

/// Creates a new synchronous "unbounded" MPMC channel.
///
/// In reality, the channel is bounded by available memory.
pub fn unbounded<T: Send>() -> (Sender<T>, Receiver<T>) {
  bounded(usize::MAX)
}

/// Creates a new asynchronous bounded MPMC channel.
///
/// A capacity of `0` creates a "rendezvous" channel, where `send().await` will not
/// complete until a `recv().await` is ready to take the value.
pub fn bounded_async<T: Send>(capacity: usize) -> (AsyncSender<T>, AsyncReceiver<T>) {
  let shared = Arc::new(MpmcShared::new(capacity));
  (
    AsyncSender {
      shared: Arc::clone(&shared),
      closed: AtomicBool::new(false),
    },
    AsyncReceiver {
      shared,
      closed: AtomicBool::new(false),
      state: AtomicU8::new(STATE_WAITING),
      is_registered: false,
    },
  )
}

/// Creates a new asynchronous "unbounded" MPMC channel.
///
/// In reality, the channel is bounded by available memory.
pub fn unbounded_async<T: Send>() -> (AsyncSender<T>, AsyncReceiver<T>) {
  bounded_async(usize::MAX)
}

// --- Trait Implementations for Public Structs ---

// Clone (Sync)
impl<T: Send> Clone for Sender<T> {
  fn clone(&self) -> Self {
    self.shared.internal.lock().sender_count += 1;
    Sender {
      shared: Arc::clone(&self.shared),
      closed: AtomicBool::new(false),
    }
  }
}
impl<T: Send> Clone for Receiver<T> {
  fn clone(&self) -> Self {
    self.shared.internal.lock().receiver_count += 1;
    Receiver {
      shared: Arc::clone(&self.shared),
      closed: AtomicBool::new(false),
    }
  }
}

// Clone (Async)
impl<T: Send> Clone for AsyncSender<T> {
  fn clone(&self) -> Self {
    self.shared.internal.lock().sender_count += 1;
    AsyncSender {
      shared: Arc::clone(&self.shared),
      closed: AtomicBool::new(false),
    }
  }
}
impl<T: Send> Clone for AsyncReceiver<T> {
  fn clone(&self) -> Self {
    self.shared.internal.lock().receiver_count += 1;
    AsyncReceiver {
      shared: Arc::clone(&self.shared),
      closed: AtomicBool::new(false),
      state: AtomicU8::new(STATE_WAITING),
      is_registered: false,
    }
  }
}

// --- Public API Method Implementations (Sync) ---

impl<T: Send> Sender<T> {
  /// Sends a value into the channel, blocking the current thread until the value
  /// is sent or the channel is closed.
  pub fn send(&self, item: T) -> Result<(), SendError> {
    if self.closed.load(Ordering::Relaxed) {
      return Err(SendError::Closed);
    }
    sync_impl::send_sync(self, item)
  }
  /// Attempts to send a value into the channel without blocking.
  pub fn try_send(&self, item: T) -> Result<(), TrySendError<T>> {
    if self.closed.load(Ordering::Relaxed) {
      return Err(TrySendError::Closed(item));
    }
    self.shared.try_send_core(item)
  }

  /// Attempts to send a batch without blocking, taking ownership of the
  /// vector. The whole batch is processed under a single lock acquisition;
  /// satisfied receivers are woken in one coalesced pass after the lock is
  /// released.
  ///
  /// `Ok(n)` means every item was sent. Otherwise returns
  /// [`TrySendBatchError`] carrying the count sent and the unsent remainder.
  pub fn try_send_batch(&self, items: Vec<T>) -> Result<usize, TrySendBatchError<T>> {
    let total = items.len();
    if total == 0 {
      return Ok(0);
    }
    if self.closed.load(Ordering::Relaxed) {
      return Err(TrySendBatchError {
        sent: 0,
        unsent: items,
        reason: BatchSendErrorReason::Closed,
      });
    }
    let mut iter = items.into_iter();
    let (sent, reason) = self.shared.try_send_batch_core(&mut iter, total);
    match reason {
      None => Ok(total),
      Some(reason) => Err(TrySendBatchError {
        sent,
        unsent: iter.collect(),
        reason,
      }),
    }
  }

  /// Sends a batch, blocking whenever the channel is full, until every item
  /// is sent or the channel closes. For rendezvous channels, the remainder is
  /// handed off item-by-item to arriving receivers.
  pub fn send_batch(&self, items: Vec<T>) -> Result<usize, SendBatchError<T>> {
    if self.closed.load(Ordering::Relaxed) {
      return Err(SendBatchError {
        sent: 0,
        unsent: items,
      });
    }
    sync_impl::send_batch_sync(self, items)
  }

  /// Attempts to send a batch in place without blocking, draining sent items
  /// from the front of `items`. Returns `Ok(k)` with the count sent (`0` if
  /// the channel was full); `Err(SendError::Closed)` only if the channel is
  /// closed and zero items were sent by this call.
  pub fn try_send_batch_mut(&self, items: &mut Vec<T>) -> Result<usize, SendError> {
    if items.is_empty() {
      return Ok(0);
    }
    if self.closed.load(Ordering::Relaxed) {
      return Err(SendError::Closed);
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

  /// Sends a batch in place, blocking whenever the channel is full. On
  /// `Err(SendError::Closed)`, the unsent items remain in `items`.
  pub fn send_batch_mut(&self, items: &mut Vec<T>) -> Result<usize, SendError> {
    if items.is_empty() {
      return Ok(0);
    }
    if self.closed.load(Ordering::Relaxed) {
      return Err(SendError::Closed);
    }
    sync_impl::send_batch_mut_sync(self, items)
  }

  /// Closes this handle to the sending end of the channel.
  ///
  /// This is an explicit alternative to `drop`. This operation will decrement the
  /// total number of active senders. If this is the last sender, the channel will
  /// be permanently disconnected, and any waiting receivers will be woken up.
  ///
  /// # Errors
  ///
  /// Returns `Err(CloseError)` if this sender handle has already been closed.
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
    let mut to_wake = Vec::new();
    {
      let mut guard = self.shared.internal.lock();
      guard.sender_count -= 1;
      if guard.sender_count == 0 {
        for waiter in &guard.waiting_sync_receivers {
          if unsafe { &*waiter.state }
            .compare_exchange(
              STATE_WAITING,
              STATE_CLOSED_BUFFERED,
              Ordering::SeqCst,
              Ordering::SeqCst,
            )
            .is_ok()
          {
            to_wake.push(crate::mpmc_v2::core::WakeRef::Thread(waiter.thread.clone()));
          }
        }
        for waiter in &guard.waiting_async_receivers {
          if unsafe { &*waiter.state }
            .compare_exchange(
              STATE_WAITING,
              STATE_CLOSED_BUFFERED,
              Ordering::SeqCst,
              Ordering::SeqCst,
            )
            .is_ok()
          {
            to_wake.push(crate::mpmc_v2::core::WakeRef::Waker(waiter.waker.clone()));
          }
        }
      }
    }
    for w in to_wake {
      w.wake();
    }
  }

  /// Returns `true` if all receivers have been dropped, meaning the channel is closed.
  pub fn is_closed(&self) -> bool {
    self.shared.internal.lock().receiver_count == 0
  }
  /// Returns the capacity of the channel. `None` for unbounded channels.
  pub fn capacity(&self) -> Option<usize> {
    if self.shared.capacity == usize::MAX {
      None
    } else {
      Some(self.shared.capacity)
    }
  }

  /// Converts this synchronous `Sender` into an asynchronous `AsyncSender`.
  ///
  /// This is a zero-cost conversion. The `Drop` implementation of the original
  /// `Sender` is not called.
  pub fn to_async(self) -> AsyncSender<T> {
    let shared = unsafe { std::ptr::read(&self.shared) };
    mem::forget(self);
    AsyncSender {
      shared,
      closed: AtomicBool::new(false),
    }
  }

  /// Returns the number of items currently in the channel's buffer.
  /// For rendezvous channels, this will usually be 0.
  #[inline]
  pub fn len(&self) -> usize {
    self.shared.internal.lock().queue.len()
  }

  /// Returns `true` if the channel's buffer is empty.
  /// For rendezvous channels, this will usually be `true`.
  #[inline]
  pub fn is_empty(&self) -> bool {
    self.len() == 0
  }

  /// Returns `true` if the channel's buffer is full.
  /// For "unbounded" channels, this will always be `false`.
  /// For rendezvous channels (capacity 0), this will be `true` if `len()` is 0.
  #[inline]
  pub fn is_full(&self) -> bool {
    if self.shared.capacity == usize::MAX {
      false
    } else {
      self.len() == self.shared.capacity
    }
  }
}

impl<T: Send> Drop for Sender<T> {
  fn drop(&mut self) {
    // The close method is idempotent, so we can call it without checking the flag.
    let _ = self.close();
  }
}

impl<T: Send> Receiver<T> {
  /// Receives a value from the channel, blocking the current thread until a value
  /// is received or the channel is disconnected.
  pub fn recv(&self) -> Result<T, RecvError> {
    if self.closed.load(Ordering::Relaxed) {
      return Err(RecvError::Disconnected);
    }
    sync_impl::recv_sync(self)
  }

  /// Attempts to receive a value from the channel without blocking.
  pub fn try_recv(&self) -> Result<T, TryRecvError> {
    if self.closed.load(Ordering::Relaxed) {
      return Err(TryRecvError::Disconnected);
    }
    self.shared.try_recv_core()
  }

  /// Attempts to receive up to `max` items without blocking. The whole batch
  /// is collected under a single lock acquisition (waiting rendezvous
  /// senders' payloads are extracted first, then the buffer is drained);
  /// freed-up senders are woken in one pass after the lock is released.
  ///
  /// Returns 1..=max items in FIFO order, `Err(TryRecvError::Empty)`, or
  /// `Err(TryRecvError::Disconnected)`.
  pub fn try_recv_batch(&self, max: usize) -> Result<Vec<T>, TryRecvError> {
    let mut out = Vec::new();
    self.try_recv_batch_mut(&mut out, max)?;
    Ok(out)
  }

  /// Attempts to receive up to `max` items without blocking, appending them
  /// to the end of `out`. Returns the number appended.
  pub fn try_recv_batch_mut(&self, out: &mut Vec<T>, max: usize) -> Result<usize, TryRecvError> {
    if max == 0 {
      return Ok(0);
    }
    if self.closed.load(Ordering::Relaxed) {
      return Err(TryRecvError::Disconnected);
    }
    self.shared.try_recv_batch_core(out, max)
  }

  /// Receives up to `max` items, blocking until at least one is available,
  /// then draining up to `max` without further waiting. Returns between 1 and
  /// `max` items in FIFO order.
  pub fn recv_batch(&self, max: usize) -> Result<Vec<T>, RecvError> {
    let mut out = Vec::new();
    self.recv_batch_mut(&mut out, max)?;
    Ok(out)
  }

  /// Receives up to `max` items, blocking until at least one is available,
  /// appending them to the end of `out`. Returns the number appended.
  pub fn recv_batch_mut(&self, out: &mut Vec<T>, max: usize) -> Result<usize, RecvError> {
    if self.closed.load(Ordering::Relaxed) {
      return Err(RecvError::Disconnected);
    }
    sync_impl::recv_batch_sync(self, out, max)
  }

  /// Receives a value from the channel, blocking for at most `timeout` duration.
  ///
  /// # Errors
  ///
  /// - `Err(RecvErrorTimeout::Timeout)` if the timeout is reached.
  /// - `Err(RecvErrorTimeout::Disconnected)` if the channel is disconnected.
  pub fn recv_timeout(&self, timeout: std::time::Duration) -> Result<T, RecvErrorTimeout> {
    sync_impl::recv_timeout_sync(self, timeout)
  }

  /// Closes this handle to the receiving end of the channel.
  ///
  /// This is an explicit alternative to `drop`. After this is called, any subsequent
  /// receive attempts on this handle will fail. If this is the last receiver, the
  /// channel will be permanently closed, and any waiting senders will be woken up.
  ///
  /// # Errors
  ///
  /// Returns `Err(CloseError)` if this receiver handle has already been closed.
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
    let mut to_wake = Vec::new();
    {
      let mut guard = self.shared.internal.lock();
      guard.receiver_count -= 1;
      let is_real_close = guard.receiver_count == 0;

      let closed_state = if is_real_close {
        if self.shared.capacity == 0 {
          STATE_CLOSED_RENDEZVOUS
        } else {
          STATE_CLOSED_BUFFERED
        }
      } else {
        STATE_SUCCESS_SPACE
      };

      if is_real_close {
        for waiter in &guard.waiting_sync_senders {
          if unsafe { &*waiter.state }
            .compare_exchange(
              STATE_WAITING,
              closed_state,
              Ordering::SeqCst,
              Ordering::SeqCst,
            )
            .is_ok()
          {
            to_wake.push(crate::mpmc_v2::core::WakeRef::Thread(waiter.thread.clone()));
          }
        }
        for waiter in &guard.waiting_async_senders {
          if unsafe { &*waiter.state }
            .compare_exchange(
              STATE_WAITING,
              closed_state,
              Ordering::SeqCst,
              Ordering::SeqCst,
            )
            .is_ok()
          {
            to_wake.push(crate::mpmc_v2::core::WakeRef::Waker(waiter.waker.clone()));
          }
        }
      } else {
        if let Some(waiter) = guard.waiting_sync_senders.front() {
          if unsafe { &*waiter.state }
            .compare_exchange(
              STATE_WAITING,
              closed_state,
              Ordering::SeqCst,
              Ordering::SeqCst,
            )
            .is_ok()
          {
            to_wake.push(crate::mpmc_v2::core::WakeRef::Thread(waiter.thread.clone()));
          }
        }
        if let Some(waiter) = guard.waiting_async_senders.front() {
          if unsafe { &*waiter.state }
            .compare_exchange(
              STATE_WAITING,
              closed_state,
              Ordering::SeqCst,
              Ordering::SeqCst,
            )
            .is_ok()
          {
            to_wake.push(crate::mpmc_v2::core::WakeRef::Waker(waiter.waker.clone()));
          }
        }
      }
    }

    for w in to_wake {
      w.wake();
    }
  }

  /// Returns `true` if the channel is empty and all senders have been dropped.
  pub fn is_closed(&self) -> bool {
    let guard = self.shared.internal.lock();
    guard.sender_count == 0
      && guard.queue.is_empty()
      && guard.waiting_sync_senders.is_empty()
      && guard.waiting_async_senders.is_empty()
  }
  /// Returns the capacity of the channel. `None` for unbounded channels.
  pub fn capacity(&self) -> Option<usize> {
    if self.shared.capacity == usize::MAX {
      None
    } else {
      Some(self.shared.capacity)
    }
  }

  /// Converts this synchronous `Receiver` into an asynchronous `AsyncReceiver`.
  ///
  /// This is a zero-cost conversion. The `Drop` implementation of the original
  /// `Receiver` is not called.
  pub fn to_async(self) -> AsyncReceiver<T> {
    let shared = unsafe { std::ptr::read(&self.shared) };
    mem::forget(self);
    AsyncReceiver {
      shared,
      closed: AtomicBool::new(false),
      state: AtomicU8::new(STATE_WAITING),
      is_registered: false,
    }
  }

  /// Returns the number of items currently in the channel's buffer.
  /// For rendezvous channels, this will usually be 0.
  #[inline]
  pub fn len(&self) -> usize {
    self.shared.internal.lock().queue.len()
  }

  /// Returns `true` if the channel's buffer is empty.
  /// For rendezvous channels, this will usually be `true`.
  #[inline]
  pub fn is_empty(&self) -> bool {
    self.len() == 0
  }

  /// Returns `true` if the channel's buffer is full.
  /// For "unbounded" channels, this will always be `false`.
  /// For rendezvous channels (capacity 0), this will be `true` if `len()` is 0.
  #[inline]
  pub fn is_full(&self) -> bool {
    if self.shared.capacity == usize::MAX {
      false
    } else {
      self.len() == self.shared.capacity
    }
  }
}

impl<T: Send> Drop for Receiver<T> {
  fn drop(&mut self) {
    let _ = self.close();
  }
}

// --- Public API Method Implementations (Async) ---

impl<T: Send> AsyncSender<T> {
  /// Sends a value into the channel asynchronously.
  ///
  /// This method returns a future that will complete once the value has been
  /// successfully sent, or when the channel is closed.
  pub fn send(&self, item: T) -> SendFuture<'_, T> {
    async_impl::SendFuture::new(self, item)
  }

  /// Attempts to send a value into the channel without blocking (or awaiting).
  pub fn try_send(&self, item: T) -> Result<(), TrySendError<T>> {
    if self.closed.load(Ordering::Relaxed) {
      return Err(TrySendError::Closed(item));
    }
    self.shared.try_send_core(item)
  }

  /// Sends a batch asynchronously, taking ownership of the vector. Resolves
  /// with `Ok(n)` once every item is sent, or [`SendBatchError`] if the
  /// channel closes mid-batch.
  ///
  /// If the future is dropped after partial progress, the unsent remainder is
  /// dropped; use [`send_batch_mut`](Self::send_batch_mut) for cancel safety.
  pub fn send_batch(&self, items: Vec<T>) -> SendBatchFuture<'_, T> {
    async_impl::SendBatchFuture::new(self, items)
  }

  /// Sends a batch asynchronously in place, draining sent items from the
  /// front of `items`. Cancel-safe: on drop or closure, unsent items —
  /// including a parked rendezvous payload — remain in `items`.
  pub fn send_batch_mut<'a>(&'a self, items: &'a mut Vec<T>) -> SendBatchMutFuture<'a, T> {
    async_impl::SendBatchMutFuture::new(self, items)
  }

  /// Attempts to send a batch without blocking. Same semantics as
  /// [`Sender::try_send_batch`].
  pub fn try_send_batch(&self, items: Vec<T>) -> Result<usize, TrySendBatchError<T>> {
    let total = items.len();
    if total == 0 {
      return Ok(0);
    }
    if self.closed.load(Ordering::Relaxed) {
      return Err(TrySendBatchError {
        sent: 0,
        unsent: items,
        reason: BatchSendErrorReason::Closed,
      });
    }
    let mut iter = items.into_iter();
    let (sent, reason) = self.shared.try_send_batch_core(&mut iter, total);
    match reason {
      None => Ok(total),
      Some(reason) => Err(TrySendBatchError {
        sent,
        unsent: iter.collect(),
        reason,
      }),
    }
  }

  /// Attempts to send a batch in place without blocking. Same semantics as
  /// [`Sender::try_send_batch_mut`].
  pub fn try_send_batch_mut(&self, items: &mut Vec<T>) -> Result<usize, SendError> {
    if items.is_empty() {
      return Ok(0);
    }
    if self.closed.load(Ordering::Relaxed) {
      return Err(SendError::Closed);
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

  /// Closes this handle to the sending end of the channel.
  ///
  /// See [`Sender::close`] for more details.
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
    let mut to_wake = Vec::new();
    {
      let mut guard = self.shared.internal.lock();
      guard.sender_count -= 1;
      if guard.sender_count == 0 {
        for waiter in &guard.waiting_sync_receivers {
          if unsafe { &*waiter.state }
            .compare_exchange(
              STATE_WAITING,
              STATE_CLOSED_BUFFERED,
              Ordering::SeqCst,
              Ordering::SeqCst,
            )
            .is_ok()
          {
            to_wake.push(crate::mpmc_v2::core::WakeRef::Thread(waiter.thread.clone()));
          }
        }
        for waiter in &guard.waiting_async_receivers {
          if unsafe { &*waiter.state }
            .compare_exchange(
              STATE_WAITING,
              STATE_CLOSED_BUFFERED,
              Ordering::SeqCst,
              Ordering::SeqCst,
            )
            .is_ok()
          {
            to_wake.push(crate::mpmc_v2::core::WakeRef::Waker(waiter.waker.clone()));
          }
        }
      }
    }
    for w in to_wake {
      w.wake();
    }
  }

  /// Returns `true` if all receivers have been dropped, meaning the channel is closed.
  pub fn is_closed(&self) -> bool {
    self.shared.internal.lock().receiver_count == 0
  }

  /// Returns the capacity of the channel. `None` for unbounded channels.
  pub fn capacity(&self) -> Option<usize> {
    if self.shared.capacity == usize::MAX {
      None
    } else {
      Some(self.shared.capacity)
    }
  }

  /// Converts this asynchronous `AsyncSender` into a synchronous `Sender`.
  ///
  /// This is a zero-cost conversion. The `Drop` implementation of the original
  /// `AsyncSender` is not called.
  pub fn to_sync(self) -> Sender<T> {
    let shared = unsafe { std::ptr::read(&self.shared) };
    mem::forget(self);
    Sender {
      shared,
      closed: AtomicBool::new(false),
    }
  }

  /// Returns the number of items currently in the channel's buffer.
  /// For rendezvous channels, this will usually be 0.
  #[inline]
  pub fn len(&self) -> usize {
    self.shared.internal.lock().queue.len()
  }

  /// Returns `true` if the channel's buffer is empty.
  /// For rendezvous channels, this will usually be `true`.
  #[inline]
  pub fn is_empty(&self) -> bool {
    self.len() == 0
  }

  /// Returns `true` if the channel's buffer is full.
  /// For "unbounded" channels, this will always be `false`.
  /// For rendezvous channels (capacity 0), this will be `true` if `len()` is 0.
  #[inline]
  pub fn is_full(&self) -> bool {
    if self.shared.capacity == usize::MAX {
      false
    } else {
      self.len() == self.shared.capacity
    }
  }
}

impl<T: Send> Drop for AsyncSender<T> {
  fn drop(&mut self) {
    let _ = self.close();
  }
}

impl<T: Send> AsyncReceiver<T> {
  /// Receives a value from the channel asynchronously.
  ///
  /// This method returns a future that will complete when a value is received,
  /// or when the channel becomes disconnected.
  pub fn recv(&self) -> RecvFuture<'_, T> {
    async_impl::RecvFuture::new(self)
  }

  /// Attempts to receive a value from the channel without blocking (or awaiting).
  pub fn try_recv(&self) -> Result<T, TryRecvError> {
    if self.closed.load(Ordering::Relaxed) {
      return Err(TryRecvError::Disconnected);
    }
    self.shared.try_recv_core()
  }

  /// Receives up to `max` items asynchronously. Resolves with between 1 and
  /// `max` items (FIFO order) once anything is available. Cancel-safe.
  pub fn recv_batch(&self, max: usize) -> RecvBatchFuture<'_, T> {
    async_impl::RecvBatchFuture::new(self, max)
  }

  /// Receives up to `max` items asynchronously, appending them to the end of
  /// `out`. Resolves with the number appended. Cancel-safe.
  pub fn recv_batch_mut<'a>(
    &'a self,
    out: &'a mut Vec<T>,
    max: usize,
  ) -> RecvBatchMutFuture<'a, T> {
    async_impl::RecvBatchMutFuture::new(self, out, max)
  }

  /// Attempts to receive up to `max` items without blocking. Same semantics
  /// as [`Receiver::try_recv_batch`].
  pub fn try_recv_batch(&self, max: usize) -> Result<Vec<T>, TryRecvError> {
    let mut out = Vec::new();
    self.try_recv_batch_mut(&mut out, max)?;
    Ok(out)
  }

  /// Attempts to receive up to `max` items without blocking, appending them
  /// to the end of `out`. Returns the number appended.
  pub fn try_recv_batch_mut(&self, out: &mut Vec<T>, max: usize) -> Result<usize, TryRecvError> {
    if max == 0 {
      return Ok(0);
    }
    if self.closed.load(Ordering::Relaxed) {
      return Err(TryRecvError::Disconnected);
    }
    self.shared.try_recv_batch_core(out, max)
  }

  /// Closes this handle to the receiving end of the channel.
  ///
  /// See [`Receiver::close`] for more details.
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
    let mut to_wake = Vec::new();
    {
      let mut guard = self.shared.internal.lock();
      guard.receiver_count -= 1;
      let is_real_close = guard.receiver_count == 0;

      let closed_state = if is_real_close {
        if self.shared.capacity == 0 {
          STATE_CLOSED_RENDEZVOUS
        } else {
          STATE_CLOSED_BUFFERED
        }
      } else {
        STATE_SUCCESS_SPACE
      };

      if is_real_close {
        for waiter in &guard.waiting_sync_senders {
          if unsafe { &*waiter.state }
            .compare_exchange(
              STATE_WAITING,
              closed_state,
              Ordering::SeqCst,
              Ordering::SeqCst,
            )
            .is_ok()
          {
            to_wake.push(crate::mpmc_v2::core::WakeRef::Thread(waiter.thread.clone()));
          }
        }
        for waiter in &guard.waiting_async_senders {
          if unsafe { &*waiter.state }
            .compare_exchange(
              STATE_WAITING,
              closed_state,
              Ordering::SeqCst,
              Ordering::SeqCst,
            )
            .is_ok()
          {
            to_wake.push(crate::mpmc_v2::core::WakeRef::Waker(waiter.waker.clone()));
          }
        }
      } else {
        if let Some(waiter) = guard.waiting_sync_senders.front() {
          if unsafe { &*waiter.state }
            .compare_exchange(
              STATE_WAITING,
              closed_state,
              Ordering::SeqCst,
              Ordering::SeqCst,
            )
            .is_ok()
          {
            to_wake.push(crate::mpmc_v2::core::WakeRef::Thread(waiter.thread.clone()));
          }
        }
        if let Some(waiter) = guard.waiting_async_senders.front() {
          if unsafe { &*waiter.state }
            .compare_exchange(
              STATE_WAITING,
              closed_state,
              Ordering::SeqCst,
              Ordering::SeqCst,
            )
            .is_ok()
          {
            to_wake.push(crate::mpmc_v2::core::WakeRef::Waker(waiter.waker.clone()));
          }
        }
      }
    }

    for w in to_wake {
      w.wake();
    }
  }

  /// Returns `true` if the channel is empty and all senders have been dropped.
  pub fn is_closed(&self) -> bool {
    let guard = self.shared.internal.lock();
    guard.sender_count == 0
      && guard.queue.is_empty()
      && guard.waiting_sync_senders.is_empty()
      && guard.waiting_async_senders.is_empty()
  }
  /// Returns the capacity of the channel. `None` for unbounded channels.
  pub fn capacity(&self) -> Option<usize> {
    if self.shared.capacity == usize::MAX {
      None
    } else {
      Some(self.shared.capacity)
    }
  }

  /// Converts this asynchronous `AsyncReceiver` into a synchronous `Receiver`.
  ///
  /// This is a zero-cost conversion. The `Drop` implementation of the original
  /// `AsyncReceiver` is not called.
  pub fn to_sync(self) -> Receiver<T> {
    if self.is_registered {
      let state_ptr = &self.state as *const AtomicU8;
      if self
        .state
        .compare_exchange(
          STATE_WAITING,
          STATE_CANCELLED,
          Ordering::SeqCst,
          Ordering::SeqCst,
        )
        .is_ok()
      {
        // Eagerly unlink before mem::forget so the pointer doesn't dangle.
        let mut guard = self.shared.internal.lock();
        guard
          .waiting_async_receivers
          .retain(|w| w.state != state_ptr);
      }
    }
    let shared = unsafe { std::ptr::read(&self.shared) };
    mem::forget(self); // AtomicU8 has no destructor; safe to forget.
    Receiver {
      shared,
      closed: AtomicBool::new(false),
    }
  }

  /// Returns the number of items currently in the channel's buffer.
  /// For rendezvous channels, this will usually be 0.
  #[inline]
  pub fn len(&self) -> usize {
    self.shared.internal.lock().queue.len()
  }

  /// Returns `true` if the channel's buffer is empty.
  /// For rendezvous channels, this will usually be `true`.
  #[inline]
  pub fn is_empty(&self) -> bool {
    self.len() == 0
  }

  /// Returns `true` if the channel's buffer is full.
  /// For "unbounded" channels, this will always be `false`.
  /// For rendezvous channels (capacity 0), this will be `true` if `len()` is 0.
  #[inline]
  pub fn is_full(&self) -> bool {
    if self.shared.capacity == usize::MAX {
      false
    } else {
      self.len() == self.shared.capacity
    }
  }
}

impl<T: Send> Drop for AsyncReceiver<T> {
  fn drop(&mut self) {
    let _ = self.close();
    if self.is_registered {
      let state_ptr = &self.state as *const AtomicU8;
      if self
        .state
        .compare_exchange(
          STATE_WAITING,
          STATE_CANCELLED,
          Ordering::SeqCst,
          Ordering::SeqCst,
        )
        .is_ok()
      {
        let mut guard = self.shared.internal.lock();
        guard
          .waiting_async_receivers
          .retain(|w| w.state != state_ptr);
      }
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::future::Future;
  use std::sync::Arc;
  use std::task::{Context, RawWaker, RawWakerVTable, Waker};
  use std::thread;
  use std::time::Duration;

  #[cfg(not(miri))]
  #[test]
  fn test_mpmc_v2_recv_timeout_spurious_wakeup_leak() {
    // Create a bounded channel of capacity 5
    let (tx, rx) = bounded::<i32>(5);
    let rx_shared = Arc::clone(&rx.shared);

    // Spawn a receiver thread calling recv_timeout_sync
    let receiver_handle = thread::spawn(move || {
      // Use a long timeout so it stays blocked
      rx.recv_timeout(Duration::from_secs(5))
    });

    // Give the receiver thread time to park
    thread::sleep(Duration::from_millis(50));

    // Assert that exactly 1 waiter is in the queue initially
    {
      let guard = rx_shared.internal.lock();
      assert_eq!(guard.waiting_sync_receivers.len(), 1);
    }

    // Trigger a spurious wakeup by unparking the receiver thread
    receiver_handle.thread().unpark();

    // Give the thread time to wake up, loop back, and park again
    thread::sleep(Duration::from_millis(50));

    // Inspect the waiter queue length under the lock
    let leaked_count = {
      let guard = rx_shared.internal.lock();
      guard.waiting_sync_receivers.len()
    };

    // If the bug is present, the queue will contain 2 waiters (the stale leaked one and the new one).
    // If fixed, the stale waiter is eagerly unlinked on retry/timeout, keeping the count at 1.
    assert_eq!(
      leaked_count, 1,
      "Waiter was leaked! Queue contains {} waiters from a single thread.",
      leaked_count
    );

    // Unblock the receiver thread and clean up
    let _ = tx.send(42);
    let _ = receiver_handle.join().unwrap();
  }

  #[test]
  fn test_mpmc_v2_async_waker_collision_deadlock() {
    fn dummy_waker() -> Waker {
      unsafe fn clone(_: *const ()) -> RawWaker {
        RawWaker::new(std::ptr::null(), &VTABLE)
      }
      unsafe fn wake(_: *const ()) {}
      unsafe fn wake_by_ref(_: *const ()) {}
      unsafe fn drop_raw(_: *const ()) {}
      static VTABLE: RawWakerVTable = RawWakerVTable::new(clone, wake, wake_by_ref, drop_raw);
      unsafe { Waker::from_raw(RawWaker::new(std::ptr::null(), &VTABLE)) }
    }

    let (_tx, rx) = bounded_async::<i32>(0);
    let rx_clone = rx.clone();

    let mut fut1 = Box::pin(rx.recv());
    let mut fut2 = Box::pin(rx_clone.recv());

    let waker = dummy_waker();
    let mut cx = Context::from_waker(&waker);

    // 1. Poll the first receiver. It should register a waiter.
    let _ = fut1.as_mut().poll(&mut cx);

    // 2. Poll the second receiver with the same waker.
    // With the state-pointer fix active, it will NOT collide; it registers its own waiter.
    let _ = fut2.as_mut().poll(&mut cx);

    // Assert that there are exactly 2 distinct waiters in the queue (no collision)
    {
      let guard = rx.shared.internal.lock();
      assert_eq!(
        guard.waiting_async_receivers.len(),
        2,
        "Async waker collision occurred! Only 1 waiter was registered for 2 futures."
      );
    }

    // 3. Drop/cancel the first future.
    // Only fut1's waiter should be unlinked.
    drop(fut1);

    // 4. Verify that the second receiver's registration remains safely in the queue.
    {
      let guard = rx.shared.internal.lock();
      assert_eq!(
        guard.waiting_async_receivers.len(),
        1,
        "fut2's waiter registration was silently lost when fut1 was dropped!"
      );
    }

    // 5. Clean up the remaining future
    drop(fut2);

    // 6. Verify that the queue is now completely empty
    {
      let guard = rx.shared.internal.lock();
      assert_eq!(
        guard.waiting_async_receivers.len(),
        0,
        "Queue is not empty after dropping both futures!"
      );
    }
  }
}
