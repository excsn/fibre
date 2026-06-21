//! Tests for the dedicated MPSC rendezvous channel (`mpsc::rendezvous`).

use fibre::error::{RecvError, RecvErrorTimeout, SendError, TryRecvError, TrySendError};
use fibre::mpsc::rendezvous::{rendezvous, rendezvous_async};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

// --- Synchronous ----------------------------------------------------------

#[test]
fn sync_basic_handoff() {
  let (tx, rx) = rendezvous::<u32>();
  let producer = thread::spawn(move || {
    for i in 0..5 {
      tx.send(i).unwrap();
    }
  });
  let mut got = Vec::new();
  for _ in 0..5 {
    got.push(rx.recv().unwrap());
  }
  producer.join().unwrap();
  assert_eq!(got, vec![0, 1, 2, 3, 4]);
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
fn sync_last_sender_drop_wakes_receiver() {
  let (tx, rx) = rendezvous::<u32>();
  let tx2 = tx.clone();
  let consumer = thread::spawn(move || rx.recv());
  thread::sleep(Duration::from_millis(50));
  drop(tx);
  // One sender remains, so the receiver stays parked.
  thread::sleep(Duration::from_millis(50));
  drop(tx2);
  assert_eq!(consumer.join().unwrap(), Err(RecvError::Disconnected));
}

#[test]
fn sync_recv_timeout_elapses() {
  let (_tx, rx) = rendezvous::<u32>();
  assert_eq!(
    rx.recv_timeout(Duration::from_millis(60)),
    Err(RecvErrorTimeout::Timeout)
  );
}

#[test]
fn sync_many_producers_one_consumer_totals() {
  const PRODUCERS: usize = 4;
  const PER: usize = 250;
  let (tx, rx) = rendezvous::<usize>();
  let mut producers = Vec::new();
  for p in 0..PRODUCERS {
    let tx = tx.clone();
    producers.push(thread::spawn(move || {
      for i in 0..PER {
        tx.send(p * PER + i).unwrap();
      }
    }));
  }
  drop(tx);

  let mut count = 0usize;
  let mut sum = 0usize;
  while let Ok(v) = rx.recv() {
    count += 1;
    sum += v;
  }
  for p in producers {
    p.join().unwrap();
  }
  assert_eq!(count, PRODUCERS * PER);
  assert_eq!(sum, (0..PRODUCERS * PER).sum::<usize>());
}

// --- Asynchronous ---------------------------------------------------------

#[cfg(not(miri))]
#[tokio::test]
async fn async_basic_handoff() {
  let (tx, rx) = rendezvous_async::<u32>();
  let producer = tokio::spawn(async move {
    for i in 0..5 {
      tx.send(i).await.unwrap();
    }
  });
  let mut got = Vec::new();
  for _ in 0..5 {
    got.push(rx.recv().await.unwrap());
  }
  producer.await.unwrap();
  assert_eq!(got, vec![0, 1, 2, 3, 4]);
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

#[cfg(not(miri))]
#[tokio::test]
async fn async_many_producers_one_consumer() {
  let (tx, rx) = rendezvous_async::<usize>();
  let received = Arc::new(AtomicUsize::new(0));
  let mut tasks = Vec::new();
  for p in 0..4 {
    let tx = tx.clone();
    tasks.push(tokio::spawn(async move {
      for i in 0..50 {
        tx.send(p * 50 + i).await.unwrap();
      }
    }));
  }
  drop(tx);
  while rx.recv().await.is_ok() {
    received.fetch_add(1, Ordering::SeqCst);
  }
  for t in tasks {
    t.await.unwrap();
  }
  assert_eq!(received.load(Ordering::SeqCst), 200);
}
