# API Reference: `fibre_ioc`

This document provides a detailed reference for the public API of the `fibre_ioc` library.

## Introduction

`fibre_ioc` is an Inversion of Control (IoC) container for Rust. Its primary purpose is to manage the lifecycle and dependencies of services within an application.

The library provides two main container types:
1.  **`Container`**: A thread-safe container suitable for most application-wide use cases. It shares services using `std::sync::Arc`.
2.  **`LocalContainer`**: A single-threaded container for performance-critical or `!Send`/`!Sync` scenarios. It shares services using `std::rc::Rc`.

Interaction with the library is primarily done through an instance of a container or the singleton `Container` instance provided by the `global()` function. Resolution of services is made ergonomic through a family of macros for both required (`resolve!`, `resolve_from!`) and optional (`maybe_resolve!`, `maybe_resolve_from!`) dependencies.

## Core Types

### struct `Container`

The `Container` is the main thread-safe IoC container that holds all service registrations. It is designed for concurrency and allows simultaneous registration and resolution of services from multiple threads.

All types registered with this container must have `'static` lifetimes and satisfy the `Send + Sync` trait bounds.

```rust
#[derive(Default)]
pub struct Container { /* private fields */ }
```

#### `impl Container`

##### **Constructors**

**`new`**

Creates a new, empty `Container`. This is useful for creating isolated, thread-safe scopes.

```rust
pub fn new() -> Self;
```

##### **Instance Registration**

**`add_instance`**

Registers an unnamed, pre-existing object instance. The instance will be treated as a singleton.

```rust
pub fn add_instance<T: Any + Send + Sync>(&self, instance: T);
```

**`add_instance_with_name`**

Registers a named, pre-existing object instance.

```rust
pub fn add_instance_with_name<T: Any + Send + Sync>(&self, name: &str, instance: T);
```

##### **Singleton Registration**

**`add_singleton`**

Registers an unnamed singleton factory. The factory is called only once, and the resulting instance is shared for all subsequent resolutions.

```rust
pub fn add_singleton<T: Any + Send + Sync>(
    &self,
    factory: impl Fn() -> T + Send + Sync + 'static,
);
```

**`add_singleton_with_name`**

Registers a named singleton factory.

```rust
pub fn add_singleton_with_name<T: Any + Send + Sync>(
    &self,
    name: &str,
    factory: impl Fn() -> T + Send + Sync + 'static,
);
```

##### **Transient Registration**

**`add_transient`**

Registers an unnamed transient factory. The factory is called every time the service is resolved.

```rust
pub fn add_transient<T: Any + Send + Sync>(
    &self,
    factory: impl Fn() -> T + Send + Sync + 'static,
);
```

**`add_transient_with_name`**

Registers a named transient factory.

```rust
pub fn add_transient_with_name<T: Any + Send + Sync>(
    &self,
    name: &str,
    factory: impl Fn() -> T + Send + Sync + 'static,
);
```

##### **Trait Registration**

**`add_singleton_trait`**

Registers an unnamed singleton factory for a trait object. The factory must return an `Arc<I>`.

```rust
pub fn add_singleton_trait<I: ?Sized + Any + Send + Sync>(
    &self,
    factory: impl Fn() -> Arc<I> + Send + Sync + 'static,
);
```

**`add_singleton_trait_with_name`**

Registers a named singleton factory for a trait object.

```rust
pub fn add_singleton_trait_with_name<I: ?Sized + Any + Send + Sync>(
    &self,
    name: &str,
    factory: impl Fn() -> Arc<I> + Send + Sync + 'static,
);
```

##### **Resolution**

**`get`**

Resolves a service from the container, returning `Option<Arc<T>>`. This is the foundational method upon which the resolution macros are built.

```rust
pub fn get<T: ?Sized + Any + Send + Sync>(&self, name: Option<&str>) -> Option<Arc<T>>;
```

---

### struct `LocalContainer`

<small>Available with the `"local"` feature flag.</small>

A single-threaded, non-thread-safe Inversion of Control container. It is more performant than the thread-safe `Container` for single-threaded scenarios and has the key advantage of being able to store types that are **not** `Send` or `Sync`.

Registration methods on `LocalContainer` require a mutable reference (`&mut self`).

```rust
#[cfg(feature = "local")]
#[derive(Default)]
pub struct LocalContainer { /* private fields */ }
```

#### `impl LocalContainer`

##### **Constructors**

**`new`**

Creates a new, empty `LocalContainer`.

```rust
#[cfg(feature = "local")]
pub fn new() -> Self;
```

##### **Singleton Registration**

**`add_singleton`**

Registers an unnamed singleton factory.

```rust
#[cfg(feature = "local")]
pub fn add_singleton<T: Any + 'static>(&mut self, factory: impl Fn() -> T + 'static);
```

