# API Reference: `fibre`

High-performance sync/async channels and hybrid locks for concurrent Rust.

## Contents

* [1. Module `fibre::oneshot`](#1-module-fibreoneshot)
    * [oneshot functions](#oneshot-functions)
    * [Struct `oneshot::Sender<T>`](#struct-oneshotsendert)
    * [Struct `oneshot::Receiver<T>`](#struct-oneshotreceivert)
    * [Struct `oneshot::ExclusiveSender<T>`](#struct-oneshotexclusivesendert)
    * [Struct `oneshot::ExclusiveReceiver<T>`](#struct-oneshotexclusivereceivert)
    * [Struct `oneshot::OneshotPairPool<T>`](#struct-oneshotoneshotpairpoolt)
    * [Struct `oneshot::PooledSender<T>`](#struct-oneshotpooledsendert)
    * [Struct `oneshot::PooledReceiver<T>`](#struct-oneshotpooledreceivert)
    * [Struct `oneshot::OneshotHostPool<H, T>`](#struct-oneshotoneshothostpoolh-t)
    * [Struct `oneshot::PoolSlot<T>`](#struct-oneshotpoolslott)
    * [Struct `oneshot::HostSender<H, T>`](#struct-oneshothostsenderh-t)
    * [Struct `oneshot::HostReceiver<H, T>`](#struct-oneshothostreceiverh-t)
    * [oneshot futures](#oneshot-futures)
* [2. Module `fibre::spsc`](#2-module-fibrespsc)
    * [spsc functions](#spsc-functions)
    * [Struct `spsc::BoundedSyncSender<T>`](#struct-spscboundedsyncsendert)
    * [Struct `spsc::BoundedSyncReceiver<T>`](#struct-spscboundedsyncreceivert)
    * [Struct `spsc::BoundedAsyncSender<T>`](#struct-spscboundedasyncsendert)
    * [Struct `spsc::BoundedAsyncReceiver<T>`](#struct-spscboundedasyncreceivert)
    * [spsc futures](#spsc-futures)
    * [Struct `spsc::rendezvous::RendezvousSyncSender<T>`](#struct-spscrendezvousrendezvoussyncsendert)
    * [Struct `spsc::rendezvous::RendezvousSyncReceiver<T>`](#struct-spscrendezvousrendezvoussyncreceivert)
    * [Struct `spsc::rendezvous::RendezvousAsyncSender<T>`](#struct-spscrendezvousrendezvousasyncsendert)
    * [Struct `spsc::rendezvous::RendezvousAsyncReceiver<T>`](#struct-spscrendezvousrendezvousasyncreceivert)
    * [spsc rendezvous futures](#spsc-rendezvous-futures)
* [3. Module `fibre::mpsc`](#3-module-fibrempsc)
    * [mpsc functions](#mpsc-functions)
    * [Struct `mpsc::UnboundedSyncSender<T>`](#struct-mpscunboundedsyncsendert)
    * [Struct `mpsc::UnboundedSyncReceiver<T>`](#struct-mpscunboundedsyncreceivert)
    * [Struct `mpsc::UnboundedAsyncSender<T>`](#struct-mpscunboundedasyncsendert)
    * [Struct `mpsc::UnboundedAsyncReceiver<T>`](#struct-mpscunboundedasyncreceivert)
    * [Struct `mpsc::BoundedSyncSender<T>`](#struct-mpscboundedsyncsendert)
    * [Struct `mpsc::BoundedSyncReceiver<T>`](#struct-mpscboundedsyncreceivert)
    * [Struct `mpsc::BoundedAsyncSender<T>`](#struct-mpscboundedasyncsendert)
    * [Struct `mpsc::BoundedAsyncReceiver<T>`](#struct-mpscboundedasyncreceivert)
    * [mpsc futures](#mpsc-futures)
    * [Struct `mpsc::rendezvous::RendezvousSyncSender<T>`](#struct-mpscrendezvousrendezvoussyncsendert)
    * [Struct `mpsc::rendezvous::RendezvousSyncReceiver<T>`](#struct-mpscrendezvousrendezvoussyncreceivert)
    * [Struct `mpsc::rendezvous::RendezvousAsyncSender<T>`](#struct-mpscrendezvousrendezvousasyncsendert)
    * [Struct `mpsc::rendezvous::RendezvousAsyncReceiver<T>`](#struct-mpscrendezvousrendezvousasyncreceivert)
    * [mpsc rendezvous futures](#mpsc-rendezvous-futures)
* [4. Module `fibre::spmc`](#4-module-fibrespmc)
    * [spmc functions](#spmc-functions)
    * [Struct `spmc::BoundedSyncSender<T>`](#struct-spmcboundedsyncsendert)
    * [Struct `spmc::BoundedSyncReceiver<T>`](#struct-spmcboundedsyncreceivert)
    * [Struct `spmc::BoundedAsyncSender<T>`](#struct-spmcboundedasyncsendert)
    * [Struct `spmc::BoundedAsyncReceiver<T>`](#struct-spmcboundedasyncreceivert)
    * [spmc futures](#spmc-futures)
* [5. Module `fibre::spmc::topic`](#5-module-fibrespmctopic)
    * [topic functions](#topic-functions)
    * [Struct `topic::TopicSender<K, T>`](#struct-topictopicsenderk-t)
    * [Struct `topic::TopicReceiver<K, T>`](#struct-topictopicreceiverk-t)
    * [Struct `topic::AsyncTopicSender<K, T>`](#struct-topicasynctopicsenderk-t)
    * [Struct `topic::AsyncTopicReceiver<K, T>`](#struct-topicasynctopicreceiverk-t)
* [6. Module `fibre::mpmc`](#6-module-fibrempmc)
    * [mpmc functions](#mpmc-functions)
    * [Struct `mpmc::Sender<T>`](#struct-mpmcsendert)
    * [Struct `mpmc::Receiver<T>`](#struct-mpmcreceivert)
    * [Struct `mpmc::AsyncSender<T>`](#struct-mpmcasyncsendert)
    * [Struct `mpmc::AsyncReceiver<T>`](#struct-mpmcasyncreceivert)
    * [Struct `mpmc::UnboundedSyncSender<T>`](#struct-mpmcunboundedsyncsendert)
    * [Struct `mpmc::UnboundedSyncReceiver<T>`](#struct-mpmcunboundedsyncreceivert)
    * [Struct `mpmc::UnboundedAsyncSender<T>`](#struct-mpmcunboundedasyncsendert)
    * [Struct `mpmc::UnboundedAsyncReceiver<T>`](#struct-mpmcunboundedasyncreceivert)
    * [mpmc futures](#mpmc-futures)
    * [Struct `mpmc::rendezvous::RendezvousSyncSender<T>`](#struct-mpmcrendezvousrendezvoussyncsendert)
    * [Struct `mpmc::rendezvous::RendezvousSyncReceiver<T>`](#struct-mpmcrendezvousrendezvoussyncreceivert)
    * [Struct `mpmc::rendezvous::RendezvousAsyncSender<T>`](#struct-mpmcrendezvousrendezvousasyncsendert)
    * [Struct `mpmc::rendezvous::RendezvousAsyncReceiver<T>`](#struct-mpmcrendezvousrendezvousasyncreceivert)
    * [mpmc rendezvous futures](#mpmc-rendezvous-futures)
* [7. Module `fibre::mpmc_exp`](#7-module-fibrempmc_exp)
    * [mpmc_exp functions](#mpmc_exp-functions)
    * [Struct `mpmc_exp::Sender<T>`](#struct-mpmc_expsendert)
    * [Struct `mpmc_exp::Receiver<T>`](#struct-mpmc_expreceivert)
    * [Struct `mpmc_exp::AsyncSender<T>`](#struct-mpmc_expasyncsendert)
    * [Struct `mpmc_exp::AsyncReceiver<T>`](#struct-mpmc_expasyncreceivert)
    * [mpmc_exp futures](#mpmc_exp-futures)
* [8. Module `fibre::sync`](#8-module-fibresync)
    * [Struct `sync::HybridMutex<T>`](#struct-synchybridmutext)
    * [Struct `sync::MutexGuard<'a, T>`](#struct-syncmutexguarda-t)
    * [Struct `sync::HybridRwLock<T>`](#struct-synchybridrwlockt)
    * [Struct `sync::ReadGuard<'a, T>`](#struct-syncreadguarda-t)
    * [Struct `sync::WriteGuard<'a, T>`](#struct-syncwriteguarda-t)
* [9. Module `fibre::sync::v2`](#9-module-fibresyncv2)
    * [Struct `v2::HybridRwLock<T>`](#struct-v2hybridrwlockt)
    * [Struct `v2::ReadGuard<'a, T>`](#struct-v2readguarda-t)
    * [Struct `v2::WriteGuard<'a, T>`](#struct-v2writeguarda-t)
* [10. Module `fibre::telemetry`](#10-module-fibretelemetry)
    * [telemetry functions](#telemetry-functions)
    * [telemetry macros](#telemetry-macros)
    * [Struct `telemetry::enabled::TelemetryEvent`](#struct-telemetryenabledtelemetryevent)
* [11. Batch Operation Semantics](#11-batch-operation-semantics)
    * [By-value send](#by-value-send)
    * [In-place send](#in-place-send)
    * [By-value receive](#by-value-receive)
    * [In-place receive](#in-place-receive)
    * [Batch edge cases](#batch-edge-cases)
    * [Batch cancellation](#batch-cancellation)
* [12. Error Handling](#12-error-handling)
    * [Enum `error::TrySendError<T>`](#enum-errortrysenderrort)
    * [Enum `error::SendError`](#enum-errorsenderror)
    * [Enum `error::TryRecvError`](#enum-errortryrecverror)
    * [Enum `error::RecvError`](#enum-errorrecverror)
    * [Enum `error::RecvErrorTimeout`](#enum-errorrecverrortimeout)
    * [Struct `error::CloseError`](#struct-errorcloseerror)
    * [Struct `error::TrySendBatchError<T>`](#struct-errortrysendbatcherrort)
    * [Struct `error::SendBatchError<T>`](#struct-errorsendbatcherrort)
    * [Enum `error::BatchSendErrorReason`](#enum-errorbatchsenderrorreason)

## 1. Module `fibre::oneshot`

A channel for sending a single value from one of potentially many senders to a single receiver. `exclusive()` builds a single-sender variant with `&mut self` receive methods. `pair_pool()` builds a pool of recycled single-sender channels: `pair()` pops a slot, whichever handle finishes last pushes it back and the handles carry no reference counts.

### oneshot functions

*   `pub fn oneshot<T>() -> (Sender<T>, Receiver<T>)`
*   `pub fn exclusive<T>() -> (ExclusiveSender<T>, ExclusiveReceiver<T>)`
*   `pub fn pair_pool<T: Send>(capacity: usize) -> OneshotPairPool<T>`

### Struct `oneshot::Sender<T>`

The sending side of a oneshot channel. `Clone`; only the first send across all clones succeeds.

*   `pub fn send(self, value: T) -> Result<(), TrySendError<T>>`: Consumes the handle.
*   `pub fn close(&self) -> Result<(), CloseError>`: `Err(CloseError)` if this handle was already closed.
*   `pub fn is_closed(&self) -> bool`: `true` once the receiver has been dropped or closed.
*   `pub fn is_sent(&self) -> bool`: `true` once any clone has sent.

### Struct `oneshot::Receiver<T>`

The receiving side of a oneshot channel. Not `Clone`.

*   `pub fn recv(&self) -> ReceiveFuture<'_, T>`: Cancel-safe.
*   `pub fn recv_blocking(&self) -> Result<T, RecvError>`: For synchronous callers; returns without parking or allocating a parker when the value has already arrived.
*   `pub fn try_recv(&self) -> Result<T, TryRecvError>`
*   `pub fn close(&self) -> Result<(), CloseError>`: Drops a value that was sent but not received.
*   `pub fn is_closed(&self) -> bool`: `true` once a value was taken or no value will ever be sent.

### Struct `oneshot::ExclusiveSender<T>`

The sending side of an `exclusive()` channel. Not `Clone`.

*   `pub fn send(self, value: T) -> Result<(), TrySendError<T>>`: Fails only with `TrySendError::Closed`, returning the value, if the receiver is gone.
*   `pub fn close(self)`
*   `pub fn is_closed(&self) -> bool`

### Struct `oneshot::ExclusiveReceiver<T>`

The receiving side of an `exclusive()` channel. Not `Clone`; receive methods take `&mut self`.

*   `pub fn recv(&mut self) -> ExclusiveReceiveFuture<'_, T>`: Cancel-safe.
*   `pub fn recv_blocking(&mut self) -> Result<T, RecvError>`: For synchronous callers; returns without parking or allocating a parker when the value has already arrived.
*   `pub fn try_recv(&mut self) -> Result<T, TryRecvError>`: `Disconnected` once the value was taken, the sender dropped without sending or this handle was closed.
*   `pub fn close(&mut self)`
*   `pub fn is_closed(&self) -> bool`

### Struct `oneshot::OneshotPairPool<T>`

A pool of `capacity` recycled oneshot channels. `Send + Sync`; share it by reference. Slots are recycled and never freed per channel. Dropping the pool while channels are in flight is safe: storage release is deferred until the last outstanding channel retires.

*   `pub fn new(capacity: usize) -> Self`
*   `pub fn capacity(&self) -> usize`
*   `pub fn pair(&self) -> Option<(PooledSender<T>, PooledReceiver<T>)>`: `None` when the pool is exhausted; a retired slot becomes available again immediately.
*   `pub fn pair_batch(&self, n: usize) -> Option<Vec<(PooledSender<T>, PooledReceiver<T>)>>`: All-or-nothing in one freelist operation; conservatively `None` when fewer than `n` slots were observed at once.

### Struct `oneshot::PooledSender<T>`

The sending side of a pooled channel. Not `Clone`.

*   `pub fn send(self, value: T) -> Result<(), TrySendError<T>>`: Fails only with `TrySendError::Closed`, returning the value, if the receiver is gone.
*   `pub fn close(self)`
*   `pub fn is_closed(&self) -> bool`

### Struct `oneshot::PooledReceiver<T>`

The receiving side of a pooled channel. Not `Clone`.

*   `pub fn recv(&mut self) -> PooledReceiveFuture<'_, T>`: Cancel-safe.
*   `pub fn recv_blocking(&mut self) -> Result<T, RecvError>`: For synchronous callers; returns without parking or allocating a parker when the value has already arrived.
*   `pub fn try_recv(&mut self) -> Result<T, TryRecvError>`
*   `pub fn close(&mut self)`
*   `pub fn is_closed(&self) -> bool`

### Struct `oneshot::OneshotHostPool<H, T>`

A pool whose cells are whole caller records (`H: Send + Sync`) with a `PoolSlot<T>` reply channel embedded, located by the projection given at construction. Cell mutation is exclusive between pop and push; `pair_init` runs its closure inside that window. Same drop deferral as `OneshotPairPool`.

*   `pub fn new(capacity: usize, make: impl FnMut() -> H, project: fn(&H) -> &PoolSlot<T>) -> Self`: `project` must always return the same field.
*   `pub fn capacity(&self) -> usize`
*   `pub fn pair_init(&self, init: impl FnOnce(&mut H)) -> Option<(HostSender<H, T>, HostReceiver<H, T>)>`
*   `pub fn pair_init_batch(&self, n: usize, init: impl FnMut(&mut H)) -> Option<Vec<(HostSender<H, T>, HostReceiver<H, T>)>>`

### Struct `oneshot::PoolSlot<T>`

The reply slot a host record embeds. Opaque; it only becomes a channel through a host pool's projection. Implements `Default`.

*   `pub fn new() -> Self`

### Struct `oneshot::HostSender<H, T>`

The sending side of a host pool channel. Not `Clone`.

*   `pub fn send(self, value: T) -> Result<(), TrySendError<T>>`: Fails only with `TrySendError::Closed`, returning the value, if the receiver is gone.
*   `pub fn close(self)`
*   `pub fn is_closed(&self) -> bool`

### Struct `oneshot::HostReceiver<H, T>`

The receiving side of a host pool channel. Not `Clone`.

*   `pub fn host(&self) -> &H`: The record this channel rides in, readable until the receiver finishes.
*   `pub fn recv(&mut self) -> HostReceiveFuture<'_, H, T>`: Cancel-safe.
*   `pub fn recv_blocking(&mut self) -> Result<T, RecvError>`: For synchronous callers; returns without parking or allocating a parker when the value has already arrived.
*   `pub fn try_recv(&mut self) -> Result<T, TryRecvError>`
*   `pub fn close(&mut self)`
*   `pub fn is_closed(&self) -> bool`

### oneshot futures

*   `ReceiveFuture<'a, T>`: `Output = Result<T, RecvError>`
*   `ExclusiveReceiveFuture<'a, T>`: `Output = Result<T, RecvError>`
*   `PooledReceiveFuture<'a, T>`: `Output = Result<T, RecvError>`
*   `HostReceiveFuture<'a, H, T>`: `Output = Result<T, RecvError>`

## 2. Module `fibre::spsc`

A lock-free bounded channel for one producer and one consumer. The rendezvous handles are also re-exported from `fibre::spsc`.

### spsc functions

*   `pub fn bounded_sync<T: Send>(capacity: usize) -> (BoundedSyncSender<T>, BoundedSyncReceiver<T>)`: Panics if `capacity == 0`.
*   `pub fn bounded_async<T: Send>(capacity: usize) -> (BoundedAsyncSender<T>, BoundedAsyncReceiver<T>)`: Panics if `capacity == 0`.
*   `pub fn rendezvous::rendezvous<T: Send>() -> (RendezvousSyncSender<T>, RendezvousSyncReceiver<T>)`: Zero-capacity direct handoff.
*   `pub fn rendezvous::rendezvous_async<T: Send>() -> (RendezvousAsyncSender<T>, RendezvousAsyncReceiver<T>)`

### Struct `spsc::BoundedSyncSender<T>`

The synchronous sending handle. Not `Clone`; `!Sync`.

*   `pub fn to_async(self) -> BoundedAsyncSender<T>`
*   `pub fn try_send(&self, item: T) -> Result<(), TrySendError<T>>`
*   `pub fn send(&self, item: T) -> Result<(), SendError>`
*   `pub fn try_send_batch(&self, items: Vec<T>) -> Result<usize, TrySendBatchError<T>>`
*   `pub fn send_batch(&self, items: Vec<T>) -> Result<usize, SendBatchError<T>>`: Blocks until all items are sent.
*   `pub fn try_send_batch_mut(&self, items: &mut Vec<T>) -> Result<usize, SendError>`: Drains sent items from the front of `items`.
*   `pub fn send_batch_mut(&self, items: &mut Vec<T>) -> Result<usize, SendError>`: Blocking in-place batch send.
*   `pub fn close(&self) -> Result<(), CloseError>`
*   `pub fn is_closed(&self) -> bool`
*   `pub fn capacity(&self) -> usize`
*   `pub fn len(&self) -> usize`
*   `pub fn is_empty(&self) -> bool`
*   `pub fn is_full(&self) -> bool`

### Struct `spsc::BoundedSyncReceiver<T>`

The synchronous receiving handle. Not `Clone`; `!Sync`.

*   `pub fn to_async(self) -> BoundedAsyncReceiver<T>`
*   `pub fn try_recv(&self) -> Result<T, TryRecvError>`
*   `pub fn recv(&self) -> Result<T, RecvError>`
*   `pub fn recv_timeout(&mut self, timeout: Duration) -> Result<T, RecvErrorTimeout>`
*   `pub fn try_recv_batch(&self, max: usize) -> Result<Vec<T>, TryRecvError>`
*   `pub fn recv_batch(&self, max: usize) -> Result<Vec<T>, RecvError>`: Blocks until at least one item, returns 1..=max.
*   `pub fn try_recv_batch_mut(&self, out: &mut Vec<T>, max: usize) -> Result<usize, TryRecvError>`: Appends to `out`.
*   `pub fn recv_batch_mut(&self, out: &mut Vec<T>, max: usize) -> Result<usize, RecvError>`: Blocking in-place batch receive.
*   `pub fn close(&self) -> Result<(), CloseError>`
*   `pub fn is_closed(&self) -> bool`
*   `pub fn capacity(&self) -> usize`
*   `pub fn len(&self) -> usize`
*   `pub fn is_empty(&self) -> bool`
*   `pub fn is_full(&self) -> bool`

### Struct `spsc::BoundedAsyncSender<T>`

The asynchronous sending handle. Not `Clone`. All send methods take `&mut self`; the returned futures are `Send`.

*   `pub fn to_sync(self) -> BoundedSyncSender<T>`
*   `pub fn send(&mut self, item: T) -> SendFuture<'_, T>`: Cancel-safe.
*   `pub fn try_send(&mut self, item: T) -> Result<(), TrySendError<T>>`
*   `pub fn send_batch(&mut self, items: Vec<T>) -> SendBatchFuture<'_, T>`
*   `pub fn send_batch_mut<'a>(&'a mut self, items: &'a mut Vec<T>) -> SendBatchMutFuture<'a, T>`: Cancel-safe.
*   `pub fn try_send_batch(&mut self, items: Vec<T>) -> Result<usize, TrySendBatchError<T>>`
*   `pub fn try_send_batch_mut(&mut self, items: &mut Vec<T>) -> Result<usize, SendError>`
*   `pub fn close(&self) -> Result<(), CloseError>`
*   `pub fn is_closed(&self) -> bool`
*   `pub fn capacity(&self) -> usize`
*   `pub fn len(&self) -> usize`
*   `pub fn is_empty(&self) -> bool`
*   `pub fn is_full(&self) -> bool`

### Struct `spsc::BoundedAsyncReceiver<T>`

The asynchronous receiving handle. Not `Clone`. Implements `futures::Stream`. All receive methods take `&mut self`; the returned futures are `Send`.

*   `pub fn to_sync(self) -> BoundedSyncReceiver<T>`
*   `pub fn recv(&mut self) -> ReceiveFuture<'_, T>`: Cancel-safe.
*   `pub fn try_recv(&mut self) -> Result<T, TryRecvError>`
*   `pub fn recv_batch(&mut self, max: usize) -> RecvBatchFuture<'_, T>`
*   `pub fn recv_batch_mut<'a>(&'a mut self, out: &'a mut Vec<T>, max: usize) -> RecvBatchMutFuture<'a, T>`
*   `pub fn try_recv_batch(&mut self, max: usize) -> Result<Vec<T>, TryRecvError>`
*   `pub fn try_recv_batch_mut(&mut self, out: &mut Vec<T>, max: usize) -> Result<usize, TryRecvError>`
*   `pub fn close(&self) -> Result<(), CloseError>`
*   `pub fn is_closed(&self) -> bool`
*   `pub fn capacity(&self) -> usize`
*   `pub fn len(&self) -> usize`
*   `pub fn is_empty(&self) -> bool`
*   `pub fn is_full(&self) -> bool`

### spsc futures

*   `SendFuture<'a, T>`: `Output = Result<(), SendError>`
*   `SendBatchFuture<'a, T>`: `Output = Result<usize, SendBatchError<T>>`
*   `SendBatchMutFuture<'a, T>`: `Output = Result<usize, SendError>`
*   `ReceiveFuture<'a, T>`: `Output = Result<T, RecvError>`. Not re-exported from `fibre::spsc`, so callers cannot name the type.
*   `RecvBatchFuture<'a, T>`: `Output = Result<Vec<T>, RecvError>`
*   `RecvBatchMutFuture<'a, T>`: `Output = Result<usize, RecvError>`

### Struct `spsc::rendezvous::RendezvousSyncSender<T>`

Synchronous sender. Not `Clone`. No batch API.

*   `pub fn send(&self, item: T) -> Result<(), SendError>`
*   `pub fn try_send(&self, item: T) -> Result<(), TrySendError<T>>`
*   `pub fn close(&self) -> Result<(), CloseError>`
*   `pub fn is_closed(&self) -> bool`
*   `pub fn len(&self) -> usize`
*   `pub fn is_empty(&self) -> bool`
*   `pub fn is_full(&self) -> bool`
*   `pub fn capacity(&self) -> Option<usize>`: Always `Some(0)`.
*   `pub fn to_async(self) -> RendezvousAsyncSender<T>`

### Struct `spsc::rendezvous::RendezvousSyncReceiver<T>`

Synchronous receiver. Not `Clone`. No batch API.

*   `pub fn recv(&self) -> Result<T, RecvError>`
*   `pub fn try_recv(&self) -> Result<T, TryRecvError>`
*   `pub fn recv_timeout(&self, timeout: Duration) -> Result<T, RecvErrorTimeout>`
*   `pub fn close(&self) -> Result<(), CloseError>`
*   `pub fn is_closed(&self) -> bool`
*   `pub fn len(&self) -> usize`
*   `pub fn is_empty(&self) -> bool`
*   `pub fn is_full(&self) -> bool`
*   `pub fn capacity(&self) -> Option<usize>`: Always `Some(0)`.
*   `pub fn to_async(self) -> RendezvousAsyncReceiver<T>`

### Struct `spsc::rendezvous::RendezvousAsyncSender<T>`

Asynchronous sender. Not `Clone`. No batch API.

*   `pub fn send(&self, item: T) -> SendFuture<'_, T>`: Not cancel-safe: a dropped send can still be delivered.
*   `pub fn try_send(&self, item: T) -> Result<(), TrySendError<T>>`
*   `pub fn close(&self) -> Result<(), CloseError>`
*   `pub fn is_closed(&self) -> bool`
*   `pub fn len(&self) -> usize`
*   `pub fn is_empty(&self) -> bool`
*   `pub fn is_full(&self) -> bool`
*   `pub fn capacity(&self) -> Option<usize>`: Always `Some(0)`.
*   `pub fn to_sync(self) -> RendezvousSyncSender<T>`

### Struct `spsc::rendezvous::RendezvousAsyncReceiver<T>`

Asynchronous receiver. Not `Clone`. No batch API. Does not implement `futures::Stream`.

*   `pub fn recv(&self) -> RecvFuture<'_, T>`: Not cancel-safe: a dropped receive can lose an item already handed to it.
*   `pub fn try_recv(&self) -> Result<T, TryRecvError>`
*   `pub fn close(&self) -> Result<(), CloseError>`
*   `pub fn is_closed(&self) -> bool`
*   `pub fn len(&self) -> usize`
*   `pub fn is_empty(&self) -> bool`
*   `pub fn is_full(&self) -> bool`
*   `pub fn capacity(&self) -> Option<usize>`: Always `Some(0)`.
*   `pub fn to_sync(self) -> RendezvousSyncReceiver<T>`

### spsc rendezvous futures

*   `rendezvous::SendFuture<'a, T>`: `Output = Result<(), SendError>`
*   `rendezvous::RecvFuture<'a, T>`: `Output = Result<T, RecvError>`

## 3. Module `fibre::mpsc`

Channels for multiple producers and one consumer. The rendezvous handles are also re-exported from `fibre::mpsc`.

### mpsc functions

*   `pub fn unbounded<T: Send>() -> (UnboundedSyncSender<T>, UnboundedSyncReceiver<T>)`
*   `pub fn unbounded_async<T: Send>() -> (UnboundedAsyncSender<T>, UnboundedAsyncReceiver<T>)`
*   `pub fn bounded<T: Send>(capacity: usize) -> (BoundedSyncSender<T>, BoundedSyncReceiver<T>)`: Panics if `capacity == 0`; use `rendezvous`.
*   `pub fn bounded_async<T: Send>(capacity: usize) -> (BoundedAsyncSender<T>, BoundedAsyncReceiver<T>)`: Panics if `capacity == 0`; use `rendezvous`.
*   `pub fn rendezvous::rendezvous<T: Send>() -> (RendezvousSyncSender<T>, RendezvousSyncReceiver<T>)`: Zero-capacity direct handoff.
*   `pub fn rendezvous::rendezvous_async<T: Send>() -> (RendezvousAsyncSender<T>, RendezvousAsyncReceiver<T>)`

### Struct `mpsc::UnboundedSyncSender<T>`

A `Clone` sync handle. Sends take `&mut self` because the handle owns its bump slab; clone a sender per thread.

*   `pub fn send(&mut self, value: T) -> Result<(), SendError>`: Never blocks.
*   `pub fn try_send(&mut self, value: T) -> Result<(), TrySendError<T>>`
*   `pub fn send_batch(&mut self, items: Vec<T>) -> Result<usize, SendBatchError<T>>`: Never blocks; one length update and one wake per batch.
*   `pub fn try_send_batch(&mut self, items: Vec<T>) -> Result<usize, TrySendBatchError<T>>`
*   `pub fn send_batch_mut(&mut self, items: &mut Vec<T>) -> Result<usize, SendError>`
*   `pub fn try_send_batch_mut(&mut self, items: &mut Vec<T>) -> Result<usize, SendError>`
*   `pub fn close(&mut self) -> Result<(), CloseError>`
*   `pub fn is_closed(&self) -> bool`
*   `pub fn sender_count(&self) -> usize`
*   `pub fn len(&self) -> usize`
*   `pub fn is_empty(&self) -> bool`
*   `pub fn to_async(self) -> UnboundedAsyncSender<T>`

### Struct `mpsc::UnboundedSyncReceiver<T>`

A sync handle. Not `Clone`; `!Sync`.

*   `pub fn recv(&self) -> Result<T, RecvError>`: Blocks if empty.
*   `pub fn try_recv(&self) -> Result<T, TryRecvError>`
*   `pub fn recv_timeout(&self, timeout: Duration) -> Result<T, RecvErrorTimeout>`
*   `pub fn recv_batch(&self, max: usize) -> Result<Vec<T>, RecvError>`
*   `pub fn try_recv_batch(&self, max: usize) -> Result<Vec<T>, TryRecvError>`
*   `pub fn recv_batch_mut(&self, out: &mut Vec<T>, max: usize) -> Result<usize, RecvError>`: Appends to `out`.
*   `pub fn try_recv_batch_mut(&self, out: &mut Vec<T>, max: usize) -> Result<usize, TryRecvError>`: Appends to `out`.
*   `pub fn close(&self) -> Result<(), CloseError>`
*   `pub fn is_closed(&self) -> bool`
*   `pub fn sender_count(&self) -> usize`
*   `pub fn len(&self) -> usize`
*   `pub fn is_empty(&self) -> bool`
*   `pub fn to_async(self) -> UnboundedAsyncReceiver<T>`

### Struct `mpsc::UnboundedAsyncSender<T>`

A `Clone` async handle. Sends take `&mut self`; clone a sender per task.

*   `pub fn send(&mut self, value: T) -> UnboundedSendFuture<'_, T>`: Completes on first poll. Cancel-safe.
*   `pub fn try_send(&mut self, value: T) -> Result<(), TrySendError<T>>`
*   `pub fn send_batch(&mut self, items: Vec<T>) -> UnboundedSendBatchFuture<'_, T>`: Completes on first poll.
*   `pub fn try_send_batch(&mut self, items: Vec<T>) -> Result<usize, TrySendBatchError<T>>`
*   `pub fn send_batch_mut<'a>(&'a mut self, items: &'a mut Vec<T>) -> UnboundedSendBatchMutFuture<'a, T>`: Completes on first poll.
*   `pub fn try_send_batch_mut(&mut self, items: &mut Vec<T>) -> Result<usize, SendError>`
*   `pub fn close(&mut self) -> Result<(), CloseError>`
*   `pub fn is_closed(&self) -> bool`
*   `pub fn sender_count(&self) -> usize`
*   `pub fn len(&self) -> usize`
*   `pub fn is_empty(&self) -> bool`
*   `pub fn to_sync(self) -> UnboundedSyncSender<T>`

### Struct `mpsc::UnboundedAsyncReceiver<T>`

An async handle. Not `Clone`. Implements `futures::Stream`.

*   `pub fn recv(&mut self) -> UnboundedRecvFuture<'_, T>`: Cancel-safe.
*   `pub fn try_recv(&mut self) -> Result<T, TryRecvError>`
*   `pub fn recv_batch(&mut self, max: usize) -> UnboundedRecvBatchFuture<'_, T>`: Cancel-safe.
*   `pub fn recv_batch_mut<'a>(&'a mut self, out: &'a mut Vec<T>, max: usize) -> UnboundedRecvBatchMutFuture<'a, T>`: Cancel-safe.
*   `pub fn try_recv_batch(&mut self, max: usize) -> Result<Vec<T>, TryRecvError>`
*   `pub fn try_recv_batch_mut(&mut self, out: &mut Vec<T>, max: usize) -> Result<usize, TryRecvError>`
*   `pub fn close(&self) -> Result<(), CloseError>`
*   `pub fn is_closed(&self) -> bool`
*   `pub fn sender_count(&self) -> usize`
*   `pub fn len(&self) -> usize`
*   `pub fn is_empty(&self) -> bool`
*   `pub fn to_sync(self) -> UnboundedSyncReceiver<T>`

### Struct `mpsc::BoundedSyncSender<T>`

A `Clone` sync handle.

*   `pub fn send(&self, item: T) -> Result<(), SendError>`: Blocks if full and resumes as soon as the receiver frees a slot. At most one blocked sender at a time spins (bounded) before parking; the rest park at once. At capacity 4 or less every blocked sender first spins briefly.
*   `pub fn try_send(&self, item: T) -> Result<(), TrySendError<T>>`
*   `pub fn send_batch(&self, items: Vec<T>) -> Result<usize, SendBatchError<T>>`: Claims runs of free capacity; blocks for the remainder.
*   `pub fn try_send_batch(&self, items: Vec<T>) -> Result<usize, TrySendBatchError<T>>`: Sends as many items as there is free capacity.
*   `pub fn send_batch_mut(&self, items: &mut Vec<T>) -> Result<usize, SendError>`
*   `pub fn try_send_batch_mut(&self, items: &mut Vec<T>) -> Result<usize, SendError>`
*   `pub fn close(&self) -> Result<(), CloseError>`
*   `pub fn is_closed(&self) -> bool`
*   `pub fn capacity(&self) -> usize`
*   `pub fn len(&self) -> usize`
*   `pub fn is_empty(&self) -> bool`
*   `pub fn is_full(&self) -> bool`
*   `pub fn to_async(self) -> BoundedAsyncSender<T>`

### Struct `mpsc::BoundedSyncReceiver<T>`

A sync handle. Not `Clone`; `!Sync`. Items still buffered when the receiver is closed or dropped are not dropped until the last sender handle drops. A buffered item holding a oneshot sender keeps it alive until then.

*   `pub fn recv(&self) -> Result<T, RecvError>`: Blocks if empty.
*   `pub fn recv_timeout(&self, timeout: Duration) -> Result<T, RecvErrorTimeout>`: Does not check this handle's closed flag, so it drains buffered items after `close()`.
*   `pub fn try_recv(&self) -> Result<T, TryRecvError>`
*   `pub fn recv_batch(&self, max: usize) -> Result<Vec<T>, RecvError>`
*   `pub fn try_recv_batch(&self, max: usize) -> Result<Vec<T>, TryRecvError>`
*   `pub fn recv_batch_mut(&self, out: &mut Vec<T>, max: usize) -> Result<usize, RecvError>`: Appends to `out`.
*   `pub fn try_recv_batch_mut(&self, out: &mut Vec<T>, max: usize) -> Result<usize, TryRecvError>`: Appends to `out`.
*   `pub fn close(&self) -> Result<(), CloseError>`
*   `pub fn is_closed(&self) -> bool`
*   `pub fn capacity(&self) -> usize`
*   `pub fn len(&self) -> usize`
*   `pub fn is_empty(&self) -> bool`
*   `pub fn is_full(&self) -> bool`
*   `pub fn to_async(self) -> BoundedAsyncReceiver<T>`

### Struct `mpsc::BoundedAsyncSender<T>`

A `Clone` async handle.

*   `pub fn send(&self, item: T) -> BoundedSendFuture<'_, T>`: Waits for capacity and resumes as soon as the receiver frees a slot. Cancel-safe.
*   `pub fn try_send(&self, item: T) -> Result<(), TrySendError<T>>`
*   `pub fn send_batch(&self, items: Vec<T>) -> BoundedSendBatchFuture<'_, T>`: Claims runs of free capacity, re-arming for the remainder.
*   `pub fn send_batch_mut<'a>(&'a self, items: &'a mut Vec<T>) -> BoundedSendBatchMutFuture<'a, T>`: Claims runs of free capacity, re-arming for the remainder.
*   `pub fn try_send_batch(&self, items: Vec<T>) -> Result<usize, TrySendBatchError<T>>`
*   `pub fn try_send_batch_mut(&self, items: &mut Vec<T>) -> Result<usize, SendError>`
*   `pub fn close(&self) -> Result<(), CloseError>`
*   `pub fn is_closed(&self) -> bool`
*   `pub fn capacity(&self) -> usize`
*   `pub fn len(&self) -> usize`
*   `pub fn is_empty(&self) -> bool`
*   `pub fn is_full(&self) -> bool`
*   `pub fn to_sync(self) -> BoundedSyncSender<T>`

### Struct `mpsc::BoundedAsyncReceiver<T>`

An async handle. Not `Clone`. Implements `futures::Stream`. Items still buffered when the receiver is closed or dropped are not dropped until the last sender handle drops. A buffered item holding a oneshot sender keeps it alive until then.

*   `pub fn recv(&self) -> BoundedRecvFuture<'_, T>`: Cancel-safe.
*   `pub fn try_recv(&self) -> Result<T, TryRecvError>`
*   `pub fn recv_batch(&self, max: usize) -> BoundedRecvBatchFuture<'_, T>`: Cancel-safe.
*   `pub fn recv_batch_mut<'a>(&'a self, out: &'a mut Vec<T>, max: usize) -> BoundedRecvBatchMutFuture<'a, T>`: Cancel-safe.
*   `pub fn try_recv_batch(&self, max: usize) -> Result<Vec<T>, TryRecvError>`
*   `pub fn try_recv_batch_mut(&self, out: &mut Vec<T>, max: usize) -> Result<usize, TryRecvError>`
*   `pub fn close(&self) -> Result<(), CloseError>`
*   `pub fn is_closed(&self) -> bool`
*   `pub fn capacity(&self) -> usize`
*   `pub fn len(&self) -> usize`
*   `pub fn is_empty(&self) -> bool`
*   `pub fn is_full(&self) -> bool`
*   `pub fn to_sync(self) -> BoundedSyncReceiver<T>`

### mpsc futures

*   `UnboundedSendFuture<'a, T>`, `BoundedSendFuture<'a, T>`: `Output = Result<(), SendError>`
*   `UnboundedSendBatchFuture<'a, T>`, `BoundedSendBatchFuture<'a, T>`: `Output = Result<usize, SendBatchError<T>>`
*   `UnboundedSendBatchMutFuture<'a, T>`, `BoundedSendBatchMutFuture<'a, T>`: `Output = Result<usize, SendError>`
*   `UnboundedRecvFuture<'a, T>`, `BoundedRecvFuture<'a, T>`: `Output = Result<T, RecvError>`
*   `UnboundedRecvBatchFuture<'a, T>`, `BoundedRecvBatchFuture<'a, T>`: `Output = Result<Vec<T>, RecvError>`
*   `UnboundedRecvBatchMutFuture<'a, T>`, `BoundedRecvBatchMutFuture<'a, T>`: `Output = Result<usize, RecvError>`

### Struct `mpsc::rendezvous::RendezvousSyncSender<T>`

Synchronous sender. `Clone`. No batch API.

*   `pub fn send(&self, item: T) -> Result<(), SendError>`
*   `pub fn try_send(&self, item: T) -> Result<(), TrySendError<T>>`
*   `pub fn close(&self) -> Result<(), CloseError>`
*   `pub fn is_closed(&self) -> bool`
*   `pub fn len(&self) -> usize`
*   `pub fn is_empty(&self) -> bool`
*   `pub fn is_full(&self) -> bool`
*   `pub fn capacity(&self) -> Option<usize>`: Always `Some(0)`.
*   `pub fn to_async(self) -> RendezvousAsyncSender<T>`

### Struct `mpsc::rendezvous::RendezvousSyncReceiver<T>`

Synchronous receiver. Not `Clone`. No batch API.

*   `pub fn recv(&self) -> Result<T, RecvError>`
*   `pub fn try_recv(&self) -> Result<T, TryRecvError>`
*   `pub fn recv_timeout(&self, timeout: Duration) -> Result<T, RecvErrorTimeout>`
*   `pub fn close(&self) -> Result<(), CloseError>`
*   `pub fn is_closed(&self) -> bool`
*   `pub fn len(&self) -> usize`
*   `pub fn is_empty(&self) -> bool`
*   `pub fn is_full(&self) -> bool`
*   `pub fn capacity(&self) -> Option<usize>`: Always `Some(0)`.
*   `pub fn to_async(self) -> RendezvousAsyncReceiver<T>`

### Struct `mpsc::rendezvous::RendezvousAsyncSender<T>`

Asynchronous sender. `Clone`. No batch API.

*   `pub fn send(&self, item: T) -> SendFuture<'_, T>`: Not cancel-safe: a dropped send can still be delivered.
*   `pub fn try_send(&self, item: T) -> Result<(), TrySendError<T>>`
*   `pub fn close(&self) -> Result<(), CloseError>`
*   `pub fn is_closed(&self) -> bool`
*   `pub fn len(&self) -> usize`
*   `pub fn is_empty(&self) -> bool`
*   `pub fn is_full(&self) -> bool`
*   `pub fn capacity(&self) -> Option<usize>`: Always `Some(0)`.
*   `pub fn to_sync(self) -> RendezvousSyncSender<T>`

### Struct `mpsc::rendezvous::RendezvousAsyncReceiver<T>`

Asynchronous receiver. Not `Clone`. No batch API. Does not implement `futures::Stream`.

*   `pub fn recv(&self) -> RecvFuture<'_, T>`: Not cancel-safe: a dropped receive can lose an item already handed to it.
*   `pub fn try_recv(&self) -> Result<T, TryRecvError>`
*   `pub fn close(&self) -> Result<(), CloseError>`
*   `pub fn is_closed(&self) -> bool`
*   `pub fn len(&self) -> usize`
*   `pub fn is_empty(&self) -> bool`
*   `pub fn is_full(&self) -> bool`
*   `pub fn capacity(&self) -> Option<usize>`: Always `Some(0)`.
*   `pub fn to_sync(self) -> RendezvousSyncReceiver<T>`

### mpsc rendezvous futures

*   `rendezvous::SendFuture<'a, T>`: `Output = Result<(), SendError>`
*   `rendezvous::RecvFuture<'a, T>`: `Output = Result<T, RecvError>`

## 4. Module `fibre::spmc`

A broadcast channel for one producer and multiple consumers. `T` must be `Send + Clone`.

### spmc functions

*   `pub fn bounded<T: Send + Clone>(capacity: usize) -> (BoundedSyncSender<T>, BoundedSyncReceiver<T>)`: Panics if `capacity == 0`.
*   `pub fn bounded_async<T: Send + Clone>(capacity: usize) -> (BoundedAsyncSender<T>, BoundedAsyncReceiver<T>)`: Panics if `capacity == 0`.

### Struct `spmc::BoundedSyncSender<T>`

The synchronous sending handle. Not `Clone`.

*   `pub fn send(&self, value: T) -> Result<(), SendError>`: Blocks if any consumer's buffer is full.
*   `pub fn try_send(&self, value: T) -> Result<(), TrySendError<T>>`
*   `pub fn send_batch(&self, items: Vec<T>) -> Result<usize, SendBatchError<T>>`: Capacity is bounded by the slowest consumer; one head update and one coalesced waker pass per batch.
*   `pub fn try_send_batch(&self, items: Vec<T>) -> Result<usize, TrySendBatchError<T>>`
*   `pub fn send_batch_mut(&self, items: &mut Vec<T>) -> Result<usize, SendError>`
*   `pub fn try_send_batch_mut(&self, items: &mut Vec<T>) -> Result<usize, SendError>`
*   `pub fn close(&mut self) -> Result<(), CloseError>`
*   `pub fn is_closed(&self) -> bool`
*   `pub fn capacity(&self) -> usize`
*   `pub fn len(&self) -> usize`
*   `pub fn is_empty(&self) -> bool`
*   `pub fn is_full(&self) -> bool`
*   `pub fn to_async(self) -> BoundedAsyncSender<T>`

### Struct `spmc::BoundedSyncReceiver<T>`

The synchronous receiving handle. `Clone`. A clone starts at the position of the handle it was cloned from, so it receives everything that handle has not yet consumed. This differs from `spmc::topic` and from `tokio::sync::broadcast::subscribe`.

*   `pub fn recv(&self) -> Result<T, RecvError>`: Blocks if this consumer's buffer is empty.
*   `pub fn recv_timeout(&self, timeout: Duration) -> Result<T, RecvErrorTimeout>`
*   `pub fn try_recv(&self) -> Result<T, TryRecvError>`
*   `pub fn recv_batch(&self, max: usize) -> Result<Vec<T>, RecvError>`: Each consumer receives (clones) every broadcast item; the consumer tail advances once per batch.
*   `pub fn try_recv_batch(&self, max: usize) -> Result<Vec<T>, TryRecvError>`
*   `pub fn recv_batch_mut(&self, out: &mut Vec<T>, max: usize) -> Result<usize, RecvError>`: Appends to `out`.
*   `pub fn try_recv_batch_mut(&self, out: &mut Vec<T>, max: usize) -> Result<usize, TryRecvError>`: Appends to `out`.
*   `pub fn close(&self) -> Result<(), CloseError>`
*   `pub fn is_closed(&self) -> bool`
*   `pub fn capacity(&self) -> usize`
*   `pub fn len(&self) -> usize`
*   `pub fn is_empty(&self) -> bool`
*   `pub fn is_full(&self) -> bool`
*   `pub fn to_async(self) -> BoundedAsyncReceiver<T>`

### Struct `spmc::BoundedAsyncSender<T>`

The asynchronous sending handle. Not `Clone`.

*   `pub fn send(&self, value: T) -> SendFuture<'_, T>`: Cancel-safe.
*   `pub fn try_send(&self, value: T) -> Result<(), TrySendError<T>>`
*   `pub fn send_batch(&self, items: Vec<T>) -> SendBatchFuture<'_, T>`
*   `pub fn send_batch_mut<'a>(&'a self, items: &'a mut Vec<T>) -> SendBatchMutFuture<'a, T>`
*   `pub fn try_send_batch(&self, items: Vec<T>) -> Result<usize, TrySendBatchError<T>>`
*   `pub fn try_send_batch_mut(&self, items: &mut Vec<T>) -> Result<usize, SendError>`
*   `pub fn close(&mut self) -> Result<(), CloseError>`
*   `pub fn is_closed(&self) -> bool`
*   `pub fn capacity(&self) -> usize`
*   `pub fn len(&self) -> usize`
*   `pub fn is_empty(&self) -> bool`
*   `pub fn is_full(&self) -> bool`
*   `pub fn to_sync(self) -> BoundedSyncSender<T>`

### Struct `spmc::BoundedAsyncReceiver<T>`

The asynchronous receiving handle. `Clone`. Implements `futures::Stream`. A clone starts at the position of the handle it was cloned from, so it receives everything that handle has not yet consumed. This differs from `spmc::topic` and from `tokio::sync::broadcast::subscribe`.

*   `pub fn recv(&self) -> RecvFuture<'_, T>`: Cancel-safe.
*   `pub fn try_recv(&self) -> Result<T, TryRecvError>`
*   `pub fn recv_batch(&self, max: usize) -> RecvBatchFuture<'_, T>`
*   `pub fn recv_batch_mut<'a>(&'a self, out: &'a mut Vec<T>, max: usize) -> RecvBatchMutFuture<'a, T>`
*   `pub fn try_recv_batch(&self, max: usize) -> Result<Vec<T>, TryRecvError>`
*   `pub fn try_recv_batch_mut(&self, out: &mut Vec<T>, max: usize) -> Result<usize, TryRecvError>`
*   `pub fn close(&self) -> Result<(), CloseError>`
*   `pub fn is_closed(&self) -> bool`
*   `pub fn capacity(&self) -> usize`
*   `pub fn len(&self) -> usize`
*   `pub fn is_empty(&self) -> bool`
*   `pub fn is_full(&self) -> bool`
*   `pub fn to_sync(self) -> BoundedSyncReceiver<T>`

### spmc futures

*   `SendFuture<'a, T>`: `Output = Result<(), SendError>`
*   `SendBatchFuture<'a, T>`: `Output = Result<usize, SendBatchError<T>>`
*   `SendBatchMutFuture<'a, T>`: `Output = Result<usize, SendError>`
*   `RecvFuture<'a, T>`: `Output = Result<T, RecvError>`
*   `RecvBatchFuture<'a, T>`: `Output = Result<Vec<T>, RecvError>`
*   `RecvBatchMutFuture<'a, T>`: `Output = Result<usize, RecvError>`

## 5. Module `fibre::spmc::topic`

A publish-subscribe channel: one producer broadcasts to the consumers subscribed to each topic. Available with the `topic` feature, on by default.

*   **Topic-based filtering**: consumers only receive messages for topics they explicitly subscribe to.
*   **Non-blocking sender**: the sender is not blocked by slow consumers. If a consumer's mailbox is full, new messages for that consumer are dropped.
*   **`Clone` requirement**: the message type `T` and topic key `K` must implement `Clone`.
*   **Sync/async agnostic**: handles convert with `to_async` and `to_sync`.
*   **Clone**: a clone gets its own empty mailbox and subscribes to the topics the original holds at that moment, so one send reaches every subscribed clone. A clone sees no messages sent before it subscribed.
*   **Sender lifetime**: dropping a subscriber does not close the sender. `send` returns `Err(SendError::Closed)` only after this handle was closed or every receiver was dropped or closed. A message to a topic with no subscribers is discarded with `Ok(())`.
*   **Mailbox size**: `mailbox_capacity == 0` is accepted and every delivery to that mailbox is dropped.
*   **Disconnection reaches subscribed receivers only**: dropping or closing the sender disconnects mailboxes subscribed at that moment. A receiver with no subscriptions never observes `Disconnected`: `recv` blocks forever and `try_recv` returns `Empty`.
*   **Receives ignore `close()`**: `recv`, `try_recv` and the `Stream` impl do not check the handle's closed flag, so they still return buffered messages after `close()`.

### topic functions

*   `pub fn channel<K, T>(mailbox_capacity: usize) -> (TopicSender<K, T>, TopicReceiver<K, T>) where K: Eq + Hash + Clone + Send + Sync + 'static, T: Send + Clone + 'static`
*   `pub fn channel_async<K, T>(mailbox_capacity: usize) -> (AsyncTopicSender<K, T>, AsyncTopicReceiver<K, T>) where K: Eq + Hash + Clone + Send + Sync + 'static, T: Send + Clone + 'static`

### Struct `topic::TopicSender<K, T>`

The synchronous sending handle. Not `Clone`; `send` takes `&self`, so one handle behind a shared struct serves many publishers.

*   `pub fn send(&self, topic: K, value: T) -> Result<(), SendError>`: Non-blocking. Drops the message for slow consumers.
*   `pub fn close(&self) -> Result<(), CloseError>`
*   `pub fn is_closed(&self) -> bool`: `true` once every receiver has been dropped or closed.
*   `pub fn to_async(self) -> AsyncTopicSender<K, T>`

### Struct `topic::TopicReceiver<K, T>`

The synchronous receiving handle. `Clone`.

*   `pub fn subscribe(&self, topic: K)`
*   `pub fn unsubscribe<Q: ?Sized>(&self, topic: &Q) where K: Borrow<Q> + papaya::Equivalent<Q>, Q: Hash + Eq`
*   `pub fn recv(&self) -> Result<(K, T), RecvError>`: Blocks if this receiver's mailbox is empty.
*   `pub fn try_recv(&self) -> Result<(K, T), TryRecvError>`
*   `pub fn recv_timeout(&self, timeout: Duration) -> Result<(K, T), RecvErrorTimeout>`
*   `pub fn close(&self) -> Result<(), CloseError>`
*   `pub fn is_closed(&self) -> bool`
*   `pub fn capacity(&self) -> usize`: The capacity of this receiver's mailbox.
*   `pub fn is_empty(&self) -> bool`: `true` if this receiver's mailbox is empty.
*   `pub fn to_async(self) -> AsyncTopicReceiver<K, T>`

### Struct `topic::AsyncTopicSender<K, T>`

The asynchronous sending handle. Not `Clone`; `send` takes `&self`, so one handle behind a shared struct serves many publishers.

*   `pub fn send(&self, topic: K, value: T) -> Result<(), SendError>`: Non-blocking; not a future.
*   `pub fn close(&self) -> Result<(), CloseError>`
*   `pub fn is_closed(&self) -> bool`: `true` once every receiver has been dropped or closed.
*   `pub fn to_sync(self) -> TopicSender<K, T>`

### Struct `topic::AsyncTopicReceiver<K, T>`

The asynchronous receiving handle. `Clone`. Implements `futures::Stream`.

*   `pub fn subscribe(&self, topic: K)`
*   `pub fn unsubscribe<Q: ?Sized>(&self, topic: &Q) where K: Borrow<Q> + papaya::Equivalent<Q>, Q: Hash + Eq`
*   `pub fn recv(&self) -> RecvFuture<'_, (K, T)>`: Cancel-safe. The future type is not re-exported, so callers cannot name it; its `Output` is `Result<(K, T), RecvError>`.
*   `pub fn try_recv(&self) -> Result<(K, T), TryRecvError>`
*   `pub fn close(&self) -> Result<(), CloseError>`
*   `pub fn is_closed(&self) -> bool`
*   `pub fn capacity(&self) -> usize`: The capacity of this receiver's mailbox.
*   `pub fn is_empty(&self) -> bool`: `true` if this receiver's mailbox is empty.
*   `pub fn to_sync(self) -> TopicReceiver<K, T>`

## 6. Module `fibre::mpmc`

Channels for many producers and many consumers. This module is also reachable as `fibre::mpmc_v2`. `BoundedSyncSender`, `BoundedSyncReceiver`, `BoundedAsyncSender` and `BoundedAsyncReceiver` are re-exports of `Sender`, `Receiver`, `AsyncSender` and `AsyncReceiver`. The rendezvous handles are also re-exported from `fibre::mpmc`.

### mpmc functions

*   `pub fn bounded<T: Send>(capacity: usize) -> (Sender<T>, Receiver<T>)`: Panics if `capacity == 0`; use `rendezvous`.
*   `pub fn bounded_async<T: Send>(capacity: usize) -> (AsyncSender<T>, AsyncReceiver<T>)`: Panics if `capacity == 0`; use `rendezvous`.
*   `pub fn unbounded<T: Send>() -> (UnboundedSyncSender<T>, UnboundedSyncReceiver<T>)`
*   `pub fn unbounded_async<T: Send>() -> (UnboundedAsyncSender<T>, UnboundedAsyncReceiver<T>)`
*   `pub fn rendezvous::rendezvous<T: Send>() -> (RendezvousSyncSender<T>, RendezvousSyncReceiver<T>)`: Zero-capacity direct handoff.
*   `pub fn rendezvous::rendezvous_async<T: Send>() -> (RendezvousAsyncSender<T>, RendezvousAsyncReceiver<T>)`

### Struct `mpmc::Sender<T>`

The synchronous `Clone` sending handle of the bounded channel.

*   `pub fn send(&self, item: T) -> Result<(), SendError>`: Blocks if the channel is full.
*   `pub fn try_send(&self, item: T) -> Result<(), TrySendError<T>>`
*   `pub fn send_batch(&self, items: Vec<T>) -> Result<usize, SendBatchError<T>>`: The items that fit are processed under one lock acquisition; satisfied receivers are woken in one coalesced pass after the lock is released.
*   `pub fn try_send_batch(&self, items: Vec<T>) -> Result<usize, TrySendBatchError<T>>`
*   `pub fn send_batch_mut(&self, items: &mut Vec<T>) -> Result<usize, SendError>`
*   `pub fn try_send_batch_mut(&self, items: &mut Vec<T>) -> Result<usize, SendError>`
*   `pub fn close(&self) -> Result<(), CloseError>`: Per handle, equivalent to dropping it. Later sends on this handle fail with `Closed`. It does not stop other handles or wake senders parked on them. `is_closed` still reads `false` on this handle while receivers remain.
*   `pub fn is_closed(&self) -> bool`: `true` once every receiver has been dropped or closed.
*   `pub fn capacity(&self) -> usize`
*   `pub fn len(&self) -> usize`
*   `pub fn is_empty(&self) -> bool`
*   `pub fn is_full(&self) -> bool`
*   `pub fn to_async(self) -> AsyncSender<T>`

### Struct `mpmc::Receiver<T>`

The synchronous `Clone` receiving handle of the bounded channel.

*   `pub fn recv(&self) -> Result<T, RecvError>`: Blocks if the channel is empty.
*   `pub fn recv_timeout(&self, timeout: Duration) -> Result<T, RecvErrorTimeout>`: Does not check this handle's closed flag, so it drains buffered items after `close()`.
*   `pub fn try_recv(&self) -> Result<T, TryRecvError>`
*   `pub fn recv_batch(&self, max: usize) -> Result<Vec<T>, RecvError>`
*   `pub fn try_recv_batch(&self, max: usize) -> Result<Vec<T>, TryRecvError>`
*   `pub fn recv_batch_mut(&self, out: &mut Vec<T>, max: usize) -> Result<usize, RecvError>`: Appends to `out`.
*   `pub fn try_recv_batch_mut(&self, out: &mut Vec<T>, max: usize) -> Result<usize, TryRecvError>`: Appends to `out`.
*   `pub fn close(&self) -> Result<(), CloseError>`: Per handle, equivalent to dropping it. The channel closes for senders only once every receiver handle is closed or dropped.
*   `pub fn is_closed(&self) -> bool`
*   `pub fn capacity(&self) -> usize`
*   `pub fn len(&self) -> usize`
*   `pub fn is_empty(&self) -> bool`
*   `pub fn is_full(&self) -> bool`
*   `pub fn to_async(self) -> AsyncReceiver<T>`

### Struct `mpmc::AsyncSender<T>`

The asynchronous `Clone` sending handle of the bounded channel.

*   `pub fn send(&self, item: T) -> SendFuture<'_, T>`: Cancel-safe.
*   `pub fn try_send(&self, item: T) -> Result<(), TrySendError<T>>`
*   `pub fn send_batch(&self, items: Vec<T>) -> SendBatchFuture<'_, T>`
*   `pub fn send_batch_mut<'a>(&'a self, items: &'a mut Vec<T>) -> SendBatchMutFuture<'a, T>`: Cancel-safe.
*   `pub fn try_send_batch(&self, items: Vec<T>) -> Result<usize, TrySendBatchError<T>>`
*   `pub fn try_send_batch_mut(&self, items: &mut Vec<T>) -> Result<usize, SendError>`
*   `pub fn close(&self) -> Result<(), CloseError>`: Per handle, equivalent to dropping it. Later sends on this handle fail with `Closed`. It does not stop other handles or wake senders parked on them. `is_closed` still reads `false` on this handle while receivers remain.
*   `pub fn is_closed(&self) -> bool`: `true` once every receiver has been dropped or closed.
*   `pub fn capacity(&self) -> usize`
*   `pub fn len(&self) -> usize`
*   `pub fn is_empty(&self) -> bool`
*   `pub fn is_full(&self) -> bool`
*   `pub fn to_sync(self) -> Sender<T>`

### Struct `mpmc::AsyncReceiver<T>`

The asynchronous `Clone` receiving handle of the bounded channel. Implements `futures::Stream`.

*   `pub fn recv(&self) -> RecvFuture<'_, T>`: Cancel-safe. Does not check this handle's closed flag, so it drains buffered items after `close()`.
*   `pub fn recv_batch(&self, max: usize) -> RecvBatchFuture<'_, T>`: Cancel-safe. Does not check this handle's closed flag.
*   `pub fn recv_batch_mut<'a>(&'a self, out: &'a mut Vec<T>, max: usize) -> RecvBatchMutFuture<'a, T>`: Cancel-safe. Does not check this handle's closed flag.
*   `pub fn try_recv(&self) -> Result<T, TryRecvError>`
*   `pub fn try_recv_batch(&self, max: usize) -> Result<Vec<T>, TryRecvError>`
*   `pub fn try_recv_batch_mut(&self, out: &mut Vec<T>, max: usize) -> Result<usize, TryRecvError>`
*   `pub fn close(&self) -> Result<(), CloseError>`: Per handle, equivalent to dropping it. The channel closes for senders only once every receiver handle is closed or dropped.
*   `pub fn is_closed(&self) -> bool`
*   `pub fn capacity(&self) -> usize`
*   `pub fn len(&self) -> usize`
*   `pub fn is_empty(&self) -> bool`
*   `pub fn is_full(&self) -> bool`
*   `pub fn to_sync(self) -> Receiver<T>`

### Struct `mpmc::UnboundedSyncSender<T>`

All four unbounded handles are `Clone`. Sends and waiting receives take `&mut self` because each handle owns its bump slab (senders) or cached waiter cell (receivers); clone a handle per thread or task. A send takes no lock unless a receiver is parked. Every receive takes the consumer mutex.

*   `pub fn send(&mut self, value: T) -> Result<(), SendError>`: Never blocks.
*   `pub fn try_send(&mut self, value: T) -> Result<(), TrySendError<T>>`
*   `pub fn send_batch(&mut self, items: Vec<T>) -> Result<usize, SendBatchError<T>>`: One publish and one wake per batch: only one parked receiver wakes. Other parked receivers stay blocked with items queued until another send or a disconnect wakes them. Single sends wake one receiver per item.
*   `pub fn try_send_batch(&mut self, items: Vec<T>) -> Result<usize, TrySendBatchError<T>>`: Same wake rule as `send_batch`.
*   `pub fn send_batch_mut(&mut self, items: &mut Vec<T>) -> Result<usize, SendError>`: Same wake rule as `send_batch`.
*   `pub fn try_send_batch_mut(&mut self, items: &mut Vec<T>) -> Result<usize, SendError>`: Same wake rule as `send_batch`.
*   `pub fn close(&mut self) -> Result<(), CloseError>`
*   `pub fn is_closed(&self) -> bool`
*   `pub fn sender_count(&self) -> usize`
*   `pub fn capacity(&self) -> usize`: Always `usize::MAX`.
*   `pub fn len(&self) -> usize`
*   `pub fn is_empty(&self) -> bool`
*   `pub fn is_full(&self) -> bool`: Always `false`.
*   `pub fn to_async(self) -> UnboundedAsyncSender<T>`

### Struct `mpmc::UnboundedSyncReceiver<T>`

The synchronous `Clone` receiving handle of the unbounded channel.

*   `pub fn recv(&mut self) -> Result<T, RecvError>`: Blocks if empty, including while items queued by a `send_batch` wait for a wake.
*   `pub fn recv_timeout(&mut self, timeout: Duration) -> Result<T, RecvErrorTimeout>`
*   `pub fn recv_batch(&mut self, max: usize) -> Result<Vec<T>, RecvError>`: Blocks until at least one item.
*   `pub fn recv_batch_mut(&mut self, out: &mut Vec<T>, max: usize) -> Result<usize, RecvError>`: Blocks until at least one item.
*   `pub fn try_recv(&self) -> Result<T, TryRecvError>`
*   `pub fn try_recv_batch(&self, max: usize) -> Result<Vec<T>, TryRecvError>`
*   `pub fn try_recv_batch_mut(&self, out: &mut Vec<T>, max: usize) -> Result<usize, TryRecvError>`
*   `pub fn close(&self) -> Result<(), CloseError>`
*   `pub fn is_closed(&self) -> bool`
*   `pub fn sender_count(&self) -> usize`
*   `pub fn capacity(&self) -> usize`: Always `usize::MAX`.
*   `pub fn len(&self) -> usize`
*   `pub fn is_empty(&self) -> bool`
*   `pub fn is_full(&self) -> bool`: Always `false`.
*   `pub fn to_async(self) -> UnboundedAsyncReceiver<T>`

### Struct `mpmc::UnboundedAsyncSender<T>`

The asynchronous `Clone` sending handle of the unbounded channel.

*   `pub fn send(&mut self, value: T) -> UnboundedSendFuture<'_, T>`: Resolves on first poll. Cancel-safe.
*   `pub fn try_send(&mut self, value: T) -> Result<(), TrySendError<T>>`
*   `pub fn send_batch(&mut self, items: Vec<T>) -> UnboundedSendBatchFuture<'_, T>`
*   `pub fn send_batch_mut<'a>(&'a mut self, items: &'a mut Vec<T>) -> UnboundedSendBatchMutFuture<'a, T>`
*   `pub fn try_send_batch(&mut self, items: Vec<T>) -> Result<usize, TrySendBatchError<T>>`
*   `pub fn try_send_batch_mut(&mut self, items: &mut Vec<T>) -> Result<usize, SendError>`
*   `pub fn close(&mut self) -> Result<(), CloseError>`
*   `pub fn is_closed(&self) -> bool`
*   `pub fn sender_count(&self) -> usize`
*   `pub fn capacity(&self) -> usize`: Always `usize::MAX`.
*   `pub fn len(&self) -> usize`
*   `pub fn is_empty(&self) -> bool`
*   `pub fn is_full(&self) -> bool`: Always `false`.
*   `pub fn to_sync(self) -> UnboundedSyncSender<T>`

### Struct `mpmc::UnboundedAsyncReceiver<T>`

The asynchronous `Clone` receiving handle of the unbounded channel. Implements `futures::Stream`.

*   `pub fn recv(&mut self) -> UnboundedRecvFuture<'_, T>`: Cancel-safe.
*   `pub fn recv_batch(&mut self, max: usize) -> UnboundedRecvBatchFuture<'_, T>`: Cancel-safe.
*   `pub fn recv_batch_mut<'a>(&'a mut self, out: &'a mut Vec<T>, max: usize) -> UnboundedRecvBatchMutFuture<'a, T>`: Cancel-safe.
*   `pub fn try_recv(&self) -> Result<T, TryRecvError>`
*   `pub fn try_recv_batch(&self, max: usize) -> Result<Vec<T>, TryRecvError>`
*   `pub fn try_recv_batch_mut(&self, out: &mut Vec<T>, max: usize) -> Result<usize, TryRecvError>`
*   `pub fn close(&self) -> Result<(), CloseError>`
*   `pub fn is_closed(&self) -> bool`
*   `pub fn sender_count(&self) -> usize`
*   `pub fn capacity(&self) -> usize`: Always `usize::MAX`.
*   `pub fn len(&self) -> usize`
*   `pub fn is_empty(&self) -> bool`
*   `pub fn is_full(&self) -> bool`: Always `false`.
*   `pub fn to_sync(self) -> UnboundedSyncReceiver<T>`

### mpmc futures

*   `SendFuture<'a, T>`, `UnboundedSendFuture<'a, T>`: `Output = Result<(), SendError>`
*   `SendBatchFuture<'a, T>`, `UnboundedSendBatchFuture<'a, T>`: `Output = Result<usize, SendBatchError<T>>`
*   `SendBatchMutFuture<'a, T>`, `UnboundedSendBatchMutFuture<'a, T>`: `Output = Result<usize, SendError>`
*   `RecvFuture<'a, T>`, `UnboundedRecvFuture<'a, T>`: `Output = Result<T, RecvError>`
*   `RecvBatchFuture<'a, T>`, `UnboundedRecvBatchFuture<'a, T>`: `Output = Result<Vec<T>, RecvError>`
*   `RecvBatchMutFuture<'a, T>`, `UnboundedRecvBatchMutFuture<'a, T>`: `Output = Result<usize, RecvError>`

### Struct `mpmc::rendezvous::RendezvousSyncSender<T>`

Synchronous sender. `Clone`. No batch API.

*   `pub fn send(&self, item: T) -> Result<(), SendError>`
*   `pub fn try_send(&self, item: T) -> Result<(), TrySendError<T>>`
*   `pub fn close(&self) -> Result<(), CloseError>`
*   `pub fn is_closed(&self) -> bool`
*   `pub fn len(&self) -> usize`
*   `pub fn is_empty(&self) -> bool`
*   `pub fn is_full(&self) -> bool`
*   `pub fn capacity(&self) -> Option<usize>`: Always `Some(0)`.
*   `pub fn to_async(self) -> RendezvousAsyncSender<T>`

### Struct `mpmc::rendezvous::RendezvousSyncReceiver<T>`

Synchronous receiver. `Clone`. No batch API.

*   `pub fn recv(&self) -> Result<T, RecvError>`
*   `pub fn try_recv(&self) -> Result<T, TryRecvError>`
*   `pub fn recv_timeout(&self, timeout: Duration) -> Result<T, RecvErrorTimeout>`
*   `pub fn close(&self) -> Result<(), CloseError>`
*   `pub fn is_closed(&self) -> bool`
*   `pub fn len(&self) -> usize`
*   `pub fn is_empty(&self) -> bool`
*   `pub fn is_full(&self) -> bool`
*   `pub fn capacity(&self) -> Option<usize>`: Always `Some(0)`.
*   `pub fn to_async(self) -> RendezvousAsyncReceiver<T>`

### Struct `mpmc::rendezvous::RendezvousAsyncSender<T>`

Asynchronous sender. `Clone`. No batch API.

*   `pub fn send(&self, item: T) -> SendFuture<'_, T>`: Not cancel-safe: a dropped send can still be delivered.
*   `pub fn try_send(&self, item: T) -> Result<(), TrySendError<T>>`
*   `pub fn close(&self) -> Result<(), CloseError>`
*   `pub fn is_closed(&self) -> bool`
*   `pub fn len(&self) -> usize`
*   `pub fn is_empty(&self) -> bool`
*   `pub fn is_full(&self) -> bool`
*   `pub fn capacity(&self) -> Option<usize>`: Always `Some(0)`.
*   `pub fn to_sync(self) -> RendezvousSyncSender<T>`

### Struct `mpmc::rendezvous::RendezvousAsyncReceiver<T>`

Asynchronous receiver. `Clone`. No batch API. Does not implement `futures::Stream`.

*   `pub fn recv(&self) -> RecvFuture<'_, T>`: Not cancel-safe: a dropped receive can lose an item already handed to it.
*   `pub fn try_recv(&self) -> Result<T, TryRecvError>`
*   `pub fn close(&self) -> Result<(), CloseError>`
*   `pub fn is_closed(&self) -> bool`
*   `pub fn len(&self) -> usize`
*   `pub fn is_empty(&self) -> bool`
*   `pub fn is_full(&self) -> bool`
*   `pub fn capacity(&self) -> Option<usize>`: Always `Some(0)`.
*   `pub fn to_sync(self) -> RendezvousSyncReceiver<T>`

### mpmc rendezvous futures

*   `rendezvous::SendFuture<'a, T>`: `Output = Result<(), SendError>`
*   `rendezvous::RecvFuture<'a, T>`: `Output = Result<T, RecvError>`

## 7. Module `fibre::mpmc_exp`

An experimental lock-free bounded MPMC channel. `capacity` is rounded up to the next power of two. `rendezvous`, `unbounded` and `unbounded_async` are re-exports of the `fibre::mpmc` items.

### mpmc_exp functions

*   `pub fn bounded<T: Send>(capacity: usize) -> (Sender<T>, Receiver<T>)`: Panics if `capacity == 0`.
*   `pub fn bounded_async<T: Send>(capacity: usize) -> (AsyncSender<T>, AsyncReceiver<T>)`: Panics if `capacity == 0`.

### Struct `mpmc_exp::Sender<T>`

A synchronous `Clone` sending handle.

*   `pub fn send(&self, item: T) -> Result<(), SendError>`
*   `pub fn try_send(&self, item: T) -> Result<(), TrySendError<T>>`
*   `pub fn try_send_batch(&self, items: Vec<T>) -> Result<usize, TrySendBatchError<T>>`
*   `pub fn try_send_batch_mut(&self, items: &mut Vec<T>) -> Result<usize, SendError>`
*   `pub fn send_batch(&self, items: Vec<T>) -> Result<usize, SendBatchError<T>>`
*   `pub fn send_batch_mut(&self, items: &mut Vec<T>) -> Result<usize, SendError>`
*   `pub fn close(&self) -> Result<(), CloseError>`
*   `pub fn is_closed(&self) -> bool`
*   `pub fn capacity(&self) -> Option<usize>`
*   `pub fn len(&self) -> usize`
*   `pub fn is_empty(&self) -> bool`
*   `pub fn is_full(&self) -> bool`
*   `pub fn to_async(self) -> AsyncSender<T>`

### Struct `mpmc_exp::Receiver<T>`

A synchronous `Clone` receiving handle.

*   `pub fn recv(&self) -> Result<T, RecvError>`
*   `pub fn try_recv(&self) -> Result<T, TryRecvError>`
*   `pub fn recv_timeout(&self, timeout: Duration) -> Result<T, RecvErrorTimeout>`
*   `pub fn try_recv_batch(&self, max: usize) -> Result<Vec<T>, TryRecvError>`
*   `pub fn try_recv_batch_mut(&self, out: &mut Vec<T>, max: usize) -> Result<usize, TryRecvError>`
*   `pub fn recv_batch(&self, max: usize) -> Result<Vec<T>, RecvError>`
*   `pub fn recv_batch_mut(&self, out: &mut Vec<T>, max: usize) -> Result<usize, RecvError>`
*   `pub fn debug_state(&self) -> (usize, usize, usize)`: Buffered length, parked send waiters and parked receive waiters.
*   `pub fn close(&self) -> Result<(), CloseError>`
*   `pub fn is_closed(&self) -> bool`
*   `pub fn capacity(&self) -> Option<usize>`
*   `pub fn len(&self) -> usize`
*   `pub fn is_empty(&self) -> bool`
*   `pub fn is_full(&self) -> bool`
*   `pub fn to_async(self) -> AsyncReceiver<T>`

### Struct `mpmc_exp::AsyncSender<T>`

An asynchronous `Clone` sending handle.

*   `pub fn send(&self, item: T) -> SendFuture<'_, T>`
*   `pub fn try_send(&self, item: T) -> Result<(), TrySendError<T>>`
*   `pub fn try_send_batch(&self, items: Vec<T>) -> Result<usize, TrySendBatchError<T>>`
*   `pub fn try_send_batch_mut(&self, items: &mut Vec<T>) -> Result<usize, SendError>`
*   `pub fn send_batch(&self, items: Vec<T>) -> SendBatchFuture<'_, T>`
*   `pub fn send_batch_mut<'a>(&'a self, items: &'a mut Vec<T>) -> SendBatchMutFuture<'a, T>`
*   `pub fn close(&self) -> Result<(), CloseError>`
*   `pub fn is_closed(&self) -> bool`
*   `pub fn capacity(&self) -> Option<usize>`
*   `pub fn len(&self) -> usize`
*   `pub fn is_empty(&self) -> bool`
*   `pub fn is_full(&self) -> bool`
*   `pub fn to_sync(self) -> Sender<T>`

### Struct `mpmc_exp::AsyncReceiver<T>`

An asynchronous `Clone` receiving handle. Implements `futures::Stream`.

*   `pub fn recv(&self) -> RecvFuture<'_, T>`
*   `pub fn try_recv(&self) -> Result<T, TryRecvError>`
*   `pub fn try_recv_batch(&self, max: usize) -> Result<Vec<T>, TryRecvError>`
*   `pub fn try_recv_batch_mut(&self, out: &mut Vec<T>, max: usize) -> Result<usize, TryRecvError>`
*   `pub fn recv_batch(&self, max: usize) -> RecvBatchFuture<'_, T>`
*   `pub fn recv_batch_mut<'a>(&'a self, out: &'a mut Vec<T>, max: usize) -> RecvBatchMutFuture<'a, T>`
*   `pub fn close(&self) -> Result<(), CloseError>`
*   `pub fn is_closed(&self) -> bool`
*   `pub fn capacity(&self) -> Option<usize>`
*   `pub fn len(&self) -> usize`
*   `pub fn is_empty(&self) -> bool`
*   `pub fn is_full(&self) -> bool`
*   `pub fn to_sync(self) -> Receiver<T>`

### mpmc_exp futures

*   `SendFuture<'a, T>`, `SendBatchFuture<'a, T>`, `SendBatchMutFuture<'a, T>`, `RecvFuture<'a, T>`, `RecvBatchFuture<'a, T>`, `RecvBatchMutFuture<'a, T>`: Same outputs as the `fibre::mpmc` futures of those names.

## 8. Module `fibre::sync`

Locks that wait either way: a synchronous caller parks its thread and an asynchronous caller yields to its executor with a waker, on the same lock at the same time.

*   **One lock, two waits**: `lock`, `read` and `write` block the calling thread; `lock_async`, `read_async` and `write_async` return futures. Any mix of the two may contend on one lock.
*   **Guards may cross an `.await`**: a guard is `Send` whenever the lock is `Sync`. A waiter for a conflicting hold waits until that guard drops and a blocking call on the holder's executor thread can deadlock.
*   **Cancellation**: dropping a pending lock future unlinks its waiter. A wake it had already received is passed on to the next waiter.
*   **Fairness**: `HybridMutex` wakes the queue head, which stays linked while it contends again; a loser re-arms and waits for the next wake. The read-write lock wakes the first queued writer and leaves it linked. With no writer queued it unlinks and wakes every reader. While a writer is queued new readers are gated behind it. A workload of reads alone never touches the queue.
*   **No poisoning**: a panic while holding a guard releases the lock on unwind.

### Struct `sync::HybridMutex<T>`

A mutual-exclusion lock. `Send` when `T: Send`; `Sync` when `T: Send`.

*   `pub fn new(data: T) -> Self`
*   `pub fn lock(&self) -> MutexGuard<'_, T>`: Blocks the calling thread until the lock is held.
*   `pub async fn lock_async(&self) -> MutexGuard<'_, T>`: Waits without blocking the executor. The future is `Send` when `T: Send`.
*   `pub fn try_lock(&self) -> Option<MutexGuard<'_, T>>`: One acquisition attempt, never waits.
*   `pub fn get_mut(&mut self) -> &mut T`: The data without locking.

### Struct `sync::MutexGuard<'a, T>`

Holds the lock until dropped. Implements `Deref<Target = T>` and `DerefMut`.

### Struct `sync::HybridRwLock<T>`

A reader-writer lock: any number of readers or one writer. `Send` when `T: Send`; `Sync` when `T: Send + Sync`.

*   `pub fn new(data: T) -> Self`
*   `pub fn read(&self) -> ReadGuard<'_, T>`: Blocks the calling thread until a shared hold is granted.
*   `pub fn write(&self) -> WriteGuard<'_, T>`: Blocks the calling thread until the exclusive hold is granted.
*   `pub async fn read_async(&self) -> ReadGuard<'_, T>`: Waits without blocking the executor. The future is `Send` when `T: Send + Sync`.
*   `pub async fn write_async(&self) -> WriteGuard<'_, T>`: Waits without blocking the executor. The future is `Send` when `T: Send + Sync`.
*   `pub fn try_read(&self) -> Option<ReadGuard<'_, T>>`: One attempt, never waits.
*   `pub fn try_write(&self) -> Option<WriteGuard<'_, T>>`: One attempt, never waits.

### Struct `sync::ReadGuard<'a, T>`

A shared hold, released on drop. Implements `Deref<Target = T>`.

### Struct `sync::WriteGuard<'a, T>`

The exclusive hold, released on drop. Implements `Deref<Target = T>` and `DerefMut`.

## 9. Module `fibre::sync::v2`

A reader-writer lock with the same names and methods as `fibre::sync::HybridRwLock` (§8). Waiters queue and wake by the §8 rules, with one exception: a writer that has taken the lock waits for slot readers without parking. `write` and `try_write` spin and yield the thread. `write_async` re-polls itself through its waker. A read guard held across an `.await` keeps that writer busy until the guard drops.

### Struct `v2::HybridRwLock<T>`

A reader-writer lock whose readers normally count themselves in one of 64 per-thread slots, each on its own cache line, so readers on different slots share no written word. Threads beyond 64 share slots. After a writer runs, readers use the shared reader count until about 4096 such reads turn the slots back on. `new` allocates all 64 slots. `Send` when `T: Send`; `Sync` when `T: Send + Sync`.

*   `pub fn new(data: T) -> Self`
*   `pub fn read(&self) -> ReadGuard<'_, T>`: Blocks the calling thread until a shared hold is granted.
*   `pub fn write(&self) -> WriteGuard<'_, T>`: Blocks the calling thread until the exclusive hold is granted.
*   `pub async fn read_async(&self) -> ReadGuard<'_, T>`: Waits without blocking the executor. The future is `Send` when `T: Send + Sync`.
*   `pub async fn write_async(&self) -> WriteGuard<'_, T>`: Waits without blocking the executor. The future is `Send` when `T: Send + Sync`.
*   `pub fn try_read(&self) -> Option<ReadGuard<'_, T>>`: One attempt, never waits.
*   `pub fn try_write(&self) -> Option<WriteGuard<'_, T>>`: One attempt at the lock. It returns `None` only when a writer holds the lock or a reader holds the shared count. Once it has the lock it waits for slot readers to finish, so it hangs if the calling thread holds a read guard.

### Struct `v2::ReadGuard<'a, T>`

A shared hold, released on drop. Implements `Deref<Target = T>`.

### Struct `v2::WriteGuard<'a, T>`

The exclusive hold, released on drop. Implements `Deref<Target = T>` and `DerefMut`.

## 10. Module `fibre::telemetry`

Event recording behind the `diagnostics` feature. The functions and macros are no-ops without the feature. Only `fibre::spmc` emits events.

### telemetry functions

*   `pub fn log_event(item_id: Option<usize>, location: &str, event_type: &str, message: Option<String>)`
*   `pub fn log_stall_event(location: &'static str, event_type: &'static str, state_val: usize, head: usize, tail: usize)`
*   `pub fn increment_counter(location: &str, counter_name: &str)`
*   `pub fn print_telemetry_report()`
*   `pub fn print_stall_report()`
*   `pub fn clear_telemetry()`

### telemetry macros

Exported at the crate root.

*   `log_event!(item_id, location, event_type)` and `log_event!(item_id, location, event_type, format_args...)`: The format arguments are skipped entirely without `diagnostics`.
*   `log_stall!(location, event_type, state, head, tail)`: Casts the last three arguments to `usize`.
*   `increment_counter!(location, counter_name)`

### Struct `telemetry::enabled::TelemetryEvent`

A recorded event. Exists only with the `diagnostics` feature. The functions above are also reachable under their `_fn` names in `telemetry::enabled` (with the feature) or `telemetry::disabled` (without it).

*   `pub seq_id: usize`
*   `pub timestamp: Instant`
*   `pub os_thread_id: ThreadId`
*   `pub tokio_task_id: Option<tokio::task::Id>`
*   `pub item_id: Option<usize>`
*   `pub location: String`
*   `pub event_type: String`
*   `pub message: Option<String>`

## 11. Batch Operation Semantics

`spsc`, `mpsc`, `spmc` and `mpmc` bounded and unbounded handles provide eight batch operations per handle pair. `oneshot`, `spmc::topic` and the rendezvous channels do not provide batch operations.

### By-value send

`try_send_batch(items: Vec<T>)` and `send_batch(items: Vec<T>)`.

*   `Ok(n)` means every item was sent.
*   On interruption the error carries the count sent and the unsent remainder.

### In-place send

`try_send_batch_mut(&mut Vec<T>)` and `send_batch_mut(&mut Vec<T>)`.

*   Sent items are drained from the front of the caller's vector; unsent items always remain in it.
*   `Err(SendError::Closed)` from a `try` variant means zero items were sent by that call.

### By-value receive

`try_recv_batch(max)` and `recv_batch(max)`.

*   Returns 1..=`max` items in FIFO order.
*   Blocking and async variants wait until at least one item is available, then drain up to `max` without further waiting.

### In-place receive

`try_recv_batch_mut(&mut Vec<T>, max)` and `recv_batch_mut(&mut Vec<T>, max)`.

*   Appends to the end of the caller's vector and returns the count appended.

### Batch edge cases

*   An empty input vector or `max == 0` returns `Ok(0)` / an empty vector immediately on an open channel.
*   On a closed channel `mpmc::Sender::send_batch`, `mpmc::Receiver::recv_batch`, `mpmc::Receiver::recv_batch_mut` and `spsc::BoundedSyncSender::send_batch` return their closed error instead.

### Batch cancellation

*   Dropping a pending by-value async send-batch future drops the unsent remainder.
*   The `_mut` send futures are cancel-safe: unsent items remain in the caller's vector.
*   All batch receive futures are cancel-safe.

## 12. Error Handling

All error types live in `fibre::error` and are re-exported from the crate root.

### Enum `error::TrySendError<T>`

Returned by non-blocking `try_send` methods.

*   `Full(T)`: The channel is full and cannot accept an item.
*   `Closed(T)`: The channel is closed because the receiver(s) have dropped or this handle was closed.
*   `Sent(T)`: (Oneshot only) A value has already been sent on this channel.
*   `pub fn into_inner(self) -> T`: Consumes the error, returning the inner value that could not be sent.

### Enum `error::SendError`

Returned by blocking or `async` `send` methods.

*   `Closed`: The channel is closed because the receiver(s) have dropped or this handle was closed.
*   `Sent`: (Oneshot only) A value has already been sent.

### Enum `error::TryRecvError`

Returned by non-blocking `try_recv` methods.

*   `Empty`: The channel is currently empty but still active.
*   `Disconnected`: The channel is empty and all senders have dropped or this receiver handle was closed.

### Enum `error::RecvError`

Returned by blocking or `async` `recv` methods.

*   `Disconnected`: The channel is empty and all senders have dropped or this receiver handle was closed.

### Enum `error::RecvErrorTimeout`

Returned by `recv_timeout` methods.

*   `Disconnected`: The channel became disconnected during the wait.
*   `Timeout`: The timeout elapsed before an item could be received.

### Struct `error::CloseError`

A unit struct returned when `close()` is called on an already-closed handle.

### Struct `error::TrySendBatchError<T>`

Returned by non-blocking `try_send_batch` methods. Carries partial-progress state.

*   `pub sent: usize`: The number of items successfully sent before the operation stopped.
*   `pub unsent: Vec<T>`: The items that were not sent, in their original order.
*   `pub reason: BatchSendErrorReason`: Why the batch stopped early.
*   `pub fn into_unsent(self) -> Vec<T>`: Consumes the error, returning the unsent items.

### Struct `error::SendBatchError<T>`

Returned by blocking or `async` `send_batch` methods. The only failure cause is channel closure.

*   `pub sent: usize`: The number of items successfully sent before the channel closed.
*   `pub unsent: Vec<T>`: The items that were not sent, in their original order.
*   `pub fn into_unsent(self) -> Vec<T>`: Consumes the error, returning the unsent items.

### Enum `error::BatchSendErrorReason`

*   `Full`: The channel was full.
*   `Closed`: The channel is closed because all receivers have been dropped.
