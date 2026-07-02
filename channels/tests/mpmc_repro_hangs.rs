use fibre::error::SendError;
use fibre::mpmc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use tokio::time::timeout;

const TEST_TIMEOUT: Duration = Duration::from_millis(1000);

// ===========================================================================
// Test 1: Asynchronous Blocked Senders Unblocking on Receiver Drop
// ===========================================================================
#[tokio::test]
async fn test_mpmc_async_receiver_drop_unblocks_blocked_senders() {
  // Create a bounded MPMC async channel with capacity 1
  let (tx, rx) = mpmc::bounded_async::<i32>(1);

  // 1. Fill the channel to capacity.
  tx.send(100).await.unwrap();
  assert_eq!(tx.len(), 1, "Channel must be full");

  // 2. Clone and spawn 3 more senders.
  // Since the channel is full, all 3 tasks must block on the MPMC send future.
  let tx2 = tx.clone();
  let h2 = tokio::spawn(async move { tx2.send(200).await });

  let tx3 = tx.clone();
  let h3 = tokio::spawn(async move { tx3.send(300).await });

  let tx4 = tx.clone();
  let h4 = tokio::spawn(async move { tx4.send(400).await });

  // Give the spawned tasks time to run and block.
  tokio::time::sleep(Duration::from_millis(100)).await;

  println!("[TEST] Dropping receiver handle...");
  // 3. Drop the receiver.
  // This must wake up and unblock ALL waiting senders so they can exit.
  drop(rx);

  println!("[TEST] Receiver dropped. Awaiting blocked senders...");

  // 4. Await all spawned senders with a timeout.
  // If the bug is present, the senders will remain pending forever and hang.
  let r2 = timeout(TEST_TIMEOUT, h2).await;
  let r3 = timeout(TEST_TIMEOUT, h3).await;
  let r4 = timeout(TEST_TIMEOUT, h4).await;

  // Unwrap the timeouts (fails the test if a timeout occurred)
  let res2 = r2.expect("Sender 2 timed out (deadlocked)!").unwrap();
  let res3 = r3.expect("Sender 3 timed out (deadlocked)!").unwrap();
  let res4 = r4.expect("Sender 4 timed out (deadlocked)!").unwrap();

  // Ensure all senders received a Closed error rather than successfully sending
  assert!(
    matches!(res2, Err(SendError::Closed)),
    "Expected Closed error on Sender 2, got {:?}",
    res2
  );
  assert!(
    matches!(res3, Err(SendError::Closed)),
    "Expected Closed error on Sender 3, got {:?}",
    res3
  );
  assert!(
    matches!(res4, Err(SendError::Closed)),
    "Expected Closed error on Sender 4, got {:?}",
    res4
  );
  
  println!("[TEST] Async unblocking test completed successfully.");
}

// ===========================================================================
// Test 2: Synchronous Blocked Senders Unblocking on Receiver Drop
// ===========================================================================
#[test]
fn test_mpmc_sync_receiver_drop_unblocks_blocked_senders() {
  // Create a bounded MPMC sync channel with capacity 1
  let (tx, rx) = mpmc::bounded::<i32>(1);

  // 1. Fill the channel to capacity.
  tx.send(100).unwrap();
  assert_eq!(tx.len(), 1, "Channel must be full");

  // 2. Clone and spawn 2 blocking sender threads.
  // Since the channel is full, both threads must park and block.
  let tx2 = tx.clone();
  let h2 = thread::spawn(move || tx2.send(200));

  let tx3 = tx.clone();
  let h3 = thread::spawn(move || tx3.send(300));

  // Give the threads time to park.
  thread::sleep(Duration::from_millis(100));

  println!("[TEST] Dropping receiver handle...");
  // 3. Drop the receiver.
  drop(rx);

  println!("[TEST] Receiver dropped. Joining blocked threads...");

  // 4. Join the threads. If they are deadlocked, this will hang.
  // We use a helper thread to enforce a timeout on the joins.
  let main_thread = thread::current();
  let finished = Arc::new(AtomicBool::new(false));
  let finished_clone = finished.clone();

  let _timer_thread = thread::spawn(move || {
    thread::sleep(TEST_TIMEOUT);
    if !finished_clone.load(Ordering::Acquire) {
      println!("[TEST ERROR] Threads timed out! Deadlock detected.");
      main_thread.unpark();
    }
  });

  let res2 = h2.join().expect("Sender thread 2 panicked");
  let res3 = h3.join().expect("Sender thread 3 panicked");

  finished.store(true, Ordering::Release);

  assert!(
    matches!(res2, Err(SendError::Closed)),
    "Expected Closed error on Sender 2, got {:?}",
    res2
  );
  assert!(
    matches!(res3, Err(SendError::Closed)),
    "Expected Closed error on Sender 3, got {:?}",
    res3
  );

  println!("[TEST] Sync unblocking test completed successfully.");
}