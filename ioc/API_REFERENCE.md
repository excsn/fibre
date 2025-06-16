# API Reference: `fibre_ioc`

This document provides a detailed reference for the public API of the `fibre_ioc` library.

## Introduction

`fibre_ioc` is an Inversion of Control (IoC) container for Rust. Its primary purpose is to manage the lifecycle and dependencies of services within an application.

The central component of the library is the `Container`, which acts as a registry for services. Interaction with the library is primarily done through an instance of `Container` or the singleton instance provided by the `global()` function.

A key pattern throughout the library is that all services are resolved wrapped in a `std::sync::Arc`, enabling safe sharing across threads. All registered types must satisfy the `Any + Send + Sync` trait bounds.

## Core Types

### struct `Container`

The `Container` is the main IoC container that holds all service registrations. It is thread-safe and allows for concurrent registration and resolution of services.

```rust
#[derive(Default)]
pub struct Container { /* private fields */ }
```

#### `impl Container`

##### **Constructors**

**`new`**

Creates a new, empty `Container`. This is useful for creating isolated scopes, such as in testing.

```rust
pub fn new() -> Self;
```

##### **Instance Registration**

These methods register a pre-existing object instance with the container. This object will be treated as a singleton.

**`add_instance`**

Registers an unnamed instance of a type `T`.

```rust
pub fn add_instance<T: Any + Send + Sync>(&self, instance: T);
```

**`add_instance_with_name`**

Registers a named instance of a type `T`.

```rust
pub fn add_instance_with_name<T: Any + Send + Sync>(&self, name: &str, instance: T);
```

##### **Singleton Registration**

These methods register a factory function that will be called exactly once to create a singleton instance of a service. The resulting instance is then shared for all subsequent resolutions.

**`add_singleton`**

Registers an unnamed singleton factory for a type `T`.

```rust
pub fn add_singleton<T: Any + Send + Sync>(
    &self,
    factory: impl Fn() -> T + Send + Sync + 'static,
);
```

**`add_singleton_with_name`**

Registers a named singleton factory for a type `T`.

```rust
pub fn add_singleton_with_name<T: Any + Send + Sync>(
    &self,
    name: &str,
    factory: impl Fn() -> T + Send + Sync + 'static,
);
```

##### **Transient Registration**

These methods register a factory function that will be called every time a service is resolved, creating a new instance on each request.

**`add_transient`**

Registers an unnamed transient factory for a type `T`.

```rust
pub fn add_transient<T: Any + Send + Sync>(
    &self,
    factory: impl Fn() -> T + Send + Sync + 'static,
);
```

**`add_transient_with_name`**

Registers a named transient factory for a type `T`.

```rust
pub fn add_transient_with_name<T: Any + Send + Sync>(
    &self,
    name: &str,
    factory: impl Fn() -> T + Send + Sync + 'static,
);
```

##### **Trait Registration**

These methods are specifically for registering services against a trait object (`dyn Trait`). The factory must return an `Arc<I>` where `I` is the trait object type. Currently, only singleton lifetimes are supported for traits.

**`add_singleton_trait`**

Registers an unnamed singleton factory for a trait object `I`.

```rust
pub fn add_singleton_trait<I: ?Sized + Any + Send + Sync>(
    &self,
    factory: impl Fn() -> Arc<I> + Send + Sync + 'static,
);
```

**`add_singleton_trait_with_name`**

Registers a named singleton factory for a trait object `I`.

```rust
pub fn add_singleton_trait_with_name<I: ?Sized + Any + Send + Sync>(
    &self,
    name: &str,
    factory: impl Fn() -> Arc<I> + Send + Sync + 'static,
);
```

##### **Resolution**

**`get`**

Resolves a service of type `T` from the container, returning an `Option`. This method does not panic if the service is not found.

*   **`T`**: The type of the service to resolve. For trait objects, use `dyn MyTrait`.
*   **`name`**: An optional string slice to resolve a named service.

```rust
pub fn get<T: ?Sized + Any + Send + Sync>(&self, name: Option<&str>) -> Option<Arc<T>>;
```

## Macros

### `resolve!`

A macro for ergonomic, panicking resolution of services from the global container. It is the primary way to retrieve dependencies in an application.

#### **Forms**

*   **Resolve unnamed concrete type:** `resolve!(MyType)`
*   **Resolve named concrete type:** `resolve!(MyType, "my_name")`
*   **Resolve unnamed trait object:** `resolve!(trait MyTrait)`
*   **Resolve named trait object:** `resolve!(trait MyTrait, "my_name")`

## Public Functions

### `global`

Returns a static reference to the one and only global `Container` instance. This instance is lazily created on its first access in a thread-safe manner.

```rust
pub fn global() -> &'static Container;
```

## Error Handling

The library uses two main strategies for handling resolution failures:

1.  **Panicking**: This is the default behavior when using the `resolve!` macro. It is designed for "fail-fast" scenarios where a dependency is considered essential for the application to run.
    *   **Missing Service**: Panics with a message like `"Failed to resolve required service: ..."`.
    *   **Circular Dependency**: Panics with a message like `"Circular dependency detected while resolving service: ..."`. This check is always active, even for the non-panicking `get` method.

2.  **Fallible `Option`**: The `container.get()` method returns `Option<Arc<T>>`. It will return `None` if a service is not found, allowing for graceful handling of optional dependencies. Note that it will still panic in the case of a circular dependency.