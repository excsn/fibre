// src/spmc/mod.rs

//! A single-producer, multi-consumer (SPMC) "broadcast" channel.
//!
//! This channel is optimized for the "fan-out" or "publish-subscribe" pattern,
//! where one producer sends messages to many consumers. Every consumer that is
//! subscribed at the time of sending will see every message.
//!
//! ## Behavior
//!
//! - **Guaranteed Delivery**: This implementation guarantees that a message sent by the
//!   producer will be delivered to all active consumers. It does not drop messages.
//! - **Blocking Sender**: If any consumer is slow and the channel's buffer fills up
//!   from that consumer's perspective, the producer will block until the slow
//!   consumer catches up. This backpressure mechanism ensures no messages are lost.
//! - **`T: Clone` Requirement**: Since every message is delivered to every consumer, the
//!   message type `T` must implement the `Clone` trait.
//! - **Mixed Paradigms**: The channel supports full interoperability between synchronous
//!   and asynchronous code. You can have a synchronous `Sender` sending to an
//!   `AsyncReceiver`, or any other combination, by using the provided `to_sync()` and
//!   `to_async()` conversion methods.
//!
//! ## When to use SPMC
//!
//! - When you need to broadcast events or state updates to multiple listeners.
//! - For implementing a "fan-out" worker pattern where a central task distributes
//!   identical jobs to a pool of workers.
//! - In any situation where a single source of data needs to be multiplexed to
//!   multiple destinations without data loss.
//!
//! # Examples
//!
//! ### Basic Synchronous Usage
//!
//! ```
//! use fibre::spmc;
//! use std::thread;
//!
//! // Create a channel with a buffer capacity of 2.
//! let (mut producer, mut receiver1) = spmc::bounded(2);
//! let mut receiver2 = receiver1.clone();
//!
//! assert_eq!(producer.len(), 0);
//! assert!(producer.is_empty());
//! assert!(!producer.is_full());
//! assert_eq!(receiver1.len(), 0);
//! assert!(receiver1.is_empty());
//!
//! // The producer sends messages.
//! producer.send("hello".to_string()).unwrap();
//! assert_eq!(producer.len(), 1);
//! assert_eq!(receiver1.len(), 1);
//! assert_eq!(receiver2.len(), 1);
//!
//! producer.send("world".to_string()).unwrap();
//! assert_eq!(producer.len(), 2); // Producer sees length based on slowest consumer
//! assert!(producer.is_full());    // Capacity is 2
//! assert_eq!(receiver1.len(), 2);
//! assert_eq!(receiver2.len(), 2);
//!
//! // Both receivers get a copy of every message.
//! let handle1 = thread::spawn(move || {
//!     assert_eq!(receiver1.recv().unwrap(), "hello");
//!     assert_eq!(receiver1.recv().unwrap(), "world");
//!     assert!(receiver1.is_empty());
//! });
//!
//! let handle2 = thread::spawn(move || {
//!     assert_eq!(receiver2.recv().unwrap(), "hello");
//!     assert_eq!(receiver2.recv().unwrap(), "world");
//! });
//!
//! handle1.join().unwrap();
//! handle2.join().unwrap();
//! ```

use std::mem;
use std::sync::atomic::AtomicBool;
// No need to import Arc here if it's only used by ring_buffer types.
mod ring_buffer;

pub use crate::error::{CloseError, RecvError, SendError, TryRecvError, TrySendError};
pub use ring_buffer::{AsyncReceiver, AsyncSender, Receiver, RecvFuture, SendFuture, Sender};

// --- Constructors ---

/// Creates a new bounded synchronous SPMC channel.
///
/// `T` must be `Send + Clone` because each consumer gets its own copy of the message.
///
/// # Panics
///
/// Panics if `capacity` is 0.
pub fn bounded<T: Send + Clone>(capacity: usize) -> (Sender<T>, Receiver<T>) {
  let (p, r) = ring_buffer::new_channel(capacity);
  (p, r)
}

/// Creates a new bounded asynchronous SPMC channel.
///
/// `T` must be `Send + Clone` because each consumer gets its own copy of the message.
///
/// # Panics
///
/// Panics if `capacity` is 0.
pub fn bounded_async<T: Send + Clone>(capacity: usize) -> (AsyncSender<T>, AsyncReceiver<T>) {
  let (p, r) = ring_buffer::new_channel(capacity);
  (p.to_async(), r.to_async())
}

