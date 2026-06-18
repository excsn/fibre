mod common;
use common::*;

use fibre::error::{RecvError, SendError, TryRecvError, TrySendError};
use fibre::mpmc;

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
        assert!(
          received_set_clone.lock().await.insert(item),
          "Duplicate item received!"
        );
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

  assert_eq!(
    received_count.load(AtomicOrdering::Relaxed),
    total_items_expected
  );
  assert_eq!(received_items_set.lock().await.len(), total_items_expected);
}

// --- Async MPMC Test Cases ---

#[cfg(not(miri))]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn async_v2_1p_1c_basic() {
  run_async_mpmc_test(1, 1, ITEMS_HIGH, 16).await;
}

#[cfg(miri)]
#[test]
fn async_v2_mp_1c_basic() {
  let rt = tokio::runtime::Builder::new_multi_thread().build().unwrap();

  rt.block_on(async {
    run_async_mpmc_test(4, 1, ITEMS_MEDIUM, 16).await;
  });
}

#[cfg(not(miri))]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn async_v2_mp_1c_basic() {
  run_async_mpmc_test(4, 1, ITEMS_MEDIUM, 16).await;
}

#[cfg(not(miri))]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn async_v2_1p_mc_basic() {
  run_async_mpmc_test(1, 4, ITEMS_HIGH, 16).await;
}

#[cfg(not(miri))]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn async_v2_mp_mc_contention() {
  run_async_mpmc_test(4, 4, ITEMS_HIGH, 4).await; // High contention
}

#[cfg(not(miri))]
#[tokio::test]
async fn async_v2_unbounded_channel() {
  let (tx, rx) = mpmc::unbounded_async();
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

#[cfg(not(miri))]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn async_v2_rendezvous_channel() {
  run_async_mpmc_test(2, 2, ITEMS_MEDIUM, 0).await;
}

#[cfg(not(miri))]
#[tokio::test]
async fn async_v2_drop_producer_signals_disconnect() {
  let (tx, rx) = mpmc::bounded_async::<i32>(5);
  let tx2 = tx.clone();

  tx.send(1).await.unwrap();
  drop(tx);

  tx2.send(2).await.unwrap();
  drop(tx2);

  assert_eq!(rx.recv().await.unwrap(), 1);
  assert_eq!(rx.recv().await.unwrap(), 2);
  assert_eq!(rx.recv().await, Err(RecvError::Disconnected));
}

#[cfg(not(miri))]
#[tokio::test]
async fn async_v2_drop_receiver_signals_closed() {
  let (tx, rx) = mpmc::bounded_async::<i32>(5);
  let rx2 = rx.clone();

  drop(rx);
  drop(rx2);

  assert_eq!(tx.send(1).await, Err(SendError::Closed));
}

