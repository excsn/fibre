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
pub mod guards;
pub mod init;
pub mod model;
// pub mod appenders;
pub mod subscriber;
// pub mod core; // For 'fibre' channels or other core utilities

// Re-export key public types for easier use by library consumers.
pub use error::{Error, Result};
pub use error_handling::{InternalErrorReport, InternalErrorSource};
pub use guards::WorkerGuardCollection;
pub use model::{LogValue, TelemetryEvent};

use std::collections::HashMap;

pub type CustomEventReceiver = fibre::mpsc::BoundedReceiver<TelemetryEvent>;

// This will be the main struct returned by initialization.
#[must_use = "The InitResult and its guards must be kept alive for logging to work correctly and flush on exit"]
pub struct InitResult {
  pub guard_collection: WorkerGuardCollection,
  // If error reporting is enabled, this receiver can be used to get internal error reports.
  pub internal_error_rx: Option<fibre::mpsc::BoundedReceiver<InternalErrorReport>>,
  pub custom_streams: HashMap<String, CustomEventReceiver>,
}

// Public initialization functions
pub use init::{find_config_file, init_from_file}; // ADDED re-export

#[cfg(test)]
mod tests {
  use super::*; // To access InitResult etc. if needed in lib.rs tests

  #[test]
  fn it_compiles() {
    let _ = find_config_file(None); // Basic check for find_config_file
    assert_eq!(2 + 2, 4);
  }
}
