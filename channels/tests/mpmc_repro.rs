// tests/mpmc_hang_repro.rs

use fibre::mpmc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

#[test]
fn sync_v2_spsc_contention_hang_repro() {
  let (tx, rx) = mpmc::bounded(4);
  let total_items = 100_000;

  // A flag to signal the main thread that the test is done, to avoid a race
  // where the main thread exits before the assertion in the consumer fails.
  let test_finished = Arc::new(AtomicBool::new(false));
  let test_finished_clone = test_finished.clone();

  let producer_handle = thread::spawn(move || {
    for i in 0..total_items {
      if tx.send(i).is_err() {
        // If send fails, the receiver must have dropped, so we can stop.
        break;
      }
    }
  });

  let consumer_handle = thread::spawn(move || {
    for i in 0..total_items {
      match rx.recv() {
        Ok(item) => {
          // Check for ordering, which also helps validate correctness.
          assert_eq!(item, i, "Received item out of order!");
        }
        Err(_) => {
          // If we get an error before receiving all items, that's a failure.
          panic!(
            "Receiver disconnected before receiving all items. Expected {}, got {}",
            total_items, i
          );
        }
      }
    }
    // Signal that we are done.
    test_finished_clone.store(true, Ordering::SeqCst);
  });

  // Wait for a reasonable time. If it hangs, this will fail.
  // The main thread will poll the `test_finished` flag.
  let start = std::time::Instant::now();
  while !test_finished.load(Ordering::SeqCst) {
    if start.elapsed() > Duration::from_secs(10) {
      // To prevent the test suite from hanging forever, we join the threads
      // which will likely just block, but we then panic with a timeout message.
      // In a real CI, the test runner would kill this after a timeout anyway.
      panic!("Test timed out after 10 seconds. Likely deadlock or livelock.");
    }
    thread::sleep(Duration::from_millis(100));
  }

  // If we reach here, the consumer finished successfully. Join the threads.
  producer_handle.join().expect("Sender panicked");
  consumer_handle.join().expect("Receiver panicked");
}

#[test]
fn repro_sync_timeout_capacity_bypass() {
  let (tx, rx) = fibre::mpmc::bounded::<i32>(1); // Strict capacity of 1

  // 1. Generate 5 abandoned waiters via timeout
  let mut handles = vec![];
  for _ in 0..5 {
    let rx_clone = rx.clone();
    handles.push(std::thread::spawn(move || {
      // Wait just long enough to time out and abandon the waiter
      let _ = rx_clone.recv_timeout(std::time::Duration::from_millis(5));
    }));
  }
  for h in handles {
    h.join().unwrap();
  }

  // 3. Prove the capacity constraint is broken
  println!("Channel Capacity: {}", tx.capacity().unwrap());
  println!("Actual Items in Queue: {}", tx.len());
  
  // 2. After the fix, all timed-out waiters must have been cleaned up.
  // A capacity=1 channel must reject a second item.
  assert_eq!(tx.try_send(0), Ok(()), "First send must succeed");
  assert!(
    tx.try_send(1).is_err(),
    "Second send must be rejected: capacity=1 must be respected after timeout cleanup"
  );
  assert_eq!(tx.len(), 1, "Queue length must equal capacity");
}

#[tokio::test]
async fn repro_async_rendezvous_payload_leak() {
  let (tx, _rx) = fibre::mpmc::bounded_async::<std::sync::Arc<()>>(0); // Rendezvous

  // 1. Create a payload we can track
  let payload = std::sync::Arc::new(());
  let weak_ref = std::sync::Arc::downgrade(&payload);

  // 2. Start a send, but cancel it via timeout
  let payload_clone = payload.clone();
  let _ = tokio::time::timeout(std::time::Duration::from_millis(10), tx.send(payload_clone)).await;

  // 3. The SendFuture was cancelled and dropped.
  // Drop our local reference to the payload.
  drop(payload);

  // 4. Prove the memory leak
  // If the channel handled cancellation correctly, the weak reference should
  // be dead (upgrade returns None) because the payload should have been dropped.
  let is_leaked = weak_ref.upgrade().is_some();

  assert!(
    !is_leaked,
    "Bug: Payload was not cleanly dropped upon future cancellation!"
  );
}
