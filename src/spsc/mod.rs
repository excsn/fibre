// src/spsc/mod.rs

//! Single-Producer, Single-Consumer (SPSC) channels.
//!
//! These channels are optimized for the case where there is only one sender (producer)
//! and only one receiver (consumer). They offer the highest throughput and lowest
//! latency for this specific 1-to-1 communication pattern.
//!
//! # Examples
//!
//! ```
//! // Create a bounded SPSC channel with a capacity of 10.
//! let (mut producer, mut consumer) = fibre::spsc::bounded_sync(10);
//!
//! std::thread::spawn(move || {
//!     for i in 0..20 {
//!         if let Err(e) = producer.send(format!("Item {}", i)) {
//!             eprintln!("SPSC send error: {:?}", e);
//!             break;
//!         }
//!         // Simple backoff if send blocks due to full queue (in a real scenario)
//!         // For this example, send might block if consumer is slow.
//!         // std::thread::sleep(std::time::Duration::from_millis(10));
//!     }
//! });
//!
//! for _ in 0..20 {
//!     match consumer.recv() {
//!         Ok(item) => println!("SPSC received: {}", item),
//!         Err(e) => {
//!             eprintln!("SPSC recv error: {:?}", e);
//!             break;
//!         }
//!     }
//! }
//! ```

mod bounded_sync;
mod bounded_async;

pub use bounded_sync::{
  bounded_sync,
  BoundedSyncConsumer,
  BoundedSyncProducer,
};

pub use bounded_async::{
    bounded_async, AsyncBoundedSpscProducer, AsyncBoundedSpscConsumer,
    SendFuture, ReceiveFuture
};

pub use crate::error::{TrySendError, SendError, TryRecvError, RecvError, RecvErrorTimeout};