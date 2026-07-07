//! Loom models of the bounded MPSC (`mpsc::bounded_v3`) sync protocol:
//! credit-before-claim `g_tail` ticket writes, the overshoot
//! SKIP-tombstone + retry, and the sync park/notify handshake
//! (`register_sync_send` / `register_sync_recv`). The chunk table is shrunk
//! under loom (see `bounded_v3::shared` MODEL_CHECK) so these tiny models don't
//! allocate atomics they never touch; chunk REUSE itself is miri's job (too many
//! items to reach under loom's exponential exploration).

use crate::mpsc::bounded;
use loom::thread;

/// The cap-2 (window-has-slack) models keep two items in flight at once, so the
/// credit-window + chunk-table protocol explores far more interleavings than
/// loom's default 1k-branch budget allows - raise it. Preemptions stay bounded
/// by the runner (`LOOM_MAX_PREEMPTIONS`), so this only lifts the ceiling, it
/// does not weaken exploration. (Cap-1 models fit the default budget and use
/// plain `loom::model`.)
fn model_slack<F: Fn() + Sync + Send + 'static>(f: F) {
  let mut builder = loom::model::Builder::new();
  builder.max_branches = 1_000_000;
  builder.check(f);
}

/// Single producer, two items through a cap-2 queue: publish/consume ordering
/// and the empty-queue consumer park/wake path.
#[test]
fn spsc_two_items() {
  model_slack(|| {
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

/// Two producers race the `g_tail` fetch_add claim (cap 2, both fit); global
/// order is not guaranteed - assert the multiset.
#[test]
fn two_producers_one_item_each() {
  model_slack(|| {
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

/// Two producers race a SINGLE credit (cap 1): both may pass `window_open`, both
/// `fetch_add`, so one claims a ticket that overshoots the window and must write
/// a SKIP tombstone then retry after the consumer drains. Exercises the
/// credit-before-claim overshoot path + the consumer stepping over the SKIP.
#[test]
fn overshoot_skip_and_retry() {
  loom::model(|| {
    let (tx, rx) = bounded::<u32>(1);
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
/// credit - exercises the producer-side register/notify handshake both ways.
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
/// observe Disconnected - the classic lost-close-wake surface.
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
