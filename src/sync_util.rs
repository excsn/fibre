//! Utilities for synchronous blocking and parking.
//! For now, these are minimal helpers around std::thread::park/unpark.
//! The channel implementations will manage the state.

use std::thread;
use std::time::Duration;

/// Parks the current thread.
#[inline]
pub(crate) fn park_thread() {
  thread::park();
}

/// Parks the current thread for a given duration.
#[inline]
pub(crate) fn park_thread_timeout(duration: Duration) {
  thread::park_timeout(duration);
}

/// Unparks the given thread.
#[inline]
pub(crate) fn unpark_thread(thread: &thread::Thread) {
  thread.unpark();
}

pub fn park_thread_timeout_cond<F>(timeout: Option<Duration>, stop_condition: F)
where
  F: Fn() -> bool,
{
  if stop_condition() {
    return;
  }
  match timeout {
    Some(t) => thread::park_timeout(t),
    None => thread::park(),
  }
}
