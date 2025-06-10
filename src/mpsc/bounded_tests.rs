use super::*;
use crate::mpsc::{bounded, bounded_async};
use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

#[test]
fn sync_send_recv() {
  let (tx, rx) = bounded(2);
  tx.send(1).unwrap();
  tx.send(2).unwrap();
  assert!(tx.is_full());
  assert_eq!(rx.recv().unwrap(), 1);
  assert_eq!(rx.recv().unwrap(), 2);
  assert!(rx.is_empty());
}

#[test]
fn sync_try_send_full() {
  let (tx, rx) = bounded(1);
  tx.try_send(10).unwrap();
  assert!(tx.is_full());
  assert_eq!(tx.try_send(20), Err(TrySendError::Full(20)));
  drop(rx);
  assert_eq!(tx.try_send(30), Err(TrySendError::Closed(30)));
}

#[test]
fn sync_send_blocks() {
  let (tx, rx) = bounded(1);
  tx.send(1).unwrap();

  let send_handle = thread::spawn(move || {
    tx.send(2).unwrap(); // This should block
  });

  thread::sleep(Duration::from_millis(100));
  assert!(!send_handle.is_finished(), "Send should have blocked");

  assert_eq!(rx.recv().unwrap(), 1);
  send_handle.join().expect("Send thread panicked");
  assert_eq!(rx.recv().unwrap(), 2);
}

#[test]
fn sync_receiver_drop() {
  let (tx, rx) = bounded(1);
  tx.send(1).unwrap();
  drop(rx);
  // The permit for item 1 should have been released.
  // The channel is now closed to the sender.
  assert!(tx.is_closed());
  assert_eq!(tx.send(2), Err(SendError::Closed));
}

#[tokio::test]
async fn async_send_recv() {
  let (tx, rx) = bounded_async(2);
  tx.send(1).await.unwrap();
  tx.send(2).await.unwrap();
  assert!(tx.is_full());

  assert_eq!(rx.recv().await.unwrap(), 1);
  assert_eq!(rx.recv().await.unwrap(), 2);
  assert!(rx.is_empty());
}

