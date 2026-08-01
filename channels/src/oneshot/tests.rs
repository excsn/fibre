use super::*;
use crate::error::{RecvError, TryRecvError, TrySendError};

use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::timeout;

const TEST_TIMEOUT: Duration = Duration::from_secs(1);

#[tokio::test]
async fn send_recv_ok() {
  let (tx, rx) = oneshot::<String>();
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
  let (tx, rx) = oneshot::<i32>();
  assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
  drop(tx); // ensure it transitions to disconnected later
  assert!(matches!(rx.try_recv(), Err(TryRecvError::Disconnected)));
}

#[tokio::test]
async fn try_recv_after_send() {
  let (tx, rx) = oneshot::<i32>();
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
  let (tx1, rx) = oneshot::<i32>();
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
  let (tx, rx) = oneshot::<String>();
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
  let (tx1, rx) = oneshot::<i32>();
  let tx2 = tx1.clone();
  let tx3 = tx1.clone();

  // Sender 1 sends successfully
  tokio::spawn(async move {
    tx1.send(1).expect("Send 1 failed");
  });

  // Receiver gets the value from sender 1
  assert_eq!(
    timeout(TEST_TIMEOUT, rx.recv())
      .await
      .expect("Timeout")
      .unwrap(),
    1
  );

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
    let (tx, rx) = oneshot::<DroppableVal>();
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
  let (tx, rx) = oneshot::<i32>();

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
  let (tx1, rx1) = oneshot::<i32>();
  let (_tx2, rx2) = oneshot::<i32>(); // This one won't receive anything

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
  let (tx_orig, rx) = oneshot::<()>();
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
  let (tx, rx) = oneshot::<i32>();
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
  let (tx1, rx) = oneshot::<i32>();
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
  let (tx3, rx2) = oneshot::<i32>();
  assert!(!tx3.is_closed());
  drop(rx2);
  assert!(tx3.is_closed()); // Now sender sees receiver is gone
}

#[test]
fn test_oneshot_drop_race_leak() {
  use std::sync::atomic::{AtomicUsize, Ordering};
  use std::sync::Arc;
  use std::thread;

  // Track how many times our custom value is dropped.
  struct DropTracker {
    counter: Arc<AtomicUsize>,
  }

  impl Drop for DropTracker {
    fn drop(&mut self) {
      self.counter.fetch_add(1, Ordering::SeqCst);
    }
  }

  let drop_counter = Arc::new(AtomicUsize::new(0));
  let tracked_value = DropTracker {
    counter: Arc::clone(&drop_counter),
  };

  let (tx, rx) = oneshot::<DropTracker>();

  let shared = Arc::clone(&tx.shared);

  // Hold the sender between its WRITING claim and its SENT publish so the
  // receiver can drop mid-write.
  shared.hold_publish.store(true, Ordering::Release);

  let sender_thread = thread::spawn(move || {
    let _ = tx.send(tracked_value);
  });

  while shared.test_phase() != super::core::STATE_WRITING {
    thread::yield_now();
  }

  drop(rx);

  shared.hold_publish.store(false, Ordering::Release);

  sender_thread.join().unwrap();

  // 7. At this point, both the Sender and Receiver handles have been dropped,
  // and the shared state has been deallocated.
  assert_eq!(
    drop_counter.load(Ordering::SeqCst),
    1,
    "The value inside the oneshot channel was leaked!"
  );
}

#[test]
fn test_oneshot_sender_count_underflow() {
  let (tx, rx) = oneshot::<i32>();
  let shared = Arc::clone(&tx.shared);

  // 1. Drop the receiver to trigger the `receiver_dropped` state
  drop(rx);

  // 2. Clone the sender.
  let tx_clone = tx.clone();

  // Verify that the count correctly incremented to 2
  assert_eq!(shared.test_sender_count(), 2);

  // 3. Drop original sender (correctly decrements count from 2 to 1)
  drop(tx);
  assert_eq!(shared.test_sender_count(), 1);

  // 4. Drop the cloned sender (correctly decrements count from 1 to 0, no underflow!)
  drop(tx_clone);

  let final_count = shared.test_sender_count();
  assert_eq!(
    final_count, 0,
    "sender_count underflowed to {}!",
    final_count
  );
}

