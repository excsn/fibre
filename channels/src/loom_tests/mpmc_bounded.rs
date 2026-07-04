//! Loom models of the bounded MPMC (`mpmc::bounded`): multi-producer publish
//! and multi-consumer take through the cell/slab core, plus the sync
//! park/notify handshake (`backoff::adaptive_wait` -> park, and the waiter wake
//! path). Handles are `Clone` and `send`/`recv` take `&self`.

use crate::mpmc::bounded;
use loom::thread;

/// Two producers race the publish into a cap-2 channel; the consumer drains
/// both. Global order is not fixed — assert the multiset.
#[test]
fn two_producers_one_item_each() {
  loom::model(|| {
    let (tx, rx) = bounded::<u32>(2);
    let tx2 = tx.clone();
    let a = thread::spawn(move || tx.send(1).unwrap());
    let b = thread::spawn(move || tx2.send(2).unwrap());
    let first = rx.recv().unwrap();
    let second = rx.recv().unwrap();
    assert_eq!(first + second, 3);
    a.join().unwrap();
    b.join().unwrap();
  });
}

/// Cap-1 backpressure: the producer's second send must block until the consumer
/// takes the first — exercises the producer park + consumer wake.
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
/// Disconnected — the lost close-wake surface.
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