#[tokio::test]
async fn async_send_waits() {
  let (tx, rx) = bounded_async(1);
  tx.send(1).await.unwrap();

  let send_task = tokio::spawn(async move {
    tx.send(2).await.unwrap();
  });

  tokio::time::sleep(Duration::from_millis(50)).await;
  assert!(!send_task.is_finished(), "Send task should be waiting");

  assert_eq!(rx.recv().await.unwrap(), 1);
  send_task.await.unwrap();
  assert_eq!(rx.recv().await.unwrap(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mixed_sync_send_async_recv() {
  // `bounded` returns a sync pair.
  let (tx_sync, rx_sync) = bounded(2);
  // Convert the receiver to its async version.
  let rx_async = rx_sync.to_async();

  // Use a blocking thread for the sync sender
  let send_handle = thread::spawn(move || {
    tx_sync.send(100).unwrap();
    tx_sync.send(200).unwrap();
  });

  // Now we can `.await` on the async receiver.
  assert_eq!(rx_async.recv().await.unwrap(), 100);
  assert_eq!(rx_async.recv().await.unwrap(), 200);

  send_handle.join().unwrap();
}

#[test]
fn mixed_async_send_sync_recv() {
  // `bounded` returns a sync pair.
  let (tx_sync, rx_sync) = bounded(2);
  // Convert the sender to its async version.
  let tx_async = tx_sync.to_async();

  let rt = tokio::runtime::Runtime::new().unwrap();
  rt.block_on(async {
    // Now we can `.await` on the async sender.
    tx_async.send(100).await.unwrap();
    tx_async.send(200).await.unwrap();
  });

  // The original sync receiver works as expected.
  assert_eq!(rx_sync.recv().unwrap(), 100);
  assert_eq!(rx_sync.recv().unwrap(), 200);
}

// --- New Intensity and Correctness Tests ---

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn high_contention_async_mpsc() {
  const NUM_SENDERS: usize = 100;
  const MSGS_PER_SENDER: usize = 10;
  const CAPACITY: usize = 10;
  let total_msgs = NUM_SENDERS * MSGS_PER_SENDER;

  let (tx, rx) = bounded_async(CAPACITY);
  let mut handles = Vec::new();

  // Spawn many sender tasks
  for i in 0..NUM_SENDERS {
    let tx_clone = tx.clone();
    let handle = tokio::spawn(async move {
      for j in 0..MSGS_PER_SENDER {
        let val = i * MSGS_PER_SENDER + j;
        tx_clone.send(val).await.unwrap();
      }
    });
    handles.push(handle);
  }
  // Drop the original sender
  drop(tx);

  // Spawn a receiver task
  let receiver_handle = tokio::spawn(async move {
    let mut received_values = HashSet::new();
    let mut count = 0;
    while let Ok(val) = rx.recv().await {
      assert!(received_values.insert(val), "Duplicate value received!");
      count += 1;
    }
    assert!(rx.is_closed(), "Receiver should be closed at the end");
    assert_eq!(count, total_msgs);
    received_values
  });

  // Wait for all senders to complete
  for handle in handles {
    handle.await.unwrap();
  }

  // Wait for the receiver to complete and verify the sum
  let received_set = receiver_handle.await.unwrap();
  let expected_sum: usize = (0..total_msgs).sum();
  let actual_sum: usize = received_set.iter().sum();
  assert_eq!(
    actual_sum, expected_sum,
    "Sum of received values is incorrect"
  );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn high_contention_mixed_sync_async() {
  const NUM_ASYNC_SENDERS: usize = 50;
  const NUM_SYNC_SENDERS: usize = 50;
  const MSGS_PER_SENDER: usize = 10;
  const CAPACITY: usize = 15;
  let total_msgs = (NUM_ASYNC_SENDERS + NUM_SYNC_SENDERS) * MSGS_PER_SENDER;

  let (tx, rx_sync) = bounded(CAPACITY);
  let rx = rx_sync.to_async(); // Use an async receiver

  let mut handles = Vec::new();

  // Spawn async senders
  for i in 0..NUM_ASYNC_SENDERS {
    let tx_clone = tx.clone().to_async();
    handles.push(tokio::spawn(async move {
      for j in 0..MSGS_PER_SENDER {
        let val = i * MSGS_PER_SENDER + j;
        tx_clone.send(val).await.unwrap();
      }
    }));
  }

  // Spawn sync senders in blocking threads
  let mut thread_handles = Vec::new();
  for i in 0..NUM_SYNC_SENDERS {
    let tx_clone = tx.clone();
    thread_handles.push(thread::spawn(move || {
      for j in 0..MSGS_PER_SENDER {
        let val = (i + NUM_ASYNC_SENDERS) * MSGS_PER_SENDER + j;
        tx_clone.send(val).unwrap();
      }
    }));
  }

  drop(tx); // Drop the original sender

  // Receive all messages
  let mut received_count = 0;
  let mut received_sum: usize = 0;
  while let Ok(val) = rx.recv().await {
    received_sum += val;
    received_count += 1;
  }

  // Await all tasks and threads
  for handle in handles {
    handle.await.unwrap();
  }
  for handle in thread_handles {
    handle.join().unwrap();
  }

  assert_eq!(received_count, total_msgs);
  let expected_sum: usize = (0..total_msgs).sum();
  assert_eq!(received_sum, expected_sum);
  assert!(rx.is_closed());
}

#[tokio::test]
async fn sender_unblocks_when_receiver_dropped() {
  let (tx, rx) = bounded_async(1);

  // Fill the channel
  tx.send("first").await.unwrap();
  assert!(tx.is_full());

  // This send will wait because the channel is full
  let waiter_handle = tokio::spawn(async move {
    let result = tx.send("second").await;
    assert!(
      matches!(result, Err(SendError::Closed)),
      "Sender should receive a Closed error"
    );
  });

  // Give the waiter time to block on the full channel
  tokio::time::sleep(Duration::from_millis(50)).await;
  assert!(!waiter_handle.is_finished(), "Sender should be blocked");

  // Drop the receiver, which should unblock the waiting sender
  drop(rx);

  // The waiter should now complete with an error
  waiter_handle.await.unwrap();
}

#[tokio::test]
async fn zero_capacity_channel_async_rendezvous() {
  let (tx, rx) = bounded_async::<i32>(0);
  let completed_send = Arc::new(AtomicUsize::new(0));

  let completed_send_clone = completed_send.clone();
  let sender_handle = tokio::spawn(async move {
    tx.send(1).await.unwrap();
    // This line should only be reached after the receiver has picked up the message
    completed_send_clone.store(1, Ordering::Relaxed);
  });

  // Give the sender a moment to call send() and wait
  tokio::time::sleep(Duration::from_millis(20)).await;
  assert_eq!(
    completed_send.load(Ordering::Relaxed),
    0,
    "Send should not complete before recv"
  );

  // Now receive, which should unblock the sender
  assert_eq!(rx.recv().await.unwrap(), 1);

  // Give the sender task time to run to completion
  tokio::time::sleep(Duration::from_millis(20)).await;
  assert_eq!(
    completed_send.load(Ordering::Relaxed),
    1,
    "Send should have completed after recv"
  );

  sender_handle.await.unwrap();
}

#[test]
fn zero_capacity_channel_sync_rendezvous() {
  let (tx, rx) = bounded::<()>(0);
  let tx_ready = Arc::new(std::sync::Barrier::new(2));
  let send_complete = Arc::new(std::sync::atomic::AtomicBool::new(false));

  let tx_ready_clone = tx_ready.clone();
  let send_complete_clone = send_complete.clone();
  let sender_handle = thread::spawn(move || {
    // Signal that the sender is ready to send
    tx_ready_clone.wait();
    // This will block until the receiver calls recv()
    tx.send(()).unwrap();
    send_complete_clone.store(true, Ordering::Relaxed);
  });

  // Wait for the sender to be ready to send
  tx_ready.wait();
  // Give the sender thread time to block in the send() call
  thread::sleep(Duration::from_millis(50));
  assert!(
    !send_complete.load(Ordering::Relaxed),
    "Send should not complete before recv"
  );

  // This will unblock the sender
  rx.recv().unwrap();

  sender_handle.join().unwrap();
  assert!(
    send_complete.load(Ordering::Relaxed),
    "Send should be complete after recv"
  );
}
