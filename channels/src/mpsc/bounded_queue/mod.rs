//! A screamingly high-performance Bounded MPSC Queue inspired by higher
//! performing C++ MPSC Queues.
//!
//! Split across submodules: [`shared`] holds the pre-allocated node pool, the
//! intrusive Vyukov list (`BoundedQueue`), the per-handle `LocalCache`, the
//! generational chunk-recycling stack, waiter coordination, and the common
//! `publish_run` send tail (see its module doc for the full algorithm);
//! [`producer`] and [`consumer`] hold the sync/async handles and their
//! send/recv futures; [`miri`] holds the `cfg(miri)` custody scaffolding.

mod consumer;
mod miri;
mod producer;
mod shared;

#[cfg(test)]
mod tests;

use std::cell::RefCell;

use crate::internal::sync::{Arc, AtomicBool, Mutex};

pub use consumer::{
  AsyncReceiver, BoundedRecvBatchFuture, BoundedRecvBatchMutFuture, Receiver, RecvFuture,
};
pub use producer::{
  AsyncSender, BoundedSendBatchFuture, BoundedSendBatchMutFuture, SendFuture, Sender,
};
pub use shared::BoundedQueue;

pub(crate) use shared::LocalCache;

pub fn bounded_async<T: Send>(capacity: usize) -> (AsyncSender<T>, AsyncReceiver<T>) {
  let shared = Arc::new(BoundedQueue::new(capacity));
  let sender = AsyncSender {
    shared: Arc::clone(&shared),
    closed: AtomicBool::new(false),
  };
  let receiver = AsyncReceiver {
    shared,
    closed: AtomicBool::new(false),
    is_registered: false,
    cache: Mutex::new(LocalCache::default()),
  };
  (sender, receiver)
}

pub fn bounded<T: Send>(capacity: usize) -> (Sender<T>, Receiver<T>) {
  let shared = Arc::new(BoundedQueue::new(capacity));
  let sender = Sender {
    shared: Arc::clone(&shared),
    closed: AtomicBool::new(false),
  };
  let receiver = Receiver {
    shared,
    closed: AtomicBool::new(false),
    cache: RefCell::new(LocalCache::default()),
  };
  (sender, receiver)
}
