// src/mpmc/mod.rs

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
  CloseError, RecvError, RecvErrorTimeout, SendError, TryRecvError, TrySendError,
};

// Re-export the futures for the public API, allowing users to `await` on sends/receives.
pub use async_impl::{RecvFuture, SendFuture};

mod async_impl;
mod backoff;
mod core;
mod sync_impl;

use self::core::MpmcShared;
use ::core::mem;
use std::sync::atomic::{AtomicBool, Ordering};
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
    let sync_waiters;
    let async_waiters;
    {
      let mut guard = self.shared.internal.lock();
      guard.sender_count -= 1;
      // If we are the last sender, the channel is now disconnected.
      // We must wake up all waiting receivers to notify them.
      if guard.sender_count == 0 {
        sync_waiters = std::mem::take(&mut guard.waiting_sync_receivers);
        async_waiters = std::mem::take(&mut guard.waiting_async_receivers);
      } else {
        return;
      }
    }
    // Wake waiters outside the lock to reduce contention.
    for waiter in sync_waiters {
      waiter
        .done
        .store(true, std::sync::atomic::Ordering::Release);
      waiter.thread.unpark();
    }
    for waiter in async_waiters {
      waiter.waker.wake();
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

  /// Receives a value from the channel, blocking for at most `timeout` duration.
  ///
  /// # Errors
  ///
  /// - `Err(RecvErrorTimeout::Timeout)` if the timeout is reached.
  /// - `Err(RecvErrorTimeout::Disconnected)` if the channel is disconnected.
  pub fn recv_timeout(&self, timeout: std::time::Duration) -> Result<T, RecvErrorTimeout> {
    if self.closed.load(Ordering::Relaxed) {
      // If the handle is closed, we can still try_recv to drain remaining items.
      // The implementation logic will handle the final disconnected state correctly.
    }
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
    let sync_waiters;
    let async_waiters;
    {
      let mut guard = self.shared.internal.lock();
      guard.receiver_count -= 1;
      // If we are the last receiver, the channel is now closed to senders.
      // We must wake up all waiting senders to notify them.
      if guard.receiver_count == 0 {
        sync_waiters = std::mem::take(&mut guard.waiting_sync_senders);
        async_waiters = std::mem::take(&mut guard.waiting_async_senders);
      } else {
        // Not the last receiver. To prevent deadlocks in rendezvous channels,
        // we wake one waiting sender of each type. They can try again and
        // potentially find another active receiver.
        sync_waiters = guard.waiting_sync_senders.pop_front().into_iter().collect();
        async_waiters = guard
          .waiting_async_senders
          .pop_front()
          .into_iter()
          .collect();
      }
    }
    // Wake waiters outside the lock.
    for waiter in sync_waiters {
      waiter
        .done
        .store(true, std::sync::atomic::Ordering::Release);
      waiter.thread.unpark();
    }
    for waiter in async_waiters {
      waiter.waker.wake();
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
    if self.closed.load(Ordering::Relaxed) {
      // Create a future that immediately returns an error.
      // We can't return the error directly, so we need a future-based way to do it.
      // The SendFuture itself will check the closed state, but this provides a fast path.
      // For now, let the SendFuture handle it.
    }
    async_impl::SendFuture::new(self, item)
  }

  /// Attempts to send a value into the channel without blocking (or awaiting).
  pub fn try_send(&self, item: T) -> Result<(), TrySendError<T>> {
    if self.closed.load(Ordering::Relaxed) {
      return Err(TrySendError::Closed(item));
    }
    self.shared.try_send_core(item)
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
    let sync_waiters;
    let async_waiters;
    {
      let mut guard = self.shared.internal.lock();
      guard.sender_count -= 1;
      if guard.sender_count == 0 {
        sync_waiters = std::mem::take(&mut guard.waiting_sync_receivers);
        async_waiters = std::mem::take(&mut guard.waiting_async_receivers);
      } else {
        return;
      }
    }
    for waiter in sync_waiters {
      waiter
        .done
        .store(true, std::sync::atomic::Ordering::Release);
      waiter.thread.unpark();
    }
    for waiter in async_waiters {
      waiter.waker.wake();
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
    let sync_waiters;
    let async_waiters;
    {
      let mut guard = self.shared.internal.lock();
      guard.receiver_count -= 1;
      if guard.receiver_count == 0 {
        sync_waiters = std::mem::take(&mut guard.waiting_sync_senders);
        async_waiters = std::mem::take(&mut guard.waiting_async_senders);
      } else {
        sync_waiters = guard.waiting_sync_senders.pop_front().into_iter().collect();
        async_waiters = guard
          .waiting_async_senders
          .pop_front()
          .into_iter()
          .collect();
      }
    }
    for waiter in sync_waiters {
      waiter
        .done
        .store(true, std::sync::atomic::Ordering::Release);
      waiter.thread.unpark();
    }
    for waiter in async_waiters {
      waiter.waker.wake();
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
  // This is a zero-cost conversion. The `Drop` implementation of the original
  /// `AsyncReceiver` is not called.
  pub fn to_sync(self) -> Receiver<T> {
    let shared = unsafe { std::ptr::read(&self.shared) };
    mem::forget(self);
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
  }
}
