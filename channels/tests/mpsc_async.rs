//! Integration tests for the default MPSC channels, asynchronous half.

mod common;
use common::*;

use fibre::mpsc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::timeout;

#[tokio::test]
async fn mpsc_async_smoke() {
  let (mut tx, mut rx) = mpsc::unbounded_async();
  tx.send(10).await.unwrap();
  assert_eq!(rx.recv().await.unwrap(), 10);
}

#[tokio::test]
async fn mpsc_async_try_recv() {
  let (mut tx, mut rx) = mpsc::unbounded_async::<i32>();
  assert_eq!(rx.try_recv(), Err(fibre::error::TryRecvError::Empty));
  tx.send(1).await.unwrap();
  // After an await, the item will be in the queue.
  assert_eq!(rx.try_recv(), Ok(1));
  assert_eq!(rx.try_recv(), Err(fibre::error::TryRecvError::Empty));
}

#[tokio::test]
async fn mpsc_async_recv_blocks() {
  let (mut tx, mut rx) = mpsc::unbounded_async();
  let handle = tokio::spawn(async move {
    tokio::time::sleep(SHORT_TIMEOUT).await;
    tx.send("hello").await.unwrap();
  });
  assert_eq!(rx.recv().await.unwrap(), "hello");
  handle.await.unwrap();
}

#[tokio::test]
async fn mpsc_async_all_producers_drop_signals_disconnect() {
  let (tx, mut rx) = mpsc::unbounded_async::<()>();
  let tx2 = tx.clone();
  drop(tx);
  drop(tx2);
  assert_eq!(rx.recv().await, Err(fibre::error::RecvError::Disconnected));
}

