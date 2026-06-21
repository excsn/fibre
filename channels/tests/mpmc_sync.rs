// Common constants for tests can be shared.

mod common;
use common::*;

use fibre::error::{RecvError, SendError, TryRecvError, TrySendError};
use fibre::mpmc;

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering as AtomicOrdering};
use std::sync::Arc;
use std::thread;

// --- Helper Function for Sync MPMC Tests ---
fn run_sync_mpmc_test(
  num_producers: usize,
  num_consumers: usize,
  items_per_producer: usize,
  channel_capacity: usize,
) {
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

  assert_eq!(
    received_count.load(AtomicOrdering::Relaxed),
    total_items_expected
  );
  assert_eq!(
    received_items_set.lock().unwrap().len(),
    total_items_expected
  );
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
fn sync_v2_try_send_recv_various_capacities() {
  // Test both power-of-two and non-power-of-two capacities
  let capacities = [1, 2, 3, 4, 10, 16, 50, 128, 768, 1023, 1024];

  for &cap in &capacities {
    let (tx, rx) = mpmc::bounded::<usize>(cap);

    // 1. Must be initially empty
    assert_eq!(
      rx.try_recv(),
      Err(TryRecvError::Empty),
      "Failed at empty check (cap: {})",
      cap
    );

    // 2. Fill exactly to the requested capacity
    for i in 0..cap {
      assert_eq!(
        tx.try_send(i),
        Ok(()),
        "Failed to send item {} (cap: {})",
        i,
        cap
      );
    }

    // 3. The very next item MUST fail with a Full error
    match tx.try_send(cap) {
      Err(TrySendError::Full(val)) => assert_eq!(val, cap),
      res => panic!("Expected Full error at cap {}, got {:?}", cap, res),
    }

    // 4. Drain exactly the requested capacity
    for i in 0..cap {
      assert_eq!(
        rx.try_recv(),
        Ok(i),
        "Failed to recv item {} (cap: {})",
        i,
        cap
      );
    }

    // 5. Must be exactly empty again
    assert_eq!(
      rx.try_recv(),
      Err(TryRecvError::Empty),
      "Failed at final empty check (cap: {})",
      cap
    );
  }
}

#[test]
fn sync_v2_concurrent_try_send_full_spam() {
  let thread_counts = [2, 4, 8];
  let capacities = [1, 2, 3, 4, 10, 16, 50, 128, 768, 1023, 1024];
  let items_per_thread = 200000;

  for &num_threads in &thread_counts {
    for &cap in &capacities {
      let (tx, rx) = mpmc::bounded::<usize>(cap);
      let successful_sends = Arc::new(AtomicUsize::new(0));
      let failed_sends = Arc::new(AtomicUsize::new(0));

      let mut handles = Vec::new();
      let barrier = Arc::new(std::sync::Barrier::new(num_threads));

      for _ in 0..num_threads {
        let tx_clone = tx.clone();
        let barrier_clone = barrier.clone();
        let succ = successful_sends.clone();
        let fail = failed_sends.clone();

        handles.push(thread::spawn(move || {
          // Wait for all threads to be ready to maximize contention
          barrier_clone.wait();

          for i in 0..items_per_thread {
            match tx_clone.try_send(i) {
              Ok(()) => {
                succ.fetch_add(1, AtomicOrdering::Relaxed);
              }
              Err(TrySendError::Full(_)) => {
                fail.fetch_add(1, AtomicOrdering::Relaxed);
              }
              Err(_) => panic!("Unexpected error type"),
            }
          }
        }));
      }

      for h in handles {
        h.join().unwrap();
      }

      // Assertions
      let successes = successful_sends.load(AtomicOrdering::Relaxed);
      let failures = failed_sends.load(AtomicOrdering::Relaxed);

      assert_eq!(
        successes, cap,
        "Expected exactly {} successes, got {} (threads: {}, cap: {})",
        cap, successes, num_threads, cap
      );
      assert_eq!(
        failures,
        (num_threads * items_per_thread) - cap,
        "Expected exactly the rest of the items to fail"
      );
      assert_eq!(tx.len(), cap, "Channel length should equal capacity");

      // Verify the queue contains valid items up to capacity
      for _ in 0..cap {
        assert!(rx.try_recv().is_ok());
      }
      assert_eq!(rx.try_recv(), Err(TryRecvError::Empty));
    }
  }
}

#[test]
fn sync_v2_interleaved_try_send_and_send() {
  let capacities = [1, 2, 3, 4, 10, 16, 50, 128, 768, 1023, 1024];
  let num_try_senders = 4;
  let num_senders = 4;
  let items_per_thread = 200000;

  for &cap in &capacities {
    let (tx, rx) = mpmc::bounded::<usize>(cap);
    let successful_try_sends = Arc::new(AtomicUsize::new(0));
    let failed_try_sends = Arc::new(AtomicUsize::new(0));

    // +1 for the consumer thread
    let barrier = Arc::new(std::sync::Barrier::new(num_try_senders + num_senders + 1));
    let mut handles = Vec::new();

    // Spawn `try_send` threads
    for _ in 0..num_try_senders {
      let tx_clone = tx.clone();
      let barrier_clone = barrier.clone();
      let succ = successful_try_sends.clone();
      let fail = failed_try_sends.clone();

      handles.push(thread::spawn(move || {
        barrier_clone.wait();
        for i in 0..items_per_thread {
          match tx_clone.try_send(i) {
            Ok(()) => {
              succ.fetch_add(1, AtomicOrdering::Relaxed);
            }
            Err(TrySendError::Full(_)) => {
              fail.fetch_add(1, AtomicOrdering::Relaxed);
            }
            Err(_) => panic!("Unexpected error type"),
          }
        }
      }));
    }

    // Spawn `send` (blocking) threads
    for _ in 0..num_senders {
      let tx_clone = tx.clone();
      let barrier_clone = barrier.clone();

      handles.push(thread::spawn(move || {
        barrier_clone.wait();
        for i in 0..items_per_thread {
          tx_clone.send(i).unwrap(); // Will block when channel is full
        }
      }));
    }

    // Spawn Consumer thread
    let consumer_barrier = barrier.clone();
    let consumer_handle = thread::spawn(move || {
      consumer_barrier.wait();
      let mut received = 0;

      // Read until all senders drop their handles and channel empties
      while let Ok(_) = rx.recv() {
        received += 1;
        // Periodically yield to let the queue fill up, maximizing
        // contention and forcing `try_send` to fail and `send` to park.
        if received % cap.max(1) == 0 {
          std::thread::yield_now();
        }
      }
      received
    });

    drop(tx); // Drop main handle so the consumer terminates when threads finish

    for h in handles {
      h.join().unwrap();
    }

    let total_received = consumer_handle.join().unwrap();

    let try_succ = successful_try_sends.load(AtomicOrdering::Relaxed);
    let try_fail = failed_try_sends.load(AtomicOrdering::Relaxed);

    // Verify try_send outcomes
    assert_eq!(
      try_succ + try_fail,
      num_try_senders * items_per_thread,
      "Not all try_sends were accounted for"
    );

    // Verify no items were lost.
    // Total received MUST exactly equal all items from the blocking senders
    // PLUS the items that successfully made it through the try_senders.
    let expected_total = (num_senders * items_per_thread) + try_succ;
    assert_eq!(
      total_received, expected_total,
      "Mismatch in received items (cap: {})",
      cap
    );
  }
}

#[test]
fn test_mpmc_is_strictly_bounded_and_rejects_excess() {
  let capacities = [1, 2, 3, 4, 7, 15, 16, 64, 100, 1023, 1024];

  for &cap in &capacities {
    let (tx, _rx) = mpmc::bounded::<usize>(cap);

    // 1. Fill exactly to requested capacity
    for i in 0..cap {
      assert_eq!(
        tx.try_send(i),
        Ok(()),
        "Failed to fill slot {}/{} (cap: {})",
        i,
        cap,
        cap
      );
    }

    // 2. Verify internal accounting reports full
    assert_eq!(tx.len(), cap, "Length must exactly equal capacity");
    assert!(tx.is_full(), "is_full() must report true at capacity");

    // 3. INDISPUTABLE PROOF: Attempting to send ONE more item MUST bounce with Full.
    // If the queue were accidentally unbounded (e.g. an inflating VecDeque), this CAS would return Ok(()).
    let excess_item = 999999;
    match tx.try_send(excess_item) {
      Err(TrySendError::Full(returned_item)) => {
        assert_eq!(
          returned_item, excess_item,
          "Must return the exact excess item"
        );
      }
      res => panic!(
        "CRITICAL BUG: Channel completely bypassed capacity limit of {}! Returned {:?}",
        cap, res
      ),
    }

    // 4. Verify length did not inflate past capacity after the rejection
    assert_eq!(tx.len(), cap, "Queue length inflated past capacity!");
  }
}

#[test]
fn sync_v2_interleaved_send_try_send_strict_bounds_deterministic() {
  let cap = 2;
  let (tx, rx) = mpmc::bounded::<usize>(cap);

  // 1. Fill the channel
  tx.try_send(10).unwrap();
  tx.try_send(20).unwrap();
  assert_eq!(tx.len(), cap);

  // 2. Spawn blocked senders
  let tx1 = tx.clone();
  let h1 = thread::spawn(move || tx1.send(30));
  let tx2 = tx.clone();
  let h2 = thread::spawn(move || tx2.send(40));

  // Let them park
  thread::sleep(std::time::Duration::from_millis(50));

  // 3. Try to bypass capacity with try_send while senders are parked
  let excess = 999;
  match tx.try_send(excess) {
    Err(TrySendError::Full(v)) => assert_eq!(v, excess),
    res => panic!("try_send bypassed capacity! Got {:?}", res),
  }
  assert_eq!(tx.len(), cap, "Queue length inflated!");

  // 4. Consume ONE item.
  assert_eq!(rx.recv().unwrap(), 10);

  // This creates space, so ONE blocked sender will wake up and fill it.
  thread::sleep(std::time::Duration::from_millis(50));

  // 5. The channel should be full again. try_send MUST fail again.
  match tx.try_send(excess) {
    Err(TrySendError::Full(v)) => assert_eq!(v, excess),
    res => panic!("try_send bypassed capacity after handoff! Got {:?}", res),
  }
  assert_eq!(tx.len(), cap);

  // 6. Drain remaining
  assert_eq!(rx.recv().unwrap(), 20);

  // The two spawned threads race to park, so 30 and 40 can arrive in either order.
  let mut remaining = vec![rx.recv().unwrap(), rx.recv().unwrap()];
  remaining.sort();
  assert_eq!(remaining, vec![30, 40]);

  h1.join().unwrap().unwrap();
  h2.join().unwrap().unwrap();
}

#[test]
fn sync_v2_concurrent_mixed_send_strictly_bounded_chaos() {
  let cap = 5;
  let items_per_thread = 50000;
  let num_try_senders = 4;
  let num_senders = 4;
  let (tx, rx) = mpmc::bounded::<usize>(cap);

  let full_errors_thrown = Arc::new(AtomicUsize::new(0));
  let is_running = Arc::new(AtomicBool::new(true));

  let mut handles = Vec::new();

  // 1. Spawn a Watchdog thread that aggressively verifies the queue never exceeds capacity
  let watchdog_tx = tx.clone();
  let watchdog_running = is_running.clone();
  handles.push(thread::spawn(move || {
    while watchdog_running.load(AtomicOrdering::Relaxed) {
      let current_len = watchdog_tx.len();
      assert!(
        current_len <= cap,
        "FATAL: Channel breached capacity! Allowed: {}, Actual: {}",
        cap,
        current_len
      );
      std::thread::yield_now();
    }
  }));

  // 2. Spawn `try_send` spammers
  for _ in 0..num_try_senders {
    let tx_clone = tx.clone();
    let err_counter = full_errors_thrown.clone();
    handles.push(thread::spawn(move || {
      let mut i = 0;
      while i < items_per_thread {
        match tx_clone.try_send(i) {
          Ok(()) => i += 1,
          Err(TrySendError::Full(_)) => {
            err_counter.fetch_add(1, AtomicOrdering::Relaxed);
            std::thread::yield_now(); // Yield to let consumer catch up
          }
          Err(_) => panic!("Unexpected error"),
        }
      }
    }));
  }

  // 3. Spawn blocking `send` spammers
  for _ in 0..num_senders {
    let tx_clone = tx.clone();
    handles.push(thread::spawn(move || {
      for i in 0..items_per_thread {
        tx_clone.send(i).unwrap(); // Will block safely
      }
    }));
  }
  drop(tx);

  // 4. Slow consumer to force backpressure and trigger Full errors
  let mut total_received = 0;
  let expected_total = 8 * items_per_thread;

  while total_received < expected_total {
    rx.recv().unwrap();
    total_received += 1;
    // Sleep briefly to back up the queue
    if total_received % 100 == 0 {
      thread::sleep(std::time::Duration::from_millis(1));
    }
  }

  is_running.store(false, AtomicOrdering::Relaxed);

  for h in handles {
    h.join().unwrap();
  }

  // 5. Verify the queue actively threw a large number of Full errors
  // Because the consumer is throttled, the try_send threads will spin-fail.
  // We expect at least a 1:1 ratio of failures to successes on average.
  let total_full_errors = full_errors_thrown.load(AtomicOrdering::Relaxed);
  let expected_minimum_errors = num_try_senders * items_per_thread;

  assert!(
    total_full_errors >= expected_minimum_errors,
    "Expected at least {} Full errors during contention (1 per item), but got {}",
    expected_minimum_errors,
    total_full_errors
  );
}
