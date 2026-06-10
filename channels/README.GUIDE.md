# Usage Guide: `fibre`

This guide provides detailed examples and an overview of the core concepts and APIs in the `fibre` library.

### Table of Contents

*   [Core Concepts](#core-concepts)
*   [Quick Start Examples](#quick-start-examples)
    *   [MPMC Sync Example](#mpmc-sync-example)
    *   [MPSC Async Stream Example](#mpsc-async-stream-example)
    *   [SPSC Hybrid Example](#spsc-hybrid-example)
    *   [SPMC Topic Example](#spmc-topic-example)
*   [API by Channel Type](#api-by-channel-type)
    *   [Module: `fibre::mpmc`](#module-fibrempmc)
    *   [Module: `fibre::mpsc`](#module-fibrempsc)
    *   [Module: `fibre::spmc`](#module-fibrespmc)
    *   [Module: `fibre::spmc::topic`](#module-fibrespmctopic)
    *   [Module: `fibre::spsc`](#module-fibrespsc)
    *   [Module: `fibre::oneshot`](#module-fibreoneshot)
*   [Batch Operations](#batch-operations)
*   [Error Handling](#error-handling)
*   [Testing and Debugging](#testing-and-debugging)

## Core Concepts

### Specialized Channels

Fibre's main philosophy is to provide the right tool for the job. Instead of using a general-purpose MPMC channel for all tasks, you can choose a specialized implementation that is algorithmically optimized for your use case, leading to better performance and lower overhead.

*   **SPSC (Single-Producer, Single-Consumer):** The fastest pattern for 1-to-1 communication. Implemented with a lock-free ring buffer. It's bounded and requires `T: Send`.
*   **MPSC (Multi-Producer, Single-Consumer):** Many threads/tasks send to one receiver. Great for collecting results or distributing work to a single processor. Implemented with a lock-free linked list and supports both bounded and unbounded modes. Requires `T: Send`.
*   **SPMC (Single-Producer, Multi-Consumer):** One thread/task broadcasts the same message to many receivers. Each consumer gets a clone of the message. Implemented with a specialized ring buffer that tracks individual consumer progress. It's bounded and requires `T: Send + Clone`. For a more flexible pub/sub model, see `spmc::topic`.
*   **MPMC (Multi-Producer, Multi-Consumer):** The most flexible pattern, allowing many-to-many communication. Implemented with a `parking_lot::Mutex` for robust state management and support for mixed sync/async waiters. Supports bounded (including rendezvous) and "unbounded" (memory-limited) capacities. Requires `T: Send`.
*   **Oneshot:** A channel for sending a single value, once, from one of potentially many senders to a single receiver. Requires `T: Send`.

### Disconnect Semantics

Fibre enforces a consistent disconnection contract across all channel types, with different behaviour depending on which *side* of the channel is closed.

**Sender-side close (graceful drain):** When all senders for a channel are dropped or explicitly closed, the channel becomes "disconnected." Any items already buffered at that point are still readable. Receivers will drain those items normally and only observe `Disconnected` (or `None` on a `Stream`) once the buffer is empty.

```rust
let (tx, rx) = fibre::mpmc::bounded(4);
tx.send(1).unwrap();
tx.send(2).unwrap();
drop(tx); // channel is now disconnected, but 1 and 2 are still buffered

assert_eq!(rx.try_recv(), Ok(1));  // still drains
assert_eq!(rx.try_recv(), Ok(2));  // still drains
assert_eq!(rx.try_recv(), Err(TryRecvError::Disconnected)); // now empty
```

**Receiver-side close (immediate stop):** When a receiver handle is dropped or explicitly closed, that handle immediately returns `Disconnected` on any subsequent `recv` or `try_recv` call — it does **not** drain remaining buffered items. Other receiver handles on the same channel (for channel types that support cloning) are unaffected.

The practical implication for graceful shutdown: **close or drop your senders** to allow receivers to drain the remaining work cleanly. Closing a receiver while items are still in the buffer abandons those items.

### Hybrid Sync/Async Model

The `mpmc`, `mpsc`, `spmc`, and `spsc` channels support full interoperability between synchronous and asynchronous code. Every sender and receiver handle has a `to_sync()` or `to_async()` method that performs a zero-cost conversion. This allows you to, for example, have producers running in standard OS threads (`std::thread`) sending data to a consumer running in a Tokio task, all on the same channel.

### Sender/Receiver Handles & The `Stream` Trait

All channels are interacted with via sender and receiver handles (e.g., `Sender`, `Receiver`, `AsyncSender`, `AsyncReceiver`; SPSC uses `BoundedSyncSender`, etc.). These handles control access to the channel and manage its lifetime. When all senders for a channel are dropped, the channel becomes "disconnected" from the perspective of the receiver. When all receivers are dropped, the channel becomes "closed" from the perspective of the sender.

**A key feature of all multi-item asynchronous receiver types (`mpmc::AsyncReceiver`, `mpsc::UnboundedAsyncReceiver`, `mpsc::BoundedAsyncReceiver`, `spmc::AsyncReceiver`, `spmc::topic::AsyncTopicReceiver`, `spsc::AsyncBoundedSpscReceiver`) is that they implement the `futures::Stream` trait.** This allows them to be used with the rich set of combinators provided by `futures::StreamExt`, such as `next()`, `map()`, `filter()`, `for_each`, and `collect()`. The `oneshot::Receiver` is an exception, as it yields at most one item and is used by awaiting its `.recv()` future directly.

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
    let (tx, mut rx) = mpsc::unbounded_async(); // Use the unbounded async MPSC constructor
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

### SPSC Hybrid Example

An example demonstrating a synchronous producer sending to an asynchronous consumer.

```rust
use fibre::spsc;
use std::thread;
use tokio::runtime::Runtime;

fn main() {
    let rt = Runtime::new().unwrap();
    rt.block_on(async {
        // Start with a sync channel, then convert the receiver to async
        let (p_sync, c_sync) = spsc::bounded_sync::<String>(2);
        let mut c_async = c_sync.to_async(); // Convert receiver to async

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

### SPMC Topic Example

An example of a publish-subscribe pattern where consumers listen to specific topics. The sender is non-blocking and will drop messages for slow consumers.

```rust
use fibre::spmc::topic;
use tokio::task;

#[tokio::main]
async fn main() {
    // Each receiver gets a private mailbox of capacity 16.
    let (tx, rx_news) = topic::channel_async(16);
    let rx_weather = rx_news.clone();

    // Subscribe to topics.
    rx_news.subscribe("news");
    rx_weather.subscribe("weather");

    // Start a news listener.
    let news_handle = task::spawn(async move {
        loop {
            match rx_news.recv().await {
                Ok((topic, msg)) => println!("[News] Got '{:?}' on topic '{}'", msg, topic),
                Err(_) => {
                    println!("[News] Channel disconnected.");
                    break;
                }
            }
        }
    });

    // Start a weather listener.
    let weather_handle = task::spawn(async move {
        match rx_weather.recv().await {
            Ok((topic, msg)) => println!("[Weather] Got '{:?}' on topic '{}'", msg, topic),
            Err(_) => println!("[Weather] Channel disconnected."),
        }
    });

    // Publish messages to different topics.
    tx.send("news", "Fibre v1.0 released!").unwrap();
    tx.send("weather", "Sunny with a high of 75°F").unwrap();
    tx.send("news", "Library gains popularity.").unwrap();

    // Drop the sender to allow consumers to terminate.
    drop(tx);

    news_handle.await.unwrap();
    weather_handle.await.unwrap();
}
```

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
*   **Key Methods:**
    *   `send(...)`: Sync sends block, async sends return a `Future`.
    *   `try_send(&self, item: T) -> Result<(), TrySendError<T>>`
    *   `recv(...)`: Sync receives block, async receives return a `Future`.
    *   `recv_timeout(&self, timeout: std::time::Duration) -> Result<T, RecvErrorTimeout>`
    *   `try_recv(&self) -> Result<T, TryRecvError>`
    *   Batch: `send_batch`, `try_send_batch`, `send_batch_mut`, `try_send_batch_mut`, `recv_batch`, `try_recv_batch`, `recv_batch_mut`, `try_recv_batch_mut` (see [Batch Operations](#batch-operations)).
    *   `to_async(self)` / `to_sync(self)`

### Module: `fibre::mpsc`

An optimized lock-free channel for many-to-one communication.

*   **Constructors:**
    *   `pub fn unbounded<T: Send>() -> (UnboundedSender<T>, UnboundedReceiver<T>)`
    *   `pub fn unbounded_async<T: Send>() -> (UnboundedAsyncSender<T>, UnboundedAsyncReceiver<T>)`
    *   `pub fn bounded<T: Send>(capacity: usize) -> (BoundedSender<T>, BoundedReceiver<T>)`
    *   `pub fn bounded_async<T: Send>(capacity: usize) -> (BoundedAsyncSender<T>, BoundedAsyncReceiver<T>)`
*   **Handles (Unbounded):**
    *   `UnboundedSender<T: Send>` (sync, `Clone`) and `UnboundedReceiver<T: Send>` (sync, `!Clone`).
    *   `UnboundedAsyncSender<T: Send>` (async, `Clone`) and `UnboundedAsyncReceiver<T: Send>` (async, `!Clone`). `UnboundedAsyncReceiver` implements `futures::Stream`.
*   **Handles (Bounded):**
    *   `BoundedSender<T: Send>` (sync, `Clone`) and `BoundedReceiver<T: Send>` (sync, `!Clone`).
    *   `BoundedAsyncSender<T: Send>` (async, `Clone`) and `BoundedAsyncReceiver<T: Send>` (async, `!Clone`). `BoundedAsyncReceiver` implements `futures::Stream`.
*   **Key Methods:**
    *   `send(...)`: Sync sends block, async sends return a `Future`.
    *   `try_send(...)`
    *   `recv(...)`: Sync receives block, async receives return a `Future`.
    *   `recv_timeout(...)`: Sync receives block with a timeout.
    *   Batch: `send_batch`, `try_send_batch`, `send_batch_mut`, `try_send_batch_mut`, `recv_batch`, `try_recv_batch`, `recv_batch_mut`, `try_recv_batch_mut` (see [Batch Operations](#batch-operations)). Unbounded batch sends never block; bounded batch sends acquire capacity permits in bulk.

### Module: `fibre::spmc`

A broadcast-style channel for one-to-many communication (bounded). `T` must be `Send + Clone`.

*   **Constructors:**
    *   `pub fn bounded<T: Send + Clone>(capacity: usize) -> (Sender<T>, Receiver<T>)` (Panics if capacity is 0).
    *   `pub fn bounded_async<T: Send + Clone>(capacity: usize) -> (AsyncSender<T>, AsyncReceiver<T>)` (Panics if capacity is 0).
*   **Handles:**
    *   `Sender<T: Send + Clone>` (sync, `!Clone`, `send` takes `&self`)
    *   `Receiver<T: Send + Clone>` (sync, `Clone`)
    *   `AsyncSender<T: Send + Clone>` (async, `!Clone`, `send` takes `&self`)
    *   `AsyncReceiver<T: Send + Clone>` (async, `Clone`). `AsyncReceiver` implements `futures::Stream`.
*   **Key Methods:**
    *   `send(&self, ...)`: Blocks if any consumer is slow and its buffer view is full. Async version returns a `Future`.
    *   `recv(&self, ...)`: Blocks if the consumer's view of the buffer is empty. Async version returns a `Future`.
    *   `recv_timeout(&self, timeout: std::time::Duration) -> Result<T, RecvErrorTimeout>`
    *   Batch: `send_batch`, `try_send_batch`, `send_batch_mut`, `try_send_batch_mut`, `recv_batch`, `try_recv_batch`, `recv_batch_mut`, `try_recv_batch_mut` (see [Batch Operations](#batch-operations)). Batch sends are bounded by the slowest consumer; every consumer receives (clones) every item.
    *   `Sender::close(&mut self)` and `AsyncSender::close(&mut self)` take `&mut self`.

### Module: `fibre::spmc::topic`

A flexible publish-subscribe channel for one-to-many communication. Messages are sent to topics, and consumers subscribe to topics they are interested in.

*   **Constructors:**
    *   `pub fn channel<K, T>(mailbox_capacity: usize) -> (TopicSender<K, T>, TopicReceiver<K, T>)`
    *   `pub fn channel_async<K, T>(mailbox_capacity: usize) -> (AsyncTopicSender<K, T>, AsyncTopicReceiver<K, T>)`
*   **Handles:**
    *   `TopicSender<K, T>` (sync, `Clone`) and `TopicReceiver<K, T>` (sync, `Clone`).
    *   `AsyncTopicSender<K, T>` (async, `Clone`) and `AsyncTopicReceiver<K, T>` (async, `Clone`). `AsyncTopicReceiver` implements `futures::Stream`.
*   **Key Methods:**
    *   `send(&self, topic: K, value: T)`: Non-blocking. Drops messages for slow consumers.
    *   `subscribe(&self, topic: K)` and `unsubscribe(&self, topic: &Q)`.
    *   `recv()`: Blocks if this receiver's private mailbox is empty. Async version returns a `Future`.
    *   `recv_timeout(...)`
    *   `to_async(self)` / `to_sync(self)`

### Module: `fibre::spsc`

A high-performance lock-free channel for one-to-one communication (bounded). `T` must be `Send`.

*   **Constructors:**
    *   `pub fn bounded_sync<T: Send>(capacity: usize) -> (BoundedSyncSender<T>, BoundedSyncReceiver<T>)` (Panics if capacity is 0).
    *   `pub fn bounded_async<T: Send>(capacity: usize) -> (AsyncBoundedSpscSender<T>, AsyncBoundedSpscReceiver<T>)` (Panics if capacity is 0).
*   **Handles:**
    *   `BoundedSyncSender<T: Send>` (`!Clone`) and `BoundedSyncReceiver<T: Send>` (`!Clone`).
    *   `AsyncBoundedSpscSender<T: Send>` (`!Clone`) and `AsyncBoundedSpscReceiver<T: Send>` (`!Clone`). `AsyncBoundedSpscReceiver` implements `futures::Stream`.
*   **Key Methods:**
    *   `send(...)`: Sync sends block, async sends return a `Future`.
    *   `try_send(...)`
    *   `recv(...)`: Sync receives block, async receives return a `Future`.
    *   `BoundedSyncReceiver::recv_timeout(...)`
    *   Batch: `send_batch`, `try_send_batch`, `send_batch_mut`, `try_send_batch_mut`, `recv_batch`, `try_recv_batch`, `recv_batch_mut`, `try_recv_batch_mut` (see [Batch Operations](#batch-operations)). The whole batch is published with a single ring-index update and one wake.

### Module: `fibre::oneshot`

A channel for sending a single value once. `T` must be `Send`.

*   **Constructors:**
    *   `pub fn oneshot<T>() -> (Sender<T>, Receiver<T>)`
*   **Handles:**
    *   `Sender<T>` (`Clone`) and `Receiver<T>` (`!Clone`).
*   **Key Methods:**
    *   `Sender::send(self, ...)`: Consumes the sender. Only the first `send` across all clones succeeds.
    *   `Receiver::recv(&self)`: Returns a `Future` that completes when the value is sent or the channel is disconnected.

## Batch Operations

All core channels (`spsc`, `mpsc`, `spmc`, `mpmc`) provide a uniform batch API that amortizes synchronization over the whole batch: a single atomic index update or lock acquisition, bulk capacity-permit handling, and coalesced wakeups. (`oneshot` and `spmc::topic` do not provide batch operations.)

### The Eight Operations

| Operation | Input | Success | Failure |
| :--- | :--- | :--- | :--- |
| `try_send_batch(items)` | `Vec<T>` (by value) | `Ok(n)` — **all** items sent | `TrySendBatchError { sent, unsent, reason }` |
| `send_batch(items)` | `Vec<T>` (by value) | `Ok(n)` — all items sent (blocks/awaits) | `SendBatchError { sent, unsent }` (channel closed) |
| `try_send_batch_mut(&mut items)` | `&mut Vec<T>` | `Ok(k)` — `k` items drained from the **front** | `SendError::Closed` only if closed with zero sent |
| `send_batch_mut(&mut items)` | `&mut Vec<T>` | `Ok(n)` — all sent (blocks/awaits) | `SendError::Closed`; unsent items remain in the vec |
| `try_recv_batch(max)` | limit | `Ok(Vec<T>)` — 1..=max items | `TryRecvError::Empty` / `Disconnected` |
| `recv_batch(max)` | limit | `Ok(Vec<T>)` — 1..=max items (blocks/awaits) | `RecvError::Disconnected` |
| `try_recv_batch_mut(&mut out, max)` | buffer + limit | `Ok(k)` — `k` items appended to `out` | `TryRecvError::Empty` / `Disconnected` |
| `recv_batch_mut(&mut out, max)` | buffer + limit | `Ok(k)` — appended (blocks/awaits) | `RecvError::Disconnected` |

### Semantics

*   **Partial progress, no silent drops.** By-value sends return the count sent *and* the unsent remainder on interruption (`.into_unsent()` recovers the items). In-place sends drain only what was actually sent.
*   **Blocking/async `recv_batch(max)` waits for at least one item**, then drains up to `max` without further waiting — the latency-friendly semantic for burst draining.
*   **Empty input or `max == 0`** returns immediately with `Ok(0)` / an empty vector, even on a closed channel.
*   **Rendezvous channels** (capacity 0, `mpsc::bounded(0)` / `mpmc::bounded(0)`): non-blocking batch sends transfer only as many items as receivers are ready for; blocking batch sends degrade gracefully to item-by-item handoff. A batch receive on `mpmc` extracts payloads directly from all currently parked senders in one pass.
*   **Cancellation.** Dropping a pending by-value async send-batch future drops the unsent remainder (consistent with the single-item send futures). The `_mut` send futures are cancel-safe: unsent items remain in your vector. All batch receive futures are cancel-safe.

### Sync Example (bounded MPSC)

```rust
use fibre::mpsc;
use std::thread;

fn main() {
    let (tx, rx) = mpsc::bounded::<u32>(64);

    let producer = thread::spawn(move || {
        // Blocks as needed until the entire batch is delivered.
        let sent = tx.send_batch((0..1000).collect()).expect("receiver alive");
        assert_eq!(sent, 1000);
    });

    let mut received = Vec::new();
    while received.len() < 1000 {
        // Blocks until at least one item, then drains up to 128 in one call.
        match rx.recv_batch_mut(&mut received, 128) {
            Ok(_count) => {}
            Err(_) => break, // Disconnected
        }
    }
    producer.join().unwrap();
    assert_eq!(received.len(), 1000);
}
```

### Async Example (MPMC)

```rust
use fibre::mpmc;

#[tokio::main]
async fn main() {
    let (tx, rx) = mpmc::bounded_async::<String>(32);

    tokio::spawn(async move {
        let batch: Vec<String> = (0..100).map(|i| format!("job-{i}")).collect();
        // Resolves once every item is sent; permits are handled in bulk.
        tx.send_batch(batch).await.expect("receiver alive");
    });

    let mut done = 0;
    while done < 100 {
        // Resolves with 1..=16 items as soon as anything is available.
        match rx.recv_batch(16).await {
            Ok(jobs) => done += jobs.len(),
            Err(_) => break, // Disconnected
        }
    }
    assert_eq!(done, 100);
}
```

### Handling Partial Sends

```rust
use fibre::mpsc;
use fibre::error::BatchSendErrorReason;

fn main() {
    let (tx, _rx) = mpsc::bounded::<u32>(4);
    match tx.try_send_batch((0..10).collect()) {
        Ok(n) => println!("all {n} sent"),
        Err(e) if e.reason == BatchSendErrorReason::Full => {
            println!("sent {}, channel full", e.sent);
            let remainder: Vec<u32> = e.into_unsent(); // retry later
            assert_eq!(remainder.len(), 6);
        }
        Err(e) => println!("channel closed, {} unsent", e.unsent.len()),
    }
}
```

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
    *   `Disconnected`: Either all senders have been dropped and the buffer is fully drained, or this receiver handle has been explicitly closed.

*   **`RecvError`:** Returned from blocking/async `recv`.
    *   `Disconnected`: The channel is empty and all senders have been dropped.

*   **`RecvErrorTimeout`:** Returned from `recv_timeout`.
    *   `Timeout`: The operation timed out before a value was received.
    *   `Disconnected`: The channel became disconnected during the wait.

*   **`TrySendBatchError<T>`:** Returned from `try_send_batch`. Carries partial-progress state.
    *   `sent: usize`: How many items were sent before the operation stopped.
    *   `unsent: Vec<T>`: The unsent remainder, in order. Recover it with `.into_unsent()`.
    *   `reason: BatchSendErrorReason`: `Full` or `Closed`.

*   **`SendBatchError<T>`:** Returned from blocking/async `send_batch`. The only cause is channel closure.
    *   `sent: usize` and `unsent: Vec<T>`, as above. Recover the remainder with `.into_unsent()`.

## Testing and Debugging

Fibre is a complex library with a high degree of concurrency. To aid in debugging, it includes an optional telemetry system and is best tested with powerful tools.

### Using `cargo-nextest`

`cargo-nextest` is the recommended test runner for `fibre` due to its ability to handle per-test timeouts (essential for detecting deadlocks) and its efficient parallel execution.

```bash
# Run all tests
cargo nextest run

# Run a specific test, e.g., in spmc/mod.rs
cargo nextest run spmc::tests::spmc_sync_slow_consumer_blocks_producer
```

### Using ThreadSanitizer (TSan)

TSan is invaluable for detecting data races in the library's `unsafe` and atomic code. It requires a nightly Rust toolchain.

```bash
# Example: Run SPMC tests with TSan on an Apple Silicon Mac
RUSTFLAGS="-Z sanitizer=thread" cargo +nightly nextest run \
  --test spmc \
  --target aarch64-apple-darwin
```

### Telemetry Feature

When the `fibre_logging` feature is enabled, the library will log detailed internal events, such as when threads park, wake, or contend for resources. This is extremely useful for diagnosing deadlocks and performance issues.

```bash
# Run tests with telemetry enabled
cargo nextest run --features fibre_logging

# After the test run, a detailed report is printed to the console.
```