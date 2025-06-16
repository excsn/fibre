//! The core implementation and synchronous API for the bounded MPSC channel.

use crate::coord::CapacityGate;
use crate::error::{CloseError, RecvError, SendError, TryRecvError, TrySendError};
use crate::mpsc::unbounded;
use crate::sync_util;

use std::mem;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;

// Import the async types for `to_async` conversions.
use super::bounded_async::{AsyncReceiver, AsyncSender};

// --- Internal Message & Permit ---

/// A private RAII guard for a capacity permit.
/// When this is dropped, a permit is released back to the gate.
#[derive(Debug)]
pub(crate) struct Permit {
  pub(crate) gate: Arc<CapacityGate>,
  // This flag prevents releasing a permit for zero-capacity (rendezvous) channels.
  pub(crate) is_rendezvous: bool,
}

impl Drop for Permit {
  fn drop(&mut self) {
    // Only release the permit if it's not a rendezvous channel.
    // In a rendezvous, the permit represents the receiver's readiness and is
    // consumed by the sender, not returned to the pool.
    if !self.is_rendezvous {
      self.gate.release();
    }
  }
}

/// The internal message type that travels through the underlying unbounded channel.
/// It bundles the user's value with the capacity permit.
pub(crate) struct BoundedMessage<T> {
  pub(crate) value: T,
  pub(crate) _permit: Permit,
}

// --- Shared State ---

/// The shared state for the bounded MPSC channel.
/// This is wrapped in an Arc and shared between the sender(s) and receiver.
#[derive(Debug)]
pub(crate) struct BoundedMpscShared<T: Send> {
  pub(crate) gate: Arc<CapacityGate>,
  // The underlying lock-free channel that transports BoundedMessage<T>
  pub(crate) channel: Arc<unbounded::MpscShared<BoundedMessage<T>>>,
}

// --- Public Channel Handles (Sync) ---

#[derive(Debug)]
pub struct Sender<T: Send> {
  pub(crate) shared: Arc<BoundedMpscShared<T>>,
  pub(crate) closed: AtomicBool,
}

#[derive(Debug)]
pub struct Receiver<T: Send> {
  pub(crate) shared: Arc<BoundedMpscShared<T>>,
  pub(crate) closed: AtomicBool,
}

// --- Sender Implementation (Sync) ---

impl<T: Send> Sender<T> {
  /// Sends a value, blocking the current thread until a slot is available
  /// in the channel if it is full.
  pub fn send(&self, value: T) -> Result<(), SendError> {
    if self.closed.load(Ordering::Relaxed)
      || self.shared.channel.receiver_dropped.load(Ordering::Acquire)
    {
      return Err(SendError::Closed);
    }

    // Block until a permit is available. For capacity 0, this waits for a receiver.
    self.shared.gate.acquire_sync();

    let permit = Permit {
      gate: self.shared.gate.clone(),
      is_rendezvous: self.capacity() == 0,
    };
    let message = BoundedMessage {
      value,
      _permit: permit,
    };

    // The underlying send is lock-free and won't block.
    // It can only fail if the receiver was dropped.
    if unbounded::send_internal(&self.shared.channel, message).is_err() {
      // The receiver was dropped. The `Permit` inside our `message` is dropped here.
      // For capacity > 0, this correctly releases the permit.
      // For capacity == 0, it does nothing, which is also correct.
      return Err(SendError::Closed);
    }

    Ok(())
  }

  /// Attempts to send a value into the channel without blocking.
  pub fn try_send(&self, value: T) -> Result<(), TrySendError<T>> {
    if self.closed.load(Ordering::Relaxed)
      || self.shared.channel.receiver_dropped.load(Ordering::Acquire)
    {
      return Err(TrySendError::Closed(value));
    }

    // Try to acquire a permit non-blockingly.
    if !self.shared.gate.try_acquire() {
      return Err(TrySendError::Full(value));
    }

    let permit = Permit {
      gate: self.shared.gate.clone(),
      is_rendezvous: self.capacity() == 0,
    };
    let message = BoundedMessage {
      value,
      _permit: permit,
    };

    if let Err(msg) = unbounded::send_internal(&self.shared.channel, message) {
      // Receiver dropped, `msg._permit` is dropped, releasing the gate slot.
      return Err(TrySendError::Closed(msg.value));
    }

    Ok(())
  }

  /// Closes this sender handle.
  ///
  /// This is an explicit alternative to `drop`. If this is the last sender handle,
  /// the channel will become disconnected from the receiver's perspective.
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
    if self
      .shared
      .channel
      .sender_count
      .fetch_sub(1, Ordering::AcqRel)
      == 1
    {
      // This was the last sender, wake the consumer to signal disconnection.
      self.shared.channel.wake_consumer();
      // Also wake the gate in case the receiver is waiting on a rendezvous
      self.shared.gate.release();
    }
  }

  /// Returns `true` if the receiver has been dropped.
  pub fn is_closed(&self) -> bool {
    self.shared.channel.receiver_dropped.load(Ordering::Acquire)
  }

  pub fn sender_count(&self) -> usize {
    self.shared.channel.sender_count.load(Ordering::Relaxed)
  }

  /// Returns the number of messages currently in the channel.
  pub fn len(&self) -> usize {
    self.shared.channel.current_len.load(Ordering::Relaxed)
  }

  /// Returns `true` if the channel is empty.
  pub fn is_empty(&self) -> bool {
    self.len() == 0
  }

  /// Returns the total capacity of the channel.
  pub fn capacity(&self) -> usize {
    self.shared.gate.capacity()
  }

  /// Returns `true` if the channel is full.
  pub fn is_full(&self) -> bool {
    // For a zero-capacity channel, it's always "full" until a receiver is ready.
    self.len() == self.capacity()
  }

  /// Converts this synchronous `Sender` into an `AsyncSender`.
  pub fn to_async(self) -> AsyncSender<T> {
    let shared = unsafe { std::ptr::read(&self.shared) };
    mem::forget(self);
    AsyncSender {
      shared,
      closed: AtomicBool::new(false),
    }
  }
}

