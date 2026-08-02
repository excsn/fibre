//! Loom models of the pooled oneshot: the recycled-slot exit protocol (whichever
//! handle exits last pushes the slot back), the tagged freelist under concurrent
//! pop/push, and the pool-drop drain (storage release deferred to the last
//! outstanding retire).
//!
//! Unlike `loom_tests::oneshot`, the wake path IS modeled here: the pool routes
//! its waker through the `internal::sync` facade, whose loom build shims
//! `AtomicWaker` over a loom Mutex, so `loom::future::block_on` models drive the
//! real `recv().await` including the WAITING announcement and the gated wake. A
//! lost wakeup (the bug class the gate exists for) surfaces as a loom deadlock
//! report. The shim is ordering-stronger than `futures_util`'s internals, so
//! these models verify the pool's protocol, not AtomicWaker's.

use crate::error::{TryRecvError, TrySendError};
use crate::oneshot::pair_pool;
use loom::thread;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

struct DropCount(Arc<AtomicUsize>);
impl Drop for DropCount {
  fn drop(&mut self) {
    self.0.fetch_add(1, Ordering::SeqCst);
  }
}

/// A sent value is received exactly once across the send/try_recv race, and the
/// recycled slot immediately serves a fresh pair.
#[test]
fn exchange_then_recycle() {
  loom::model(|| {
    let pool = pair_pool::<u32>(1);
    let (tx, mut rx) = pool.pair().unwrap();
    let t = thread::spawn(move || {
      tx.send(1).unwrap();
    });
    loop {
      match rx.try_recv() {
        Ok(v) => {
          assert_eq!(v, 1);
          break;
        }
        Err(TryRecvError::Empty) => thread::yield_now(),
        Err(TryRecvError::Disconnected) => panic!("false disconnect after successful send"),
      }
    }
    t.join().unwrap();
    drop(rx);
    let (tx2, mut rx2) = pool.pair().expect("slot must have been recycled");
    tx2.send(2).unwrap();
    assert_eq!(rx2.try_recv().unwrap(), 2);
  });
}

/// Whichever side loses the send-vs-receiver-drop race, the value drops exactly
/// once and the slot comes back.
#[test]
fn send_races_receiver_drop() {
  loom::model(|| {
    let drops = Arc::new(AtomicUsize::new(0));
    let pool = pair_pool::<DropCount>(1);
    let (tx, rx) = pool.pair().unwrap();
    let d = Arc::clone(&drops);
    let t = thread::spawn(move || match tx.send(DropCount(d)) {
      Ok(()) => {}
      Err(TrySendError::Closed(v)) => drop(v),
      Err(_) => unreachable!(),
    });
    drop(rx);
    t.join().unwrap();
    assert_eq!(drops.load(Ordering::SeqCst), 1);
    assert!(pool.pair().is_some());
  });
}

/// Both handles dropping concurrently retire the slot exactly once.
#[test]
fn sender_drop_races_receiver_drop() {
  loom::model(|| {
    let pool = pair_pool::<u32>(1);
    let (tx, rx) = pool.pair().unwrap();
    let t = thread::spawn(move || {
      drop(tx);
    });
    drop(rx);
    t.join().unwrap();
    assert!(pool.pair().is_some());
  });
}

/// The pool handle dropping while a channel is in flight defers the storage
/// release to the channel's retire; the channel stays fully usable.
#[test]
fn pool_drop_races_inflight_channel() {
  loom::model(|| {
    let pool = pair_pool::<u32>(1);
    let (tx, mut rx) = pool.pair().unwrap();
    let t = thread::spawn(move || {
      tx.send(3).unwrap();
    });
    drop(pool);
    loop {
      match rx.try_recv() {
        Ok(v) => {
          assert_eq!(v, 3);
          break;
        }
        Err(TryRecvError::Empty) => thread::yield_now(),
        Err(TryRecvError::Disconnected) => panic!("false disconnect after successful send"),
      }
    }
    t.join().unwrap();
  });
}

