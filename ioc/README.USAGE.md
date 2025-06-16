# Usage Guide: `fibre_ioc`

This guide provides a detailed overview of the `fibre_ioc` container, its core concepts, and examples of common usage patterns.

## Table of Contents

- [Core Concepts](#core-concepts)
- [Choosing a Container: `Container` vs. `LocalContainer`](#choosing-a-container-container-vs-localcontainer)
- [Quick Start: Trait-Based Injection](#quick-start-trait-based-injection)
- [Service Registration](#service-registration)
- [Service Resolution](#service-resolution)
- [Advanced Topics](#advanced-topics)
  - [Using `LocalContainer` for `!Send` Types](#using-localcontainer-for-send-types)
  - [Isolated Containers for Testing](#isolated-containers-for-testing)
- [Error Handling](#error-handling)

## Core Concepts

*   **Container**: The central registry for all your services. It holds the "recipes" (providers) for creating service instances.
*   **Provider**: An internal enum that defines how a service is created and its lifetime. The main variants are `Singleton` (created once, then shared) and `Transient` (created new each time).
*   **Resolution**: The process of requesting an instance of a service from the container.
*   **InjectionKey**: A service is uniquely identified by a combination of its `TypeId` and an optional `String` name. This allows you to register multiple, distinct providers for the same Rust type.

## Choosing a Container: `Container` vs. `LocalContainer`

Fibre provides two distinct container types to suit different needs.

1.  **`Container` (Thread-Safe)**
    *   **Use when**: Building a typical multi-threaded application where services might be accessed from anywhere.
    *   **Sharing**: Uses `std::sync::Arc` for shared ownership.
    *   **Bounds**: Requires all registered types to be `Send + Sync`.
    *   **Access**: The `global()` function provides a convenient, application-wide instance of this container.

2.  **`LocalContainer` (Single-Threaded)**
    *   **Feature Flag**: Requires the `"local"` feature to be enabled.
    *   **Use when**: You need maximum performance in a single-threaded context (like a game loop or a single-threaded executor) or when you need to store types that are not `Send` or `Sync` (e.g., `Rc`, `RefCell`, or types from C FFI).
    *   **Sharing**: Uses `std::rc::Rc` for shared ownership.
    *   **Bounds**: Does **not** require types to be `Send` or `Sync`.
    *   **API**: Registration methods (`add_*`) take `&mut self`.

## Quick Start: Trait-Based Injection

The most powerful feature of an IoC container is decoupling components. Here is a canonical example using the thread-safe `global` container where a `ReportService` depends on a `Logger` abstraction (a trait), not a concrete type.

```rust
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
        println!("[LOG]: {}", message);
    }
}

// 3. Define a service that depends on the abstraction
struct ReportService {
    logger: Arc<dyn Logger>,
}

impl ReportService {
    fn generate_report(&self) {
        self.logger.log("Starting report generation.");
        // ... logic ...
        self.logger.log("Finished report generation.");
    }
}

fn main() {
    // 4. Register the dependencies
    global().add_singleton_trait::<dyn Logger>(|| Arc::new(ConsoleLogger));

    // The ReportService factory *resolves* its dependency from the container.
    global().add_singleton(|| ReportService {
        logger: resolve!(trait Logger),
    });

    // 5. Use the top-level service
    let service = resolve!(ReportService);
    service.generate_report();
}
```

## Service Registration

You can register services with different lifetimes. The concepts are the same for both `Container` and `LocalContainer`, but the trait bounds and method signatures differ slightly.

### Singleton

A singleton service is instantiated only once. The same instance is returned for all subsequent resolutions.

**For `Container` (thread-safe):**
```rust
// The factory is called only on the first resolution.
global().add_singleton(|| AppConfig { /* ... */ });
```

**For `LocalContainer` (single-threaded):**
```rust
# #[cfg(feature = "local")] {
use fibre_ioc::LocalContainer;
let mut container = LocalContainer::new();
container.add_singleton(|| LocalConfig { /* ... */ });
# }
```

### Transient

A transient service is instantiated every time it is resolved.

**For `Container` (thread-safe):**
```rust
// Factory is called every time `resolve!(RequestContext)` is used.
global().add_transient(|| RequestContext { /* ... */ });
```

**For `LocalContainer` (single-threaded):**
```rust
# #[cfg(feature = "local")] {
use fibre_ioc::LocalContainer;
let mut container = LocalContainer::new();
container.add_transient(|| LocalRequestContext { /* ... */ });
# }
```

### Instance (`Container` only)

You can register a pre-existing object with the thread-safe `Container`. This is effectively a pre-initialized singleton. `LocalContainer` does not have this method.

```rust
let preconfigured_service = MyService::new("special_config");
global().add_instance(preconfigured_service);
```

### Named Registrations

All registration methods have a `_with_name` variant (e.g., `add_singleton_with_name`) that accepts a string slice name. This allows you to register multiple providers for the same type.

```rust
// Register two different configurations for the same struct
global().add_instance_with_name("default_config", AppConfig { /* ... */ });
global().add_instance_with_name("test_config", AppConfig { /* ... */ });

// Later, you can resolve the specific one you need
let config = resolve!(AppConfig, "test_config");
```

## Service Resolution

### Using the `resolve!` Macro

The `resolve!` macro is the most ergonomic way to get dependencies **from the global `Container`**. It panics if the requested service is not registered. It cannot be used with a `LocalContainer`.

```rust
// Resolve an unnamed service by type
let service = resolve!(MyService);

// Resolve a named service by type and name
let named_service = resolve!(MyService, "my_name");

// Resolve an unnamed trait object
let greeter = resolve!(trait Greeter);
```

### Using the Fallible `get` Method

For cases where a service might be optional, or when using a `LocalContainer`, you must use the `get()` method. It returns an `Option`, allowing you to handle a missing registration gracefully.

**For `Container`:**
```rust
use fibre_ioc::global;
if let Some(service) = global().get::<MyService>(None) {
    // service is an Arc<MyService>
    service.do_work();
}
```

**For `LocalContainer`:**
```rust
# #[cfg(feature = "local")] {
use fibre_ioc::LocalContainer;
let mut container = LocalContainer::new();
container.add_singleton(|| MyLocalService);

if let Some(service) = container.get::<MyLocalService>(None) {
    // service is an Rc<MyLocalService>
    service.do_local_work();
}
# }
```

## Advanced Topics

### Using `LocalContainer` for `!Send` Types

The primary advantage of `LocalContainer` is its ability to manage types that don't implement `Send` or `Sync`, which is impossible in the thread-safe `Container`.

```rust
# #[cfg(feature = "local")] {
use fibre_ioc::LocalContainer;
use std::rc::Rc;
use std::cell::RefCell;

// This service contains an Rc and a RefCell, so it is !Send and !Sync.
struct UiState {
    user_name: Rc<String>,
    counter: RefCell<i32>,
}

let mut container = LocalContainer::new();

container.add_singleton(|| UiState {
    user_name: Rc::new("Alice".to_string()),
    counter: RefCell::new(0),
});

let ui_state = container.get::<UiState>(None).unwrap();

// We can now work with the non-thread-safe service
assert_eq!(*ui_state.user_name, "Alice");
*ui_state.counter.borrow_mut() += 1;
assert_eq!(*ui_state.counter.borrow(), 1);
# }
```

### Isolated Containers for Testing

While the `global()` container is convenient, it can create cross-contamination in tests. For robust testing, always create a new `Container` or `LocalContainer` for each test.

```rust
use fibre_ioc::Container;

#[test]
fn my_isolated_test() {
    let container = Container::new();

    // Register a mock service only in this local container
    container.add_instance(MockDatabase::new());

    // Pass the container to the system under test
    let service = MyService::new(&container);

    service.run_logic(); // This will use the MockDatabase

    // When `container` goes out of scope, all its services are dropped.
}
```

## Error Handling

Error handling is identical for both container types.

*   **Missing Services**: `resolve!` panics. `get()` returns `None`.
*   **Circular Dependencies**: The container automatically detects circular dependencies during resolution and will **always panic** with a "Circular dependency detected" message. This prevents infinite recursion and is not recoverable.