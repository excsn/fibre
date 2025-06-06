# Usage Guide: fibre

This guide provides detailed examples and an overview of the core concepts and APIs in the `fibre` library.

### Table of Contents

*   [Core Concepts](#core-concepts)
*   [Quick Start Examples](#quick-start-examples)
    *   [MPMC Sync Example](#mpmc-sync-example)
    *   [MPSC Async Example](#mpsc-async-example)
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

*   **SPSC (Single-Producer, Single-Consumer):** The fastest pattern. Use when one thread/task sends to exactly one other thread/task.
*   **MPSC (Multi-Producer, Single-Consumer):** Many threads/tasks send to one receiver. Great for collecting results or distributing work to a single processor.
*   **SPMC (Single-Producer, Multi-Consumer):** One thread/task broadcasts the same message to many receivers.
*   **MPMC (Multi-Producer, Multi-Consumer):** The most flexible pattern, allowing many-to-many communication.

### Hybrid Sync/Async Model

The `mpmc`, `mpsc`, and `spmc` channels support full interoperability between synchronous and asynchronous code. Every sender and receiver handle has a `to_sync()` or `to_async()` method that performs a zero-cost conversion. This allows you to, for example, have producers running in standard OS threads (`std::thread`) sending data to a consumer running in a Tokio task, all on the same channel.

### Sender/Receiver Handles

All channels are interacted with via sender and receiver handles (e.g., `Sender`, `Receiver`, `Producer`, `Consumer`). These handles control access to the channel and manage its lifetime. When all senders for a channel are dropped, the channel becomes "disconnected." When all receivers are dropped, the channel becomes "closed."

## Quick Start Examples

### MPMC Sync Example

A simple many-to-many example using standard threads.

```rust
use fibre::mpmc;
use std::thread;

fn main() {
    let (tx, rx) = mpmc::channel(4); // Bounded channel with capacity 4
    let num_producers = 2;
    let items_per_producer = 5;

    // Spawn producers
    for i in 0..num_producers {
        let tx_clone = tx.clone();
        thread::spawn(move || {
            for j in 0..items_per_producer {
                let msg = format!("Producer {} says: {}", i, j);
                tx_clone.send(msg).unwrap();
            }
        });
    }
    drop(tx); // Drop the original sender

    // Receive all messages in the main thread
    for _ in 0..(num_producers * items_per_producer) {
        let received = rx.recv().unwrap();
        println!("Got: {}", received);
    }
}
```

### MPSC Async Example

An example with multiple asynchronous producers sending to a single consumer task.

```rust
use fibre::mpsc;
use tokio::task;

#[tokio::main]
async fn main() {
    let (tx, mut rx) = mpsc::channel_async();
    let num_producers = 3;
    let items_per_producer = 5;

    // Spawn async producers
    for i in 0..num_producers {
        let tx_clone = tx.clone();
        task::spawn(async move {
            for j in 0..items_per_producer {
                tx_clone.send((i, j)).await.unwrap();
            }
        });
    }
    drop(tx);

    // The single consumer receives all the work
    let mut total = 0;
    while let Ok(msg) = rx.recv().await {
        println!("Received: {:?}", msg);
        total += 1;
    }
    assert_eq!(total, num_producers * items_per_producer);
}
```

## API by Channel Type

### Module: `fibre::mpmc`

A flexible channel for many-to-many communication.

*   **Constructors:**
    *   `pub fn channel<T: Send>(capacity: usize) -> (Sender<T>, Receiver<T>)`: Creates a bounded, synchronous channel. `capacity = 0` creates a rendezvous channel.
    *   `pub fn channel_async<T: Send>(capacity: usize) -> (AsyncSender<T>, AsyncReceiver<T>)`: Creates a bounded, asynchronous channel.
    *   `pub fn unbounded<T: Send>() -> (Sender<T>, Receiver<T>)`: Creates an "unbounded" synchronous channel.
    *   `pub fn unbounded_async<T: Send>() -> (AsyncSender<T>, AsyncReceiver<T>)`: Creates an "unbounded" asynchronous channel.
*   **Handles:**
    *   `Sender<T>` and `Receiver<T>` (Sync)
    *   `AsyncSender<T>` and `AsyncReceiver<T>` (Async)
*   **Key Methods (on all handles):**
    *   `send(item: T) -> Result<_, SendError>`: Sends an item, blocking or awaiting if the channel is full.
    *   `recv() -> Result<T, RecvError>`: Receives an item, blocking or awaiting if the channel is empty.
    *   `try_send(item: T) -> Result<(), TrySendError<T>>`: Attempts to send an item without blocking.
    *   `try_recv() -> Result<T, TryRecvError>`: Attempts to receive an item without blocking.
    *   `to_sync(self)` / `to_async(self)`: Converts between sync and async handle types.

### Module: `fibre::mpsc`

An optimized lock-free channel for many-to-one communication.

*   **Constructors:**
    *   `pub fn channel<T: Send>() -> (Producer<T>, Consumer<T>)`: Creates a synchronous MPSC channel.
    *   `pub fn channel_async<T: Send>() -> (AsyncProducer<T>, AsyncConsumer<T>)`: Creates an asynchronous MPSC channel.
*   **Handles:**
    *   `Producer<T>` (sync, `Clone`) and `Consumer<T>` (sync, `!Clone`).
    *   `AsyncProducer<T>` (async, `Clone`) and `AsyncConsumer<T>` (async, `!Clone`).
*   **Key Methods:**
    *   `Producer::send(&self, value: T) -> Result<(), SendError>`: Sends a value.
    *   `Consumer::recv(&mut self) -> Result<T, RecvError>`: Receives a value.
    *   Async variants return a `Future` for send/recv operations.

### Module: `fibre::spmc`

A broadcast-style channel for one-to-many communication.

*   **Constructors:**
    *   `pub fn channel<T: Send + Clone>(capacity: usize) -> (Producer<T>, Receiver<T>)`: Creates a synchronous SPMC channel. Panics if capacity is 0.
    *   `pub fn channel_async<T: Send + Clone>(capacity: usize) -> (AsyncProducer<T>, AsyncReceiver<T>)`: Creates an asynchronous SPMC channel. Panics if capacity is 0.
*   **Handles:**
    *   `Producer<T>` (sync, `!Clone`) and `Receiver<T>` (sync, `Clone`).
    *   `AsyncProducer<T>` (async, `!Clone`) and `AsyncReceiver<T>` (async, `Clone`).
*   **Key Methods:**
    *   `Producer::send(&mut self, value: T) -> Result<(), SendError>`: Broadcasts a value to all consumers.
    *   `Receiver::recv(&mut self) -> Result<T, RecvError>`: Receives a broadcasted value.
    *   Async variants return a `Future` for send/recv operations.

### Module: `fibre::spsc`

A high-performance lock-free channel for one-to-one communication.

*   **Constructors:**
    *   `pub fn bounded_sync<T>(capacity: usize) -> (BoundedSyncProducer<T>, BoundedSyncConsumer<T>)`: Creates a bounded, synchronous SPSC channel. Panics if capacity is 0.
    *   `pub fn bounded_async<T>(capacity: usize) -> (AsyncBoundedSpscProducer<T>, AsyncBoundedSpscConsumer<T>)`: Creates a bounded, asynchronous SPSC channel. Panics if capacity is 0.
*   **Handles:**
    *   `BoundedSyncProducer<T>` and `BoundedSyncConsumer<T>`.
    *   `AsyncBoundedSpscProducer<T>` and `AsyncBoundedSpscConsumer<T>`.
*   **Key Methods:**
    *   `send(item: T)` and `recv()` methods for sending and receiving data, with sync and async variants.

### Module: `fibre::oneshot`

A channel for sending a single value once.

*   **Constructors:**
    *   `pub fn channel<T>() -> (Sender<T>, Receiver<T>)`: Creates a oneshot channel.
*   **Handles:**
    *   `Sender<T>` (`Clone`) and `Receiver<T>` (`!Clone`).
*   **Key Methods:**
    *   `Sender::send(self, value: T) -> Result<(), TrySendError<T>>`: Sends the single value. Consumes the sender.
    *   `Receiver::recv(&mut self) -> ReceiveFuture<'_, T>`: Returns a future that resolves to the sent value.

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
    *   `Empty`: The channel is currently empty.
    *   `Disconnected`: The channel is empty and all senders have been dropped.

*   **`RecvError`:** Returned from blocking/async `recv`.
    *   `Disconnected`: The channel is empty and all senders have been dropped.