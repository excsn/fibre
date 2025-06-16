# Usage Guide: `fibre_ioc`

This guide provides a detailed overview of the `fibre_ioc` container, its core concepts, and examples of common usage patterns.

## Table of Contents

- [Core Concepts](#core-concepts)
- [Quick Start: Trait-Based Injection](#quick-start-trait-based-injection)
- [Service Registration](#service-registration)
  - [Singleton](#singleton)
  - [Transient](#transient)
  - [Instance](#instance)
  - [Named Registrations](#named-registrations)
- [Service Resolution](#service-resolution)
  - [Using the `resolve!` Macro](#using-the-resolve-macro)
  - [Using the Fallible `get` Method](#using-the-fallible-get-method)
- [Advanced Topics](#advanced-topics)
  - [Working with Traits](#working-with-traits)
  - [Isolated Containers for Testing](#isolated-containers-for-testing)
- [Error Handling](#error-handling)
  - [Missing Services](#missing-services)
  - [Circular Dependencies](#circular-dependencies)

## Core Concepts

*   **Container**: The central registry for all your services. It holds the "recipes" (providers) for creating service instances. You can use the `global()` container for application-wide dependencies or create isolated instances with `Container::new()`.

*   **Provider**: An internal enum that defines how a service is created and its lifetime. The main variants are `Singleton` (created once, then shared) and `Transient` (created new each time).

*   **Resolution**: The process of requesting an instance of a service from the container. This is typically done via the `resolve!` macro.

*   **InjectionKey**: A service is uniquely identified by a combination of its `TypeId` and an optional `String` name. This allows you to register multiple, distinct providers for the same Rust type.

## Quick Start: Trait-Based Injection

The most powerful feature of an IoC container is its ability to decouple components. Here is a canonical example where a `ReportService` depends on a `Logger` abstraction (a trait), not a concrete type.

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
    // Register ConsoleLogger as the implementation for the `dyn Logger` trait.
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

You can register services with different lifetimes. All registration methods are available on both the global container and any local `Container` instance.

### Singleton

A singleton service is instantiated only once. The same instance is returned for all subsequent resolutions. This is useful for shared resources like configuration, database connection pools, or thread-safe clients.

```rust
struct AppConfig { database_url: String }

// Factory is called only on the first resolution.
global().add_singleton(|| AppConfig {
    database_url: "postgres://...".to_string()
});
```
**Method Signatures:**
```rust
pub fn add_singleton<T: Any + Send + Sync>(
    &self,
    factory: impl Fn() -> T + Send + Sync + 'static
);
```

### Transient

A transient service is instantiated every time it is resolved. This is useful for short-lived objects that carry request-specific state, like a request handler or a unit of work.

```rust
struct RequestContext { request_id: u64 }

// Factory is called every time `resolve!(RequestContext)` is used.
global().add_transient(|| RequestContext {
    request_id: rand::random()
});
```
**Method Signatures:**
```rust
pub fn add_transient<T: Any + Send + Sync>(
    &self,
    factory: impl Fn() -> T + Send + Sync + 'static
);
```

### Instance

You can register an object that has already been created. This is effectively a pre-initialized singleton.

```rust
let preconfigured_service = MyService::new("special_config");

global().add_instance(preconfigured_service);
```
**Method Signatures:**
```rust
pub fn add_instance<T: Any + Send + Sync>(&self, instance: T);
```

### Named Registrations

All registration methods have a `_with_name` variant (e.g., `add_singleton_with_name`) that accepts a string slice name. This allows you to register multiple, distinct providers for the same type.

```rust
// Register two different configurations for the same struct
global().add_instance_with_name("default_config", AppConfig { ... });
global().add_instance_with_name("test_config", AppConfig { ... });

// Later, you can resolve the specific one you need
let config = resolve!(AppConfig, "test_config");
```

## Service Resolution

### Using the `resolve!` Macro

The `resolve!` macro is the most ergonomic way to get dependencies. It panics if the requested service is not registered, enforcing a "fail-fast" policy where missing dependencies cause an immediate crash.

```rust
// Resolve an unnamed service by type
let service = resolve!(MyService);

// Resolve a named service by type and name
let named_service = resolve!(MyService, "my_name");

// Resolve an unnamed trait object
let greeter = resolve!(trait Greeter);

// Resolve a named trait object
let german_greeter = resolve!(trait Greeter, "german");
```

### Using the Fallible `get` Method

For cases where a service might be optional, you can use the `container.get()` method directly. It returns an `Option<Arc<T>>`, allowing you to handle a missing registration gracefully.

```rust
use fibre_ioc::global;

if let Some(optional_service) = global().get::<OptionalService>(None) {
    optional_service.do_work();
} else {
    println!("Optional service not configured, skipping.");
}
```
**Method Signature:**
```rust
pub fn get<T: ?Sized + Any + Send + Sync>(&self, name: Option<&str>) -> Option<Arc<T>>;
```

## Advanced Topics

### Working with Traits

To register a service that implements a trait, use the `add_singleton_trait` or `add_transient_trait` methods. Your factory must return an `Arc<dyn YourTrait>`.

```rust
trait Notifier: Send + Sync { fn notify(&self, msg: &str); }
struct EmailNotifier;
impl Notifier for EmailNotifier { /* ... */ }

// The factory returns an Arc'd trait object.
global().add_singleton_trait::<dyn Notifier>(|| {
    Arc::new(EmailNotifier)
});

// Resolve it using the `trait` keyword in the macro.
let notifier = resolve!(trait Notifier);
notifier.notify("Hello!");
```

### Isolated Containers for Testing

While the `global()` container is great for applications, it can create cross-contamination in tests. For testing, it's best to create a new `Container` for each test or test module.

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

### Missing Services

*   **`resolve!` macro**: Panics with a "Failed to resolve required service" message.
*   **`container.get()` method**: Returns `None`.

### Circular Dependencies

The container automatically detects circular dependencies during resolution and will always panic, regardless of whether you use `resolve!` or `get()`. This prevents infinite recursion and stack overflows.

```rust
// This test will panic with a "Circular dependency detected" message.
#[test]
#[should_panic(expected = "Circular dependency detected")]
fn test_circular_dependency() {
    struct ServiceA { _b: Arc<ServiceB> }
    struct ServiceB { _a: Arc<ServiceA> }

    global().add_singleton_with_name("circular_a", || ServiceA {
        _b: resolve!(ServiceB, "circular_b"),
    });
    global().add_singleton_with_name("circular_b", || ServiceB {
        _a: resolve!(ServiceA, "circular_a"),
    });

    // Act: Resolving either service triggers the panic.
    resolve!(ServiceA, "circular_a");
}
```