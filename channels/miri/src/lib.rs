//! Shared helpers for the fibre miri suites. See GUIDE.md for the rules of
//! this crate (small counts, no tokio, no sleeps, manual polling).

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};

pub use futures::executor::block_on;

/// Item counts sized for the interpreter. `ITEMS_CROSSING` is chosen to cross
/// every internal chunk boundary in the crate at least twice (spsc test caps
/// are single digits; mpsc unbounded slabs are 128 nodes, bounded recycle
/// chunks 64).
pub const ITEMS_SMALL: usize = 40;
pub const ITEMS_CROSSING: usize = 300;

/// Polls a future exactly once with a no-op waker. Drives waiter
/// register/unregister paths deterministically without any runtime; pair with
/// dropping the future mid-`Pending` to exercise cancel-safety `Drop` impls.
pub fn poll_once<F: Future + ?Sized>(fut: Pin<&mut F>) -> Poll<F::Output> {
  let waker = futures::task::noop_waker();
  let mut cx = Context::from_waker(&waker);
  fut.poll(&mut cx)
}

/// Counts drops of held values. Miri turns any double-drop into a hard error
/// on the counter's refcount, and the final count assertion catches leaks.
#[derive(Debug)]
pub struct DropCounter(Arc<AtomicUsize>);

impl DropCounter {
  pub fn new(counter: &Arc<AtomicUsize>) -> Self {
    DropCounter(Arc::clone(counter))
  }
}

impl Drop for DropCounter {
  fn drop(&mut self) {
    self.0.fetch_add(1, Ordering::Relaxed);
  }
}

pub fn drop_counter() -> Arc<AtomicUsize> {
  Arc::new(AtomicUsize::new(0))
}

pub fn drops(counter: &Arc<AtomicUsize>) -> usize {
  counter.load(Ordering::Relaxed)
}
