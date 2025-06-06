//! Utilities for asynchronous operations, wakers, and future skeletons.

// Re-export AtomicWaker from futures-util for internal crate use.
// This is the recommended way to get a robust AtomicWaker.
pub(crate) use futures_util::task::AtomicWaker;