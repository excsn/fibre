use crate::entry::CacheEntry;
use crate::metrics::Metrics;
use crate::policy::{AccessEvent, AccessInfo, AdmissionDecision, CachePolicy};
use crate::store::ShardedStore;
use crate::task::notifier::Notification;
use crate::task::timer::TimerWheel;
use crate::EvictionReason;

use fibre::mpsc;
use parking_lot::Mutex;
use std::hash::{BuildHasher, Hash};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// The number of entries to sample from each shard on an expiration cleanup tick.
const JANITOR_EXPIRE_SAMPLE_SIZE: usize = 10;
/// The max number of access events to drain from a shard's buffer on a maintenance tick.
const MAINTENANCE_DRAIN_LIMIT: usize = 128;

/// A context object holding the thread-safe parts of the cache that the
/// janitor needs to access.
pub(crate) struct JanitorContext<K: Send, V: Send + Sync, H> {
  pub(crate) store: Arc<ShardedStore<K, V, H>>,
  pub(crate) metrics: Arc<Metrics>,
  pub(crate) eviction_policy: Arc<dyn CachePolicy<K, V>>,
  pub(crate) timer_wheel: Option<Arc<TimerWheel>>,
  pub(crate) capacity: u64,
  pub(crate) time_to_idle: Option<Duration>,
  pub(crate) notification_sender: Option<mpsc::BoundedSender<Notification<K, V>>>,
}

