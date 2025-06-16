use fibre_ioc::{global, resolve};
use std::sync::Arc;

// 1. Define the abstraction (the trait)
trait Logger: Send + Sync {
  fn log(&self, message: &str);
}

// 2. Define a concrete implementation
struct ConsoleLogger;
impl Logger for ConsoleLogger {
  fn log(&self, message: &str) {
    println!("[CONSOLE LOG]: {}", message);
  }
}

// 3. Define a service that depends on the abstraction
struct ReportService {
  logger: Arc<dyn Logger>,
}

impl ReportService {
  fn generate_report(&self) {
    self.logger.log("Starting report generation.");
    // ... logic to generate report ...
    self.logger.log("Finished report generation.");
  }
}

fn main() {
  // --- Registration ---

  // Register the concrete ConsoleLogger as the implementation for the `dyn Logger` trait.
  // The container will store Arc<ConsoleLogger> but serve it as Arc<dyn Logger>.
  global().add_singleton_trait::<dyn Logger>(|| Arc::new(ConsoleLogger));

  // Register the ReportService. Its factory *resolves* its own dependency (the logger).
  // This is the "inversion of control". ReportService doesn't create its logger.
  global().add_singleton(|| ReportService {
    logger: resolve!(trait Logger),
  });

  // --- Resolution and Usage ---
  println!("Resolving the high-level service...");
  let report_service = resolve!(ReportService);

  println!("Using the service...");
  report_service.generate_report();

  // The output will show messages from the ConsoleLogger, proving the dependency was injected.
}
