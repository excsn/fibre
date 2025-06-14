use crate::entry::CacheEntry;
use crate::policy::AccessEvent;
use crate::sync::{HybridMutex, HybridRwLock};

use core::fmt;
use fibre::mpsc;
use std::collections::HashMap;
use std::hash::{BuildHasher, Hash, Hasher};
use std::sync::Arc;

use crossbeam_utils::CachePadded;

/// A helper function to hash a key using a `BuildHasher`.
#[inline]
pub(crate) fn hash_key<K: Hash, H: BuildHasher>(hasher: &H, key: &K) -> u64 {
  let mut state = hasher.build_hasher();
  key.hash(&mut state);
  state.finish()
}

pub type AccessEventSender<K> = fibre::mpsc::UnboundedSender<AccessEvent<K>>;
pub type AccessEventReceiver<K> = fibre::mpsc::UnboundedReceiver<AccessEvent<K>>;

/// A single, independently locked partition of the cache.
/// It contains both the data HashMap and a buffer for access events.
pub(crate) struct Shard<K: Send, V, H> {
  pub(crate) map: HybridRwLock<HashMap<K, Arc<CacheEntry<V>>, H>>,
  // A bounded MPSC channel to buffer access events for the eviction policy.
  pub(crate) event_buffer_tx: AccessEventSender<K>,
  pub(crate) event_buffer_rx: AccessEventReceiver<K>,
  // A lock to ensure only one thread performs maintenance at a time.
  pub(crate) maintenance_lock: HybridMutex<()>,
}

/// A cache store that is partitioned into multiple, independently locked shards.
///
/// This design allows for high concurrency by ensuring that operations on
/// different keys are unlikely to contend for the same lock.
pub(crate) struct ShardedStore<K: Send, V, H> {
  pub(crate) shards: Box<[CachePadded<Shard<K, V, H>>]>,
  pub(crate) hasher: H,
}

impl<K: Send, V, H> fmt::Debug for ShardedStore<K, V, H> {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("ShardedStore")
      .field("num_shards", &self.shards.len())
      .finish()
  }
}

impl<K, V, H> ShardedStore<K, V, H>
where
  K: Eq + Hash + Send,
  V: Send,
  H: BuildHasher + Clone,
{
  /// Creates a new `ShardedStore` with the specified number of shards and hasher.
  pub(crate) fn new(num_shards: usize, hasher: H) -> Self {

    let mut shards = Vec::with_capacity(num_shards);
    for _ in 0..num_shards {
      let shard_map = HashMap::with_hasher(hasher.clone());
      let lock = HybridRwLock::new(shard_map);
      let (tx, rx) = mpsc::unbounded();

      let shard = Shard {
        map: lock,
        event_buffer_tx: tx,
        event_buffer_rx: rx,
        maintenance_lock: HybridMutex::new(()),
      };

      shards.push(CachePadded::new(shard));
    }

    Self {
      shards: shards.into_boxed_slice(),
      hasher,
    }
  }

  /// Returns a reference to the `Shard` for a given key.
  #[inline]
  pub(crate) fn get_shard(&self, key: &K) -> &Shard<K, V, H> {
    let hash = hash_key(&self.hasher, key);
    let index = hash as usize & (self.shards.len() - 1); 
    &self.shards[index]
  }

  /// Returns an iterator over all the shards.
  pub(crate) fn iter_shards(&self) -> impl Iterator<Item = &Shard<K, V, H>> {
    self.shards.iter().map(|padded_shard| &**padded_shard)
  }
}
