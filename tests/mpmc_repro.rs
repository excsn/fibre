// tests/mpmc_hang_repro.rs

use fibre::mpmc as mpmc;
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