/// `try_recv` decides "disconnected" from a stale state snapshot.
///
/// It loads `state` once at the top of the function, then, in the `EMPTY` branch, pairs
/// that snapshot with a *fresh* `sender_count` load. If a sender completes its whole
/// sequence in that window (CAS `EMPTY`->`WRITING`, write the value, swap to `SENT`, then
/// drop the last `Sender` so the count reaches 0), the receiver compares a stale `EMPTY`
/// against a current count of 0 and reports `Disconnected` while the value sits in
/// `STATE_SENT`. The `compare_exchange(EMPTY, CLOSED)` on the way out fails, correctly,
/// but its result is discarded, so nothing catches the contradiction.
///
/// The window is a few instructions wide, so this spins on `try_recv` to hit it.
#[test]
fn try_recv_never_reports_disconnected_after_a_successful_send() {
  use std::sync::atomic::{AtomicUsize, Ordering};
  use std::sync::Arc;
  use std::thread;

  const ROUNDS: usize = 50_000;

  let (work_tx, work_rx) = std::sync::mpsc::channel::<Sender<u64>>();
  let sends_ok = Arc::new(AtomicUsize::new(0));

  let sender_thread = {
    let sends_ok = Arc::clone(&sends_ok);
    thread::spawn(move || {
      while let Ok(tx) = work_rx.recv() {
        if tx.send(1).is_ok() {
          sends_ok.fetch_add(1, Ordering::Relaxed);
        }
      }
    })
  };

  let mut false_disconnects = 0usize;
  for _ in 0..ROUNDS {
    let (tx, rx) = oneshot::<u64>();
    work_tx.send(tx).unwrap();
    loop {
      match rx.try_recv() {
        Ok(_) => break,
        Err(TryRecvError::Empty) => std::hint::spin_loop(),
        Err(TryRecvError::Disconnected) => {
          false_disconnects += 1;
          break;
        }
      }
    }
  }
  drop(work_tx);
  sender_thread.join().unwrap();

  let sent = sends_ok.load(Ordering::SeqCst);
  assert_eq!(sent, ROUNDS, "a send failed, so the receives are not conclusive");
  assert_eq!(
    false_disconnects, 0,
    "{false_disconnects} of {ROUNDS} receives reported Disconnected even though every send returned Ok"
  );
}

mod exclusive {
  use super::super::exclusive;
  use crate::error::{RecvError, TryRecvError, TrySendError};

  use std::sync::atomic::{AtomicUsize, Ordering};
  use std::sync::Arc;
  use std::time::Duration;
  use tokio::time::timeout;

  const TEST_TIMEOUT: Duration = Duration::from_secs(1);

  struct DropTracker(Arc<AtomicUsize>);
  impl Drop for DropTracker {
    fn drop(&mut self) {
      self.0.fetch_add(1, Ordering::SeqCst);
    }
  }

  #[tokio::test]
  async fn send_recv_ok() {
    let (tx, mut rx) = exclusive::<String>();

    tokio::spawn(async move {
      tx.send("hello exclusive".to_string()).expect("Send failed");
    });

    let received = timeout(TEST_TIMEOUT, rx.recv())
      .await
      .expect("Receive timed out")
      .unwrap();
    assert_eq!(received, "hello exclusive");
  }

  #[test]
  fn send_recv_sync() {
    let (tx, mut rx) = exclusive::<u64>();
    assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
    tx.send(7).unwrap();
    assert_eq!(rx.try_recv().unwrap(), 7);
    assert!(matches!(rx.try_recv(), Err(TryRecvError::Disconnected)));
  }

  #[tokio::test]
  async fn sender_drop_disconnects() {
    let (tx, mut rx) = exclusive::<u32>();
    drop(tx);
    assert_eq!(
      timeout(TEST_TIMEOUT, rx.recv()).await.expect("Timeout"),
      Err(RecvError::Disconnected)
    );
  }

  #[test]
  fn sender_close_disconnects_try_recv() {
    let (tx, mut rx) = exclusive::<u32>();
    assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
    tx.close();
    assert!(matches!(rx.try_recv(), Err(TryRecvError::Disconnected)));
  }

  #[test]
  fn send_fails_if_receiver_dropped() {
    let (tx, rx) = exclusive::<String>();
    drop(rx);

    let message = "won't be sent".to_string();
    match tx.send(message.clone()) {
      Err(TrySendError::Closed(returned)) => assert_eq!(returned, message),
      res => panic!("Expected TrySendError::Closed, got {:?}", res),
    }
  }

  #[test]
  fn send_fails_if_receiver_closed() {
    let (tx, mut rx) = exclusive::<u32>();
    rx.close();
    assert!(matches!(tx.send(1), Err(TrySendError::Closed(1))));
    assert!(matches!(rx.try_recv(), Err(TryRecvError::Disconnected)));
  }

