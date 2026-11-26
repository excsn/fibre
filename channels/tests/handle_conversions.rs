//! A handle closed before `to_async` or `to_sync` stays closed after it, and
//! the conversion does not count it out of the channel a second time.

use fibre::error::{TryRecvError, TrySendError};
use fibre::{mpmc, mpsc, spmc, spsc};

fn assert_send_closed<T: std::fmt::Debug>(res: Result<(), TrySendError<T>>) {
  assert!(matches!(res, Err(TrySendError::Closed(_))), "a closed sender accepted a send: {res:?}");
}

fn assert_recv_disconnected<T: std::fmt::Debug>(res: Result<T, TryRecvError>) {
  assert!(matches!(res, Err(TryRecvError::Disconnected)), "a closed receiver did not report Disconnected: {res:?}");
}

// --- spsc rendezvous: one handle per side ----------------------------------

#[test]
fn spsc_rendezvous_sync_sender_closed_before_to_async_stays_closed() {
  let (tx, rx) = spsc::rendezvous::rendezvous::<u32>();
  tx.close().unwrap();
  let tx = tx.to_async();
  assert_send_closed(tx.try_send(1));
  drop(tx);
  assert!(rx.is_closed());
}

#[test]
fn spsc_rendezvous_async_sender_closed_before_to_sync_stays_closed() {
  let (tx, rx) = spsc::rendezvous::rendezvous_async::<u32>();
  tx.close().unwrap();
  let tx = tx.to_sync();
  assert_send_closed(tx.try_send(1));
  drop(tx);
  assert!(rx.is_closed());
}

#[test]
fn spsc_rendezvous_sync_receiver_closed_before_to_async_stays_closed() {
  let (tx, mut rx) = spsc::rendezvous::rendezvous::<u32>();
  rx.close().unwrap();
  let mut rx = rx.to_async();
  assert_recv_disconnected(rx.try_recv());
  drop(rx);
  assert!(tx.is_closed());
}

#[test]
fn spsc_rendezvous_async_receiver_closed_before_to_sync_stays_closed() {
  let (tx, mut rx) = spsc::rendezvous::rendezvous_async::<u32>();
  rx.close().unwrap();
  let mut rx = rx.to_sync();
  assert_recv_disconnected(rx.try_recv());
  drop(rx);
  assert!(tx.is_closed());
}

// --- spsc bounded --------------------------------------------------------------

#[test]
fn spsc_bounded_sync_sender_closed_before_to_async_stays_closed() {
  let (tx, rx) = spsc::bounded_sync::<u32>(4);
  tx.close().unwrap();
  let mut tx = tx.to_async();
  assert_send_closed(tx.try_send(1));
  drop(tx);
  assert_recv_disconnected(rx.try_recv());
}

#[test]
fn spsc_bounded_async_sender_closed_before_to_sync_stays_closed() {
  let (tx, mut rx) = spsc::bounded_async::<u32>(4);
  tx.close().unwrap();
  let tx = tx.to_sync();
  assert_send_closed(tx.try_send(1));
  drop(tx);
  assert_recv_disconnected(rx.try_recv());
}

#[test]
fn spsc_bounded_sync_receiver_closed_before_to_async_stays_closed() {
  let (tx, rx) = spsc::bounded_sync::<u32>(4);
  rx.close().unwrap();
  let mut rx = rx.to_async();
  assert_recv_disconnected(rx.try_recv());
  drop(rx);
  assert_send_closed(tx.try_send(1));
}

#[test]
fn spsc_bounded_async_receiver_closed_before_to_sync_stays_closed() {
  let (mut tx, rx) = spsc::bounded_async::<u32>(4);
  rx.close().unwrap();
  let rx = rx.to_sync();
  assert_recv_disconnected(rx.try_recv());
  drop(rx);
  assert_send_closed(tx.try_send(1));
}

// --- mpsc rendezvous: the sender is shared, the receiver is not ----------------

