use fibre_ioc::{global, resolve};
use std::panic;

struct UnregisteredService;

fn main() {
  // --- Using the panicking `resolve!` macro ---
  println!("Attempting to resolve a service that was never registered...");

  let result = panic::catch_unwind(|| {
    // This line will panic!
    let _service = resolve!(UnregisteredService);
  });

  assert!(result.is_err(), "resolve! should have panicked.");
  println!("Successfully caught the expected panic from resolve!.");

  // --- Using the non-panicking `get()` method ---
  println!("\nNow, attempting to resolve using the fallible `get()` method...");

  let service_option = global().get::<UnregisteredService>(None);

  match service_option {
    Some(_) => panic!("Should not have found the service!"),
    None => println!("Correctly received `None` for the missing service."),
  }
}
