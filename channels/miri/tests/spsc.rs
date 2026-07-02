//! Miri suite for the SPSC channel: owned-index ring (wraparound, non-pow2
//! capacity, cached counterpart indices), waiter protocol, conversions.

use fibre::error::{RecvError, TryRecvError, TrySendError};
use fibre::spsc;
use fibre_miri::{block_on, drop_counter, drops, poll_once, DropCounter, ITEMS_CROSSING};

use std::pin::pin;
use std::thread;

#[test]
fn sync_roundtrip_wraps_non_pow2_capacity() {
  // cap 3 -> phys 4: exercises the logical-vs-physical capacity split.
  let (tx, rx) = spsc::bounded_sync::<usize>(3);
  for cycle in 0..20 {
    for i in 0..3 {
      tx.send(cycle * 3 + i).unwrap();
    }
    assert!(tx.is_full());
    for i in 0..3 {
      assert_eq!(rx.recv().unwrap(), cycle * 3 + i);
    }
    assert!(rx.is_empty());
  }
}

#[test]
fn threaded_ring_race() {
  let (tx, rx) = spsc::bounded_sync::<usize>(4);
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
fn values_dropped_exactly_once() {
  let counter = drop_counter();
  {
    let (tx, rx) = spsc::bounded_sync(4);
    for _ in 0..4 {
      tx.send(DropCounter::new(&counter)).unwrap();
    }
    drop(rx.recv().unwrap());
    assert_eq!(drops(&counter), 1);
    drop(tx);
    drop(rx); // three still buffered
  }
  assert_eq!(drops(&counter), 4);
}

#[test]
fn batch_paths_cross_wrap() {
  let (tx, rx) = spsc::bounded_sync::<usize>(4);
  let producer = thread::spawn(move || {
    let mut next = 0;
    while next < ITEMS_CROSSING {
      let batch: Vec<usize> = (next..(next + 7).min(ITEMS_CROSSING)).collect();
      next += batch.len();
      tx.send_batch(batch).unwrap();
    }
  });
  let mut expected = 0;
  while expected < ITEMS_CROSSING {
    for v in rx.recv_batch(5).unwrap() {
      assert_eq!(v, expected);
      expected += 1;
    }
  }
  producer.join().unwrap();
}

#[test]
fn async_send_future_registers_then_cancels() {
  let (mut tx, mut rx) = spsc::bounded_async::<u32>(1);
  block_on(tx.send(1)).unwrap();
  {
    let mut fut = pin!(tx.send(2));
    assert!(poll_once(fut.as_mut()).is_pending()); // registered as send waiter
  } // dropped while registered: Drop must unregister; item 2 is cancelled
  assert_eq!(block_on(rx.recv()).unwrap(), 1);
  block_on(tx.send(3)).unwrap();
  assert_eq!(block_on(rx.recv()).unwrap(), 3);
}

#[test]
fn async_recv_future_registers_then_cancels() {
  let (mut tx, mut rx) = spsc::bounded_async::<u32>(2);
  {
    let mut fut = pin!(rx.recv());
    assert!(poll_once(fut.as_mut()).is_pending());
  }
  assert_eq!(rx.try_recv(), Err(TryRecvError::Empty));
  block_on(tx.send(5)).unwrap();
  assert_eq!(block_on(rx.recv()).unwrap(), 5);
}

#[test]
fn try_send_full_and_closed() {
  let (mut tx, rx) = spsc::bounded_async::<u32>(1);
  tx.try_send(1).unwrap();
  assert!(matches!(tx.try_send(2), Err(TrySendError::Full(2))));
  drop(rx);
  assert!(matches!(tx.try_send(3), Err(TrySendError::Closed(3))));
}

#[test]
fn sync_producer_async_consumer_cross_thread() {
  let (tx, rx) = spsc::bounded_sync::<usize>(2);
  let mut arx = rx.to_async();
  let producer = thread::spawn(move || {
    for i in 0..ITEMS_CROSSING {
      tx.send(i).unwrap();
    }
  });
  for i in 0..ITEMS_CROSSING {
    assert_eq!(block_on(arx.recv()).unwrap(), i);
  }
  producer.join().unwrap();
  assert_eq!(block_on(arx.recv()), Err(RecvError::Disconnected));
}

#[test]
fn async_producer_sync_consumer_cross_thread() {
  let (tx, rx) = spsc::bounded_async::<usize>(2);
  let mut atx = tx;
  let producer = thread::spawn(move || {
    for i in 0..64 {
      block_on(atx.send(i)).unwrap();
    }
  });
  let srx = rx.to_sync();
  for i in 0..64 {
    assert_eq!(srx.recv().unwrap(), i);
  }
  producer.join().unwrap();
}

#[test]
fn cancelled_send_future_drops_value_exactly_once() {
  let counter = drop_counter();
  let (mut tx, mut rx) = spsc::bounded_async(1);
  block_on(tx.send(DropCounter::new(&counter))).unwrap();
  {
    let mut fut = pin!(tx.send(DropCounter::new(&counter)));
    assert!(poll_once(fut.as_mut()).is_pending());
  } // cancelled while holding the value: must drop it exactly once
  assert_eq!(drops(&counter), 1);
  drop(block_on(rx.recv()).unwrap());
  assert_eq!(drops(&counter), 2);
}

#[test]
fn batch_mut_variants_roundtrip() {
  let (tx, rx) = spsc::bounded_sync::<usize>(4);
  let producer = thread::spawn(move || {
    let mut pending: Vec<usize> = (0..ITEMS_CROSSING).collect();
    while !pending.is_empty() {
      tx.send_batch_mut(&mut pending).unwrap();
    }
  });
  let mut out = Vec::new();
  while out.len() < ITEMS_CROSSING {
    match rx.try_recv_batch_mut(&mut out, 6) {
      Ok(_) | Err(fibre::error::TryRecvError::Empty) => {}
      Err(e) => panic!("unexpected: {e:?}"),
    }
  }
  producer.join().unwrap();
  assert_eq!(out, (0..ITEMS_CROSSING).collect::<Vec<_>>());
}

#[test]
fn send_batch_to_closed_returns_all_unsent() {
  let counter = drop_counter();
  let (tx, rx) = spsc::bounded_sync(4);
  drop(rx);
  let items: Vec<DropCounter> = (0..6).map(|_| DropCounter::new(&counter)).collect();
  let err = tx.send_batch(items).unwrap_err();
  assert_eq!(err.sent, 0);
  drop(err); // unsent values come back and drop exactly once
  assert_eq!(drops(&counter), 6);
}

#[test]
fn stream_yields_then_ends_on_disconnect() {
  use futures::StreamExt;
  let (mut tx, mut rx) = spsc::bounded_async::<u32>(2);
  block_on(tx.send(1)).unwrap();
  block_on(tx.send(2)).unwrap();
  assert_eq!(block_on(rx.next()), Some(1));
  assert_eq!(block_on(rx.next()), Some(2));
  drop(tx);
  assert_eq!(block_on(rx.next()), None);
}

#[test]
fn recv_timeout_zero_and_success() {
  let (tx, mut rx) = spsc::bounded_sync::<u32>(2);
  // Zero timeout on an empty channel must take the immediate-timeout path
  // (register/unregister without parking).
  assert!(matches!(
    rx.recv_timeout(std::time::Duration::ZERO),
    Err(fibre::error::RecvErrorTimeout::Timeout)
  ));
  tx.send(9).unwrap();
  assert_eq!(rx.recv_timeout(std::time::Duration::ZERO).unwrap(), 9);
}

#[test]
fn conversion_carries_buffered_items() {
  let (tx, rx) = spsc::bounded_sync::<u32>(4);
  tx.send(1).unwrap();
  tx.send(2).unwrap();
  let mut arx = rx.to_async();
  assert_eq!(block_on(arx.recv()).unwrap(), 1);
  let mut atx = tx.to_async();
  block_on(atx.send(3)).unwrap();
  assert_eq!(block_on(arx.recv()).unwrap(), 2);
  assert_eq!(block_on(arx.recv()).unwrap(), 3);
}
