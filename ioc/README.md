# Fibre IoC

[![Crates.io](https://img.shields.io/crates/v/fibre_ioc.svg)](https://crates.io/crates/fibre_ioc)
[![Docs.rs](https://docs.rs/fibre_ioc/badge.svg)](https://docs.rs/fibre_ioc)
[![License](https://img.shields.io/crates/l/fibre_ioc.svg)](https://github.com/excsn/fibre/blob/main/ioc/LICENSE)
[![CI](https://github.com/excsn/fibre/actions/workflows/ci.yml/badge.svg)](https://github.com/excsn/fibre/actions/workflows/ci.yml)

`fibre_ioc` is a flexible, thread-safe and dynamic Inversion of Control (IoC) container for Rust.

It provides a robust solution for managing dependencies within an application, promoting loose coupling and enhancing testability. Unlike containers that require a single, upfront initialization, Fibre allows for dynamic registration of services at any point during the application's lifecycle, making it ideal for complex, modular systems.

## Key Features

### Thread-Safe by Design
The container is built from the ground up for concurrency. It uses high-performance concurrent data structures (`dashmap` and `once_cell`) to ensure that registering and resolving services is completely thread-safe, without sacrificing performance.

### Multiple Service Lifetimes
Fibre supports different service lifetimes to suit various use cases:
*   **Singleton**: A single instance is created on first use and shared for all subsequent requests.
*   **Transient**: A new instance is created every time the service is requested.
*   **Instance**: A pre-existing object is registered directly with the container.

### Global & Local Containers
For convenience, a static global container is available via the `global()` function. For testing or modularity, you can create fully isolated `Container` instances with `Container::new()`, preventing any interference between dependency scopes.

### Ergonomic and Type-Safe API
Resolve dependencies effortlessly with the `resolve!` macro, which provides a concise, panicking API for retrieving services. For cases where failure is expected, the `container.get()` method returns an `Option`, allowing for graceful error handling.

### Robust Circular Dependency Detection
Fibre automatically detects and prevents circular dependencies during resolution. If Service `A` depends on `B`, and `B` depends on `A`, the container will panic with a clear error message instead of causing a stack overflow.

## Installation

Add Fibre to your project by running:
```sh
cargo add fibre_ioc
```

## Getting Started

For a detailed guide on core concepts and advanced usage patterns, please see the **[Usage Guide](README.USAGE.md)**.

The `tests/` directory in the repository also contains a comprehensive suite of examples demonstrating every feature, from basic registration to complex concurrent scenarios.

For a complete API reference, see the **[documentation on docs.rs](https://docs.rs/fibre_ioc)**.

## License

This library is distributed under the terms of the **MPL-2.0** license.
