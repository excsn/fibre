// examples/basic_usage.rs

use log::{debug, error, info, warn}; // Using the `log` facade
use std::path::Path;
use tracing::instrument; // Using `tracing` for structured logs and spans

// A module to simulate our application's logic.
mod my_app {
  use tracing::{debug, info, instrument};

  #[instrument]
  pub fn run_task(task_id: u32) {
    info!(task_id, "Starting complex task");
    // Some work is done here...
    debug!(task_id, "An intermediate step in the task succeeded.");
    info!(task_id, "Task finished.");
  }
}

// A module to simulate a noisy dependency.
mod noisy_crate {
  use tracing::debug;

  pub fn do_something_verbose() {
    debug!("This is a very frequent and noisy debug message.");
  }
}

fn main() {
  // --- Initialization ---
  // Create the log directory for the example.
  let log_dir = Path::new("logs");
  if !log_dir.exists() {
    std::fs::create_dir(log_dir).expect("Failed to create log directory");
  }

  // Find and initialize fibre_telemetry from our example config file.
  // We place the config inside the `examples` directory for cargo to find it.
  let config_path = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/examples/fibre_telemetry.yaml"));
  let _guards = fibre_telemetry::init::init_from_file(config_path)
    .expect("Failed to initialize fibre_telemetry");

  // --- Logging ---
  println!("\n--- Emitting Logs ---\n");

  // These logs use the `log` crate facade.
  info!("Application starting up. This should go to console and file.");
  debug!("This top-level debug message should NOT appear anywhere (root is INFO).");
  warn!("A minor warning has occurred. This should go to console and file.");
  error!("A critical error! This should go to console and file.");

  println!("\n--- Running 'my_app' Task (logs to file only) ---\n");
  my_app::run_task(123);

  println!("\n--- Simulating 'noisy_crate' (should not produce output) ---\n");
  noisy_crate::do_something_verbose();

  println!("\n--- Logging Complete ---\n");
  println!("Check the console output and the contents of 'logs/mvp_example.log'");

  // The `_guards` variable is dropped here, flushing any buffered file writes.
}