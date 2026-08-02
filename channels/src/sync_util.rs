//! Utilities for synchronous blocking and parking.
//! For now, these are minimal helpers around std::thread::park/unpark.
//! The channel implementations will manage the state.
//!
//! Parking goes through `internal::sync` so loom can model it (under loom,
//! `park_timeout` is a panicking stub - timeout paths are not modeled).

use crate::internal::sync::thread::{self, Thread};
use std::future::Future;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use std::time::Duration;

/// Parks the current thread.
#[inline]
pub(crate) fn park_thread() {
  thread::park();
}

struct ThreadUnparker(Thread);

impl Wake for ThreadUnparker {
  fn wake(self: Arc<Self>) {
    self.0.unpark();
  }

  fn wake_by_ref(self: &Arc<Self>) {
    self.0.unpark();
  }
}

/// Drives a future on the current thread, parking between polls; the waker is
/// this thread's unpark handle. Park/unpark go through the facade; the `Arc`
/// around the unparker is plain std refcounting, not protocol synchronization.
///
/// The parker is built only after a poll has returned `Pending`, so a value
/// that already arrived costs no allocation. That rests on every future driven
/// here re-registering its waker on each poll that returns `Pending`: the
/// no-op waker the first poll sees must not be the one left registered.
pub(crate) fn block_on<F: Future>(fut: F) -> F::Output {
  let mut fut = std::pin::pin!(fut);
  if let Poll::Ready(out) = fut.as_mut().poll(&mut Context::from_waker(Waker::noop())) {
    return out;
  }

  let waker = Waker::from(Arc::new(ThreadUnparker(thread::current())));
  let mut cx = Context::from_waker(&waker);
  loop {
    match fut.as_mut().poll(&mut cx) {
      Poll::Ready(out) => return out,
      Poll::Pending => park_thread(),
    }
  }
}

/// Parks the current thread for a given duration.
#[inline]
pub(crate) fn park_thread_timeout(duration: Duration) {
  thread::park_timeout(duration);
}
