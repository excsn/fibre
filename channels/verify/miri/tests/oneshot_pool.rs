//! Miri suite for the pooled oneshot: slot recycling across reuse, the
//! register-then-cancel waker path, unconsumed-value drops on every exit
//! ordering, and the pool-drop drain protocol (storage release deferred to the
//! last outstanding retire, the suite's main use-after-free surface).

use fibre::error::{RecvError, TryRecvError, TrySendError};
use fibre::oneshot::{pair_pool, OneshotHostPool, PoolSlot};
use fibre_miri::{block_on, drop_counter, drops, poll_once, DropCounter};

use std::pin::pin;
use std::thread;

#[test]
fn send_then_recv() {
  let pool = pair_pool::<u32>(2);
  let (tx, mut rx) = pool.pair().unwrap();
  tx.send(42).unwrap();
  assert_eq!(block_on(rx.recv()).unwrap(), 42);
}

#[test]
fn recv_registers_then_cancels() {
  let pool = pair_pool::<u32>(1);
  let (tx, mut rx) = pool.pair().unwrap();
  {
    let mut fut = pin!(rx.recv());
    assert!(poll_once(fut.as_mut()).is_pending());
  }
  tx.send(7).unwrap();
  assert_eq!(block_on(rx.recv()).unwrap(), 7);
}

#[test]
fn slot_reuse_after_each_exit_ordering() {
  let pool = pair_pool::<u32>(1);
  for round in 0..4u32 {
    let (tx, mut rx) = pool.pair().expect("recycled slot available");
    match round % 4 {
      0 => {
        tx.send(round).unwrap();
        assert_eq!(block_on(rx.recv()).unwrap(), round);
      }
      1 => {
        drop(rx);
        assert!(matches!(tx.send(round), Err(TrySendError::Closed(_))));
      }
      2 => {
        drop(tx);
        assert!(matches!(block_on(rx.recv()), Err(RecvError::Disconnected)));
      }
      _ => {
        tx.send(round).unwrap();
        drop(rx);
      }
    }
  }
}

#[test]
fn unconsumed_value_drops_once() {
  let counter = drop_counter();
  let pool = pair_pool::<DropCounter>(1);
  let (tx, rx) = pool.pair().unwrap();
  tx.send(DropCounter::new(&counter)).unwrap();
  drop(rx);
  assert_eq!(drops(&counter), 1);
}

#[test]
fn rejected_value_rides_back_and_drops_once() {
  let counter = drop_counter();
  let pool = pair_pool::<DropCounter>(1);
  let (tx, rx) = pool.pair().unwrap();
  drop(rx);
  let rejected = tx.send(DropCounter::new(&counter));
  assert!(rejected.is_err());
  drop(rejected);
  assert_eq!(drops(&counter), 1);
}

#[test]
fn pool_drop_defers_release_to_last_retire() {
  let counter = drop_counter();
  let pool = pair_pool::<DropCounter>(2);
  let (tx_a, mut rx_a) = pool.pair().unwrap();
  let (tx_b, rx_b) = pool.pair().unwrap();
  drop(pool);
  tx_a.send(DropCounter::new(&counter)).unwrap();
  drop(block_on(rx_a.recv()).unwrap());
  drop(tx_b);
  drop(rx_b);
  assert_eq!(drops(&counter), 1);
}

#[test]
fn cross_thread_exchange() {
  let pool = pair_pool::<u64>(8);
  let mut senders = Vec::new();
  let mut receivers = Vec::new();
  for _ in 0..8 {
    let (tx, rx) = pool.pair().unwrap();
    senders.push(tx);
    receivers.push(rx);
  }
  let t = thread::spawn(move || {
    for (i, tx) in senders.into_iter().enumerate() {
      tx.send(i as u64).unwrap();
    }
  });
  for (i, rx) in receivers.iter_mut().enumerate() {
    assert_eq!(block_on(rx.recv()).unwrap(), i as u64);
  }
  t.join().unwrap();
}

#[test]
fn cross_thread_pool_drop_races_retires() {
  let pool = pair_pool::<u64>(4);
  let mut pairs = Vec::new();
  for _ in 0..4 {
    pairs.push(pool.pair().unwrap());
  }
  let t = thread::spawn(move || {
    for (tx, mut rx) in pairs {
      tx.send(1).unwrap();
      assert_eq!(block_on(rx.recv()).unwrap(), 1);
    }
  });
  drop(pool);
  t.join().unwrap();
}

#[test]
fn batch_all_or_nothing() {
  let pool = pair_pool::<u32>(3);
  let batch = pool.pair_batch(2).unwrap();
  assert!(pool.pair_batch(2).is_none());
  assert!(pool.pair().is_some());
  for (tx, mut rx) in batch {
    tx.send(1).unwrap();
    assert_eq!(block_on(rx.recv()).unwrap(), 1);
  }
}

