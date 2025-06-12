// src/mpsc/mod.rs

//! A multi-producer, single-consumer (MPSC) channel.
//!
//! This channel is optimized for the MPSC pattern, offering higher performance
//! than a general-purpose MPMC channel by leveraging a lock-free design. It supports
//! mixed sync/async operations, allowing, for example, a synchronous `Sender` to
//! send to an asynchronous `AsyncReceiver`.

mod bounded_async;
mod bounded_sync;
mod unbounded;

use crate::coord::CapacityGate;

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

// --- Public re-exports ---
pub use crate::error::{CloseError, RecvError, SendError, TryRecvError, TrySendError};

pub use unbounded::{
  AsyncReceiver as UnboundedAsyncReceiver, AsyncSender as UnboundedAsyncSender,
  Receiver as UnboundedReceiver, RecvFuture as UnboundedRecvFuture,
  SendFuture as UnboundedSendFuture, Sender as UnboundedSender,
};

pub use bounded_async::{
  AsyncReceiver as BoundedAsyncReceiver, AsyncSender as BoundedAsyncSender,
  RecvFuture as BoundedRecvFuture, SendFuture as BoundedSendFuture,
};

pub use bounded_sync::{Receiver as BoundedReceiver, Sender as BoundedSender};

// --- Unbounded Constructors ---
/// Creates a new unbounded synchronous MPSC channel.
pub fn unbounded<T: Send>() -> (UnboundedSender<T>, UnboundedReceiver<T>) {
  let shared = Arc::new(unbounded::MpscShared::new());
  let producer = UnboundedSender {
    shared: Arc::clone(&shared),
    closed: AtomicBool::new(false),
  };
  let consumer = UnboundedReceiver {
    shared,
    closed: AtomicBool::new(false),
  };
  (producer, consumer)
}

/// Creates a new unbounded asynchronous MPSC channel.
pub fn unbounded_async<T: Send>() -> (UnboundedAsyncSender<T>, UnboundedAsyncReceiver<T>) {
  let shared = Arc::new(unbounded::MpscShared::new());
  let producer = UnboundedAsyncSender {
    shared: Arc::clone(&shared),
    closed: AtomicBool::new(false),
  };
  let consumer = UnboundedAsyncReceiver {
    shared,
    closed: AtomicBool::new(false),
  };
  (producer, consumer)
}

// --- Bounded Constructors ---

/// Creates a new bounded synchronous MPSC channel with a given capacity.
pub fn bounded<T: Send>(capacity: usize) -> (BoundedSender<T>, BoundedReceiver<T>) {
  let shared = Arc::new(bounded_sync::BoundedMpscShared {
    gate: Arc::new(CapacityGate::new(capacity)),
    channel: Arc::new(unbounded::MpscShared::new()),
  });

  let sender = BoundedSender {
    shared: shared.clone(),
    closed: AtomicBool::new(false),
  };
  let receiver = BoundedReceiver {
    shared,
    closed: AtomicBool::new(false),
  };

  (sender, receiver)
}

