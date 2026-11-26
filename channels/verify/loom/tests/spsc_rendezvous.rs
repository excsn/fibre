//! Loom models of the SPSC rendezvous (`spsc::rendezvous`) handshake: the
//! zero-capacity direct handoff through the shared `AtomicU8` state machine and
//! the sender/receiver park/unpark in `internal::rendezvous` (the same core
//! MPMC rendezvous uses, in its MPSC single-slot configuration).

#![cfg(loom)]

use fibre::spsc::rendezvous::rendezvous;
use loom::thread;

use std::time::Duration;

/// One value handed off sender -> receiver. Either side may arrive first; loom
/// explores both interleavings, exercising the park-then-wake on whichever side
/// blocks waiting for its counterpart.
#[test]
fn handoff() {
  loom::model(|| {
    let (tx, mut rx) = rendezvous::<u32>();
    let t = thread::spawn(move || {
      tx.send(5).unwrap();
    });
    assert_eq!(rx.recv().unwrap(), 5);
    t.join().unwrap();
  });
}

/// Sender drops without ever sending: a receiver that has parked (or is about
/// to) must observe Disconnected rather than hang - the lost close-wake surface.
#[test]
fn sender_drop_disconnects_receiver() {
  loom::model(|| {
    let (tx, mut rx) = rendezvous::<u32>();
    let t = thread::spawn(move || {
      drop(tx);
    });
    assert!(rx.recv().is_err());
    t.join().unwrap();
  });
}

/// A receive that times out took nothing, so the racing `try_send` must not
/// report a delivery. A zero timeout reaches the cancel path without parking,
/// which keeps the model free of timed waits.
#[test]
fn timed_out_recv_and_try_send_agree_on_delivery() {
  loom::model(|| {
    let (tx, mut rx) = rendezvous::<u32>();
    let t = thread::spawn(move || tx.try_send(5).is_ok());
    let received = rx.recv_timeout(Duration::ZERO);
    let sent = t.join().unwrap();
    assert_eq!(
      sent,
      received.is_ok(),
      "try_send returned {sent} and recv_timeout returned {received:?}"
    );
  });
}
