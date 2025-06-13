use crate::metrics::Metrics;
use crate::policy::CachePolicy;
use crate::store::ShardedStore;
use crate::task::notifier::Notification;
use crate::EvictionReason;

use std::hash::{BuildHasher, Hash};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use fibre::mpsc;

/// The number of entries to sample from each shard on a cleanup tick.
const JANITOR_SAMPLE_SIZE: usize = 10;

/// A context object holding the thread-safe parts of the cache that the
/// janitor needs to access. This avoids a circular Arc reference.
pub(crate) struct JanitorContext<K: Send, V: Send + Sync, H> {
  pub(crate) store: Arc<ShardedStore<K, V, H>>,
  pub(crate) metrics: Arc<Metrics>,
  pub(crate) eviction_policy: Arc<dyn CachePolicy<K, V>>,
  pub(crate) capacity: u64,
  pub(crate) time_to_idle: Option<Duration>,
  pub(crate) notification_sender: Option<mpsc::BoundedSender<Notification<K, V>>>,
}

/// The background task responsible for periodic cleanup of the cache.
pub(crate) struct Janitor {
  handle: JoinHandle<()>,
  stop_flag: Arc<AtomicBool>,
}

impl Janitor {
  /// Spawns a new janitor thread.
  pub(crate) fn spawn<K, V, H>(context: JanitorContext<K, V, H>, tick_interval: Duration) -> Self
  where
    K: Eq + Hash + Clone + Send + Sync + 'static,
    V: Send + Sync + 'static,
    H: BuildHasher + Clone + Send + Sync + 'static,
  {
    let stop_flag = Arc::new(AtomicBool::new(false));
    let stop_clone = stop_flag.clone();

    let handle = thread::spawn(move || {
      while !stop_clone.load(Ordering::Relaxed) {
        let sleep_start = std::time::Instant::now();
        while sleep_start.elapsed() < tick_interval {
          if stop_clone.load(Ordering::Relaxed) {
            return;
          }
          thread::sleep(tick_interval);
        }

        Self::cleanup(&context);
      }
    });

    Self { handle, stop_flag }
  }

  /// The main cleanup routine, dispatching to specialized cleanup functions.
  fn cleanup<K, V, H>(context: &JanitorContext<K, V, H>)
  where
    K: Eq + Hash + Clone + Send + Sync + 'static,
    V: Send + Sync,
    H: BuildHasher + Clone + Send + Sync,
  {
    Self::cleanup_expired(context);
    Self::cleanup_capacity(context);
  }

  /// Removes expired items based on TTL/TTI.
  fn cleanup_expired<K, V, H>(context: &JanitorContext<K, V, H>)
  where
    K: Eq + Hash + Clone + Send,
    V: Send + Sync,
    H: BuildHasher + Clone + Send + Sync,
  {
    for shard_lock in context.store.iter_shards() {
      let mut expired_keys = Vec::new();

      // Phase 1: Scan for expired keys under a read lock.
      {
        let guard = shard_lock.read();
        for key in guard.keys().take(JANITOR_SAMPLE_SIZE) {
          if let Some(entry) = guard.get(key) {
            if entry.is_expired(context.time_to_idle) {
              expired_keys.push(key.clone());
            }
          }
        }
      }

      // Phase 2: Remove the collected keys under a write lock.
      if !expired_keys.is_empty() {
        let mut guard = shard_lock.write_sync();
        for key in expired_keys {
          if let Some(entry) = guard.get(&key) {
            if entry.is_expired(context.time_to_idle) {
              if let Some(removed_entry) = guard.remove(&key) {
                // Update metrics and notify other components.
                context
                  .metrics
                  .evicted_by_ttl
                  .fetch_add(1, Ordering::Relaxed);
                context
                  .metrics
                  .current_cost
                  .fetch_sub(removed_entry.cost(), Ordering::Relaxed);
                context.eviction_policy.on_remove(&key);
                if let Some(sender) = &context.notification_sender {
                  let _ =
                    sender.try_send((key.clone(), removed_entry.value(), EvictionReason::Expired));
                }
              }
            }
          }
        }
      }
    }
  }

  /// Removes items if the cache is over its cost capacity.
  fn cleanup_capacity<K, V, H>(context: &JanitorContext<K, V, H>)
  where
    K: Eq + Hash + Clone + Send + Sync + 'static,
    V: Send + Sync,
    H: BuildHasher + Clone + Send + Sync,
  {
    let current_cost = context.metrics.current_cost.load(Ordering::Relaxed);
    if current_cost > context.capacity {
      let cost_to_free = current_cost - context.capacity;

      // The policy returns the actual victims and their total cost.
      let (victims, _total_cost_released) = context.eviction_policy.evict(cost_to_free);

      if victims.is_empty() {
        return;
      }

      // The fix is to separate the removal from the store (which requires shard locks)
      // from the releasing of cost (which requires the gate lock).

      // Phase 1: Remove items from the store and collect their info.
      let mut notifications_to_send = Vec::new();

      for key in victims {
        let shard = context.store.get_shard(&key);
        let mut guard = shard.write_sync(); // Lock B

        if let Some(removed_entry) = guard.remove(&key) {
          context
            .metrics
            .evicted_by_capacity
            .fetch_add(1, Ordering::Relaxed);

          // The policy's internal state must be updated while we have the key.
          context.eviction_policy.on_remove(&key);

          let cost = removed_entry.cost();
          context
            .metrics
            .current_cost
            .fetch_sub(cost, Ordering::Relaxed);

          if let Some(_) = &context.notification_sender {
            notifications_to_send.push((
              key.clone(),
              removed_entry.value(),
              EvictionReason::Capacity,
            ));
          }
        }
        // Shard lock (Lock B) is released here as `guard` goes out of scope.
      }

      // Phase 3: Send notifications.
      if let Some(sender) = &context.notification_sender {
        for (key, value, reason) in notifications_to_send {
          let _ = sender.try_send((key, value, reason));
        }
      }
    }
  }

  /// Signals the janitor thread to stop and waits for it to finish.
  pub(crate) fn stop(self) {
    self.stop_flag.store(true, Ordering::Relaxed);
  }
}
