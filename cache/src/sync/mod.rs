//! Locking primitives from the `fibre` (channels) crate. Shards use the
//! reader-biased `fibre::sync::v2::HybridRwLock`.

pub use fibre::sync::HybridMutex;
pub use fibre::sync::v2::{HybridRwLock, WriteGuard};