#[test]
fn mpsc_rendezvous_sync_sender_closed_before_to_async_leaves_its_clone_counted() {
  let (tx, rx) = mpsc::rendezvous::rendezvous::<u32>();
  let other = tx.clone();
  tx.close().unwrap();
  let tx = tx.to_async();
  assert_send_closed(tx.try_send(1));
  drop(tx);
  assert!(!rx.is_closed(), "the clone that is still open no longer counts");
  drop(other);
}

#[test]
fn mpsc_rendezvous_async_sender_closed_before_to_sync_leaves_its_clone_counted() {
  let (tx, rx) = mpsc::rendezvous::rendezvous_async::<u32>();
  let other = tx.clone();
  tx.close().unwrap();
  let tx = tx.to_sync();
  assert_send_closed(tx.try_send(1));
  drop(tx);
  assert!(!rx.is_closed(), "the clone that is still open no longer counts");
  drop(other);
}

#[test]
fn mpsc_rendezvous_sync_receiver_closed_before_to_async_stays_closed() {
  let (tx, mut rx) = mpsc::rendezvous::rendezvous::<u32>();
  rx.close().unwrap();
  let mut rx = rx.to_async();
  assert_recv_disconnected(rx.try_recv());
  drop(rx);
  assert!(tx.is_closed());
}

#[test]
fn mpsc_rendezvous_async_receiver_closed_before_to_sync_stays_closed() {
  let (tx, mut rx) = mpsc::rendezvous::rendezvous_async::<u32>();
  rx.close().unwrap();
  let mut rx = rx.to_sync();
  assert_recv_disconnected(rx.try_recv());
  drop(rx);
  assert!(tx.is_closed());
}

// --- mpmc bounded ----------------------------------------------------------------

#[test]
fn mpmc_bounded_sync_sender_closed_before_to_async_leaves_its_clone_counted() {
  let (tx, rx) = mpmc::bounded::<u32>(4);
  let other = tx.clone();
  tx.close().unwrap();
  let tx = tx.to_async();
  assert_send_closed(tx.try_send(1));
  drop(tx);
  other.try_send(2).unwrap();
  assert_eq!(rx.try_recv(), Ok(2));
}

#[test]
fn mpmc_bounded_async_sender_closed_before_to_sync_leaves_its_clone_counted() {
  let (tx, rx) = mpmc::bounded_async::<u32>(4);
  let other = tx.clone();
  tx.close().unwrap();
  let tx = tx.to_sync();
  assert_send_closed(tx.try_send(1));
  drop(tx);
  other.try_send(2).unwrap();
  assert_eq!(rx.try_recv(), Ok(2));
}

#[test]
fn mpmc_bounded_sync_receiver_closed_before_to_async_leaves_its_clone_counted() {
  let (tx, rx) = mpmc::bounded::<u32>(4);
  let other = rx.clone();
  rx.close().unwrap();
  let rx = rx.to_async();
  assert_recv_disconnected(rx.try_recv());
  drop(rx);
  tx.try_send(2).unwrap();
  assert_eq!(other.try_recv(), Ok(2));
}

#[test]
fn mpmc_bounded_async_receiver_closed_before_to_sync_leaves_its_clone_counted() {
  let (tx, rx) = mpmc::bounded_async::<u32>(4);
  let other = rx.clone();
  rx.close().unwrap();
  let rx = rx.to_sync();
  assert_recv_disconnected(rx.try_recv());
  drop(rx);
  tx.try_send(2).unwrap();
  assert_eq!(other.try_recv(), Ok(2));
}

// --- mpmc rendezvous --------------------------------------------------------------

#[test]
fn mpmc_rendezvous_sync_sender_closed_before_to_async_leaves_its_clone_counted() {
  let (tx, rx) = mpmc::rendezvous::rendezvous::<u32>();
  let other = tx.clone();
  tx.close().unwrap();
  let tx = tx.to_async();
  assert_send_closed(tx.try_send(1));
  drop(tx);
  assert!(!rx.is_closed(), "the clone that is still open no longer counts");
  drop(other);
}

#[test]
fn mpmc_rendezvous_async_sender_closed_before_to_sync_leaves_its_clone_counted() {
  let (tx, rx) = mpmc::rendezvous::rendezvous_async::<u32>();
  let other = tx.clone();
  tx.close().unwrap();
  let tx = tx.to_sync();
  assert_send_closed(tx.try_send(1));
  drop(tx);
  assert!(!rx.is_closed(), "the clone that is still open no longer counts");
  drop(other);
}

