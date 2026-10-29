use crate::entry::CacheEntry;
use crate::loader::{LoadFuture, Loader};
use crate::metrics::Metrics;
use crate::store::ShardedStore;
use crate::sync::HybridMutex;
use crate::task::janitor::Janitor;
use crate::task::notifier::{Notification, Notifier};
use crate::TaskSpawner;

use std::hash::{BuildHasher, Hash};
use std::sync::Arc;
use std::time::Duration;
use std::{fmt, thread};

use ahash::HashMap;
use fibre::mpsc;


/// The internal, thread-safe core of the cache.
pub(crate) struct CacheShared<K: Send, V: Send + Sync, H> {
  pub(crate) store: Arc<ShardedStore<K, V, H>>,
  pub(crate) clock: Arc<crate::time::CoarseClock>,
  pub(crate) metrics: Arc<Metrics>,
  pub(crate) janitor: Option<Janitor>,
  pub(crate) notification_sender: Option<mpsc::BoundedSyncSender<Notification<K, V>>>,
  pub(crate) notifier: Option<Notifier<K, V>>,
  pub(crate) capacity: u64,
  pub(crate) time_to_live: Option<Duration>,
  pub(crate) time_to_idle: Option<Duration>,
  pub(crate) stale_while_revalidate: Option<Duration>,
  pub(crate) loader: Option<Loader<K, V>>,
  pub(crate) spawner: Option<Arc<dyn TaskSpawner>>,
  pub(crate) pending_loads: Box<[HybridMutex<HashMap<K, Arc<LoadFuture<V>>>>]>,
  pub(crate) maintenance_probability_denominator: u32,
  pub(crate) maintenance_on_introspection: bool,
  // False when no shard policy uses access events, letting reads skip the
  // access batcher entirely.
  pub(crate) track_reads: bool,
  pub(crate) builtin_reads: BuiltinReads,
  // Non-blocking maintenance requests from async paths to the janitor
  // (shard index). `Some` whenever the janitor is running. Lossy by design:
  // a full buffer means the janitor is already busy.
  pub(crate) maintenance_signal: Option<std::sync::mpsc::SyncSender<usize>>,
}

impl<K: Send, V: Send + Sync, H> fmt::Debug for CacheShared<K, V, H> {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("CacheShared")
      .field("capacity", &self.capacity)
      .field("time_to_live", &self.time_to_live)
      .field("time_to_idle", &self.time_to_idle)
      .field("metrics", &self.metrics.snapshot())
      .finish_non_exhaustive()
  }
}

impl<K: Send, V: Send + Sync, H> Drop for CacheShared<K, V, H> {
  fn drop(&mut self) {
    if let Some(janitor) = self.janitor.take() {
      janitor.stop();
    }
    if let Some(notifier) = self.notifier.take() {
      notifier.stop();
    }
  }
}

impl<K, V, H> CacheShared<K, V, H>
where
  K: Eq + Hash + Clone + Send,
  V: Send + Sync,
  H: BuildHasher + Clone,
{
  /// Flushes pending read-access events for all shards.
  /// Called by introspection methods when `maintenance_on_introspection` is enabled.
  pub(crate) fn flush_for_introspection(&self) {
    if !self.maintenance_on_introspection {
      return;
    }
    for shard in self.store.shards.iter() {
      let _guard = shard.maintenance_lock.lock();
      crate::task::janitor::perform_shard_maintenance(shard);
    }
  }
}

impl<K: Send, V: Send + Sync, H> CacheShared<K, V, H> {

  #[inline]
  pub fn get_shard_index_from_hash(&self, hash: u64) -> usize {
    hash as usize & (self.store.shards.len() - 1)
  }

  pub(crate) fn spawn_loader_task(shared: Arc<Self>, key: K, future: Arc<LoadFuture<V>>)
  where
    K: Eq + Hash + Clone + Send + Sync + 'static,
    V: Send + Sync + 'static,
    H: BuildHasher + Clone + Send + Sync + 'static,
  {
    let loader = match &shared.loader {
      Some(l) => l.clone(),
      None => return,
    };

    match loader {
      Loader::Sync(sync_loader) => {
        thread::spawn(move || {
          let (value, cost) = sync_loader(key.clone());
          let new_cache_entry = CacheEntry::new(
            value,
            cost,
            shared.time_to_live,
            shared.time_to_idle,
          );
          let value_arc_to_return = new_cache_entry.value();

          let shard = shared.store.get_shard(&key);
          {
            let mut guard = shard.map.write();
            guard.insert(key.clone(), new_cache_entry);

          }



          let hash = crate::store::hash_key(&shared.store.hasher, &key);
          let index = hash as usize & (shared.pending_loads.len() - 1);
          shared.pending_loads[index].lock().remove(&key);

          future.complete(value_arc_to_return);
        });
      }
      Loader::Async(async_loader) => {
        let spawner = shared
          .spawner
          .as_ref()
          .expect("Spawner must exist for async loader");
        let task = {
          let shared = shared.clone();
          async move {
            let (value, cost) = async_loader(key.clone()).await;
            let new_cache_entry = CacheEntry::new(
              value,
              cost,
              shared.time_to_live,
              shared.time_to_idle,
            );
            let value_arc_to_return = new_cache_entry.value();

            let shard = shared.store.get_shard(&key);
            {
              let mut guard = shard.map.write_async().await;
              guard.insert(key.clone(), new_cache_entry);

            }



            {
              let hash = crate::store::hash_key(&shared.store.hasher, &key);
              let index = hash as usize & (shared.pending_loads.len() - 1);
              shared.pending_loads[index].lock_async().await.remove(&key);
            }
            future.complete(value_arc_to_return);
          }
        };
        spawner.spawn(Box::pin(task));
      }
    }
  }
}

/// How a hit reaches the shard's built-in policy, if there is one.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum BuiltinReads {
  None,
  Ring,
  Freq,
}