#[tokio::test]
async fn mpsc_async_consumer_drop_cleans_up() {
  let drop_count = Arc::new(AtomicUsize::new(0));
  struct DropCounter(Arc<AtomicUsize>);
  impl Drop for DropCounter {
    fn drop(&mut self) {
      self.0.fetch_add(1, Ordering::SeqCst);
    }
  }

  let (mut tx, rx) = mpsc::unbounded_async();
  tx.send(DropCounter(drop_count.clone())).await.unwrap();
  tx.send(DropCounter(drop_count.clone())).await.unwrap();

  drop(rx);
  drop(tx);

  // Give a moment for any background drop tasks if necessary, though it should be immediate.
  tokio::time::sleep(std::time::Duration::from_millis(10)).await;
  assert_eq!(drop_count.load(Ordering::SeqCst), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mpsc_async_multi_producer_stress() {
  let (tx, mut rx) = mpsc::unbounded_async();
  let num_producers = 8;
  let items_per_producer = ITEMS_HIGH;
  let total_items = num_producers * items_per_producer;
  let sum = Arc::new(AtomicUsize::new(0));

  let mut handles = Vec::new();
  for _ in 0..num_producers {
    let mut tx_clone = tx.clone();
    handles.push(tokio::spawn(async move {
      for i in 1..=items_per_producer {
        tx_clone.send(i).await.unwrap();
      }
    }));
  }
  drop(tx);

  let sum_clone = sum.clone();
  let consumer_handle = tokio::spawn(async move {
    for _ in 0..total_items {
      sum_clone.fetch_add(rx.recv().await.unwrap(), Ordering::Relaxed);
    }
  });

  for handle in handles {
    handle.await.unwrap();
  }
  consumer_handle.await.unwrap();

  let expected_sum = num_producers * (items_per_producer * (items_per_producer + 1) / 2);
  assert_eq!(sum.load(Ordering::Relaxed), expected_sum);
}

#[tokio::test]
async fn mpsc_sync_producer_to_async_consumer() {
  let (tx_async, mut rx_async) = mpsc::unbounded_async();
  let mut tx_sync = tx_async.to_sync();

  // Use tokio's blocking thread pool for the sync operation
  let producer_handle = tokio::task::spawn_blocking(move || {
    tx_sync.send(123).unwrap();
  });

  assert_eq!(rx_async.recv().await.unwrap(), 123);
  producer_handle.await.unwrap();
}

#[tokio::test]
async fn test_mpsc_unbounded_recv_cancel_safe() {
  let (mut tx, mut rx) = mpsc::unbounded_async::<i32>();

  // 1. Park a recv future, then cancel it
  {
    let fut = rx.recv();
    let res = timeout(Duration::from_millis(50), fut).await;
    assert!(res.is_err(), "Future should park and time out");
  }

  // 2. Send an item; it must securely reach the receiver despite the prior cancellation
  tx.send(100).await.unwrap();
  assert_eq!(rx.try_recv(), Ok(100));
}

// --- Bounded cancel-safety ---------------------------------------------------

#[tokio::test]
async fn test_mpsc_bounded_send_reclaims_payload_on_cancel() {
  let (tx, _rx) = mpsc::bounded_async::<Arc<()>>(1);

  // 1. Fill the channel
  tx.send(Arc::new(())).await.unwrap();

  let payload = Arc::new(());
  let weak_ref = Arc::downgrade(&payload);

  // 2. Start a send that will block, then cancel it via timeout
  {
    let fut = tx.send(payload.clone());
    let res = timeout(Duration::from_millis(50), fut).await;
    assert!(res.is_err(), "Future should park and time out");
  } // `fut` is dropped here

  // 3. Drop our local strong reference
  drop(payload);

  // 4. Verify the future reclaimed and dropped the payload
  assert!(
    weak_ref.upgrade().is_none(),
    "Memory Leak: SendFuture was cancelled, but the payload was leaked into the queue!"
  );
}

#[tokio::test]
async fn test_mpsc_bounded_send_does_not_ghost_deliver() {
  let (tx, rx) = mpsc::bounded_async::<i32>(1);

  // 1. Fill the channel
  tx.send(10).await.unwrap();

  // 2. Spawn a blocked send
  let tx_clone = tx.clone();
  let handle = tokio::spawn(async move {
    let _ = tx_clone.send(20).await;
  });

  // Give it time to park
  tokio::time::sleep(Duration::from_millis(50)).await;

  // 3. Abort the blocked task
  handle.abort();
  let _ = handle.await; // Wait for abort to finish

  // 4. The receiver makes space
  assert_eq!(rx.try_recv(), Ok(10));

  // 5. Verify the cancelled '20' was not ghost-delivered
  let ghost_item = rx.try_recv();
  assert!(
    ghost_item.is_err(),
    "Ghost Delivery Bug: Cancelled SendFuture delivered an item! Got {:?}",
    ghost_item
  );
}

#[tokio::test]
async fn test_mpsc_bounded_recv_cancel_safe() {
  let (tx, rx) = mpsc::bounded_async::<i32>(2);

  // 1. Park a recv future, then cancel it
  {
    let fut = rx.recv();
    let res = timeout(Duration::from_millis(50), fut).await;
    assert!(res.is_err(), "Future should park and time out");
  } // `fut` dropped

  // 2. Verify channel still works
  tx.send(42).await.unwrap();
  assert_eq!(
    rx.try_recv(),
    Ok(42),
    "Channel broke after RecvFuture was cancelled"
  );
}

#[tokio::test]
async fn test_mpsc_bounded_batch_send_mut_retains_items() {
  let (tx, rx) = mpsc::bounded_async::<i32>(2);

  // 1. Fill the channel
  tx.send(1).await.unwrap();
  tx.send(2).await.unwrap();

  let mut items = vec![3, 4, 5];

  // 2. Attempt batch send, which will block. Cancel it.
  {
    let fut = tx.send_batch_mut(&mut items);
    let res = timeout(Duration::from_millis(50), fut).await;
    assert!(res.is_err());
  }

  // 3. Ensure unsent items remain in the original vector
  assert_eq!(
    items,
    vec![3, 4, 5],
    "Cancelled batch send modified the items vector"
  );
  assert_eq!(rx.try_recv(), Ok(1));
}
