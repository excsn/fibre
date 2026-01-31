mod common;
use common::*;

use fibre::mpsc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

#[tokio::test]
async fn mpsc_async_smoke() {
  let (tx, mut rx) = mpsc::unbounded_v1_async();
  tx.send(10).await.unwrap();
  assert_eq!(rx.recv().await.unwrap(), 10);
}

#[tokio::test]
async fn mpsc_async_try_recv() {
  let (tx, mut rx) = mpsc::unbounded_v1_async::<i32>();
  assert_eq!(rx.try_recv(), Err(fibre::error::TryRecvError::Empty));
  tx.send(1).await.unwrap();
  // After an await, the item will be in the queue.
  assert_eq!(rx.try_recv(), Ok(1));
  assert_eq!(rx.try_recv(), Err(fibre::error::TryRecvError::Empty));
}

#[tokio::test]
async fn mpsc_async_recv_blocks() {
  let (tx, mut rx) = mpsc::unbounded_v1_async();
  let handle = tokio::spawn(async move {
    tokio::time::sleep(SHORT_TIMEOUT).await;
    tx.send("hello").await.unwrap();
  });
  assert_eq!(rx.recv().await.unwrap(), "hello");
  handle.await.unwrap();
}

#[tokio::test]
async fn mpsc_async_all_producers_drop_signals_disconnect() {
  let (tx, mut rx) = mpsc::unbounded_v1_async::<()>();
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

  let (tx, rx) = mpsc::unbounded_v1_async();
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
  let (tx, mut rx) = mpsc::unbounded_v1_async();
  let num_producers = 8;
  let items_per_producer = ITEMS_HIGH;
  let total_items = num_producers * items_per_producer;
  let sum = Arc::new(AtomicUsize::new(0));

  let mut handles = Vec::new();
  for _ in 0..num_producers {
    let tx_clone = tx.clone();
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
  let (tx_async, mut rx_async) = mpsc::unbounded_v1_async();
  let tx_sync = tx_async.to_sync();

  // Use tokio's blocking thread pool for the sync operation
  let producer_handle = tokio::task::spawn_blocking(move || {
    tx_sync.send(123).unwrap();
  });

  assert_eq!(rx_async.recv().await.unwrap(), 123);
  producer_handle.await.unwrap();
}