// --- Conversion Methods ---

impl<T: Send + Clone> Sender<T> {
  /// Converts this synchronous `Sender` into an asynchronous `AsyncSender`.
  ///
  /// This is a zero-cost conversion that facilitates interoperability between
  /// synchronous and asynchronous code. The `Drop` implementation of the
  /// original `Sender` is not called.
  pub fn to_async(self) -> AsyncSender<T> {
    let shared = unsafe { std::ptr::read(&self.shared) };
    mem::forget(self);
    AsyncSender {
      shared,
      closed: AtomicBool::new(false),
    }
  }
}

impl<T: Send + Clone> AsyncSender<T> {
  /// Converts this asynchronous `AsyncSender` into a synchronous `Sender`.
  ///
  /// This is a zero-cost conversion that facilitates interoperability between
  /// synchronous and asynchronous code. The `Drop` implementation of the
  /// original `AsyncSender` is not called.
  pub fn to_sync(self) -> Sender<T> {
    let shared = unsafe { std::ptr::read(&self.shared) };
    mem::forget(self);
    Sender {
      shared,
      closed: AtomicBool::new(false),
    }
  }
}

impl<T: Send + Clone> Receiver<T> {
  /// Converts this synchronous `Receiver` into an asynchronous `AsyncReceiver`.
  ///
  /// This is a zero-cost conversion. The original `Receiver`'s `Drop`
  /// implementation is not called.
  pub fn to_async(self) -> AsyncReceiver<T> {
    let shared = unsafe { std::ptr::read(&self.shared) };
    let tail = unsafe { std::ptr::read(&self.tail) };
    mem::forget(self);
    AsyncReceiver {
      shared,
      tail,
      closed: AtomicBool::new(false),
    }
  }
}