**`add_singleton_with_name`**

Registers a named singleton factory.

```rust
#[cfg(feature = "local")]
pub fn add_singleton_with_name<T: Any + 'static>(
    &mut self,
    name: &str,
    factory: impl Fn() -> T + 'static,
);
```

##### **Transient Registration**

**`add_transient`**

Registers an unnamed transient factory.

```rust
#[cfg(feature = "local")]
pub fn add_transient<T: Any + 'static>(&mut self, factory: impl Fn() -> T + 'static);
```

**`add_transient_with_name`**

Registers a named transient factory.

```rust
#[cfg(feature = "local")]
pub fn add_transient_with_name<T: Any + 'static>(
    &mut self,
    name: &str,
    factory: impl Fn() -> T + 'static,
);
```

##### **Trait Registration**

**`add_singleton_trait`**

Registers an unnamed singleton factory for a trait object. The factory must return an `Rc<I>`.

```rust
#[cfg(feature = "local")]
pub fn add_singleton_trait<I: ?Sized + Any + 'static>(
    &mut self,
    factory: impl Fn() -> Rc<I> + 'static,
);
```

**`add_singleton_trait_with_name`**

Registers a named singleton factory for a trait object.

```rust
#[cfg(feature = "local")]
pub fn add_singleton_trait_with_name<I: ?Sized + Any + 'static>(
    &mut self,
    name: &str,
    factory: impl Fn() -> Rc<I> + 'static,
);
```

##### **Resolution**

**`get`**

Resolves a service from the container, returning `Option<Rc<T>>`.

```rust
#[cfg(feature = "local")]
pub fn get<T: ?Sized + Any + 'static>(&self, name: Option<&str>) -> Option<Rc<T>>;
```

---

## Macros

The library provides a family of macros for ergonomic service resolution. They are split into two categories: those that work on the global container and those that work on a specific container instance.

### Global Container Macros

These macros are convenient shortcuts that always operate on the `Container` instance returned by `global()`.

#### `resolve!`

Resolves a **required** service from the global container. This macro panics if the service is not found.

**Forms**
*   `resolve!(MyType)`
*   `resolve!(MyType, "my_name")`
*   `resolve!(trait MyTrait)`
*   `resolve!(trait MyTrait, "my_name")`

#### `maybe_resolve!`

Resolves an **optional** service from the global container. This macro returns an `Option` (`None` if the service is not found).

**Forms**
*   `maybe_resolve!(MyType)` -> `Option<Arc<MyType>>`
*   `maybe_resolve!(MyType, "my_name")`
*   `maybe_resolve!(trait MyTrait)` -> `Option<Arc<dyn MyTrait>>`
*   `maybe_resolve!(trait MyTrait, "my_name")`

### Container-Specific Macros

These foundational macros allow you to resolve services from any container instance, including `Container` and `LocalContainer`. This is especially useful for testing and managing isolated scopes.

#### `resolve_from!`

Resolves a **required** service from a specific container instance. This macro panics if the service is not found.

**Forms**
*   `resolve_from!(my_container, MyType)`
*   `resolve_from!(my_container, MyType, "my_name")`
*   `resolve_from!(my_container, trait MyTrait)`
*   `resolve_from!(my_container, trait MyTrait, "my_name")`

#### `maybe_resolve_from!`

Resolves an **optional** service from a specific container instance. This macro returns an `Option`.

**Forms**
*   `maybe_resolve_from!(my_container, MyType)`
*   `maybe_resolve_from!(my_container, MyType, "my_name")`
*   `maybe_resolve_from!(my_container, trait MyTrait)`
*   `maybe_resolve_from!(my_container, trait MyTrait, "my_name")`

## Public Functions

### `global`

Returns a static reference to the one and only global thread-safe `Container` instance. This instance is lazily created on its first access.

```rust
pub fn global() -> &'static Container;
```

## Error Handling

The library uses two main strategies for handling resolution failures in **both** container types:

1.  **Panicking (for required dependencies)**: This is the behavior when using the `resolve!` and `resolve_from!` macros. It is designed for "fail-fast" scenarios where a dependency is essential for the application to run.
    *   **Missing Service**: Panics with a message like `"Failed to resolve required service: ..."`.
    *   **Circular Dependency**: Panics with a message like `"Circular dependency detected while resolving service: ..."`.

2.  **Fallible `Option` (for optional dependencies)**: The `maybe_resolve!` and `maybe_resolve_from!` macros are the primary tools for this pattern. They return `None` if a service is not found, allowing for graceful handling. These macros are built upon the underlying `container.get()` methods, which provide the same fallible behavior.

Note that a circular dependency will *always* cause a panic, even when using non-panicking resolution macros or methods.