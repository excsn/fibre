//! Integration tests for the SPSC batch send/receive APIs.

mod common;

use common::*;

use fibre::error::{BatchSendErrorReason, RecvError, SendError, TryRecvError};
use fibre::spsc;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

// --- Sync tests ---

#[test]
fn sync_try_send_batch_all_fit() {
  let (p, c) = spsc::bounded_sync::<u32>(8);
  assert_eq!(p.try_send_batch(vec![1, 2, 3, 4]).unwrap(), 4);
  assert_eq!(c.len(), 4);
  assert_eq!(c.try_recv_batch(8).unwrap(), vec![1, 2, 3, 4]);
}

#[test]
fn sync_try_send_batch_partial_full() {
  let (p, c) = spsc::bounded_sync::<u32>(4);
  let err = p.try_send_batch((0..6).collect()).unwrap_err();
  assert_eq!(err.sent, 4);
  assert_eq!(err.reason, BatchSendErrorReason::Full);
  assert_eq!(err.unsent, vec![4, 5]);
  assert_eq!(c.try_recv_batch(10).unwrap(), vec![0, 1, 2, 3]);
}

#[test]
fn sync_try_send_batch_closed() {
  let (p, c) = spsc::bounded_sync::<u32>(4);
  drop(c);
  let err = p.try_send_batch(vec![1, 2]).unwrap_err();
  assert_eq!(err.sent, 0);
  assert_eq!(err.reason, BatchSendErrorReason::Closed);
  assert_eq!(err.into_unsent(), vec![1, 2]);
}

#[test]
fn sync_send_batch_empty_input() {
  let (p, c) = spsc::bounded_sync::<u32>(2);
  assert_eq!(p.try_send_batch(Vec::new()).unwrap(), 0);
  assert_eq!(p.send_batch(Vec::new()).unwrap(), 0);
  let mut empty = Vec::new();
  assert_eq!(p.try_send_batch_mut(&mut empty).unwrap(), 0);
  assert_eq!(p.send_batch_mut(&mut empty).unwrap(), 0);
  assert_eq!(c.try_recv_batch(0).unwrap(), Vec::<u32>::new());
  assert_eq!(c.recv_batch(0).unwrap(), Vec::<u32>::new());
}

#[test]
fn sync_try_send_batch_mut_drains_front() {
  let (p, c) = spsc::bounded_sync::<u32>(3);
  let mut items = vec![10, 20, 30, 40, 50];
  assert_eq!(p.try_send_batch_mut(&mut items).unwrap(), 3);
  assert_eq!(items, vec![40, 50]);
  // Channel full now.
  assert_eq!(p.try_send_batch_mut(&mut items).unwrap(), 0);
  assert_eq!(items, vec![40, 50]);
  assert_eq!(c.try_recv_batch(10).unwrap(), vec![10, 20, 30]);
  assert_eq!(p.try_send_batch_mut(&mut items).unwrap(), 2);
  assert!(items.is_empty());
}

#[test]
fn sync_try_send_batch_mut_closed() {
  let (p, c) = spsc::bounded_sync::<u32>(3);
  drop(c);
  let mut items = vec![1, 2];
  assert_eq!(p.try_send_batch_mut(&mut items), Err(SendError::Closed));
  assert_eq!(items, vec![1, 2]);
}

#[test]
fn sync_send_batch_blocks_until_consumed() {
  let (p, c) = spsc::bounded_sync::<usize>(4);
  let producer = thread::spawn(move || p.send_batch((0..32).collect()).unwrap());

  let mut received = Vec::new();
  while received.len() < 32 {
    match c.recv_batch_mut(&mut received, 8) {
      Ok(_) => {}
      Err(e) => panic!("unexpected recv error: {e:?}"),
    }
  }
  assert_eq!(producer.join().unwrap(), 32);
  assert_eq!(received, (0..32).collect::<Vec<_>>());
}

#[test]
fn sync_send_batch_mut_blocks_until_consumed() {
  let (p, c) = spsc::bounded_sync::<usize>(2);
  let producer = thread::spawn(move || {
    let mut items: Vec<usize> = (0..16).collect();
    let sent = p.send_batch_mut(&mut items).unwrap();
    assert_eq!(sent, 16);
    assert!(items.is_empty());
  });

  let mut received = Vec::new();
  while received.len() < 16 {
    c.recv_batch_mut(&mut received, 4).unwrap();
  }
  producer.join().unwrap();
  assert_eq!(received, (0..16).collect::<Vec<_>>());
}

