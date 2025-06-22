// tests/topic_spmc_async.rs

mod common;
use common::*;

use fibre::error::{RecvError, SendError, TryRecvError};
use fibre::spmc::topic as spmc_topic;
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::Barrier;
use tokio::time::timeout;

#[tokio::test]
async fn async_topic_single_subscriber_receives() {
  let (tx, rx) = spmc_topic::channel_async::<&str, String>(16);
  rx.subscribe("topic1");

  tx.send("topic1", "hello".to_string()).unwrap();
  tx.send("topic2", "world".to_string()).unwrap(); // This should be ignored

  let received = timeout(SHORT_TIMEOUT, rx.recv()).await.unwrap().unwrap();
  assert_eq!(received, ("topic1", "hello".to_string()));

  // Recv should now pend
  let res = timeout(SHORT_TIMEOUT, rx.recv()).await;
  assert!(
    res.is_err(),
    "Receiver should not have received a second message"
  );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn async_topic_multiple_subscribers_same_topic() {
  let (tx, rx1) = spmc_topic::channel_async(16);
  let rx2 = rx1.clone();
  let rx3 = rx1.clone();

  rx1.subscribe("news");
  rx2.subscribe("news");
  rx3.subscribe("news");

  let barrier = Arc::new(Barrier::new(4));

  let h1 = {
    let barrier = barrier.clone();
    tokio::spawn(async move {
      barrier.wait().await;
      assert_eq!(rx1.recv().await.unwrap().1, "breaking news");
    })
  };
  let h2 = {
    let barrier = barrier.clone();
    tokio::spawn(async move {
      barrier.wait().await;
      assert_eq!(rx2.recv().await.unwrap().1, "breaking news");
    })
  };
  let h3 = {
    let barrier = barrier.clone();
    tokio::spawn(async move {
      barrier.wait().await;
      assert_eq!(rx3.recv().await.unwrap().1, "breaking news");
    })
  };

  barrier.wait().await;
  tx.send("news", "breaking news".to_string()).unwrap();

  h1.await.unwrap();
  h2.await.unwrap();
  h3.await.unwrap();
}

#[tokio::test]
async fn async_topic_unsubscribe_works() {
  let (tx, rx) = spmc_topic::channel_async(16);
  rx.subscribe("topic1");

  tx.send("topic1", "first".to_string()).unwrap();
  assert_eq!(rx.recv().await.unwrap().1, "first");

  rx.unsubscribe(&"topic1");
  tx.send("topic1", "second".to_string()).unwrap();

  let res = timeout(SHORT_TIMEOUT, rx.recv()).await;
  assert!(
    res.is_err(),
    "Receiver should have timed out waiting for a message"
  );
}

#[tokio::test]
async fn async_topic_slow_consumer_drops_messages() {
  // Mailbox capacity of 1
  let (tx, rx) = spmc_topic::channel_async(1);
  rx.subscribe("important");

  tx.send("important", "msg1".to_string()).unwrap();
  tx.send("important", "msg2".to_string()).unwrap(); // Dropped
  tx.send("important", "msg3".to_string()).unwrap(); // Dropped

  assert_eq!(rx.recv().await.unwrap().1, "msg1");

  // The mailbox is now empty. msg2 and msg3 were dropped.
  let res = timeout(SHORT_TIMEOUT, rx.recv()).await;
  assert!(
    res.is_err(),
    "Receiver should have timed out, proving other messages were dropped"
  );
}

#[tokio::test]
async fn async_topic_sender_drop_disconnects_receivers() {
  let (tx, rx) = spmc_topic::channel_async(16);
  rx.subscribe("a");
  tx.send("a", 1).unwrap();

  drop(tx);

  assert_eq!(rx.recv().await.unwrap().1, 1);
  assert_eq!(rx.recv().await, Err(RecvError::Disconnected));
}

#[tokio::test]
async fn async_topic_all_receivers_drop_closes_sender() {
  let (tx, rx1) = spmc_topic::channel_async::<&str, i32>(16);
  let rx2 = rx1.clone();

  rx1.subscribe("a");
  rx2.subscribe("b");

  assert!(tx.send("a", 1).is_ok());

  drop(rx1);
  drop(rx2);

  tokio::time::sleep(SHORT_TIMEOUT).await;

  assert_eq!(tx.send("a", 2), Err(SendError::Closed));
  assert!(tx.is_closed());
}

#[tokio::test]
async fn mixed_sync_sender_async_receiver() {
  let (tx_sync, rx_sync) = spmc_topic::channel::<&str, i32>(16);
  let rx_async = rx_sync.to_async();

  rx_async.subscribe("a");

  let send_handle = tokio::task::spawn_blocking(move || {
    tx_sync.send("a", 123).unwrap();
  });

  assert_eq!(rx_async.recv().await.unwrap().1, 123);
  send_handle.await.unwrap();
}

#[tokio::test]
async fn mixed_async_sender_sync_receiver() {
  let (tx_async, rx_async) = spmc_topic::channel_async::<&str, i32>(16);
  let rx_sync = rx_async.to_sync();

  rx_sync.subscribe("b");

  let send_task = tokio::spawn(async move {
    tx_async.send("b", 456).unwrap();
  });

  let recv_handle = tokio::task::spawn_blocking(move || rx_sync.recv().unwrap());

  let result = recv_handle.await.unwrap();
  assert_eq!(result, ("b", 456));
  send_task.await.unwrap();
}
