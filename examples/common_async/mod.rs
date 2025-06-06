// examples/common_async/mod.rs
use std::future::Future;
use tokio::runtime::Runtime;

/// Helper to run a simple async main function.
pub fn run_async<F, T>(future: F) -> T
where
  F: Future<Output = T> + Send,
  T: Send,
{
  let rt = Runtime::new().expect("Failed to create Tokio runtime");
  rt.block_on(future)
}

// Helper to spawn a tokio task and immediately block on it for testing sync/async interop
pub fn block_on_tokio_task<F, T>(future: F) -> T
where
  F: Future<Output = T> + Send + 'static,
  T: Send + 'static,
{
  let rt = tokio::runtime::Builder::new_current_thread()
    .enable_all()
    .build()
    .expect("Failed to build Tokio runtime for task");
  rt.block_on(future)
}
