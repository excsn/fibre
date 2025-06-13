use crate::entry::CacheEntry;
use crate::loader::{LoadFuture, Loader};
use crate::metrics::Metrics;
use crate::policy::{AccessInfo, AdmissionDecision, CachePolicy};
use crate::store::ShardedStore;
use crate::task::janitor::Janitor;
use crate::task::notifier::{Notification, Notifier};
use crate::TaskSpawner;

use std::hash::{BuildHasher, Hash};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;
use std::{fmt, thread};

use ahash::HashMap;
use fibre::mpsc;
use parking_lot::Mutex;

/// The internal, thread-safe core of the cache.
///
/// This struct is wrapped in an `Arc` and shared between the cache handles
/// (`Cache`, `AsyncCache`) and any background tasks.
pub(crate) struct CacheShared<K: Send, V: Send + Sync, H> {
  // These are the primary owners of the state.
  pub(crate) store: Arc<ShardedStore<K, V, H>>,
  pub(crate) metrics: Arc<Metrics>,

  // Components for managing the cache logic
  pub(crate) eviction_policy: Arc<dyn CachePolicy<K, V>>,

  pub(crate) janitor: Option<Janitor>,

  pub(crate) notification_sender: Option<mpsc::BoundedSender<Notification<K, V>>>,
  pub(crate) notifier: Option<Notifier<K, V>>,

  // Configuration from the builder.
  pub(crate) capacity: u64,
  pub(crate) time_to_live: Option<Duration>,
  pub(crate) time_to_idle: Option<Duration>,
  pub(crate) stale_while_revalidate: Option<Duration>,
  pub(crate) loader: Option<Loader<K, V>>,
  pub(crate) spawner: Option<Arc<dyn TaskSpawner>>,
  pub(crate) pending_loads: Mutex<HashMap<K, Arc<LoadFuture<V>>>>,
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
  /// A "raw" get that performs a lookup without updating any stats or timers.
  /// Used for internal double-checks to avoid metric side-effects.
  pub(crate) fn raw_get(&self, key: &K) -> Option<Arc<CacheEntry<V>>>
  where
    K: Eq + Hash,
    H: BuildHasher + Clone,
  {
    let shard = self.store.get_shard(key);
    let guard = shard.read();
    if let Some(entry) = guard.get(key) {
      if entry.is_expired(self.time_to_idle) {
        None
      } else {
        Some(entry.clone())
      }
    } else {
      None
    }
  }

  /// Centralized eviction logic.
  ///
  /// This can be called by the janitor or by a loader task when space is needed.
  pub(crate) fn run_eviction(&self, cost_to_free: u64)
  where
    K: Eq + Hash + Clone + Sync,
    V: Send + Sync,
    H: BuildHasher + Clone + Sync,
  {
    let (victims, total_cost_released) = self.eviction_policy.evict(cost_to_free);
    if victims.is_empty() {
      return;
    }

    for key in victims {
      let shard = self.store.get_shard(&key);
      let mut guard = shard.write_sync();
      if let Some(removed_entry) = guard.remove(&key) {
        self
          .metrics
          .evicted_by_capacity
          .fetch_add(1, Ordering::Relaxed);
        self.eviction_policy.on_remove(&key);

        if let Some(sender) = &self.notification_sender {
          let _ = sender.try_send((
            key.clone(),
            removed_entry.value(),
            crate::EvictionReason::Capacity,
          ));
        }
      }
    }

    if total_cost_released > 0 {
      self
        .metrics
        .current_cost
        .fetch_sub(total_cost_released, Ordering::Relaxed);
    }
  }

