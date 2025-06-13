use std::{future::Future, pin::Pin};

/// A trait for spawning a future onto an asynchronous runtime.
pub trait TaskSpawner: Send + Sync + 'static {
  /// Spawns a type-erased future.
  fn spawn(&self, future: Pin<Box<dyn Future<Output = ()> + Send>>);
}

#[cfg(feature = "tokio")]
pub struct TokioSpawner(tokio::runtime::Handle);

#[cfg(feature = "tokio")]
impl TokioSpawner {
  /// Creates a spawner that uses the current Tokio runtime context.
  /// Panics if called outside of a Tokio runtime.
  pub fn new() -> Self {
    Self(tokio::runtime::Handle::current())
  }
}

#[cfg(feature = "tokio")]
impl TaskSpawner for TokioSpawner {
  fn spawn(&self, future: Pin<Box<dyn Future<Output = ()> + Send>>)
  {
    self.0.spawn(future);
  }
}
