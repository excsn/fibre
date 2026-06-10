//! Integration tests for the SPMC (broadcast) batch send/receive APIs.

mod common;

#[allow(unused_imports)]
use common::*;

use fibre::error::{BatchSendErrorReason, RecvError, TryRecvError};
use fibre::spmc;

use std::thread;
use std::time::Duration;

// --- Sync tests ---

#[test]
fn sync_try_send_batch_all_fit() {
  let (tx, rx) = spmc::bounded::<u32>(8);
  assert_eq!(tx.try_send_batch(vec![1, 2, 3]).unwrap(), 3);
  assert_eq!(rx.try_recv_batch(8).unwrap(), vec![1, 2, 3]);
}

#[test]
fn sync_try_send_batch_partial_slowest_consumer() {
  let (tx, rx) = spmc::bounded::<u32>(4);
  let err = tx.try_send_batch((0..6).collect()).unwrap_err();
  assert_eq!(err.sent, 4);
  assert_eq!(err.reason, BatchSendErrorReason::Full);
  assert_eq!(err.unsent, vec![4, 5]);
  assert_eq!(rx.try_recv_batch(10).unwrap(), vec![0, 1, 2, 3]);
}

#[test]
fn sync_try_send_batch_no_consumers() {
  let (tx, rx) = spmc::bounded::<u32>(4);
  drop(rx);
  let err = tx.try_send_batch(vec![1, 2]).unwrap_err();
  assert_eq!(err.sent, 0);
  assert_eq!(err.reason, BatchSendErrorReason::Closed);
  assert_eq!(err.into_unsent(), vec![1, 2]);
}

#[test]
fn sync_batch_fanout_every_receiver_sees_all() {
  let (tx, rx1) = spmc::bounded::<u32>(16);
  let rx2 = rx1.clone();
  let rx3 = rx1.clone();

  assert_eq!(tx.try_send_batch((0..10).collect()).unwrap(), 10);

  for rx in [&rx1, &rx2, &rx3] {
    let mut got = Vec::new();
    while got.len() < 10 {
      rx.try_recv_batch_mut(&mut got, 4).unwrap();
    }
    assert_eq!(got, (0..10).collect::<Vec<_>>());
  }
}

#[test]
fn sync_send_batch_mut_drains_front() {
  let (tx, rx) = spmc::bounded::<u32>(3);
  let mut items = vec![10, 20, 30, 40, 50];
  assert_eq!(tx.try_send_batch_mut(&mut items).unwrap(), 3);
  assert_eq!(items, vec![40, 50]);
  assert_eq!(tx.try_send_batch_mut(&mut items).unwrap(), 0);
  assert_eq!(rx.try_recv_batch(10).unwrap(), vec![10, 20, 30]);
  assert_eq!(tx.try_send_batch_mut(&mut items).unwrap(), 2);
  assert!(items.is_empty());
}

#[test]
fn sync_send_batch_blocks_on_slow_consumer_and_unblocks() {
  let (tx, rx) = spmc::bounded::<usize>(4);

  let producer = thread::spawn(move || tx.send_batch((0..16).collect()).unwrap());

  // Give the producer time to fill the buffer and park on the slow consumer.
  thread::sleep(Duration::from_millis(100));

  let mut got = Vec::new();
  while got.len() < 16 {
    got.extend(rx.recv_batch(8).unwrap());
  }
  assert_eq!(producer.join().unwrap(), 16);
  assert_eq!(got, (0..16).collect::<Vec<_>>());
}

#[test]
fn sync_send_batch_unblocks_at_slowest_consumer_pace() {
  // One fast consumer, one slow consumer: the producer's progress is bounded
  // by the slow consumer; once the slow consumer resumes, the producer
  // completes.
  let (tx, rx_fast) = spmc::bounded::<usize>(4);
  let rx_slow = rx_fast.clone();

  let producer = thread::spawn(move || tx.send_batch((0..12).collect()).unwrap());

  let fast = thread::spawn(move || {
    let mut got = Vec::new();
    while got.len() < 12 {
      got.extend(rx_fast.recv_batch(12).unwrap());
    }
    got
  });

  // Stalled slow consumer: producer can write at most `capacity` items ahead
  // of it. Wait, then verify the producer is still blocked.
  thread::sleep(Duration::from_millis(150));
  assert!(!producer.is_finished(), "producer must block on the slowest consumer");

  // Resume the slow consumer.
  let mut slow_got = Vec::new();
  while slow_got.len() < 12 {
    slow_got.extend(rx_slow.recv_batch(4).unwrap());
  }

  assert_eq!(producer.join().unwrap(), 12);
  assert_eq!(fast.join().unwrap(), (0..12).collect::<Vec<_>>());
  assert_eq!(slow_got, (0..12).collect::<Vec<_>>());
}

#[test]
fn sync_recv_batch_blocks_until_first() {
  let (tx, rx) = spmc::bounded::<u32>(8);
  let consumer = thread::spawn(move || rx.recv_batch(8).unwrap());
  thread::sleep(Duration::from_millis(100));
  tx.try_send_batch(vec![5, 6]).unwrap();
  let got = consumer.join().unwrap();
  assert!(!got.is_empty() && got.len() <= 2);
  assert_eq!(got[0], 5);
}

#[test]
fn sync_recv_batch_disconnected_after_drain() {
  let (tx, rx) = spmc::bounded::<u32>(8);
  tx.try_send_batch(vec![1, 2, 3]).unwrap();
  drop(tx);
  assert_eq!(rx.recv_batch(10).unwrap(), vec![1, 2, 3]);
  assert_eq!(rx.recv_batch(10), Err(RecvError::Disconnected));
  assert_eq!(rx.try_recv_batch(10), Err(TryRecvError::Disconnected));
}