#[test]
fn sync_send_batch_closed_mid_batch() {
  let (p, c) = spsc::bounded_sync::<u32>(2);
  let producer = thread::spawn(move || p.send_batch((0..10).collect()));

  // Take a couple of items, then drop the consumer while the producer is
  // parked with a remainder.
  assert_eq!(c.recv().unwrap(), 0);
  thread::sleep(Duration::from_millis(100));
  drop(c);

  let err = producer.join().unwrap().unwrap_err();
  assert!(err.sent >= 1, "at least one item must have been sent");
  assert_eq!(err.sent + err.unsent.len(), 10);
  // The unsent remainder must be the exact suffix.
  assert_eq!(
    err.unsent,
    ((err.sent as u32)..10).collect::<Vec<_>>()
  );
}

#[test]
fn sync_send_batch_mut_closed_leaves_remainder() {
  let (p, c) = spsc::bounded_sync::<u32>(2);
  let producer = thread::spawn(move || {
    let mut items: Vec<u32> = (0..10).collect();
    let res = p.send_batch_mut(&mut items);
    (res, items)
  });

  assert_eq!(c.recv().unwrap(), 0);
  thread::sleep(Duration::from_millis(100));
  drop(c);

  let (res, items) = producer.join().unwrap();
  assert_eq!(res, Err(SendError::Closed));
  assert!(!items.is_empty());
  // Remainder is the exact unsent suffix.
  let sent = 10 - items.len() as u32;
  assert_eq!(items, (sent..10).collect::<Vec<_>>());
}

#[test]
fn sync_recv_batch_blocks_until_first_item() {
  let (p, c) = spsc::bounded_sync::<u32>(8);
  let consumer = thread::spawn(move || c.recv_batch(8).unwrap());
  thread::sleep(Duration::from_millis(100));
  p.try_send_batch(vec![7, 8, 9]).unwrap();
  let got = consumer.join().unwrap();
  assert!(!got.is_empty() && got.len() <= 3);
  assert_eq!(got[0], 7);
}

#[test]
fn sync_recv_batch_respects_max() {
  let (p, c) = spsc::bounded_sync::<u32>(8);
  p.try_send_batch((0..8).collect()).unwrap();
  let first = c.recv_batch(3).unwrap();
  assert_eq!(first, vec![0, 1, 2]);
  let mut out = vec![99];
  assert_eq!(c.recv_batch_mut(&mut out, 3).unwrap(), 3);
  assert_eq!(out, vec![99, 3, 4, 5]);
}

#[test]
fn sync_recv_batch_disconnected() {
  let (p, c) = spsc::bounded_sync::<u32>(4);
  p.try_send_batch(vec![1, 2]).unwrap();
  drop(p);
  // Drains remaining items first.
  assert_eq!(c.recv_batch(10).unwrap(), vec![1, 2]);
  assert_eq!(c.recv_batch(10), Err(RecvError::Disconnected));
  assert_eq!(c.try_recv_batch(10), Err(TryRecvError::Disconnected));
}

#[test]
fn sync_try_recv_batch_empty() {
  let (_p, c) = spsc::bounded_sync::<u32>(4);
  assert_eq!(c.try_recv_batch(4), Err(TryRecvError::Empty));
}

#[test]
fn sync_batch_wrap_around() {
  let (p, c) = spsc::bounded_sync::<usize>(5);
  // Advance the indices so batches repeatedly cross the ring seam.
  for cycle in 0..20 {
    let base = cycle * 3;
    assert_eq!(p.try_send_batch(vec![base, base + 1, base + 2]).unwrap(), 3);
    assert_eq!(c.try_recv_batch(3).unwrap(), vec![base, base + 1, base + 2]);
  }
}

#[test]
fn sync_batch_stress_interleaved() {
  #[cfg(not(miri))]
  const ITEMS: usize = 100_000;
  #[cfg(miri)]
  const ITEMS: usize = 2_000;
  const CAPACITY: usize = 64;

  let (p, c) = spsc::bounded_sync::<usize>(CAPACITY);

  let producer = thread::spawn(move || {
    let mut next = 0;
    while next < ITEMS {
      let batch: Vec<usize> = (next..(next + 17).min(ITEMS)).collect();
      next += batch.len();
      p.send_batch(batch).unwrap();
    }
  });

  let consumer = thread::spawn(move || {
    let mut expected = 0;
    while expected < ITEMS {
      let got = c.recv_batch(23).unwrap();
      for v in got {
        assert_eq!(v, expected);
        expected += 1;
      }
    }
  });

  producer.join().unwrap();
  consumer.join().unwrap();
}

