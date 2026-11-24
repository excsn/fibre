//! Tests for the dedicated MPMC rendezvous channel (`mpmc::rendezvous`).

use fibre::error::{RecvError, RecvErrorTimeout, SendError, TryRecvError, TrySendError};
use fibre::mpmc::rendezvous::{rendezvous, rendezvous_async};
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
  got.sort();
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
fn sync_try_send_meets_parked_receiver() {
  let (tx, rx) = rendezvous::<u32>();
  let consumer = thread::spawn(move || rx.recv().unwrap());
  // Let the receiver park.
  thread::sleep(Duration::from_millis(50));
  tx.try_send(42).expect("a parked receiver should accept try_send");
  assert_eq!(consumer.join().unwrap(), 42);
}

#[test]
fn sync_send_blocks_until_received() {
  let (tx, rx) = rendezvous::<u32>();
  let order = Arc::new(AtomicUsize::new(0));
  let o2 = order.clone();
  let producer = thread::spawn(move || {
    tx.send(1).unwrap();
    // send must only return after the receiver took the item.
    o2.fetch_add(10, Ordering::SeqCst);
  });
  thread::sleep(Duration::from_millis(80));
  // Producer is still blocked in send; nothing added yet.
  assert_eq!(order.load(Ordering::SeqCst), 0);
  assert_eq!(rx.recv().unwrap(), 1);
  producer.join().unwrap();
  assert_eq!(order.load(Ordering::SeqCst), 10);
}

#[test]
fn sync_receiver_drop_disconnects_sender() {
  let (tx, rx) = rendezvous::<u32>();
  drop(rx);
  assert_eq!(tx.send(1), Err(SendError::Closed));
  match tx.try_send(1) {
    Err(TrySendError::Closed(1)) => {}
    other => panic!("expected Closed, got {:?}", other),
  }
}

#[test]
fn sync_sender_drop_disconnects_receiver() {
  let (tx, rx) = rendezvous::<u32>();
  drop(tx);
  assert_eq!(rx.recv(), Err(RecvError::Disconnected));
  assert_eq!(rx.try_recv(), Err(TryRecvError::Disconnected));
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
fn sync_parked_receiver_woken_on_sender_drop() {
  let (tx, rx) = rendezvous::<u32>();
  let consumer = thread::spawn(move || rx.recv());
  thread::sleep(Duration::from_millis(50));
  drop(tx);
  assert_eq!(consumer.join().unwrap(), Err(RecvError::Disconnected));
}

#[test]
fn sync_recv_timeout_elapses() {
  let (_tx, rx) = rendezvous::<u32>();
  let start = std::time::Instant::now();
  assert_eq!(
    rx.recv_timeout(Duration::from_millis(80)),
    Err(RecvErrorTimeout::Timeout)
  );
  assert!(start.elapsed() >= Duration::from_millis(70));
}

#[test]
fn sync_recv_timeout_receives() {
  let (tx, rx) = rendezvous::<u32>();
  let producer = thread::spawn(move || {
    thread::sleep(Duration::from_millis(30));
    tx.send(99).unwrap();
  });
  assert_eq!(rx.recv_timeout(Duration::from_secs(5)), Ok(99));
  producer.join().unwrap();
}

#[test]
fn sync_multi_producer_multi_consumer_totals() {
  const PRODUCERS: usize = 4;
  const PER: usize = 200;
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

  let received = Arc::new(AtomicUsize::new(0));
  let sum = Arc::new(AtomicUsize::new(0));
  let mut consumers = Vec::new();
  for _ in 0..3 {
    let rx = rx.clone();
    let received = received.clone();
    let sum = sum.clone();
    consumers.push(thread::spawn(move || {
      while let Ok(v) = rx.recv() {
        received.fetch_add(1, Ordering::SeqCst);
        sum.fetch_add(v, Ordering::SeqCst);
      }
    }));
  }
  drop(rx);

  for p in producers {
    p.join().unwrap();
  }
  for c in consumers {
    c.join().unwrap();
  }
  assert_eq!(received.load(Ordering::SeqCst), PRODUCERS * PER);
  let expected: usize = (0..PRODUCERS * PER).sum();
  assert_eq!(sum.load(Ordering::SeqCst), expected);
}

// --- Asynchronous ---------------------------------------------------------

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
  got.sort();
  assert_eq!(got, vec![0, 1, 2, 3, 4]);
}

#[tokio::test]
async fn async_sender_drop_disconnects_receiver() {
  let (tx, rx) = rendezvous_async::<u32>();
  drop(tx);
  assert_eq!(rx.recv().await, Err(RecvError::Disconnected));
}

#[tokio::test]
async fn async_receiver_drop_disconnects_sender() {
  let (tx, rx) = rendezvous_async::<u32>();
  drop(rx);
  assert_eq!(tx.send(1).await, Err(SendError::Closed));
}

