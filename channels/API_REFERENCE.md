# API Reference: `fibre`

## 1. Core Concepts

`fibre` is a library of high-performance, memory-efficient, and flexible channels for concurrent programming in Rust. It provides specialized implementations for common producer/consumer patterns.

*   **Specialized Channels**: The library offers distinct channel types, each optimized for a specific concurrency pattern to ensure maximum performance and low overhead:
    *   `spsc`: **S**ingle-**P**roducer, **S**ingle-**C**onsumer. The fastest channel for 1-to-1 communication, implemented with a lock-free ring buffer.
    *   `mpsc`: **M**ulti-**P**roducer, **S**ingle-**C**onsumer. Ideal for collecting work from many sources into one processor. Supports both bounded and unbounded modes.
    *   `spmc`: **S**ingle-**P**roducer, **M**ulti-**C**onsumer. A "broadcast" or "fan-out" channel where one producer sends cloned messages to many consumers. It also includes a `topic`-based pub/sub variant.
    *   `mpmc`: **M**ulti-**P**roducer, **M**ulti-**C**onsumer. The most flexible channel for many-to-many communication, supporting both bounded and unbounded modes.
    *   `oneshot`: A channel for sending a single value, exactly once.

*   **Hybrid Sync/Async Model**: A core feature of `fibre` is the seamless interoperability between synchronous (`std::thread`) and asynchronous (`tokio`) code. All `Sender` and `Receiver` handles provide `to_sync()` or `to_async()` methods that perform a zero-cost conversion. This allows, for example, a synchronous thread to send data to an asynchronous task on the same channel.

*   **Sender and Receiver Handles**: Interaction with channels is done through `Sender` and `Receiver` handles. These handles control access and lifetime. When all `Sender` handles for a channel are dropped, it becomes "disconnected." When all `Receiver` handles are dropped, it becomes "closed." Handle cloning semantics vary by channel type (e.g., `mpmc::Sender` is `Clone`, but `spsc::BoundedSyncSender` is not).

*   **Stream API**: All asynchronous receivers that can yield multiple items (`mpmc::AsyncReceiver`, `mpsc::UnboundedAsyncReceiver`, `mpsc::BoundedAsyncReceiver`, `spmc::AsyncReceiver`, `spsc::AsyncBoundedSpscReceiver`, `spmc::topic::AsyncTopicReceiver`) implement the `futures::Stream` trait, allowing them to be used with the rich combinator library from `futures-util`.

## 2. Error Handling

`fibre` uses a consistent set of error types to signal the outcome of channel operations.

### `TrySendError<T>`

Returned by non-blocking `try_send` methods.

*   **Enum Variants**:
    *   `Full(T)`: The channel is full and cannot accept an item.
    *   `Closed(T)`: The channel is closed because the receiver(s) have dropped.
    *   `Sent(T)`: (Oneshot only) A value has already been sent on this channel.
*   **Methods**:
    *   `pub fn into_inner(self) -> T`: Consumes the error, returning the inner value that could not be sent.

### `SendError`

Returned by blocking or `async` `send` methods.

*   **Enum Variants**:
    *   `Closed`: The channel is closed because the receiver(s) have dropped.
    *   `Sent`: (Oneshot only) A value has already been sent.

### `TryRecvError`

Returned by non-blocking `try_recv` methods.

*   **Enum Variants**:
    *   `Empty`: The channel is currently empty but still active.
    *   `Disconnected`: The channel is empty and all senders have dropped.

### `RecvError`

Returned by blocking or `async` `recv` methods.

*   **Enum Variants**:
    *   `Disconnected`: The channel is empty and all senders have dropped.

### `RecvErrorTimeout`

Returned by `recv_timeout` methods.

*   **Enum Variants**:
    *   `Disconnected`: The channel became disconnected during the wait.
    *   `Timeout`: The timeout elapsed before an item could be received.

### `CloseError`

A unit-like struct returned when `close()` is called on an already-closed handle.

## 3. Module `fibre::oneshot`

