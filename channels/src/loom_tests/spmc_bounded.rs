//! Loom models of the bounded SPMC broadcast (`spmc::bounded`): the single
//! producer's ring publish, the `left_right` consumer registry, and a
//! consumer's PARK_CONSUMING park/wake handshake.
//!
//! No disconnect model: on sender drop the broadcast wake unparks a consumer
//! that may then observe disconnection and return without parking, leaving an
//! unconsumed notify - benign on std, but loom asserts on it, and the race is
//! inherent to broadcast-wake so it can't be interleaved away.

use crate::spmc::bounded;
use loom::thread;

/// Producer publishes one item; a single consumer (which may race ahead and
/// park on the empty ring) receives it - exercises the PARK_CONSUMING wake.
#[test]
fn one_item() {
  loom::model(|| {
    let (tx, rx) = bounded::<u32>(2);
    let t = thread::spawn(move || {
      tx.send(7).unwrap();
    });
    assert_eq!(rx.recv().unwrap(), 7);
    t.join().unwrap();
  });
}
