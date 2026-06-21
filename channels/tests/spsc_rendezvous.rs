//! Tests for the dedicated SPSC rendezvous channel (`spsc::rendezvous`).

use fibre::error::{RecvError, RecvErrorTimeout, SendError, TryRecvError, TrySendError};
use fibre::spsc::rendezvous::{rendezvous, rendezvous_async};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

// --- Synchronous ----------------------------------------------------------

#[test]
fn sync_basic_handoff() {
  let (tx, rx) = rendezvous::<u32>();
  let producer = thread::spawn(move || {
    for i in 0..10 {
      tx.send(i).unwrap();
    }
  });
  for i in 0..10 {
    assert_eq!(rx.recv().unwrap(), i);
  }
  producer.join().unwrap();
}

#[test]
fn sync_try_send_without_receiver_is_full() {
  let (tx, _rx) = rendezvous::<u32>();
  match tx.try_send(7) {
    Err(TrySendError::Full(7)) => {}
    other => panic!("expected Full(7), got {:?}", other),
  }
}

#[test]
fn sync_try_recv_without_sender_is_empty() {
  let (_tx, rx) = rendezvous::<u32>();
  assert_eq!(rx.try_recv(), Err(TryRecvError::Empty));
}

#[test]
fn sync_try_send_meets_parked_receiver() {
  let (tx, rx) = rendezvous::<u32>();
  let consumer = thread::spawn(move || rx.recv().unwrap());
  thread::sleep(Duration::from_millis(50));
  tx.try_send(42).expect("a parked receiver should accept try_send");
  assert_eq!(consumer.join().unwrap(), 42);
}

#[test]
fn sync_receiver_drop_disconnects_sender() {
  let (tx, rx) = rendezvous::<u32>();
  drop(rx);
  assert_eq!(tx.send(1), Err(SendError::Closed));
}

#[test]
fn sync_sender_drop_disconnects_receiver() {
  let (tx, rx) = rendezvous::<u32>();
  drop(tx);
  assert_eq!(rx.recv(), Err(RecvError::Disconnected));
}

#[test]
fn sync_parked_sender_woken_on_receiver_drop() {
  let (tx, rx) = rendezvous::<u32>();
  let producer = thread::spawn(move || tx.send(1));
  thread::sleep(Duration::from_millis(50));
  drop(rx);
  assert_eq!(producer.join().unwrap(), Err(SendError::Closed));
}

#[test]
fn sync_recv_timeout_elapses_then_receives() {
  let (tx, rx) = rendezvous::<u32>();
  assert_eq!(
    rx.recv_timeout(Duration::from_millis(40)),
    Err(RecvErrorTimeout::Timeout)
  );
  let producer = thread::spawn(move || {
    thread::sleep(Duration::from_millis(20));
    tx.send(5).unwrap();
  });
  assert_eq!(rx.recv_timeout(Duration::from_secs(5)), Ok(5));
  producer.join().unwrap();
}

// --- Asynchronous ---------------------------------------------------------

#[cfg(not(miri))]
#[tokio::test]
async fn async_basic_handoff() {
  let (tx, rx) = rendezvous_async::<u32>();
  let producer = tokio::spawn(async move {
    for i in 0..10 {
      tx.send(i).await.unwrap();
    }
  });
  for i in 0..10 {
    assert_eq!(rx.recv().await.unwrap(), i);
  }
  producer.await.unwrap();
}

#[cfg(not(miri))]
#[tokio::test]
async fn async_sender_drop_disconnects_receiver() {
  let (tx, rx) = rendezvous_async::<u32>();
  drop(tx);
  assert_eq!(rx.recv().await, Err(RecvError::Disconnected));
}

#[cfg(not(miri))]
#[tokio::test]
async fn async_cancelled_send_does_not_ghost_deliver() {
  use tokio::time::timeout;
  let (tx, rx) = rendezvous_async::<u32>();
  {
    let fut = tx.send(123);
    assert!(timeout(Duration::from_millis(60), fut).await.is_err());
  }
  assert_eq!(rx.try_recv(), Err(TryRecvError::Empty));
}

#[cfg(not(miri))]
#[tokio::test]
async fn async_cancelled_send_reclaims_payload() {
  use tokio::time::timeout;
  let (tx, _rx) = rendezvous_async::<Arc<()>>();
  let payload = Arc::new(());
  let weak = Arc::downgrade(&payload);
  {
    let fut = tx.send(payload.clone());
    assert!(timeout(Duration::from_millis(40), fut).await.is_err());
  }
  drop(payload);
  assert!(weak.upgrade().is_none(), "payload leaked on cancel");
}

// --- Hybrid sync/async ----------------------------------------------------

#[cfg(not(miri))]
#[tokio::test]
async fn hybrid_async_sender_sync_receiver() {
  let (tx, rx) = rendezvous_async::<u32>();
  let rx = rx.to_sync();
  let consumer = thread::spawn(move || {
    let mut got = Vec::new();
    for _ in 0..3 {
      got.push(rx.recv().unwrap());
    }
    got
  });
  for i in 0..3 {
    tx.send(i).await.unwrap();
  }
  assert_eq!(consumer.join().unwrap(), vec![0, 1, 2]);
}
