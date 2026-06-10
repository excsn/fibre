//! Integration tests for the MPSC batch send/receive APIs (unbounded + bounded).

mod common;

#[allow(unused_imports)]
use common::*;

use fibre::error::{BatchSendErrorReason, RecvError, SendError, TryRecvError};
use fibre::mpsc;

use std::thread;
use std::time::Duration;

// --- Unbounded sync tests ---

#[test]
fn unbounded_sync_send_batch_basic() {
  let (tx, rx) = mpsc::unbounded::<u32>();
  assert_eq!(tx.send_batch(vec![1, 2, 3]).unwrap(), 3);
  assert_eq!(tx.len(), 3);
  assert_eq!(rx.try_recv_batch(10).unwrap(), vec![1, 2, 3]);
  assert_eq!(rx.len(), 0);
}

#[test]
fn unbounded_sync_send_batch_block_boundaries() {
  // BLOCK_CAPACITY is 32; cover sizes around it.
  for &n in &[1usize, 31, 32, 33, 100] {
    let (tx, rx) = mpsc::unbounded::<usize>();
    assert_eq!(tx.send_batch((0..n).collect()).unwrap(), n);
    let mut out = Vec::new();
    while out.len() < n {
      rx.recv_batch_mut(&mut out, 7).unwrap();
    }
    assert_eq!(out, (0..n).collect::<Vec<_>>());
  }
}

#[test]
fn unbounded_sync_send_batch_closed() {
  let (tx, rx) = mpsc::unbounded::<u32>();
  drop(rx);
  let err = tx.try_send_batch(vec![1, 2]).unwrap_err();
  assert_eq!(err.sent, 0);
  assert_eq!(err.reason, BatchSendErrorReason::Closed);
  assert_eq!(err.into_unsent(), vec![1, 2]);

  let mut items = vec![3, 4];
  assert_eq!(tx.send_batch_mut(&mut items), Err(SendError::Closed));
  assert_eq!(items, vec![3, 4]);
}

#[test]
fn unbounded_sync_send_batch_mut_drains_all() {
  let (tx, rx) = mpsc::unbounded::<u32>();
  let mut items = vec![1, 2, 3, 4];
  assert_eq!(tx.send_batch_mut(&mut items).unwrap(), 4);
  assert!(items.is_empty());
  assert_eq!(rx.recv_batch(10).unwrap(), vec![1, 2, 3, 4]);
}

#[test]
fn unbounded_sync_recv_batch_blocks_until_first() {
  let (tx, rx) = mpsc::unbounded::<u32>();
  let consumer = thread::spawn(move || rx.recv_batch(16).unwrap());
  thread::sleep(Duration::from_millis(100));
  tx.send_batch(vec![1, 2, 3]).unwrap();
  let got = consumer.join().unwrap();
  assert!(!got.is_empty() && got.len() <= 3);
  assert_eq!(got[0], 1);
}

#[test]
fn unbounded_sync_recv_batch_disconnected() {
  let (tx, rx) = mpsc::unbounded::<u32>();
  tx.send_batch(vec![1, 2]).unwrap();
  drop(tx);
  assert_eq!(rx.recv_batch(10).unwrap(), vec![1, 2]);
  assert_eq!(rx.recv_batch(10), Err(RecvError::Disconnected));
  assert_eq!(rx.try_recv_batch(10), Err(TryRecvError::Disconnected));
}

#[test]
fn unbounded_sync_multi_producer_batches() {
  #[cfg(not(miri))]
  const BATCHES: usize = 100;
  #[cfg(miri)]
  const BATCHES: usize = 5;
  const BATCH_SIZE: usize = 17;
  const PRODUCERS: usize = 4;

  let (tx, rx) = mpsc::unbounded::<usize>();
  let mut handles = Vec::new();
  for p in 0..PRODUCERS {
    let tx = tx.clone();
    handles.push(thread::spawn(move || {
      for b in 0..BATCHES {
        let base = (p * BATCHES + b) * BATCH_SIZE;
        tx.send_batch((base..base + BATCH_SIZE).collect()).unwrap();
      }
    }));
  }
  drop(tx);

  let mut all = Vec::new();
  loop {
    match rx.recv_batch_mut(&mut all, 64) {
      Ok(_) => {}
      Err(RecvError::Disconnected) => break,
    }
  }
  for h in handles {
    h.join().unwrap();
  }
  assert_eq!(all.len(), PRODUCERS * BATCHES * BATCH_SIZE);
  all.sort();
  for (i, v) in all.iter().enumerate() {
    assert_eq!(*v, i);
  }
}