  pub(crate) fn process_evicted_victims(&self, victim_keys: Vec<K>)
  where
    K: Eq + Hash + Clone + Sync, // Ensure K is Clone for notifications
    V: Send + Sync,
    H: BuildHasher + Clone + Sync,
  {
    if victim_keys.is_empty() {
      return;
    }

    let mut total_cost_released_by_victims = 0;
    let mut actual_victims_removed_count = 0;
    let mut notifications_to_send = Vec::new();

    for victim_key in victim_keys {
      let victim_shard = self.store.get_shard(&victim_key);
      // IMPORTANT: Need to decide on sync/async lock here.
      // If called from sync path (VacantEntry), use write_sync.
      // If called from async path (loader), use write_async.
      // This suggests process_evicted_victims might need to be generic or have variants.
      // For now, let's assume a sync context for VacantEntry's call.
      let mut victim_shard_guard = victim_shard.write_sync();

      if let Some(removed_entry) = victim_shard_guard.remove(&victim_key) {
        total_cost_released_by_victims += removed_entry.cost();
        actual_victims_removed_count += 1;

        self.eviction_policy.on_remove(&victim_key); // Notify policy

        if let Some(sender) = &self.notification_sender {
          notifications_to_send.push((
            victim_key.clone(),
            removed_entry.value(),
            crate::EvictionReason::Capacity, // Or a new "PolicyDecision"
          ));
        }
      }
    }

    if actual_victims_removed_count > 0 {
      self
        .metrics
        .current_cost
        .fetch_sub(total_cost_released_by_victims, Ordering::Relaxed);
      // Consider a different metric than evicted_by_capacity if these are policy-driven admit-time evictions
      self
        .metrics
        .evicted_by_capacity
        .fetch_add(actual_victims_removed_count, Ordering::Relaxed);
    }

    if let Some(sender) = &self.notification_sender {
      for notification in notifications_to_send {
        let _ = sender.try_send(notification);
      }
    }
  }

  pub(crate) async fn process_evicted_victims_async(&self, victim_keys: Vec<K>)
  where
    K: Eq + Hash + Clone + Send + Sync, // K needs to be Send + Sync for async
    V: Send + Sync,
    H: BuildHasher + Clone + Send + Sync, // H needs Send + Sync
  {
    if victim_keys.is_empty() {
      return;
    }

    let mut total_cost_released_by_victims = 0;
    let mut actual_victims_removed_count = 0;
    // For async, sending notifications might also need care if the channel send can block.
    // fibre::mpsc::Sender::try_send is non-blocking.
    // If we used an async channel, we might await sends.
    let mut notifications_to_send = Vec::new();

    for victim_key in victim_keys {
      let victim_shard = self.store.get_shard(&victim_key);
      let mut victim_shard_guard = victim_shard.write_async().await; // ASYNC LOCK

      if let Some(removed_entry) = victim_shard_guard.remove(&victim_key) {
        total_cost_released_by_victims += removed_entry.cost();
        actual_victims_removed_count += 1;

        self.eviction_policy.on_remove(&victim_key);

        if let Some(sender) = &self.notification_sender {
          notifications_to_send.push((
            victim_key.clone(),
            removed_entry.value(),
            crate::EvictionReason::Capacity, // Or PolicyDecision
          ));
        }
      }
      // victim_shard_guard (async) dropped here
    }

    if actual_victims_removed_count > 0 {
      self
        .metrics
        .current_cost
        .fetch_sub(total_cost_released_by_victims, Ordering::Relaxed);
      self
        .metrics
        .evicted_by_capacity
        .fetch_add(actual_victims_removed_count, Ordering::Relaxed);
    }

    if let Some(sender) = &self.notification_sender {
      for notification in notifications_to_send {
        // try_send is fine from async context as it's non-blocking
        let _ = sender.try_send(notification);
      }
    }
  }

