// src/lib.rs

//! High-performance, memory-efficient sync/async channels for Rust.
//!
//! Fibre provides a suite of channel types optimized for various concurrency patterns,
//! including SPSC, MPMC, MPSC, SPMC, and Oneshot. It aims for peak performance
//! while offering both synchronous and asynchronous APIs.

// Add the new cache module, gated by its feature flag.
#[cfg(feature = "cache")]
pub mod cache;

pub mod coord;
pub mod error;

// Channel type modules
pub mod oneshot;
pub mod spsc;
pub mod mpsc;
pub mod spmc;
pub mod mpmc;
pub mod telemetry;

// Internal utilities - not part of public API but exposed for crate use
mod internal;
mod sync_util;
mod async_util;

// Public re-exports for convenience (will grow)
pub use error::{CloseError, RecvError, RecvErrorTimeout, SendError, TryRecvError, TrySendError};