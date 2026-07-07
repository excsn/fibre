//! Loom models of the unbounded MPMC (`mpmc::unbounded`): the single-producer
//! publish into the shared chain and the consumer park/notify + close handshake.
//! Each handle keeps its own slab/cursor so `send`/`recv` take `&mut self`.
//!
//! No multi-producer model here: the consumer takes the spinlock-backed
//! HybridMutex, so a lock-free 2-producer publish racing a locking consumer
//! blows loom's branch cap. The lock is modeled in `hybrid_mutex`, and the
//! lock-free multi-producer chain publish is the same slab-chain covered by
//! `mpsc_unbounded::two_producers_one_item_each` (whose receiver uses the cheap
//! facade `Mutex`, so it fits).

use crate::mpmc::unbounded;
use loom::thread;

/// Producer publishes then drops: the consumer drains the item, then observes
/// Disconnected - the lost close-wake surface.
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
