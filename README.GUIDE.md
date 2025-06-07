# Usage Guide: `fibre`

This guide provides detailed examples and an overview of the core concepts and APIs in the `fibre` library.

### Table of Contents

*   [Core Concepts](#core-concepts)
*   [Quick Start Examples](#quick-start-examples)
    *   [MPMC Sync Example](#mpmc-sync-example)
    *   [MPSC Async Stream Example](#mpsc-async-stream-example)
    *   [SPSC Hybrid Example with Lifecycle Control](#spsc-hybrid-example-with-lifecycle-control)
*   [Common API Across Channels](#common-api-across-channels)
*   [API by Channel Type](#api-by-channel-type)
    *   [Module: `fibre::mpmc`](#module-fibrempmc)
    *   [Module: `fibre::mpsc`](#module-fibrempsc)
    *   [Module: `fibre::spmc`](#module-fibrespmc)
    *   [Module: `fibre::spsc`](#module-fibrespsc)
    *   [Module: `fibre::oneshot`](#module-fibreoneshot)
*   [Error Handling](#error-handling)

## Core Concepts

### Specialized Channels

Fibre's main philosophy is to provide the right tool for the job. Instead of using a general-purpose MPMC channel for all tasks, you can choose a specialized implementation that is algorithmically optimized for your use case, leading to better performance and lower overhead.

*   **SPSC (Single-Producer, Single-Receiver):** The fastest pattern for 1-to-1 communication. Implemented with a lock-free ring buffer. It's bounded and requires `T: Send`.
*   **MPSC (Multi-Producer, Single-Receiver):** Many threads/tasks send to one receiver. Great for collecting results or distributing work to a single processor. Implemented with a lock-free linked list and is unbounded. Requires `T: Send`.
*   **SPMC (Single-Producer, Multi-Receiver):** One thread/task broadcasts the same message to many receivers. Each consumer gets a clone of the message. Implemented with a specialized ring buffer that tracks individual consumer progress. It's bounded and requires `T: Send + Clone`.
*   **MPMC (Multi-Producer, Multi-Receiver):** The most flexible pattern, allowing many-to-many communication. Implemented with a `parking_lot::Mutex` for robust state management and support for mixed sync/async waiters. Supports bounded (including rendezvous) and "unbounded" (memory-limited) capacities. Requires `T: Send`.
*   **Oneshot:** A channel for sending a single value, once, from one of potentially many senders to a single receiver. Requires `T: Send`.

### Hybrid Sync/Async Model

The `mpmc`, `mpsc`, `spmc`, and `spsc` channels support full interoperability between synchronous and asynchronous code. Every sender and receiver handle has a `to_sync()` or `to_async()` method that performs a zero-cost conversion. This allows you to, for example, have producers running in standard OS threads (`std::thread`) sending data to a consumer running in a Tokio task, all on the same channel.

### Sender/Receiver Handles & The `Stream` Trait

All channels are interacted with via sender and receiver handles (e.g., `Sender`, `Receiver`, `AsyncSender`, `AsyncReceiver`). These handles control access to the channel and manage its lifecycle. When all senders for a channel are dropped or closed, the channel becomes "disconnected" from the perspective of the receiver. When all receivers are dropped or closed, the channel becomes "closed" from the perspective of the sender.

**A key feature of all asynchronous receiver types (`AsyncReceiver`, etc.) is that they implement the `futures::Stream` trait.** This allows them to be used with the rich set of combinators provided by `futures::StreamExt`, such as `next()`, `map()`, `filter()`, `for_each`, and `collect()`.

## Quick Start Examples

### MPMC Sync Example

A simple many-to-many example using standard threads.

```rust
use fibre::mpmc;
use std::thread;

fn main() {
    let (tx, rx) = mpmc::bounded(4); // Bounded channel with capacity 4
    let num_producers = 2;
    let items_per_producer = 5;
    let mut handles = Vec::new();

    // Spawn producers
    for i in 0..num_producers {
        let tx_clone = tx.clone();
        handles.push(thread::spawn(move || {
            for j in 0..items_per_producer {
                let msg = format!("Sender {} says: {}", i, j);
                if tx_clone.send(msg).is_err() {
                    eprintln!("Sender {}: Receiver dropped!", i);
                    break;
                }
            }
        }));
    }
    drop(tx); // Drop the original sender

    // Spawn consumers (or receive in main thread)
    let mut consumer_handles = Vec::new();
    let num_consumers = 2; // Example with multiple consumers
    for c_id in 0..num_consumers {
        let rx_clone = rx.clone();
        consumer_handles.push(thread::spawn(move || {
            loop {
                match rx_clone.recv() {
                    Ok(received) => println!("Receiver {}: Got: {}", c_id, received),
                    Err(_) => { // Disconnected
                        println!("Receiver {}: Channel disconnected.", c_id);
                        break;
                    }
                }
            }
        }));
    }
    drop(rx); // Drop original receiver

    for handle in handles { // Sender handles
        handle.join().unwrap();
    }
    for handle in consumer_handles {
        handle.join().unwrap();
    }
}
```

### MPSC Async Stream Example

An example with multiple asynchronous producers sending to a single consumer task, which processes items using the `Stream` API.

```rust
use fibre::mpsc;
use tokio::task;
use futures_util::StreamExt; // Import the StreamExt trait for .next()

#[tokio::main]
async fn main() {
    let (tx, mut rx) = mpsc::unbounded_async(); // MPSC is unbounded
    let num_producers = 3;
    let items_per_producer = 5;

    // Spawn async producers
    for i in 0..num_producers {
        let tx_clone = tx.clone();
        task::spawn(async move {
            for j in 0..items_per_producer {
                if tx_clone.send((i, j)).await.is_err() {
                    eprintln!("Async Sender {}: Receiver dropped!", i);
                    break;
                }
            }
        });
    }
    drop(tx); // Signal to the consumer that all producers are potentially done after their work

    // The single consumer receives all the work using a while-let loop on the stream
    let mut total = 0;
    while let Some(msg) = rx.next().await {
        println!("Received: {:?}", msg);
        total += 1;
    }
    assert_eq!(total, num_producers * items_per_producer);
    println!("MPSC async stream example finished.");
}
```

### SPSC Hybrid Example with Lifecycle Control

An example demonstrating a synchronous producer sending to an asynchronous consumer, using the new `close()` method for explicit lifecycle management.

```rust
use fibre::spsc;
use std::thread;
use tokio::runtime::Runtime;

fn main() {
    let rt = Runtime::new().unwrap();
    rt.block_on(async {
        // Start with an async channel, then convert the producer to sync
        let (p_async, mut c_async) = spsc::bounded_async::<String>(2);
        let mut p_sync = p_async.to_sync(); // Convert producer to sync

        let producer_thread = thread::spawn(move || {
            p_sync.send("Hello from sync thread!".to_string()).unwrap();
            p_sync.send("Goodbye from sync thread!".to_string()).unwrap();
            
            // Explicitly close the channel instead of relying on drop
            p_sync.close().unwrap();
            println!("Producer closed the channel.");
        });

        println!("Async consumer received: {}", c_async.recv().await.unwrap());
        println!("Async consumer received: {}", c_async.recv().await.unwrap());
        
        // After receiving all messages, the next recv sees the disconnect signal
        assert!(matches!(c_async.recv().await, Err(fibre::error::RecvError::Disconnected)));
        println!("Consumer confirmed channel is disconnected.");

        producer_thread.join().unwrap();
    });
}
```

## Common API Across Channels

All channel senders and receivers share a consistent set of methods for predictable lifecycle management and inspection.

*   `len(&self) -> usize`: Returns the number of items in the channel. For senders, this may be an approximation based on the slowest consumer.
*   `is_empty(&self) -> bool`: Returns `true` if the channel's buffer is empty.
*   `is_full(&self) -> bool`: (Bounded channels only) Returns `true` if the channel is at capacity.
*   `capacity(&self) -> usize`: (Bounded channels only) Returns the channel's capacity.
*   `is_closed(&self) -> bool`: Returns `true` if the other side of the channel has been dropped/closed.
*   `close(&self) -> Result<(), CloseError>`: Explicitly closes this handle, contributing to the channel's overall disconnected/closed state. This is an idempotent alternative to `drop`.

## API by Channel Type

_(Note: `T` must generally be `Send`. Specific trait bounds like `Clone` are noted.)_

### Module: `fibre::mpmc`

A flexible channel for many-to-many communication.

*   **Constructors:**
    *   `pub fn bounded<T: Send>(capacity: usize) -> (Sender<T>, Receiver<T>)`
    *   `pub fn bounded_async<T: Send>(capacity: usize) -> (AsyncSender<T>, AsyncReceiver<T>)`
    *   `pub fn unbounded<T: Send>() -> (Sender<T>, Receiver<T>)`
    *   `pub fn unbounded_async<T: Send>() -> (AsyncSender<T>, AsyncReceiver<T>)`
*   **Handles:**
    *   `Sender<T: Send>` (`Clone`) and `Receiver<T: Send>` (`Clone`).
    *   `AsyncSender<T: Send>` (`Clone`) and `AsyncReceiver<T: Send>` (`Clone`). `AsyncReceiver` implements `futures::Stream`.
*   **Key Methods:** `send`, `try_send`, `recv`, `try_recv`, `close`, `is_closed`, `capacity`, and conversions.

### Module: `fibre::mpsc`

An optimized lock-free channel for many-to-one communication (unbounded).

*   **Constructors:**
    *   `pub fn unbounded<T: Send>() -> (Sender<T>, Receiver<T>)`
    *   `pub fn unbounded_async<T: Send>() -> (AsyncSender<T>, AsyncReceiver<T>)`
*   **Handles:**
    *   `Sender<T: Send>` (`Clone`) and `Receiver<T: Send>` (`!Clone`).
    *   `AsyncSender<T: Send>` (`Clone`) and `AsyncReceiver<T: Send>` (`!Clone`). `AsyncReceiver` implements `futures::Stream`.
*   **Key Methods:** `send`, `recv`, `close`, `is_closed`, and conversions. `Sender::is_empty()` is provided but is a non-guaranteed approximation.

### Module: `fibre::spmc`

A broadcast-style channel for one-to-many communication (bounded). `T` must be `Send + Clone`.

*   **Constructors:**
    *   `pub fn bounded<T: Send + Clone>(capacity: usize) -> (Sender<T>, Receiver<T>)` (Panics if capacity is 0).
    *   `pub fn bounded_async<T: Send + Clone>(capacity: usize) -> (AsyncSender<T>, AsyncReceiver<T>)`
*   **Handles:**
    *   `Sender<T: Send + Clone>` (`!Clone`) and `Receiver<T: Send + Clone>` (`Clone`).
    *   `AsyncSender<T: Send + Clone>` (`!Clone`) and `AsyncReceiver<T: Send + Clone>` (`Clone`). `AsyncReceiver` implements `futures::Stream`.
*   **Key Methods:** `send`, `try_send`, `recv`, `close`, `is_closed`, `capacity`, and conversions.

### Module: `fibre::spsc`

A high-performance lock-free channel for one-to-one communication (bounded). `T` must be `Send`.

*   **Constructors:**
    *   `pub fn bounded_sync<T: Send>(capacity: usize) -> (BoundedSyncSender<T>, BoundedSyncReceiver<T>)` (Panics if capacity is 0).
    *   `pub fn bounded_async<T: Send>(capacity: usize) -> (AsyncBoundedSpscSender<T>, AsyncBoundedSpscReceiver<T>)`
*   **Handles:**
    *   `BoundedSyncSender<T: Send>` (`!Clone`) and `BoundedSyncReceiver<T: Send>` (`!Clone`).
    *   `AsyncBoundedSpscSender<T: Send>` (`!Clone`) and `AsyncBoundedSpscReceiver<T: Send>` (`!Clone`). `AsyncBoundedSpscReceiver` implements `futures::Stream`.
*   **Key Methods:** `send`, `try_send`, `recv`, `recv_timeout`, `close`, `is_closed`, `capacity`, and conversions.

### Module: `fibre::oneshot`

A channel for sending a single value once. `T` must be `Send`.

*   **Constructors:**
    *   `pub fn oneshot<T: Send>() -> (Sender<T>, Receiver<T>)`
*   **Handles:**
    *   `Sender<T>` (`Clone`) and `Receiver<T>` (`!Clone`).
*   **Key Methods:**
    *   `Sender::send(self, ...)` consumes the sender.
    *   `Receiver::recv(&self) -> ReceiveFuture`
    *   `close()`, `is_closed()`, `is_sent()`.

## Error Handling

Fibre uses a clear set of error enums to signal the result of channel operations.

*   **`TrySendError<T>`:** Returned from `try_send`.
    *   `Full(T)`: The channel is full. The unsent item is returned.
    *   `Closed(T)`: The receiver was dropped. The unsent item is returned.
    *   `Sent(T)`: (Oneshot only) A value was already sent.
    *   Use `.into_inner()` to recover the value.

*   **`SendError`:** Returned from blocking/async `send`.
    *   `Closed`: The receiver was dropped.
    *   `Sent`: (Oneshot only) A value was already sent.

*   **`TryRecvError`:** Returned from `try_recv`.
    *   `Empty`: The channel is currently empty but not disconnected.
    *   `Disconnected`: The channel is empty and all senders have been dropped.

*   **`RecvError`:** Returned from blocking/async `recv`.
    *   `Disconnected`: The channel is empty and all senders have been dropped.

*   **`CloseError`:** Returned from `close()` if the handle was already closed.

*   **`RecvErrorTimeout`:** Returned from `recv_timeout`.
    *   `Timeout`: The operation timed out before a value was received.
    *   `Disconnected`: The channel became disconnected during the wait.