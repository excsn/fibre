// src/guards.rs
// Holds onto resources that need to be kept alive for the duration of logging
// and cleaned up on drop, like tracing-appender's WorkerGuards.

use tracing_appender::non_blocking::WorkerGuard;

/// A collection of `WorkerGuard`s from `tracing-appender`.
///
/// When this struct is dropped, all contained guards are dropped, ensuring that
/// any buffered log messages in non-blocking writers are flushed.
#[must_use = "The InitResult and its guards must be kept alive for logging to work correctly and flush on exit"]
pub struct WorkerGuardCollection {
  guards: Vec<WorkerGuard>,
}

impl WorkerGuardCollection {
  pub(crate) fn new() -> Self {
    Self { guards: Vec::new() }
  }

  pub(crate) fn add(&mut self, guard: WorkerGuard) {
    self.guards.push(guard);
  }

  #[cfg(test)] // Helper for tests if needed
  pub(crate) fn is_empty(&self) -> bool {
    self.guards.is_empty()
  }
}