  pub(crate) fn spawn_loader_task(shared: Arc<Self>, key: K, future: Arc<LoadFuture<V>>)
  where
    K: Eq + Hash + Clone + Send + Sync + 'static,
    V: Send + Sync + 'static,
    H: BuildHasher + Clone + Send + Sync + 'static,
  {
    let loader = match &shared.loader {
      Some(l) => l.clone(),
      None => {
        // If no loader, complete future with some kind of "not found" or panic.
        // For now, assume loader always exists if this is called.
        // Or, the `get_with` path should check for loader existence first.
        // To prevent panic, let's just return if no loader.
        // The LoadFuture would eventually time out or be dropped if not completed.
        // A better approach is for LoadFuture to also support an Error state.
        return;
      }
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
          let access_info = AccessInfo {
            key: &key,
            entry: &new_cache_entry,
          };

          let admission_decision = shared.eviction_policy.on_admit(&access_info);
          let mut victims_to_process: Option<Vec<K>> = None;

          // Lock the specific shard for the new item
          let new_item_shard = shared.store.get_shard(&key);
          let mut new_item_shard_guard = new_item_shard.write_sync();

          match admission_decision {
            AdmissionDecision::Admit => {
              new_item_shard_guard.insert(key.clone(), new_cache_entry);
              shared.metrics.inserts.fetch_add(1, Ordering::Relaxed);
              shared
                .metrics
                .current_cost
                .fetch_add(cost, Ordering::Relaxed);
              shared.metrics.keys_admitted.fetch_add(1, Ordering::Relaxed);
            }
            AdmissionDecision::Reject => {
              shared.metrics.keys_rejected.fetch_add(1, Ordering::Relaxed);
              // value_arc_to_return will be used to complete the future, then dropped by waiters if not used.
            }
            AdmissionDecision::AdmitAndEvict(victim_keys) => {
              new_item_shard_guard.insert(key.clone(), new_cache_entry);
              shared.metrics.inserts.fetch_add(1, Ordering::Relaxed);
              shared
                .metrics
                .current_cost
                .fetch_add(cost, Ordering::Relaxed);
              shared.metrics.keys_admitted.fetch_add(1, Ordering::Relaxed);
              victims_to_process = Some(victim_keys);
            }
          }
          drop(new_item_shard_guard); // Release lock before processing victims

          if let Some(victims) = victims_to_process {
            shared.process_evicted_victims(victims);
          }

          shared.pending_loads.lock().remove(&key);
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
            let access_info = AccessInfo {
              key: &key,
              entry: &new_cache_entry,
            };

            let admission_decision = shared.eviction_policy.on_admit(&access_info);
            let mut victims_to_process: Option<Vec<K>> = None;

            let new_item_shard = shared.store.get_shard(&key);
            {
              let mut new_item_shard_guard = new_item_shard.write_async().await;

              match admission_decision {
                AdmissionDecision::Admit => {
                  new_item_shard_guard.insert(key.clone(), new_cache_entry);
                  shared.metrics.inserts.fetch_add(1, Ordering::Relaxed);
                  shared
                    .metrics
                    .current_cost
                    .fetch_add(cost, Ordering::Relaxed);
                  shared.metrics.keys_admitted.fetch_add(1, Ordering::Relaxed);
                }
                AdmissionDecision::Reject => {
                  shared.metrics.keys_rejected.fetch_add(1, Ordering::Relaxed);
                }
                AdmissionDecision::AdmitAndEvict(victim_keys) => {
                  new_item_shard_guard.insert(key.clone(), new_cache_entry);
                  shared.metrics.inserts.fetch_add(1, Ordering::Relaxed);
                  shared
                    .metrics
                    .current_cost
                    .fetch_add(cost, Ordering::Relaxed);
                  shared.metrics.keys_admitted.fetch_add(1, Ordering::Relaxed);
                  victims_to_process = Some(victim_keys);
                }
              }
            }

            if let Some(victims) = victims_to_process {
              shared.process_evicted_victims_async(victims).await; // AWAIT THE ASYNC CALL
            }

            shared.pending_loads.lock().remove(&key);
            future.complete(value_arc_to_return);
          }
        };
        spawner.spawn(Box::pin(task));
      }
    }
  }
}