A channel for sending a single value from one of potentially many senders to a single receiver.

### Functions

*   `pub fn oneshot<T>() -> (Sender<T>, Receiver<T>)`

### Struct `Sender<T>`

The sending side of a oneshot channel. Can be cloned. `send` consumes the handle.

*   **Methods**:
    *   `pub fn send(self, value: T) -> Result<(), TrySendError<T>>`
    *   `pub fn close(&self) -> Result<(), CloseError>`
    *   `pub fn is_closed(&self) -> bool`
    *   `pub fn is_sent(&self) -> bool`

### Struct `Receiver<T>`

The receiving side of a oneshot channel. Cannot be cloned.

*   **Methods**:
    *   `pub fn recv(&self) -> ReceiveFuture<'_, T>`
    *   `pub fn try_recv(&self) -> Result<T, TryRecvError>`
    *   `pub fn close(&self) -> Result<(), CloseError>`
    *   `pub fn is_closed(&self) -> bool`

## 4. Module `fibre::spsc`

A high-performance, lock-free, bounded channel for one producer and one consumer.

### Functions

*   `pub fn bounded_sync<T: Send>(capacity: usize) -> (BoundedSyncSender<T>, BoundedSyncReceiver<T>)`
*   `pub fn bounded_async<T: Send>(capacity: usize) -> (AsyncBoundedSpscSender<T>, AsyncBoundedSpscReceiver<T>)`

### Struct `BoundedSyncSender<T>`

The synchronous, non-cloneable sending handle.

*   **Methods**:
    *   `pub fn to_async(self) -> AsyncBoundedSpscSender<T>`
    *   `pub fn try_send(&self, item: T) -> Result<(), TrySendError<T>>`
    *   `pub fn send(&self, item: T) -> Result<(), SendError>`
    *   `pub fn close(&self) -> Result<(), CloseError>`
    *   `pub fn is_closed(&self) -> bool`
    *   `pub fn capacity(&self) -> usize`
    *   `pub fn len(&self) -> usize`
    *   `pub fn is_empty(&self) -> bool`
    *   `pub fn is_full(&self) -> bool`

### Struct `BoundedSyncReceiver<T>`

The synchronous, non-cloneable receiving handle.

*   **Methods**:
    *   `pub fn to_async(self) -> AsyncBoundedSpscReceiver<T>`
    *   `pub fn try_recv(&self) -> Result<T, TryRecvError>`
    *   `pub fn recv(&self) -> Result<T, RecvError>`
    *   `pub fn recv_timeout(&mut self, timeout: std::time::Duration) -> Result<T, RecvErrorTimeout>`
    *   `pub fn close(&self) -> Result<(), CloseError>`
    *   `is_closed`, `capacity`, `len`, `is_empty`, `is_full`

### Struct `AsyncBoundedSpscSender<T>`

The asynchronous, non-cloneable sending handle.

*   **Methods**:
    *   `pub fn to_sync(self) -> BoundedSyncSender<T>`
    *   `pub fn send(&self, item: T) -> SendFuture<'_, T>`
    *   `try_send`, `close`, `is_closed`, `capacity`, `len`, `is_empty`, `is_full`

### Struct `AsyncBoundedSpscReceiver<T>`

The asynchronous, non-cloneable receiving handle. Implements `futures::Stream`.

*   **Methods**:
    *   `pub fn to_sync(self) -> BoundedSyncReceiver<T>`
    *   `pub fn recv(&self) -> ReceiveFuture<'_, T>`
    *   `try_recv`, `close`, `is_closed`, `capacity`, `len`, `is_empty`, `is_full`

## 5. Module `fibre::mpsc`

An optimized channel for multiple producers and one consumer.

*Note: The `mpsc` module provides both bounded and unbounded channels. The types are prefixed accordingly (e.g., `UnboundedSender`, `BoundedSender`) and are all exported directly from the `mpsc` module.*

### Functions

