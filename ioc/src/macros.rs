//! Public macros for ergonomic service resolution.

/// Resolves a service from the global container.
///
/// This macro is the primary way to get dependencies. It panics if the
/// requested service is not registered, ensuring that all required
/// dependencies are present at runtime.
///
/// # Panics
///
/// This macro will panic if the service cannot be resolved. For a non-panicking
/// version, use `global().get(...)` directly.
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
///
/// ```
/// use fibre_ioc::{global, resolve};
/// use std::sync::Arc;
///
/// trait Greeter { fn greet(&self) -> String; }
/// struct EnglishGreeter;
/// impl Greeter for EnglishGreeter { fn greet(&self) -> String { "Hello!".to_string() } }
///
/// // Register a trait implementation
/// global().add_singleton_trait(None, || Arc::new(EnglishGreeter));
///
/// // Resolve the trait object
/// let greeter = resolve!(trait Greeter);
/// assert_eq!(greeter.greet(), "Hello!");
/// ```
#[macro_export]
macro_rules! resolve {
    // Arm for resolving a concrete type: resolve!(MyService)
    ($type:ty) => {
        $crate::global()
            .get::<$type>(None)
            .unwrap_or_else(|| {
                panic!(
                    "Failed to resolve required service: {}",
                    std::any::type_name::<$type>()
                )
            })
    };

    // Arm for resolving a named concrete type: resolve!(MyService, "name")
    ($type:ty, $name:expr) => {
        $crate::global()
            .get::<$type>(Some($name))
            .unwrap_or_else(|| {
                panic!(
                    "Failed to resolve required service with name '{}': {}",
                    $name,
                    std::any::type_name::<$type>()
                )
            })
    };

    // CORRECTED: Arm for resolving a trait object: resolve!(trait MyTrait)
    // We use `:ident` to capture the trait's name, not `:ty`.
    (trait $trait_ident:ident) => {
        $crate::global()
            // We construct `dyn Trait` manually inside the macro expansion.
            .get::<dyn $trait_ident>(None)
            .unwrap_or_else(|| {
                panic!(
                    "Failed to resolve required trait service: {}",
                    std::any::type_name::<dyn $trait_ident>()
                )
            })
    };

    // CORRECTED: Arm for resolving a named trait object: resolve!(trait MyTrait, "name")
    (trait $trait_ident:ident, $name:expr) => {
        $crate::global()
            .get::<dyn $trait_ident>(Some($name))
            .unwrap_or_else(|| {
                panic!(
                    "Failed to resolve required trait service with name '{}': {}",
                    $name,
                    std::any::type_name::<dyn $trait_ident>()
                )
            })
    };
}