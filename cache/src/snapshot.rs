// This entire module is only compiled when the 'serde' feature is enabled.
#![cfg(feature = "serde")]

use crate::error::BuildError;
use crate::time;
use crate::CacheBuilder;

use futures_util::future;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::hash::BuildHasher;
use std::hash::Hash;
use std::time::Duration;

use crate::AsyncCache;
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

#[cfg(test)]
impl<K, V> CacheSnapshot<K, V> {
  /// Exposes the internal entries slice for testing purposes only.
  pub fn test_get_entries(&self) -> &[PersistentEntry<K, V>] {
    &self.entries
  }
}

// --- Snapshot-based Build Method ---
// This block is gated by `serde` and has the `DeserializeOwned` bound.
impl<K, V, H> CacheBuilder<K, V, H>
where
  K: Eq + Hash + Clone + Send + Sync + DeserializeOwned + 'static,
  V: Clone + Send + Sync + DeserializeOwned + 'static,
  H: BuildHasher + Clone + Send + Sync + 'static,
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
    let shared = self.build_shared_core(Some(snapshot))?;
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
    let shared = self.build_shared_core(Some(snapshot))?;
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
      .map(|s| s.map.write())
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
      future::join_all(self.shared.store.iter_shards().map(|s| s.map.write_async())).await;

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

#[cfg(test)]
mod tests {
  #![cfg(feature = "serde")]

  use crate::{builder::CacheBuilder, snapshot::CacheSnapshot};
  use serde::{Deserialize, Serialize};
  use std::time::Duration;

  #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
  struct TestValue {
    id: u32,
    data: String,
  }

  #[test]
  fn test_sync_snapshot_and_restore() {
    // 1. Create and populate the original cache
    let original_cache = CacheBuilder::default()
      .capacity(100)
      .time_to_live(Duration::from_secs(5))
      .janitor_tick_interval(Duration::from_secs(60))
      .build()
      .unwrap();

    original_cache.insert(
      1,
      TestValue {
        id: 1,
        data: "one".to_string(),
      },
      1,
    );
    original_cache.insert(
      2,
      TestValue {
        id: 2,
        data: "two".to_string(),
      },
      1,
    );

    std::thread::sleep(Duration::from_secs(1)); // Sleep for 1 sec

    // 2. Create and serialize the snapshot (remaining TTL is ~4s)
    let snapshot = original_cache.to_snapshot();
    assert_eq!(snapshot.test_get_entries().len(), 2);
    let serialized = bincode::serialize(&snapshot).expect("Serialization failed");

    // 3. Deserialize and build a new cache from the snapshot
    let deserialized: CacheSnapshot<u32, TestValue> =
      bincode::deserialize(&serialized).expect("Deserialization failed");

    let new_cache = CacheBuilder::default()
      .build_from_snapshot(deserialized)
      .unwrap();

    // 4. Verify the new cache's state
    assert_eq!(new_cache.metrics().current_cost, 2);
    assert!(new_cache.fetch(&1).is_some());

    // Now sleep for the remaining time + margin
    std::thread::sleep(Duration::from_secs(5));
    assert!(
      new_cache.fetch(&1).is_none(),
      "Item should have expired based on its original insertion time"
    );
  }

  #[tokio::test]
  async fn test_async_snapshot_and_restore() {
    // 1. Create and populate the original cache
    let original_cache = CacheBuilder::default()
      .capacity(100)
      .time_to_live(Duration::from_secs(10))
      .build_async()
      .unwrap();

    original_cache
      .insert(
        1,
        TestValue {
          id: 1,
          data: "one".to_string(),
        },
        1,
      )
      .await;

    tokio::time::sleep(Duration::from_secs(2)).await;

    // 2. Create and serialize the snapshot
    let snapshot = original_cache.to_snapshot().await;
    let serialized = bincode::serialize(&snapshot).expect("Serialization failed");

    // 3. Deserialize and build a new cache
    let deserialized: CacheSnapshot<u32, TestValue> =
      bincode::deserialize(&serialized).expect("Deserialization failed");

    let new_cache = CacheBuilder::default()
      .build_from_snapshot_async(deserialized)
      .unwrap();

    // 4. Verify the new cache's state
    assert_eq!(new_cache.metrics().current_cost, 1);
    assert_eq!(new_cache.fetch(&1).await.unwrap().id, 1);
  }

  #[test]
  fn test_snapshot_excludes_expired_items() {
    let cache = CacheBuilder::default()
      .time_to_live(Duration::from_millis(100))
      .build()
      .unwrap();

    cache.insert(1, 1, 1);
    std::thread::sleep(Duration::from_millis(200));

    let snapshot = cache.to_snapshot();
    assert!(
      snapshot.test_get_entries().is_empty(),
      "Snapshot should not include expired items"
    );
  }
}
