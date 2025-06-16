use crate::entry::CacheEntry;
use crate::loader::{LoadFuture, Loader};
use crate::metrics::Metrics;
use crate::policy::{AccessEvent, CachePolicy};
use crate::store::ShardedStore;
use crate::sync::HybridMutex;
use crate::task::janitor::Janitor;
use crate::task::notifier::{Notification, Notifier};
use crate::TaskSpawner;

use std::hash::{BuildHasher, Hash};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;
use std::{fmt, thread};

use ahash::HashMap;
use equivalent::Equivalent;
use fibre::mpsc;

/// The internal, thread-safe core of the cache.
pub(crate) struct CacheShared<K: Send, V: Send + Sync, H> {
  pub(crate) store: Arc<ShardedStore<K, V, H>>,
  pub(crate) metrics: Arc<Metrics>,
  pub(crate) cache_policy: Box<[Arc<dyn CachePolicy<K, V>>]>,
  pub(crate) janitor: Option<Janitor>,
  pub(crate) notification_sender: Option<mpsc::BoundedSender<Notification<K, V>>>,
  pub(crate) notifier: Option<Notifier<K, V>>,
  pub(crate) capacity: u64,
  pub(crate) time_to_live: Option<Duration>,
  pub(crate) time_to_idle: Option<Duration>,
  pub(crate) stale_while_revalidate: Option<Duration>,
  pub(crate) loader: Option<Loader<K, V>>,
  pub(crate) spawner: Option<Arc<dyn TaskSpawner>>,
  pub(crate) pending_loads: Box<[HybridMutex<HashMap<K, Arc<LoadFuture<V>>>>]>,
  pub(crate) maintenance_probability_denominator: u32,
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

impl<K: Send, V: Send + Sync, H> CacheShared<K, V, H> {

  pub fn get_shard_index<Q>(&self, key: &Q) -> usize
  where
    K: Hash,
    Q: Hash + Equivalent<K> + ?Sized,
    H: BuildHasher,
  {
    return self.store.get_shard_index(key);
  }

  pub fn get_shard_index_from_hash(&self, hash: u64) -> usize
  where
    K: Hash,
    H: BuildHasher,
  {
    return hash as usize & (self.store.shards.len() - 1);
  }

  pub fn get_cache_policy<Q>(&self, key: &Q) -> &Arc<(dyn CachePolicy<K, V> + 'static)>
  where
    K: Hash,
    Q: Hash + Equivalent<K> + ?Sized,
    H: BuildHasher,
  {
    let shard_index = self.get_shard_index(key);
    return &self.cache_policy[shard_index];
  }

  pub fn get_cache_policy_shard_idx(&self, shard_idx: usize) -> &Arc<(dyn CachePolicy<K, V> + 'static)>
  where
    K: Hash,
    H: BuildHasher,
  {
    return &self.cache_policy[shard_idx];
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
          let new_cache_entry = Arc::new(CacheEntry::new(
            value,
            cost,
            shared.time_to_live,
            shared.time_to_idle,
          ));
          let value_arc_to_return = new_cache_entry.value();

          let shard = shared.store.get_shard(&key);
          {
            let mut guard = shard.map.write();
            let old_entry = guard.insert(key.clone(), new_cache_entry);

            // Update cost metrics immediately
            let old_cost = old_entry.map_or(0, |e| e.cost());
            shared
              .metrics
              .current_cost
              .fetch_add(cost, Ordering::Relaxed);
            shared
              .metrics
              .current_cost
              .fetch_sub(old_cost, Ordering::Relaxed);
          }

          // Record the write event in the buffer for the janitor to process later
          let _ = shard
            .event_buffer_tx
            .try_send(AccessEvent::Write(key.clone(), cost));

          shared.metrics.inserts.fetch_add(1, Ordering::Relaxed);
          shared.metrics.keys_admitted.fetch_add(1, Ordering::Relaxed);
          shared
            .metrics
            .total_cost_added
            .fetch_add(cost, Ordering::Relaxed);

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
            let new_cache_entry = Arc::new(CacheEntry::new(
              value,
              cost,
              shared.time_to_live,
              shared.time_to_idle,
            ));
            let value_arc_to_return = new_cache_entry.value();

            let shard = shared.store.get_shard(&key);
            {
              let mut guard = shard.map.write_async().await;
              let old_entry = guard.insert(key.clone(), new_cache_entry);

              // Update cost metrics immediately
              let old_cost = old_entry.map_or(0, |e| e.cost());
              shared
                .metrics
                .current_cost
                .fetch_add(cost, Ordering::Relaxed);
              shared
                .metrics
                .current_cost
                .fetch_sub(old_cost, Ordering::Relaxed);
            }

            // Record the write event
            let _ = shard
              .event_buffer_tx
              .try_send(AccessEvent::Write(key.clone(), cost));

            shared.metrics.inserts.fetch_add(1, Ordering::Relaxed);
            shared.metrics.keys_admitted.fetch_add(1, Ordering::Relaxed);
            shared
              .metrics
              .total_cost_added
              .fetch_add(cost, Ordering::Relaxed);

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
