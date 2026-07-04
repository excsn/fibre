//! `fibre_logging` - A flexible logging library built on top of `tracing`.
//!
//! This library aims to provide powerful, configuration-driven logging capabilities,
//! inspired by frameworks like log4j/logback and log4rs, while leveraging the
//! `tracing` ecosystem for instrumentation and structured logging.

/// Prints library chatter to stderr only when `FIBRE_LOGGING_VERBOSE` is set.
/// Errors and warnings are printed unconditionally (directly via `eprintln!`).
macro_rules! vlog {
  ($($arg:tt)*) => {
    if $crate::verbose() {
      eprintln!($($arg)*);
    }
  };
}
pub(crate) use vlog;

/// Whether diagnostic chatter is enabled via the `FIBRE_LOGGING_VERBOSE`
/// environment variable (any value other than "0").
pub(crate) fn verbose() -> bool {
  use std::sync::OnceLock;
  static VERBOSE: OnceLock<bool> = OnceLock::new();
  *VERBOSE.get_or_init(|| std::env::var_os("FIBRE_LOGGING_VERBOSE").is_some_and(|v| v != "0"))
}

// Declare modules following the file structure
pub mod config;
pub mod encoders;
pub mod error;
pub mod error_handling;
pub mod init;
pub mod model;
mod roller;
pub mod subscriber;

#[cfg(debug_assertions)]
pub mod debug_report;

// This section defines what `debug_report` means in a release build.
#[cfg(not(debug_assertions))]
pub mod debug_report {
  // In release mode, the functions exist but do nothing.
  // They are marked `inline(always)` so the compiler can completely remove the calls.
  #[inline(always)]
  pub fn print_debug_report() {}

  #[inline(always)]
  pub fn clear_debug_report() {}

  #[inline(always)]
  pub fn debug_report_totals() -> (usize, usize) {
    (0, 0)
  }
}

// Re-export key public types for easier use by library consumers.
pub use error::{Error, Result};
pub use error_handling::{InternalErrorReport, InternalErrorSource};
pub use model::{LogValue, LogEvent};

use std::{
  collections::HashMap,
  sync::{atomic::AtomicBool, Arc},
  thread::JoinHandle,
  time::{Duration, Instant},
};

pub type CustomEventReceiver = fibre::mpsc::BoundedSyncReceiver<LogEvent>;

/// A handle to a spawned appender background task.
/// The task will terminate when the sender half of its channel is dropped.
pub type AppenderTaskHandle = JoinHandle<()>;

// Public initialization functions
pub use init::{find_config_file, init_from_file};

/// How long `Drop` waits for appender tasks to flush before abandoning them.
const DEFAULT_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

// This will be the main struct returned by initialization.
#[must_use = "The InitResult and its guards must be kept alive for logging to work correctly and flush on exit"]
pub struct InitResult {
  pub appender_task_handles: Vec<AppenderTaskHandle>,
  pub(crate) appender_task_names: Vec<String>,
  pub(crate) shutdown_signal: Arc<AtomicBool>,
  // If error reporting is enabled, this receiver can be used to get internal error reports.
  pub internal_error_rx: Option<fibre::mpsc::BoundedSyncReceiver<InternalErrorReport>>,
  pub custom_streams: HashMap<String, CustomEventReceiver>,
}

impl InitResult {
  /// Shuts down all appender tasks, waiting at most `timeout` for them to
  /// flush. Tasks that don't finish in time are reported and abandoned
  /// instead of hanging the process. `Drop` calls this with a 5s default.
  pub fn shutdown(mut self, timeout: Duration) {
    self.shutdown_impl(timeout);
  }

  fn shutdown_impl(&mut self, timeout: Duration) {
    if self.appender_task_handles.is_empty() {
      return;
    }

    // 1. Tell all writer threads to stop their loops.
    self
      .shutdown_signal
      .store(true, std::sync::atomic::Ordering::SeqCst);

    vlog!("[fibre_logging] Shutting down. Waiting for appender tasks to flush...");

    // 2. Join with a deadline. A writer stuck in a blocking write (full disk,
    // dead pipe) must not hang process exit forever.
    let mut names = std::mem::take(&mut self.appender_task_names).into_iter();
    let mut pending: Vec<(String, AppenderTaskHandle)> = self
      .appender_task_handles
      .drain(..)
      .map(|handle| {
        (
          names.next().unwrap_or_else(|| "appender".to_string()),
          handle,
        )
      })
      .collect();

    let deadline = Instant::now() + timeout;
    loop {
      let mut i = 0;
      while i < pending.len() {
        if pending[i].1.is_finished() {
          let (name, handle) = pending.swap_remove(i);
          if let Err(e) = handle.join() {
            eprintln!(
              "[fibre_logging:ERROR] Appender task '{}' panicked during shutdown: {:?}",
              name, e
            );
          }
        } else {
          i += 1;
        }
      }

      if pending.is_empty() || Instant::now() >= deadline {
        break;
      }
      std::thread::sleep(Duration::from_millis(10));
    }

    for (name, _handle) in pending {
      eprintln!(
        "[fibre_logging:ERROR] Appender task '{}' did not shut down within {:?}; abandoning it.",
        name, timeout
      );
    }

    vlog!("[fibre_logging] Shutdown complete.");
  }
}

impl Drop for InitResult {
  fn drop(&mut self) {
    self.shutdown_impl(DEFAULT_SHUTDOWN_TIMEOUT);
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::sync::atomic::Ordering;
  use std::thread;

  fn make_init_result(
    handles: Vec<AppenderTaskHandle>,
    names: Vec<String>,
    signal: Arc<AtomicBool>,
  ) -> InitResult {
    InitResult {
      appender_task_handles: handles,
      appender_task_names: names,
      shutdown_signal: signal,
      internal_error_rx: None,
      custom_streams: HashMap::new(),
    }
  }

  #[test]
  fn shutdown_joins_cooperative_tasks_promptly() {
    let signal = Arc::new(AtomicBool::new(false));
    let task_signal = Arc::clone(&signal);
    let handle = thread::spawn(move || {
      while !task_signal.load(Ordering::Relaxed) {
        thread::sleep(Duration::from_millis(5));
      }
    });

    let result = make_init_result(vec![handle], vec!["coop".to_string()], signal);
    let start = Instant::now();
    result.shutdown(Duration::from_secs(5));
    assert!(
      start.elapsed() < Duration::from_secs(1),
      "cooperative task should be joined well before the timeout"
    );
  }

  #[test]
  fn shutdown_abandons_stuck_tasks_at_timeout() {
    let signal = Arc::new(AtomicBool::new(false));
    // Simulates a writer stuck in a blocking write: never observes the signal.
    let handle = thread::spawn(|| loop {
      thread::sleep(Duration::from_secs(60));
    });

    let result = make_init_result(vec![handle], vec!["stuck".to_string()], signal);
    let start = Instant::now();
    result.shutdown(Duration::from_millis(200));
    let elapsed = start.elapsed();
    assert!(elapsed >= Duration::from_millis(200));
    assert!(
      elapsed < Duration::from_secs(2),
      "shutdown must return at the timeout instead of hanging: {:?}",
      elapsed
    );
  }
}
