// src/oneshot/mod.rs

//! A oneshot channel for sending a single value between asynchronous tasks
//! or synchronous and asynchronous code.
//!
//! The `Sender` can be cloned, allowing multiple potential senders. However, only
//! the first successful `send` operation across all clones will transmit a value.
//! All subsequent attempts to send by any `Sender` clone will fail.
//! There is only one `Receiver` for the channel.
//!
//! # Examples
//!
//! ```
//! use fibre::oneshot;
//! use fibre::error::{TrySendError, RecvError};
//! use std::thread;
//!
//! // Basic usage
//! let (tx, rx) = oneshot::channel::<String>();
//!
//! tokio::runtime::Runtime::new().unwrap().block_on(async {
//!     tokio::spawn(async move {
//!         // Sender takes self by value, so it's consumed.
//!         // If you need to use it multiple times (e.g. in a loop trying to send), clone it.
//!         if let Err(TrySendError::Sent(val_returned)) = tx.send("Hello from oneshot!".to_string()) {
//!             println!("Failed to send, value already sent: {}", val_returned);
//!         } else {
//!             // Send was successful, or receiver was dropped.
//!         }
//!     });
//!
//!     match rx.recv().await {
//!         Ok(value) => println!("Received: {}", value),
//!         Err(RecvError::Disconnected) => println!("Channel disconnected, no value sent."),
//!     }
//! });
//! ```
//!
//! ```
//! // Example: All senders drop
//! use fibre::oneshot;
//! use fibre::error::RecvError;
//!
//! let (tx1, rx) = oneshot::channel::<i32>();
//! let tx2 = tx1.clone();
//!
//! tokio::runtime::Runtime::new().unwrap().block_on(async {
//!     drop(tx1);
//!     drop(tx2); // All senders dropped
//!
//!     assert_eq!(rx.recv().await, Err(RecvError::Disconnected));
//! });
//! ```
//!
//! ```
//! // Example: Receiver drops
//! use fibre::oneshot;
//! use fibre::error::TrySendError;
//!
//! let (tx, rx) = oneshot::channel::<i32>();
//! drop(rx); // Receiver dropped
//!
//! match tx.send(123) {
//!     Err(TrySendError::Closed(val)) => {
//!         assert_eq!(val, 123);
//!         println!("Send failed as expected: receiver dropped.");
//!     }
//!     _ => panic!("Send should have failed."),
//! }
//! ```

// Re-export relevant errors.
pub use crate::error::{CloseError, RecvError, SendError, TryRecvError, TrySendError};

mod core; // Internal implementation details

use self::core::{OneShotShared, STATE_SENT, STATE_TAKEN}; // Import shared state and constants

use std::fmt; // For Sender/Receiver Debug impls
use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};

/// Creates a new oneshot channel, returning a `Sender` and `Receiver` pair.
pub fn oneshot<T>() -> (Sender<T>, Receiver<T>) {
  let shared = Arc::new(OneShotShared::new());
  (
    Sender {
      shared: Arc::clone(&shared),
      closed: AtomicBool::new(false),
    },
    Receiver {
      shared,
      closed: AtomicBool::new(false),
      _phantom_not_sync: PhantomData, // Receiver often not Sync if recv() takes &mut self
    },
  )
}

/// The sending side of a oneshot channel.
///
/// Senders can be cloned. Only the first successful `send` will deliver a value.
/// The `send` method consumes the `Sender` instance. To attempt multiple sends
/// (e.g., from different tasks or in a loop until success/failure), clone the `Sender`.
pub struct Sender<T> {
  shared: Arc<OneShotShared<T>>,
  closed: AtomicBool,
}

impl<T> fmt::Debug for Sender<T> {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("Sender")
      .field("shared", &self.shared) // OneShotShared is Debug
      .field("closed", &self.closed.load(Ordering::Relaxed))
      .finish()
  }
}

/// The receiving side of a oneshot channel.
pub struct Receiver<T> {
  shared: Arc<OneShotShared<T>>,
  closed: AtomicBool,
  // Used to make Receiver !Sync by default, as recv() takes &mut self
  // to modify internal future state implicitly or if Receiver held state.
  _phantom_not_sync: PhantomData<*mut ()>,
}

impl<T> fmt::Debug for Receiver<T> {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("Receiver")
      .field("shared", &self.shared)
      .field("closed", &self.closed.load(Ordering::Relaxed))
      .finish()
  }
}

// --- Sender Implementation ---
impl<T> Sender<T> {
  /// Attempts to send a value on the oneshot channel.
  ///
  /// This method consumes the `Sender` instance. If the send is successful,
  /// the value is delivered to the `Receiver`. If a value has already been
  /// sent by another `Sender` clone, if the `Receiver` has been dropped,
  /// or if this handle was explicitly closed, this operation will fail.
  /// The original `value` will be returned within the [`TrySendError`].
  ///
  /// This is a non-blocking operation in terms of waiting for the receiver.
  pub fn send(self, value: T) -> Result<(), TrySendError<T>> {
    // Check if this specific handle was explicitly closed.
    if self.closed.load(Ordering::Relaxed) {
      // The Drop impl will not run close_internal again because the flag is set.
      return Err(TrySendError::Closed(value));
    }
    // The actual send logic is in OneShotShared.
    // `self` is consumed, and its Drop impl will be called implicitly after this.
    self.shared.send(value)
  }

