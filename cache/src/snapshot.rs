// This entire module is only compiled when the 'serde' feature is enabled.
#![cfg(feature = "serde")]
#[cfg(feature = "serde")]
use crate::error::BuildError;
use crate::time;
#[cfg(feature = "serde")]
use crate::CacheBuilder;

use futures_util::future;
#[cfg(feature = "serde")]
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
#[cfg(feature = "serde")]
use std::hash::BuildHasher;
#[cfg(feature = "serde")]
use std::hash::Hash;
use std::time::Duration;

use crate::AsyncCache;
#[cfg(feature = "serde")]
use crate::Cache;

/// An internal, serializable representation of a single cache entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PersistentEntry<K, V> {
  pub(crate) key: K,
  pub(crate) value: V,
  pub(crate) cost: u64,
  // Remaining time to live.
  pub(crate) ttl_remaining: Option<Duration>,
}

/// A serializable, point-in-time snapshot of the cache's data.
///
/// This struct can be created with [`Cache::to_snapshot()`] and used to
/// restore a cache with [`CacheBuilder::with_snapshot()`].
///
/// It implements `Serialize` and `Deserialize` (if `K` and `V` do), allowing
/// you to use any `serde`-compatible format (like JSON, Bincode, etc.) for
/// persistence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheSnapshot<K, V> {
  // We store the data in the internal persistent format.
  pub(crate) entries: Vec<PersistentEntry<K, V>>,
  // We can also store key metadata about the cache itself.
  capacity: u64,
  shards: usize,
}

impl<K, V> CacheSnapshot<K, V> {
  // This function is internal. The public API is on the Cache/Builder.
  pub(crate) fn new(entries: Vec<PersistentEntry<K, V>>, capacity: u64, shards: usize) -> Self {
    Self {
      entries,
      capacity,
      shards,
    }
  }
}

// --- Snapshot-based Build Method ---
// This block is gated by `serde` and has the `DeserializeOwned` bound.
#[cfg(feature = "serde")]
impl<K, V, H> CacheBuilder<K, V, H>
where
  K: Eq + Hash + Clone + Send + Sync + DeserializeOwned + 'static,
  V: Clone + Send + Sync + DeserializeOwned+ 'static,
  H: BuildHasher + Clone + Send + Sync+ 'static,
{
  /// Builds a new cache and pre-populates it with entries from a snapshot.
  ///
  /// The builder's `capacity` and `shards` settings are overridden by the
  /// values stored in the snapshot to ensure consistency.
  pub fn build_from_snapshot(
    mut self,
    snapshot: CacheSnapshot<K, V>,
  ) -> Result<Cache<K, V, H>, BuildError> {
    // Override settings from snapshot for consistency.
    self.capacity = snapshot.capacity;
    self.shards = snapshot.shards;

    self.validate()?;
    let shared = self.build_shared_core(Some(snapshot));
    Ok(Cache { shared })
  }


  /// Builds a new cache and pre-populates it with entries from a snapshot.
  ///
  /// The builder's `capacity` and `shards` settings are overridden by the
  /// values stored in the snapshot to ensure consistency.
  pub fn build_from_snapshot_async(
    mut self,
    snapshot: CacheSnapshot<K, V>,
  ) -> Result<AsyncCache<K, V, H>, BuildError> {
    // Override settings from snapshot for consistency.
    self.capacity = snapshot.capacity;
    self.shards = snapshot.shards;

    self.validate()?;
    let shared = self.build_shared_core(Some(snapshot));
    Ok(AsyncCache { shared })
  }
}

impl<K, V, H> Cache<K, V, H>
where
  K: Eq + Hash + Clone + Send + Sync + Serialize,
  V: Clone + Send + Sync + Serialize,
  H: BuildHasher + Clone,
{
  /// Creates a serializable snapshot of the cache's current state.
  ///
  /// This operation acquires a write lock on all cache shards, effectively
  /// pausing all other cache operations until the snapshot is complete.
  /// Expired items are not included in the snapshot.
  pub fn to_snapshot(&self) -> CacheSnapshot<K, V> {
    let shard_guards = self
      .shared
      .store
      .iter_shards()
      .map(|s| s.write_sync())
      .collect::<Vec<_>>();

    let mut persistent_entries = Vec::new();
    let now_duration = time::now_duration();

    for guard in shard_guards.iter() {
      for (key, entry) in guard.iter() {
        if entry.is_expired(self.shared.time_to_idle) {
          continue;
        }

        let expires_at_nanos = entry.expires_at.load(std::sync::atomic::Ordering::Relaxed);
        let ttl_remaining = if expires_at_nanos == 0 {
          None
        } else {
          let expires_duration = Duration::from_nanos(expires_at_nanos);
          expires_duration.checked_sub(now_duration)
        };

        persistent_entries.push(PersistentEntry {
          key: key.clone(),
          value: entry.value().as_ref().clone(),
          cost: entry.cost(),
          ttl_remaining,
        });
      }
    }

    CacheSnapshot::new(
      persistent_entries,
      self.shared.capacity,
      self.shared.store.iter_shards().count(),
    )
  }
}

impl<K, V, H> AsyncCache<K, V, H>
where
  K: Eq + Hash + Clone + Send + Sync + Serialize,
  V: Clone + Send + Sync + Serialize,
  H: BuildHasher + Clone,
{
  pub async fn to_snapshot(&self) -> CacheSnapshot<K, V> {
    // 1. Asynchronously acquire all write locks.
    let shard_guards =
      future::join_all(self.shared.store.iter_shards().map(|s| s.write_async())).await;

    let mut persistent_entries = Vec::new();
    let now_duration = time::now_duration();

    for guard in shard_guards.iter() {
      for (key, entry) in guard.iter() {
        if entry.is_expired(self.shared.time_to_idle) {
          continue;
        }

        let expires_at_nanos = entry.expires_at.load(std::sync::atomic::Ordering::Relaxed);
        let ttl_remaining = if expires_at_nanos == 0 {
          None
        } else {
          let expires_duration = Duration::from_nanos(expires_at_nanos);
          expires_duration.checked_sub(now_duration)
        };

        persistent_entries.push(PersistentEntry {
          key: key.clone(),
          value: entry.value().as_ref().clone(),
          cost: entry.cost(),
          ttl_remaining,
        });
      }
    }

    CacheSnapshot::new(
      persistent_entries,
      self.shared.capacity,
      self.shared.store.iter_shards().count(),
    )
  }
}
