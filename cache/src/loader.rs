use parking_lot::Mutex;
use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};
use std::thread::Thread;

/// Represents a waiter in the queue for a `LoadFuture`.
pub(crate) enum Waiter {
  Sync(Thread),
  Async(Waker),
}

impl Waiter {
  fn wake(self) {
    match self {
      Waiter::Sync(thread) => thread.unpark(),
      Waiter::Async(waker) => waker.wake(),
    }
  }
}

/// The internal state of a value being loaded.
pub(crate) enum State<V> {
  Computing,
  Complete(Arc<V>),
}

/// The internal, mutex-protected core of the LoadFuture.
pub(crate) struct Inner<V> {
  pub(crate) state: State<V>,
  pub(crate) waiters: VecDeque<Waiter>,
}

/// A future that represents a value being computed for the cache.
/// It can be awaited by multiple sync threads and async tasks simultaneously.
pub(crate) struct LoadFuture<V> {
  pub(crate) inner: Mutex<Inner<V>>,
}

impl<V> LoadFuture<V> {
  /// Creates a new `LoadFuture` in the "Computing" state.
  pub fn new() -> Self {
    Self {
      inner: Mutex::new(Inner {
        state: State::Computing,
        waiters: VecDeque::new(),
      }),
    }
  }

  /// Completes the future with a value, waking all waiters.
  pub fn complete(&self, value: Arc<V>) {
    let mut inner = self.inner.lock();
    inner.state = State::Complete(value);
    for waiter in inner.waiters.drain(..) {
      waiter.wake();
    }
  }
}

impl<V> Future for &LoadFuture<V> {
  type Output = Arc<V>;

  fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    let mut inner = self.inner.lock();
    match &inner.state {
      State::Complete(value) => Poll::Ready(value.clone()),
      State::Computing => {
        inner.waiters.push_back(Waiter::Async(cx.waker().clone()));
        Poll::Pending
      }
    }
  }
}

/// An enum that holds either a synchronous or an asynchronous loader function.
///
/// This is stored in the `CacheBuilder` and `CacheShared` to define how
/// missing values should be computed. We use `Box<dyn ...>` to store the
/// closure, which can have an unknown size.
/// An enum that holds either a synchronous or an asynchronous loader function.
///
/// The function must return a tuple containing the value and its associated cost.
pub(crate) enum Loader<K, V> {
  Sync(Arc<dyn Fn(K) -> (V, u64) + Send + Sync>),
  Async(Arc<dyn Fn(K) -> Pin<Box<dyn Future<Output = (V, u64)> + Send>> + Send + Sync>),
}

impl<K, V> Clone for Loader<K, V> {
  fn clone(&self) -> Self {
    match self {
      Loader::Sync(f) => Loader::Sync(f.clone()),
      Loader::Async(f) => Loader::Async(f.clone()),
    }
  }
}
