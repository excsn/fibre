// examples/rolling_usage.rs

use log::info;
use std::path::Path;
use std::thread;
use std::time::Duration;

fn main() {
  // --- Initialization ---
  let log_dir = Path::new("logs/rolling");
  if !log_dir.exists() {
    std::fs::create_dir_all(log_dir).expect("Failed to create rolling log directory");
  }

  // Find and initialize fibre_telemetry from our new rolling config file.
  let config_path = Path::new(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/examples/fibre_telemetry.rolling.yaml"
  ));
  let _guards = fibre_telemetry::init::init_from_file(config_path)
    .expect("Failed to initialize fibre_telemetry");

  // --- Logging ---
  println!("\n--- Logging every 5 seconds for 70 seconds to trigger rotation ---");
  println!("A new log file should be created in 'logs/rolling/' after the minute changes.");

  for i in 1..=14 {
    info!("Logging message #{}", i);
    thread::sleep(Duration::from_secs(5));
  }

  println!("\n--- Logging Complete ---");
  println!("Check the 'logs/rolling' directory for multiple 'app.log.YYYY-MM-DD-HH-mm' files.");

  // The `_guards` variable is dropped here, flushing any buffered file writes.
}
