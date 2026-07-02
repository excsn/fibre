//! Miri suite for the bounded MPSC channel: Vyukov publish, node pool +
//! chunk-stack recycling, capacity park/wake.

use fibre::error::{RecvError, TryRecvError, TrySendError};
use fibre::mpsc;
use fibre_miri::{block_on, drop_counter, drops, poll_once, DropCounter, ITEMS_CROSSING};

use std::pin::pin;
use std::thread;

#[test]
fn capacity_blocks_and_recycles() {
  // Small capacity forces constant node recycling through the chunk stack.
  let (tx, rx) = mpsc::bounded::<usize>(2);
  let producer = thread::spawn(move || {
    for i in 0..ITEMS_CROSSING {
      tx.send(i).unwrap();
    }
  });
  for i in 0..ITEMS_CROSSING {
    assert_eq!(rx.recv().unwrap(), i);
  }
  producer.join().unwrap();
  assert_eq!(rx.recv(), Err(RecvError::Disconnected));
}

#[test]
fn multi_producer_race() {
  let (tx, rx) = mpsc::bounded::<(usize, usize)>(4);
  let mut handles = Vec::new();
  for p in 0..3usize {
    let txc = tx.clone();
    handles.push(thread::spawn(move || {
      for i in 0..60usize {
        txc.send((p, i)).unwrap();
      }
    }));
  }
  drop(tx);
  let mut next = [0usize; 3];
  for _ in 0..180 {
    let (p, i) = rx.recv().unwrap();
    assert_eq!(next[p], i);
    next[p] += 1;
  }
  for h in handles {
    h.join().unwrap();
  }
}

#[test]
fn try_send_full_and_values_dropped_once() {
  let counter = drop_counter();
  {
    let (tx, rx) = mpsc::bounded(2);
    tx.try_send(DropCounter::new(&counter)).unwrap();
    tx.try_send(DropCounter::new(&counter)).unwrap();
    match tx.try_send(DropCounter::new(&counter)) {
      Err(TrySendError::Full(v)) => drop(v),
      other => panic!("expected Full, got {:?}", other.map(|_| ())),
    }
    assert_eq!(drops(&counter), 1); // the rejected value
    drop(tx);
    drop(rx); // two buffered
  }
  assert_eq!(drops(&counter), 3);
}

#[test]
fn receiver_drop_closes_senders() {
  let (tx, rx) = mpsc::bounded::<u32>(2);
  drop(rx);
  assert!(tx.send(1).is_err());
  assert!(matches!(tx.try_send(2), Err(TrySendError::Closed(2))));
}

#[test]
fn batch_paths_recycle_chunks() {
  let (tx, rx) = mpsc::bounded::<usize>(8);
  let producer = thread::spawn(move || {
    let mut next = 0;
    while next < ITEMS_CROSSING {
      let batch: Vec<usize> = (next..(next + 5).min(ITEMS_CROSSING)).collect();
      next += batch.len();
      tx.send_batch(batch).unwrap();
    }
  });
  let mut expected = 0;
  while expected < ITEMS_CROSSING {
    for v in rx.recv_batch(7).unwrap() {
      assert_eq!(v, expected);
      expected += 1;
    }
  }
  producer.join().unwrap();
}

#[test]
fn async_send_future_registers_then_cancels() {
  let (tx, rx) = mpsc::bounded_async::<u32>(1);
  block_on(tx.send(1)).unwrap();
  {
    let mut fut = pin!(tx.send(2));
    assert!(poll_once(fut.as_mut()).is_pending()); // parked in async_send_waiters
  } // Drop must unregister the send waiter
  assert_eq!(block_on(rx.recv()).unwrap(), 1);
  block_on(tx.send(3)).unwrap();
  assert_eq!(block_on(rx.recv()).unwrap(), 3);
}

#[test]
fn async_recv_future_registers_then_cancels() {
  let (tx, rx) = mpsc::bounded_async::<u32>(2);
  {
    let mut fut = pin!(rx.recv());
    assert!(poll_once(fut.as_mut()).is_pending());
  }
  assert_eq!(rx.try_recv(), Err(TryRecvError::Empty));
  block_on(tx.send(4)).unwrap();
  assert_eq!(block_on(rx.recv()).unwrap(), 4);
}

#[test]
fn sync_producer_async_consumer_cross_thread() {
  let (tx, rx) = mpsc::bounded_async::<usize>(2);
  let stx = tx.to_sync();
  let producer = thread::spawn(move || {
    for i in 0..64 {
      stx.send(i).unwrap();
    }
  });
  for i in 0..64 {
    assert_eq!(block_on(rx.recv()).unwrap(), i);
  }
  producer.join().unwrap();
}

#[test]
fn batch_wake_of_multiple_parked_senders() {
  // Two senders park on a full cap-1 channel; the consumer's chunk flush must
  // wake them (notify_sync_senders' batch wake), letting both complete.
  let (tx, rx) = mpsc::bounded::<u32>(1);
  tx.send(0).unwrap(); // full
  let tx1 = tx.clone();
  let tx2 = tx.clone();
  drop(tx);
  let p1 = thread::spawn(move || tx1.send(1).unwrap());
  let p2 = thread::spawn(move || tx2.send(2).unwrap());
  let mut got = Vec::new();
  for _ in 0..3 {
    got.push(rx.recv().unwrap());
  }
  p1.join().unwrap();
  p2.join().unwrap();
  got.sort_unstable();
  assert_eq!(got, vec![0, 1, 2]);
}

#[test]
fn cancelled_send_future_drops_value_exactly_once() {
  let counter = drop_counter();
  let (tx, rx) = mpsc::bounded_async(1);
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
fn recv_timeout_zero_and_success() {
  let (tx, rx) = mpsc::bounded::<u32>(2);
  assert!(matches!(
    rx.recv_timeout(std::time::Duration::ZERO),
    Err(fibre::error::RecvErrorTimeout::Timeout)
  ));
  tx.send(8).unwrap();
  assert_eq!(rx.recv_timeout(std::time::Duration::ZERO).unwrap(), 8);
}
