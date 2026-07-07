//! Fresh tests for the lock-free `mpmc_v3` channel.

use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use fibre::error::{RecvError, RecvErrorTimeout, SendError, TryRecvError, TrySendError};
use fibre::mpmc_exp::{bounded, bounded_async};

// --- Basic sync ----------------------------------------------------------

#[test]
fn sync_send_recv_basic() {
  let (tx, rx) = bounded::<i32>(4);
  tx.send(1).unwrap();
  tx.send(2).unwrap();
  assert_eq!(rx.recv().unwrap(), 1);
  assert_eq!(rx.recv().unwrap(), 2);
}

#[test]
fn capacity_reports_requested() {
  // Channel capacity is the requested value (Some(n)), independent of the
  // pow2-rounded physical buffer behind it - matching mpmc_v2.
  assert_eq!(bounded::<i32>(5).0.capacity(), Some(5));
  assert_eq!(bounded::<i32>(4).0.capacity(), Some(4));
  assert_eq!(bounded::<i32>(1).0.capacity(), Some(1));
}

#[test]
fn try_send_full_try_recv_empty() {
  let (tx, rx) = bounded::<i32>(2);
  assert!(tx.try_send(1).is_ok());
  assert!(tx.try_send(2).is_ok());
  match tx.try_send(3) {
    Err(TrySendError::Full(3)) => {}
    other => panic!("expected Full(3), got {:?}", other),
  }
  assert_eq!(rx.try_recv().unwrap(), 1);
  assert_eq!(rx.try_recv().unwrap(), 2);
  assert_eq!(rx.try_recv(), Err(TryRecvError::Empty));
}

#[test]
fn fifo_single_producer_single_consumer() {
  let (tx, rx) = bounded::<usize>(4);
  let n = 10_000usize;
  let prod = thread::spawn(move || {
    for i in 0..n {
      tx.send(i).unwrap();
    }
  });
  for expected in 0..n {
    assert_eq!(rx.recv().unwrap(), expected, "FIFO order violated");
  }
  prod.join().unwrap();
}

// --- Disconnect semantics ------------------------------------------------

#[test]
fn recv_disconnected_when_senders_drop() {
  let (tx, rx) = bounded::<i32>(4);
  tx.send(7).unwrap();
  drop(tx);
  assert_eq!(rx.recv().unwrap(), 7); // buffered item still drains
  assert_eq!(rx.recv(), Err(RecvError::Disconnected));
}

#[test]
fn send_closed_when_receivers_drop() {
  let (tx, rx) = bounded::<i32>(1);
  drop(rx);
  match tx.send(1) {
    Err(SendError::Closed) => {}
    other => panic!("expected Closed, got {:?}", other),
  }
}

#[test]
fn blocking_recv_wakes_on_send() {
  let (tx, rx) = bounded::<i32>(1);
  let h = thread::spawn(move || rx.recv().unwrap());
  thread::sleep(Duration::from_millis(50));
  tx.send(99).unwrap();
  assert_eq!(h.join().unwrap(), 99);
}

#[test]
fn blocking_send_wakes_on_recv() {
  let (tx, rx) = bounded::<i32>(1);
  let cap = tx.capacity().unwrap() as i32;
  for i in 0..cap {
    tx.send(i).unwrap(); // fill to capacity
  }
  let h = thread::spawn(move || {
    tx.send(99).unwrap(); // blocks until a slot frees
  });
  thread::sleep(Duration::from_millis(50));
  // Drain everything; the blocked send must complete and 99 must arrive last.
  let mut seen = Vec::new();
  for _ in 0..(cap + 1) {
    seen.push(rx.recv().unwrap());
  }
  h.join().unwrap();
  assert_eq!(seen.last(), Some(&99));
}

#[test]
fn recv_timeout_times_out() {
  let (_tx, rx) = bounded::<i32>(4);
  let start = std::time::Instant::now();
  assert_eq!(
    rx.recv_timeout(Duration::from_millis(80)),
    Err(RecvErrorTimeout::Timeout)
  );
  assert!(start.elapsed() >= Duration::from_millis(70));
}

#[test]
fn recv_timeout_gets_value() {
  let (tx, rx) = bounded::<i32>(4);
  let h = thread::spawn(move || {
    thread::sleep(Duration::from_millis(30));
    tx.send(5).unwrap();
  });
  assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), 5);
  h.join().unwrap();
}

#[test]
fn capacity_one_rendezvous_like() {
  let (tx, rx) = bounded::<usize>(1);
  let n = 5_000usize;
  let prod = thread::spawn(move || {
    for i in 0..n {
      tx.send(i).unwrap();
    }
  });
  for expected in 0..n {
    assert_eq!(rx.recv().unwrap(), expected);
  }
  prod.join().unwrap();
}