#[test]
fn mpmc_rendezvous_sync_receiver_closed_before_to_async_leaves_its_clone_counted() {
  let (tx, rx) = mpmc::rendezvous::rendezvous::<u32>();
  let other = rx.clone();
  rx.close().unwrap();
  let rx = rx.to_async();
  assert_recv_disconnected(rx.try_recv());
  drop(rx);
  assert!(!tx.is_closed(), "the clone that is still open no longer counts");
  drop(other);
}

#[test]
fn mpmc_rendezvous_async_receiver_closed_before_to_sync_leaves_its_clone_counted() {
  let (tx, rx) = mpmc::rendezvous::rendezvous_async::<u32>();
  let other = rx.clone();
  rx.close().unwrap();
  let rx = rx.to_sync();
  assert_recv_disconnected(rx.try_recv());
  drop(rx);
  assert!(!tx.is_closed(), "the clone that is still open no longer counts");
  drop(other);
}

// --- spmc ring senders --------------------------------------------------------------

#[test]
fn spmc_sync_sender_closed_before_to_async_stays_closed() {
  let (mut tx, mut rx) = spmc::bounded::<u32>(4);
  tx.close().unwrap();
  let mut tx = tx.to_async();
  assert_send_closed(tx.try_send(1));
  drop(tx);
  assert_recv_disconnected(rx.try_recv());
}

#[test]
fn spmc_async_sender_closed_before_to_sync_stays_closed() {
  let (mut tx, mut rx) = spmc::bounded_async::<u32>(4);
  tx.close().unwrap();
  let tx = tx.to_sync();
  assert_send_closed(tx.try_send(1));
  drop(tx);
  assert_recv_disconnected(rx.try_recv());
}

// --- spmc topic receivers -------------------------------------------------------------

#[test]
fn topic_sync_receiver_closed_before_to_async_leaves_its_clone_counted() {
  let (tx, rx) = spmc::topic::channel::<&'static str, u32>(4);
  let other = rx.clone();
  rx.close().unwrap();
  let rx = rx.to_async();
  drop(rx);
  assert_eq!(tx.send("a", 1), Ok(()));
  assert!(!tx.is_closed(), "the clone that is still open no longer counts");
  drop(other);
}

#[test]
fn topic_async_receiver_closed_before_to_sync_leaves_its_clone_counted() {
  let (tx, rx) = spmc::topic::channel_async::<&'static str, u32>(4);
  let other = rx.clone();
  rx.close().unwrap();
  let rx = rx.to_sync();
  drop(rx);
  assert!(!tx.is_closed(), "the clone that is still open no longer counts");
  drop(other);
}

#[test]
fn topic_async_receiver_closed_then_dropped_leaves_its_clone_counted() {
  let (tx, rx) = spmc::topic::channel_async::<&'static str, u32>(4);
  let other = rx.clone();
  rx.close().unwrap();
  drop(rx);
  assert!(!tx.is_closed(), "the clone that is still open no longer counts");
  drop(other);
}

#[test]
fn topic_sync_receiver_closed_is_no_longer_delivered_to() {
  let (tx, rx) = spmc::topic::channel::<&'static str, u32>(4);
  rx.subscribe("a");
  let other = rx.clone();
  rx.close().unwrap();
  tx.send("a", 1).unwrap();
  assert!(rx.try_recv().is_err(), "a closed receiver still received a message");
  assert_eq!(other.try_recv(), Ok(("a", 1)));
}

#[test]
fn topic_async_receiver_closed_is_no_longer_delivered_to() {
  let (tx, rx) = spmc::topic::channel_async::<&'static str, u32>(4);
  rx.subscribe("a");
  let other = rx.clone();
  rx.close().unwrap();
  tx.send("a", 1).unwrap();
  assert!(rx.try_recv().is_err(), "a closed receiver still received a message");
  assert_eq!(other.try_recv(), Ok(("a", 1)));
}