#[test]
fn sync_batch_drop_correctness() {
  struct Droppable(Arc<AtomicUsize>);
  impl Drop for Droppable {
    fn drop(&mut self) {
      self.0.fetch_add(1, Ordering::Relaxed);
    }
  }

  let drops = Arc::new(AtomicUsize::new(0));
  {
    let (p, c) = spsc::bounded_sync::<Droppable>(2);
    let items: Vec<Droppable> = (0..5).map(|_| Droppable(drops.clone())).collect();
    let err = p.try_send_batch(items).unwrap_err();
    assert_eq!(err.sent, 2);
    assert_eq!(err.unsent.len(), 3);
    drop(err); // Drops the 3 unsent items.
    assert_eq!(drops.load(Ordering::Relaxed), 3);
    drop(p);
    drop(c); // Drops the 2 in-channel items.
  }
  assert_eq!(drops.load(Ordering::Relaxed), 5);
}

// --- Async tests ---

#[cfg(not(miri))]
mod async_tests {
  use super::*;
  use tokio::time::timeout;

  #[tokio::test]
  async fn async_send_batch_all_fit() {
    let (mut p, mut c) = spsc::bounded_async::<u32>(8);
    assert_eq!(p.send_batch(vec![1, 2, 3]).await.unwrap(), 3);
    assert_eq!(c.recv_batch(8).await.unwrap(), vec![1, 2, 3]);
  }

  #[tokio::test]
  async fn async_send_batch_blocks_then_completes() {
    let (mut p, mut c) = spsc::bounded_async::<usize>(4);

    let send_task = tokio::spawn(async move { p.send_batch((0..32).collect()).await.unwrap() });

    let mut received = Vec::new();
    while received.len() < 32 {
      let got = timeout(LONG_TIMEOUT, c.recv_batch(8)).await.unwrap().unwrap();
      received.extend(got);
    }
    assert_eq!(send_task.await.unwrap(), 32);
    assert_eq!(received, (0..32).collect::<Vec<_>>());
  }

  #[tokio::test]
  async fn async_send_batch_mut_cancel_safe_remainder() {
    let (mut p, mut c) = spsc::bounded_async::<u32>(2);
    let mut items = vec![1, 2, 3, 4, 5];
    {
      let fut = p.send_batch_mut(&mut items);
      // Poll once via a short timeout; the channel fills at 2 items.
      let res = timeout(Duration::from_millis(100), fut).await;
      assert!(res.is_err(), "future should still be pending");
    }
    // Cancellation (drop) leaves the unsent items in the caller's vec.
    assert_eq!(items, vec![3, 4, 5]);
    assert_eq!(c.try_recv_batch(10).unwrap(), vec![1, 2]);
  }

  #[tokio::test]
  async fn async_send_batch_closed_mid_batch() {
    let (mut p, c) = spsc::bounded_async::<u32>(2);
    let send_task = tokio::spawn(async move { p.send_batch((0..10).collect()).await });

    tokio::time::sleep(Duration::from_millis(50)).await;
    drop(c);

    let err = send_task.await.unwrap().unwrap_err();
    assert_eq!(err.sent + err.unsent.len(), 10);
    assert_eq!(err.unsent, ((err.sent as u32)..10).collect::<Vec<_>>());
  }

  #[tokio::test]
  async fn async_recv_batch_waits_for_first_item() {
    let (mut p, mut c) = spsc::bounded_async::<u32>(8);

    let recv_task = tokio::spawn(async move { c.recv_batch(8).await.unwrap() });
    tokio::time::sleep(Duration::from_millis(50)).await;
    p.try_send_batch(vec![5, 6]).unwrap();

    let got = timeout(LONG_TIMEOUT, recv_task).await.unwrap().unwrap();
    assert!(!got.is_empty() && got.len() <= 2);
    assert_eq!(got[0], 5);
  }

  #[tokio::test]
  async fn async_recv_batch_mut_appends() {
    let (mut p, mut c) = spsc::bounded_async::<u32>(8);
    p.try_send_batch(vec![1, 2, 3]).unwrap();
    let mut out = vec![0];
    assert_eq!(c.recv_batch_mut(&mut out, 2).await.unwrap(), 2);
    assert_eq!(out, vec![0, 1, 2]);
  }

  #[tokio::test]
  async fn async_recv_batch_disconnected() {
    let (mut p, mut c) = spsc::bounded_async::<u32>(4);
    p.try_send_batch(vec![9]).unwrap();
    drop(p);
    assert_eq!(c.recv_batch(4).await.unwrap(), vec![9]);
    assert_eq!(c.recv_batch(4).await, Err(RecvError::Disconnected));
  }

  #[tokio::test]
  async fn async_recv_batch_cancel_safe() {
    let (mut p, mut c) = spsc::bounded_async::<u32>(4);
    {
      let fut = c.recv_batch(4);
      let res = timeout(Duration::from_millis(50), fut).await;
      assert!(res.is_err(), "future should be pending on empty channel");
    }
    // Dropping the pending future must not lose items or break the channel.
    p.try_send_batch(vec![1, 2]).unwrap();
    assert_eq!(c.recv_batch(4).await.unwrap(), vec![1, 2]);
  }
}
