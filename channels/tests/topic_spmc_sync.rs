// tests/topic_spmc_sync.rs

mod common;
use common::*;

use fibre::error::{RecvError, SendError, TryRecvError};
use fibre::spmc::topic as spmc_topic;
use std::collections::HashSet;
use std::sync::{Arc, Barrier};
use std::thread;

#[test]
fn sync_topic_single_subscriber_receives() {
  let (tx, rx) = spmc_topic::channel::<&str, String>(16);
  rx.subscribe("topic1");

  tx.send("topic1", "hello".to_string()).unwrap();
  tx.send("topic2", "world".to_string()).unwrap(); // This should be ignored

  assert_eq!(rx.recv().unwrap(), ("topic1", "hello".to_string()));
  // Receiver's mailbox should now be empty
  assert_eq!(rx.try_recv(), Err(TryRecvError::Empty));
}

#[test]
fn sync_topic_multiple_subscribers_same_topic() {
  let (tx, rx1) = spmc_topic::channel(16);
  let rx2 = rx1.clone();
  let rx3 = rx1.clone();

  rx1.subscribe("news");
  rx2.subscribe("news");
  rx3.subscribe("news");

  let barrier = Arc::new(Barrier::new(4));
  let mut handles = vec![];

  let t1_barrier = barrier.clone();
  handles.push(thread::spawn(move || {
    t1_barrier.wait();
    assert_eq!(rx1.recv().unwrap().1, "breaking news");
  }));
  let t2_barrier = barrier.clone();
  handles.push(thread::spawn(move || {
    t2_barrier.wait();
    assert_eq!(rx2.recv().unwrap().1, "breaking news");
  }));
  let t3_barrier = barrier.clone();
  handles.push(thread::spawn(move || {
    t3_barrier.wait();
    assert_eq!(rx3.recv().unwrap().1, "breaking news");
  }));

  barrier.wait();
  tx.send("news", "breaking news".to_string()).unwrap();

  for h in handles {
    h.join().unwrap();
  }
}

#[test]
fn sync_topic_unsubscribe_works() {
  let (tx, rx) = spmc_topic::channel(16);
  rx.subscribe("topic1");

  tx.send("topic1", "first".to_string()).unwrap();
  assert_eq!(rx.recv().unwrap().1, "first");

  rx.unsubscribe(&"topic1");
  tx.send("topic1", "second".to_string()).unwrap();

  // Give a moment for the message to be (not) delivered
  thread::sleep(SHORT_TIMEOUT);

  // Should be empty as we are no longer subscribed
  assert_eq!(rx.try_recv(), Err(TryRecvError::Empty));
}

#[test]
fn sync_topic_slow_consumer_drops_messages() {
  // Mailbox capacity of 1
  let (tx, rx) = spmc_topic::channel(1);
  rx.subscribe("important");

  // First message fills the mailbox
  tx.send("important", "msg1".to_string()).unwrap();

  // These next two messages should be dropped because the consumer is slow
  // and the mailbox is full. The sender does NOT block.
  tx.send("important", "msg2".to_string()).unwrap();
  tx.send("important", "msg3".to_string()).unwrap();

  // Check internal dropped count (test-only helper would be nice, but we can infer)

  // Consumer finally receives the first message
  assert_eq!(rx.recv().unwrap().1, "msg1");

  // The mailbox is now empty. msg2 and msg3 were dropped.
  assert_eq!(rx.try_recv(), Err(TryRecvError::Empty));
}

#[test]
fn sync_topic_sender_drop_disconnects_receivers() {
  let (tx, rx) = spmc_topic::channel(16);
  rx.subscribe("a");
  tx.send("a", 1).unwrap();

  // Drop the sender
  drop(tx);

  assert_eq!(rx.recv().unwrap().1, 1);
  // Now that the buffer is empty and sender is gone, it's disconnected
  assert_eq!(rx.recv(), Err(RecvError::Disconnected));
}

#[test]
fn sync_topic_all_receivers_drop_closes_sender() {
  let (tx, rx1) = spmc_topic::channel::<&str, i32>(16);
  let rx2 = rx1.clone();

  rx1.subscribe("a");
  rx2.subscribe("b");

  // Senders can still send
  assert!(tx.send("a", 1).is_ok());

  drop(rx1);
  drop(rx2);

  // Give a moment for the drop to propagate
  thread::sleep(SHORT_TIMEOUT);

  // Now all receivers are gone, send should fail
  assert_eq!(tx.send("a", 2), Err(SendError::Closed));
  assert!(tx.is_closed());
}

#[test]
fn sync_topic_clone_receiver_inherits_subscriptions() {
  let (tx, rx1) = spmc_topic::channel(16);
  rx1.subscribe("a");
  rx1.subscribe("b");

  // Clone should get a copy of the subscriptions
  let rx2 = rx1.clone();

  // Unsubscribe original from "b", should not affect clone
  rx1.unsubscribe(&"b");

  tx.send("a", 10).unwrap();
  tx.send("b", 20).unwrap();

  // rx1 only gets "a"
  assert_eq!(rx1.recv().unwrap(), ("a", 10));
  assert_eq!(rx1.try_recv(), Err(TryRecvError::Empty));

  // rx2 gets both
  let mut results = HashSet::new();
  results.insert(rx2.recv().unwrap());
  results.insert(rx2.recv().unwrap());

  let expected: HashSet<(&str, i32)> = [("a", 10), ("b", 20)].iter().cloned().collect();
  assert_eq!(results, expected);
}