/// Creates a new bounded asynchronous MPSC channel with a given capacity.
pub fn bounded_async<T: Send>(capacity: usize) -> (BoundedAsyncSender<T>, BoundedAsyncReceiver<T>) {
  let shared = Arc::new(bounded_sync::BoundedMpscShared {
    gate: Arc::new(CapacityGate::new(capacity)),
    channel: Arc::new(unbounded::MpscShared::new()),
  });

  let sender = BoundedAsyncSender {
    shared: shared.clone(),
    closed: AtomicBool::new(false),
  };
  let receiver = BoundedAsyncReceiver {
    shared,
    closed: AtomicBool::new(false),
  };

  (sender, receiver)
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::thread;
  use std::time::Duration;
  use tokio::runtime::Runtime;

  #[test]
  fn sync_to_sync_blocking() {
    let (tx, rx) = unbounded::<i32>();
    assert_eq!(tx.len(), 0);
    assert!(rx.is_empty());
    let handle = thread::spawn(move || {
      let val = rx.recv().unwrap();
      assert!(rx.is_empty()); // Check state after recv within the thread
      val
    });
    thread::sleep(Duration::from_millis(50));
    tx.send(123).unwrap();
    assert_eq!(tx.len(), 1);
    assert_eq!(handle.join().unwrap(), 123);
    // rx is consumed by the thread, cannot access its len() here
  }

  #[tokio::test]
  async fn async_to_async() {
    let (tx, rx) = unbounded_async::<i32>();
    assert_eq!(tx.len(), 0);
    assert!(rx.is_empty());
    let handle = tokio::spawn(async move {
      let val = rx.recv().await.unwrap();
      assert!(rx.is_empty()); // Check state after recv within the task
      val
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    tx.send(456).await.unwrap();
    assert_eq!(tx.len(), 1);
    assert_eq!(handle.await.unwrap(), 456);
    // rx is consumed by the task
  }

  #[tokio::test]
  async fn sync_to_async_conversion() {
    let (tx_async, rx_async) = unbounded_async::<i32>();
    assert_eq!(tx_async.len(), 0);
    assert!(rx_async.is_empty());
    let tx_sync = tx_async.to_sync();

    let producer_handle = thread::spawn(move || {
      thread::sleep(Duration::from_millis(50));
      assert_eq!(tx_sync.len(), 0);
      tx_sync.send(789).unwrap();
      assert_eq!(tx_sync.len(), 1);
    });

    assert_eq!(rx_async.recv().await.unwrap(), 789);
    producer_handle.join().unwrap();
    assert!(rx_async.is_empty());
  }

  #[test]
  fn async_to_sync_conversion() {
    let (tx_async, rx_sync_orig) = unbounded_async::<i32>();
    assert_eq!(tx_async.len(), 0);
    let rx_sync = rx_sync_orig.to_sync();
    assert!(rx_sync.is_empty());

    let rt = Runtime::new().unwrap();
    rt.spawn(async move {
      tokio::time::sleep(Duration::from_millis(50)).await;
      tx_async.send(101).await.unwrap();
    });

    assert_eq!(rx_sync.recv().unwrap(), 101);
    assert!(rx_sync.is_empty());
  }

  #[test]
  fn len_and_is_empty_sync() {
    let (tx, rx) = unbounded::<i32>();
    assert_eq!(tx.len(), 0);
    assert!(rx.is_empty());

    tx.send(1).unwrap();
    assert_eq!(tx.len(), 1);
    assert!(!rx.is_empty());
    assert_eq!(rx.len(), 1);

    tx.send(2).unwrap();
    assert_eq!(tx.len(), 2);
    assert!(!rx.is_empty());
    assert_eq!(rx.len(), 2);

    assert_eq!(rx.recv().unwrap(), 1);
    assert_eq!(tx.len(), 1);
    assert!(!rx.is_empty());
    assert_eq!(rx.len(), 1);

    assert_eq!(rx.recv().unwrap(), 2);
    assert_eq!(tx.len(), 0);
    assert!(rx.is_empty());
    assert_eq!(rx.len(), 0);

    drop(tx);
    assert!(rx.is_empty());
    assert_eq!(rx.recv(), Err(RecvError::Disconnected));
  }

  #[tokio::test]
  async fn len_and_is_empty_async() {
    let (tx, rx) = unbounded_async::<i32>();
    assert_eq!(tx.len(), 0);
    assert!(rx.is_empty());

    tx.send(1).await.unwrap();
    assert_eq!(tx.len(), 1);
    assert!(!rx.is_empty());
    assert_eq!(rx.len(), 1);

    tx.send(2).await.unwrap();
    assert_eq!(tx.len(), 2);
    assert!(!rx.is_empty());
    assert_eq!(rx.len(), 2);

    assert_eq!(rx.recv().await.unwrap(), 1);
    assert_eq!(tx.len(), 1);
    assert!(!rx.is_empty());
    assert_eq!(rx.len(), 1);

    assert_eq!(rx.recv().await.unwrap(), 2);
    assert_eq!(tx.len(), 0);
    assert!(rx.is_empty());
    assert_eq!(rx.len(), 0);

    drop(tx);
    assert!(rx.is_empty());
    assert_eq!(rx.recv().await, Err(RecvError::Disconnected));
  }

  #[test]
  fn close_and_is_closed() {
    // Test sender close
    let (tx, rx) = unbounded::<i32>();
    let tx2 = tx.clone();

    assert!(!tx.is_closed());
    assert!(!rx.is_closed());
    tx.send(1).unwrap();

    tx.close().unwrap();
    assert_eq!(tx.send(2), Err(SendError::Closed)); // Cannot send on closed handle
    assert_eq!(tx.close(), Err(CloseError)); // Idempotent

    assert_eq!(rx.recv().unwrap(), 1);
    assert!(!rx.is_closed()); // Channel not closed until all senders are gone

    tx2.close().unwrap(); // Close the last sender
    assert!(rx.is_closed()); // Now it's closed
    assert_eq!(rx.recv(), Err(RecvError::Disconnected));

    // Test receiver close
    let (tx, rx) = unbounded::<i32>();
    assert!(!tx.is_closed());
    rx.close().unwrap();
    assert!(tx.is_closed()); // Sender sees receiver is gone
    assert_eq!(rx.close(), Err(CloseError));
    assert_eq!(rx.recv(), Err(RecvError::Disconnected));
    assert_eq!(tx.send(1), Err(SendError::Closed));
  }
}

#[cfg(test)]
mod bounded_tests;
