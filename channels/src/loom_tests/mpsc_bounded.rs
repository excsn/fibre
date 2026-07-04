//! Loom models of the bounded MPSC (`mpsc::bounded_queue`) sync protocol:
//! Vyukov `head.swap` publish chain, node-pool handoff, and the sync
//! park/notify handshake (`register_sync_send` / `register_sync_recv`).

use crate::mpsc::bounded;
use loom::thread;

/// Single producer, two items through a cap-2 queue: publish/consume ordering
/// and the empty-queue consumer park/wake path.
#[test]
fn spsc_two_items() {
  loom::model(|| {
    let (tx, rx) = bounded::<u32>(2);
    let t = thread::spawn(move || {
      tx.send(1).unwrap();
      tx.send(2).unwrap();
    });
    assert_eq!(rx.recv().unwrap(), 1);
    assert_eq!(rx.recv().unwrap(), 2);
    t.join().unwrap();
  });
}

/// Two producers race the swap-publish and the shared node pool; FIFO per
/// producer is guaranteed, global order is not — assert the multiset.
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

/// Cap-1 backpressure: the second send must park until the consumer frees a
/// node — exercises the producer-side register/notify handshake both ways.
#[test]
fn capacity_one_backpressure() {
  loom::model(|| {
    let (tx, rx) = bounded::<u32>(1);
    let t = thread::spawn(move || {
      tx.send(1).unwrap();
      tx.send(2).unwrap(); // blocks until the consumer pops item 1
    });
    assert_eq!(rx.recv().unwrap(), 1);
    assert_eq!(rx.recv().unwrap(), 2);
    t.join().unwrap();
  });
}

/// Producer sends then disconnects: the consumer must drain the item and then
/// observe Disconnected — the classic lost-close-wake surface.
#[test]
fn disconnect_after_send_drains_then_errors() {
  loom::model(|| {
    let (tx, rx) = bounded::<u32>(1);
    let t = thread::spawn(move || {
      tx.send(7).unwrap();
      // tx dropped here
    });
    assert_eq!(rx.recv().unwrap(), 7);
    assert!(rx.recv().is_err());
    t.join().unwrap();
  });
}
