# fibre

[![crates.io](https://img.shields.io/crates/v/fibre.svg)](https://crates.io/crates/fibre)
[![docs.rs](https://docs.rs/fibre/badge.svg)](https://docs.rs/fibre)
[![License: MPL-2.0](https://img.shields.io/badge/License-MPL%202.0-brightgreen.svg)](https://opensource.org/licenses/MPL-2.0)

`fibre` provides a suite of high-performance, memory-efficient sync/async channels for Rust. It is designed to offer the best possible performance for a given concurrency pattern by providing specialized channel implementations rather than a single, general-purpose one. This allows developers to solve concurrency problems with tools that are tailored for their specific needs, from blazing-fast SPSC queues to flexible MPMC channels.

## Current Status: Stable

`fibre` is stable. The API is stable, but minor breaking changes may occur before version 1.0 as feedback is incorporated and improvements are made.

## Notable Users

[Hi Stakes Markets Game](https://www.histakesgame.com) -  The worlds most advanced financial simulator, available on iPhone and Android.

## Key Features

### Comprehensive Channel Suite

Fibre offers a wide range of channel types, each optimized for a specific producer-consumer pattern:

*   **`spsc`**: A lock-free Single-Producer, Single-Consumer ring buffer, ideal for maximum throughput in 1-to-1 communication. Bounded. Requires `T: Send`.
*   **`mpsc`**: A lock-free Multi-Producer, Single-Consumer channel, perfect for scenarios where many tasks need to send work to a single processing task. Supports both bounded and unbounded modes. Requires `T: Send`.
*   **`spmc`**: A "broadcast" style Single-Producer, Multi-Consumer channel where each message is cloned and delivered to every active consumer. Bounded. Requires `T: Send + Clone`.
*   **`spmc::topic`**: A "publish-subscribe" variant of SPMC where the producer sends messages to named topics, and consumers subscribe to the topics they're interested in. The sender is non-blocking, dropping messages for slow consumers. Requires `K: Send + Sync + Hash + Eq + Clone` and `T: Send + Clone`.
*   **`mpmc`**: A flexible and robust Multi-Producer, Multi-Consumer channel for general-purpose use where producer and consumer counts are dynamic. Supports bounded (including rendezvous) and "unbounded" capacities. Requires `T: Send`.
*   **`oneshot`**: A channel for sending a single value once, perfect for futures and promise-style patterns. Requires `T: Send`.

### Hybrid Sync/Async API

A standout feature is the ability to seamlessly mix synchronous and asynchronous code. You can create a synchronous `Sender` and an asynchronous `AsyncReceiver` (or any other combination) from the same SPSC, MPSC, SPMC, or MPMC channel. This is enabled by zero-cost `to_sync()` and `to_async()` conversion methods on the channel handles, providing maximum flexibility for integrating into different codebases and runtimes.

### Consistent and Ergonomic API

One of the core design goals of `fibre` is API consistency. A developer should not have to re-learn methods for each channel type. All channels share a common set of methods with consistent semantics, allowing for predictable and ergonomic use.

**API Parity Overview**

The following tables summarize the consistent API surface across all channel senders and receivers.

**Sender API**

| Method | MPMC Sender | MPSC Sender (B/U) | SPMC Sender | SPMC Topic Sender | SPSC Sender | Oneshot Sender |
| :--- | :---: | :---: | :---: | :---: | :---: | :---: |
| `send()`/`send().await` | ✅ | ✅ | ✅ | ✅ (non-blocking) | ✅ | ✅ (Consumes self) |
| `try_send()` | ✅ | ✅ | ✅ | N/A | ✅ | ✅ (send is try) |
| `close()` | ✅ | ✅ | ✅ (`&mut self`) | ✅ | ✅ | ✅ |
| `is_closed()` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `len()` | ✅ | ✅ | ✅ | ❌ | ✅ | N/A |
| `is_empty()` | ✅ | ✅ | ✅ | ❌ | ✅ | ❌ |
| `is_full()` | ✅ | ✅ (bounded) | ✅ | ❌ | ✅ | N/A |
| `capacity()` | ✅ | ✅ (bounded) | ✅ | ❌ | ✅ | N/A |
| `clone()` | ✅ | ✅ | ❌ | ✅ | ❌ | ✅ |
| `send_batch()` | ✅ | ✅ | ✅ | ❌ | ✅ | N/A |
| `try_send_batch()` | ✅ | ✅ | ✅ | ❌ | ✅ | N/A |
| `send_batch_mut()` | ✅ | ✅ | ✅ | ❌ | ✅ | N/A |
| `try_send_batch_mut()` | ✅ | ✅ | ✅ | ❌ | ✅ | N/A |

**Receiver API**

| Method | MPMC Receiver | MPSC Receiver (B/U) | SPMC Receiver | SPMC Topic Receiver | SPSC Receiver | Oneshot Receiver |
| :--- | :---: | :---: | :---: | :---: | :---: | :---: |
| `recv()`/`recv().await` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `try_recv()` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `recv_timeout()` | ✅ | ✅ | ✅ | ✅ | ✅ (`&mut self`) | N/A |
| `subscribe/unsubscribe`| N/A | N/A | N/A | ✅ | N/A | N/A |
| `close()` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `is_closed()` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `len()` | ✅ | ✅ | ✅ | ❌ | ✅ | N/A |
| `is_empty()` | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ |
| `is_full()` | ✅ | ✅ (bounded) | ✅ | ❌ | ✅ | N/A |
| `capacity()` | ✅ | ✅ (bounded) | ✅ | ✅ | ✅ | N/A |
| `clone()` | ✅ | ❌ | ✅ | ✅ | ❌ | ❌ |
| `recv_batch()` | ✅ | ✅ | ✅ | ❌ | ✅ | N/A |
| `try_recv_batch()` | ✅ | ✅ | ✅ | ❌ | ✅ | N/A |
| `recv_batch_mut()` | ✅ | ✅ | ✅ | ❌ | ✅ | N/A |
| `try_recv_batch_mut()` | ✅ | ✅ | ✅ | ❌ | ✅ | N/A |

### Batch Operations

Every core channel (SPSC, MPSC, SPMC, MPMC) offers a full batch API suite — `send_batch`, `try_send_batch`, `recv_batch`, `try_recv_batch` plus in-place `_mut` variants — that amortizes synchronization over the whole batch: a single atomic index update or lock acquisition, bulk capacity-permit handling, and coalesced wakeups. Batch operations have partial-progress semantics: by-value sends return the count sent and the unsent remainder on interruption (no owned value is ever silently dropped), and in-place variants drain sent items from the front of the caller's `Vec` (send) or append to it (receive). Blocking/async `recv_batch(max)` waits until at least one item is available, then drains up to `max` without further waiting.

### Performance-Oriented Design

Performance is a primary goal. Fibre uses proven, high-performance algorithms for each channel type, including:
*   Lock-free ring buffers for SPSC.
*   Lock-free linked lists for MPSC.
*   A specialized ring buffer for SPMC that tracks individual consumer progress, ensuring backpressure from the slowest consumer.
*   A non-blocking, topic-based SPMC variant built on a concurrent hash map, copy-on-write lists, and individual mailboxes for high-performance pub/sub.
*   A fair, hybrid semaphore built on `parking_lot::Mutex` for MPMC and bounded MPSC channels.
*   Cache-line padding on critical atomic data to minimize false sharing and maximize throughput on multi-core systems.

### Ergonomic and Safe

*   **Explicit Lifecycle Control:** All channel handles provide an idempotent `close()` method as an explicit alternative to `drop`, giving developers fine-grained control over the channel lifecycle.
*   **Clear Error Handling & Drop Safety:** Descriptive error types (`TrySendError<T>`, `RecvError`, `CloseError`) allow for value recovery and clear error reporting. Channels correctly signal disconnection when handles are dropped or explicitly closed, and any buffered items are properly deallocated.
*   **Defined Disconnect Semantics:** Fibre follows a consistent two-sided disconnection contract across all channel types. When all **senders** are dropped or closed, any items already buffered in the channel remain readable; receivers will drain them before observing `Disconnected`. When a **receiver** is dropped or explicitly closed, that handle immediately returns `Disconnected` on any subsequent receive call — it does not attempt to drain remaining buffered items. This distinction is important for graceful shutdown: drop your senders to let receivers drain cleanly, rather than closing receivers while items are still in flight.
*   **Thread Safety:** Each channel type enforces appropriate `Send` and `Sync` bounds, ensuring correct concurrent usage. For example, SPSC and MPSC consumer handles are `!Sync` as they are designed for single-threaded consumption.

## Installation

Add Fibre to your project by including it in your `Cargo.toml`:

```toml
[dependencies]
fibre = "0.5.0" # Replace with the latest version
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