# `fibre` API Reference

## Introduction & Core Concepts

Fibre is a high-performance concurrency library for Rust, providing a suite of channel types optimized for various communication patterns. It is designed with a focus on performance, memory efficiency, and ergonomic APIs for both synchronous and asynchronous programming.

### Core Architectural Principles

*   **Specialized Implementations:** Instead of a single general-purpose channel, Fibre provides distinct implementations for SPSC (Single-Sender, Single-Receiver), MPSC, SPMC, MPMC, and Oneshot patterns. This allows developers to choose the most efficient algorithm for their specific use case.
*   **Hybrid Sync/Async Operation:** A key feature of the MPSC, SPMC, MPMC, and SPSC channels is their ability to support mixed-paradigm usage. A synchronous handle (e.g., `mpmc::Sender`) can communicate with an asynchronous handle (`mpmc::AsyncReceiver`) on the same channel. This is facilitated by zero-cost `to_sync()` and `to_async()` conversion methods on the handles.
*   **Performance Focus:** The library uses high-performance algorithms, including lock-free ring buffers (SPSC, SPMC), lock-free linked lists (MPSC), and high-performance mutexes from `parking_lot` (MPMC). The use of cache-line padding on critical atomic variables further minimizes contention in multi-core scenarios.
*   **Idiomatic Error Handling:** All fallible operations return a `Result`, and the error types are designed to be descriptive and ergonomic. For instance, `TrySendError<T>` returns the item that failed to be sent, preventing data loss.

### Primary Handles

Interaction with the library is primarily through sender and receiver handles, which are created by top-level functions in each module (e.g., `fibre::mpmc::channel()`).

*   **`Sender` / `Sender`:** These handles are used to send data into a channel. Depending on the channel type, they may be clonable to allow for multiple producers.
*   **`Receiver` / `Receiver`:** These handles are used to receive data from a channel. Depending on the channel type, they may be clonable to allow for multiple consumers.
*   **`Async` Variants:** Each handle type generally has an `Async` counterpart (e.g., `AsyncSender`, `AsyncReceiver`) for use in `async` code.

---

## Error Handling

The library uses a set of descriptive enum-based errors for fallible operations. All blocking or potentially failing operations return a standard `Result<T, E>`.

### Enum `error::TrySendError<T>`

Returned by non-blocking `try_send` operations.

*   **Variants:**
    *   `Full(T)`: The channel is full and cannot accept more items. The unsent item is returned.
    *   `Closed(T)`: The channel is closed because all receivers have been dropped. The unsent item is returned.
    *   `Sent(T)`: (Oneshot channels only) A value has already been sent on this channel. The unsent item is returned.
*   **Methods:**
    *   `pub fn into_inner(self) -> T`: Consumes the error, returning the inner value that failed to be sent.

### Enum `error::SendError`

Returned by blocking or asynchronous `send` operations.

*   **Variants:**
    *   `Closed`: The channel is closed because all receivers have been dropped.
    *   `Sent`: (Oneshot channels only) A value has already been sent on this channel.

### Enum `error::TryRecvError`

Returned by non-blocking `try_recv` operations.

*   **Variants:**
    *   `Empty`: The channel is currently empty but not disconnected.
    *   `Disconnected`: The channel is empty, and all senders have been dropped.

### Enum `error::RecvError`

Returned by blocking or asynchronous `recv` operations.

*   **Variants:**
    *   `Disconnected`: The channel is empty, and all senders have been dropped.

### Enum `error::RecvErrorTimeout`

Returned by `recv_timeout` operations.

*   **Variants:**
    *   `Disconnected`: The channel is empty, and all senders have been dropped.
    *   `Timeout`: The timeout elapsed before an item could be received.

### Struct `error::CloseError`

Returned when attempting to close an already closed channel. (Currently not used by any public channel API but defined for future use).

---

## API Reference by Module

### Module `mpmc` (Multi-Sender, Multi-Receiver)

A flexible, lock-based channel where many producers can send to many consumers. Supports both bounded and "unbounded" (memory-limited) capacities, as well as rendezvous channels (capacity 0). `T` must be `Send`.

#### **Functions**

*   `pub fn channel<T: Send>(capacity: usize) -> (Sender<T>, Receiver<T>)`
    *   Creates a new synchronous, bounded MPMC channel.
*   `pub fn unbounded<T: Send>() -> (Sender<T>, Receiver<T>)`
    *   Creates a new synchronous, "unbounded" MPMC channel.
*   `pub fn channel_async<T: Send>(capacity: usize) -> (AsyncSender<T>, AsyncReceiver<T>)`
    *   Creates a new asynchronous, bounded MPMC channel.
*   `pub fn unbounded_async<T: Send>() -> (AsyncSender<T>, AsyncReceiver<T>)`
    *   Creates a new asynchronous, "unbounded" MPMC channel.