  /// Closes this `Sender` handle.
  ///
  /// This is an explicit alternative to `drop`. It signals that this handle
  /// will not be used to send a value. If this is the last `Sender` handle,
  /// the channel will become disconnected, and the `Receiver` will be notified.
  ///
  /// This operation is idempotent.
  ///
  /// # Errors
  ///
  /// Returns `Err(CloseError)` if this handle has already been closed.
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

  /// The internal logic for closing/dropping a sender handle.
  fn close_internal(&self) {
    self.shared.decrement_senders();
  }

  /// Checks if the oneshot channel's `Receiver` has been dropped.
  pub fn is_closed(&self) -> bool {
    self.shared.receiver_dropped.load(Ordering::Acquire)
  }

  /// Checks if a value has already been successfully sent on this channel
  /// (by any `Sender` clone).
  pub fn is_sent(&self) -> bool {
    let state = self.shared.state.load(Ordering::Acquire);
    state == STATE_SENT || state == STATE_TAKEN
  }
}

impl<T> Clone for Sender<T> {
  fn clone(&self) -> Self {
    self.shared.increment_senders();
    Sender {
      shared: Arc::clone(&self.shared),
      closed: AtomicBool::new(false),
    }
  }
}

impl<T> Drop for Sender<T> {
  fn drop(&mut self) {
    if !self.closed.swap(true, Ordering::AcqRel) {
      self.close_internal();
    }
  }
}

// --- Receiver Implementation ---
impl<T> Receiver<T> {
  /// Waits asynchronously for the value to be sent on the channel.
  ///
  /// This returns a future that resolves to the sent value or an error if the
  /// channel is disconnected.
  pub fn recv(&self) -> ReceiveFuture<'_, T> {
    ReceiveFuture {
      receiver_shared: &self.shared,
      closed_flag: &self.closed,
    }
  }

  /// Attempts to receive a value non-blockingly.
  ///
  /// Returns:
  /// - `Ok(T)` if a value is immediately available.
  /// - `Err(TryRecvError::Empty)` if no value has been sent yet.
  /// - `Err(TryRecvError::Disconnected)` if no value was sent and all senders have dropped,
  ///   or if this receiver handle was explicitly closed.
  pub fn try_recv(&self) -> Result<T, TryRecvError> {
    if self.closed.load(Ordering::Relaxed) {
      return Err(TryRecvError::Disconnected);
    }
    self.shared.try_recv()
  }

  /// Closes the receiving end of the channel.
  ///
  /// This is an explicit alternative to `drop`. After this is called, any future
  /// send attempts will fail. If a value was already sent but not yet received,
  /// it will be dropped.
  ///
  /// This operation is idempotent.
  ///
  /// # Errors
  ///
  /// Returns `Err(CloseError)` if this handle has already been closed.
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

  /// The internal logic for closing/dropping a receiver handle.
  fn close_internal(&self) {
    self.shared.mark_receiver_dropped();

    // If a value was sent (STATE_SENT) but never taken, we must drop it.
    if self
      .shared
      .state
      .compare_exchange(
        STATE_SENT,
        STATE_TAKEN, // Mark as taken because we are handling its drop
        Ordering::AcqRel,
        Ordering::Relaxed,
      )
      .is_ok()
    {
      // We successfully claimed the value for cleanup.
      let mut guard = self
        .shared
        .value_slot
        .lock()
        .unwrap_or_else(|e| e.into_inner());
      if let Some(mut mu_value) = guard.take() {
        unsafe {
          mu_value.assume_init_drop();
        }
      }
    }
  }

  /// Checks if the channel is definitively closed from the receiver's perspective.
  ///
  /// This means either a value has been taken, or no value will ever be sent.
  pub fn is_closed(&self) -> bool {
    let state = self.shared.state.load(Ordering::Acquire);

    if state == core::STATE_TAKEN || state == core::STATE_CLOSED {
      return true;
    }

    if self.shared.sender_count.load(Ordering::Acquire) == 0 {
      // Senders are gone. If state is EMPTY, no value will come.
      // If state is SENT, a value is still pending.
      return state == core::STATE_EMPTY || state == core::STATE_WRITING;
    }

    false
  }
}

impl<T> Drop for Receiver<T> {
  fn drop(&mut self) {
    if !self.closed.swap(true, Ordering::AcqRel) {
      self.close_internal();
    }
  }
}

// --- ReceiveFuture ---
#[must_use = "futures do nothing unless you .await or poll them"]
pub struct ReceiveFuture<'a, T> {
  // The future borrows the Arc and closed flag from the Receiver.
  receiver_shared: &'a Arc<OneShotShared<T>>,
  closed_flag: &'a AtomicBool,
}

impl<'a, T> Future for ReceiveFuture<'a, T> {
  type Output = Result<T, RecvError>;

  fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    // Check if the receiver handle was explicitly closed.
    if self.closed_flag.load(Ordering::Relaxed) {
      return Poll::Ready(Err(RecvError::Disconnected));
    }

    let shared_ref: &OneShotShared<T> = &*self.receiver_shared;
    shared_ref.poll_recv(cx)
  }
}

// --- Send/Sync Implementations ---
unsafe impl<T: Send> Send for Sender<T> {}
unsafe impl<T: Send> Sync for Sender<T> {}

unsafe impl<T: Send> Send for Receiver<T> {}
// Receiver is !Sync due to the PhantomData field.

#[cfg(test)]
mod tests;
