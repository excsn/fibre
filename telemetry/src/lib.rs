//! `fibre_telemetry` - A flexible logging library built on top of `tracing`.
//!
//! This library aims to provide powerful, configuration-driven logging capabilities,
//! inspired by frameworks like log4j/logback and log4rs, while leveraging the
//! `tracing` ecosystem for instrumentation and structured logging.

// Declare modules following the file structure
pub mod config;
pub mod encoders;
pub mod error;
pub mod error_handling;
pub mod init;
pub mod model;
mod roller;
pub mod subscriber;

// Re-export key public types for easier use by library consumers.
pub use error::{Error, Result};
pub use error_handling::{InternalErrorReport, InternalErrorSource};
pub use model::{LogValue, TelemetryEvent};

use std::{collections::HashMap, sync::{atomic::AtomicBool, Arc}, thread::JoinHandle};

pub type CustomEventReceiver = fibre::mpsc::BoundedReceiver<TelemetryEvent>;

/// A handle to a spawned appender background task.
/// The task will terminate when the sender half of its channel is dropped.
pub type AppenderTaskHandle = JoinHandle<()>;

// Public initialization functions
pub use init::{find_config_file, init_from_file};

// This will be the main struct returned by initialization.
#[must_use = "The InitResult and its guards must be kept alive for logging to work correctly and flush on exit"]
pub struct InitResult {
  pub appender_task_handles: Vec<AppenderTaskHandle>,
  pub(crate) shutdown_signal: Arc<AtomicBool>,
  // If error reporting is enabled, this receiver can be used to get internal error reports.
  pub internal_error_rx: Option<fibre::mpsc::BoundedReceiver<InternalErrorReport>>,
  pub custom_streams: HashMap<String, CustomEventReceiver>,
}

impl Drop for InitResult {
  fn drop(&mut self) {
    // 1. SET THE SHUTDOWN SIGNAL. This tells all writer threads to stop their loops.
    self
      .shutdown_signal
      .store(true, std::sync::atomic::Ordering::SeqCst);

    // 2. NOW, wait for them to finish.
    println!("[fibre_telemetry] Shutting down. Waiting for appender tasks to flush...");
    for handle in self.appender_task_handles.drain(..) {
      if let Err(e) = handle.join() {
        eprintln!(
          "[fibre_telemetry:ERROR] Appender task panicked during shutdown: {:?}",
          e
        );
      }
    }
    println!("[fibre_telemetry] Shutdown complete.");
  }
}
