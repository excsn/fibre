//! Miri suite for the MPMC channel (mpmc_v2): bounded and unbounded under
//! multi-producer multi-consumer races, waiter registration/cancel paths.

use fibre::error::TryRecvError;
use fibre::mpmc;
use fibre_miri::{block_on, drop_counter, drops, poll_once, DropCounter, ITEMS_SMALL};

use std::pin::pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;

#[test]
fn bounded_2p_2c_race() {
  let (tx, rx) = mpmc::bounded::<usize>(2);
  let total = ITEMS_SMALL * 2;
  let sum = Arc::new(AtomicUsize::new(0));
  let mut handles = Vec::new();
  for _ in 0..2 {
    let txc = tx.clone();
    handles.push(thread::spawn(move || {
      for i in 1..=ITEMS_SMALL {
        txc.send(i).unwrap();
      }
    }));
  }
  drop(tx);
  for _ in 0..2 {
    let rxc = rx.clone();
    let sumc = sum.clone();
    handles.push(thread::spawn(move || {
      while let Ok(v) = rxc.recv() {
        sumc.fetch_add(v, Ordering::Relaxed);
      }
    }));
  }
  drop(rx);
  for h in handles {
    h.join().unwrap();
  }
  let _ = total;
  assert_eq!(
    sum.load(Ordering::Relaxed),
    2 * (ITEMS_SMALL * (ITEMS_SMALL + 1) / 2)
  );
}

#[test]
fn unbounded_smoke_and_ownership() {
  let counter = drop_counter();
  {
    let (mut tx, mut rx) = mpmc::unbounded();
    for _ in 0..ITEMS_SMALL {
      tx.send(DropCounter::new(&counter)).unwrap();
    }
    for _ in 0..10 {
      drop(rx.recv().unwrap());
    }
    drop(tx);
    drop(rx);
  }
  assert_eq!(drops(&counter), ITEMS_SMALL);
}

#[test]
fn unbounded_2p_2c_race() {
  let (tx, rx) = mpmc::unbounded::<usize>();
  let sum = Arc::new(AtomicUsize::new(0));
  let mut handles = Vec::new();
  for _ in 0..2 {
    let mut txc = tx.clone();
    handles.push(thread::spawn(move || {
      for i in 1..=ITEMS_SMALL {
        txc.send(i).unwrap();
      }
    }));
  }
  drop(tx);
  for _ in 0..2 {
    let mut rxc = rx.clone();
    let sumc = sum.clone();
    handles.push(thread::spawn(move || {
      while let Ok(v) = rxc.recv() {
        sumc.fetch_add(v, Ordering::Relaxed);
      }
    }));
  }
  drop(rx);
  for h in handles {
    h.join().unwrap();
  }
  assert_eq!(
    sum.load(Ordering::Relaxed),
    2 * (ITEMS_SMALL * (ITEMS_SMALL + 1) / 2)
  );
}

#[test]
fn unbounded_fulfilled_then_dropped_future_reclaims_item() {
  let counter = drop_counter();
  let (mut tx, mut rx) = mpmc::unbounded_async();
  {
    let mut fut = pin!(rx.recv());
    assert!(poll_once(fut.as_mut()).is_pending()); // registered as a waiter
    tx.try_send(DropCounter::new(&counter)).unwrap(); // handoff fulfills it
    // Dropped without being polled again: the delivered value must be
    // reclaimed, not leaked or double-dropped.
  }
  assert_eq!(drops(&counter), 0);
  drop(rx.try_recv().unwrap());
  assert_eq!(drops(&counter), 1);
  drop(tx);
}

#[test]
fn unbounded_disconnect_wakes_parked_consumers() {
  let (tx, mut rx) = mpmc::unbounded::<u32>();
  let mut rx2 = rx.clone();
  let c1 = thread::spawn(move || rx.recv());
  let c2 = thread::spawn(move || rx2.recv());
  drop(tx);
  assert!(c1.join().unwrap().is_err());
  assert!(c2.join().unwrap().is_err());
}

#[test]
fn async_send_future_registers_then_cancels() {
  let (tx, rx) = mpmc::bounded_async::<u32>(1);
  block_on(tx.send(1)).unwrap();
  {
    let mut fut = pin!(tx.send(2));
    assert!(poll_once(fut.as_mut()).is_pending());
  } // cancelled while waiting for capacity
  assert_eq!(block_on(rx.recv()).unwrap(), 1);
  block_on(tx.send(3)).unwrap();
  assert_eq!(block_on(rx.recv()).unwrap(), 3);
}

#[test]
fn async_recv_future_registers_then_cancels() {
  let (tx, rx) = mpmc::bounded_async::<u32>(2);
  {
    let mut fut = pin!(rx.recv());
    assert!(poll_once(fut.as_mut()).is_pending());
  }
  assert_eq!(rx.try_recv(), Err(TryRecvError::Empty));
  block_on(tx.send(6)).unwrap();
  assert_eq!(block_on(rx.recv()).unwrap(), 6);
}

#[test]
fn mixed_sync_async_cross_thread() {
  let (tx, rx) = mpmc::bounded_async::<usize>(2);
  let stx = tx.to_sync();
  let producer = thread::spawn(move || {
    for i in 0..ITEMS_SMALL {
      stx.send(i).unwrap();
    }
  });
  for i in 0..ITEMS_SMALL {
    assert_eq!(block_on(rx.recv()).unwrap(), i);
  }
  producer.join().unwrap();
}

#[test]
fn cancelled_send_future_drops_value_exactly_once() {
  let counter = drop_counter();
  let (tx, rx) = mpmc::bounded_async(1);
  block_on(tx.send(DropCounter::new(&counter))).unwrap();
  {
    let mut fut = pin!(tx.send(DropCounter::new(&counter)));
    assert!(poll_once(fut.as_mut()).is_pending());
  }
  assert_eq!(drops(&counter), 1);
  drop(block_on(rx.recv()).unwrap());
  assert_eq!(drops(&counter), 2);
}

#[test]
fn batch_roundtrip() {
  let (tx, rx) = mpmc::bounded::<usize>(8);
  let producer = thread::spawn(move || {
    let mut next = 0;
    while next < ITEMS_SMALL {
      let batch: Vec<usize> = (next..(next + 5).min(ITEMS_SMALL)).collect();
      next += batch.len();
      tx.send_batch(batch).unwrap();
    }
  });
  let mut expected = 0;
  while expected < ITEMS_SMALL {
    for v in rx.recv_batch(3).unwrap() {
      assert_eq!(v, expected);
      expected += 1;
    }
  }
  producer.join().unwrap();
}

#[test]
fn two_async_consumers_cross_thread() {
  let (tx, rx) = mpmc::bounded_async::<usize>(2);
  let rx2 = rx.clone();
  let total = ITEMS_SMALL * 2;
  let c1 = thread::spawn(move || {
    let mut n = 0;
    while block_on(rx.recv()).is_ok() {
      n += 1;
    }
    n
  });
  let c2 = thread::spawn(move || {
    let mut n = 0;
    while block_on(rx2.recv()).is_ok() {
      n += 1;
    }
    n
  });
  for i in 0..total {
    block_on(tx.send(i)).unwrap();
  }
  drop(tx);
  let n = c1.join().unwrap() + c2.join().unwrap();
  assert_eq!(n, total);
}
