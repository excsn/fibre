//! Miri suite for the unbounded MPSC channel (v3): Vyukov chain with
//! per-handle bump slabs, whose slab seal/refcount lifecycle is the newest
//! unsafe core in the crate.

use fibre::error::{RecvError, TryRecvError};
use fibre::mpsc;
use fibre_miri::{block_on, drop_counter, drops, poll_once, DropCounter, ITEMS_CROSSING};

use std::pin::pin;
use std::thread;

// --- v3 (default) -------------------------------------------------------------

#[test]
fn v3_fifo_across_slab_boundaries() {
  let (tx, rx) = mpsc::unbounded();
  for i in 0..ITEMS_CROSSING {
    tx.send(i).unwrap();
  }
  for i in 0..ITEMS_CROSSING {
    assert_eq!(rx.recv().unwrap(), i);
  }
  assert!(rx.is_empty());
}

#[test]
fn v3_multi_producer_race_per_producer_order() {
  let (tx, rx) = mpsc::unbounded();
  let mut handles = Vec::new();
  for p in 0..3usize {
    let txc = tx.clone();
    handles.push(thread::spawn(move || {
      for i in 0..100usize {
        txc.send((p, i)).unwrap();
      }
    }));
  }
  drop(tx);
  let mut next = [0usize; 3];
  for _ in 0..300 {
    let (p, i) = rx.recv().unwrap();
    assert_eq!(next[p], i);
    next[p] += 1;
  }
  assert_eq!(rx.recv(), Err(RecvError::Disconnected));
  for h in handles {
    h.join().unwrap();
  }
}

#[test]
fn v3_drop_with_in_flight_items_spanning_slabs() {
  let counter = drop_counter();
  {
    let (tx, rx) = mpsc::unbounded();
    for _ in 0..ITEMS_CROSSING {
      tx.send(DropCounter::new(&counter)).unwrap();
    }
    for _ in 0..10 {
      drop(rx.recv().unwrap());
    }
    assert_eq!(drops(&counter), 10);
    drop(tx);
    drop(rx);
  }
  assert_eq!(drops(&counter), ITEMS_CROSSING);
}

#[test]
fn v3_handle_drop_seals_partial_slab() {
  let counter = drop_counter();
  let (tx, rx) = mpsc::unbounded();
  for _ in 0..5 {
    tx.send(DropCounter::new(&counter)).unwrap();
  }
  drop(tx); // partial slab sealed here
  for _ in 0..5 {
    drop(rx.recv().unwrap());
  }
  assert!(matches!(rx.recv(), Err(RecvError::Disconnected)));
  assert_eq!(drops(&counter), 5);
}

#[test]
fn v3_clone_storm_and_receiver_close() {
  let counter = drop_counter();
  let (tx, rx) = mpsc::unbounded();
  for _ in 0..8 {
    let txc = tx.clone();
    txc.send(DropCounter::new(&counter)).unwrap();
    // txc drops here, sealing its slab with 1 of SLAB_NODES used
  }
  rx.close().unwrap(); // drains promptly
  assert_eq!(drops(&counter), 8);
  assert!(tx.is_closed());
  drop(tx);
}

#[test]
fn v3_batch_send_batch_recv() {
  let (tx, rx) = mpsc::unbounded();
  assert_eq!(
    tx.send_batch((0..ITEMS_CROSSING).collect()).unwrap(),
    ITEMS_CROSSING
  );
  let mut out = Vec::new();
  let mut got = 0;
  while got < ITEMS_CROSSING {
    got += rx.recv_batch_mut(&mut out, 64).unwrap();
  }
  assert_eq!(out, (0..ITEMS_CROSSING).collect::<Vec<_>>());
}

#[test]
fn v3_async_recv_registers_then_cancels() {
  let (tx, mut rx) = mpsc::unbounded_async::<u32>();
  {
    let mut fut = pin!(rx.recv());
    assert!(poll_once(fut.as_mut()).is_pending());
  } // dropped while registered
  assert_eq!(rx.try_recv(), Err(TryRecvError::Empty));
  block_on(tx.send(3)).unwrap();
  assert_eq!(block_on(rx.recv()).unwrap(), 3);
}

#[test]
fn v3_sync_producer_async_consumer_cross_thread() {
  let (tx, rx) = mpsc::unbounded_async::<usize>();
  let stx = tx.to_sync();
  let producer = thread::spawn(move || {
    for i in 0..ITEMS_CROSSING {
      stx.send(i).unwrap();
    }
  });
  let mut rx = rx;
  for i in 0..ITEMS_CROSSING {
    assert_eq!(block_on(rx.recv()).unwrap(), i);
  }
  producer.join().unwrap();
  assert_eq!(block_on(rx.recv()), Err(RecvError::Disconnected));
}

#[test]
fn v3_sender_to_async_carries_partial_slab() {
  let (tx, rx) = mpsc::unbounded::<u32>();
  tx.send(1).unwrap();
  tx.send(2).unwrap();
  let atx = tx.to_async(); // partial slab moves with the handle
  block_on(atx.send(3)).unwrap();
  for expected in 1..=3 {
    assert_eq!(rx.recv().unwrap(), expected);
  }
  drop(atx);
  assert!(matches!(rx.recv(), Err(RecvError::Disconnected)));
}

#[test]
fn v3_send_batch_mut_and_try_recv_batch() {
  let (tx, rx) = mpsc::unbounded::<usize>();
  let mut pending: Vec<usize> = (0..ITEMS_CROSSING).collect();
  assert_eq!(tx.send_batch_mut(&mut pending).unwrap(), ITEMS_CROSSING);
  assert!(pending.is_empty());
  let mut out = Vec::new();
  while out.len() < ITEMS_CROSSING {
    rx.try_recv_batch_mut(&mut out, 50).unwrap();
  }
  assert_eq!(out, (0..ITEMS_CROSSING).collect::<Vec<_>>());
}

#[test]
fn v3_stream_yields_then_ends_on_disconnect() {
  use futures::StreamExt;
  let (tx, mut rx) = mpsc::unbounded_async::<u32>();
  block_on(tx.send(1)).unwrap();
  block_on(tx.send(2)).unwrap();
  assert_eq!(block_on(rx.next()), Some(1));
  assert_eq!(block_on(rx.next()), Some(2));
  drop(tx);
  assert_eq!(block_on(rx.next()), None);
}

#[test]
fn v3_recv_timeout_zero_and_success() {
  let (tx, rx) = mpsc::unbounded::<u32>();
  assert!(matches!(
    rx.recv_timeout(std::time::Duration::ZERO),
    Err(fibre::error::RecvErrorTimeout::Timeout)
  ));
  tx.send(4).unwrap();
  assert_eq!(rx.recv_timeout(std::time::Duration::ZERO).unwrap(), 4);
}

#[test]
fn v3_async_producers_cross_thread_per_producer_order() {
  let (tx, rx) = mpsc::unbounded_async::<(usize, usize)>();
  let tx2 = tx.clone();
  let p1 = thread::spawn(move || {
    for i in 0..50 {
      block_on(tx.send((0, i))).unwrap();
    }
  });
  let p2 = thread::spawn(move || {
    for i in 0..50 {
      block_on(tx2.send((1, i))).unwrap();
    }
  });
  let mut rx = rx;
  let mut next = [0usize; 2];
  for _ in 0..100 {
    let (p, i) = block_on(rx.recv()).unwrap();
    assert_eq!(next[p], i);
    next[p] += 1;
  }
  p1.join().unwrap();
  p2.join().unwrap();
}

