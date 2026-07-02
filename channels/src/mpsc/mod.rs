//! A multi-producer, single-consumer (MPSC) channel.
//!
//! This channel is optimized for the MPSC pattern, offering higher performance
//! than a general-purpose MPMC channel by leveraging a lock-free design. It supports
//! mixed sync/async operations, allowing, for example, a synchronous `Sender` to
//! send to an asynchronous `AsyncReceiver`.

pub(crate) mod bounded_queue;
pub mod rendezvous;
mod unbounded_v3;

// --- Public re-exports ---
pub use crate::error::{
  BatchSendErrorReason, CloseError, RecvError, SendBatchError, SendError, TryRecvError,
  TrySendBatchError, TrySendError,
};

pub use unbounded_v3::{
  AsyncReceiver as UnboundedAsyncReceiver, AsyncSender as UnboundedAsyncSender,
  Receiver as UnboundedSyncReceiver, RecvBatchFuture as UnboundedRecvBatchFuture,
  RecvBatchMutFuture as UnboundedRecvBatchMutFuture, RecvFuture as UnboundedRecvFuture,
  SendBatchFuture as UnboundedSendBatchFuture, SendBatchMutFuture as UnboundedSendBatchMutFuture,
  SendFuture as UnboundedSendFuture, Sender as UnboundedSyncSender,
};

pub use rendezvous::{
  RendezvousAsyncReceiver, RendezvousAsyncSender, RendezvousSyncReceiver, RendezvousSyncSender,
};

pub use bounded_queue::{
  AsyncReceiver as BoundedAsyncReceiver, AsyncSender as BoundedAsyncSender,
  BoundedRecvBatchFuture, BoundedRecvBatchMutFuture,
  BoundedSendBatchFuture, BoundedSendBatchMutFuture,
  RecvFuture as BoundedRecvFuture, SendFuture as BoundedSendFuture,
  Receiver as BoundedSyncReceiver, Sender as BoundedSyncSender,
};

// --- Unbounded V3 (Vyukov Chain + Per-Handle Bump Slabs) Constructors ---
/// Creates a new unbounded synchronous MPSC channel.
pub fn unbounded<T: Send>() -> (UnboundedSyncSender<T>, UnboundedSyncReceiver<T>) {
  unbounded_v3::channel()
}

/// Creates a new unbounded asynchronous MPSC channel.
pub fn unbounded_async<T: Send>() -> (UnboundedAsyncSender<T>, UnboundedAsyncReceiver<T>) {
  unbounded_v3::channel_async()
}

// --- Bounded Constructors ---

/// Creates a new bounded synchronous MPSC channel with a given capacity.
///
/// # Panics
///
/// Panics if `capacity == 0`. Use [`mpsc::rendezvous`](rendezvous::rendezvous)
/// for a zero-capacity rendezvous channel.
pub fn bounded<T: Send>(capacity: usize) -> (BoundedSyncSender<T>, BoundedSyncReceiver<T>) {
  assert!(
    capacity != 0,
    "mpsc::bounded(0) is not a rendezvous channel; use mpsc::rendezvous::rendezvous() instead"
  );
  bounded_queue::bounded(capacity)
}

/// Creates a new bounded asynchronous MPSC channel with a given capacity.
///
/// # Panics
///
/// Panics if `capacity == 0`. Use
/// [`mpsc::rendezvous_async`](rendezvous::rendezvous_async) for a zero-capacity
/// rendezvous channel.
pub fn bounded_async<T: Send>(capacity: usize) -> (BoundedAsyncSender<T>, BoundedAsyncReceiver<T>) {
  assert!(
    capacity != 0,
    "mpsc::bounded_async(0) is not a rendezvous channel; use mpsc::rendezvous::rendezvous_async() instead"
  );
  bounded_queue::bounded_async(capacity)
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::thread;
  use std::time::Duration;

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
    // This assertion is racy. The consumer thread can receive the item and
    // change the length to 0 before this line executes.
    // assert_eq!(tx.len(), 1);
    assert_eq!(handle.join().unwrap(), 123);
    // rx is consumed by the thread, cannot access its len() here
  }

  #[tokio::test]
  async fn async_to_async() {
    let (tx, mut rx) = unbounded_async::<i32>();
    assert_eq!(tx.len(), 0);
    assert!(rx.is_empty());
    let handle = tokio::spawn(async move {
      let val = rx.recv().await.unwrap();
      assert!(rx.is_empty()); // Check state after recv within the task
      val
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    tx.send(456).await.unwrap();
    // This assertion is racy. The consumer task can receive the item and
    // change the length to 0 before this line executes.
    // assert_eq!(tx.len(), 1);
    assert_eq!(handle.await.unwrap(), 456);
    // rx is consumed by the task
  }

  #[tokio::test]
  async fn sync_to_async_conversion() {
    let (tx_async, mut rx_async) = unbounded_async::<i32>();
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

    let rt = tokio::runtime::Builder::new_multi_thread()
      .enable_time()
      .build()
      .unwrap();
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
    let (tx, mut rx) = unbounded_async::<i32>();
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
