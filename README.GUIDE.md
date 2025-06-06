# Usage Guide: `fibre`

This guide provides detailed examples and an overview of the core concepts and APIs in the `fibre` library.

### Table of Contents

*   [Core Concepts](#core-concepts)
*   [Quick Start Examples](#quick-start-examples)
    *   [MPMC Sync Example](#mpmc-sync-example)
    *   [MPSC Async Example](#mpsc-async-example)
    *   [SPSC Hybrid Example](#spsc-hybrid-example)
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

*   **SPSC (Single-Producer, Single-Consumer):** The fastest pattern. Use when one thread/task sends to exactly one other thread/task. Implemented with a lock-free ring buffer.
*   **MPSC (Multi-Producer, Single-Consumer):** Many threads/tasks send to one receiver. Great for collecting results or distributing work to a single processor. Implemented with a lock-free linked list.
*   **SPMC (Single-Producer, Multi-Consumer):** One thread/task broadcasts the same message to many receivers. Each consumer gets a clone of the message. Implemented with a specialized ring buffer that tracks individual consumer progress.
*   **MPMC (Multi-Producer, Multi-Consumer):** The most flexible pattern, allowing many-to-many communication. Implemented with a `parking_lot::Mutex` for robust state management and support for mixed sync/async waiters.
*   **Oneshot:** A channel for sending a single value, once, from one of potentially many senders to a single receiver.

### Hybrid Sync/Async Model

The `mpmc`, `mpsc`, `spmc`, and `spsc` channels support full interoperability between synchronous and asynchronous code. Every sender and receiver handle has a `to_sync()` or `to_async()` method that performs a zero-cost conversion. This allows you to, for example, have producers running in standard OS threads (`std::thread`) sending data to a consumer running in a Tokio task, all on the same channel.

### Sender/Receiver Handles

All channels are interacted with via sender and receiver handles (e.g., `Sender`, `Receiver`, `Producer`, `Consumer`). These handles control access to the channel and manage its lifetime. When all senders for a channel are dropped, the channel becomes "disconnected" from the perspective of the receiver. When all receivers are dropped, the channel becomes "closed" from the perspective of the sender.

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
    let mut handles = Vec::new();

    // Spawn producers
    for i in 0..num_producers {
        let tx_clone = tx.clone();
        handles.push(thread::spawn(move || {
            for j in 0..items_per_producer {
                let msg = format!("Producer {} says: {}", i, j);
                if tx_clone.send(msg).is_err() {
                    eprintln!("Producer {}: Receiver dropped!", i);
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
                    Ok(received) => println!("Consumer {}: Got: {}", c_id, received),
                    Err(_) => { // Disconnected
                        println!("Consumer {}: Channel disconnected.", c_id);
                        break;
                    }
                }
            }
        }));
    }
    drop(rx); // Drop original receiver

    for handle in handles { // Producer handles
        handle.join().unwrap();
    }
    for handle in consumer_handles {
        handle.join().unwrap();
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
                if tx_clone.send((i, j)).await.is_err() {
                    eprintln!("Async Producer {}: Receiver dropped!", i);
                    break;
                }
            }
        });
    }
    drop(tx); // Signal to the consumer that all producers are potentially done after their work

    // The single consumer receives all the work
    let mut total = 0;
    while let Ok(msg) = rx.recv().await {
        println!("Received: {:?}", msg);
        total += 1;
    }
    assert_eq!(total, num_producers * items_per_producer);
    println!("MPSC async example finished.");
}
```

### SPSC Hybrid Example

An example demonstrating a synchronous producer sending to an asynchronous consumer.

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
            // p_sync is dropped here, consumer will see Disconnected after messages
        });

        println!("Async consumer received: {}", c_async.recv().await.unwrap());
        println!("Async consumer received: {}", c_async.recv().await.unwrap());
        assert!(matches!(c_async.recv().await, Err(fibre::error::RecvError::Disconnected)));

        producer_thread.join().unwrap();
    });
}
```

## API by Channel Type

_(Note: `T` must generally be `Send`. Specific trait bounds like `Clone` or `Unpin` are noted.)_

### Module: `fibre::mpmc`

A flexible channel for many-to-many communication.

*   **Constructors:**
    *   `pub fn channel<T: Send>(capacity: usize) -> (Sender<T>, Receiver<T>)`: Creates a bounded, synchronous channel. `capacity = 0` creates a rendezvous channel.
    *   `pub fn channel_async<T: Send>(capacity: usize) -> (AsyncSender<T>, AsyncReceiver<T>)`: Creates a bounded, asynchronous channel. `capacity = 0` for rendezvous.
    *   `pub fn unbounded<T: Send>() -> (Sender<T>, Receiver<T>)`: Creates an "unbounded" synchronous channel.
    *   `pub fn unbounded_async<T: Send>() -> (AsyncSender<T>, AsyncReceiver<T>)`: Creates an "unbounded" asynchronous channel.
*   **Handles:**
    *   `Sender<T: Send>` (`Clone`) and `Receiver<T: Send>` (`Clone`).
    *   `AsyncSender<T: Send>` (`Clone`) and `AsyncReceiver<T: Send>` (`Clone`).
*   **Key Methods:**
    *   `Sender::send(&self, item: T) -> Result<(), SendError>`
    *   `Sender::try_send(&self, item: T) -> Result<(), TrySendError<T>>`
    *   `Sender::to_async(self) -> AsyncSender<T>`
    *   `Receiver::recv(&self) -> Result<T, RecvError>`
    *   `Receiver::try_recv(&self) -> Result<T, TryRecvError>`
    *   `Receiver::to_async(self) -> AsyncReceiver<T>`
    *   `AsyncSender::send(&self, item: T) -> SendFuture<'_, T>` (Requires `T: Unpin`)
    *   `AsyncSender::try_send(&self, item: T) -> Result<(), TrySendError<T>>`
    *   `AsyncSender::to_sync(self) -> Sender<T>`
    *   `AsyncReceiver::recv(&self) -> ReceiveFuture<'_, T>` (Requires `T: Unpin`)
    *   `AsyncReceiver::try_recv(&self) -> Result<T, TryRecvError>`
    *   `AsyncReceiver::to_sync(self) -> Receiver<T>`
    *   All handles: `is_closed(&self)` (for senders), `is_disconnected(&self)` (for receivers), `capacity(&self) -> Option<usize>`.

### Module: `fibre::mpsc`

An optimized lock-free channel for many-to-one communication.

*   **Constructors:**
    *   `pub fn channel<T: Send>() -> (Producer<T>, Consumer<T>)`: Creates a synchronous MPSC channel.
    *   `pub fn channel_async<T: Send>() -> (AsyncProducer<T>, AsyncConsumer<T>)`: Creates an asynchronous MPSC channel.
*   **Handles:**
    *   `Producer<T: Send>` (sync, `Clone`) and `Consumer<T: Send>` (sync, `!Clone`).
    *   `AsyncProducer<T: Send>` (async, `Clone`) and `AsyncConsumer<T: Send>` (async, `!Clone`).
*   **Key Methods:**
    *   `Producer::send(&self, value: T) -> Result<(), SendError>`
    *   `Producer::to_async(self) -> AsyncProducer<T>`
    *   `Consumer::recv(&mut self) -> Result<T, RecvError>`
    *   `Consumer::try_recv(&mut self) -> Result<T, TryRecvError>`
    *   `Consumer::to_async(self) -> AsyncConsumer<T>`
    *   `AsyncProducer::send(&self, value: T) -> SendFuture<'_, T>` (Requires `T: Unpin`)
    *   `AsyncProducer::to_sync(self) -> Producer<T>`
    *   `AsyncConsumer::recv(&mut self) -> RecvFuture<'_, T>`
    *   `AsyncConsumer::try_recv(&mut self) -> Result<T, TryRecvError>`
    *   `AsyncConsumer::to_sync(self) -> Consumer<T>`

### Module: `fibre::spmc`

A broadcast-style channel for one-to-many communication. `T` must be `Send + Clone`.

*   **Constructors:**
    *   `pub fn channel<T: Send + Clone>(capacity: usize) -> (Producer<T>, Receiver<T>)`: Creates a synchronous SPMC channel. Panics if capacity is 0.
    *   `pub fn channel_async<T: Send + Clone>(capacity: usize) -> (AsyncProducer<T>, AsyncReceiver<T>)`: Creates an asynchronous SPMC channel. Panics if capacity is 0.
*   **Handles:**
    *   `Producer<T: Send + Clone>` (sync, `!Clone`) and `Receiver<T: Send + Clone>` (sync, `Clone`).
    *   `AsyncProducer<T: Send + Clone>` (async, `!Clone`) and `AsyncReceiver<T: Send + Clone>` (async, `Clone`).
*   **Key Methods:**
    *   `Producer::send(&mut self, value: T) -> Result<(), SendError>`
    *   `Producer::to_async(self) -> AsyncProducer<T>`
    *   `Receiver::recv(&mut self) -> Result<T, RecvError>`
    *   `Receiver::try_recv(&mut self) -> Result<T, TryRecvError>`
    *   `Receiver::to_async(self) -> AsyncReceiver<T>`
    *   `AsyncProducer::send(&mut self, value: T) -> SendFuture<'_, T>` (Requires `T: Unpin`)
    *   `AsyncProducer::to_sync(self) -> Producer<T>`
    *   `AsyncReceiver::recv(&mut self) -> RecvFuture<'_, T>`
    *   `AsyncReceiver::try_recv(&mut self) -> Result<T, TryRecvError>`
    *   `AsyncReceiver::to_sync(self) -> Receiver<T>`

### Module: `fibre::spsc`

A high-performance lock-free channel for one-to-one communication. `T` must be `Send`.

*   **Constructors:**
    *   `pub fn bounded_sync<T: Send>(capacity: usize) -> (BoundedSyncProducer<T>, BoundedSyncConsumer<T>)`: Creates a bounded, synchronous SPSC channel. Panics if capacity is 0.
    *   `pub fn bounded_async<T: Send>(capacity: usize) -> (AsyncBoundedSpscProducer<T>, AsyncBoundedSpscConsumer<T>)`: Creates a bounded, asynchronous SPSC channel. Panics if capacity is 0.
*   **Handles:**
    *   `BoundedSyncProducer<T: Send>` (`!Clone`) and `BoundedSyncConsumer<T: Send>` (`!Clone`).
    *   `AsyncBoundedSpscProducer<T: Send>` (`!Clone`) and `AsyncBoundedSpscConsumer<T: Send>` (`!Clone`).
*   **Key Methods:**
    *   `BoundedSyncProducer::send(&mut self, item: T) -> Result<(), SendError>`
    *   `BoundedSyncProducer::try_send(&mut self, item: T) -> Result<(), TrySendError<T>>`
    *   `BoundedSyncProducer::to_async(self) -> AsyncBoundedSpscProducer<T>`
    *   `BoundedSyncConsumer::recv(&mut self) -> Result<T, RecvError>`
    *   `BoundedSyncConsumer::try_recv(&mut self) -> Result<T, TryRecvError>`
    *   `BoundedSyncConsumer::recv_timeout(&mut self, timeout: Duration) -> Result<T, RecvErrorTimeout>`
    *   `BoundedSyncConsumer::to_async(self) -> AsyncBoundedSpscConsumer<T>`
    *   `AsyncBoundedSpscProducer::send(&self, item: T) -> SendFuture<'_, T>` (Requires `T: Unpin`)
    *   `AsyncBoundedSpscProducer::try_send(&self, item: T) -> Result<(), TrySendError<T>>`
    *   `AsyncBoundedSpscProducer::to_sync(self) -> BoundedSyncProducer<T>`
    *   `AsyncBoundedSpscConsumer::recv(&self) -> ReceiveFuture<'_, T>` (Requires `T: Unpin`)
    *   `AsyncBoundedSpscConsumer::try_recv(&self) -> Result<T, TryRecvError>`
    *   `AsyncBoundedSpscConsumer::to_sync(self) -> BoundedSyncConsumer<T>`

### Module: `fibre::oneshot`

A channel for sending a single value once.

*   **Constructors:**
    *   `pub fn channel<T>() -> (Sender<T>, Receiver<T>)`: Creates a oneshot channel.
*   **Handles:**
    *   `Sender<T>` (`Clone`) and `Receiver<T>` (`!Clone`).
*   **Key Methods:**
    *   `Sender::send(self, value: T) -> Result<(), TrySendError<T>>`: Sends the single value. Consumes the sender. Fails if already sent or receiver dropped.
    *   `Sender::is_closed(&self) -> bool`: Checks if the receiver has been dropped.
    *   `Sender::is_sent(&self) -> bool`: Checks if a value has been successfully sent (or taken).
    *   `Receiver::recv(&mut self) -> ReceiveFuture<'_, T>`: Returns a future that resolves to the sent value or an error if sender drops. (Requires `T: Unpin`)
    *   `Receiver::try_recv(&mut self) -> Result<T, TryRecvError>`: Attempts to receive the value non-blockingly.
    *   `Receiver::is_closed(&self) -> bool`: Checks if the channel is terminally closed (value taken, or senders dropped without sending).

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

*   **`RecvErrorTimeout`:** Returned from `recv_timeout`.
    *   `Timeout`: The operation timed out before a value was received.
    *   `Disconnected`: The channel became disconnected during the wait.
