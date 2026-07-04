//! Loom models of the unbounded MPMC (`mpmc::unbounded`): multi-producer publish
//! into the shared chain and the consumer park/notify handshake. Each handle
//! keeps its own slab/cursor so `send`/`recv` take `&mut self`; clone a handle
//! per thread.

use crate::mpmc::unbounded;
use loom::thread;

/// Two producers each publish one item; the consumer drains both. Global order
/// is not fixed — assert the multiset.
#[test]
fn two_producers_one_item_each() {
  loom::model(|| {
    let (tx1, mut rx) = unbounded::<u32>();
    let mut tx2 = tx1.clone();
    let mut tx1 = tx1;
    let a = thread::spawn(move || tx1.send(1).unwrap());
    let b = thread::spawn(move || tx2.send(2).unwrap());
    let first = rx.recv().unwrap();
    let second = rx.recv().unwrap();
    assert_eq!(first + second, 3);
    a.join().unwrap();
    b.join().unwrap();
  });
}

/// Producer publishes then drops: the consumer drains the item, then observes
/// Disconnected — the lost close-wake surface.
#[test]
fn disconnect_after_send_drains_then_errors() {
  loom::model(|| {
    let (tx, mut rx) = unbounded::<u32>();
    let t = thread::spawn(move || {
      let mut tx = tx;
      tx.send(9).unwrap();
    });
    assert_eq!(rx.recv().unwrap(), 9);
    assert!(rx.recv().is_err());
    t.join().unwrap();
  });
}
