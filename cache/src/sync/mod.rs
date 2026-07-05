//! Hybrid sync/async locking primitives. These now live in the `fibre`
//! (channels) crate and are re-exported here so `fibre_cache` and `fibre` share
//! one implementation. See `fibre::sync`.

pub use fibre::sync::{HybridMutex, HybridRwLock, MutexGuard, ReadGuard, WriteGuard};
