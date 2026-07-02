//! Miri suite for the SPMC broadcast channel: per-consumer cursors over the
//! shared ring, backpressure from the slowest consumer.

use fibre::error::RecvError;
use fibre::spmc;
use fibre_miri::{block_on, drop_counter, drops, poll_once, DropCounter, ITEMS_SMALL};

use std::pin::pin;
use std::thread;

#[test]
fn broadcast_delivers_every_item_to_every_consumer() {
  let (tx, rx1) = spmc::bounded::<usize>(4);
  let rx2 = rx1.clone();
  let mut handles = Vec::new();
  for rx in [rx1, rx2] {
    handles.push(thread::spawn(move || {
      for i in 0..ITEMS_SMALL {
        assert_eq!(rx.recv().unwrap(), i);
      }
      assert_eq!(rx.recv(), Err(RecvError::Disconnected));
    }));
  }
  for i in 0..ITEMS_SMALL {
    tx.send(i).unwrap();
  }
  drop(tx);
  for h in handles {
    h.join().unwrap();
  }
}

#[test]
fn values_dropped_exactly_once_per_clone() {
  // Each consumer receives a clone; the ring's own copy must also drop exactly
  // once, so total drops = ring originals + delivered clones.
  let counter = drop_counter();
  {
    #[derive(Debug)]
    struct CloneCounted(std::sync::Arc<std::sync::atomic::AtomicUsize>);
    impl Clone for CloneCounted {
      fn clone(&self) -> Self {
        CloneCounted(self.0.clone())
      }
    }
    impl Drop for CloneCounted {
      fn drop(&mut self) {
        self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
      }
    }
    let (tx, rx) = spmc::bounded(4);
    tx.send(CloneCounted(counter.clone())).unwrap();
    tx.send(CloneCounted(counter.clone())).unwrap();
    let got = rx.recv().unwrap(); // one clone out
    drop(got);
    drop(tx);
    drop(rx);
  }
  // 2 ring originals + 1 delivered clone = 3 drops.
  assert_eq!(drops(&counter), 3);
}

#[test]
fn async_recv_registers_then_cancels() {
  let (tx, rx) = spmc::bounded_async::<u32>(2);
  {
    let mut fut = pin!(rx.recv());
    assert!(poll_once(fut.as_mut()).is_pending());
  }
  block_on(tx.send(2)).unwrap();
  assert_eq!(block_on(rx.recv()).unwrap(), 2);
}

#[test]
fn backpressure_from_slowest_consumer_cross_thread() {
  let (tx, rx1) = spmc::bounded::<usize>(2);
  let rx2 = rx1.clone();
  let c1 = thread::spawn(move || {
    for i in 0..ITEMS_SMALL {
      assert_eq!(rx1.recv().unwrap(), i);
    }
  });
  let c2 = thread::spawn(move || {
    for i in 0..ITEMS_SMALL {
      assert_eq!(rx2.recv().unwrap(), i);
    }
  });
  for i in 0..ITEMS_SMALL {
    tx.send(i).unwrap(); // blocks on the slower consumer at cap 2
  }
  c1.join().unwrap();
  c2.join().unwrap();
}

#[test]
fn dropping_slow_consumer_releases_backpressure() {
  // rx2 never receives; its cursor pins the ring until it drops. The sender
  // can only finish if the drop correctly removes the cursor.
  let (tx, rx1) = spmc::bounded::<usize>(2);
  let rx2 = rx1.clone();
  let sender = thread::spawn(move || {
    for i in 0..ITEMS_SMALL {
      tx.send(i).unwrap();
    }
  });
  for i in 0..4 {
    assert_eq!(rx1.recv().unwrap(), i);
  }
  drop(rx2); // sender may be parked on rx2's lagging cursor right now
  for i in 4..ITEMS_SMALL {
    assert_eq!(rx1.recv().unwrap(), i);
  }
  sender.join().unwrap();
}

#[test]
fn batch_roundtrip() {
  let (tx, rx) = spmc::bounded::<usize>(4);
  let consumer = thread::spawn(move || {
    let mut expected = 0;
    while expected < ITEMS_SMALL {
      for v in rx.recv_batch(3).unwrap() {
        assert_eq!(v, expected);
        expected += 1;
      }
    }
  });
  for i in 0..ITEMS_SMALL {
    tx.send(i).unwrap();
  }
  consumer.join().unwrap();
}
