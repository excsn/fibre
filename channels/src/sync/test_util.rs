use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Wake, Waker};

/// A waker that counts how many times it has been woken, so tests can assert
/// exactly which unlock paths fired a wake and how often.
pub(super) struct CountingWaker {
  wakes: AtomicUsize,
}

impl CountingWaker {
  pub(super) fn count(&self) -> usize {
    self.wakes.load(Ordering::SeqCst)
  }
}

impl Wake for CountingWaker {
  fn wake(self: Arc<Self>) {
    self.wakes.fetch_add(1, Ordering::SeqCst);
  }

  fn wake_by_ref(self: &Arc<Self>) {
    self.wakes.fetch_add(1, Ordering::SeqCst);
  }
}

pub(super) fn counting_waker() -> (Arc<CountingWaker>, Waker) {
  let cw = Arc::new(CountingWaker {
    wakes: AtomicUsize::new(0),
  });
  let waker = Waker::from(cw.clone());
  (cw, waker)
}
