use fibre_ioc::{global, Container}; // Note: we don't need resolve! here

// A function that configures dependencies and runs some logic.
// By accepting a `&Container`, it can be tested with a controlled environment.
fn process_data(container: &Container) -> String {
  // Register a data source ONLY within the scope of this container.
  container.add_instance("test data".to_string());

  // Resolve the dependency from the provided container.
  let data = container
    .get::<String>(None)
    .expect("Data not found in container");
  format!("Processed: {}", data.to_uppercase())
}

fn main() {
  // --- Test Scenario with a Local Container ---
  println!("--- Running with a local container ---");
  let test_container = Container::new();
  let result = process_data(&test_container);

  println!("Result: {}", result);
  assert_eq!(result, "Processed: TEST DATA");

  // --- Verify Isolation ---
  // The service registered in `test_container` should NOT exist in the global container.
  let global_result = global().get::<String>(None);
  assert!(
    global_result.is_none(),
    "Dependency should not have leaked into the global container!"
  );

  println!("\nVerified that the local container is isolated from the global one.");
}
