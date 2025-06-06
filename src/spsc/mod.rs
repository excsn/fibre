// src/spsc/mod.rs

//! Single-Sender, Single-Receiver (SPSC) channels.
//!
//! These channels are optimized for the case where there is only one sender (producer)
//! and only one receiver (consumer). They offer the highest throughput and lowest
//! latency for this specific 1-to-1 communication pattern.
//!
//! Both synchronous and asynchronous bounded SPSC channels are provided.
//! They share an underlying core implementation (`SpscShared`) ensuring
//! consistent behavior and allowing for conversion between synchronous and
//! asynchronous channel ends if needed (though direct conversion methods like
//! `to_async` are typically added to the producer/consumer structs themselves for
//! public API convenience, which is done in `bounded_sync.rs` and `bounded_async.rs`).
//!
//! # Features
//! - **Bounded**: Channels have a fixed capacity set at creation.
//! - **Blocking/Async**: Both blocking synchronous operations and non-blocking
//!   asynchronous (Future-based) operations are supported.
//! - **Drop Safety**: Dropping either the producer or consumer will correctly
//!   signal the other end (e.g., consumer receives `Disconnected`, producer's
//!   send attempts result in `Closed`).
//! - **Cache-Padding**: Internal head/tail pointers and parking flags are
//!   cache-line padded to reduce false sharing in scenarios where producer and
//!   consumer might run on different cores.
//!
//! # Examples
//!
//! ### Synchronous SPSC Channel
//!
//! ```
//! use fibre::spsc;
//! use std::thread;
//!
//! // Create a bounded synchronous SPSC channel with a capacity of 5.
//! let (mut producer, mut consumer) = spsc::bounded_sync(5);
//!
//! let producer_handle = thread::spawn(move || {
//!     for i in 0..10 {
//!         match producer.send(format!("Sync Item {}", i)) {
//!             Ok(()) => println!("[Sync Sender] Sent item {}", i),
//!             Err(e) => {
//!                 eprintln!("[Sync Sender] Send error: {:?}", e);
//!                 break;
//!             }
//!         }
//!         // The producer will block here if the queue is full.
//!     }
//! });
//!
//! let consumer_handle = thread::spawn(move || {
//!     for _ in 0..10 {
//!         match consumer.recv() {
//!             Ok(item) => println!("[Sync Receiver] Received: {}", item),
//!             Err(e) => {
//!                 eprintln!("[Sync Receiver] Recv error: {:?}", e);
//!                 break;
//!             }
//!         }
//!     }
//! });
//!
//! producer_handle.join().unwrap();
//! consumer_handle.join().unwrap();
//! ```
//!
//! ### Asynchronous SPSC Channel
//!
//! ```
//! use fibre::spsc;
//!
//! async fn async_spsc_example() {
//!     // Create a bounded asynchronous SPSC channel with a capacity of 3.
//!     let (producer, mut consumer) = spsc::bounded_async(3);
//!
//!     let producer_task = tokio::spawn(async move {
//!         for i in 0..7 {
//!             if let Err(e) = producer.send(format!("Async Item {}", i)).await {
//!                 eprintln!("[Async Sender] Send error: {:?}", e);
//!                 break;
//!             }
//!             println!("[Async Sender] Sent item {}", i);
//!             // The producer's send future will complete when there's space.
//!             // tokio::task::yield_now().await; // Optionally yield
//!         }
//!     });
//!
//!     let consumer_task = tokio::spawn(async move {
//!         for _ in 0..7 {
//!             match consumer.recv().await {
//!                 Ok(item) => println!("[Async Receiver] Received: {}", item),
//!                 Err(e) => {
//!                     eprintln!("[Async Receiver] Recv error: {:?}", e);
//!                     break;
//!                 }
//!             }
//!         }
//!     });
//!
//!     producer_task.await.unwrap();
//!     consumer_task.await.unwrap();
//! }
//!
//! // To run the async example:
//! // tokio::runtime::Runtime::new().unwrap().block_on(async_spsc_example());
//! ```

// Private modules that form the SPSC implementation.
// `bounded_sync` contains the SpscShared core and synchronous P/C.
// `bounded_async` contains the asynchronous P/C wrappers and futures.
mod bounded_async;
mod bounded_sync;

// Publicly re-export the primary channel constructors and types.
pub use bounded_sync::{
  bounded_sync,        // fn to create sync SPSC channel
  BoundedSyncReceiver, // Sync consumer type
  BoundedSyncSender, // Sync producer type
                       // SpscShared is intentionally not re-exported here as it's an internal detail
                       // primarily for sharing between bounded_sync and bounded_async.
};

pub use bounded_async::{
  bounded_async,            // fn to create async SPSC channel
  AsyncBoundedSpscReceiver, // Async consumer type
  AsyncBoundedSpscSender, // Async producer type
  ReceiveFuture,            // Future returned by async recv
  SendFuture,               // Future returned by async send
};

// Re-export common error types used by SPSC channels.
pub use crate::error::{RecvError, RecvErrorTimeout, SendError, TryRecvError, TrySendError};