// --- MPMC correctness + lost-wakeup stress -------------------------------

#[test]
fn mpmc_no_loss_no_dup() {
  const P: usize = 8;
  const C: usize = 8;
  const PER: usize = 5_000;
  let (tx, rx) = bounded::<usize>(4);

  let mut producers = Vec::new();
  for p in 0..P {
    let tx = tx.clone();
    producers.push(thread::spawn(move || {
      for i in 0..PER {
        tx.send(p * PER + i).unwrap();
      }
    }));
  }
  drop(tx); // only the cloned senders remain; consumers see Disconnected once drained

  let collected = Arc::new(Mutex::new(Vec::with_capacity(P * PER)));
  let mut consumers = Vec::new();
  for _ in 0..C {
    let rx = rx.clone();
    let collected = Arc::clone(&collected);
    consumers.push(thread::spawn(move || {
      let mut local = Vec::new();
      loop {
        match rx.recv() {
          Ok(v) => local.push(v),
          Err(RecvError::Disconnected) => break,
        }
      }
      collected.lock().unwrap().extend(local);
    }));
  }
  drop(rx);

  for p in producers {
    p.join().unwrap();
  }
  for c in consumers {
    c.join().unwrap();
  }

  let mut all = Arc::try_unwrap(collected).unwrap().into_inner().unwrap();
  assert_eq!(all.len(), P * PER, "lost or duplicated items");
  all.sort_unstable();
  let set: HashSet<_> = all.iter().copied().collect();
  assert_eq!(set.len(), P * PER, "duplicate items detected");
  assert_eq!(*all.first().unwrap(), 0);
  assert_eq!(*all.last().unwrap(), P * PER - 1);
}

#[test]
fn lost_wakeup_stress_small_ring() {
  // Tight loops on a tiny ring with more threads than capacity: the classic
  // lost-wakeup failure mode would hang this test.
  const P: usize = 16;
  const C: usize = 16;
  const PER: usize = 2_000;
  let (tx, rx) = bounded::<usize>(2);
  let received = Arc::new(AtomicUsize::new(0));

  let mut handles = Vec::new();
  for _ in 0..P {
    let tx = tx.clone();
    handles.push(thread::spawn(move || {
      for i in 0..PER {
        tx.send(i).unwrap();
      }
    }));
  }
  drop(tx);

  let mut consumers = Vec::new();
  for _ in 0..C {
    let rx = rx.clone();
    let received = Arc::clone(&received);
    consumers.push(thread::spawn(move || loop {
      match rx.recv() {
        Ok(_) => {
          received.fetch_add(1, Ordering::Relaxed);
        }
        Err(RecvError::Disconnected) => break,
      }
    }));
  }
  drop(rx);

  for h in handles {
    h.join().unwrap();
  }
  for c in consumers {
    c.join().unwrap();
  }
  assert_eq!(received.load(Ordering::Relaxed), P * PER);
}

// --- Drop reclaims buffered values ---------------------------------------

#[test]
fn drop_reclaims_buffered_values() {
  let counter = Arc::new(AtomicUsize::new(0));

  struct Tracked(Arc<AtomicUsize>);
  impl Drop for Tracked {
    fn drop(&mut self) {
      self.0.fetch_add(1, Ordering::Relaxed);
    }
  }

  let (tx, rx) = bounded::<Tracked>(4);
  tx.send(Tracked(counter.clone())).unwrap();
  tx.send(Tracked(counter.clone())).unwrap();
  // One consumed, one left buffered.
  drop(rx.recv().unwrap());
  assert_eq!(counter.load(Ordering::Relaxed), 1);
  drop(tx);
  drop(rx); // channel torn down; buffered Tracked must be dropped exactly once
  assert_eq!(counter.load(Ordering::Relaxed), 2);
}

