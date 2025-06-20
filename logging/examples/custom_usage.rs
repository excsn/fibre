// examples/custom_usage.rs

use fibre_logging::{init, InternalErrorSource};
use log::info;
use std::fs;
use std::path::Path;
use std::thread;
use std::time::Duration;
use tracing::{event, Level};

fn main() {
  // --- Setup Phase ---
  println!("--- Test Setup ---");
  let log_dir = Path::new("logs");
  let failing_path = log_dir.join("failing_write.log");

  // 1. Clean up from any previous failed runs.
  if log_dir.exists() {
    // If the file exists and is read-only, we must make it writable before deleting.
    if failing_path.exists() {
      if let Ok(metadata) = fs::metadata(&failing_path) {
        let mut perms = metadata.permissions();
        if perms.readonly() {
          perms.set_readonly(false);
          fs::set_permissions(&failing_path, perms)
            .expect("Failed to make old log writable for cleanup");
        }
      }
    }
    // Now we can safely remove the directory and its contents.
    fs::remove_dir_all(log_dir).expect("Failed to clean up old log directory");
  }
  
  // Create the directory fresh for this run.
  fs::create_dir(log_dir).expect("Failed to create log directory");

  fs::write(&failing_path, "Initial content.\n").expect("Failed to create failing log file");

  // --- Initialization Phase ---
  println!("\n--- Initializing fibre_logging ---");
  let config_path = Path::new(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/examples/fibre_logging.advanced.yaml"
  ));
  let mut init_result =
    init::init_from_file(config_path).expect("Failed to initialize fibre_logging");

  // --- Post-Init Setup to Trigger Runtime Error ---
  println!("Setting file to read-only to trigger a runtime write error.");
  let mut perms = fs::metadata(&failing_path)
    .expect("Failed to get metadata")
    .permissions();
  perms.set_readonly(true);
  fs::set_permissions(&failing_path, perms).expect("Failed to set read-only permissions");

  // --- Actor/Consumer Phase ---
  // 1. Spawn a thread to consume internal error reports.
  if let Some(error_rx) = init_result.internal_error_rx.take() {
    println!("Internal error channel is active. Spawning error consumer thread.");
    thread::spawn(move || {
      // CORRECTED: Use the documented `recv()` method, which blocks until an item is available
      // or the channel is disconnected.
      match error_rx.recv() {
        Ok(report) => {
          println!("\n!!! [Error Consumer] Received Internal Error Report !!!");
          println!("  Source:    {}", report.source);
          println!("  Message:   {}", report.error_message);
          println!("  Timestamp: {}", report.timestamp);
          if let InternalErrorSource::AppenderWrite { appender_name } = report.source {
            assert_eq!(appender_name, "failing_file");
          } else {
            panic!("Received unexpected error source!");
          }
        }
        Err(_) => {
          panic!("[Error Consumer] Channel disconnected before an error report was received!");
        }
      }
      println!("[Error Consumer] Test complete.");
    });
  } else {
    panic!("Internal error channel is disabled, but this test requires it.");
  }

  // 2. Spawn a thread to consume the custom metrics stream.
  if let Some(metrics_rx) = init_result.custom_streams.remove("metrics_stream") {
    println!("Custom 'metrics_stream' channel is active. Spawning metrics consumer thread.");
    thread::spawn(move || {
      // CORRECTED: Use the documented `recv()` method.
      match metrics_rx.recv() {
        Ok(event) => {
          println!("\n>>> [Metrics Consumer] Received LogEvent >>>");
          println!("  Target:  {}", event.target);
          println!("  Level:   {}", event.level);
          println!("  Message: {}", event.message.as_deref().unwrap_or("N/A"));
          println!("  Fields:  {:?}", event.fields);
          assert_eq!(event.target, "METRICS");
        }
        Err(_) => {
          panic!("[Metrics Consumer] Channel disconnected before a metric event was received!");
        }
      }
      println!("[Metrics Consumer] Test complete.");
    });
  }

  // --- Logging Phase ---
  println!("\n--- Emitting Logs ---");
  thread::sleep(Duration::from_millis(200));

  info!("Emitting a metric event...");
  event!(target: "METRICS", Level::TRACE, message = "Database query finished.", duration_ms = 123, db_table = "users");

  info!("Emitting an event that will cause a write error...");
  event!(target: "FAIL", Level::ERROR, "This write will fail.");

  // --- Shutdown Phase ---
  info!("Example finished. Shutting down in 2 seconds...");
  println!("\n--- Waiting for consumer threads to finish... ---");
  thread::sleep(Duration::from_secs(2));

  // --- Test Cleanup ---
  println!("\n--- Test Cleanup ---");
  let mut perms = fs::metadata(&failing_path).unwrap().permissions();
  perms.set_readonly(false);
  fs::set_permissions(&failing_path, perms).unwrap();
  fs::remove_file(&failing_path).expect("Failed to clean up failing log file.");
  println!("Example complete.");
}
