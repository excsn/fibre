//! Loom model of `sync::HybridMutex`: the FIFO wait-list spinlock, the
//! spin-then-park acquire, and the release / barge / wake-to-recontend handoff.
//!
//! The data `UnsafeCell` stays std (loom's closure-cell can't back a guard's
//! `Deref`), so this models the lock's LIVENESS/ORDERING protocol - no deadlock,
//! no lost wakeup, no ABA on the intrusive wait-list handoff - not mutual
//! exclusion itself (a std-cell `+= 1` isn't a loom sync point, so loom won't
//! interleave it). That's the right target: the risk in a custom
//! spin/queue/park/barge/wake-to-recontend lock is a *lost wakeup*, which loom
//! surfaces as a deadlock.

use crate::sync::HybridMutex;
use loom::sync::Arc;
use loom::thread;

/// Two threads each take the lock and increment. Exercises the contended
/// acquire (spin → queue → park) and the release wake on whichever side loses
/// the race; a lost wakeup would hang and loom reports it as a deadlock.
#[test]
fn two_threads_increment() {
  loom::model(|| {
    let m = Arc::new(HybridMutex::new(0usize));
    let m2 = Arc::clone(&m);
    let t = thread::spawn(move || {
      *m2.lock() += 1;
    });
    *m.lock() += 1;
    t.join().unwrap();
    assert_eq!(*m.lock(), 2);
  });
}