#[test]
fn unbounded_sync_len_consistent_after_batches() {
  let (tx, rx) = mpsc::unbounded::<u32>();
  tx.send_batch((0..50).collect()).unwrap();
  assert_eq!(tx.len(), 50);
  let got = rx.try_recv_batch(20).unwrap();
  assert_eq!(got.len(), 20);
  assert_eq!(rx.len(), 30);
  let got = rx.try_recv_batch(100).unwrap();
  assert_eq!(got.len(), 30);
  assert_eq!(rx.len(), 0);
}

// --- Bounded sync tests ---

#[test]
fn bounded_sync_try_send_batch_partial() {
  let (tx, rx) = mpsc::bounded::<u32>(4);
  let err = tx.try_send_batch((0..6).collect()).unwrap_err();
  assert_eq!(err.sent, 4);
  assert_eq!(err.reason, BatchSendErrorReason::Full);
  assert_eq!(err.unsent, vec![4, 5]);
  assert_eq!(rx.try_recv_batch(10).unwrap(), vec![0, 1, 2, 3]);
}

#[test]
fn bounded_sync_try_send_batch_full_then_zero() {
  let (tx, _rx) = mpsc::bounded::<u32>(2);
  assert_eq!(tx.try_send_batch(vec![1, 2]).unwrap(), 2);
  let err = tx.try_send_batch(vec![3]).unwrap_err();
  assert_eq!(err.sent, 0);
  assert_eq!(err.reason, BatchSendErrorReason::Full);
}

#[test]
fn bounded_sync_send_batch_blocks_until_drained() {
  let (tx, rx) = mpsc::bounded::<usize>(4);
  let producer = thread::spawn(move || tx.send_batch((0..32).collect()).unwrap());

  let mut received = Vec::new();
  while received.len() < 32 {
    rx.recv_batch_mut(&mut received, 8).unwrap();
  }
  assert_eq!(producer.join().unwrap(), 32);
  assert_eq!(received, (0..32).collect::<Vec<_>>());
}

#[test]
fn bounded_sync_send_batch_mut_partial_progress() {
  let (tx, rx) = mpsc::bounded::<u32>(3);
  let mut items = vec![10, 20, 30, 40, 50];
  assert_eq!(tx.try_send_batch_mut(&mut items).unwrap(), 3);
  assert_eq!(items, vec![40, 50]);
  assert_eq!(tx.try_send_batch_mut(&mut items).unwrap(), 0);
  assert_eq!(rx.try_recv_batch(10).unwrap(), vec![10, 20, 30]);
  assert_eq!(tx.try_send_batch_mut(&mut items).unwrap(), 2);
  assert!(items.is_empty());
}

