//! The global IoC container instance and access functions.

use crate::container::Container;
use once_cell::sync::Lazy;

// The one and only global container instance.
// It will be created on its first access in a thread-safe manner.
static GLOBAL_CONTAINER: Lazy<Container> = Lazy::new(Container::default);

/// Provides a reference to the global container instance.
///
/// This function allows for direct interaction with the container, such as
/// registering services from anywhere in an application.
///
/// # Examples
///
/// ```
/// use fibre_ioc::global;
///
/// fn register_services() {
///   // Get the global container and register a service.
///   global().add_instance(String::from("Hello from global!"));
/// }
/// ```
pub fn global() -> &'static Container {
  &GLOBAL_CONTAINER
}