// examples/tracing_usage.rs

use std::path::Path;
use std::thread;
use std::time::Duration;
use tracing::{debug, error, info, info_span, instrument, warn, Instrument, Level};

// A module simulating a database interaction.
// We use `instrument` to automatically create a span around this function.
#[instrument(
    name = "db_query",
    skip(sql), // Don't include the SQL query in the span's fields by default
    fields(db.table = "users", db.statement = %sql) // Add custom fields
)]
fn execute_query(sql: &str, user_id: u32) {
  info!(target: "database", "Executing query for user_id={}", user_id);
  thread::sleep(Duration::from_millis(50)); // Simulate DB work
  tracing::event!(
      target: "database",
      Level::DEBUG,
      query.success = true,
      message = "Query finished."
  );
}

// A module simulating an external API call.
mod api_client {
  use std::thread;
  use std::time::Duration;
  use tracing::{info, info_span, instrument};

  // This function will be manually instrumented with a span.
  pub fn call_external_service() {
    // Manually create a span. This is useful when you need more control
    // than the `#[instrument]` attribute provides.
    let api_span = info_span!(
      "api_call",
      http.method = "GET",
      http.url = "https://api.example.com/data"
    );

    // The `in_scope` method makes this span the current one for its duration.
    // Any events logged inside this block will be associated with this span.
    let _enter = api_span.enter();

    info!("Calling external service...");
    thread::sleep(Duration::from_millis(75));
    info!(http.status_code = 200, "Service call successful.");
  }
}

fn main() {
  // --- Initialization ---
  // Initialize fibre_telemetry from a dedicated config file for this example.
  let config_path = Path::new(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/examples/fibre_telemetry.tracing.yaml"
  ));
  let _guards = fibre_telemetry::init::init_from_file(config_path)
    .expect("Failed to initialize fibre_telemetry");

  // --- Tracing Operations ---
  println!("\n--- Running Tracing Example ---\n");

  // 1. A top-level span to encompass the entire request processing.
  // All events and sub-spans within this block will be children of `process_request`.
  let request_span = info_span!("process_request", request_id = "xyz-123");
  let _guard = request_span.enter();

  info!("Request processing started. This is a standard event.");
  tracing::event!(
    Level::WARN,
    "user.credit" = 5,
    message = "User credit is low. This event has a structured field."
  );

  // 2. Call the function with the `#[instrument]` attribute.
  // This will create a `db_query` span that is a child of `process_request`.
  execute_query("SELECT * FROM users WHERE id = ?", 101);

  // 3. Call the function that creates its own span manually.
  // This will create an `api_call` span, also a child of `process_request`.
  api_client::call_external_service();

  // 4. Using `instrument()` to attach a span to a future or block of code.
  async {
    info!("Processing payment...");
    thread::sleep(Duration::from_millis(30));
    error!(
      error.code = "payment_failed",
      "Payment provider rejected transaction."
    );
  }
  .instrument(info_span!("payment_processing"))
  .now_or_never(); // We use now_or_never to run the async block synchronously for the example.

  info!("Request processing finished.");

  println!("\n--- Tracing Complete ---\n");
  println!("Check the console output and the contents of 'logs/tracing_example.log'");
  println!("The log file should contain JSON objects with span details.");

  // `_guards` is dropped here, flushing logs.
}

// Dummy `now_or_never` for non-async context
trait FutureExt: std::future::Future {
  fn now_or_never(self) -> Option<Self::Output>;
}

impl<F: std::future::Future> FutureExt for F {
  fn now_or_never(self) -> Option<Self::Output> {
    use std::sync::Arc;
    use std::task::{Context, Poll, Wake, Waker};

    struct NoOpWaker;
    impl Wake for NoOpWaker {
      fn wake(self: Arc<Self>) {}
    }
    let waker = Waker::from(Arc::new(NoOpWaker));
    let mut cx = Context::from_waker(&waker);

    // Pin the future to the stack
    let mut pinned_future = self;
    let mut future = unsafe { std::pin::Pin::new_unchecked(&mut pinned_future) };

    match future.as_mut().poll(&mut cx) {
      Poll::Ready(output) => Some(output),
      Poll::Pending => None,
    }
  }
}
