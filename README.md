# fibre

[![crates.io](https://img.shields.io/crates/v/fibre.svg)](https://crates.io/crates/fibre)
[![docs.rs](https://docs.rs/fibre/badge.svg)](https://docs.rs/fibre)
[![License: MPL-2.0](https://img.shields.io/badge/License-MPL%202.0-brightgreen.svg)](https://opensource.org/licenses/MPL-2.0)

Fibre provides a suite of high-performance, memory-efficient sync/async channels for Rust. It is designed to offer the best possible performance for a given concurrency pattern by providing specialized channel implementations rather than a single, general-purpose one. This allows developers to solve concurrency problems with tools that are tailored for their specific needs, from blazing-fast SPSC queues to flexible MPMC channels.

## Note

`fibre` is in BETA.

## Key Features

### Comprehensive Channel Suite

Fibre offers a wide range of channel types, each optimized for a specific producer-consumer pattern:

*   **`spsc`**: A lock-free Single-Producer, Single-Consumer ring buffer, ideal for maximum throughput in 1-to-1 communication.
*   **`mpsc`**: A lock-free Multi-Producer, Single-Consumer channel, perfect for scenarios where many tasks need to send work to a single processing task.
*   **`spmc`**: A "broadcast" style Single-Producer, Multi-Consumer channel where each message is cloned and delivered to every active consumer.
*   **`mpmc`**: A flexible and robust Multi-Producer, Multi-Consumer channel for general-purpose use where producer and consumer counts are dynamic.
*   **`oneshot`**: A channel for sending a single value once, perfect for futures and promise-style patterns.

### Hybrid Sync/Async API

A standout feature is the ability to seamlessly mix synchronous and asynchronous code. You can create a synchronous `Sender` and an asynchronous `AsyncReceiver` (or any other combination) from the same MPSC, SPMC, or MPMC channel. This is enabled by zero-cost `to_sync()` and `to_async()` conversion methods on the channel handles, providing maximum flexibility for integrating into different codebases and runtimes.

### Performance-Oriented Design

Performance is a primary goal. Fibre uses proven, high-performance algorithms for each channel type, including lock-free data structures and cache-line padding on critical atomic data to minimize contention and maximize throughput on multi-core systems.

## Installation

Add Fibre to your project by including it in your `Cargo.toml`:

```toml
[dependencies]
fibre = "0.1.0"
```

Or by using the command line:

```sh
cargo add fibre
```

There are no system prerequisites other than a standard Rust toolchain.

## Getting Started

For a detailed guide, API overview, and code examples, please see the **[Usage Guide](./README.USAGE.md)**.

The full API reference is available on **[docs.rs](https://docs.rs/fibre)**.

## License

This library is distributed under the terms of both the MIT License and the Apache License (Version 2.0).

