//! Loom model of `sync::v2::HybridRwLock`'s inner lock: fetch-add readers
//! against pending writers and the wait-list handoff. The data `UnsafeCell`
//! stays std, so this models liveness and ordering (no deadlock, no lost
//! wakeup), not mutual exclusion itself. The slot path, whose Dekker pair
//! needs SeqCst load and store semantics loom does not implement, is in
//! `hybrid_rwlock_v2_advisory.rs`.

#![cfg(loom)]

use fibre::sync::v2::HybridRwLock;
use loom::sync::Arc;
use loom::thread;

/// Two writers contend: one takes the inner lock, the other counts itself
/// pending, spins once, queues and parks; the unlock must wake it.
#[test]
fn two_writers_increment() {
  loom::model(|| {
    let l = Arc::new(HybridRwLock::new(0usize));
    let l2 = Arc::clone(&l);
    let t = thread::spawn(move || {
      *l2.write() += 1;
    });
    *l.write() += 1;
    t.join().unwrap();
    assert_eq!(*l.read(), 2);
  });
}

/// After a write the slots are off, so the reader takes the inner count and
/// may back out against the pending writer; the back-out must not lose the
/// writer's wake.
#[test]
fn inner_reader_against_pending_writer() {
  loom::model(|| {
    let l = Arc::new(HybridRwLock::new(0usize));
    drop(l.write());
    let l2 = Arc::clone(&l);
    let t = thread::spawn(move || {
      *l2.write() += 1;
    });
    let seen = *l.read();
    assert!(seen <= 1);
    t.join().unwrap();
    assert_eq!(*l.read(), 1);
  });
}
