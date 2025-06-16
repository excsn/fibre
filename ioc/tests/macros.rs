// ioc/tests/macros.rs

//! Tests specifically for the resolution macros.
//! This file verifies the behavior of:
//! - `maybe_resolve!`
//! - `resolve!` (after refactoring)
//! - `maybe_resolve_from!`
//! - `resolve_from!`
//! against both the global `Container` and local `Container`/`LocalContainer` instances.

use fibre_ioc::{
  global, maybe_resolve, maybe_resolve_from, resolve, resolve_from, Container, LocalContainer,
};
use std::rc::Rc;
use std::sync::Arc;

// --- Test Fixtures ---

// Service for thread-safe container tests
struct MacroTestService {
  value: i32,
}
trait MacroTestTrait: Send + Sync {
  fn value(&self) -> i32;
}
impl MacroTestTrait for MacroTestService {
  fn value(&self) -> i32 {
    self.value
  }
}
struct UnregisteredService; // An unregistered type

// Service for single-threaded container tests (doesn't need Send + Sync)
struct LocalTestService {
  value: i32,
}
trait LocalTestTrait {
  fn value(&self) -> i32;
}
impl LocalTestTrait for LocalTestService {
  fn value(&self) -> i32 {
    self.value
  }
}
struct LocalUnregisteredService;

// --- Global Macro Tests ---

#[test]
fn test_maybe_resolve_global() {
  // Arrange
  global().add_singleton(|| MacroTestService { value: 42 });
  global().add_singleton_with_name("named", || MacroTestService { value: 43 });
  global().add_singleton_trait::<dyn MacroTestTrait>(|| Arc::new(MacroTestService { value: 44 }));
  global().add_singleton_trait_with_name::<dyn MacroTestTrait>("named_trait", || {
    Arc::new(MacroTestService { value: 45 })
  });

  // Act & Assert: Success cases
  assert_eq!(maybe_resolve!(MacroTestService).unwrap().value, 42);
  assert_eq!(maybe_resolve!(MacroTestService, "named").unwrap().value, 43);
  assert_eq!(maybe_resolve!(trait MacroTestTrait).unwrap().value(), 44);
  assert_eq!(
    maybe_resolve!(trait MacroTestTrait, "named_trait")
      .unwrap()
      .value(),
    45
  );

  // Act & Assert: Failure cases
  assert!(maybe_resolve!(UnregisteredService).is_none());
  assert!(maybe_resolve!(MacroTestService, "missing_name").is_none());
  trait MissingTrait: Send + Sync {}
  assert!(maybe_resolve!(trait MissingTrait).is_none());
  assert!(maybe_resolve!(trait MacroTestTrait, "missing_name").is_none());
}

#[test]
#[should_panic(expected = "Failed to resolve required service:")]
fn test_refactored_resolve_panics_on_missing() {
  // This is a sanity check to ensure the refactored `resolve!` macro still panics correctly.
  resolve!(UnregisteredService);
}

// --- `_from` Macro Tests with `Container` ---

#[test]
fn test_macros_with_custom_container() {
  // Arrange
  let container = Container::new();
  container.add_singleton(|| MacroTestService { value: 100 });
  container.add_singleton_with_name("named", || MacroTestService { value: 101 });
  container.add_singleton_trait::<dyn MacroTestTrait>(|| Arc::new(MacroTestService { value: 102 }));
  container.add_singleton_trait_with_name::<dyn MacroTestTrait>("named_trait", || {
    Arc::new(MacroTestService { value: 103 })
  });

  // Act & Assert with maybe_resolve_from!
  assert_eq!(
    maybe_resolve_from!(&container, MacroTestService)
      .unwrap()
      .value,
    100
  );
  assert_eq!(
    maybe_resolve_from!(&container, MacroTestService, "named")
      .unwrap()
      .value,
    101
  );
  assert!(maybe_resolve_from!(&container, UnregisteredService).is_none());

  // Act & Assert with resolve_from!
  assert_eq!(resolve_from!(&container, trait MacroTestTrait).value(), 102);
  assert_eq!(
    resolve_from!(&container, trait MacroTestTrait, "named_trait").value(),
    103
  );
}

#[test]
#[should_panic(expected = "Failed to resolve required trait service:")]
fn test_resolve_from_panics_on_missing_in_custom_container() {
  let container = Container::new();
  trait MissingTrait: Send + Sync {}
  // This should panic because the service is not in `container`.
  resolve_from!(&container, trait MissingTrait);
}

// --- `_from` Macro Tests with `LocalContainer` ---

#[test]
fn test_macros_with_local_container() {
  // Arrange
  let mut container = LocalContainer::new();
  container.add_singleton(|| LocalTestService { value: 200 });
  container.add_singleton_with_name("named", || LocalTestService { value: 201 });
  container.add_singleton_trait::<dyn LocalTestTrait>(|| Rc::new(LocalTestService { value: 202 }));
  container.add_singleton_trait_with_name::<dyn LocalTestTrait>("named_trait", || {
    Rc::new(LocalTestService { value: 203 })
  });

  // Act & Assert with maybe_resolve_from!
  assert_eq!(
    maybe_resolve_from!(&container, LocalTestService)
      .unwrap()
      .value,
    200
  );
  assert_eq!(
    maybe_resolve_from!(&container, LocalTestService, "named")
      .unwrap()
      .value,
    201
  );
  assert!(maybe_resolve_from!(&container, LocalUnregisteredService).is_none());

  // Act & Assert with resolve_from!
  assert_eq!(resolve_from!(&container, trait LocalTestTrait).value(), 202);
  assert_eq!(
    resolve_from!(&container, trait LocalTestTrait, "named_trait").value(),
    203
  );
}

#[test]
#[should_panic(expected = "Failed to resolve required service:")]
fn test_resolve_from_panics_on_missing_in_local_container() {
  let container = LocalContainer::new();
  // This should panic because the service is not in `container`.
  resolve_from!(&container, LocalUnregisteredService);
}
