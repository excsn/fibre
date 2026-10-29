//! Advisory loom model of `sync::v2::HybridRwLock`'s slot path: a slot
//! reader's increment and `visible` load against a writer's `visible` store
//! and slot loads, a Dekker pair that relies on SeqCst load and store
//! semantics loom does not implement. See `verify/scripts/loom.sh`.

#![cfg(loom)]

use fibre::sync::v2::HybridRwLock;
use loom::sync::Arc;
use loom::thread;

/// Either the writer sees the slot or the reader sees `visible` cleared; the
/// reader must never observe a torn pair and must not sleep through the unlock.
#[test]
fn slot_reader_and_writer() {
  loom::model(|| {
    let l = Arc::new(HybridRwLock::new((0u64, 0u64)));
    let l2 = Arc::clone(&l);
    let t = thread::spawn(move || {
      let mut g = l2.write();
      g.0 += 1;
      g.1 += 1;
    });
    {
      let g = l.read();
      assert_eq!(g.0, g.1);
    }
    t.join().unwrap();
    assert_eq!(*l.read(), (1, 1));
  });
}