#[cfg(not(miri))]
#[tokio::test]
async fn async_v2_select_compatibility() {
  let (tx1, rx1) = mpmc::bounded_async(1);
  let (tx2, rx2) = mpmc::bounded_async(1);

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

#[cfg(not(miri))]
#[tokio::test]
async fn async_v2_try_send_recv_various_capacities() {
  // TrySend and TryRecv are synchronous non-blocking calls, so we test
  // them thoroughly on the async handles as well.
  let capacities = [1, 2, 3, 4, 10, 16, 50, 128, 768, 1023, 1024];

  for &cap in &capacities {
    let (tx, rx) = mpmc::bounded_async::<usize>(cap);

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

#[cfg(not(miri))]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn async_v2_concurrent_try_send_full_spam() {
  use fibre::error::{TryRecvError, TrySendError};
  use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
  use std::sync::Arc;

  let task_counts = [2, 4, 8];
  let capacities = [1, 2, 3, 4, 10, 16, 50, 128, 768, 1023, 1024];
  let items_per_task = 200000;

  for &num_tasks in &task_counts {
    for &cap in &capacities {
      let (tx, rx) = mpmc::bounded_async::<usize>(cap);
      let successful_sends = Arc::new(AtomicUsize::new(0));
      let failed_sends = Arc::new(AtomicUsize::new(0));

      let mut handles = Vec::new();
      let barrier = Arc::new(tokio::sync::Barrier::new(num_tasks));

      for _ in 0..num_tasks {
        let tx_clone = tx.clone();
        let barrier_clone = barrier.clone();
        let succ = successful_sends.clone();
        let fail = failed_sends.clone();

        handles.push(tokio::spawn(async move {
          // Wait for all tasks to be ready to maximize contention
          barrier_clone.wait().await;

          for i in 0..items_per_task {
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
        h.await.unwrap();
      }

      // Assertions
      let successes = successful_sends.load(AtomicOrdering::Relaxed);
      let failures = failed_sends.load(AtomicOrdering::Relaxed);

      assert_eq!(
        successes, cap,
        "Expected exactly {} successes, got {} (tasks: {}, cap: {})",
        cap, successes, num_tasks, cap
      );
      assert_eq!(
        failures,
        (num_tasks * items_per_task) - cap,
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

#[cfg(not(miri))]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn async_v2_interleaved_try_send_and_send() {
  use fibre::error::TrySendError;
  use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
  use std::sync::Arc;

  let capacities = [1, 2, 3, 4, 10, 16, 50, 128, 768, 1023, 1024];
  let num_try_senders = 4;
  let num_senders = 4;
  let items_per_task = 200000;

  for &cap in &capacities {
    let (tx, rx) = mpmc::bounded_async::<usize>(cap);
    let successful_try_sends = Arc::new(AtomicUsize::new(0));
    let failed_try_sends = Arc::new(AtomicUsize::new(0));

    // +1 for the consumer task
    let barrier = Arc::new(tokio::sync::Barrier::new(num_try_senders + num_senders + 1));
    let mut handles = Vec::new();

    // Spawn `try_send` tasks
    for _ in 0..num_try_senders {
      let tx_clone = tx.clone();
      let barrier_clone = barrier.clone();
      let succ = successful_try_sends.clone();
      let fail = failed_try_sends.clone();

      handles.push(tokio::spawn(async move {
        barrier_clone.wait().await;
        for i in 0..items_per_task {
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

    // Spawn `send` (awaiting) tasks
    for _ in 0..num_senders {
      let tx_clone = tx.clone();
      let barrier_clone = barrier.clone();

      handles.push(tokio::spawn(async move {
        barrier_clone.wait().await;
        for i in 0..items_per_task {
          tx_clone.send(i).await.unwrap(); // Will await when channel is full
        }
      }));
    }

    // Spawn Consumer task
    let consumer_barrier = barrier.clone();
    let consumer_handle = tokio::spawn(async move {
      consumer_barrier.wait().await;
      let mut received = 0;

      // Read until all senders drop their handles and channel empties
      while let Ok(_) = rx.recv().await {
        received += 1;
        // Periodically yield to let the queue fill up, maximizing
        // contention and forcing `try_send` to fail and `send` to park.
        if received % cap.max(1) == 0 {
          tokio::task::yield_now().await;
        }
      }
      received
    });

    drop(tx); // Drop main handle so the consumer terminates when tasks finish

    for h in handles {
      h.await.unwrap();
    }

    let total_received = consumer_handle.await.unwrap();

    let try_succ = successful_try_sends.load(AtomicOrdering::Relaxed);
    let try_fail = failed_try_sends.load(AtomicOrdering::Relaxed);

    // Verify try_send outcomes
    assert_eq!(
      try_succ + try_fail,
      num_try_senders * items_per_task,
      "Not all try_sends were accounted for"
    );

    // Verify no items were lost.
    let expected_total = (num_senders * items_per_task) + try_succ;
    assert_eq!(
      total_received, expected_total,
      "Mismatch in received items (cap: {})",
      cap
    );
  }
}

#[cfg(not(miri))]
#[tokio::test]
async fn async_v2_interleaved_send_try_send_strict_bounds_deterministic() {
  let cap = 2;
  let (tx, rx) = mpmc::bounded_async::<usize>(cap);

  // 1. Fill the channel
  tx.try_send(10).unwrap();
  tx.try_send(20).unwrap();
  assert_eq!(tx.len(), cap);

  // 2. Spawn blocked senders
  let tx1 = tx.clone();
  let h1 = tokio::spawn(async move { tx1.send(30).await });
  let tx2 = tx.clone();
  let h2 = tokio::spawn(async move { tx2.send(40).await });

  // Let them park
  tokio::time::sleep(std::time::Duration::from_millis(50)).await;

  // 3. Try to bypass capacity with try_send while senders are parked
  let excess = 999;
  match tx.try_send(excess) {
    Err(fibre::error::TrySendError::Full(v)) => assert_eq!(v, excess),
    res => panic!("try_send bypassed capacity! Got {:?}", res),
  }
  assert_eq!(tx.len(), cap, "Queue length inflated!");

  // 4. Consume ONE item.
  assert_eq!(rx.recv().await.unwrap(), 10);

  // This creates space, so ONE blocked sender will wake up and fill it.
  tokio::time::sleep(std::time::Duration::from_millis(50)).await;

  // 5. The channel should be full again. try_send MUST fail again.
  match tx.try_send(excess) {
    Err(fibre::error::TrySendError::Full(v)) => assert_eq!(v, excess),
    res => panic!("try_send bypassed capacity after handoff! Got {:?}", res),
  }
  assert_eq!(tx.len(), cap);

  // 6. Drain remaining
  assert_eq!(rx.recv().await.unwrap(), 20);

  // The two spawned tasks race to park, so 30 and 40 can arrive in either order.
  let mut remaining = vec![rx.recv().await.unwrap(), rx.recv().await.unwrap()];
  remaining.sort();
  assert_eq!(remaining, vec![30, 40]);

  h1.await.unwrap().unwrap();
  h2.await.unwrap().unwrap();
}

#[cfg(not(miri))]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn async_v2_concurrent_mixed_send_strictly_bounded_chaos() {
  use std::sync::atomic::AtomicBool;

  use fibre::error::TrySendError;

  let cap = 5;
  let items_per_task = 50000;
  let num_try_senders = 4;
  let num_senders = 4;
  let (tx, rx) = mpmc::bounded_async::<usize>(cap);

  let full_errors_thrown = Arc::new(AtomicUsize::new(0));
  let is_running = Arc::new(AtomicBool::new(true));

  let mut handles = Vec::new();

  // 1. Spawn a Watchdog task that aggressively verifies the queue never exceeds capacity
  let watchdog_tx = tx.clone();
  let watchdog_running = is_running.clone();
  handles.push(tokio::spawn(async move {
    while watchdog_running.load(AtomicOrdering::Relaxed) {
      let current_len = watchdog_tx.len();
      assert!(
        current_len <= cap,
        "FATAL: Channel breached capacity! Allowed: {}, Actual: {}",
        cap,
        current_len
      );
      tokio::task::yield_now().await;
    }
  }));

  // 2. Spawn `try_send` spammers
  for _ in 0..num_try_senders {
    let tx_clone = tx.clone();
    let err_counter = full_errors_thrown.clone();
    handles.push(tokio::spawn(async move {
      let mut i = 0;
      while i < items_per_task {
        match tx_clone.try_send(i) {
          Ok(()) => i += 1,
          Err(TrySendError::Full(_)) => {
            err_counter.fetch_add(1, AtomicOrdering::Relaxed);
            tokio::task::yield_now().await; // Yield to let consumer catch up
          }
          Err(_) => panic!("Unexpected error"),
        }
      }
    }));
  }

  // 3. Spawn awaiting `send` spammers
  for _ in 0..num_senders {
    let tx_clone = tx.clone();
    handles.push(tokio::spawn(async move {
      for i in 0..items_per_task {
        tx_clone.send(i).await.unwrap(); // Will park safely
      }
    }));
  }
  drop(tx);

  // 4. Slow consumer to force backpressure and trigger Full errors
  let mut total_received = 0;
  let expected_total = 8 * items_per_task;

  while total_received < expected_total {
    rx.recv().await.unwrap();
    total_received += 1;
    // Sleep briefly to back up the queue
    if total_received % 100 == 0 {
      tokio::time::sleep(std::time::Duration::from_millis(1)).await;
    }
  }

  is_running.store(false, AtomicOrdering::Relaxed);

  for h in handles {
    h.await.unwrap();
  }

  // 5. Verify the queue actively threw a large number of Full errors
  // Because the consumer is throttled, the try_send tasks will spin-fail.
  // We expect at least a 1:1 ratio of failures to successes on average.
  let total_full_errors = full_errors_thrown.load(AtomicOrdering::Relaxed);
  let expected_minimum_errors = num_try_senders * items_per_task;

  assert!(
    total_full_errors >= expected_minimum_errors,
    "Expected at least {} Full errors during contention (1 per item), but got {}",
    expected_minimum_errors,
    total_full_errors
  );
}
