// examples/rolling_usage.rs

use log::{info, trace};
use std::path::Path;
use std::thread;
use std::time::Duration;

fn main() -> fibre_logging::Result<()> {
  // 1. Initialize the log system from a configuration file.
  let config_path = Path::new(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/examples/fibre_logging.rolling.yaml"
  ));
  let _guards = fibre_logging::init::init_from_file(config_path)
    .expect("Failed to initialize fibre_logging");
  // `_guards` must be kept alive to flush all logs on exit.

  println!("\n--- Spawning ALL threads to run concurrently ---");
  println!("This is a true stress test simulating a complex application.");
  println!("Time-based and size-based logging will happen simultaneously.");

  // A single vector to hold all thread handles.
  let mut all_handles = vec![];

  // --- Spawn the threads for the "time-based" test ---
  // These threads will log to the 'console' and 'simple_time_log' appenders.
  for thread_id in 0..3 {
    let handle = thread::spawn(move || {
      for i in 1..=10 {
        info!("Thread {} | General Log Message #{}", thread_id, i);
        thread::sleep(Duration::from_secs(thread_id + 1));
      }
    });
    all_handles.push(handle);
  }

  // --- Spawn the threads for the "size-based" test ---
  // These threads will log to the 'advanced_size_log' appender.
  let large_string = "A".repeat(100);
  for thread_id in 3..7 {
    let large_string_clone = large_string.clone();
    let handle = thread::spawn(move || {
      // 4 threads * 20 messages = 80 total messages.
      // Each message is ~155 bytes, so ~12.4KB total.
      // With a 1KB file size, this should trigger ~12 rolls.
      for i in 1..=20 {
        // MODIFIED: A controlled loop of 20 messages per thread.
        trace!(
            target: "high_volume",
            "Thread {} | High-Volume Message #{} | Data: {}",
            thread_id,
            i,
            large_string_clone
        );
      }
    });
    all_handles.push(handle);
  }

  println!(
    "All {} threads have been spawned and are running in parallel.",
    all_handles.len()
  );

  // --- Wait for ALL threads to complete at the very end ---
  // The main thread will block here until every single spawned thread has finished.
  for handle in all_handles {
    handle.join().unwrap();
  }

  println!("\n--- All threads have completed. Test finished. ---");
  trace!(target: "high_volume", "This is the very last message, post-roll.");
  println!("Check the 'logs/rolling/simple' and 'logs/rolling/advanced' directories.");

  // The `_guards` are dropped here, flushing any final log messages.
  Ok(())
}
