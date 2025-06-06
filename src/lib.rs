#![cfg_attr(not(feature = "std"), no_std)] // Aim for no_std compatibility if possible
#![warn(missing_docs, missing_debug_implementations, rust_2018_idioms)]
// #![deny(unsafe_code)] // We will use unsafe, so deny won't work. Add specific allow/deny later.

//! High-performance, memory-efficient sync/async channels for Rust.
//!
//! Fibre provides a suite of channel types optimized for various concurrency patterns,
//! including SPSC, MPMC, MPSC, SPMC, and Oneshot. It aims for peak performance
//! while offering both synchronous and asynchronous APIs.

// Conditional compilation for alloc/std features
#[cfg(all(not(feature = "std"), feature = "alloc"))]
extern crate alloc;
#[cfg(feature = "alloc")]
use alloc::boxed::Box; // Example usage

// Core modules that will be fleshed out
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
pub use error::{RecvError, SendError, TryRecvError, TrySendError};

// Helper function to check if a type is Send + Sync.
// Useful for static assertions in generic code.
#[allow(dead_code)]
fn assert_send_sync<T: Send + Sync>() {}