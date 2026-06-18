use fibre::mpmc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use tokio::time::sleep;

#[test]
fn sync_v2_spsc_contention_hang_repro() {
  let (tx, rx) = mpmc::bounded(4);
  #[cfg(not(miri))]
  let total_items = 100_000;
  #[cfg(miri)]
  let total_items = 100;

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
#[cfg(not(miri))]
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

#[cfg(not(miri))]
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

#[cfg(not(miri))]
#[tokio::test]
async fn reproduce_fibre_silent_drop() {
  const HWM: usize = 1000;
  const TOTAL_SENDS: u64 = 500_000;

  // 1. Initialize the bounded MPMC channel
  let (tx, rx) = fibre::mpmc::bounded_async::<usize>(HWM);

  let tx_count = Arc::new(AtomicU64::new(0));
  let rx_count = Arc::new(AtomicU64::new(0));
  let producer_done = Arc::new(AtomicBool::new(false));

  // 2. Spawn a fast producer thread/task
  let producer_done_clone = producer_done.clone();
  let tx_clone = tx.clone();
  let tx_count_clone = tx_count.clone();
  let producer = tokio::spawn(async move {
    for i in 0..TOTAL_SENDS {
      match tx_clone.send(i as usize).await {
        Ok(()) => {
          tx_count_clone.fetch_add(1, Ordering::Relaxed);
        }
        Err(_) => break,
      }
    }
    producer_done_clone.store(true, Ordering::SeqCst);
  });

  // 3. Spawn a slow, batching consumer task
  let producer_done_for_consumer = producer_done.clone();
  let rx_count_clone = rx_count.clone();
  let rx_clone = rx.clone();
  let consumer = tokio::spawn(async move {
    let mut batch = Vec::new();
    loop {
      // Slow down consumption to force the channel to stay full
      sleep(Duration::from_micros(100)).await;

      match rx_clone.try_recv_batch_mut(&mut batch, 32) {
        Ok(count) => {
          if count > 0 {
            rx_count_clone.fetch_add(count as u64, Ordering::Relaxed);
            batch.clear();
          }
        }
        Err(fibre::TryRecvError::Disconnected) => break,
        Err(fibre::TryRecvError::Empty) => {
          if producer_done_for_consumer.load(Ordering::SeqCst) {
            break;
          }
        }
      }
    }
  });

  // 4. Wait for both to complete
  let _ = tokio::join!(producer, consumer);

  let sent = tx_count.load(Ordering::SeqCst);
  let received = rx_count.load(Ordering::SeqCst);
  let remaining = rx.len();

  println!(
    "[TEST RESULT] Sent: {} | Received: {} | Remaining in Queue: {} | Discarded: {}",
    sent,
    received,
    remaining,
    sent.saturating_sub(received + remaining as u64)
  );

  assert_eq!(
    sent,
    received + remaining as u64,
    "Channel silently discarded messages!"
  );
}

// Simple LCG pseudo-random generator with zero external dependencies
fn pseudo_random(seed: &mut u64) -> u64 {
  *seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
  *seed
}

#[cfg(not(miri))]
#[tokio::test]
async fn stress_test_mpmc_contention() {
  const CAPACITY: usize = 1000;
  const NUM_PEERS: usize = 8;
  const ITEMS_PER_SENDER: u64 = 5_000_000;
  const TOTAL_EXPECTED: u64 = ITEMS_PER_SENDER * NUM_PEERS as u64;

  let (tx, rx) = fibre::mpmc::bounded_async::<usize>(CAPACITY);

  let global_sent = Arc::new(AtomicU64::new(0));
  let global_received = Arc::new(AtomicU64::new(0));
  let senders_done = Arc::new(AtomicBool::new(false));

  let mut sender_handles = Vec::with_capacity(NUM_PEERS);
  let mut receiver_handles = Vec::with_capacity(NUM_PEERS);

  // 1. Spawn 8 parallel senders
  for sender_id in 0..NUM_PEERS {
    let tx_clone = tx.clone();
    let sent_counter = global_sent.clone();

    sender_handles.push(tokio::spawn(async move {
      let mut seed = (sender_id + 1) as u64;

      for i in 0..ITEMS_PER_SENDER {
        let item = (sender_id * 1_000_000) as usize + i as usize;

        let use_fast_path = (pseudo_random(&mut seed) % 2) == 0;

        if use_fast_path {
          match tx_clone.try_send(item) {
            Ok(()) => {
              sent_counter.fetch_add(1, Ordering::Relaxed);
            }
            Err(fibre::TrySendError::Full(returned_item)) => {
              match tx_clone.send(returned_item).await {
                Ok(()) => {
                  sent_counter.fetch_add(1, Ordering::Relaxed);
                }
                Err(_) => break, // Channel closed
              }
            }
            Err(fibre::TrySendError::Closed(_)) => break,
            Err(_) => unreachable!(),
          }
        } else {
          match tx_clone.send(item).await {
            Ok(()) => {
              sent_counter.fetch_add(1, Ordering::Relaxed);
            }
            Err(_) => break,
          }
        }
      }
    }));
  }

  // 2. Spawn 8 parallel receivers
  for receiver_id in 0..NUM_PEERS {
    let rx_clone = rx.clone();
    let recv_counter = global_received.clone();
    let senders_done_clone = senders_done.clone();

    receiver_handles.push(tokio::spawn(async move {
      let mut seed = (receiver_id + 99) as u64;
      let mut batch = Vec::with_capacity(32);

      loop {
        let use_try_recv = (pseudo_random(&mut seed) % 2) == 0;

        if use_try_recv {
          match rx_clone.try_recv_batch_mut(&mut batch, 32) {
            Ok(count) => {
              if count > 0 {
                recv_counter.fetch_add(count as u64, Ordering::Relaxed);
                batch.clear();
              } else {
                if senders_done_clone.load(Ordering::SeqCst) && rx_clone.is_empty() {
                  break;
                }
                sleep(Duration::from_micros(10)).await;
              }
            }
            Err(fibre::TryRecvError::Empty) => {
              if senders_done_clone.load(Ordering::SeqCst) && rx_clone.is_empty() {
                break;
              }
              sleep(Duration::from_micros(10)).await;
            }
            Err(fibre::TryRecvError::Disconnected) => break,
          }
        } else {
          match rx_clone.recv().await {
            Ok(_item) => {
              recv_counter.fetch_add(1, Ordering::Relaxed);
            }
            Err(_) => break, // Breaks cleanly when the channel is closed
          }
        }
      }
    }));
  }

  // 3. Drop the main thread's tx handle immediately
  // This allows the channel to close automatically when the last sender task finishes.
  drop(tx);

  // 4. Await all senders to finish
  for handle in sender_handles {
    handle.await.unwrap();
  }
  senders_done.store(true, Ordering::SeqCst);

  // 5. Await all receivers to finish
  for handle in receiver_handles {
    handle.await.unwrap();
  }

  // Final assertions and report
  let msgs_sent = global_sent.load(Ordering::SeqCst);
  let msgs_recv = global_received.load(Ordering::SeqCst);
  let remaining = rx.len();

  println!(
    "[STRESS TEST RESULT] Total Expected: {} | Sent: {} | Received: {} | Remaining in Queue: {} | Discarded: {}",
    TOTAL_EXPECTED,
    msgs_sent,
    msgs_recv,
    remaining,
    msgs_sent.saturating_sub(msgs_recv + remaining as u64)
  );

  assert_eq!(
    msgs_sent, TOTAL_EXPECTED,
    "Not all items were successfully sent!"
  );

  assert_eq!(
    msgs_sent,
    msgs_recv + remaining as u64,
    "Channel silently discarded messages under high contention!"
  );
}

// ===========================================================================
// Test 3: Lost Wakeup on Exact-Capacity Batch Receive (Async)
// ===========================================================================
#[cfg(not(miri))]
#[tokio::test]
async fn test_mpmc_async_batch_recv_lost_wakeup() {
  use tokio::time::timeout;

  let cap = 4;
  let (tx, rx) = mpmc::bounded_async::<i32>(cap);

  // 1. Completely fill the channel
  for i in 0..cap {
    tx.send(i as i32).await.unwrap();
  }

  // 2. Spawn a sender that will park because the channel is full
  let tx_clone = tx.clone();
  let blocked_send = tokio::spawn(async move {
    tx_clone.send(99).await.unwrap();
  });

  // Give the sender time to firmly park in the waiting queue
  tokio::time::sleep(Duration::from_millis(50)).await;

  // 3. Receive exactly `cap` items in a single batch.
  // In the buggy code, `from_buffer == max` AND `got == max`, so the wake loop is bypassed!
  let mut out = Vec::new();
  let n = rx.try_recv_batch_mut(&mut out, cap).unwrap();
  assert_eq!(n, cap, "Should have drained the exact capacity");

  // 4. The blocked sender MUST wake up and send 99.
  // If the bug is present, this timeout will expire and panic.
  match timeout(Duration::from_millis(500), blocked_send).await {
    Ok(_) => {
      println!("[TEST] Sender successfully unblocked after batch receive.");
    }
    Err(_) => {
      panic!("REGRESSION/BUG: Async Sender remained permanently blocked after exact-capacity batch receive freed space!");
    }
  }
}

// ===========================================================================
// Test 4: Lost Wakeup on Exact-Capacity Batch Receive (Sync)
// ===========================================================================
#[test]
#[cfg(not(miri))]
fn test_mpmc_sync_batch_recv_lost_wakeup() {
  let cap = 4;
  let (tx, rx) = mpmc::bounded::<i32>(cap);

  // 1. Completely fill the channel
  for i in 0..cap {
    tx.try_send(i as i32).unwrap();
  }

  // 2. Spawn a sender that will park because the channel is full
  let tx_clone = tx.clone();
  let handle = thread::spawn(move || {
    tx_clone.send(99).unwrap();
  });

  // Give the sender time to firmly park in the waiting queue
  thread::sleep(Duration::from_millis(50));

  // 3. Receive exactly `cap` items in a single batch.
  let mut out = Vec::new();
  let n = rx.try_recv_batch_mut(&mut out, cap).unwrap();
  assert_eq!(n, cap, "Should have drained the exact capacity");

  // 4. Wait for the sender thread. Use a channel watchdog to prevent infinite test suite hangs.
  let (done_tx, done_rx) = std::sync::mpsc::channel();
  thread::spawn(move || {
    let _ = handle.join();
    let _ = done_tx.send(());
  });

  if done_rx.recv_timeout(Duration::from_millis(500)).is_err() {
    panic!("REGRESSION/BUG: Sync Sender remained permanently blocked after exact-capacity batch receive freed space!");
  } else {
    println!("[TEST] Sync Sender successfully unblocked after batch receive.");
  }
}
