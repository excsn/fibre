use std::path::Path;
use tracing::{event, info, Level};

fn main() {
  let config_path = Path::new(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/examples/fibre_telemetry.debug.yaml"
  ));
  let _guards = fibre_telemetry::init::init_from_file(config_path).expect("Failed to initialize");

  println!("--- Running Application with Debug Report Enabled ---");

  println!("A debug report will be printed every 5 seconds.");
  info!("This is a standard log, it will go to the console.");

  // This event is targeted at 'my_app' and will be captured by the debug_report appender.
  event!(target: "my_app", Level::DEBUG, item_id = 123, message = "Processing item.");

  // We can increment counters by sending an event with a special "counter" field.
  event!(target: "my_app", Level::INFO, counter = "items_processed");
  event!(target: "my_app", Level::INFO, counter = "items_processed");
  event!(target: "my_app", Level::WARN, counter = "errors_encountered");

  for i in 0..15 {
    info!(message = format!("Main loop, iteration {}", i));
    event!(target: "my_app", Level::INFO, counter = "items_processed");
    if i % 5 == 0 {
      event!(target: "my_app", Level::WARN, counter = "errors_encountered");
    }
    std::thread::sleep(std::time::Duration::from_secs(1));
  }

  info!("This is another standard log for the console.");

  println!("\n--- Application finished. Printing debug report. ---\n");

  // At the end of our test or run, we print the collected report.
  fibre_telemetry::debug_report::print_debug_report();
}
