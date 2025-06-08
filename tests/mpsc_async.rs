mod common;
use common::*;

use fibre::mpsc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;

#[test]
fn mpsc_sync_spsc_smoke() {
  let (tx, mut rx) = mpsc::unbounded();
  tx.send(10).unwrap();
  assert_eq!(rx.recv().unwrap(), 10);
}

#[test]
fn mpsc_sync_try_send() {
  let (tx, mut rx) = mpsc::unbounded::<i32>();

  // Successful try_send
  assert_eq!(tx.try_send(10), Ok(()));
  assert_eq!(rx.recv().unwrap(), 10);

  // Drop the receiver to close the channel
  drop(rx);

  // try_send on a closed channel should fail and return the value
  match tx.try_send(20) {
    Err(fibre::error::TrySendError::Closed(val)) => assert_eq!(val, 20),
    other => panic!("Expected TrySendError::Closed, got {:?}", other),
  }

  // Also check async sender
  let (tx_async, rx_async) = mpsc::unbounded_async::<i32>();
  assert_eq!(tx_async.try_send(30), Ok(()));
  let mut rx_async_recv = rx_async.to_sync(); // use sync recv for simplicity
  assert_eq!(rx_async_recv.recv().unwrap(), 30);

  drop(rx_async_recv);
  match tx_async.try_send(40) {
    Err(fibre::error::TrySendError::Closed(val)) => assert_eq!(val, 40),
    other => panic!("Expected TrySendError::Closed, got {:?}", other),
  }
}

#[test]
fn mpsc_sync_try_recv() {
  let (tx, mut rx) = mpsc::unbounded::<i32>();
  assert_eq!(rx.try_recv(), Err(fibre::error::TryRecvError::Empty));
  tx.send(1).unwrap();
  assert_eq!(rx.try_recv(), Ok(1));
  assert_eq!(rx.try_recv(), Err(fibre::error::TryRecvError::Empty));
}

#[test]
fn mpsc_sync_recv_blocks() {
  let (tx, mut rx) = mpsc::unbounded();
  let handle = thread::spawn(move || {
    thread::sleep(SHORT_TIMEOUT);
    tx.send("hello").unwrap();
  });
  // This will block until the thread sends the message
  assert_eq!(rx.recv().unwrap(), "hello");
  handle.join().unwrap();
}

#[test]
fn mpsc_sync_all_producers_drop_signals_disconnect() {
  let (tx, mut rx) = mpsc::unbounded::<()>();
  let tx2 = tx.clone();
  drop(tx);
  drop(tx2);
  assert_eq!(rx.recv(), Err(fibre::error::RecvError::Disconnected));
}

#[test]
fn mpsc_sync_consumer_drop_cleans_up() {
  let drop_count = Arc::new(AtomicUsize::new(0));
  struct DropCounter(Arc<AtomicUsize>);
  impl Drop for DropCounter {
    fn drop(&mut self) {
      self.0.fetch_add(1, Ordering::SeqCst);
    }
  }

  let (tx, rx) = mpsc::unbounded();
  tx.send(DropCounter(drop_count.clone())).unwrap();
  tx.send(DropCounter(drop_count.clone())).unwrap();

  // Drop consumer, then producers. MpscShared drop should clean up.
  drop(rx);
  drop(tx);

  assert_eq!(drop_count.load(Ordering::SeqCst), 2);
}

#[test]
fn mpsc_sync_multi_producer_stress() {
  let (tx, mut rx) = mpsc::unbounded();
  let num_producers = 8;
  let items_per_producer = ITEMS_HIGH;
  let total_items = num_producers * items_per_producer;
  let sum = Arc::new(AtomicUsize::new(0));

  let mut handles = Vec::new();
  for _ in 0..num_producers {
    let tx_clone = tx.clone();
    handles.push(thread::spawn(move || {
      for i in 1..=items_per_producer {
        tx_clone.send(i).unwrap();
      }
    }));
  }
  drop(tx);

  let sum_clone = Arc::clone(&sum);
  let consumer_handle = thread::spawn(move || {
    // The closure now moves `sum_clone`, not `sum`.
    for _ in 0..total_items {
      sum_clone.fetch_add(rx.recv().unwrap(), Ordering::Relaxed);
    }
  });

  for handle in handles {
    handle.join().unwrap();
  }
  consumer_handle.join().unwrap();

  let expected_sum = num_producers * (items_per_producer * (items_per_producer + 1) / 2);
  assert_eq!(sum.load(Ordering::Relaxed), expected_sum);
}

#[test]
fn mpsc_async_producer_to_sync_consumer() {
  let (tx_async, rx_async) = mpsc::unbounded_async();
  let mut rx_sync = rx_async.to_sync();

  // Spawn a new OS thread to host the Tokio runtime.
  let producer_handle = thread::spawn(move || {
    // Create a runtime inside the new thread.
    let rt = tokio::runtime::Builder::new_current_thread()
      .enable_all()
      .build()
      .unwrap();

    // Block this new thread on the async producer's logic.
    rt.block_on(async move {
      // Give the sync receiver a moment to block, making the test more robust.
      tokio::time::sleep(std::time::Duration::from_millis(50)).await;
      tx_async.send(456).await.unwrap();
    });
  });

  // The main test thread now blocks on the sync receiver.
  // It will be unparked by the `send` call from the other thread.
  assert_eq!(rx_sync.recv().unwrap(), 456);

  // Clean up the producer thread.
  producer_handle.join().unwrap();
}
