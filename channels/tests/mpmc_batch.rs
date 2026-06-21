//! Integration tests for the MPMC v2 batch send/receive APIs.

mod common;

#[allow(unused_imports)]
use common::*;

use fibre::error::{BatchSendErrorReason, RecvError, SendError, TryRecvError};
use fibre::mpmc;

use std::collections::HashSet;
use std::thread;
use std::time::Duration;

// --- Sync tests ---

#[test]
fn sync_try_send_batch_all_fit() {
  let (tx, rx) = mpmc::bounded::<u32>(8);
  assert_eq!(tx.try_send_batch(vec![1, 2, 3]).unwrap(), 3);
  assert_eq!(rx.try_recv_batch(8).unwrap(), vec![1, 2, 3]);
}

#[test]
fn sync_try_send_batch_partial_full() {
  let (tx, rx) = mpmc::bounded::<u32>(4);
  let err = tx.try_send_batch((0..6).collect()).unwrap_err();
  assert_eq!(err.sent, 4);
  assert_eq!(err.reason, BatchSendErrorReason::Full);
  assert_eq!(err.unsent, vec![4, 5]);
  assert_eq!(rx.try_recv_batch(10).unwrap(), vec![0, 1, 2, 3]);
}

#[test]
fn sync_try_send_batch_closed() {
  let (tx, rx) = mpmc::bounded::<u32>(4);
  drop(rx);
  let err = tx.try_send_batch(vec![1, 2]).unwrap_err();
  assert_eq!(err.sent, 0);
  assert_eq!(err.reason, BatchSendErrorReason::Closed);
  assert_eq!(err.into_unsent(), vec![1, 2]);
}

#[test]
fn sync_unbounded_send_batch_never_full() {
  let (tx, rx) = mpmc::unbounded::<usize>();
  assert_eq!(tx.try_send_batch((0..1000).collect()).unwrap(), 1000);
  let mut all = Vec::new();
  while all.len() < 1000 {
    rx.recv_batch_mut(&mut all, 128).unwrap();
  }
  assert_eq!(all, (0..1000).collect::<Vec<_>>());
}

#[test]
fn sync_send_batch_blocks_until_drained() {
  let (tx, rx) = mpmc::bounded::<usize>(4);
  let producer = thread::spawn(move || tx.send_batch((0..32).collect()).unwrap());

  let mut received = Vec::new();
  while received.len() < 32 {
    rx.recv_batch_mut(&mut received, 8).unwrap();
  }
  assert_eq!(producer.join().unwrap(), 32);
  assert_eq!(received, (0..32).collect::<Vec<_>>());
}

#[test]
fn sync_send_batch_mut_partial_then_closed() {
  let (tx, rx) = mpmc::bounded::<u32>(3);
  let mut items = vec![10, 20, 30, 40, 50];
  assert_eq!(tx.try_send_batch_mut(&mut items).unwrap(), 3);
  assert_eq!(items, vec![40, 50]);
  assert_eq!(tx.try_send_batch_mut(&mut items).unwrap(), 0);
  drop(rx);
  assert_eq!(tx.try_send_batch_mut(&mut items), Err(SendError::Closed));
  assert_eq!(items, vec![40, 50]);
}

#[test]
fn sync_send_batch_closed_mid_batch() {
  let (tx, rx) = mpmc::bounded::<u32>(2);
  let producer = thread::spawn(move || tx.send_batch((0..10).collect()));

  assert_eq!(rx.recv().unwrap(), 0);
  thread::sleep(Duration::from_millis(100));
  drop(rx);

  let err = producer.join().unwrap().unwrap_err();
  assert!(err.sent >= 1);
  assert_eq!(err.sent + err.unsent.len(), 10);
  assert_eq!(err.unsent, ((err.sent as u32)..10).collect::<Vec<_>>());
}

#[test]
fn sync_recv_batch_blocks_until_first() {
  let (tx, rx) = mpmc::bounded::<u32>(8);
  let consumer = thread::spawn(move || rx.recv_batch(8).unwrap());
  thread::sleep(Duration::from_millis(100));
  tx.try_send_batch(vec![1, 2, 3]).unwrap();
  let got = consumer.join().unwrap();
  assert!(!got.is_empty() && got.len() <= 3);
  assert_eq!(got[0], 1);
}

#[test]
fn sync_recv_batch_disconnected() {
  let (tx, rx) = mpmc::bounded::<u32>(4);
  tx.try_send_batch(vec![1, 2]).unwrap();
  drop(tx);
  assert_eq!(rx.recv_batch(10).unwrap(), vec![1, 2]);
  assert_eq!(rx.recv_batch(10), Err(RecvError::Disconnected));
  assert_eq!(rx.try_recv_batch(10), Err(TryRecvError::Disconnected));
}

