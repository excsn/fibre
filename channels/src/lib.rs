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

/// Hybrid sync/async locking primitives (`HybridMutex`, `HybridRwLock`).
/// FIFO wait-list, spin-yield-then-park, barging.
pub mod sync;

// Internal utilities
mod internal;
mod sync_util;
mod async_util;

// Loom model checks for migrated channels - the single cfg(loom) gate for the
// whole test tree (see internal/sync.rs for the primitive switch, and
// channels/scripts/loom.sh to run).
#[cfg(all(test, loom))]
mod loom_tests;

pub use error::{
  BatchSendErrorReason, CloseError, RecvError, RecvErrorTimeout, SendBatchError, SendError,
  TryRecvError, TrySendBatchError, TrySendError,
};