*   `pub fn unbounded<T: Send>() -> (UnboundedSender<T>, UnboundedReceiver<T>)`
*   `pub fn unbounded_async<T: Send>() -> (UnboundedAsyncSender<T>, UnboundedAsyncReceiver<T>)`
*   `pub fn bounded<T: Send>(capacity: usize) -> (BoundedSender<T>, BoundedReceiver<T>)`
*   `pub fn bounded_async<T: Send>(capacity: usize) -> (BoundedAsyncSender<T>, BoundedAsyncReceiver<T>)`

### Unbounded MPSC Types

*   **Struct `UnboundedSender<T: Send>`**: A cloneable, sync handle for the unbounded channel.
    *   `send(&self, value: T) -> Result<(), SendError>`: Non-blocking send.
    *   Methods: `try_send`, `is_closed`, `close`, `sender_count`, `len`, `is_empty`, `to_async`.
*   **Struct `UnboundedReceiver<T: Send>`**: A non-cloneable, sync handle.
    *   `recv(&self) -> Result<T, RecvError>`: Blocks if empty.
    *   `recv_timeout(&self, timeout: std::time::Duration) -> Result<T, RecvErrorTimeout>`
    *   Methods: `try_recv`, `is_closed`, `close`, `sender_count`, `len`, `is_empty`, `to_async`.
*   **Struct `UnboundedAsyncSender<T: Send>`**: A cloneable, async handle.
    *   `send(&self, value: T) -> UnboundedSendFuture<'_, T>`: Non-blocking future.
    *   Methods: `try_send`, `is_closed`, `close`, `sender_count`, `len`, `is_empty`, `to_sync`.
*   **Struct `UnboundedAsyncReceiver<T: Send>`**: A non-cloneable, async handle. Implements `futures::Stream`.
    *   `recv(&self) -> UnboundedRecvFuture<'_, T>`: Returns a future that waits for an item.
    *   Methods: `try_recv`, `is_closed`, `close`, `sender_count`, `len`, `is_empty`, `to_sync`.

### Bounded MPSC Types

*   **Struct `BoundedSender<T: Send>`**: A cloneable, sync handle for the bounded channel.
    *   `send(&self, value: T) -> Result<(), SendError>`: Blocks if full.
    *   Methods: `try_send`, `clone`, `is_closed`, `close`, `sender_count`, `len`, `is_empty`, `capacity`, `is_full`, `to_async`.
*   **Struct `BoundedReceiver<T: Send>`**: A non-cloneable, sync handle.
    *   `recv(&self) -> Result<T, RecvError>`: Blocks if empty.
    *   `recv_timeout(&self, timeout: std::time::Duration) -> Result<T, RecvErrorTimeout>`
    *   Methods: `try_recv`, `is_closed`, `close`, `sender_count`, `len`, `is_empty`, `capacity`, `is_full`, `to_async`.
*   **Struct `BoundedAsyncSender<T: Send>`**: A cloneable, async handle.
    *   `send(&self, value: T) -> BoundedSendFuture<'_, T>`: Returns a future that waits for capacity.
    *   Methods: `try_send`, `clone`, `is_closed`, `close`, `sender_count`, `len`, `is_empty`, `capacity`, `is_full`, `to_sync`.
*   **Struct `BoundedAsyncReceiver<T: Send>`**: A non-cloneable, async handle. Implements `futures::Stream`.
    *   `recv(&self) -> BoundedRecvFuture<'_, T>`: Returns a future that waits for an item.
    *   Methods: `try_recv`, `is_closed`, `close`, `sender_count`, `len`, `is_empty`, `capacity`, `is_full`, `to_sync`.

## 6. Module `fibre::spmc`

A broadcast-style channel for one producer and multiple consumers. `T` must be `Send + Clone`.

### Functions

*   `pub fn bounded<T: Send + Clone>(capacity: usize) -> (Sender<T>, Receiver<T>)`
*   `pub fn bounded_async<T: Send + Clone>(capacity: usize) -> (AsyncSender<T>, AsyncReceiver<T>)`

