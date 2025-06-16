//! Public macros for ergonomic service resolution.

// --- Foundational Macros ---

/// Resolves a service from a specific container instance, returning an `Option`.
///
/// This is the non-panicking, foundational macro for resolution. It allows you to
/// work with any container instance (including `Container` and `LocalContainer`).
///
/// # Returns
///
/// `Option<Arc<T>>` or `Option<Rc<T>>`, depending on the container type.
/// Returns `None` if the service is not found.
///
/// # Panics
///
/// This macro will still panic if a circular dependency is detected during resolution.
///
/// # Examples
///
/// ```
/// use fibre_ioc::{Container, maybe_resolve_from};
/// let container = Container::new();
/// container.add_instance(String::from("hello"));
///
/// // Resolve an existing service
/// let message = maybe_resolve_from!(&container, String);
/// assert_eq!(*message.unwrap(), "hello");
///
/// // Attempt to resolve a non-existent service
/// let missing = maybe_resolve_from!(&container, i32);
/// assert!(missing.is_none());
/// ```
#[macro_export]
macro_rules! maybe_resolve_from {
  // Arm for resolving a concrete type: maybe_resolve_from!(container, MyService)
  ($container:expr, $type:ty) => {
    $container.get::<$type>(None)
  };

  // Arm for resolving a named concrete type: maybe_resolve_from!(container, MyService, "name")
  ($container:expr, $type:ty, $name:expr) => {
    $container.get::<$type>(Some($name))
  };

  // Arm for resolving a trait object: maybe_resolve_from!(container, trait MyTrait)
  ($container:expr, trait $trait_ident:ident) => {
    $container.get::<dyn $trait_ident>(None)
  };

  // Arm for resolving a named trait object: maybe_resolve_from!(container, trait MyTrait, "name")
  ($container:expr, trait $trait_ident:ident, $name:expr) => {
    $container.get::<dyn $trait_ident>(Some($name))
  };
}

/// Resolves a required service from a specific container instance.
///
/// This is the panicking, foundational macro for resolution. It is intended for
/// resolving dependencies that are essential for the application's function.
///
/// # Panics
///
/// This macro will panic if the service cannot be resolved or if a circular
/// dependency is detected.
///
/// # Examples
///
/// ```
/// use fibre_ioc::{Container, resolve_from};
/// let container = Container::new();
/// container.add_instance(String::from("hello"));
///
/// // Resolve an existing service
/// let message = resolve_from!(&container, String);
/// assert_eq!(*message, "hello");
/// ```
#[macro_export]
macro_rules! resolve_from {
    // Arm for resolving a concrete type: resolve_from!(container, MyService)
    ($container:expr, $type:ty) => {
        $crate::maybe_resolve_from!($container, $type)
            .unwrap_or_else(|| {
                panic!(
                    "Failed to resolve required service: {}",
                    std::any::type_name::<$type>()
                )
            })
    };

    // Arm for resolving a named concrete type: resolve_from!(container, MyService, "name")
    ($container:expr, $type:ty, $name:expr) => {
        $crate::maybe_resolve_from!($container, $type, $name)
            .unwrap_or_else(|| {
                panic!(
                    "Failed to resolve required service with name '{}': {}",
                    $name,
                    std::any::type_name::<$type>()
                )
            })
    };

    // Arm for resolving a trait object: resolve_from!(container, trait MyTrait)
    ($container:expr, trait $trait_ident:ident) => {
        $crate::maybe_resolve_from!($container, trait $trait_ident)
            .unwrap_or_else(|| {
                panic!(
                    "Failed to resolve required trait service: {}",
                    std::any::type_name::<dyn $trait_ident>()
                )
            })
    };

    // Arm for resolving a named trait object: resolve_from!(container, trait MyTrait, "name")
    ($container:expr, trait $trait_ident:ident, $name:expr) => {
        $crate::maybe_resolve_from!($container, trait $trait_ident, $name)
            .unwrap_or_else(|| {
                panic!(
                    "Failed to resolve required trait service with name '{}': {}",
                    $name,
                    std::any::type_name::<dyn $trait_ident>()
                )
            })
    };
}

// --- Global Container Macros ---

/// Resolves an optional service from the global container.
///
/// This macro is the primary way to get optional dependencies. It returns an `Option`
/// containing the service if it is registered, or `None` if it is not.
///
/// # Returns
///
/// `Option<Arc<T>>`.
///
/// # Examples
///
/// ```
/// use fibre_ioc::{global, maybe_resolve};
///
/// global().add_instance(String::from("hello"));
///
/// // Resolve an existing service
/// let message = maybe_resolve!(String);
/// assert!(message.is_some());
///
/// // Attempt to resolve a non-existent service
/// let missing = maybe_resolve!(i32);
/// assert!(missing.is_none());
/// ```
#[macro_export]
macro_rules! maybe_resolve {
    // Delegate all patterns to maybe_resolve_from with the global container
    ($($tt:tt)*) => {
        $crate::maybe_resolve_from!($crate::global(), $($tt)*)
    };
}

/// Resolves a required service from the global container.
///
/// This macro is the primary way to get essential dependencies. It panics if the
/// requested service is not registered, ensuring that all required
/// dependencies are present at runtime.
///
/// This macro is a convenience wrapper around `resolve_from!(global(), ...)`.
///
/// # Panics
///
/// This macro will panic if the service cannot be resolved. For a non-panicking
/// version, use `maybe_resolve!(...)`.
///
/// # Examples
///
/// ```
/// use fibre_ioc::{global, resolve};
/// use std::sync::Arc;
///
/// // Register a simple type
/// global().add_singleton(None, || String::from("hello"));
///
/// // Resolve it
/// let message = resolve!(String);
/// assert_eq!(*message, "hello");
/// ```
#[macro_export]
macro_rules! resolve {
    // Delegate all patterns to resolve_from with the global container
    ($($tt:tt)*) => {
        $crate::resolve_from!($crate::global(), $($tt)*)
    };
}