#[test]
fn sync_batch_wrap_around_with_drop_check() {
  use std::sync::atomic::{AtomicUsize, Ordering};
  use std::sync::Arc;

  #[derive(Clone)]
  struct Droppable(#[allow(dead_code)] u32, Arc<AtomicUsize>);
  impl Drop for Droppable {
    fn drop(&mut self) {
      self.1.fetch_add(1, Ordering::Relaxed);
    }
  }

  let drops = Arc::new(AtomicUsize::new(0));
  {
    let (tx, rx) = spmc::bounded::<Droppable>(4);
    // Multiple laps over the ring force slot overwrites (stale values must be
    // dropped in place, exactly once).
    for lap in 0..6u32 {
      let batch: Vec<Droppable> = (0..3).map(|i| Droppable(lap * 3 + i, drops.clone())).collect();
      assert_eq!(tx.try_send_batch(batch).unwrap(), 3);
      let got = rx.try_recv_batch(3).unwrap();
      assert_eq!(got.len(), 3);
      // `got` holds clones; originals stay in slots until overwritten.
    }
    drop(tx);
    drop(rx);
  }
  // 18 originals + 18 clones, all dropped exactly once.
  assert_eq!(drops.load(std::sync::atomic::Ordering::Relaxed), 36);
}

#[test]
fn sync_batch_empty_and_zero_max() {
  let (tx, rx) = spmc::bounded::<u32>(4);
  assert_eq!(tx.try_send_batch(Vec::new()).unwrap(), 0);
  assert_eq!(tx.send_batch(Vec::new()).unwrap(), 0);
  let mut empty = Vec::new();
  assert_eq!(tx.try_send_batch_mut(&mut empty).unwrap(), 0);
  assert_eq!(rx.try_recv_batch(0).unwrap(), Vec::<u32>::new());
  assert_eq!(rx.recv_batch(0).unwrap(), Vec::<u32>::new());
}

// --- Async tests ---

#[cfg(not(miri))]
mod async_tests {
  use super::*;
  use tokio::time::timeout;

  #[tokio::test]
  async fn async_send_batch_all_fit() {
    let (tx, rx) = spmc::bounded_async::<u32>(8);
    assert_eq!(tx.send_batch(vec![1, 2, 3]).await.unwrap(), 3);
    assert_eq!(rx.recv_batch(8).await.unwrap(), vec![1, 2, 3]);
  }

  #[tokio::test]
  async fn async_send_batch_blocks_then_completes() {
    let (tx, rx) = spmc::bounded_async::<usize>(4);
    let send_task = tokio::spawn(async move { tx.send_batch((0..16).collect()).await.unwrap() });

    let mut got = Vec::new();
    while got.len() < 16 {
      let batch = timeout(LONG_TIMEOUT, rx.recv_batch(8)).await.unwrap().unwrap();
      got.extend(batch);
    }
    assert_eq!(send_task.await.unwrap(), 16);
    assert_eq!(got, (0..16).collect::<Vec<_>>());
  }

  #[tokio::test]
  async fn async_send_batch_mut_cancel_safe() {
    let (tx, rx) = spmc::bounded_async::<u32>(2);
    let mut items = vec![1, 2, 3, 4, 5];
    {
      let fut = tx.send_batch_mut(&mut items);
      let res = timeout(Duration::from_millis(100), fut).await;
      assert!(res.is_err(), "future should be pending: buffer fills at 2");
    }
    assert_eq!(items, vec![3, 4, 5]);
    assert_eq!(rx.try_recv_batch(10).unwrap(), vec![1, 2]);
  }

  #[tokio::test]
  async fn async_send_batch_closed_mid_batch() {
    let (tx, rx) = spmc::bounded_async::<u32>(2);
    let send_task = tokio::spawn(async move { tx.send_batch((0..10).collect()).await });

    tokio::time::sleep(Duration::from_millis(50)).await;
    drop(rx);

    let err = send_task.await.unwrap().unwrap_err();
    assert_eq!(err.sent + err.unsent.len(), 10);
    assert_eq!(err.unsent, ((err.sent as u32)..10).collect::<Vec<_>>());
  }

  #[tokio::test]
  async fn async_fanout_recv_batch() {
    let (tx, rx1) = spmc::bounded_async::<u32>(16);
    let rx2 = rx1.clone();

    assert_eq!(tx.send_batch((0..8).collect()).await.unwrap(), 8);

    for rx in [rx1, rx2] {
      let mut got = Vec::new();
      while got.len() < 8 {
        let mut out = Vec::new();
        let k = rx.recv_batch_mut(&mut out, 3).await.unwrap();
        assert_eq!(k, out.len());
        got.extend(out);
      }
      assert_eq!(got, (0..8).collect::<Vec<_>>());
    }
  }

  #[tokio::test]
  async fn async_recv_batch_cancel_safe() {
    let (tx, rx) = spmc::bounded_async::<u32>(4);
    {
      let fut = rx.recv_batch(4);
      let res = timeout(Duration::from_millis(50), fut).await;
      assert!(res.is_err(), "future should be pending on empty channel");
    }
    tx.try_send_batch(vec![1, 2]).unwrap();
    assert_eq!(rx.recv_batch(4).await.unwrap(), vec![1, 2]);
  }

  #[tokio::test]
  async fn async_recv_batch_disconnected() {
    let (tx, rx) = spmc::bounded_async::<u32>(4);
    tx.try_send_batch(vec![7]).unwrap();
    drop(tx);
    assert_eq!(rx.recv_batch(4).await.unwrap(), vec![7]);
    assert_eq!(rx.recv_batch(4).await, Err(RecvError::Disconnected));
  }
}