#[tokio::test]
async fn async_cancelled_send_does_not_ghost_deliver() {
  use tokio::time::timeout;
  let (tx, rx) = rendezvous_async::<u32>();
  {
    let fut = tx.send(123);
    // No receiver yet: the future parks, then we cancel it.
    assert!(timeout(Duration::from_millis(60), fut).await.is_err());
  }
  // The cancelled send must not have delivered anything.
  assert_eq!(rx.try_recv(), Err(TryRecvError::Empty));
}

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
  // The parked payload must have been dropped with the cancelled future.
  assert!(weak.upgrade().is_none(), "payload leaked on cancel");
}

#[tokio::test]
async fn async_cancelled_recv_is_clean() {
  use tokio::time::timeout;
  let (tx, rx) = rendezvous_async::<u32>();
  {
    let fut = rx.recv();
    assert!(timeout(Duration::from_millis(40), fut).await.is_err());
  }
  // Channel still usable after a cancelled recv.
  let producer = tokio::spawn(async move { tx.send(5).await });
  assert_eq!(rx.recv().await, Ok(5));
  producer.await.unwrap().unwrap();
}

// --- Hybrid sync/async ----------------------------------------------------

#[tokio::test]
async fn hybrid_sync_sender_async_receiver() {
  let (tx, rx) = rendezvous_async::<u32>();
  let tx = tx.to_sync();
  let producer = thread::spawn(move || {
    for i in 0..3 {
      tx.send(i).unwrap();
    }
  });
  let mut got = Vec::new();
  for _ in 0..3 {
    got.push(rx.recv().await.unwrap());
  }
  producer.join().unwrap();
  got.sort();
  assert_eq!(got, vec![0, 1, 2]);
}

#[test]
fn item_delivered_to_a_dropped_recv_future_goes_to_the_next_receive() {
  use std::future::Future;
  use std::task::{Context, Waker};

  let (tx, rx) = rendezvous_async::<u32>();
  {
    let mut fut = Box::pin(rx.recv());
    assert!(fut
      .as_mut()
      .poll(&mut Context::from_waker(Waker::noop()))
      .is_pending());
    assert!(tx.try_send(5).is_ok());
  }
  assert_eq!(rx.try_recv(), Ok(5));
}

#[test]
fn items_returned_by_dropped_recv_futures_keep_their_order() {
  use std::future::Future;
  use std::task::{Context, Waker};

  let (tx, rx) = rendezvous_async::<u32>();
  let rx2 = rx.clone();
  let mut cx = Context::from_waker(Waker::noop());
  let mut first = Box::pin(rx.recv());
  let mut second = Box::pin(rx2.recv());
  assert!(first.as_mut().poll(&mut cx).is_pending());
  assert!(second.as_mut().poll(&mut cx).is_pending());
  assert!(tx.try_send(1).is_ok());
  assert!(tx.try_send(2).is_ok());
  drop(first);
  drop(second);

  assert_eq!(rx.try_recv(), Ok(1));
  assert_eq!(rx.try_recv(), Ok(2));
  assert_eq!(rx.try_recv(), Err(TryRecvError::Empty));
}

#[test]
fn returned_item_is_received_before_a_parked_senders_item() {
  use std::future::Future;
  use std::task::{Context, Poll, Waker};

  let (tx, rx) = rendezvous_async::<u32>();
  let mut cx = Context::from_waker(Waker::noop());
  {
    let mut fut = Box::pin(rx.recv());
    assert!(fut.as_mut().poll(&mut cx).is_pending());
    assert!(tx.try_send(1).is_ok());
  }
  let mut parked = Box::pin(tx.send(2));
  assert!(parked.as_mut().poll(&mut cx).is_pending());

  assert_eq!(rx.try_recv(), Ok(1));
  assert_eq!(rx.try_recv(), Ok(2));
  assert!(matches!(parked.as_mut().poll(&mut cx), Poll::Ready(Ok(()))));
}

#[test]
fn returned_item_is_received_after_the_last_sender_is_gone() {
  use std::future::Future;
  use std::task::{Context, Waker};

  let (tx, rx) = rendezvous_async::<u32>();
  {
    let mut fut = Box::pin(rx.recv());
    assert!(fut
      .as_mut()
      .poll(&mut Context::from_waker(Waker::noop()))
      .is_pending());
    assert!(tx.try_send(7).is_ok());
  }
  drop(tx);

  assert_eq!(rx.try_recv(), Ok(7));
  assert_eq!(rx.try_recv(), Err(TryRecvError::Disconnected));
}

#[test]
fn returned_item_is_dropped_with_the_last_receiver() {
  use std::future::Future;
  use std::task::{Context, Waker};

  struct Counted(Arc<AtomicUsize>);
  impl Drop for Counted {
    fn drop(&mut self) {
      self.0.fetch_add(1, Ordering::SeqCst);
    }
  }

  let dropped = Arc::new(AtomicUsize::new(0));
  let (tx, rx) = rendezvous_async::<Counted>();
  {
    let mut fut = Box::pin(rx.recv());
    assert!(fut
      .as_mut()
      .poll(&mut Context::from_waker(Waker::noop()))
      .is_pending());
    assert!(tx.try_send(Counted(Arc::clone(&dropped))).is_ok());
  }
  assert_eq!(dropped.load(Ordering::SeqCst), 0);
  drop(rx);
  assert_eq!(dropped.load(Ordering::SeqCst), 1);
  drop(tx);
}
