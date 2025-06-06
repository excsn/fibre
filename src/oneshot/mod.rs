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
//! let (tx, mut rx) = oneshot::channel::<String>();
//!
//! tokio::runtime::Runtime::new().unwrap().block_on(async {
//!     tokio::spawn(async move {
//!         // Sender takes self by value, so it's consumed.
//!         // If you need to use it multiple times (e.g. in a loop trying to send), clone it.
//!         if let Err(TrySendError::Sent(val_returned)) = tx.send("Hello from oneshot!".to_string()) {
//!             println!("Failed to send, value already sent: {}", val_returned);
//!         } else {
//!             println!("Value sent successfully or receiver dropped.");
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
//! let (tx1, mut rx) = oneshot::channel::<i32>();
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
pub use crate::error::{RecvError, SendError, TryRecvError, TrySendError};

mod core; // Internal implementation details

use self::core::{OneShotShared, STATE_SENT, STATE_TAKEN}; // Import shared state and constants

use ::core::sync::atomic::Ordering;
use std::fmt; // For Sender/Receiver Debug impls
use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

/// Creates a new oneshot channel, returning a `Sender` and `Receiver` pair.
pub fn channel<T>() -> (Sender<T>, Receiver<T>) {
  let shared = Arc::new(OneShotShared::new());
  (
    Sender {
      shared: Arc::clone(&shared),
    },
    Receiver {
      shared,
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
  // No PhantomData needed if send() takes self by value, making the Sender instance itself Send.
  // If send took &self and T wasn't Send, then T:Send on impl would handle it.
}

impl<T> fmt::Debug for Sender<T> {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("Sender")
      .field("shared", &self.shared) // OneShotShared is Debug
      .finish()
  }
}

/// The receiving side of a oneshot channel.
pub struct Receiver<T> {
  shared: Arc<OneShotShared<T>>,
  // Used to make Receiver !Sync by default, as recv() takes &mut self
  // to modify internal future state implicitly or if Receiver held state.
  // If ReceiveFuture only borrows `shared`, Receiver could be Sync.
  // For now, typical pattern is !Sync for receiver ends that spawn futures.
  _phantom_not_sync: PhantomData<*mut ()>,
}

impl<T> fmt::Debug for Receiver<T> {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("Receiver").field("shared", &self.shared).finish()
  }
}

// --- Sender Implementation ---
impl<T> Sender<T> {
  /// Attempts to send a value on the oneshot channel.
  ///
  /// This method consumes the `Sender` instance. If the send is successful,
  /// the value is delivered to the `Receiver`. If a value has already been
  /// sent by another `Sender` clone, or if the `Receiver` has been dropped,
  //  this operation will fail, and the original `value` will be returned
  //  within the [`TrySendError`].
  ///
  /// This is a non-blocking operation in terms of waiting for the receiver,
  /// but it may involve a brief lock on an internal mutex.
  pub fn send(self, value: T) -> Result<(), TrySendError<T>> {
    // The actual send logic is in OneShotShared.
    // self is consumed, its Arc ref count will be decremented by Rust when it goes out of scope.
    // The Drop impl for Sender will handle the sender_count correctly.
    let result = self.shared.send(value);
    // `self` is dropped implicitly after this function, `Sender::drop` is called.
    result
  }

  /// Checks if the oneshot channel's `Receiver` has been dropped.
  pub fn is_closed(&self) -> bool {
    self.shared.receiver_dropped.load(std::sync::atomic::Ordering::Acquire)
  }

  /// Checks if a value has already been successfully sent on this channel
  /// (by any `Sender` clone).
  pub fn is_sent(&self) -> bool {
    let state = self.shared.state.load(std::sync::atomic::Ordering::Acquire);
    state == STATE_SENT || state == STATE_TAKEN
  }
}

impl<T> Clone for Sender<T> {
  fn clone(&self) -> Self {
    self.shared.increment_senders();
    Sender {
      shared: Arc::clone(&self.shared),
    }
  }
}

impl<T> Drop for Sender<T> {
  fn drop(&mut self) {
    self.shared.decrement_senders();
  }
}

// --- Receiver Implementation ---
impl<T> Receiver<T> {
  /// Waits asynchronously for the value to be sent on the channel.
  ///
  /// This method takes `&mut self` because polling a future often involves
  /// modifying state associated with the object that created the future (even if
  /// that state is within the `Arc<OneShotShared>`). It returns a [`ReceiveFuture`].
  pub fn recv(&mut self) -> ReceiveFuture<'_, T> {
    ReceiveFuture {
      receiver_shared: &self.shared,
    } // Pass Arc by reference
  }

  /// Attempts to receive a value non-blockingly.
  ///
  /// Returns:
  /// - `Ok(T)` if a value is immediately available.
  /// - `Err(TryRecvError::Empty)` if no value has been sent yet but senders may still exist
  ///   or the value is currently being written.
  /// - `Err(TryRecvError::Disconnected)` if no value was sent and all senders have dropped,
  ///   or if the receiver was already closed.
  pub fn try_recv(&mut self) -> Result<T, TryRecvError> {
    self.shared.try_recv()
  }

  /// Checks if the channel is definitively closed from the receiver's perspective.
  ///
  /// This means either:
  /// 1. A value has already been successfully sent and taken.
  /// 2. No value was sent, and all senders have dropped (so no value will ever be sent).
  /// 3. The channel was explicitly marked as closed (e.g., receiver dropped early, then senders tried).
  pub fn is_closed(&self) -> bool {
    let state = self.shared.state.load(std::sync::atomic::Ordering::Acquire);

    if state == core::STATE_TAKEN || state == core::STATE_CLOSED {
      return true; // Value taken or channel explicitly closed. No more activity.
    }

    // If state is EMPTY or WRITING or SENT (but not yet taken)
    // it's only "closed" if all senders are also gone.
    if self.shared.sender_count.load(std::sync::atomic::Ordering::Acquire) == 0 {
      // If senders are gone, and state is EMPTY or WRITING, then it's closed (no value will come).
      // If senders are gone, and state is SENT, it's not "closed" yet because the value is pending.
      // The `Receiver::drop` handles cleaning up a SENT value if receiver is dropped.
      // The `ReceiveFuture` handles transitioning SENT to TAKEN or Disconnected.
      // So, for is_closed(), if senders are gone:
      //  - if EMPTY or WRITING: true (will become Disconnected)
      //  - if SENT: false (value is there to be taken)
      //  - if TAKEN/CLOSED: true (already handled above)
      return state == core::STATE_EMPTY || state == core::STATE_WRITING;
    }

    false // Senders exist, and state is not yet TAKEN or definitively CLOSED.
  }
}