#### **Struct `mpmc::Sender<T: Send>`**

A synchronous sending handle. It is `Clone`.

*   `pub fn send(&self, item: T) -> Result<(), SendError>`
*   `pub fn try_send(&self, item: T) -> Result<(), TrySendError<T>>`
*   `pub fn is_closed(&self) -> bool`
*   `pub fn capacity(&self) -> Option<usize>`
*   `pub fn to_async(self) -> AsyncSender<T>`

#### **Struct `mpmc::Receiver<T: Send>`**

A synchronous receiving handle. It is `Clone`.

*   `pub fn recv(&self) -> Result<T, RecvError>`
*   `pub fn try_recv(&self) -> Result<T, TryRecvError>`
*   `pub fn is_closed(&self) -> bool`
*   `pub fn capacity(&self) -> Option<usize>`
*   `pub fn to_async(self) -> AsyncReceiver<T>`

#### **Struct `mpmc::AsyncSender<T: Send>`**

An asynchronous sending handle. It is `Clone`.

*   `pub fn send(&self, item: T) -> SendFuture<'_, T>`
    *   Requires `T: Send`.
*   `pub fn try_send(&self, item: T) -> Result<(), TrySendError<T>>`
*   `pub fn is_closed(&self) -> bool`
*   `pub fn capacity(&self) -> Option<usize>`
*   `pub fn to_sync(self) -> Sender<T>`

#### **Struct `mpmc::AsyncReceiver<T: Send>`**

An asynchronous receiving handle. It is `Clone`.

*   `pub fn recv(&self) -> ReceiveFuture<'_, T>`
    *   Requires `T: Send`.
*   `pub fn try_recv(&self) -> Result<T, TryRecvError>`
*   `pub fn is_closed(&self) -> bool`
*   `pub fn capacity(&self) -> Option<usize>`
*   `pub fn to_sync(self) -> Receiver<T>`

#### **Future Types**

*   `pub struct SendFuture<'a, T: Send>` implements `Future<Output = Result<(), SendError>>`.
*   `pub struct ReceiveFuture<'a, T: Send>` implements `Future<Output = Result<T, RecvError>>`.

---

### Module `mpsc` (Multi-Sender, Single-Receiver)

A highly optimized lock-free channel where many producers can send to a single consumer. `T` must be `Send`.

#### **Functions**

*   `pub fn channel<T: Send>() -> (Sender<T>, Receiver<T>)`
*   `pub fn channel_async<T: Send>() -> (AsyncSender<T>, AsyncReceiver<T>)`

#### **Struct `mpsc::Sender<T: Send>`**

A synchronous sending handle. It is `Clone`.

*   `pub fn send(&self, value: T) -> Result<(), SendError>`
*   `pub fn to_async(self) -> AsyncSender<T>`

#### **Struct `mpsc::Receiver<T: Send>`**

A synchronous receiving handle. It is not `Clone`.

*   `pub fn recv(&mut self) -> Result<T, RecvError>`
*   `pub fn try_recv(&mut self) -> Result<T, TryRecvError>`
*   `pub fn to_async(self) -> AsyncReceiver<T>`

#### **Struct `mpsc::AsyncSender<T: Send>`**

An asynchronous sending handle. It is `Clone`.

*   `pub fn send(&self, value: T) -> SendFuture<'_, T>`
    *   Requires `T: Send`.
*   `pub fn to_sync(self) -> Sender<T>`

#### **Struct `mpsc::AsyncReceiver<T: Send>`**

An asynchronous receiving handle. It is not `Clone`.

*   `pub fn recv(&mut self) -> RecvFuture<'_, T>`
*   `pub fn try_recv(&mut self) -> Result<T, TryRecvError>`
*   `pub fn to_sync(self) -> Receiver<T>`

#### **Future Types**

*   `pub struct SendFuture<'a, T: Send>` implements `Future<Output = Result<(), SendError>>`.
*   `pub struct RecvFuture<'a, T: Send>` implements `Future<Output = Result<T, RecvError>>`.

---

### Module `spmc` (Single-Sender, Multi-Receiver)

A "broadcast" channel where a single producer sends messages to many consumers. Each message is cloned for each consumer. `T` must be `Send + Clone`.

#### **Functions**

*   `pub fn channel<T: Send + Clone>(capacity: usize) -> (Sender<T>, Receiver<T>)`
    *   Panics if `capacity` is 0.
*   `pub fn channel_async<T: Send + Clone>(capacity: usize) -> (AsyncSender<T>, AsyncReceiver<T>)`
    *   Panics if `capacity` is 0.

#### **Struct `spmc::Sender<T: Send + Clone>`**

A synchronous sending handle. It is not `Clone`.

*   `pub fn send(&mut self, value: T) -> Result<(), SendError>`
*   `pub fn to_async(self) -> AsyncSender<T>`

