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
//! - **Blocking Producer**: If any consumer is slow and the channel's buffer fills up
//!   from that consumer's perspective, the producer will block until the slow
//!   consumer catches up. This backpressure mechanism ensures no messages are lost.
//! - **`T: Clone` Requirement**: Since every message is delivered to every consumer, the
//!   message type `T` must implement the `Clone` trait.
//! - **Mixed Paradigms**: The channel supports full interoperability between synchronous
//!   and asynchronous code. You can have a synchronous `Producer` sending to an
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
//! let (mut producer, mut receiver1) = spmc::channel(2);
//! let mut receiver2 = receiver1.clone();
//!
//! // The producer sends messages.
//! producer.send("hello".to_string()).unwrap();
//! producer.send("world".to_string()).unwrap();
//!
//! // Both receivers get a copy of every message.
//! let handle1 = thread::spawn(move || {
//!     assert_eq!(receiver1.recv().unwrap(), "hello");
//!     assert_eq!(receiver1.recv().unwrap(), "world");
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
//!
//! ### Mixed Sync/Async Usage
//!
//! ```
//! use fibre::spmc;
//! use std::thread;
//! use std::time::Duration;
//!
//! # async fn run() {
//! // Create an async channel.
//! let (producer_async, mut receiver_async) = spmc::channel_async(1);
//!
//! // Convert the async producer to a sync one for use in a standard thread.
//! let mut producer_sync = producer_async.to_sync();
//!
//! // A standard thread produces a message.
//! let producer_handle = thread::spawn(move || {
//!     thread::sleep(Duration::from_millis(50));
//!     println!("[Sync Thread] Sending message...");
//!     producer_sync.send(123).unwrap();
//! });
//!
//! // The async receiver awaits the message.
//! println!("[Async Task] Waiting for message...");
//! let value = receiver_async.recv().await.unwrap();
//! assert_eq!(value, 123);
//! println!("[Async Task] Received: {}", value);
//!
//! producer_handle.join().unwrap();
//! # }
//! # tokio::runtime::Runtime::new().unwrap().block_on(run());
//! ```

use std::marker::PhantomData;
use std::mem;
use std::sync::Arc;
mod ring_buffer;

pub use crate::error::{RecvError, SendError, TryRecvError};
pub use ring_buffer::{AsyncProducer, AsyncReceiver, Producer, Receiver, RecvFuture, SendFuture};

// --- Constructors ---

/// Creates a new bounded synchronous SPMC channel.
///
/// `T` must be `Clone` because each consumer gets its own copy of the message.
///
/// # Panics
///
/// Panics if `capacity` is 0.
pub fn channel<T: Send + Clone>(capacity: usize) -> (Producer<T>, Receiver<T>) {
  let (p, r) = ring_buffer::new_channel(capacity);
  (p, r)
}

/// Creates a new bounded asynchronous SPMC channel.
///
/// `T` must be `Clone` because each consumer gets its own copy of the message.
///
/// # Panics
///
/// Panics if `capacity` is 0.
pub fn channel_async<T: Send + Clone>(capacity: usize) -> (AsyncProducer<T>, AsyncReceiver<T>) {
  let (p, r) = ring_buffer::new_channel(capacity);
  (p.to_async(), r.to_async())
}

// --- Conversion Methods ---

impl<T: Send + Clone> Producer<T> {
  /// Converts this synchronous `Producer` into an asynchronous `AsyncProducer`.
  ///
  /// This is a zero-cost conversion that facilitates interoperability between
  /// synchronous and asynchronous code. The `Drop` implementation of the
  /// original `Producer` is not called.
  pub fn to_async(self) -> AsyncProducer<T> {
    let shared = unsafe { std::ptr::read(&self.shared) };
    mem::forget(self);
    AsyncProducer {
      shared,
      _phantom: PhantomData,
    }
  }
}

impl<T: Send + Clone> AsyncProducer<T> {
  /// Converts this asynchronous `AsyncProducer` into a synchronous `Producer`.
  ///
  /// This is a zero-cost conversion that facilitates interoperability between
  /// synchronous and asynchronous code. The `Drop` implementation of the
  /// original `AsyncProducer` is not called.
  pub fn to_sync(self) -> Producer<T> {
    let shared = unsafe { std::ptr::read(&self.shared) };
    mem::forget(self);
    Producer {
      shared,
      _phantom: PhantomData,
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
    AsyncReceiver { shared, tail }
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
    Receiver { shared, tail }
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::thread;
  use std::time::Duration;

  #[test]
  fn spmc_single_recv() {
    let (mut tx, mut rx) = channel(2);
    tx.send(10).unwrap();
    assert_eq!(rx.recv().unwrap(), 10);
  }

  #[test]
  fn spmc_multiple_receivers() {
    let (mut tx, mut rx1) = channel(4);
    let mut rx2 = rx1.clone();

    tx.send(1).unwrap();
    tx.send(2).unwrap();

    assert_eq!(rx1.recv().unwrap(), 1);
    assert_eq!(rx2.recv().unwrap(), 1);

    assert_eq!(rx1.recv().unwrap(), 2);
    assert_eq!(rx2.recv().unwrap(), 2);
  }

  #[test]
  fn spmc_drop_receiver_unblocks_producer() {
    let (mut tx, mut rx_fast) = channel(1);
    let rx_slow = rx_fast.clone();

    tx.send(1).unwrap();
    assert_eq!(rx_fast.recv().unwrap(), 1);

    let send_handle = thread::spawn(move || {
      // This will block, as rx_slow hasn't read item 1.
      tx.send(2).unwrap();
    });

    thread::sleep(Duration::from_millis(100));
    assert!(!send_handle.is_finished());

    // Drop the slow consumer. This should unblock the producer.
    drop(rx_slow);

    send_handle
      .join()
      .expect("Producer should be unblocked and finish");

    assert_eq!(rx_fast.recv().unwrap(), 2);
  }
}
