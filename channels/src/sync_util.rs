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