### Struct `Sender<T: Send + Clone>`

The synchronous, non-cloneable sending handle.

*   **Methods**:
    *   `send(&self, value: T) -> Result<(), SendError>`: Blocks if any consumer's buffer is full.
    *   `try_send(&self, value: T) -> Result<(), TrySendError<T>>`
    *   `close(&mut self) -> Result<(), CloseError>`
    *   `to_async`, `is_closed`, `capacity`, `len`, `is_empty`, `is_full`.

### Struct `Receiver<T: Send + Clone>`

The synchronous, cloneable receiving handle.

*   **Methods**:
    *   `recv(&self) -> Result<T, RecvError>`: Blocks if this consumer's buffer is empty.
    *   `recv_timeout(&self, timeout: std::time::Duration) -> Result<T, RecvErrorTimeout>`
    *   `try_recv(&self) -> Result<T, TryRecvError>`
    *   `close(&self) -> Result<(), CloseError>`
    *   `to_async`, `is_closed`, `capacity`, `len`, `is_empty`, `is_full`.

### Struct `AsyncSender<T: Send + Clone>`

The asynchronous, non-cloneable sending handle.

*   **Methods**:
    *   `send(&self, value: T) -> SendFuture<'_, T>`
    *   `try_send`, `close(&mut self)`, `to_sync`, `is_closed`, `capacity`, `len`, `is_empty`, `is_full`.

### Struct `AsyncReceiver<T: Send + Clone>`

The asynchronous, cloneable receiving handle. Implements `futures::Stream`.

*   **Methods**:
    *   `recv(&self) -> RecvFuture<'_, T>`
    *   `try_recv`, `close`, `to_sync`, `is_closed`, `capacity`, `len`, `is_empty`, `is_full`.

## 7. Module `fibre::mpmc`

A flexible, lock-based channel for many producers and many consumers.

### Functions

*   `pub fn bounded<T: Send>(capacity: usize) -> (Sender<T>, Receiver<T>)`
*   `pub fn bounded_async<T: Send>(capacity: usize) -> (AsyncSender<T>, AsyncReceiver<T>)`
*   `pub fn unbounded<T: Send>() -> (Sender<T>, Receiver<T>)`
*   `pub fn unbounded_async<T: Send>() -> (AsyncSender<T>, AsyncReceiver<T>)`

### Struct `Sender<T: Send>`

The synchronous, cloneable sending handle.

*   **Methods**:
    *   `send(&self, item: T) -> Result<(), SendError>`: Blocks if the channel is full.
    *   `try_send(&self, item: T) -> Result<(), TrySendError<T>>`
    *   `close(&self) -> Result<(), CloseError>`
    *   `to_async`, `is_closed`, `capacity`, `len`, `is_empty`, `is_full`.

### Struct `Receiver<T: Send>`

The synchronous, cloneable receiving handle.

*   **Methods**:
    *   `recv(&self) -> Result<T, RecvError>`: Blocks if the channel is empty.
    *   `recv_timeout(&self, timeout: std::time::Duration) -> Result<T, RecvErrorTimeout>`
    *   `try_recv(&self) -> Result<T, TryRecvError>`
    *   `close(&self) -> Result<(), CloseError>`
    *   `to_async`, `is_closed`, `capacity`, `len`, `is_empty`, `is_full`.

### Struct `AsyncSender<T: Send>`

The asynchronous, cloneable sending handle.

*   **Methods**:
    *   `send(&self, item: T) -> SendFuture<'_, T>`
    *   `try_send`, `close`, `to_sync`, `is_closed`, `capacity`, `len`, `is_empty`, `is_full`.

### Struct `AsyncReceiver<T: Send>`

The asynchronous, cloneable receiving handle. Implements `futures::Stream`.

*   **Methods**:
    *   `recv(&self) -> RecvFuture<'_, T>`
    *   `try_recv`, `close`, `to_sync`, `is_closed`, `capacity`, `len`, `is_empty`, `is_full`.

