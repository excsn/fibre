//! High-performance, memory-efficient sync/async channels for Rust.
//!
//! Fibre provides a suite of channel types optimized for various concurrency patterns,
//! including SPSC, MPMC, MPSC, SPMC, and Oneshot. It aims for peak performance
//! while offering both synchronous and asynchronous APIs.

pub mod error;

// Channel type modules
pub mod oneshot;
pub mod spsc;
pub mod mpsc;
pub mod spmc;
pub mod mpmc_v2;
pub use mpmc_v2 as mpmc;
pub mod mpmc_exp;
pub mod telemetry;

// Internal utilities
mod internal;
mod sync_util;
mod async_util;

pub use error::{
  BatchSendErrorReason, CloseError, RecvError, RecvErrorTimeout, SendBatchError, SendError,
  TryRecvError, TrySendBatchError, TrySendError,
};