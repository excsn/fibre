mod common;
use common::*;

use fibre::spmc;
use std::thread;

#[test]
fn spmc_sync_spsc_smoke() {
  let (tx, rx) = spmc::bounded(2);
  tx.send(10).unwrap();
  assert_eq!(rx.recv().unwrap(), 10);
}

#[test]
fn spmc_sync_try_recv() {
  let (tx, rx) = spmc::bounded::<i32>(2);
  assert_eq!(rx.try_recv(), Err(fibre::error::TryRecvError::Empty));
  tx.send(1).unwrap();
  assert_eq!(rx.try_recv(), Ok(1));
  assert_eq!(rx.try_recv(), Err(fibre::error::TryRecvError::Empty));
}

#[test]
fn spmc_sync_multi_consumer() {
  let (tx, rx1) = spmc::bounded(ITEMS_LOW);
  let rx2 = rx1.clone();
  let rx3 = rx1.clone();

  for i in 0..ITEMS_LOW {
    tx.send(i).unwrap();
  }

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
  let (tx, rx_fast) = spmc::bounded(1); // Capacity of 1
  let rx_slow = rx_fast.clone();

  // Fill the buffer
  tx.send(1).unwrap();

  // Fast consumer reads, freeing its own slot view
  assert_eq!(rx_fast.recv().unwrap(), 1);

  // Sender tries to send again. It should block because rx_slow hasn't read item 1.
  let send_handle = thread::spawn(move || {
    tx.send(2).unwrap(); // This should block.
  });

  thread::sleep(SHORT_TIMEOUT);
  assert!(!send_handle.is_finished(), "Sender should have blocked");

  // Slow consumer finally reads, unblocking the producer
  assert_eq!(rx_slow.recv().unwrap(), 1);

  send_handle
    .join()
    .expect("Sender panicked or was not unblocked");

  // Both can now receive the new item
  assert_eq!(rx_fast.recv().unwrap(), 2);
  assert_eq!(rx_slow.recv().unwrap(), 2);
}

#[test]
fn spmc_sync_all_receivers_drop_closes_channel() {
  let (tx, rx) = spmc::bounded(2);
  let rx2 = rx.clone();

  tx.send(1).unwrap(); // Should succeed

  drop(rx);
  drop(rx2);

  // Now that all receivers are gone, send should fail
  assert_eq!(tx.send(2), Err(fibre::error::SendError::Closed));
}

#[test]
fn spmc_sync_sender_unblocks_when_all_receivers_dropped() {
  let (tx, rx1) = spmc::bounded(1);
  let rx2 = rx1.clone();

  // Fill the buffer
  tx.send(1).unwrap();

  // Fast consumer rx1 reads its copy, but slow consumer rx2 does not
  assert_eq!(rx1.recv().unwrap(), 1);

  // Sending again blocks because rx2 is still lagging
  let handle = thread::spawn(move || tx.send(2));

  // Give the sender thread some time to park
  thread::sleep(SHORT_TIMEOUT);
  assert!(!handle.is_finished(), "Sender thread should be blocked");

  // Drop all active receivers
  drop(rx1);
  drop(rx2);

  let res = handle.join().expect("Sender thread panicked");
  assert!(
    matches!(res, Err(fibre::error::SendError::Closed)),
    "Expected Err(SendError::Closed), got {:?}",
    res
  );
}