impl<T: Send> Clone for Sender<T> {
  /// Clones this sender.
  ///
  /// A bounded MPSC channel can have multiple producers.
  fn clone(&self) -> Self {
    self
      .shared
      .channel
      .sender_count
      .fetch_add(1, Ordering::Relaxed);
    Self {
      shared: self.shared.clone(),
      closed: AtomicBool::new(false),
    }
  }
}

impl<T: Send> Drop for Sender<T> {
  fn drop(&mut self) {
    if !self.closed.swap(true, Ordering::AcqRel) {
      self.close_internal();
    }
  }
}

// --- Receiver Implementation (Sync) ---

impl<T: Send> Receiver<T> {
  /// Receives a value, blocking the current thread until one is available.
  pub fn recv(&self) -> Result<T, RecvError> {
    if self.closed.load(Ordering::Relaxed) {
      return Err(RecvError::Disconnected);
    }

    // For rendezvous channels, the receiver must signal its readiness
    // by providing a permit for the sender to acquire.
    if self.capacity() == 0 {
      self.shared.gate.release();
    }

    loop {
      match self.try_recv_internal_no_release() {
        Ok(value) => return Ok(value),
        Err(TryRecvError::Disconnected) => return Err(RecvError::Disconnected),
        Err(TryRecvError::Empty) => {}
      }

      let lf_shared = &self.shared.channel;
      *lf_shared.consumer_thread.lock().unwrap() = Some(thread::current());
      lf_shared.consumer_parked.store(true, Ordering::Release);

      match self.try_recv_internal_no_release() {
        Ok(value) => {
          if lf_shared
            .consumer_parked
            .compare_exchange(true, false, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
          {
            *lf_shared.consumer_thread.lock().unwrap() = None;
          }
          return Ok(value);
        }
        Err(TryRecvError::Disconnected) => {
          if lf_shared
            .consumer_parked
            .compare_exchange(true, false, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
          {
            *lf_shared.consumer_thread.lock().unwrap() = None;
          }
          return Err(RecvError::Disconnected);
        }
        Err(TryRecvError::Empty) => {
          sync_util::park_thread();
          if lf_shared
            .consumer_parked
            .compare_exchange(true, false, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
          {
            *lf_shared.consumer_thread.lock().unwrap() = None;
          }
        }
      }
    }
  }

  // Private helper to avoid calling release in the blocking `recv` loop.
  fn try_recv_internal_no_release(&self) -> Result<T, TryRecvError> {
    if self.closed.load(Ordering::Relaxed) {
      return Err(TryRecvError::Disconnected);
    }
    self.shared.channel.try_recv_internal().map(|msg| msg.value)
  }

  /// Attempts to receive a value without blocking.
  pub fn try_recv(&self) -> Result<T, TryRecvError> {
    if self.closed.load(Ordering::Relaxed) {
      return Err(TryRecvError::Disconnected);
    }
    // For rendezvous, release a permit to signal readiness for a try_send.
    if self.capacity() == 0 {
      // Note: This permit may be "leaked" if no sender is ready, but this is
      // acceptable for `try_recv` semantics. The alternative is much more complex.
      self.shared.gate.release();
    }
    self.shared.channel.try_recv_internal().map(|msg| msg.value)
  }

  /// Closes the receiving end of the channel.
  ///
  /// This is an explicit alternative to `drop`. After this is called, any
  /// `send` attempts will fail. Any buffered items are drained and dropped.
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
    self
      .shared
      .channel
      .receiver_dropped
      .store(true, Ordering::Release);
    // Drain the channel using the internal recv function to bypass the `self.closed` check.
    // This ensures any buffered items (and their permits) are dropped, unblocking senders.
    while self.shared.channel.try_recv_internal().is_ok() {}
    // Also wake the gate in case a sender is waiting on a rendezvous
    self.shared.gate.release();
  }

  /// Returns `true` if all senders have been dropped and the channel is empty.
  pub fn is_closed(&self) -> bool {
    let chan = &self.shared.channel;
    chan.sender_count.load(Ordering::Acquire) == 0 && self.is_empty()
  }

  pub fn sender_count(&self) -> usize {
    self.shared.channel.sender_count.load(Ordering::Relaxed)
  }

  /// Returns the number of messages currently in the channel.
  pub fn len(&self) -> usize {
    self.shared.channel.current_len.load(Ordering::Relaxed)
  }

  /// Returns `true` if the channel is empty.
  pub fn is_empty(&self) -> bool {
    self.len() == 0
  }

  /// Returns the total capacity of the channel.
  pub fn capacity(&self) -> usize {
    self.shared.gate.capacity()
  }

  /// Returns `true` if the channel is full.
  pub fn is_full(&self) -> bool {
    self.len() == self.capacity()
  }

  /// Converts this synchronous `Receiver` into an `AsyncReceiver`.
  pub fn to_async(self) -> AsyncReceiver<T> {
    let shared = unsafe { std::ptr::read(&self.shared) };
    mem::forget(self);
    AsyncReceiver {
      shared,
      closed: AtomicBool::new(false),
    }
  }
}

impl<T: Send> Drop for Receiver<T> {
  fn drop(&mut self) {
    if !self.closed.swap(true, Ordering::AcqRel) {
      self.close_internal();
    }
  }
}
