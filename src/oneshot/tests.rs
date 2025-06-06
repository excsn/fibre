use super::*; // Import items from oneshot::mod (Sender, Receiver, channel, errors)
use crate::error::{RecvError, TryRecvError, TrySendError}; // Specific errors
use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
use std::time::Duration;
use tokio::time::timeout; // For testing futures with timeouts

const TEST_TIMEOUT: Duration = Duration::from_secs(1);

#[tokio::test]
async fn send_recv_ok() {
  let (tx, mut rx) = channel::<String>();
  let message = "hello oneshot".to_string();

  tokio::spawn(async move {
    tx.send(message.clone()).expect("Send failed");
  });

  let received = timeout(TEST_TIMEOUT, rx.recv())
    .await
    .expect("Receive timed out")
    .unwrap();
  assert_eq!(received, "hello oneshot");
}

#[tokio::test]
async fn try_recv_before_send() {
  let (tx, mut rx) = channel::<i32>();
  assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
  drop(tx); // ensure it transitions to disconnected later
  assert!(matches!(rx.try_recv(), Err(TryRecvError::Disconnected)));
}

#[tokio::test]
async fn try_recv_after_send() {
  let (tx, mut rx) = channel::<i32>();
  tx.send(123).expect("Send failed");
  assert_eq!(rx.try_recv().unwrap(), 123);
  // Second try_recv should indicate it's taken (effectively Empty or Disconnected if senders gone)
  // Since no more senders, it should be Disconnected if state became TAKEN
  // Or Empty if state became TAKEN but sender_count was >0 (not possible here as tx consumed)
  // Our try_recv returns Empty if TAKEN and senders might exist, Disconnected if TAKEN and senders gone.
  // After tx.send(), sender_count drops to 0 on Sender::drop.
  assert!(matches!(
    rx.try_recv(),
    Err(TryRecvError::Disconnected) | Err(TryRecvError::Empty)
  ));
}

#[tokio::test]
async fn recv_after_all_senders_dropped_no_send() {
  let (tx1, mut rx) = channel::<i32>();
  let tx2 = tx1.clone();
  let tx3 = tx2.clone();

  drop(tx1);
  drop(tx2);
  drop(tx3); // All senders dropped

  match timeout(TEST_TIMEOUT, rx.recv()).await {
    Ok(Err(RecvError::Disconnected)) => {} // Expected
    res => panic!("Expected Disconnected, got {:?}", res),
  }
}

#[tokio::test]
async fn send_fails_if_receiver_dropped() {
  let (tx, rx) = channel::<String>();
  drop(rx); // Receiver dropped

  let message = "won't be sent".to_string();
  match tx.send(message.clone()) {
    Err(TrySendError::Closed(returned_message)) => {
      assert_eq!(returned_message, message);
    }
    res => panic!("Expected TrySendError::Closed, got {:?}", res),
  }
}

#[tokio::test]
async fn only_first_send_succeeds_cloned_senders() {
  let (tx1, mut rx) = channel::<i32>();
  let tx2 = tx1.clone();
  let tx3 = tx1.clone();

  // Sender 1 sends successfully
  tokio::spawn(async move {
    tx1.send(1).expect("Send 1 failed");
  });

  // Receiver gets the value from sender 1
  assert_eq!(timeout(TEST_TIMEOUT, rx.recv()).await.expect("Timeout").unwrap(), 1);

  // Sender 2 tries to send, should fail
  match tx2.send(2) {
    Err(TrySendError::Sent(val)) => assert_eq!(val, 2),
    res => panic!("Expected TrySendError::Sent from tx2, got {:?}", res),
  }

  // Sender 3 tries to send, should also fail
  match tx3.send(3) {
    Err(TrySendError::Sent(val)) => assert_eq!(val, 3),
    res => panic!("Expected TrySendError::Sent from tx3, got {:?}", res),
  }
}

#[tokio::test]
async fn receiver_dropped_after_send_value_is_dropped() {
  static DROP_COUNT: AtomicUsize = AtomicUsize::new(0);
  #[derive(Debug)]
  struct DroppableVal(String);
  impl Drop for DroppableVal {
    fn drop(&mut self) {
      println!("Dropping DroppableVal: {}", self.0);
      DROP_COUNT.fetch_add(1, AtomicOrdering::Relaxed);
    }
  }

  DROP_COUNT.store(0, AtomicOrdering::Relaxed);
  {
    let (tx, mut rx) = channel::<DroppableVal>();
    tx.send(DroppableVal("should be dropped".to_string()))
      .expect("Send failed");
    // Value is sent, now in OneShotShared::value_slot

    // Don't call rx.recv(), instead drop rx.
    // Receiver::drop should take the value from slot and drop it.
    drop(rx);
  }
  // After rx is dropped, the DroppableVal should have been dropped.
  assert_eq!(DROP_COUNT.load(AtomicOrdering::Relaxed), 1);
}

