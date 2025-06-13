//! A high-performance, concurrent, sync/async cache designed for flexibility
//! and support for non-cloneable values.
//!
//! # Features
//! - **High Concurrency**: Built with a sharded architecture to minimize lock contention.
//! - **Sync & Async**: Provides both blocking synchronous and non-blocking `async` APIs.
//! - **Non-Clone Support**: Stores values in an `Arc<V>`, avoiding `V: Clone` bounds.
//! - **Rich Policies**: Time-to-Live (TTL), Time-to-Idle (TTI), and advanced
//!   eviction strategies like TinyLFU.
//! - **Observability**: Exposes detailed metrics for monitoring cache performance.
//! - **Persistence**: Optional `serde` feature for saving and loading cache state.

// Public modules that form the API
pub mod builder;
pub mod entry_api;
pub mod entry_api_async;
pub mod error;
pub mod handles;
pub mod listener;
pub mod metrics;
pub mod runtime;
pub mod policy;

// Internal, crate-only modules
// mod backpressure;
mod entry;
mod loader;
mod shared;
mod store;
mod sync;
mod task;
mod time;

#[cfg(feature = "serde")]
pub mod snapshot;

// Re-export the primary user-facing types for convenience
pub use builder::CacheBuilder;
pub use entry_api::Entry;
pub use entry_api_async::AsyncEntry;
pub use handles::{AsyncCache, Cache};
pub use listener::{EvictionListener, EvictionReason};
pub use metrics::MetricsSnapshot;
pub use runtime::TaskSpawner;