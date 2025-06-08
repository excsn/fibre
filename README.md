# fibre

[![crates.io](https://img.shields.io/crates/v/fibre.svg)](https://crates.io/crates/fibre)
[![docs.rs](https://docs.rs/fibre/badge.svg)](https://docs.rs/fibre)
[![License: MPL-2.0](https://img.shields.io/badge/License-MPL%202.0-brightgreen.svg)](https://opensource.org/licenses/MPL-2.0)

Fibre provides a suite of high-performance, memory-efficient sync/async channels for Rust. It is designed to offer the best possible performance for a given concurrency pattern by providing specialized channel implementations rather than a single, general-purpose one. This allows developers to solve concurrency problems with tools that are tailored for their specific needs, from blazing-fast SPSC queues to flexible MPMC channels.

## Note

`fibre` is in BETA. The API is generally stable, but minor breaking changes may occur before version 1.0 as feedback is incorporated and improvements are made.

## Key Features

### Comprehensive Channel Suite

Fibre offers a wide range of channel types, each optimized for a specific producer-consumer pattern:

*   **`spsc`**: A lock-free Single-Producer, Single-Consumer ring buffer, ideal for maximum throughput in 1-to-1 communication. Bounded. Requires `T: Send`.
*   **`mpsc`**: A lock-free Multi-Producer, Single-Consumer channel, perfect for scenarios where many tasks need to send work to a single processing task. Unbounded. Requires `T: Send`.
*   **`spmc`**: A "broadcast" style Single-Producer, Multi-Consumer channel where each message is cloned and delivered to every active consumer. Bounded. Requires `T: Send + Clone`.
*   **`mpmc`**: A flexible and robust Multi-Producer, Multi-Consumer channel for general-purpose use where producer and consumer counts are dynamic. Supports bounded (including rendezvous) and "unbounded" capacities. Requires `T: Send`.
*   **`oneshot`**: A channel for sending a single value once, perfect for futures and promise-style patterns. Requires `T: Send`.

### Hybrid Sync/Async API

A standout feature is the ability to seamlessly mix synchronous and asynchronous code. You can create a synchronous `Sender` and an asynchronous `AsyncReceiver` (or any other combination) from the same SPSC, MPSC, SPMC, or MPMC channel. This is enabled by zero-cost `to_sync()` and `to_async()` conversion methods on the channel handles, providing maximum flexibility for integrating into different codebases and runtimes.

### Consistent and Ergonomic API

One of the core design goals of `fibre` is API consistency. A developer should not have to re-learn methods for each channel type. All channels share a common set of methods with consistent semantics, allowing for predictable and ergonomic use.

**API Parity Overview**

The following tables summarize the consistent API surface across all channel senders and receivers.

**Sender API**

| Method | MPMC Sender | MPSC Sender | SPMC Sender | SPSC Sender | Oneshot Sender |
| :--- | :---: | :---: | :---: | :---: | :---: |
| `send()`/`send().await` | ✅ | ✅ | ✅ | ✅ | ✅ (Consumes self) |
| `try_send()` | ✅ | ✅ | ✅ | ✅ | ❌ (send is try) |
| `close()` | ✅ | ✅ | ✅ | ✅ | ✅ |
| `is_closed()` | ✅ | ✅ | ✅ | ✅ | ✅ |
| `len()` | ✅ | ✅ | ✅ | ✅ | N/A |
| `is_empty()` | ✅ | ✅ | ✅ | ✅ | ❌ |
| `is_full()` | ✅ | N/A | ✅ | ✅ | N/A |
| `capacity()` | ✅ | N/A | ✅ | ✅ | N/A |
| `clone()` | ✅ | ✅ | ❌ | ❌ | ✅ |

**Receiver API**

| Method | MPMC Receiver | MPSC Receiver | SPMC Receiver | SPSC Receiver | Oneshot Receiver |
| :--- | :---: | :---: | :---: | :---: | :---: |
| `recv()`/`recv().await` | ✅ | ✅ | ✅ | ✅ | ✅ |
| `try_recv()` | ✅ | ✅ | ✅ | ✅ | ✅ |
| `close()` | ✅ | ✅ | ✅ | ✅ | ✅ |
| `is_closed()` | ✅ | ✅ | ✅ | ✅ | ✅ |
| `len()` | ✅ | ✅ | ✅ | ✅ | N/A |
| `is_empty()` | ✅ | ✅ | ✅ | ✅ | ❌ |
| `is_full()` | ✅ | N/A | ✅ | ✅ | N/A |
| `capacity()` | ✅ | N/A | ✅ | ✅ | N/A |
| `clone()` | ✅ | ❌ | ✅ | ❌ | ❌ |

### Performance-Oriented Design

Performance is a primary goal. Fibre uses proven, high-performance algorithms for each channel type, including:
*   Lock-free ring buffers for SPSC.
*   Lock-free linked lists for MPSC.
*   A specialized ring buffer for SPMC that tracks individual consumer progress, ensuring backpressure from the slowest consumer.
*   High-performance `parking_lot::Mutex` for MPMC.
*   Cache-line padding on critical atomic data to minimize false sharing and maximize throughput on multi-core systems.

### Ergonomic and Safe

*   **Explicit Lifecycle Control:** All channel handles provide an idempotent `close()` method as an explicit alternative to `drop`, giving developers fine-grained control over the channel lifecycle.
*   **Clear Error Handling & Drop Safety:** Descriptive error types (`TrySendError<T>`, `RecvError`, `CloseError`) allow for value recovery and clear error reporting. Channels correctly signal disconnection when handles are dropped or explicitly closed, and any buffered items are properly deallocated.
*   **Thread Safety:** Each channel type enforces appropriate `Send` and `Sync` bounds, ensuring correct concurrent usage. For example, SPSC and MPSC consumer handles are `!Sync` as they are designed for single-threaded consumption.

## Installation

Add Fibre to your project by including it in your `Cargo.toml`:

```toml
[dependencies]
fibre = "0.3.0" # Replace with the latest version
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

This library is distributed under the terms of the **Mozilla Public License Version 2.0 (MPL-2.0)**.
You can find a copy of the license in the [LICENSE](./LICENSE) file or at [https://opensource.org/licenses/MPL-2.0](https://opensource.org/licenses/MPL-2.0).