#[tokio::test]
async fn receiver_dropped_while_sender_sending_concurrently() {
  // This test is harder to make deterministic without more complex sync.
  // The idea is sender starts to send, receiver drops mid-way.
  // With current Mutex in send, this race is less likely to manifest subtly.
  // The sender will either complete send then receiver drop cleans up,
  // or sender sees receiver_dropped flag before completing send.
  let (tx, rx) = channel::<i32>();

  let sender_task = tokio::spawn(async move {
    // Simulate some work before actual send logic hits the critical part
    tokio::time::sleep(Duration::from_millis(10)).await;
    tx.send(123) // This will either be Ok or Err(Closed)
  });

  tokio::time::sleep(Duration::from_millis(5)).await; // Try to drop receiver before send completes
  drop(rx);

  match sender_task.await.unwrap() {
    Ok(()) => println!("Sender completed send (receiver likely dropped after value placed)"),
    Err(TrySendError::Closed(_)) => println!("Sender saw receiver dropped before completing send"),
    Err(e) => panic!("Unexpected send error: {:?}", e),
  }
  // No assertion on outcome, just that it doesn't deadlock or panic unexpectedly.
}

#[tokio::test]
async fn select_on_recv() {
  let (tx1, mut rx1) = channel::<i32>();
  let (_tx2, mut rx2) = channel::<i32>(); // This one won't receive anything

  tokio::spawn(async move {
    tokio::time::sleep(Duration::from_millis(50)).await;
    tx1.send(100).unwrap();
  });

  let start = std::time::Instant::now();
  tokio::select! {
      biased; // For predictability in test
      Ok(val) = rx1.recv() => {
          assert_eq!(val, 100);
          assert!(start.elapsed() >= Duration::from_millis(40)); // Ensure it waited
      }
      _ = rx2.recv() => {
          panic!("Should not have received from rx2");
      }
      _ = tokio::time::sleep(TEST_TIMEOUT) => {
          panic!("Select timed out");
      }
  }
}

#[tokio::test]
async fn sender_clones_drop_receiver_gets_disconnected() {
  let (tx_orig, mut rx) = channel::<()>();
  let mut senders = Vec::new();
  for _ in 0..5 {
    senders.push(tx_orig.clone());
  }
  drop(tx_orig); // Original sender dropped

  // Drop cloned senders one by one
  while let Some(s) = senders.pop() {
    assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty))); // Still empty, senders exist
    drop(s);
  }
  // All senders are now dropped
  assert_eq!(rx.recv().await, Err(RecvError::Disconnected));
}

#[tokio::test]
async fn send_consumes_sender() {
  let (tx, mut rx) = channel::<i32>();
  // tx.send(1); // This consumes tx
  // tx.send(2); // This would be a compile error: value used after move

  // To show it's consumed:
  let _ = tx.send(1); // tx is moved here.

  // If we wanted to check sender_count, we'd need to inspect Arc<OneShotShared>
  // but for this test, just ensuring it compiles (or doesn't for misuse) is key.
  // We check the drop behavior implicitly via other tests (all senders dropped).

  // Let's ensure receiver gets the value.
  assert_eq!(rx.recv().await.unwrap(), 1);
}

#[tokio::test]
async fn is_closed_and_is_sent_semantics() {
  let (tx1, mut rx) = channel::<i32>();
  let tx2 = tx1.clone();

  assert!(!tx1.is_closed()); // Receiver exists
  assert!(!tx1.is_sent());
  assert!(!rx.is_closed()); // Senders exist

  // Send a value
  let tx_to_send = tx1.clone(); // Clone for sending
  drop(tx1); // Drop one clone

  tx_to_send.send(123).unwrap();
  assert!(tx2.is_sent()); // Another sender clone checks
                          // rx.is_closed() might be false now if tx2 still exists, even if value is sent.
                          // is_closed for receiver means "no more values will EVER come AND none came".

  assert_eq!(rx.recv().await.unwrap(), 123);
  // After recv, is_sent should still be true (or concept of is_taken matters)
  // Let's say is_sent refers to whether the send operation has completed.
  assert!(tx2.is_sent());
  // Now, rx.is_closed should be true if tx2 is the only sender and it drops,
  // or if it was already true because send completed and no more senders.
  // This semantic needs to be precise.
  // `Receiver::is_closed` means: all senders gone AND no value was successfully sent *and not yet taken*.
  // If a value was sent and taken, the channel fulfilled its purpose.
  // If all senders drop and value was sent but not taken, `is_closed` could be false until recv or rx drop.

  drop(tx2); // Drop the last sender
             // Now, rx.is_closed() should be true if interpreted as "no more activity possible, value taken".
             // Or, if is_closed means "no value *will be* sent AND senders are gone":
             // Since a value *was* sent, is_closed (from receiver's perspective of new values) is true.
  assert!(rx.is_closed());

  // Test receiver dropped
  let (tx3, rx2) = channel::<i32>();
  assert!(!tx3.is_closed());
  drop(rx2);
  assert!(tx3.is_closed()); // Now sender sees receiver is gone
}
