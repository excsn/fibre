// Common constants for tests can be shared.

mod common;
use common::*;

use fibre::error::{RecvError, SendError, TrySendError, TryRecvError};
use fibre::mpmc as mpmc;

use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
use std::sync::Arc;
use std::thread;

// --- Helper Function for Sync MPMC Tests ---
fn run_sync_mpmc_test(num_producers: usize, num_consumers: usize, items_per_producer: usize, channel_capacity: usize) {
  let (tx, rx) = mpmc::bounded(channel_capacity);
  let total_items_expected = num_producers * items_per_producer;
  let received_items_set = Arc::new(std::sync::Mutex::new(HashSet::new()));
  let received_count = Arc::new(AtomicUsize::new(0));

  // --- Spawn Receivers ---
  let mut consumer_handles = Vec::new();
  for _ in 0..num_consumers {
    let rx_clone = rx.clone();
    let received_set_clone = Arc::clone(&received_items_set);
    let received_count_clone = Arc::clone(&received_count);

    consumer_handles.push(thread::spawn(move || {
      let mut local_count = 0;
      while let Ok(item) = rx_clone.recv() {
        assert!(
          received_set_clone.lock().unwrap().insert(item),
          "Duplicate item received!"
        );
        received_count_clone.fetch_add(1, AtomicOrdering::Relaxed);
        local_count += 1;
      }
      local_count
    }));
  }
  drop(rx); // Drop original handle

  // --- Spawn Senders ---
  let mut producer_handles = Vec::new();
  for p_id in 0..num_producers {
    let tx_clone = tx.clone();
    producer_handles.push(thread::spawn(move || {
      for i in 0..items_per_producer {
        let item = p_id * items_per_producer + i;
        tx_clone.send(item).unwrap();
      }
    }));
  }
  drop(tx); // Drop original handle

  // --- Join and Assert ---
  for handle in producer_handles {
    handle.join().expect("Sender thread panicked");
  }
  for handle in consumer_handles {
    handle.join().expect("Receiver thread panicked");
  }

  assert_eq!(received_count.load(AtomicOrdering::Relaxed), total_items_expected);
  assert_eq!(received_items_set.lock().unwrap().len(), total_items_expected);
}

// --- Sync MPMC Test Cases ---

#[test]
fn sync_v2_1p_1c_basic() {
  run_sync_mpmc_test(1, 1, ITEMS_HIGH, 16);
}

#[test]
fn sync_v2_mp_1c_basic() {
  run_sync_mpmc_test(4, 1, ITEMS_MEDIUM, 16);
}

#[test]
fn sync_v2_1p_mc_basic() {
  run_sync_mpmc_test(1, 4, ITEMS_HIGH, 16);
}

#[test]
fn sync_v2_mp_mc_contention() {
  run_sync_mpmc_test(4, 4, ITEMS_HIGH, 4); // High contention
}

#[test]
fn sync_v2_unbounded_channel() {
  let (tx, rx) = mpmc::unbounded();
  let num_items = 5000;

  let producer = thread::spawn(move || {
    for i in 0..num_items {
      tx.send(i).unwrap();
    }
  });

  let consumer = thread::spawn(move || {
    for i in 0..num_items {
      assert_eq!(rx.recv().unwrap(), i);
    }
  });

  producer.join().unwrap();
  consumer.join().unwrap();
}

#[test]
fn sync_v2_rendezvous_channel() {
  run_sync_mpmc_test(2, 2, ITEMS_MEDIUM, 0);
}

#[test]
fn sync_v2_drop_producer_signals_disconnect() {
  let (tx, rx) = mpmc::bounded::<i32>(5);
  let tx2 = tx.clone();

  tx.send(1).unwrap();
  drop(tx);

  tx2.send(2).unwrap();
  drop(tx2);

  assert_eq!(rx.recv().unwrap(), 1);
  assert_eq!(rx.recv().unwrap(), 2);
  assert_eq!(rx.recv(), Err(RecvError::Disconnected));
}

#[test]
fn sync_v2_drop_receiver_signals_closed() {
  let (tx, rx) = mpmc::bounded::<i32>(5);
  let rx2 = rx.clone();

  drop(rx);
  drop(rx2);

  assert_eq!(tx.send(1), Err(SendError::Closed));
}

#[test]
fn sync_v2_try_send_full_and_try_recv_empty() {
  let (tx, rx) = mpmc::bounded(1);
  tx.send(100).unwrap();

  match tx.try_send(200) {
    Err(TrySendError::Full(val)) => assert_eq!(val, 200),
    _ => panic!("Expected channel to be full"),
  }

  assert_eq!(rx.recv().unwrap(), 100);

  match rx.try_recv() {
    Err(TryRecvError::Empty) => {} // Expected
    _ => panic!("Expected channel to be empty"),
  }
}
