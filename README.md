# Fibre

[![License: MPL-2.0](https://img.shields.io/badge/License-MPL%202.0-brightgreen.svg)](https://opensource.org/licenses/MPL-2.0)
[![Crates.io](https://img.shields.io/crates/v/fibre?label=fibre)](https://crates.io/crates/fibre)
[![Crates.io](https://img.shields.io/crates/v/fibre_cache?label=fibre_cache)](https://crates.io/crates/fibre_cache)

`fibre` is a workspace of high-performance, concurrent building blocks for Rust, designed to enable the development of best in class low-latency, low-overhead applications.

## Core Philosophy

*   **Performance-Oriented Design**: The libraries are designed from the ground up for speed and low overhead, using techniques like lock-free data structures and per-shard locking.
*   **Seamless Sync/Async Integration**: First-class support for both synchronous (`std::thread`) and asynchronous (`async/await`) Rust allows components to be integrated into any application architecture.
*   **Specialized, Not Generalized**: The project's philosophy is that the best performance comes from using the right tool for the job. We provide specialized implementations for specific concurrency patterns.
*   **Ergonomic and Safe API**: While performance is critical, the APIs are designed to be consistent, predictable, and safe, leveraging Rust's type system to prevent common concurrency errors.

## Crates in this Workspace

*   **[`fibre`](./channels/README.md)**: A library of specialized, high-performance, and memory-efficient sync/async channels (`spsc`, `mpsc`, `spmc`, `mpmc`).
*   **[`fibre_cache`](./cache/README.md)**: A comprehensive, flexible, and high-performance concurrent caching library with modern eviction policies.

## Documentation

Detailed documentation and usage guides can be found within each crate's directory:

*   **`fibre` (Channels)**: [Usage Guide](./channels/README.GUIDE.md) | [API Reference (docs.rs)](https://docs.rs/fibre)
*   **`fibre_cache` (Cache)**: [Usage Guide](./cache/README.GUIDE.md) | [API Reference (docs.rs)](https://docs.rs/fibre_cache)

## License

This project is distributed under the terms of the **Mozilla Public License Version 2.0 (MPL-2.0)**. See [LICENSE](./LICENSE) for details.