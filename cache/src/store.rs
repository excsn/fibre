use crate::entry::CacheEntry;
use crate::sync::HybridRwLock;

use core::fmt;
use std::collections::HashMap;
use std::hash::{BuildHasher, Hash, Hasher};
use std::sync::Arc;

use crossbeam_utils::CachePadded;

/// A helper function to hash a key using a `BuildHasher`.
#[inline]
fn hash_key<K: Hash, H: BuildHasher>(hasher: &H, key: &K) -> u64 {
  let mut state = hasher.build_hasher();
  key.hash(&mut state);
  state.finish()
}

/// A cache store that is partitioned into multiple, independently locked shards.
///
/// This design allows for high concurrency by ensuring that operations on
/// different keys are unlikely to contend for the same lock.
pub(crate) struct ShardedStore<K, V, H> {
  shards: Box<[CachePadded<HybridRwLock<HashMap<K, Arc<CacheEntry<V>>, H>>>]>,
  hasher: H,
}

impl<K, V, H> fmt::Debug for ShardedStore<K, V, H> {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("ShardedStore")
      .field("num_shards", &self.shards.len())
      .finish()
  }
}

impl<K, V, H> ShardedStore<K, V, H>
where
  K: Eq + Hash,
  H: BuildHasher + Clone,
{
  /// Creates a new `ShardedStore` with the specified number of shards and hasher.
  pub(crate) fn new(num_shards: usize, hasher: H) -> Self {
    let mut shards = Vec::with_capacity(num_shards);
    for _ in 0..num_shards {
      
      let shard_map = HashMap::with_hasher(hasher.clone());
      let lock = HybridRwLock::new(shard_map);
      
      shards.push(CachePadded::new(lock));
    }

    Self {
      shards: shards.into_boxed_slice(),
      hasher,
    }
  }

  /// Returns a reference to the `RwLock` guarding the shard for a given key.
  ///
  /// The caller can then acquire a read or write lock on this shard.
  #[inline]
  pub(crate) fn get_shard(&self, key: &K) -> &HybridRwLock<HashMap<K, Arc<CacheEntry<V>>, H>> {
    // Calculate the hash of the key.
    let hash = hash_key(&self.hasher, key);
    // Determine the shard index using the modulo operator.
    // This is safe because we validate that num_shards > 0 in the builder.
    let index = hash as usize % self.shards.len();

    // The `CachePadded` wrapper implements `Deref`, so we can directly
    // return a reference to the inner `RwLock`.
    &self.shards[index]
  }

  /// Returns an iterator over all the shard locks.
  /// This is useful for "stop-the-world" operations like `save()` or `clear()`.
  pub(crate) fn iter_shards(
    &self,
  ) -> impl Iterator<Item = &HybridRwLock<HashMap<K, Arc<CacheEntry<V>>, H>>> {
    self.shards.iter().map(|padded_lock| &**padded_lock)
  }
}
