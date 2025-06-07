// src/mpsc/mod.rs

//! A multi-producer, single-consumer (MPSC) channel.
//!
//! This channel is optimized for the MPSC pattern, offering higher performance
//! than a general-purpose MPMC channel by leveraging a lock-free design. It supports
//! mixed sync/async operations, allowing, for example, a synchronous `Sender` to
//! send to an asynchronous `AsyncReceiver`.

use std::marker::PhantomData;
use std::mem;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
mod lockfree;

// Public re-exports
pub use crate::error::{CloseError, RecvError, SendError, TryRecvError, TrySendError};
pub use lockfree::{AsyncReceiver, AsyncSender, Receiver, RecvFuture, SendFuture, Sender};

// --- Sync Constructor ---
/// Creates a new unbounded synchronous MPSC channel.
pub fn unbounded<T: Send>() -> (Sender<T>, Receiver<T>) {
  let shared = Arc::new(lockfree::MpscShared::new());
  let producer = Sender {
    shared: Arc::clone(&shared),
    closed: AtomicBool::new(false),
  };
  let consumer = Receiver {
    shared,
    closed: AtomicBool::new(false),
  };
  (producer, consumer)
}

// --- Async Constructor ---
/// Creates a new unbounded asynchronous MPSC channel.
pub fn unbounded_async<T: Send>() -> (AsyncSender<T>, AsyncReceiver<T>) {
  let shared = Arc::new(lockfree::MpscShared::new());
  let producer = AsyncSender {
    shared: Arc::clone(&shared),
    closed: AtomicBool::new(false),
  };
  let consumer = AsyncReceiver {
    shared,
    closed: AtomicBool::new(false),
  };
  (producer, consumer)
}

// --- Conversion Methods ---

impl<T: Send> Sender<T> {
  /// Converts this synchronous `Sender` into an asynchronous `AsyncSender`.
  pub fn to_async(self) -> AsyncSender<T> {
    let shared = unsafe { std::ptr::read(&self.shared) };
    mem::forget(self);
    AsyncSender {
      shared,
      closed: AtomicBool::new(false),
    }
  }
}

impl<T: Send> Receiver<T> {
  /// Converts this synchronous `Receiver` into an asynchronous `AsyncReceiver`.
  pub fn to_async(self) -> AsyncReceiver<T> {
    let shared = unsafe { std::ptr::read(&self.shared) };
    mem::forget(self);
    AsyncReceiver {
      shared,
      closed: AtomicBool::new(false),
    }
  }
}

impl<T: Send> AsyncSender<T> {
  /// Converts this asynchronous `AsyncSender` into a synchronous `Sender`.
  pub fn to_sync(self) -> Sender<T> {
    let shared = unsafe { std::ptr::read(&self.shared) };
    mem::forget(self);
    Sender {
      shared,
      closed: AtomicBool::new(false),
    }
  }
}

impl<T: Send> AsyncReceiver<T> {
  /// Converts this asynchronous `AsyncReceiver` into a synchronous `Receiver`.
  pub fn to_sync(self) -> Receiver<T> {
    let shared = unsafe { std::ptr::read(&self.shared) };
    mem::forget(self);
    Receiver {
      shared,
      closed: AtomicBool::new(false),
    }
  }
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
