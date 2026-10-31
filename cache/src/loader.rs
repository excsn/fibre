use crate::sync::HybridMutex;
use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::task::{Context, Poll, Waker};
use std::thread::{Thread, ThreadId};

pub(crate) const NO_LOADER: &str = "fibre_cache: fetch_with missed and the cache has no loader";
pub(crate) const LOADER_PANICKED: &str = "fibre_cache: the loader panicked for this key";
pub(crate) const LOAD_DROPPED: &str = "fibre_cache: the task running the loader was dropped, as when its runtime shuts down";
pub(crate) const LOADER_REENTERED: &str = "fibre_cache: a loader called fetch_with on the key it is loading";

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
  Panicked,
  Dropped,
}

/// The internal, mutex-protected core of the LoadFuture.
pub(crate) struct Inner<V> {
  pub(crate) state: State<V>,
  pub(crate) waiters: VecDeque<Waiter>,
}

impl<V> Inner<V> {
  fn finish(&mut self, state: State<V>) {
    self.state = state;
    for waiter in self.waiters.drain(..) {
      waiter.wake();
    }
  }
}

/// A future that represents a value being computed for the cache.
/// It can be awaited by multiple sync threads and async tasks simultaneously.
pub(crate) struct LoadFuture<V> {
  pub(crate) inner: HybridMutex<Inner<V>>,
  /// The thread running the loader, when it runs on its caller's thread.
  pub(crate) leader: OnceLock<ThreadId>,
  /// Cleared while a refresh waits in the queue. Whoever sets it runs the load.
  started: AtomicBool,
  /// Set when the task running the loader was dropped. A waiter holding `inner`
  /// at that moment sees this after releasing it and finishes the future.
  pub(crate) dropped: AtomicBool,
}

impl<V> LoadFuture<V> {
  /// Creates a new `LoadFuture` in the "Computing" state.
  pub fn new(leader: Option<ThreadId>) -> Self {
    let future = Self::queued();
    future.started.store(true, Ordering::Relaxed);
    if let Some(leader) = leader {
      let _ = future.leader.set(leader);
    }
    future
  }

  /// A load waiting in the refresh queue, not yet run by anyone.
  pub fn queued() -> Self {
    Self {
      inner: HybridMutex::new(Inner {
        state: State::Computing,
        waiters: VecDeque::new(),
      }),
      leader: OnceLock::new(),
      started: AtomicBool::new(false),
      dropped: AtomicBool::new(false),
    }
  }

  /// True for the one caller that takes a queued load and must run it.
  pub fn claim(&self) -> bool {
    !self.started.swap(true, Ordering::SeqCst)
  }

  pub fn is_dropped(&self) -> bool {
    self.dropped.load(Ordering::SeqCst)
  }

  /// Marks the load as dropped and wakes its waiters. Never waits, since a task
  /// can be dropped on an executor thread.
  pub fn drop_load(&self) {
    self.dropped.store(true, Ordering::SeqCst);
    if let Some(mut inner) = self.inner.try_lock() {
      if matches!(inner.state, State::Computing) {
        inner.finish(State::Dropped);
      }
    }
  }

  pub(crate) fn drop_load_locked(&self, inner: &mut Inner<V>) {
    inner.finish(State::Dropped);
  }

  pub fn complete(&self, value: Arc<V>) {
    self.inner.lock().finish(State::Complete(value));
  }

  pub fn fail(&self) {
    self.inner.lock().finish(State::Panicked);
  }

  pub async fn complete_async(&self, value: Arc<V>) {
    self.inner.lock_async().await.finish(State::Complete(value));
  }

  pub async fn fail_async(&self) {
    self.inner.lock_async().await.finish(State::Panicked);
  }
}

impl<V> Future for &LoadFuture<V> {
  type Output = Arc<V>;

  fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    let Some(mut inner) = self.inner.try_lock() else {
      cx.waker().wake_by_ref();
      return Poll::Pending;
    };
    match &inner.state {
      State::Complete(value) => Poll::Ready(value.clone()),
      State::Panicked => panic!("{}", LOADER_PANICKED),
      State::Dropped => panic!("{}", LOAD_DROPPED),
      State::Computing => {
        if self.is_dropped() {
          inner.finish(State::Dropped);
          panic!("{}", LOAD_DROPPED);
        }
        inner.waiters.push_back(Waiter::Async(cx.waker().clone()));
        drop(inner);
        if self.is_dropped() {
          cx.waker().wake_by_ref();
        }
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
