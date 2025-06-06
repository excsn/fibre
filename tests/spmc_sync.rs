mod common;
use common::*;

use fibre::spmc;
use std::thread;

#[test]
fn spmc_sync_spsc_smoke() {
  let (mut tx, mut rx) = spmc::channel(2);
  tx.send(10).unwrap();
  assert_eq!(rx.recv().unwrap(), 10);
}

#[test]
fn spmc_sync_try_recv() {
  let (mut tx, mut rx) = spmc::channel::<i32>(2);
  assert_eq!(rx.try_recv(), Err(fibre::error::TryRecvError::Empty));
  tx.send(1).unwrap();
  assert_eq!(rx.try_recv(), Ok(1));
  assert_eq!(rx.try_recv(), Err(fibre::error::TryRecvError::Empty));
}

#[test]
fn spmc_sync_multi_consumer() {
  let (mut tx, mut rx1) = spmc::channel(ITEMS_LOW);
  let mut rx2 = rx1.clone();
  let mut rx3 = rx1.clone();

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
  let (mut tx, mut rx_fast) = spmc::channel(1); // Capacity of 1
  let mut rx_slow = rx_fast.clone();

  // Fill the buffer
  tx.send(1).unwrap();

  // Fast consumer reads, freeing its own slot view
  assert_eq!(rx_fast.recv().unwrap(), 1);

  // Producer tries to send again. It should block because rx_slow hasn't read item 1.
  let send_handle = thread::spawn(move || {
    tx.send(2).unwrap(); // This should block.
  });

  thread::sleep(SHORT_TIMEOUT);
  assert!(!send_handle.is_finished(), "Producer should have blocked");

  // Slow consumer finally reads, unblocking the producer
  assert_eq!(rx_slow.recv().unwrap(), 1);

  send_handle
    .join()
    .expect("Producer panicked or was not unblocked");

  // Both can now receive the new item
  assert_eq!(rx_fast.recv().unwrap(), 2);
  assert_eq!(rx_slow.recv().unwrap(), 2);
}

#[test]
fn spmc_sync_all_receivers_drop_closes_channel() {
  let (mut tx, rx) = spmc::channel(2);
  let rx2 = rx.clone();

  tx.send(1).unwrap(); // Should succeed

  drop(rx);
  drop(rx2);

  // Now that all receivers are gone, send should fail
  assert_eq!(tx.send(2), Err(fibre::error::SendError::Closed));
}
