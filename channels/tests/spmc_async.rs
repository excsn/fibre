mod common;
use common::*;

use fibre::spmc;
use std::sync::Arc;
use tokio::sync::Barrier;

#[cfg(not(miri))]
#[tokio::test]
async fn spmc_async_spsc_smoke() {
  let (tx, rx) = spmc::bounded_async(2);
  tx.send(10).await.unwrap();
  assert_eq!(rx.recv().await.unwrap(), 10);
}

#[cfg(not(miri))]
#[tokio::test]
async fn spmc_async_try_recv() {
  let (tx, rx) = spmc::bounded_async::<i32>(2);
  assert_eq!(rx.try_recv(), Err(fibre::error::TryRecvError::Empty));
  tx.send(1).await.unwrap();
  assert_eq!(rx.try_recv(), Ok(1));
  assert_eq!(rx.try_recv(), Err(fibre::error::TryRecvError::Empty));
}

#[cfg(not(miri))]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn spmc_async_multi_consumer() {
  let (tx, rx1) = spmc::bounded_async(ITEMS_LOW);
  let rx2 = rx1.clone();
  let rx3 = rx1.clone();

  let barrier = Arc::new(Barrier::new(4));

  let h1 = {
    let barrier = barrier.clone();
    tokio::spawn(async move {
      barrier.wait().await;
      for i in 0..ITEMS_LOW {
        assert_eq!(rx1.recv().await.unwrap(), i);
      }
    })
  };
  let h2 = {
    let barrier = barrier.clone();
    tokio::spawn(async move {
      barrier.wait().await;
      for i in 0..ITEMS_LOW {
        assert_eq!(rx2.recv().await.unwrap(), i);
      }
    })
  };
  let h3 = {
    let barrier = barrier.clone();
    tokio::spawn(async move {
      barrier.wait().await;
      for i in 0..ITEMS_LOW {
        assert_eq!(rx3.recv().await.unwrap(), i);
      }
    })
  };

  barrier.wait().await; // Ensure all consumers are ready
  for i in 0..ITEMS_LOW {
    tx.send(i).await.unwrap();
  }

  h1.await.unwrap();
  h2.await.unwrap();
  h3.await.unwrap();
}

#[cfg(not(miri))]
#[tokio::test]
async fn spmc_async_slow_consumer_blocks_producer() {
  let (tx, rx_fast) = spmc::bounded_async(1);
  let rx_slow = rx_fast.clone();

  tx.send(1).await.unwrap();
  assert_eq!(rx_fast.recv().await.unwrap(), 1);

  // This send should block because rx_slow is behind.
  // We use `tokio::select!` to race it against a timeout.
  tokio::select! {
      _ = tx.send(2) => {
          panic!("Sender should have blocked, but it completed the send");
      },
      _ = tokio::time::sleep(SHORT_TIMEOUT) => {
          // This is the expected outcome
      }
  }

  // Slow consumer catches up, unblocking the producer.
  assert_eq!(rx_slow.recv().await.unwrap(), 1);

  // The send should now be able to complete.
  tokio::time::timeout(SHORT_TIMEOUT, tx.send(2))
    .await
    .expect("Sender was not unblocked after slow consumer caught up")
    .unwrap();

  assert_eq!(rx_fast.recv().await.unwrap(), 2);
  assert_eq!(rx_slow.recv().await.unwrap(), 2);
}

#[cfg(not(miri))]
#[tokio::test]
async fn spmc_async_sender_unblocks_when_all_receivers_dropped() {
  let (tx, rx1) = spmc::bounded_async(1);
  let rx2 = rx1.clone();

  // Fill the buffer
  tx.send(1).await.unwrap();

  // Fast consumer rx1 reads its copy, but slow consumer rx2 does not
  assert_eq!(rx1.recv().await.unwrap(), 1);

  // Sending again blocks because rx2 is still lagging
  let handle = tokio::spawn(async move { tx.send(2).await });

  // Give the sender task some time to yield and register
  tokio::time::sleep(SHORT_TIMEOUT).await;
  assert!(!handle.is_finished(), "Sender task should be blocked");

  // Drop all active receivers
  drop(rx1);
  drop(rx2);

  let res = handle.await.expect("Sender task panicked");
  assert!(
    matches!(res, Err(fibre::error::SendError::Closed)),
    "Expected Err(SendError::Closed), got {:?}",
    res
  );
}