/// The lost-wake surface: a parked `recv().await` racing the send must wake in
/// every interleaving of the WAITING announcement vs the VALUE publish.
#[test]
fn parked_recv_never_misses_the_wake() {
  loom::model(|| {
    let pool = pair_pool::<u32>(1);
    let (tx, mut rx) = pool.pair().unwrap();
    let t = thread::spawn(move || {
      tx.send(1).unwrap();
    });
    let got = loom::future::block_on(async { rx.recv().await.unwrap() });
    assert_eq!(got, 1);
    t.join().unwrap();
    drop(rx);
    assert!(pool.pair().is_some());
  });
}

/// Same surface for the close path: a sender dropping without sending must wake
/// a parked receiver into Disconnected, never strand it.
#[test]
fn parked_recv_never_misses_the_close() {
  loom::model(|| {
    let pool = pair_pool::<u32>(1);
    let (tx, mut rx) = pool.pair().unwrap();
    let t = thread::spawn(move || {
      drop(tx);
    });
    let got = loom::future::block_on(async { rx.recv().await });
    assert!(got.is_err());
    t.join().unwrap();
    drop(rx);
    assert!(pool.pair().is_some());
  });
}

/// Two channels retiring while the pool drops: the drain word must free the
/// storage exactly once, on whichever settle observes it emptying.
#[test]
fn pool_drop_races_two_retires() {
  loom::model(|| {
    let pool = pair_pool::<u32>(2);
    let (tx_a, rx_a) = pool.pair().unwrap();
    let (tx_b, rx_b) = pool.pair().unwrap();
    let t = thread::spawn(move || {
      drop(tx_a);
      drop(rx_a);
    });
    drop(pool);
    drop(tx_b);
    drop(rx_b);
    t.join().unwrap();
  });
}

/// A batch chain-pop racing a retire's push: the tagged head must either serve
/// a consistent chain or retry, never a corrupted one.
#[test]
fn batch_pop_races_retire() {
  loom::model(|| {
    let pool = Arc::new(pair_pool::<u32>(2));
    let (tx, rx) = pool.pair().unwrap();
    let t = thread::spawn(move || {
      drop(tx);
      drop(rx);
    });
    let pairs = loop {
      if let Some(v) = pool.pair_batch(2) {
        break v;
      }
      thread::yield_now();
    };
    assert_eq!(pairs.len(), 2);
    for (tx, mut rx) in pairs {
      tx.send(5).unwrap();
      assert_eq!(rx.try_recv().unwrap(), 5);
    }
    t.join().unwrap();
  });
}

/// A slot retired on one thread and re-paired on another: the reset must be
/// fully visible through the push/pop edge, and the second exchange must never
/// observe the first's value.
#[test]
fn recycled_slot_crosses_threads_clean() {
  loom::model(|| {
    let pool = Arc::new(pair_pool::<u32>(1));
    let (tx, mut rx) = pool.pair().unwrap();
    let t = thread::spawn(move || {
      tx.send(1).unwrap();
      loop {
        match rx.try_recv() {
          Ok(v) => {
            assert_eq!(v, 1);
            break;
          }
          Err(TryRecvError::Empty) => thread::yield_now(),
          Err(TryRecvError::Disconnected) => panic!("false disconnect"),
        }
      }
    });
    let (tx2, mut rx2) = loop {
      if let Some(pair) = pool.pair() {
        break pair;
      }
      thread::yield_now();
    };
    tx2.send(2).unwrap();
    assert_eq!(rx2.try_recv().unwrap(), 2);
    t.join().unwrap();
  });
}

/// Two threads pairing concurrently from a two-slot pool get distinct slots and
/// both exchanges complete.
#[test]
fn concurrent_pairs_from_freelist() {
  loom::model(|| {
    let pool = Arc::new(pair_pool::<u32>(2));
    let p = Arc::clone(&pool);
    let t = thread::spawn(move || {
      let (tx, mut rx) = p.pair().expect("two slots for two threads");
      tx.send(10).unwrap();
      assert_eq!(rx.try_recv().unwrap(), 10);
    });
    let (tx, mut rx) = pool.pair().expect("two slots for two threads");
    tx.send(20).unwrap();
    assert_eq!(rx.try_recv().unwrap(), 20);
    t.join().unwrap();
  });
}
