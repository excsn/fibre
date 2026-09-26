//! Advisory loom models of the bounded MPSC: `channels/verify/scripts/loom.sh`
//! runs them and reports failures without failing the tier. Each one depends
//! on SeqCst load/store semantics, which loom 0.7 models as AcqRel; the
//! argument and the tier that covers it are in `channels/verify/MODEL.md`.

#![cfg(loom)]

use fibre::mpsc::bounded;
use loom::thread;

fn model_slack<F: Fn() + Sync + Send + 'static>(f: F) {
  let mut builder = loom::model::Builder::new();
  builder.max_branches = 1_000_000;
  builder.check(f);
}

/// The async twin: the parked `SendFuture` is counted awake, stored through
/// its `notified` flag and woken by the sync consumer's single drain, then
/// cold-sends. Also covers `register_async_send` returning `None` when the
/// pop races the re-registration.
#[test]
fn blocked_async_send_resumes_on_single_drain() {
  model_slack(|| {
    let (tx, rx) = bounded::<u32>(2);
    tx.try_send(1).unwrap();
    tx.try_send(2).unwrap();
    let atx = tx.to_async();
    let t = thread::spawn(move || loom::future::block_on(atx.send(3)).unwrap());
    assert_eq!(rx.recv().unwrap(), 1);
    t.join().unwrap();
    assert_eq!(rx.recv().unwrap(), 2);
    assert_eq!(rx.recv().unwrap(), 3);
  });
}