## 8. Module `fibre::spmc::topic`

A multi-consumer "topic" or "publish-subscribe" channel. This channel allows a single producer to broadcast messages to multiple consumers, where each consumer subscribes to specific "topics". A message sent to a topic is delivered to all consumers subscribed to that topic.

*   **Behavior**:
    *   **Topic-Based Filtering**: Consumers only receive messages for topics they explicitly subscribe to.
    *   **Non-Blocking Sender**: The sender is not blocked by slow consumers. If a consumer's internal message buffer (mailbox) is full, new messages for that consumer are dropped, ensuring the sender and other consumers are not impacted.
    *   **`Clone` Requirement**: Since a message can be delivered to multiple subscribers, the message type `T` and topic key `K` must implement `Clone`.
    *   **Sync/Async Agnostic**: Full interoperability between sync and async code.

### Functions

*   `pub fn channel<K, T>(mailbox_capacity: usize) -> (TopicSender<K, T>, TopicReceiver<K, T>)`
*   `pub fn channel_async<K, T>(mailbox_capacity: usize) -> (AsyncTopicSender<K, T>, AsyncTopicReceiver<K, T>)`

### Struct `TopicSender<K, T>`

The synchronous, cloneable sending handle.

*   **Methods**:
    *   `pub fn send(&self, topic: K, value: T) -> Result<(), SendError>`: Non-blocking send. Drops message for slow consumers.
    *   `pub fn close(&self) -> Result<(), CloseError>`
    *   `pub fn is_closed(&self) -> bool`
    *   `pub fn to_async(self) -> AsyncTopicSender<K, T>`

### Struct `TopicReceiver<K, T>`

The synchronous, cloneable receiving handle.

*   **Methods**:
    *   `pub fn subscribe(&self, topic: K)`
    *   `pub fn unsubscribe<Q: ?Sized>(&self, topic: &Q)`
    *   `pub fn recv(&self) -> Result<(K, T), RecvError>`: Blocks if this receiver's mailbox is empty.
    *   `pub fn try_recv(&self) -> Result<(K, T), TryRecvError>`
    *   `pub fn recv_timeout(&self, timeout: std::time::Duration) -> Result<(K, T), RecvErrorTimeout>`
    *   `pub fn close(&self) -> Result<(), CloseError>`
    *   `pub fn is_closed(&self) -> bool`
    *   `pub fn capacity(&self) -> usize`: Returns the capacity of this receiver's mailbox.
    *   `pub fn is_empty(&self) -> bool`: Returns `true` if this receiver's mailbox is empty.
    *   `pub fn to_async(self) -> AsyncTopicReceiver<K, T>`

### Struct `AsyncTopicSender<K, T>`

The asynchronous, cloneable sending handle.

*   **Methods**:
    *   `pub fn send(&self, topic: K, value: T) -> Result<(), SendError>`: Non-blocking, fire-and-forget send.
    *   `pub fn close(&self) -> Result<(), CloseError>`
    *   `pub fn is_closed(&self) -> bool`
    *   `pub fn to_sync(self) -> TopicSender<K, T>`

### Struct `AsyncTopicReceiver<K, T>`

The asynchronous, cloneable receiving handle. Implements `futures::Stream`.

*   **Methods**:
    *   `pub fn subscribe(&self, topic: K)`
    *   `pub fn unsubscribe<Q: ?Sized>(&self, topic: &Q)`
    *   `pub fn recv(&self) -> RecvFuture<'_, (K, T)>`: Returns a future that waits for a message.
    *   `pub fn try_recv(&self) -> Result<(K, T), TryRecvError>`
    *   `pub fn close(&self) -> Result<(), CloseError>`
    *   `pub fn is_closed(&self) -> bool`
    *   `pub fn capacity(&self) -> usize`: Returns the capacity of this receiver's mailbox.
    *   `pub fn is_empty(&self) -> bool`: Returns `true` if this receiver's mailbox is empty.
    *   `pub fn to_sync(self) -> TopicReceiver<K, T>`