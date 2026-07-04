//! Loom models of the MPMC rendezvous (`mpmc::rendezvous`): zero-capacity direct
//! handoff through `internal::rendezvous` (the FIFO multi-waiter configuration)
//! and the sender/receiver park/unpark.

use crate::mpmc::rendezvous::rendezvous;
use loom::thread;

/// One value handed off sender -> receiver; either side may park waiting for
/// its counterpart. loom explores both arrival orders.
#[test]
fn handoff() {
  loom::model(|| {
    let (tx, rx) = rendezvous::<u32>();
    let t = thread::spawn(move || {
      tx.send(5).unwrap();
    });
    assert_eq!(rx.recv().unwrap(), 5);
    t.join().unwrap();
  });
}

/// Sender drops without sending: a parked/racing receiver must observe
/// Disconnected rather than hang.
#[test]
fn sender_drop_disconnects_receiver() {
  loom::model(|| {
    let (tx, rx) = rendezvous::<u32>();
    let t = thread::spawn(move || {
      drop(tx);
    });
    assert!(rx.recv().is_err());
    t.join().unwrap();
  });
}