  #[test]
  fn receiver_dropped_after_send_value_is_dropped() {
    let counter = Arc::new(AtomicUsize::new(0));
    {
      let (tx, rx) = exclusive::<DropTracker>();
      tx.send(DropTracker(Arc::clone(&counter))).unwrap();
      drop(rx);
    }
    assert_eq!(counter.load(Ordering::SeqCst), 1);
  }

  #[test]
  fn receiver_closed_after_send_value_is_dropped() {
    let counter = Arc::new(AtomicUsize::new(0));
    let (tx, mut rx) = exclusive::<DropTracker>();
    tx.send(DropTracker(Arc::clone(&counter))).unwrap();
    rx.close();
    assert_eq!(counter.load(Ordering::SeqCst), 1);
    drop(rx);
    assert_eq!(counter.load(Ordering::SeqCst), 1);
  }

  #[test]
  fn rejected_send_returns_value_exactly_once() {
    let counter = Arc::new(AtomicUsize::new(0));
    let (tx, rx) = exclusive::<DropTracker>();
    drop(rx);
    match tx.send(DropTracker(Arc::clone(&counter))) {
      Err(TrySendError::Closed(v)) => {
        assert_eq!(counter.load(Ordering::SeqCst), 0);
        drop(v);
      }
      res => panic!("Expected TrySendError::Closed, got {:?}", res.map_err(|_| ())),
    }
    assert_eq!(counter.load(Ordering::SeqCst), 1);
  }

  #[test]
  fn send_racing_receiver_drop_never_leaks() {
    use std::thread;

    for _ in 0..1000 {
      let counter = Arc::new(AtomicUsize::new(0));
      let (tx, rx) = exclusive::<DropTracker>();
      let t = {
        let counter = Arc::clone(&counter);
        thread::spawn(move || {
          let _ = tx.send(DropTracker(counter));
        })
      };
      drop(rx);
      t.join().unwrap();
      assert_eq!(counter.load(Ordering::SeqCst), 1);
    }
  }

  #[tokio::test]
  async fn is_closed_semantics() {
    let (tx, mut rx) = exclusive::<u32>();
    assert!(!tx.is_closed());
    assert!(!rx.is_closed());

    tx.send(5).unwrap();
    assert!(!rx.is_closed());
    assert_eq!(rx.recv().await.unwrap(), 5);
    assert!(rx.is_closed());

    let (tx2, rx2) = exclusive::<u32>();
    drop(rx2);
    assert!(tx2.is_closed());
  }

  #[tokio::test]
  async fn select_on_recv() {
    let (tx1, mut rx1) = exclusive::<i32>();
    let (_tx2, mut rx2) = exclusive::<i32>();

    tokio::spawn(async move {
      tokio::time::sleep(Duration::from_millis(50)).await;
      tx1.send(100).unwrap();
    });

    tokio::select! {
        biased;
        Ok(val) = rx1.recv() => {
            assert_eq!(val, 100);
        }
        _ = rx2.recv() => {
            panic!("Should not have received from rx2");
        }
        _ = tokio::time::sleep(TEST_TIMEOUT) => {
            panic!("Select timed out");
        }
    }
  }

  #[test]
  fn try_recv_never_reports_disconnected_after_a_successful_send() {
    use std::thread;

    const ROUNDS: usize = 50_000;

    let (work_tx, work_rx) = std::sync::mpsc::channel::<super::super::ExclusiveSender<u64>>();
    let sends_ok = Arc::new(AtomicUsize::new(0));

    let sender_thread = {
      let sends_ok = Arc::clone(&sends_ok);
      thread::spawn(move || {
        while let Ok(tx) = work_rx.recv() {
          if tx.send(1).is_ok() {
            sends_ok.fetch_add(1, Ordering::Relaxed);
          }
        }
      })
    };

    let mut false_disconnects = 0usize;
    for _ in 0..ROUNDS {
      let (tx, mut rx) = exclusive::<u64>();
      work_tx.send(tx).unwrap();
      loop {
        match rx.try_recv() {
          Ok(_) => break,
          Err(TryRecvError::Empty) => std::hint::spin_loop(),
          Err(TryRecvError::Disconnected) => {
            false_disconnects += 1;
            break;
          }
        }
      }
    }
    drop(work_tx);
    sender_thread.join().unwrap();

    assert_eq!(sends_ok.load(Ordering::SeqCst), ROUNDS);
    assert_eq!(false_disconnects, 0);
  }
}