#[test]
fn sync_mp_mc_batch_totals() {
  #[cfg(not(miri))]
  const BATCHES: usize = 50;
  #[cfg(miri)]
  const BATCHES: usize = 3;
  const BATCH_SIZE: usize = 9;
  const PRODUCERS: usize = 3;
  const CONSUMERS: usize = 3;

  let (tx, rx) = mpmc::bounded::<usize>(16);
  let mut producers = Vec::new();
  for p in 0..PRODUCERS {
    let tx = tx.clone();
    producers.push(thread::spawn(move || {
      for b in 0..BATCHES {
        let base = (p * BATCHES + b) * BATCH_SIZE;
        tx.send_batch((base..base + BATCH_SIZE).collect()).unwrap();
      }
    }));
  }
  drop(tx);

  let mut consumers = Vec::new();
  for _ in 0..CONSUMERS {
    let rx = rx.clone();
    consumers.push(thread::spawn(move || {
      let mut got = Vec::new();
      loop {
        match rx.recv_batch_mut(&mut got, 7) {
          Ok(_) => {}
          Err(RecvError::Disconnected) => break,
        }
      }
      got
    }));
  }
  drop(rx);

  for p in producers {
    p.join().unwrap();
  }
  let mut all: Vec<usize> = Vec::new();
  for c in consumers {
    all.extend(c.join().unwrap());
  }
  assert_eq!(all.len(), PRODUCERS * BATCHES * BATCH_SIZE);
  let unique: HashSet<usize> = all.iter().copied().collect();
  assert_eq!(unique.len(), all.len(), "no duplicates");
}

// --- Async tests ---

#[cfg(not(miri))]
mod async_tests {
  use super::*;
  use tokio::time::timeout;

  #[tokio::test]
  async fn async_send_batch_all_fit() {
    let (tx, rx) = mpmc::bounded_async::<u32>(8);
    assert_eq!(tx.send_batch(vec![1, 2, 3]).await.unwrap(), 3);
    assert_eq!(rx.recv_batch(8).await.unwrap(), vec![1, 2, 3]);
  }

  #[tokio::test]
  async fn async_send_batch_blocks_then_completes() {
    let (tx, rx) = mpmc::bounded_async::<usize>(4);
    let send_task = tokio::spawn(async move { tx.send_batch((0..32).collect()).await.unwrap() });

    let mut received = Vec::new();
    while received.len() < 32 {
      let got = timeout(LONG_TIMEOUT, rx.recv_batch(8)).await.unwrap().unwrap();
      received.extend(got);
    }
    assert_eq!(send_task.await.unwrap(), 32);
    assert_eq!(received, (0..32).collect::<Vec<_>>());
  }

  #[tokio::test]
  async fn async_send_batch_mut_cancel_safe() {
    let (tx, rx) = mpmc::bounded_async::<u32>(2);
    let mut items = vec![1, 2, 3, 4, 5];
    {
      let fut = tx.send_batch_mut(&mut items);
      let res = timeout(Duration::from_millis(100), fut).await;
      assert!(res.is_err(), "future should be pending: channel fills at 2");
    }
    assert_eq!(items, vec![3, 4, 5]);
    assert_eq!(rx.try_recv_batch(10).unwrap(), vec![1, 2]);
    // The waiter queue must be clean after cancellation.
    assert_eq!(tx.try_send_batch_mut(&mut items).unwrap(), 2);
  }

  #[tokio::test]
  async fn async_send_batch_closed_mid_batch() {
    let (tx, rx) = mpmc::bounded_async::<u32>(2);
    let send_task = tokio::spawn(async move { tx.send_batch((0..10).collect()).await });

    tokio::time::sleep(Duration::from_millis(50)).await;
    drop(rx);

    let err = send_task.await.unwrap().unwrap_err();
    assert_eq!(err.sent + err.unsent.len(), 10);
    assert_eq!(err.unsent, ((err.sent as u32)..10).collect::<Vec<_>>());
  }

  #[tokio::test]
  async fn async_recv_batch_cancel_safe() {
    let (tx, rx) = mpmc::bounded_async::<u32>(4);
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
    let (tx, rx) = mpmc::bounded_async::<u32>(4);
    tx.try_send_batch(vec![9]).unwrap();
    drop(tx);
    assert_eq!(rx.recv_batch(4).await.unwrap(), vec![9]);
    assert_eq!(rx.recv_batch(4).await, Err(RecvError::Disconnected));
  }

  #[tokio::test]
  async fn async_unbounded_batch_roundtrip() {
    let (tx, rx) = mpmc::unbounded_async::<usize>();
    assert_eq!(tx.send_batch((0..100).collect()).await.unwrap(), 100);
    let mut all = Vec::new();
    while all.len() < 100 {
      let mut out = Vec::new();
      rx.recv_batch_mut(&mut out, 33).await.unwrap();
      all.extend(out);
    }
    assert_eq!(all, (0..100).collect::<Vec<_>>());
  }
}
