//! Loom models of both oneshot cores: the clonable packed-word state machine
//! (`oneshot::oneshot`) and the single-sender flag-word design
//! (`oneshot::exclusive`). The prime surface is the historical false
//! Disconnected: a receiver pairing a stale phase with a fresh sender count
//! while a send completed in between.
//!
//! Sync paths only: the async register-then-recheck handshake synchronizes
//! through `futures_util::AtomicWaker`'s internal std atomics, which loom
//! cannot see, so an async model false-deadlocks on a stale state read whose
//! real-world visibility the waker's RMWs guarantee. Async coverage is the
//! tokio tests and the miri suite.

use crate::error::TryRecvError;
use crate::oneshot::{exclusive, oneshot};
use loom::thread;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

struct DropCount(Arc<AtomicUsize>);
impl Drop for DropCount {
  fn drop(&mut self) {
    self.0.fetch_add(1, Ordering::SeqCst);
  }
}

/// A completed send must never be read as Disconnected, even though the
/// sender handle drops (count reaches 0) right after publishing.
#[test]
fn send_ok_never_reads_disconnected() {
  loom::model(|| {
    let (tx, rx) = oneshot::<u32>();
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
  });
}

/// A concurrent clone drop must not make the receiver conclude Disconnected
/// while the other clone's send lands.
#[test]
fn concurrent_sender_drop_does_not_mask_sent_value() {
  loom::model(|| {
    let (tx, rx) = oneshot::<u32>();
    let tx2 = tx.clone();
    let a = thread::spawn(move || {
      tx.send(1).unwrap();
    });
    let b = thread::spawn(move || {
      drop(tx2);
    });
    loop {
      match rx.try_recv() {
        Ok(v) => {
          assert_eq!(v, 1);
          break;
        }
        Err(TryRecvError::Empty) => thread::yield_now(),
        Err(TryRecvError::Disconnected) => panic!("false disconnect with a send in flight"),
      }
    }
    a.join().unwrap();
    b.join().unwrap();
  });
}

/// Receiver drop racing the send: the value must be dropped exactly once,
/// whichever side ends up reclaiming it (receiver cleanup, last-sender-drop
/// reclaim after a mid-WRITING receiver drop, or the send Err return).
#[test]
fn receiver_drop_races_send_no_leak_no_double_drop() {
  loom::model(|| {
    let drops = Arc::new(AtomicUsize::new(0));
    let (tx, rx) = oneshot::<DropCount>();
    let d = Arc::clone(&drops);
    let t = thread::spawn(move || {
      let _ = tx.send(DropCount(d));
    });
    drop(rx);
    t.join().unwrap();
    assert_eq!(drops.load(Ordering::SeqCst), 1);
  });
}

#[test]
fn exclusive_send_races_receiver_drop_no_leak() {
  loom::model(|| {
    let drops = Arc::new(AtomicUsize::new(0));
    let (tx, rx) = exclusive::<DropCount>();
    let d = Arc::clone(&drops);
    let t = thread::spawn(move || {
      let _ = tx.send(DropCount(d));
    });
    drop(rx);
    t.join().unwrap();
    assert_eq!(drops.load(Ordering::SeqCst), 1);
  });
}

#[test]
fn exclusive_sender_drop_disconnects() {
  loom::model(|| {
    let (tx, mut rx) = exclusive::<u32>();
    let t = thread::spawn(move || {
      drop(tx);
    });
    loop {
      match rx.try_recv() {
        Err(TryRecvError::Empty) => thread::yield_now(),
        Err(TryRecvError::Disconnected) => break,
        Ok(_) => panic!("no value was ever sent"),
      }
    }
    t.join().unwrap();
  });
}