#[test]
fn bounded_sync_send_batch_closed_mid_batch() {
  let (tx, rx) = mpsc::bounded::<u32>(2);
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
fn bounded_sync_permit_conservation() {
  // Catches release_many accounting drift / Permit::into_parts leaks:
  // after repeated full-capacity batch send + batch recv cycles, the gate
  // must still allow a full-capacity batch.
  const CAPACITY: usize = 8;
  #[cfg(not(miri))]
  const CYCLES: usize = 100;
  #[cfg(miri)]
  const CYCLES: usize = 5;

  let (tx, rx) = mpsc::bounded::<usize>(CAPACITY);
  for cycle in 0..CYCLES {
    let base = cycle * CAPACITY;
    assert_eq!(
      tx.try_send_batch((base..base + CAPACITY).collect()).unwrap(),
      CAPACITY,
      "cycle {cycle}: gate must have all permits available"
    );
    let got = rx.try_recv_batch(CAPACITY).unwrap();
    assert_eq!(got.len(), CAPACITY);
    assert_eq!(got, (base..base + CAPACITY).collect::<Vec<_>>());
  }
  // And once more for good measure: the channel must be fully usable.
  assert_eq!(tx.try_send_batch((0..CAPACITY).collect()).unwrap(), CAPACITY);
}

#[test]
fn bounded_sync_rendezvous_try_send_batch_no_receiver_ready() {
  let (tx, _rx) = mpsc::bounded::<u32>(0);
  let err = tx.try_send_batch(vec![1, 2, 3]).unwrap_err();
  assert_eq!(err.sent, 0);
  assert_eq!(err.reason, BatchSendErrorReason::Full);
  assert_eq!(err.unsent, vec![1, 2, 3]);
}

#[test]
fn bounded_sync_rendezvous_send_batch_completes_via_recvs() {
  let (tx, rx) = mpsc::bounded::<u32>(0);
  let producer = thread::spawn(move || tx.send_batch(vec![1, 2, 3]).unwrap());

  // Rendezvous readiness permits are only delivered to senders already parked
  // at the gate (a permit released early is discarded), so let the producer
  // park before each recv — same sequencing as the existing single-item
  // rendezvous tests.
  let mut got = Vec::new();
  for _ in 0..3 {
    thread::sleep(Duration::from_millis(100));
    got.push(rx.recv().unwrap());
  }
  assert_eq!(producer.join().unwrap(), 3);
  assert_eq!(got, vec![1, 2, 3]);
}

#[test]
fn bounded_sync_rendezvous_recv_timeout_no_livelock() {
  // Regression test: recv_timeout on a capacity-0 channel releases a
  // readiness permit at entry AND inside its retry loop. The second release
  // used to hit the CapacityGate's empty-queue clamp and discard the permit
  // already committed to the woken (but not yet running) sender, which then
  // re-parked — livelocking the handoff forever. The gate now reserves
  // committed handoff permits out of the clamp's reach.
  let (tx, rx) = mpsc::bounded::<u32>(0);
  let producer = thread::spawn(move || tx.send_batch(vec![1, 2, 3, 4, 5]).unwrap());

  let mut got = Vec::new();
  let mut attempts = 0;
  while got.len() < 5 {
    attempts += 1;
    assert!(
      attempts < 200,
      "livelock: handoffs are not completing (got {:?})",
      got
    );
    match rx.recv_timeout(Duration::from_millis(50)) {
      Ok(v) => got.push(v),
      Err(fibre::error::RecvErrorTimeout::Timeout) => continue,
      Err(e) => panic!("unexpected error: {e:?}"),
    }
  }
  assert_eq!(producer.join().unwrap(), 5);
  assert_eq!(got, vec![1, 2, 3, 4, 5]);
}

#[test]
fn bounded_sync_recv_batch_blocks_until_first() {
  let (tx, rx) = mpsc::bounded::<u32>(8);
  let consumer = thread::spawn(move || rx.recv_batch(8).unwrap());
  thread::sleep(Duration::from_millis(100));
  tx.try_send_batch(vec![1, 2, 3]).unwrap();
  let got = consumer.join().unwrap();
  assert!(!got.is_empty() && got.len() <= 3);
  assert_eq!(got[0], 1);
}

#[test]
fn bounded_sync_recv_batch_disconnected() {
  let (tx, rx) = mpsc::bounded::<u32>(4);
  tx.try_send_batch(vec![1, 2]).unwrap();
  drop(tx);
  assert_eq!(rx.recv_batch(10).unwrap(), vec![1, 2]);
  assert_eq!(rx.recv_batch(10), Err(RecvError::Disconnected));
}

// --- Bounded async tests ---

#[cfg(not(miri))]
mod bounded_async_tests {
  use super::*;
  use tokio::time::timeout;

  #[tokio::test]
  async fn async_send_batch_all_fit() {
    let (tx, rx) = mpsc::bounded_async::<u32>(8);
    assert_eq!(tx.send_batch(vec![1, 2, 3]).await.unwrap(), 3);
    assert_eq!(rx.recv_batch(8).await.unwrap(), vec![1, 2, 3]);
  }

  #[tokio::test]
  async fn async_send_batch_rearms_until_complete() {
    let (tx, rx) = mpsc::bounded_async::<usize>(4);
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
    let (tx, rx) = mpsc::bounded_async::<u32>(2);
    let mut items = vec![1, 2, 3, 4, 5];
    {
      let fut = tx.send_batch_mut(&mut items);
      let res = timeout(Duration::from_millis(100), fut).await;
      assert!(res.is_err(), "future should be pending: channel fills at 2");
    }
    assert_eq!(items, vec![3, 4, 5]);
    assert_eq!(rx.try_recv_batch(10).unwrap(), vec![1, 2]);
    // The channel must remain fully functional after cancellation.
    assert_eq!(tx.try_send_batch_mut(&mut items).unwrap(), 2);
  }

  #[tokio::test]
  async fn async_send_batch_closed_mid_batch() {
    let (tx, rx) = mpsc::bounded_async::<u32>(2);
    let send_task = tokio::spawn(async move { tx.send_batch((0..10).collect()).await });

    tokio::time::sleep(Duration::from_millis(50)).await;
    drop(rx);

    let err = send_task.await.unwrap().unwrap_err();
    assert_eq!(err.sent + err.unsent.len(), 10);
    assert_eq!(err.unsent, ((err.sent as u32)..10).collect::<Vec<_>>());
  }

  #[tokio::test]
  async fn async_rendezvous_recv_batch_against_blocking_senders() {
    let (tx, rx) = mpsc::bounded_async::<u32>(0);
    let mut senders = Vec::new();
    for i in 0..3 {
      let tx = tx.clone();
      senders.push(tokio::spawn(async move { tx.send(i).await }));
    }
    drop(tx);

    // Let all senders park at the capacity gate before signalling readiness:
    // a rendezvous permit released with no parked sender is discarded.
    tokio::time::sleep(Duration::from_millis(50)).await;

    let mut got = Vec::new();
    while got.len() < 3 {
      let mut batch = timeout(LONG_TIMEOUT, rx.recv_batch(10)).await.unwrap().unwrap();
      got.append(&mut batch);
      tokio::task::yield_now().await;
    }
    got.sort();
    assert_eq!(got, vec![0, 1, 2]);
    for s in senders {
      s.await.unwrap().unwrap();
    }
  }

  #[tokio::test]
  async fn async_recv_batch_cancel_safe() {
    let (tx, rx) = mpsc::bounded_async::<u32>(4);
    {
      let fut = rx.recv_batch(4);
      let res = timeout(Duration::from_millis(50), fut).await;
      assert!(res.is_err());
    }
    tx.try_send_batch(vec![1, 2]).unwrap();
    assert_eq!(rx.recv_batch(4).await.unwrap(), vec![1, 2]);
  }
}

// --- Unbounded async tests ---

#[cfg(not(miri))]
mod unbounded_async_tests {
  use super::*;
  use tokio::time::timeout;

  #[tokio::test]
  async fn async_send_batch_completes_immediately() {
    let (tx, rx) = mpsc::unbounded_async::<u32>();
    assert_eq!(tx.send_batch(vec![1, 2, 3]).await.unwrap(), 3);
    assert_eq!(rx.recv_batch(10).await.unwrap(), vec![1, 2, 3]);
  }

  #[tokio::test]
  async fn async_send_batch_mut_drains() {
    let (tx, rx) = mpsc::unbounded_async::<u32>();
    let mut items = vec![5, 6, 7];
    assert_eq!(tx.send_batch_mut(&mut items).await.unwrap(), 3);
    assert!(items.is_empty());
    let mut out = Vec::new();
    assert_eq!(rx.recv_batch_mut(&mut out, 10).await.unwrap(), 3);
    assert_eq!(out, vec![5, 6, 7]);
  }

  #[tokio::test]
  async fn async_recv_batch_waits_for_items() {
    let (tx, rx) = mpsc::unbounded_async::<u32>();
    let recv_task = tokio::spawn(async move { rx.recv_batch(8).await.unwrap() });
    tokio::time::sleep(Duration::from_millis(50)).await;
    tx.try_send_batch(vec![9, 10]).unwrap();
    let got = timeout(LONG_TIMEOUT, recv_task).await.unwrap().unwrap();
    assert!(!got.is_empty() && got.len() <= 2);
    assert_eq!(got[0], 9);
  }

  #[tokio::test]
  async fn async_recv_batch_disconnected() {
    let (tx, rx) = mpsc::unbounded_async::<u32>();
    tx.try_send_batch(vec![1]).unwrap();
    drop(tx);
    assert_eq!(rx.recv_batch(4).await.unwrap(), vec![1]);
    assert_eq!(rx.recv_batch(4).await, Err(RecvError::Disconnected));
  }

  #[tokio::test]
  async fn async_recv_batch_cancel_safe() {
    let (tx, rx) = mpsc::unbounded_async::<u32>();
    {
      let fut = rx.recv_batch(4);
      let res = timeout(Duration::from_millis(50), fut).await;
      assert!(res.is_err(), "future should be pending on empty channel");
    }
    tx.try_send_batch(vec![1, 2]).unwrap();
    assert_eq!(rx.recv_batch(4).await.unwrap(), vec![1, 2]);
  }

  #[tokio::test]
  async fn async_send_batch_closed() {
    let (tx, rx) = mpsc::unbounded_async::<u32>();
    drop(rx);
    let err = tx.send_batch(vec![1, 2]).await.unwrap_err();
    assert_eq!(err.sent, 0);
    assert_eq!(err.into_unsent(), vec![1, 2]);
  }
}
