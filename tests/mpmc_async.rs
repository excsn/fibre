mod common;
use common::*;

use fibre::error::{RecvError, SendError};
use fibre::mpmc as mpmc;

use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
use std::sync::Arc;

// --- Helper Function for Async MPMC Tests ---
async fn run_async_mpmc_test(
  num_producers: usize,
  num_consumers: usize,
  items_per_producer: usize,
  channel_capacity: usize,
) {
  let (tx, rx) = mpmc::bounded_async(channel_capacity);
  let total_items_expected = num_producers * items_per_producer;
  let received_items_set = Arc::new(tokio::sync::Mutex::new(HashSet::new()));
  let received_count = Arc::new(AtomicUsize::new(0));

  // --- Spawn Receivers ---
  let mut consumer_handles = Vec::new();
  for _ in 0..num_consumers {
    let rx_clone = rx.clone();
    let received_set_clone = Arc::clone(&received_items_set);
    let received_count_clone = Arc::clone(&received_count);

    consumer_handles.push(tokio::spawn(async move {
      while let Ok(item) = rx_clone.recv().await {
        assert!(received_set_clone.lock().await.insert(item), "Duplicate item received!");
        received_count_clone.fetch_add(1, AtomicOrdering::Relaxed);
      }
    }));
  }
  drop(rx);

  // --- Spawn Senders ---
  let mut producer_handles = Vec::new();
  for p_id in 0..num_producers {
    let tx_clone = tx.clone();
    producer_handles.push(tokio::spawn(async move {
      for i in 0..items_per_producer {
        let item = p_id * items_per_producer + i;
        tx_clone.send(item).await.unwrap();
      }
    }));
  }
  drop(tx);

  // --- Join and Assert ---
  for handle in producer_handles {
    handle.await.expect("Sender task panicked");
  }
  for handle in consumer_handles {
    handle.await.expect("Receiver task panicked");
  }

  assert_eq!(received_count.load(AtomicOrdering::Relaxed), total_items_expected);
  assert_eq!(received_items_set.lock().await.len(), total_items_expected);
}

// --- Async MPMC Test Cases ---

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn async_v2_1p_1c_basic() {
  run_async_mpmc_test(1, 1, ITEMS_HIGH, 16).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn async_v2_mp_1c_basic() {
  run_async_mpmc_test(4, 1, ITEMS_MEDIUM, 16).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn async_v2_1p_mc_basic() {
  run_async_mpmc_test(1, 4, ITEMS_HIGH, 16).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn async_v2_mp_mc_contention() {
  run_async_mpmc_test(4, 4, ITEMS_HIGH, 4).await; // High contention
}

#[tokio::test]
async fn async_v2_unbounded_channel() {
  let (tx, mut rx) = mpmc::unbounded_async();
  let num_items = 5000;

  let producer = tokio::spawn(async move {
    for i in 0..num_items {
      tx.send(i).await.unwrap();
    }
  });

  let consumer = tokio::spawn(async move {
    for i in 0..num_items {
      assert_eq!(rx.recv().await.unwrap(), i);
    }
  });

  producer.await.unwrap();
  consumer.await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn async_v2_rendezvous_channel() {
  run_async_mpmc_test(2, 2, ITEMS_MEDIUM, 0).await;
}

#[tokio::test]
async fn async_v2_drop_producer_signals_disconnect() {
  let (tx, mut rx) = mpmc::bounded_async::<i32>(5);
  let tx2 = tx.clone();

  tx.send(1).await.unwrap();
  drop(tx);

  tx2.send(2).await.unwrap();
  drop(tx2);

  assert_eq!(rx.recv().await.unwrap(), 1);
  assert_eq!(rx.recv().await.unwrap(), 2);
  assert_eq!(rx.recv().await, Err(RecvError::Disconnected));
}

#[tokio::test]
async fn async_v2_drop_receiver_signals_closed() {
  let (tx, rx) = mpmc::bounded_async::<i32>(5);
  let rx2 = rx.clone();

  drop(rx);
  drop(rx2);

  assert_eq!(tx.send(1).await, Err(SendError::Closed));
}

#[tokio::test]
async fn async_v2_select_compatibility() {
  let (tx1, mut rx1) = mpmc::bounded_async(1);
  let (tx2, mut rx2) = mpmc::bounded_async(1);

  tokio::spawn(async move {
    tokio::time::sleep(SHORT_TIMEOUT).await;
    tx2.send(100).await.unwrap();
  });

  // rx1 will never receive anything. Its future will be polled and then dropped.
  // This must not cause a panic or lost message.
  tokio::select! {
      res1 = rx1.recv() => {
          panic!("Should not have received from rx1, got {:?}", res1);
      }
      res2 = rx2.recv() => {
          assert_eq!(res2.unwrap(), 100);
      }
  }

  // Now send to rx1 to make sure its waiter wasn't left in a corrupt state.
  tx1.send(200).await.unwrap();
  assert_eq!(rx1.recv().await.unwrap(), 200);
}