#### **Struct `spmc::Receiver<T: Send + Clone>`**

A synchronous receiving handle. It is `Clone`.

*   `pub fn recv(&mut self) -> Result<T, RecvError>`
*   `pub fn try_recv(&mut self) -> Result<T, TryRecvError>`
*   `pub fn to_async(self) -> AsyncReceiver<T>`

#### **Struct `spmc::AsyncSender<T: Send + Clone>`**

An asynchronous sending handle. It is not `Clone`.

*   `pub fn send(&mut self, value: T) -> SendFuture<'_, T>`
    *   Requires `T: Send + Clone`.
*   `pub fn to_sync(self) -> Sender<T>`

#### **Struct `spmc::AsyncReceiver<T: Send + Clone>`**

An asynchronous receiving handle. It is `Clone`.

*   `pub fn recv(&mut self) -> RecvFuture<'_, T>`
*   `pub fn try_recv(&mut self) -> Result<T, TryRecvError>`
*   `pub fn to_sync(self) -> Receiver<T>`

#### **Future Types**

*   `pub struct SendFuture<'a, T: Send + Clone>` implements `Future<Output = Result<(), SendError>>`.
*   `pub struct RecvFuture<'a, T: Send + Clone>` implements `Future<Output = Result<T, RecvError>>`.

---

### Module `spsc` (Single-Sender, Single-Receiver)

The fastest channel type, optimized for 1-to-1 communication using a lock-free ring buffer. `T` must be `Send`.

#### **Functions**

*   `pub fn bounded_sync<T: Send>(capacity: usize) -> (BoundedSyncSender<T>, BoundedSyncReceiver<T>)`
    *   Panics if `capacity` is 0.
*   `pub fn bounded_async<T: Send>(capacity: usize) -> (AsyncBoundedSpscSender<T>, AsyncBoundedSpscReceiver<T>)`
    *   Panics if `capacity` is 0.

#### **Struct `spsc::BoundedSyncSender<T: Send>`**

A synchronous sending handle. It is not `Clone`.

*   `pub fn send(&mut self, item: T) -> Result<(), SendError>`
*   `pub fn try_send(&mut self, item: T) -> Result<(), TrySendError<T>>`
*   `pub fn to_async(self) -> AsyncBoundedSpscSender<T>`

#### **Struct `spsc::BoundedSyncReceiver<T: Send>`**

A synchronous receiving handle. It is not `Clone`.

*   `pub fn recv(&mut self) -> Result<T, RecvError>`
*   `pub fn recv_timeout(&mut self, timeout: Duration) -> Result<T, RecvErrorTimeout>`
*   `pub fn try_recv(&mut self) -> Result<T, TryRecvError>`
*   `pub fn to_async(self) -> AsyncBoundedSpscReceiver<T>`

#### **Struct `spsc::AsyncBoundedSpscSender<T: Send>`**

An asynchronous sending handle. It is not `Clone`.

*   `pub fn send(&self, item: T) -> SendFuture<'_, T>`
    *   Requires `T: Send`.
*   `pub fn try_send(&self, item: T) -> Result<(), TrySendError<T>>`
*   `pub fn to_sync(self) -> BoundedSyncSender<T>`

#### **Struct `spsc::AsyncBoundedSpscReceiver<T: Send>`**

An asynchronous receiving handle. It is not `Clone`.

*   `pub fn recv(&self) -> ReceiveFuture<'_, T>`
    *   Requires `T: Send`.
*   `pub fn try_recv(&self) -> Result<T, TryRecvError>`
*   `pub fn to_sync(self) -> BoundedSyncReceiver<T>`

#### **Future Types**

*   `pub struct SendFuture<'a, T: Send>` implements `Future<Output = Result<(), SendError>>`.
*   `pub struct ReceiveFuture<'a, T: Send>` implements `Future<Output = Result<T, RecvError>>`.

---

### Module `oneshot`

A channel for sending a single value from one of potentially many senders to a single receiver.

#### **Functions**

*   `pub fn channel<T>() -> (Sender<T>, Receiver<T>)`

#### **Struct `oneshot::Sender<T>`**

The sending side of a oneshot channel. It is `Clone`.

*   `pub fn send(self, value: T) -> Result<(), TrySendError<T>>`
*   `pub fn is_closed(&self) -> bool`
*   `pub fn is_sent(&self) -> bool`

#### **Struct `oneshot::Receiver<T>`**

The receiving side of a oneshot channel. It is not `Clone`.

*   `pub fn recv(&mut self) -> ReceiveFuture<'_, T>`
*   `pub fn try_recv(&mut self) -> Result<T, TryRecvError>`
*   `pub fn is_closed(&self) -> bool`

#### **Future Types**

*   `pub struct ReceiveFuture<'a, T>` implements `Future<Output = Result<T, RecvError>>`.