impl<T: Send + Clone> AsyncReceiver<T> {
  /// Converts this asynchronous `AsyncReceiver` into a synchronous `Receiver`.
  ///
  /// This is a zero-cost conversion. The original `AsyncReceiver`'s `Drop`
  /// implementation is not called.
  pub fn to_sync(self) -> Receiver<T> {
    let shared = unsafe { std::ptr::read(&self.shared) };
    let tail = unsafe { std::ptr::read(&self.tail) };
    mem::forget(self);
    Receiver {
      shared,
      tail,
      closed: AtomicBool::new(false),
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::thread;
  use std::time::Duration;

  pub const SHORT_TIMEOUT: Duration = Duration::from_millis(500);
  pub const LONG_TIMEOUT: Duration = Duration::from_secs(3);
  pub const STRESS_TIMEOUT: Duration = Duration::from_secs(15);
  pub const ITEMS_LOW: usize = 50;
  pub const ITEMS_MEDIUM: usize = 200;
  pub const ITEMS_HIGH: usize = 1000;

  #[test]
  fn spmc_single_recv() {
    let (mut tx, rx) = bounded(2);
    assert_eq!(tx.len(), 0);
    assert!(tx.is_empty());
    assert!(!tx.is_full());
    assert_eq!(rx.len(), 0);
    assert!(rx.is_empty());
    assert!(!rx.is_full()); // A receiver is "full" if len == capacity

    tx.send(10).unwrap();
    assert_eq!(tx.len(), 1);
    assert!(!tx.is_empty());
    assert_eq!(rx.len(), 1);
    assert!(!rx.is_empty());

    assert_eq!(rx.recv().unwrap(), 10);
    assert_eq!(tx.len(), 0);
    assert!(tx.is_empty());
    assert_eq!(rx.len(), 0);
    assert!(rx.is_empty());
  }

  #[test]
  fn spmc_multiple_receivers_len_checks() {
    let (mut tx, rx1) = bounded(4);
    let rx2 = rx1.clone();

    assert_eq!(tx.len(), 0);
    assert!(tx.is_empty());
    assert_eq!(rx1.len(), 0);
    assert!(rx1.is_empty());
    assert_eq!(rx2.len(), 0);
    assert!(rx2.is_empty());

    tx.send(1).unwrap(); // head = 1, min_tail = 0, tx.len = 1
    assert_eq!(tx.len(), 1);
    assert_eq!(rx1.len(), 1);
    assert_eq!(rx2.len(), 1);

    tx.send(2).unwrap(); // head = 2, min_tail = 0, tx.len = 2
    assert_eq!(tx.len(), 2);
    assert_eq!(rx1.len(), 2);
    assert_eq!(rx2.len(), 2);

    assert_eq!(rx1.recv().unwrap(), 1); // rx1.tail = 1. rx2.tail = 0. head = 2. min_tail = 0.
    assert_eq!(tx.len(), 2); // Producer len depends on slowest (rx2): 2 - 0 = 2
    assert_eq!(rx1.len(), 1); // rx1 has 1 item left (item 2)
    assert_eq!(rx2.len(), 2); // rx2 still has 2 items (1, 2)

    assert_eq!(rx2.recv().unwrap(), 1); // rx1.tail = 1. rx2.tail = 1. head = 2. min_tail = 1.
    assert_eq!(tx.len(), 1); // Both have consumed item 1: 2 - 1 = 1
    assert_eq!(rx1.len(), 1);
    assert_eq!(rx2.len(), 1);

    assert_eq!(rx1.recv().unwrap(), 2); // rx1.tail = 2. rx2.tail = 1. head = 2. min_tail = 1.
    assert_eq!(tx.len(), 1); // Producer len depends on slowest (rx2): 2 - 1 = 1
    assert_eq!(rx1.len(), 0);
    assert_eq!(rx2.len(), 1);

    assert_eq!(rx2.recv().unwrap(), 2); // rx1.tail = 2. rx2.tail = 2. head = 2. min_tail = 2.
    assert_eq!(tx.len(), 0); // All items consumed by all: 2 - 2 = 0
    assert!(tx.is_empty());
    assert_eq!(rx1.len(), 0);
    assert!(rx1.is_empty());
    assert_eq!(rx2.len(), 0);
    assert!(rx2.is_empty());
  }

  #[test]
  fn spmc_sync_try_send() {
    let (mut tx, rx1) = bounded(1);
    let rx2 = rx1.clone(); // Keep another receiver

    assert!(tx.try_send(10).is_ok()); // head = 1. min_tail(rx1,rx2) = 0. tx.len = 1.
    assert_eq!(tx.len(), 1);
    assert!(tx.is_full());

    // Channel is full because capacity is 1 and item 10 hasn't been read by rx2 yet.
    match tx.try_send(20) {
      Err(TrySendError::Full(val)) => assert_eq!(val, 20),
      other => panic!("Expected TrySendError::Full, got {:?}", other),
    }

    assert_eq!(rx1.recv().unwrap(), 10); // rx1.tail = 1. rx2.tail = 0. head = 1. min_tail = 0.
    assert_eq!(tx.len(), 1); // Producer still sees len 1 due to rx2: 1 - 0 = 1
    assert!(tx.is_full()); // Still full from producer's perspective because of rx2

    assert_eq!(rx2.recv().unwrap(), 10); // rx1.tail = 1. rx2.tail = 1. head = 1. min_tail = 1.
    assert_eq!(tx.len(), 0); // Now both receivers have consumed item 10: 1 - 1 = 0
    assert!(!tx.is_full());
    assert!(tx.is_empty());

    assert!(tx.try_send(30).is_ok()); // head = 2. min_tail(rx1,rx2) = 1. tx.len = 1.
    assert_eq!(tx.len(), 1);
    assert!(tx.is_full());
  }

  #[test]
  fn spmc_multiple_receivers() {
    let (mut tx, rx1) = bounded(ITEMS_LOW);
    let rx2 = rx1.clone();
    let rx3 = rx1.clone();

    for i in 0..ITEMS_LOW {
      tx.send(i).unwrap();
    }
    assert_eq!(tx.len(), ITEMS_LOW);
    assert!(tx.is_full());

    let h1 = thread::spawn(move || {
      for i in 0..ITEMS_LOW {
        assert_eq!(rx1.recv().unwrap(), i);
      }
    });
    let h2 = thread::spawn(move || {
      for i in 0..ITEMS_LOW {
        assert_eq!(rx2.recv().unwrap(), i);
      }
    });
    let h3 = thread::spawn(move || {
      for i in 0..ITEMS_LOW {
        assert_eq!(rx3.recv().unwrap(), i);
      }
    });

    h1.join().unwrap();
    h2.join().unwrap();
    h3.join().unwrap();
  }

  #[test]
  fn spmc_sync_slow_consumer_blocks_producer() {
    let (mut tx, rx_fast) = bounded(1); // Capacity of 1
    let rx_slow = rx_fast.clone();

    tx.send(1).unwrap();
    assert_eq!(tx.len(), 1);
    assert!(tx.is_full());

    assert_eq!(rx_fast.recv().unwrap(), 1);
    // After rx_fast reads, tx.len() is still 1 because rx_slow hasn't read.
    // head = 1, rx_fast.tail = 1, rx_slow.tail = 0. min_tail = 0. len = 1 - 0 = 1.
    assert_eq!(tx.len(), 1);
    assert!(tx.is_full());

    let send_handle = thread::spawn(move || {
      tx.send(2).unwrap();
    });

    thread::sleep(SHORT_TIMEOUT);
    assert!(!send_handle.is_finished(), "Sender should have blocked");

    assert_eq!(rx_slow.recv().unwrap(), 1);
    // After rx_slow reads, tx.len() becomes 0 relative to current head (if head was 1).
    // head = 1, rx_fast.tail = 1, rx_slow.tail = 1. min_tail = 1. len = 1 - 1 = 0.
    // But the send_handle is trying to send item 2, so head will become 2.
    // So, after send_handle unblocks: head = 2. min_tail = 1. len = 1.

    send_handle
      .join()
      .expect("Sender panicked or was not unblocked");

    assert_eq!(rx_fast.len(), 1); // Item 2 is available for rx_fast
    assert_eq!(rx_slow.len(), 1); // Item 2 is available for rx_slow

    assert_eq!(rx_fast.recv().unwrap(), 2);
    assert_eq!(rx_slow.recv().unwrap(), 2);

    assert!(rx_fast.is_empty());
    assert!(rx_slow.is_empty());
  }

  #[test]
  fn spmc_sync_all_receivers_drop_closes_channel() {
    let (mut tx, rx) = bounded(2);
    let rx2 = rx.clone();
    assert_eq!(tx.capacity(), 2);
    assert_eq!(rx.capacity(), 2);

    tx.send(1).unwrap();

    drop(rx);
    drop(rx2);

    assert!(tx.is_closed());
    assert_eq!(tx.send(2), Err(SendError::Closed));
  }

  #[test]
  fn spmc_close_and_is_closed() {
    // --- Test producer closing ---
  let (mut tx, rx) = bounded(2);
  tx.send(10).unwrap();
  assert!(!tx.is_closed());
  assert!(!rx.is_closed());

  tx.close().unwrap();
  assert_eq!(tx.close(), Err(CloseError)); // Idempotent
  assert_eq!(tx.send(20), Err(SendError::Closed));

  // After the producer is closed, the receiver can still drain the buffer.
  assert_eq!(rx.recv().unwrap(), 10);

  // The next recv will fail, confirming the channel is now disconnected.
  assert_eq!(rx.recv(), Err(RecvError::Disconnected));
  
  // After the failed recv, is_closed is definitively true.
  assert!(rx.is_closed()); // Now empty and producer is gone

  // --- Test receiver closing ---
  // (This part of the test would have run if the first part didn't panic)
  let (mut tx, rx1) = bounded(2);
  let rx2 = rx1.clone();
  tx.send(1).unwrap();

  // Close one receiver
  rx1.close().unwrap();
  assert_eq!(rx1.close(), Err(CloseError)); // Idempotent
  assert_eq!(rx1.recv(), Err(RecvError::Disconnected)); // Closed handle
  assert!(!tx.is_closed()); // Other receiver still active

  // Close the second receiver
  assert_eq!(rx2.recv().unwrap(), 1); // Can still recv
  rx2.close().unwrap();
  assert_eq!(rx2.recv(), Err(RecvError::Disconnected));

  // Now the sender should see it's closed
  assert!(tx.is_closed());
  assert_eq!(tx.send(2), Err(SendError::Closed));
  }
}