struct Req {
  id: u64,
  reply: PoolSlot<u64>,
}

#[test]
fn host_pool_record_and_recycle() {
  let pool = OneshotHostPool::new(
    2,
    || Req {
      id: 0,
      reply: PoolSlot::new(),
    },
    |r| &r.reply,
  );
  for round in 0..4u64 {
    let (tx, mut rx) = pool.pair_init(|r| r.id = round).unwrap();
    assert_eq!(rx.host().id, round);
    tx.send(round * 3).unwrap();
    assert_eq!(block_on(rx.recv()).unwrap(), round * 3);
  }
}

#[test]
fn host_pool_drop_defers_release() {
  let pool = OneshotHostPool::new(
    1,
    || Req {
      id: 0,
      reply: PoolSlot::new(),
    },
    |r| &r.reply,
  );
  let (tx, mut rx) = pool.pair_init(|r| r.id = 9).unwrap();
  drop(pool);
  assert_eq!(rx.host().id, 9);
  tx.send(9).unwrap();
  assert_eq!(block_on(rx.recv()).unwrap(), 9);
}

#[test]
fn recycled_slot_crosses_threads() {
  let pool = std::sync::Arc::new(pair_pool::<Box<u64>>(1));
  for round in 0..4u64 {
    let (tx, mut rx) = pool.pair().expect("recycled slot available");
    let t = thread::spawn(move || {
      tx.send(Box::new(round * 10)).unwrap();
      assert_eq!(*block_on(rx.recv()).unwrap(), round * 10);
    });
    t.join().unwrap();
  }
}

#[test]
fn register_cancel_then_recycle_reuses_cleanly() {
  let pool = pair_pool::<u64>(1);
  let (tx, mut rx) = pool.pair().unwrap();
  {
    let mut fut = pin!(rx.recv());
    assert!(poll_once(fut.as_mut()).is_pending());
  }
  drop(rx);
  assert!(matches!(tx.send(1), Err(TrySendError::Closed(1))));
  let (tx2, mut rx2) = pool.pair().expect("slot recycled after cancelled registration");
  let t = thread::spawn(move || {
    tx2.send(2).unwrap();
  });
  assert_eq!(block_on(rx2.recv()).unwrap(), 2);
  t.join().unwrap();
}

#[test]
fn churn_over_tiny_pool() {
  let pool = std::sync::Arc::new(pair_pool::<u64>(2));
  let mut handles = Vec::new();
  for worker in 0..3u64 {
    let pool = std::sync::Arc::clone(&pool);
    handles.push(thread::spawn(move || {
      for i in 0..8u64 {
        let pair = loop {
          if let Some(p) = pool.pair() {
            break p;
          }
          std::thread::yield_now();
        };
        let (tx, mut rx) = pair;
        tx.send(worker * 100 + i).unwrap();
        assert_eq!(block_on(rx.recv()).unwrap(), worker * 100 + i);
      }
    }));
  }
  for h in handles {
    h.join().unwrap();
  }
  assert_eq!(pool.pair_batch(2).map(|v| v.len()), Some(2));
}

#[test]
fn batch_pop_races_retires() {
  let pool = std::sync::Arc::new(pair_pool::<u64>(2));
  let (tx, mut rx) = pool.pair().unwrap();
  let t = thread::spawn(move || {
    tx.send(1).unwrap();
    assert_eq!(block_on(rx.recv()).unwrap(), 1);
  });
  let batch = loop {
    if let Some(v) = pool.pair_batch(2) {
      break v;
    }
    std::thread::yield_now();
  };
  t.join().unwrap();
  for (tx, mut rx) in batch {
    tx.send(2).unwrap();
    assert_eq!(block_on(rx.recv()).unwrap(), 2);
  }
}

#[test]
fn host_record_read_across_threads() {
  let pool = std::sync::Arc::new(OneshotHostPool::new(
    1,
    || Req {
      id: 0,
      reply: PoolSlot::new(),
    },
    |r| &r.reply,
  ));
  for round in 0..3u64 {
    let (tx, mut rx) = pool.pair_init(|r| r.id = round).unwrap();
    let t = thread::spawn(move || {
      tx.send(round + 1).unwrap();
    });
    assert_eq!(rx.host().id, round);
    assert_eq!(block_on(rx.recv()).unwrap(), round + 1);
    t.join().unwrap();
  }
}

#[test]
fn recv_blocking_parks_and_wakes() {
  let pool = pair_pool::<u64>(1);
  for round in 0..3u64 {
    let (tx, mut rx) = pool.pair().unwrap();
    let t = thread::spawn(move || {
      tx.send(round).unwrap();
    });
    assert_eq!(rx.recv_blocking().unwrap(), round);
    t.join().unwrap();
  }
}
