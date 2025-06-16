use fibre_ioc::{global, resolve};
use std::sync::Arc;

// --- Test Fixtures ---

// The trait must be Send + Sync for the container to accept it.
trait Greeter: Send + Sync {
  fn greet(&self) -> String;
}

struct EnglishGreeter;
impl Greeter for EnglishGreeter {
  fn greet(&self) -> String {
    "Hello!".to_string()
  }
}

// A simple struct for testing.
#[derive(Debug, PartialEq, Eq)]
struct SimpleService {
  id: u32,
}

// --- Basic Tests ---

#[test]
fn test_unnamed_singleton_factory() {
  // Arrange
  global().add_singleton(|| SimpleService { id: 101 });

  // Act
  let r1 = resolve!(SimpleService);
  let r2 = resolve!(SimpleService);

  // Assert
  assert_eq!(r1.id, 101);
  // Ensure it's a singleton by checking pointer equality.
  assert!(Arc::ptr_eq(&r1, &r2));
}

#[test]
fn test_named_singleton_instance() {
  // Arrange
  let instance = SimpleService { id: 202 };
  global().add_instance_with_name("named_instance", instance);

  // Act
  let r1 = resolve!(SimpleService, "named_instance");
  let r2 = resolve!(SimpleService, "named_instance");

  // Assert
  assert_eq!(r1.id, 202);
  assert!(Arc::ptr_eq(&r1, &r2));
}

#[test]
fn test_unnamed_transient_factory() {
  // Arrange
  // Use a unique type to avoid conflicts with other tests using unnamed SimpleService
  struct TransientService {
    id: u32,
  }
  global().add_transient(|| TransientService { id: 303 });

  // Act
  let r1 = resolve!(TransientService);
  let r2 = resolve!(TransientService);

  // Assert
  assert_eq!(r1.id, 303);
  assert_eq!(r2.id, 303);
  // Ensure it's a transient by checking the pointers are different.
  assert!(!Arc::ptr_eq(&r1, &r2));
}

#[test]
fn test_unnamed_trait_resolution() {
  // Arrange: Call the correct API, providing the Arc'd trait object.
  global().add_singleton_trait::<dyn Greeter>(|| Arc::new(EnglishGreeter));

  // Act
  let greeter = resolve!(trait Greeter);

  // Assert
  assert_eq!(greeter.greet(), "Hello!");
}

#[test]
fn test_named_trait_resolution() {
  // Arrange
  struct GermanGreeter;
  impl Greeter for GermanGreeter {
    fn greet(&self) -> String {
      "Hallo!".to_string()
    }
  }
  global().add_singleton_trait_with_name::<dyn Greeter>("german", || Arc::new(GermanGreeter));

  // Act
  let greeter = resolve!(trait Greeter, "german");

  // Assert
  assert_eq!(greeter.greet(), "Hallo!");
}

#[test]
#[should_panic(expected = "Failed to resolve required service")]
fn test_resolve_panics_on_missing_concrete_service() {
  struct MissingService;
  resolve!(MissingService);
}

#[test]
#[should_panic(expected = "Failed to resolve required trait service")]
fn test_resolve_panics_on_missing_trait_service() {
  // The test trait must also be Send + Sync to be a valid type for `get`.
  trait MissingTrait: Send + Sync {}
  resolve!(trait MissingTrait);
}
