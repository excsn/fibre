//! Loom models of the unbounded MPSC (`mpsc::unbounded`, the v3 Vyukov-chain +
//! per-handle bump-slab design): the multi-producer publish into the shared
//! chain and the single consumer's park/notify handshake. Senders take
//! `&mut self` (each keeps its own slab), so multi-producer models clone a
//! sender per thread.

use crate::mpsc::unbounded;
use loom::thread;

/// Two producers each publish one item into the chain; the single consumer
/// drains both. Per-producer order is FIFO, global order is not - assert the
/// multiset.
#[test]
fn two_producers_one_item_each() {
  loom::model(|| {
    let (tx1, rx) = unbounded::<u32>();
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

/// The consumer races ahead of a single producer and parks on the empty chain;
/// the producer's publish must wake it (lost-wakeup surface).
#[test]
fn producer_sends_consumer_parks() {
  loom::model(|| {
    let (tx, rx) = unbounded::<u32>();
    let t = thread::spawn(move || {
      let mut tx = tx;
      tx.send(7).unwrap();
    });
    assert_eq!(rx.recv().unwrap(), 7);
    t.join().unwrap();
  });
}

/// Producer publishes then drops: the consumer must drain the item and then
/// observe Disconnected - the classic lost close-wake surface.
#[test]
fn disconnect_after_send_drains_then_errors() {
  loom::model(|| {
    let (tx, rx) = unbounded::<u32>();
    let t = thread::spawn(move || {
      let mut tx = tx;
      tx.send(9).unwrap();
      // tx dropped here
    });
    assert_eq!(rx.recv().unwrap(), 9);
    assert!(rx.recv().is_err());
    t.join().unwrap();
  });
}