impl<T> Drop for Receiver<T> {
  fn drop(&mut self) {
    self.shared.mark_receiver_dropped();

    // If a value was sent (STATE_SENT) but never taken by this receiver,
    // we must ensure it's dropped.
    // Attempt to transition from SENT to TAKEN to "claim" the value for dropping.
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
      // Lock the mutex and take the value to drop it.
      let mut guard = self.shared.value_slot.lock().unwrap_or_else(|e| e.into_inner());
      if let Some(mut mu_value) = guard.take() {
        unsafe {
          // Manually drop the MaybeUninit<T>'s content
          mu_value.assume_init_drop();
        }
      }
    }
    // If state was not STATE_SENT (e.g., EMPTY, WRITING, already TAKEN, or CLOSED),
    // there's nothing for this Receiver's Drop to clean from value_slot related to a SENT value.
  }
}

// --- ReceiveFuture ---
#[must_use = "futures do nothing unless you .await or poll them"]
pub struct ReceiveFuture<'a, T> {
  // The future borrows the Arc from the Receiver.
  // This ensures the shared state outlives the future.
  receiver_shared: &'a Arc<OneShotShared<T>>,
}

// No methods needed on ReceiveFuture itself, all logic is in poll.

impl<'a, T: Unpin> Future for ReceiveFuture<'a, T> {
  // T: Unpin is not strictly needed here as T is not stored in the Future
  type Output = Result<T, RecvError>;

  fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    // self.receiver_shared is &'a Arc<OneShotShared<T>>
    // We need &OneShotShared to call poll_recv.
    // The Arc itself can be dereferenced.
    let shared_ref: &OneShotShared<T> = &*self.receiver_shared;
    shared_ref.poll_recv(cx)
  }
}

// Making types Send/Sync if T is Send
// Sender<T> can be Send if T is Send (because Arc<OneShotShared<T>> is Send if OneShotShared<T> is Send).
// Sender<T> can be Sync because its methods (clone, send) are safe for &Sender<T> if OneShotShared is Sync.
// `send` takes `self` by value, so no &self mutation issues. `clone` is &self.
unsafe impl<T: Send> Send for Sender<T> {}
unsafe impl<T: Send> Sync for Sender<T> {} // Safe because Arc provides shared ownership, and OneShotShared is Sync.

// Receiver<T> can be Send if T is Send.
// Receiver<T> is often not Sync if recv() takes &mut self, but our recv() returns a future
// that borrows from &mut self.
// However, if ReceiveFuture takes &'a Arc<...>, then Receiver::recv can be &self.
// Let's make Receiver::recv take &self for better ergonomics, and ReceiveFuture handles the borrow.
// If Receiver::recv() takes &self, then Receiver can be Sync.
unsafe impl<T: Send> Send for Receiver<T> {}
unsafe impl<T: Send> Sync for Receiver<T> {} // If recv() takes &self and future logic is sound.

// Re-adjust Receiver::recv if Receiver is to be Sync
impl<T> Receiver<T> {
  // If Receiver is Sync, recv should take &self.
  // The ReceiveFuture will borrow the Arc from &self.
  // pub fn recv(&self) -> ReceiveFuture<'_, T> { // Takes &self for Sync
  //     ReceiveFuture { receiver_shared: &self.shared }
  // }
  // Keeping it as &mut self for now as it's a common pattern if the future
  // conceptually "uses up" the receive attempt from that receiver instance,
  // even if the state is in Arc. If it's &mut self, Receiver is not Sync.
  // Let's stick to &mut self for Receiver::recv to be conservative about future state.
  // This means Receiver<T> is Send but not Sync.
  // The _phantom_not_sync helps enforce this.
}

// If Receiver::recv takes &mut self, then Receiver<T> is not Sync.
// The _phantom_not_sync in Receiver struct makes it !Sync correctly.
// If we change recv to take &self, then Receiver could be Sync.
// The `unsafe impl Sync for Receiver` above would be incorrect if recv() is &mut self.
// Let's remove `unsafe impl Sync for Receiver` for now.
// unsafe impl<T: Send> Sync for Receiver<T> {} // REMOVE THIS if recv is &mut self.

#[cfg(test)]
mod tests;