/// The background task responsible for periodic cleanup and maintenance of the cache.
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

        // Perform all maintenance tasks.
        Self::cleanup(&context);

        // Sleep for the remaining duration of the tick interval.
        if let Some(remaining) = tick_interval.checked_sub(sleep_start.elapsed()) {
          thread::sleep(remaining);
        }
      }
    });

    Self { handle, stop_flag }
  }

  /// The main cleanup and maintenance routine.
  fn cleanup<K, V, H>(context: &JanitorContext<K, V, H>)
  where
    K: Eq + Hash + Clone + Send + Sync + 'static,
    V: Send + Sync + 'static,
    H: BuildHasher + Clone + Send + Sync + 'static,
  {
    // First, apply all buffered access events to the eviction policy.
    Self::perform_maintenance(context);
    // Then, clean up expired items from TTL and TTI.
    Self::cleanup_ttl(context);
    Self::cleanup_tti(context);
    // Finally, enforce capacity.
    Self::cleanup_capacity(context);
  }

  /// Drains access event buffers and applies them to the eviction policy.
  fn perform_maintenance<K, V, H>(context: &JanitorContext<K, V, H>)
  where
    K: Eq + Hash + Clone + Send + Sync + 'static,
    V: Send + Sync,
    H: BuildHasher + Clone + Send + Sync,
  {
    for shard in context.store.iter_shards() {
      // Drain a bounded number of events to prevent this loop from running too long.
      for _ in 0..MAINTENANCE_DRAIN_LIMIT {
        match shard.event_buffer_rx.try_recv() {
          Ok(event) => {
            match event {
              AccessEvent::Read(key) => {
                // We need to look up the entry in the map to create AccessInfo.
                let guard = shard.map.read();
                if let Some(entry) = guard.get(&key) {
                  let info = AccessInfo { key: &key, entry };
                  context.eviction_policy.on_access(&info);
                }
              }
              AccessEvent::Write(key, cost) => {
                let decision = context.eviction_policy.on_admit(&key, cost);

                if let AdmissionDecision::AdmitAndEvict(victims) = decision {
                  let mut notifications_to_send = Vec::new();
                  let mut total_cost_released = 0;

                  for victim_key in victims {
                    let victim_shard = context.store.get_shard(&victim_key);
                    let mut guard = victim_shard.map.write_sync();

                    if let Some(removed_entry) = guard.remove(&victim_key) {
                      let victim_cost = removed_entry.cost();
                      total_cost_released += victim_cost;
                      context
                        .metrics
                        .evicted_by_capacity
                        .fetch_add(1, Ordering::Relaxed);
                      if context.notification_sender.is_some() {
                        notifications_to_send.push((
                          victim_key.clone(),
                          removed_entry.value(),
                          EvictionReason::Capacity,
                        ));
                      }
                    }
                  }

                  context
                    .metrics
                    .current_cost
                    .fetch_sub(total_cost_released, Ordering::Relaxed);

                  if let Some(sender) = &context.notification_sender {
                    for notif in notifications_to_send {
                      let _ = sender.try_send(notif);
                    }
                  }
                }
              }
            }
          }
          Err(_) => {
            // The buffer is empty for this shard, move to the next one.
            break;
          }
        }
      }
    }
  }

  /// Removes expired items based on TTL.
  fn cleanup_ttl<K, V, H>(context: &JanitorContext<K, V, H>)
  where
    K: Eq + Hash + Clone + Send,
    V: Send + Sync,
    H: BuildHasher + Clone + Send + Sync,
  {
    let expired_hashes = match &context.timer_wheel {
      Some(wheel) => wheel.advance(),
      None => return,
    };

    if expired_hashes.is_empty() {
      return;
    }

    // Group hashes by shard to minimize locking
    let mut hashes_by_shard: Vec<Vec<u64>> = vec![Vec::new(); context.store.shards.len()];
    for hash in expired_hashes {
      let index = hash as usize % context.store.shards.len();
      hashes_by_shard[index].push(hash);
    }

    for (i, hashes) in hashes_by_shard.into_iter().enumerate() {
      if hashes.is_empty() {
        continue;
      }

      let shard = &context.store.shards[i];
      let mut guard = shard.map.write_sync();

      // It's more efficient to retain than to remove one-by-one.
      guard.retain(|key, entry| {
        let key_hash = crate::store::hash_key(&context.store.hasher, key);
        // If the timer wheel fired for this item's hash, we treat it as expired.
        // The is_expired() check is redundant and racy, as the janitor tick might
        // run slightly before the entry's exact expiration timestamp.
        if hashes.contains(&key_hash) {
          // This entry has expired.
          context.eviction_policy.on_remove(key);
          context
            .metrics
            .evicted_by_ttl
            .fetch_add(1, Ordering::Relaxed);
          context
            .metrics
            .current_cost
            .fetch_sub(entry.cost(), Ordering::Relaxed);
          if let Some(sender) = &context.notification_sender {
            let _ = sender.try_send((key.clone(), entry.value(), EvictionReason::Expired));
          }
          false // Remove from map.
        } else {
          true // Keep in map.
        }
      });
    }
  }

  /// Removes expired items based on TTI by sampling.
  fn cleanup_tti<K, V, H>(context: &JanitorContext<K, V, H>)
  where
    K: Eq + Hash + Clone + Send,
    V: Send + Sync,
    H: BuildHasher + Clone + Send + Sync,
  {
    if let Some(tti) = context.time_to_idle {
      for shard in context.store.iter_shards() {
        let mut guard = shard.map.write_sync();
        let mut victims = Vec::new();

        for (key, entry) in guard.iter().take(JANITOR_EXPIRE_SAMPLE_SIZE) {
          if entry.is_expired(Some(tti)) && !entry.is_expired(None) {
            victims.push(key.clone());
          }
        }

        for key in victims {
          if let Some(entry) = guard.remove(&key) {
            context.eviction_policy.on_remove(&key);
            context
              .metrics
              .evicted_by_tti
              .fetch_add(1, Ordering::Relaxed);
            context
              .metrics
              .current_cost
              .fetch_sub(entry.cost(), Ordering::Relaxed);

            // An item expired by TTI might still have a TTL timer. Cancel it.
            if let Some(wheel) = &context.timer_wheel {
              if let Some(handle) = &entry.ttl_timer_handle {
                wheel.cancel(handle);
              }
            }

            if let Some(sender) = &context.notification_sender {
              let _ = sender.try_send((key, entry.value(), EvictionReason::Expired));
            }
          }
        }
      }
    }
  }

  /// Removes items if the cache is over its cost capacity.
  fn cleanup_capacity<K, V, H>(context: &JanitorContext<K, V, H>)
  where
    K: Eq + Hash + Clone + Send + Sync,
    V: Send + Sync,
    H: BuildHasher + Clone + Send + Sync,
  {
    let current_cost = context.metrics.current_cost.load(Ordering::Relaxed);
    if current_cost > context.capacity {
      let cost_to_free = current_cost - context.capacity;
      let (victims, total_cost_released) = context.eviction_policy.evict(cost_to_free);

      if victims.is_empty() {
        return;
      }

      let mut notifications_to_send = Vec::new();
      for key in victims {
        let shard = context.store.get_shard(&key);
        let mut guard = shard.map.write_sync();

        if let Some(removed_entry) = guard.remove(&key) {
          // The policy already updated its own state when evict() was called,
          // so we do not call on_remove() here. We just remove the entry
          // from the primary cache storage.
          context
            .metrics
            .evicted_by_capacity
            .fetch_add(1, Ordering::Relaxed);
          if let Some(_) = &context.notification_sender {
            notifications_to_send.push((
              key.clone(),
              removed_entry.value(),
              EvictionReason::Capacity,
            ));
          }
        }
      }

      context
        .metrics
        .current_cost
        .fetch_sub(total_cost_released, Ordering::Relaxed);

      if let Some(sender) = &context.notification_sender {
        for (key, value, reason) in notifications_to_send {
          let _ = sender.try_send((key, value, reason));
        }
      }
    }
  }

  /// Signals the janitor thread to stop.
  pub(crate) fn stop(self) {
    self.stop_flag.store(true, Ordering::Relaxed);
  }
}
