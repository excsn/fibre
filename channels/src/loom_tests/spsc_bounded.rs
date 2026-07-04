//! Loom models of the bounded SPSC (`spsc::bounded_sync`) sync protocol:
//! the owned-index Lamport ring (producer-owned `tail`, consumer-owned `head`
//! with cached counterpart indices) and the sync park/notify handshake
//! (`register` / `notify_senders` / `notify_receivers`, both directions).

use crate::spsc::bounded_sync;
use loom::thread;

/// Producer sends two items through a cap-2 ring; the consumer drains them.
/// Exercises the publish (tail store) / consume (head store) ordering and the
/// empty-ring consumer park/wake path when it races ahead of the producer.
#[test]
fn two_items() {
  loom::model(|| {
    let (tx, rx) = bounded_sync::<u32>(2);
    let t = thread::spawn(move || {
      tx.send(1).unwrap();
      tx.send(2).unwrap();
    });
    assert_eq!(rx.recv().unwrap(), 1);
    assert_eq!(rx.recv().unwrap(), 2);
    t.join().unwrap();
  });
}

/// Cap-1 backpressure: the producer's second send must park until the consumer
/// pops the first item — exercises the producer-side register/park and the
/// consumer-side `notify_senders` wake.
#[test]
fn capacity_one_backpressure() {
  loom::model(|| {
    let (tx, rx) = bounded_sync::<u32>(1);
    let t = thread::spawn(move || {
      tx.send(1).unwrap();
      tx.send(2).unwrap(); // blocks until the consumer pops item 1
    });
    assert_eq!(rx.recv().unwrap(), 1);
    assert_eq!(rx.recv().unwrap(), 2);
    t.join().unwrap();
  });
}

/// The consumer parks on an empty ring before the producer's single send —
/// exercises the consumer-side register/park and the producer's
/// `notify_receivers` wake (the mirror of `capacity_one_backpressure`).
#[test]
fn consumer_parks_then_wakes() {
  loom::model(|| {
    let (tx, rx) = bounded_sync::<u32>(2);
    let t = thread::spawn(move || {
      tx.send(42).unwrap();
    });
    assert_eq!(rx.recv().unwrap(), 42);
    t.join().unwrap();
  });
}

/// Producer sends then disconnects: the consumer must drain the item and then
/// observe Disconnected — the classic lost-close-wake surface.
#[test]
fn disconnect_after_send_drains_then_errors() {
  loom::model(|| {
    let (tx, rx) = bounded_sync::<u32>(1);
    let t = thread::spawn(move || {
      tx.send(7).unwrap();
      // tx dropped here
    });
    assert_eq!(rx.recv().unwrap(), 7);
    assert!(rx.recv().is_err());
    t.join().unwrap();
  });
}
