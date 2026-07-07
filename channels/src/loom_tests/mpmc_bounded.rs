//! Loom models of the bounded MPMC (`mpmc::bounded`): the sync park/notify
//! handshake (`backoff::adaptive_wait` -> park, and the waiter wake path) and
//! close handling. Handles are `Clone` and `send`/`recv` take `&self`.
//!
//! No multi-producer model here: every mpmc op takes the spinlock-backed
//! HybridMutex, and three contending threads explode loom's spin-interleaving
//! branch cap. The lock's own contention is modeled in `hybrid_mutex`; these
//! single-producer models cover the channel's park/wake and close logic.

use crate::mpmc::bounded;
use loom::thread;

/// Cap-1 backpressure: the producer's second send must block until the consumer
/// takes the first - exercises the producer park + consumer wake.
#[test]
fn capacity_one_backpressure() {
  loom::model(|| {
    let (tx, rx) = bounded::<u32>(1);
    let t = thread::spawn(move || {
      tx.send(1).unwrap();
      tx.send(2).unwrap();
    });
    assert_eq!(rx.recv().unwrap(), 1);
    assert_eq!(rx.recv().unwrap(), 2);
    t.join().unwrap();
  });
}

/// Producer sends then disconnects: the consumer drains the item, then observes
/// Disconnected - the lost close-wake surface.
#[test]
fn disconnect_after_send_drains_then_errors() {
  loom::model(|| {
    let (tx, rx) = bounded::<u32>(1);
    let t = thread::spawn(move || {
      tx.send(7).unwrap();
    });
    assert_eq!(rx.recv().unwrap(), 7);
    assert!(rx.recv().is_err());
    t.join().unwrap();
  });
}