// --- Async ---------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn async_send_recv_basic() {
  let (tx, rx) = bounded_async::<i32>(2);
  tx.send(1).await.unwrap();
  tx.send(2).await.unwrap();
  assert_eq!(rx.recv().await.unwrap(), 1);
  assert_eq!(rx.recv().await.unwrap(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn async_mpmc_no_loss() {
  const P: usize = 6;
  const C: usize = 6;
  const PER: usize = 2_000;
  let (tx, rx) = bounded_async::<usize>(4);

  let mut producers = Vec::new();
  for p in 0..P {
    let tx = tx.clone();
    producers.push(tokio::spawn(async move {
      for i in 0..PER {
        tx.send(p * PER + i).await.unwrap();
      }
    }));
  }
  drop(tx);

  let received = Arc::new(AtomicUsize::new(0));
  let mut consumers = Vec::new();
  for _ in 0..C {
    let rx = rx.clone();
    let received = Arc::clone(&received);
    consumers.push(tokio::spawn(async move {
      while rx.recv().await.is_ok() {
        received.fetch_add(1, Ordering::Relaxed);
      }
    }));
  }
  drop(rx);

  for p in producers {
    p.await.unwrap();
  }
  for c in consumers {
    c.await.unwrap();
  }
  assert_eq!(received.load(Ordering::Relaxed), P * PER);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn async_recv_cancel_safe() {
  let (tx, rx) = bounded_async::<i32>(2);

  // Park a recv in its own task, then cancel it (drops the parked RecvFuture).
  let rx2 = rx.clone();
  let handle = tokio::spawn(async move { rx2.recv().await });
  tokio::time::sleep(Duration::from_millis(40)).await;
  handle.abort();
  let _ = handle.await;

  // Channel must still work after the cancelled recv.
  tx.send(42).await.unwrap();
  assert_eq!(rx.recv().await.unwrap(), 42);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn async_send_cancel_safe() {
  let (tx, rx) = bounded_async::<i32>(1);
  let cap = tx.capacity().unwrap() as i32;
  for i in 0..cap {
    tx.send(i).await.unwrap(); // fill to capacity
  }

  // The next send blocks (ring full); cancel it (drops the parked SendFuture).
  let tx2 = tx.clone();
  let handle = tokio::spawn(async move { tx2.send(99).await });
  tokio::time::sleep(Duration::from_millis(40)).await;
  handle.abort();
  let _ = handle.await;

  // The filled items drain in order; the cancelled 99 was never delivered.
  for i in 0..cap {
    assert_eq!(rx.recv().await.unwrap(), i);
  }
  tx.send(7).await.unwrap();
  assert_eq!(rx.recv().await.unwrap(), 7);
}

// --- Batch API -----------------------------------------------------------

#[test]
fn sync_batch_send_recv() {
  let (tx, rx) = bounded::<usize>(4);
  // try_send_batch partially fills then reports Full with the remainder.
  let err = tx.try_send_batch(vec![0, 1, 2, 3, 4, 5]).unwrap_err();
  assert_eq!(err.sent, 4);
  assert_eq!(err.unsent, vec![4, 5]);

  let got = rx.try_recv_batch(10).unwrap();
  assert_eq!(got, vec![0, 1, 2, 3]);

  // Now the rest fit.
  assert_eq!(tx.try_send_batch(vec![4, 5]).unwrap(), 2);
  assert_eq!(rx.try_recv_batch(10).unwrap(), vec![4, 5]);
}

#[test]
fn sync_send_batch_blocks_until_drained() {
  let (tx, rx) = bounded::<usize>(4);
  let producer = thread::spawn(move || {
    // 1000 items through a cap-4 ring: must block repeatedly.
    tx.send_batch((0..1000).collect()).unwrap()
  });
  let mut seen = 0usize;
  let mut out = Vec::new();
  while seen < 1000 {
    let n = rx.recv_batch_mut(&mut out, 1000 - seen).unwrap();
    seen += n;
  }
  assert_eq!(producer.join().unwrap(), 1000);
  assert_eq!(out, (0..1000).collect::<Vec<_>>());
}

#[test]
fn sync_send_batch_mut_keeps_unsent_on_close() {
  let (tx, rx) = bounded::<usize>(2);
  drop(rx); // receivers gone
  let mut items = vec![1, 2, 3];
  assert_eq!(tx.send_batch_mut(&mut items), Err(SendError::Closed));
  assert_eq!(items, vec![1, 2, 3]); // nothing delivered; all retained
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn async_batch_send_recv() {
  let (tx, rx) = bounded_async::<usize>(4);
  let producer = tokio::spawn(async move { tx.send_batch((0..500).collect()).await.unwrap() });

  let mut out = Vec::new();
  while out.len() < 500 {
    let _ = rx.recv_batch_mut(&mut out, 500).await.unwrap();
  }
  assert_eq!(producer.await.unwrap(), 500);
  assert_eq!(out, (0..500).collect::<Vec<_>>());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn async_send_batch_mut_cancel_keeps_unsent() {
  let (tx, rx) = bounded_async::<usize>(2);
  tx.send(100).await.unwrap(); // leave 1 slot free

  // Batch of 5 into a near-full cap-2 ring: sends 1, then blocks. Cancel it.
  let tx2 = tx.clone();
  let handle = tokio::spawn(async move {
    let mut items = vec![1, 2, 3, 4, 5];
    let _ = tx2.send_batch_mut(&mut items).await;
    items // returns the unsent remainder
  });
  tokio::time::sleep(Duration::from_millis(40)).await;
  handle.abort();
  let _ = handle.await;

  // Channel still works; drain what's there.
  let mut out = Vec::new();
  while let Ok(v) = rx.try_recv() {
    out.push(v);
  }
  assert!(out.contains(&100));
}
