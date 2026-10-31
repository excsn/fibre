# Usage Guide: `fibre`

This guide shows how to choose, create, use and shut down `fibre` channels and locks.

## Table of Contents

* [Core Concepts](#core-concepts)
* [Quick Start](#quick-start)
    * [MPMC Sync Example](#mpmc-sync-example)
    * [MPSC Async Stream Example](#mpsc-async-stream-example)
    * [SPSC Hybrid Example](#spsc-hybrid-example)
    * [SPMC Broadcast Example](#spmc-broadcast-example)
    * [SPMC Topic Example](#spmc-topic-example)
    * [Oneshot Example](#oneshot-example)
* [Choosing a Channel](#choosing-a-channel)
    * [Comparing Capacity Modes](#comparing-capacity-modes)
    * [Comparing Guarantees](#comparing-guarantees)
    * [Comparing Sender Methods](#comparing-sender-methods)
    * [Comparing Receiver Methods](#comparing-receiver-methods)
* [Mixing Sync and Async Code](#mixing-sync-and-async-code)
* [Consuming a Receiver as a Stream](#consuming-a-receiver-as-a-stream)
* [Sending Many to Many](#sending-many-to-many)
* [Collecting Work From Many Producers](#collecting-work-from-many-producers)
* [Broadcasting to Every Consumer](#broadcasting-to-every-consumer)
* [Publishing to Topics](#publishing-to-topics)
* [Handing Off With Rendezvous](#handing-off-with-rendezvous)
* [Sending One Value](#sending-one-value)
    * [Recycling Channels With a Pool](#recycling-channels-with-a-pool)
    * [Pooling Whole Request Records](#pooling-whole-request-records)
* [Sending in Batches](#sending-in-batches)
    * [Using the Eight Operations](#using-the-eight-operations)
    * [Relying on Batch Semantics](#relying-on-batch-semantics)
    * [Batching With Bounded MPSC](#batching-with-bounded-mpsc)
    * [Batching With Async MPMC](#batching-with-async-mpmc)
    * [Handling Partial Sends](#handling-partial-sends)
* [Cancelling a Pending Send or Receive](#cancelling-a-pending-send-or-receive)
* [Shutting Down a Channel](#shutting-down-a-channel)
    * [Draining After Senders Close](#draining-after-senders-close)
    * [Stopping a Receiver](#stopping-a-receiver)
    * [Closing Handles Explicitly](#closing-handles-explicitly)
* [Sharing a Lock Between Threads and Tasks](#sharing-a-lock-between-threads-and-tasks)
    * [Understanding How the Locks Wait](#understanding-how-the-locks-wait)
    * [Using the Read-Biased Lock](#using-the-read-biased-lock)
* [Turning Off the Topic Feature](#turning-off-the-topic-feature)
* [Recording Telemetry](#recording-telemetry)
* [Measuring Performance](#measuring-performance)
    * [Reading the Results](#reading-the-results)
    * [Understanding the Design](#understanding-the-design)
* [Error Handling](#error-handling)

## Core Concepts

* **SPSC:** Single-producer, single-consumer: the fastest pattern for 1-to-1 communication, bounded and backed by a lock-free ring buffer.
* **MPSC:** Multi-producer, single-consumer: many threads or tasks send to one receiver, in bounded or unbounded mode.
* **SPMC:** Single-producer, multi-consumer: one sender broadcasts and every consumer receives a clone of every message.
* **SPMC Topic:** A publish-subscribe variant of SPMC where consumers only receive the topics they subscribe to.
* **MPMC:** Multi-producer, multi-consumer: the most flexible pattern, for dynamic producer and consumer counts.
* **Oneshot:** A channel for sending a single value, once, from one of potentially many senders to a single receiver.
* **Rendezvous:** A zero-capacity channel family that hands each item directly from sender to receiver.
* **Handle:** A sender or receiver value that controls access to a channel and its lifetime, with cloning rules that vary by channel type (`mpmc::Sender` is `Clone`, `spsc::BoundedSyncSender` is not).
* **Disconnected:** The state a receiver observes once all senders are dropped or closed and the buffer is empty.
* **Closed:** The state a sender observes once all receivers are dropped.
* **Hybrid model:** Sync and async code interoperate on one channel through `to_sync()` and `to_async()`, a zero-cost conversion available on every handle except the oneshot family.
* **Stream:** The `futures::Stream` trait, implemented by `mpmc::AsyncReceiver`, `mpmc::UnboundedAsyncReceiver`, `mpsc::UnboundedAsyncReceiver`, `mpsc::BoundedAsyncReceiver`, `spmc::BoundedAsyncReceiver`, `spsc::BoundedAsyncReceiver` and `spmc::topic::AsyncTopicReceiver`, but not by async rendezvous receivers.
* **Cancel safety:** A cancel-safe future can be dropped while pending without losing anything: a dropped receive leaves the message receivable and a dropped send has not sent.
* **Batch operation:** A send or receive of many items that amortizes synchronization over the whole batch.
* **Hybrid lock:** A lock in `fibre::sync` that serves blocking callers and futures at once.

## Quick Start

Async examples need `tokio` with the `macros`, `rt-multi-thread` and `time` features. Stream examples also need `futures-util`.

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

Multiple asynchronous producers send to a single consumer task, which processes items using the `Stream` API.

```rust
use fibre::mpsc;
use tokio::task;
use futures_util::StreamExt; // Import the StreamExt trait for .next()

#[tokio::main]
async fn main() {
    let (tx, mut rx) = mpsc::unbounded_async(); // Use the unbounded async MPSC constructor
    let num_producers = 3;
    let items_per_producer = 5;

    // Spawn async producers (send takes `&mut self`: one clone per task)
    for i in 0..num_producers {
        let mut tx_clone = tx.clone();
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

A synchronous producer sends to an asynchronous consumer.

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

### SPMC Broadcast Example

Every consumer receives a clone of every message.

```rust
use fibre::spmc;
use std::thread;

fn main() {
    let (tx, rx1) = spmc::bounded::<String>(4);
    let rx2 = rx1.clone();

    let first = thread::spawn(move || rx1.recv().unwrap());
    let second = thread::spawn(move || rx2.recv().unwrap());

    tx.send("hello".to_string()).unwrap();

    assert_eq!(first.join().unwrap(), "hello");
    assert_eq!(second.join().unwrap(), "hello");
}
```

### SPMC Topic Example

Consumers listen to specific topics. The sender is non-blocking and drops messages for slow consumers.

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

### Oneshot Example

One task sends a single value to another.

```rust
use fibre::oneshot;

#[tokio::main]
async fn main() {
    let (tx, mut rx) = oneshot::exclusive::<u32>();

    tokio::spawn(async move {
        tx.send(42).unwrap();
    });

    assert_eq!(rx.recv().await.unwrap(), 42);
}
```

## Choosing a Channel

Fibre provides the right tool for the job. Instead of using a general-purpose MPMC channel for all tasks, choose the implementation that is algorithmically optimized for your use case, which gives better performance and lower overhead. All constructors follow one shape.

```rust
use fibre::{mpmc, mpsc, oneshot, spmc, spsc};

fn main() {
    let (_tx, _rx) = spsc::bounded_sync::<u32>(1024);
    let (_tx, _rx) = mpsc::bounded::<u32>(128);
    let (_tx, _rx) = mpsc::unbounded::<u32>();
    let (_tx, _rx) = spmc::bounded::<u32>(128);
    let (_tx, _rx) = spmc::topic::channel::<&'static str, u32>(16);
    let (_tx, _rx) = mpmc::bounded::<u32>(128);
    let (_tx, _rx) = mpmc::unbounded::<u32>();
    let (_tx, _rx) = oneshot::oneshot::<u32>();
}
```

Each type parameter has a requirement: `T: Send` for `spsc`, `mpsc`, `mpmc` and `oneshot`; `T: Send + Clone` for `spmc`; `K: Send + Sync + Hash + Eq + Clone + 'static` and `T: Send + Clone + 'static` for `spmc::topic`.

Each channel type enforces appropriate `Send` and `Sync` bounds. SPSC and MPSC sync consumer handles are `!Sync` because they are designed for single-threaded consumption. SPSC async handles take `&mut self` on their send and receive methods, so exclusive single-producer and single-consumer access is checked at compile time.

### Comparing Capacity Modes

| Capacity mode | SPSC | MPSC | SPMC | SPMC Topic | MPMC | Oneshot |
| :--- | :---: | :---: | :---: | :---: | :---: | :---: |
| Bounded | ✅ | ✅ | ✅ | ✅ (mailbox) | ✅ | N/A |
| Unbounded | ❌ | ✅ | ❌ | ❌ | ✅ | N/A |
| Rendezvous (zero-capacity) | ✅ | ✅ | ❌ | ❌ | ✅ | N/A |

Rendezvous channels are a dedicated, zero-capacity family that hand each item directly from sender to receiver.

### Comparing Guarantees

| Guarantee | SPSC | MPSC | SPMC | SPMC Topic | MPMC | Oneshot |
| :--- | :---: | :---: | :---: | :---: | :---: | :---: |
| Cancel-safe `recv` future | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| Cancel-safe `send` future | ✅ | ✅ | ✅ | N/A | ✅ | N/A |
| Cancel-safe on rendezvous | ❌ | ❌ | N/A | N/A | ❌ | N/A |
| Order | FIFO | Total order, FIFO per producer | Every consumer sees send order | Send order per consumer, with gaps when its mailbox is full | Total order; each consumer gets an increasing subsequence; FIFO per producer | Single value |

- Cancel-safe means dropping a pending future loses nothing. A dropped `recv` future leaves the message receivable. A dropped `send` future has not sent.
- Rendezvous is the exception: a dropped receive future can lose an item already handed to it. A dropped send can still be delivered.
- Batch sends: dropping a pending `send_batch` future leaves the items already sent delivered and drops the rest. `send_batch_mut` leaves the unsent items in the caller's vector.
- SPMC Topic and Oneshot sends are not futures.

### Comparing Sender Methods

All channels share a common set of methods with consistent semantics. Unbounded `mpsc` and `mpmc` senders take `&mut self` for `send`, `try_send`, the batch sends and `close`.

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
| `clone()` | ✅ | ✅ | ❌ | ❌ | ❌ | ✅ |
| `send_batch()` | ✅ | ✅ | ✅ | ❌ | ✅ | N/A |
| `try_send_batch()` | ✅ | ✅ | ✅ | ❌ | ✅ | N/A |
| `send_batch_mut()` | ✅ | ✅ | ✅ | ❌ | ✅ | N/A |
| `try_send_batch_mut()` | ✅ | ✅ | ✅ | ❌ | ✅ | N/A |

### Comparing Receiver Methods

`recv_timeout` exists on sync receivers only and takes `&mut self` on `mpmc::UnboundedSyncReceiver`.

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

## Mixing Sync and Async Code

Every sender and receiver handle on `mpmc`, `mpsc`, `spmc`, `spsc` and `spmc::topic` has a `to_sync()` or `to_async()` method. Use it to run producers in standard OS threads that send to a consumer running in a Tokio task, all on the same channel.

```rust
use fibre::mpmc;
use std::thread;

#[tokio::main]
async fn main() {
    let (tx, rx) = mpmc::bounded::<u32>(4);
    let rx = rx.to_async();

    let producer = thread::spawn(move || tx.send(1).unwrap());

    assert_eq!(rx.recv().await.unwrap(), 1);
    producer.join().unwrap();
}
```

## Consuming a Receiver as a Stream

Multi-item async receivers work with the combinators in `futures::StreamExt`, such as `next()`, `map()`, `filter()`, `for_each` and `collect()`. The `oneshot` receivers yield at most one item, so await their `.recv()` future directly. Async rendezvous receivers do not implement `Stream`.

```rust
use futures_util::StreamExt;

#[tokio::main]
async fn main() {
    let (tx, rx) = fibre::mpmc::bounded_async::<u32>(4);
    tx.send(1).await.unwrap();
    tx.send(2).await.unwrap();
    drop(tx);

    let items: Vec<u32> = rx.collect().await;
    assert_eq!(items, vec![1, 2]);
}
```

## Sending Many to Many

Use `mpmc` when producer and consumer counts are dynamic. The bounded handles `Sender`, `Receiver`, `AsyncSender` and `AsyncReceiver` are `Clone` and take `&self`. The bounded channel is serialized by a `fibre::sync::HybridMutex` and supports mixed sync/async waiters. `bounded(0)` panics; use `mpmc::rendezvous`.

The unbounded channel is a dedicated implementation with lock-free producers and a mutex-serialized consumer side, not the bounded channel with infinite capacity. It is limited only by memory. Its handles are also `Clone`, but sends and waiting receives take `&mut self`, so clone a handle per thread or task.

```rust
use fibre::mpmc;
use std::thread;

fn main() {
    let (mut tx, rx) = mpmc::unbounded::<u32>();

    let workers: Vec<_> = (0..3)
        .map(|_| {
            let mut rx = rx.clone();
            thread::spawn(move || {
                let mut count = 0u32;
                while rx.recv().is_ok() {
                    count += 1;
                }
                count
            })
        })
        .collect();
    drop(rx);

    for i in 0..9 {
        tx.send(i).unwrap();
    }
    drop(tx);

    let total: u32 = workers.into_iter().map(|w| w.join().unwrap()).sum();
    assert_eq!(total, 9);
}
```

Send items one at a time when several receivers are parked. An unbounded `send_batch` wakes only one parked receiver and the others stay blocked with items queued until another send or a disconnect wakes them.

## Collecting Work From Many Producers

Use `mpsc` to collect results or distribute work to a single processor. Unbounded MPSC uses a lock-free slab-backed linked chain and its sends take `&mut self`, so clone a sender per thread or task. Bounded MPSC uses chunked slot arrays with a credit window and its sends take `&self`. `bounded(0)` panics; use `mpsc::rendezvous`.

```rust
use fibre::mpsc;
use std::thread;

fn main() {
    let (tx, rx) = mpsc::bounded::<u32>(16);

    let producers: Vec<_> = (0..4u32)
        .map(|p| {
            let tx = tx.clone();
            thread::spawn(move || {
                for i in 0..100 {
                    tx.send(p * 100 + i).unwrap();
                }
            })
        })
        .collect();
    drop(tx);

    let mut total = 0;
    while rx.recv().is_ok() {
        total += 1;
    }
    for producer in producers {
        producer.join().unwrap();
    }
    assert_eq!(total, 400);
}
```

Items still buffered when a bounded receiver is closed or dropped stay alive until the last sender handle drops. Drain the receiver first if those items hold resources such as a oneshot sender.

## Broadcasting to Every Consumer

Use `spmc` when every consumer must see every message without loss. It uses a specialized ring buffer that tracks individual consumer progress. `send` blocks if any consumer is slow and its buffer view is full. `recv` blocks if the consumer's view of the buffer is empty. `capacity == 0` panics.

A clone starts at the position of the handle it was cloned from, so it receives everything that handle has not yet consumed.

```rust
use fibre::spmc;

fn main() {
    let (tx, rx) = spmc::bounded::<u32>(4);
    tx.send(1).unwrap();

    let late = rx.clone();
    assert_eq!(rx.recv().unwrap(), 1);
    assert_eq!(late.recv().unwrap(), 1);
}
```

Use `spmc::topic` instead where `tokio::sync::broadcast::subscribe()` semantics are wanted.

## Publishing to Topics

Use `spmc::topic` for pub/sub. Each receiver has a private mailbox. A full mailbox drops new messages for that receiver and the sender never blocks. `send` takes `&self`, so one handle behind a shared struct serves many publishers.

A clone gets its own empty mailbox and the original's current subscriptions. It sees nothing sent before it subscribed.

```rust
use fibre::spmc::topic;

fn main() {
    let (tx, rx) = topic::channel::<&'static str, u32>(4);
    rx.subscribe("prices");
    let other = rx.clone();

    tx.send("prices", 1).unwrap();
    tx.send("news", 2).unwrap(); // no subscribers: discarded with Ok(())

    assert_eq!(rx.try_recv().unwrap(), ("prices", 1));
    assert_eq!(other.recv().unwrap(), ("prices", 1));

    rx.unsubscribe(&"prices");
}
```

Keep a subscription on any receiver that waits for disconnection. Dropping or closing the sender only disconnects mailboxes subscribed at that moment, so a receiver with no subscriptions blocks forever in `recv`.

## Handing Off With Rendezvous

Use `rendezvous` when a send should complete only when a receiver takes the item. Handles expose single-item `send`, `recv`, `try_*`, `recv_timeout` on sync receivers, `close` and conversions, with no batch API. SPSC handles are `!Clone`. MPSC senders are `Clone` with a single `!Clone` receiver. MPMC senders and receivers are all `Clone`.

```rust
use fibre::mpsc::rendezvous;
use std::thread;

fn main() {
    let (tx, rx) = rendezvous::rendezvous::<u32>();

    let producer = thread::spawn(move || tx.send(7).unwrap());

    assert_eq!(rx.recv().unwrap(), 7);
    producer.join().unwrap();
}
```

## Sending One Value

Prefer `exclusive()` for plain request/response: the send is one slot write plus one atomic flag update and the receiver takes the value without a claim cycle. Use `oneshot()` when several candidate senders race to fulfil one slot: only the first `send` across all clones succeeds.

Every oneshot receiver (`Receiver`, `ExclusiveReceiver`, `PooledReceiver`, `HostReceiver`) also offers `recv_blocking()` for synchronous callers. It parks only when the value has not arrived yet and allocates the parker only then, so a value already in the slot costs the same as `try_recv`. Sends need no sync/async split because a oneshot send never waits.

```rust
use fibre::oneshot;

fn main() {
    let (tx, rx) = oneshot::oneshot::<u32>();
    let second = tx.clone();

    tx.send(1).unwrap();
    assert!(second.send(2).is_err());

    assert_eq!(rx.recv_blocking().unwrap(), 1);
}
```

### Recycling Channels With a Pool

`OneshotPairPool::pair()` is the zero-channel-allocation path for high-rate request/response. The pool owns recycled slot storage. `pair()` pops a slot (`None` on exhaustion), the handles keep the `exclusive()` contract and whichever handle finishes last pushes the slot back, so per-channel cost is a freelist pop and push instead of an allocation and free. `pair_batch(n)` amortizes creation to one freelist operation. Prefer a pool once oneshot creation rate matters. A pool outlives its channels safely because release defers to the last retire.

```rust
use fibre::oneshot;

fn main() {
    let pool = oneshot::pair_pool::<u32>(8);

    let (tx, mut rx) = pool.pair().expect("a free slot");
    tx.send(7).unwrap();
    assert_eq!(rx.recv_blocking().unwrap(), 7);
}
```

### Pooling Whole Request Records

`OneshotHostPool::pair_init` is the zero-allocation path for the full request, not just the channel. Each pool cell is a whole caller record with a `PoolSlot<T>` reply channel embedded, located by the projection given at construction. `pair_init` pops the record and its channel as one unit, runs `init` on the record exclusively and `HostReceiver::host()` reads it for the channel's lifetime. On record-carrying workloads this measures about twice the plain pool with a per-request record allocation ([data](./docs/benches/oneshot_pool.md)); on recordless workloads the two pools measure identically. `pair_init_batch(n, init)` is the batch form.

```rust
use fibre::oneshot::{OneshotHostPool, PoolSlot};

struct Request {
    id: u64,
    reply: PoolSlot<u64>,
}

fn main() {
    let pool = OneshotHostPool::new(
        4,
        || Request { id: 0, reply: PoolSlot::new() },
        |r| &r.reply,
    );

    let (tx, mut rx) = pool.pair_init(|r| r.id = 7).expect("a free cell");
    assert_eq!(rx.host().id, 7);

    tx.send(rx.host().id * 2).unwrap();
    assert_eq!(rx.recv_blocking().unwrap(), 14);
}
```

## Sending in Batches

All core channels (`spsc`, `mpsc`, `spmc`, `mpmc`) provide a uniform batch API that amortizes synchronization over the whole batch: a single atomic index update or lock acquisition, bulk capacity claims and coalesced wakeups. `oneshot` and `spmc::topic` do not provide batch operations.

### Using the Eight Operations

| Operation | Input | Success | Failure |
| :--- | :--- | :--- | :--- |
| `try_send_batch(items)` | `Vec<T>` (by value) | `Ok(n)` - **all** items sent | `TrySendBatchError { sent, unsent, reason }` |
| `send_batch(items)` | `Vec<T>` (by value) | `Ok(n)` - all items sent (blocks/awaits) | `SendBatchError { sent, unsent }` (channel closed) |
| `try_send_batch_mut(&mut items)` | `&mut Vec<T>` | `Ok(k)` - `k` items drained from the **front** | `SendError::Closed` only if closed with zero sent |
| `send_batch_mut(&mut items)` | `&mut Vec<T>` | `Ok(n)` - all sent (blocks/awaits) | `SendError::Closed`; unsent items remain in the vec |
| `try_recv_batch(max)` | limit | `Ok(Vec<T>)` - 1..=max items | `TryRecvError::Empty` / `Disconnected` |
| `recv_batch(max)` | limit | `Ok(Vec<T>)` - 1..=max items (blocks/awaits) | `RecvError::Disconnected` |
| `try_recv_batch_mut(&mut out, max)` | buffer + limit | `Ok(k)` - `k` items appended to `out` | `TryRecvError::Empty` / `Disconnected` |
| `recv_batch_mut(&mut out, max)` | buffer + limit | `Ok(k)` - appended (blocks/awaits) | `RecvError::Disconnected` |

### Relying on Batch Semantics

*   **Partial progress, no drops on an error return.** By-value sends return the count sent *and* the unsent remainder on interruption (`.into_unsent()` recovers the items). In-place sends drain only what was actually sent.
*   **Blocking/async `recv_batch(max)` waits for at least one item**, then drains up to `max` without further waiting: the latency-friendly semantic for burst draining.
*   **Empty input or `max == 0`** returns immediately with `Ok(0)` / an empty vector on an open channel. On a closed channel `mpmc::Sender::send_batch`, `mpmc::Receiver::recv_batch`, `mpmc::Receiver::recv_batch_mut` and `spsc::BoundedSyncSender::send_batch` return their closed error instead.
*   **Rendezvous channels** (`spsc`/`mpsc`/`mpmc::rendezvous`) do **not** provide batch operations. The buffered `bounded`/`unbounded` constructors are the ones that offer batch.
*   **Unbounded batch sends never block.** Bounded batch sends claim runs of free capacity. SPMC batch sends are bounded by the slowest consumer and every consumer receives (clones) every item. SPSC wakeups are coalesced to one per batch.
*   **Cancellation.** Dropping a pending by-value async send-batch future drops the unsent remainder (consistent with the single-item send futures). The `_mut` send futures are cancel-safe: unsent items remain in your vector. All batch receive futures are cancel-safe.

### Batching With Bounded MPSC

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

### Batching With Async MPMC

```rust
use fibre::mpmc;

#[tokio::main]
async fn main() {
    let (tx, rx) = mpmc::bounded_async::<String>(32);

    tokio::spawn(async move {
        let batch: Vec<String> = (0..100).map(|i| format!("job-{i}")).collect();
        // Resolves once every item is sent.
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

## Cancelling a Pending Send or Receive

Dropping a pending single-item future is safe on the bounded and unbounded channels, SPMC, `spmc::topic` and `oneshot`. A dropped receive leaves the message for the next receive. A dropped send has not sent. The losing branch of `select!` drops its future this way.

```rust
use std::time::Duration;

#[tokio::main]
async fn main() {
    let (tx, rx) = fibre::mpsc::bounded_async::<u32>(4);

    tokio::select! {
        _ = rx.recv() => unreachable!("nothing has been sent yet"),
        _ = tokio::time::sleep(Duration::from_millis(10)) => {}
    }
    tx.send(7).await.unwrap();
    assert_eq!(rx.recv().await.unwrap(), 7);

    let (tx, rx) = fibre::mpsc::bounded_async::<u32>(1);
    tx.send(1).await.unwrap();
    tokio::select! {
        _ = tx.send(2) => unreachable!("the channel is full"),
        _ = tokio::time::sleep(Duration::from_millis(10)) => {}
    }
    assert_eq!(rx.recv().await.unwrap(), 1);
    assert!(rx.try_recv().is_err());
}
```

Where a batch send may be cancelled, use `send_batch_mut`: the items it has not sent stay in your vector. Dropping a pending `send_batch` drops its unsent items.

## Shutting Down a Channel

Fibre has a two-sided disconnection contract, with different behaviour depending on which side of the channel is closed. For a graceful shutdown, close or drop your senders so receivers can drain the remaining work. Closing a receiver while items are still in the buffer abandons those items.

### Draining After Senders Close

When all senders are dropped or explicitly closed, the channel becomes disconnected. Items already buffered are still readable. Receivers drain them normally and only observe `Disconnected` (or `None` on a `Stream`) once the buffer is empty.

```rust
use fibre::error::TryRecvError;

fn main() {
    let (tx, rx) = fibre::mpmc::bounded(4);
    tx.send(1).unwrap();
    tx.send(2).unwrap();
    drop(tx); // channel is now disconnected, but 1 and 2 are still buffered

    assert_eq!(rx.try_recv(), Ok(1));  // still drains
    assert_eq!(rx.try_recv(), Ok(2));  // still drains
    assert_eq!(rx.try_recv(), Err(TryRecvError::Disconnected)); // now empty
}
```

A topic receiver with no subscriptions is the exception: it never observes `Disconnected`.

### Stopping a Receiver

A closed receiver handle returns `Disconnected` from `try_recv` and blocking `recv` without draining. Other receiver handles on the same channel are unaffected. These calls still drain what is buffered: `recv_timeout` on `mpmc::Receiver` and `mpsc::BoundedSyncReceiver`, `recv`, `recv_batch` and `recv_batch_mut` on `mpmc::AsyncReceiver` and every receive on a topic receiver.

```rust
use fibre::error::TryRecvError;

fn main() {
    let (tx, rx) = fibre::mpmc::bounded::<u32>(4);
    tx.send(1).unwrap();

    rx.close().unwrap();
    assert_eq!(rx.try_recv(), Err(TryRecvError::Disconnected));
    assert!(tx.send(2).is_err());
}
```

### Closing Handles Explicitly

All channel handles provide a `close()` method as an explicit alternative to `drop`. Calling it twice returns `CloseError` on the handles whose `close` returns a `Result`. Channels signal disconnection when handles are dropped or explicitly closed and buffered items are deallocated.

`close()` is per handle. On `mpmc`, a sender's `close()` makes that handle's later sends fail with `Closed` and wakes receivers only when the last sender goes. It does not stop other sender handles. There is no channel-wide close that fails pending sends while receivers keep draining.

```rust
fn main() {
    let (tx, rx) = fibre::mpmc::bounded::<u32>(4);
    let other = tx.clone();

    tx.close().unwrap();
    assert!(tx.close().is_err());
    assert!(tx.send(1).is_err());

    other.send(2).unwrap();
    assert_eq!(rx.recv().unwrap(), 2);
}
```

## Sharing a Lock Between Threads and Tasks

`fibre::sync::HybridMutex` and `HybridRwLock` serve blocking callers and futures on one lock at the same time.

```rust
use fibre::sync::HybridMutex;
use std::sync::Arc;

#[tokio::main]
async fn main() {
    let counter = Arc::new(HybridMutex::new(0u64));

    let thread_counter = Arc::clone(&counter);
    let blocking = std::thread::spawn(move || {
        *thread_counter.lock() += 1;
    });

    let task_counter = Arc::clone(&counter);
    let task = tokio::spawn(async move {
        let mut guard = task_counter.lock_async().await;
        tokio::task::yield_now().await; // a guard may cross an .await
        *guard += 1;
    });

    task.await.unwrap();
    blocking.join().unwrap();
    assert_eq!(*counter.lock(), 2);
}
```

### Understanding How the Locks Wait

Both locks share one FIFO wait list and one contention strategy. An acquisition spins with `yield_now` before it queues. A caller that arrives while the lock is free takes it without queueing (barging). An unlock releases the lock first and then wakes waiters to contend again. Ownership is never handed to a waiter, so a wake never depends on a suspended task running.

A guard is a reference to the lock, so a task may hold one across a suspension. A task suspended while holding a guard still blocks conflicting waiters and a blocking call on its executor thread can deadlock.

The read-write lock is write-preferring so that a stream of readers cannot starve a writer.

### Using the Read-Biased Lock

`fibre::sync::v2::HybridRwLock` has the same names and methods. Reach for it when reads dominate: readers normally count themselves in per-thread slots on separate cache lines, while each lock allocates all 64 slots. A writer waiting for slot readers spins and yields rather than parks, so avoid holding its read guards across an `.await` where writers are expected. Its `try_write` can wait for slot readers and hangs if the calling thread holds a read guard.

```rust
use fibre::sync::v2::HybridRwLock;

fn main() {
    let lock = HybridRwLock::new(vec![1, 2, 3]);

    assert_eq!(lock.read().len(), 3);
    lock.write().push(4);
    assert_eq!(*lock.read(), vec![1, 2, 3, 4]);
}
```

## Turning Off the Topic Feature

`spmc::topic` is compiled by the `topic` feature, which is on by default and pulls in `papaya`. Turn it off with `default-features = false`, then read back what is enabled with `cargo tree`.

```toml
[dependencies]
fibre = { version = "0", default-features = false }
```

```bash
cargo tree -e features -i fibre
```

## Recording Telemetry

The `diagnostics` feature is off by default and pulls in `tokio`. When it is enabled, the SPMC channel logs internal events such as when threads park or wake. No other channel emits telemetry. Without the feature the telemetry calls are no-ops. Use it to diagnose deadlocks and performance issues.

```toml
[dependencies]
fibre = { version = "0", features = ["diagnostics"] }
```

```rust
use fibre::spmc;

fn main() {
    let (tx, rx) = spmc::bounded::<u32>(4);
    tx.send(1).unwrap();
    assert_eq!(rx.recv().unwrap(), 1);

    fibre::telemetry::print_telemetry_report();
    fibre::telemetry::print_stall_report();
}
```

```bash
# Run tests with telemetry enabled
cargo nextest run --features diagnostics

# Call fibre::telemetry::print_telemetry_report() to print the collected events.
```

## Measuring Performance

Benchmarks are criterion targets declared in `Cargo.toml`. Full results are in [`docs/benches/`](./docs/benches/).

```bash
cargo bench -p fibre --bench mpsc_bounded
```

### Reading the Results

**Performance highlights** (Apple M4 Pro) - **SPSC** 44 / 183 Melem/s (sync/async, Cap-1024, 1P / 1C; batch up to ~185 Melem/s) · **MPSC** ~37 / ~60-63 Melem/s (sync/async, Cap-128, 4-14P / 1C) · **SPMC** 15-20 Melem/s broadcast (Cap-128, 1P / 1-4C) · **SPMC Topic** up to 15 Melem/s async (1P / 14C) · **MPMC** 22 / 73 Melem/s (sync/async, Cap-128, 1P / 1C) · **Oneshot** ~45 Melem/s async, `exclusive()` ~55 (~98 transfer-only), `pair_pool()` ~145 (~199 transfer-only)

| Channel | Configuration | Sync | Async |
| :--- | :--- | ---: | ---: |
| **SPSC** | Cap-1024, 1P / 1C | 44.5 Melem/s | 183 Melem/s |
| **SPSC** | Cap-1024, 1P / 1C, batch 512 | 180 Melem/s | 146 Melem/s |
| **MPSC** | Cap-128, 4P / 1C | 37.9 Melem/s | 60.2 Melem/s |
| **MPSC** | Cap-128, 14P / 1C | 37.4 Melem/s | 63.4 Melem/s |
| **SPMC** | Cap-128, 1P / 1C | 15.0 Melem/s | 14.8 Melem/s |
| **SPMC** | Cap-128, 1P / 4C | 15.7 Melem/s | 19.5 Melem/s |
| **SPMC Topic** | 1P / 1C | 18.1 Melem/s | 7.8 Melem/s |
| **SPMC Topic** | 1P / 14C | 726 Kelem/s | 14.9 Melem/s |
| **MPMC** | Cap-128, 1P / 1C | 42.1 Melem/s | 73.2 Melem/s |
| **MPMC** | Cap-128, 14P / 14C | 18.9 Melem/s | 19.9 Melem/s |
| **Oneshot** | clonable / exclusive | - | 44.6 / 54.7 Melem/s |
| **Oneshot Pool** | pair / host | - | 145.2 / 144.9 Melem/s |

- SPSC numbers are measured with a real spawned producer and consumer (thread or task) per iteration, so they include genuine cross-core synchronization; throughput scales with buffer size (Cap-128: 27 / 139 Melem/s sync/async, Cap-1024: 44.5 / 183 Melem/s). Batch APIs at Cap-1024 reach 150-185 Melem/s sync.
- MPSC throughput at Cap-128 stays level as producers are added: sync ~37-38 Melem/s at 4P / 1C and 14P / 1C (~71 at 1P / 1C), async ~60-67 Melem/s from 1P / 1C to 14P / 1C.
- SPMC and SPMC Topic are broadcast: figures are per message sent. Each message is cloned and delivered to every consumer (for Topic, every consumer subscribed to its topic).
- Oneshot figures are create+send+recv per op. Transfer-only (pre-created channels): clonable ~69.5, exclusive ~98 Melem/s, vs tokio's oneshot at ~41 full / ~70 transfer-only ([data](./docs/benches/oneshot.md)).
- Oneshot Pool figures are the same full cycle over recycled slots: no allocation per channel, ~199 Melem/s transfer-only, ~186-189 Melem/s cross-thread at 10k ops and a record-carrying request/reply through the host pool at 86 Melem/s vs 42 with a per-request allocation ([data](./docs/benches/oneshot_pool.md)).
- SPMC Topic's async 1P / 14C result (14.9 Melem/s) is ~20× faster than the sync equivalent (726 Kelem/s) because the non-blocking sender fully decouples producers from slow consumers.
- MPMC async at 73 Melem/s (Cap-128, 1P / 1C) avoids lock overhead when the buffer has slack.

### Understanding the Design

Each channel type uses an algorithm chosen for its pattern:

*   Lock-free ring buffers for SPSC.
*   Lock-free linked lists for unbounded MPSC and an array-based buffer for bounded MPSC.
*   A specialized ring buffer for SPMC that tracks individual consumer progress, ensuring backpressure from the slowest consumer.
*   A non-blocking, topic-based SPMC variant built on a concurrent hash map, left-right subscriber lists and individual mailboxes for high-performance pub/sub.
*   One `fibre::sync::HybridMutex` per bounded MPMC channel, with waiter queues for sync and async callers.
*   Cache-line padding on critical atomic data to minimize false sharing and maximize throughput on multi-core systems.

## Error Handling

Fibre uses a clear set of error types to signal the result of channel operations, in `fibre::error` and re-exported from the crate root.

*   **`TrySendError<T>`:** Returned from `try_send`.
    *   `Full(T)`: The channel is full. The unsent item is returned.
    *   `Closed(T)`: The receiver was dropped or this handle was closed. The unsent item is returned.
    *   `Sent(T)`: (Oneshot only) A value was already sent.
    *   Use `.into_inner()` to recover the value.
*   **`SendError`:** Returned from blocking/async `send`.
    *   `Closed`: The receiver was dropped or this handle was closed.
    *   `Sent`: (Oneshot only) A value was already sent.
*   **`TryRecvError`:** Returned from `try_recv`.
    *   `Empty`: The channel is currently empty but not disconnected.
    *   `Disconnected`: Either all senders have been dropped and the buffer is fully drained or this receiver handle has been explicitly closed.
*   **`RecvError`:** Returned from blocking/async `recv`.
    *   `Disconnected`: The channel is empty and all senders have been dropped or this receiver handle was closed.
*   **`RecvErrorTimeout`:** Returned from `recv_timeout`.
    *   `Timeout`: The operation timed out before a value was received.
    *   `Disconnected`: The channel became disconnected during the wait.
*   **`TrySendBatchError<T>`:** Returned from `try_send_batch`. Carries partial-progress state.
    *   `sent: usize`: How many items were sent before the operation stopped.
    *   `unsent: Vec<T>`: The unsent remainder, in order. Recover it with `.into_unsent()`.
    *   `reason: BatchSendErrorReason`: `Full` or `Closed`.
*   **`SendBatchError<T>`:** Returned from blocking/async `send_batch`. The only cause is channel closure.
    *   `sent: usize` and `unsent: Vec<T>`, as above. Recover the remainder with `.into_unsent()`.
*   **`CloseError`:** Returned when `close()` is called on an already-closed handle.

```rust
use fibre::error::{RecvErrorTimeout, TryRecvError, TrySendError};
use fibre::mpmc;
use std::time::Duration;

fn main() {
    let (tx, rx) = mpmc::bounded::<u32>(1);
    tx.send(1).unwrap();

    match tx.try_send(2) {
        Ok(()) => println!("sent"),
        Err(TrySendError::Full(value)) => println!("full, {value} comes back"),
        Err(TrySendError::Closed(value)) => println!("closed, {value} comes back"),
        Err(TrySendError::Sent(_)) => unreachable!("oneshot only"),
    }

    match rx.try_recv() {
        Ok(value) => println!("got {value}"),
        Err(TryRecvError::Empty) => println!("nothing yet"),
        Err(TryRecvError::Disconnected) => println!("finished"),
    }

    match rx.recv_timeout(Duration::from_millis(1)) {
        Ok(value) => println!("got {value}"),
        Err(RecvErrorTimeout::Timeout) => println!("timed out"),
        Err(RecvErrorTimeout::Disconnected) => println!("finished"),
    }
}
```
