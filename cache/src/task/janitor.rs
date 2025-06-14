use crate::metrics::Metrics;
use crate::policy::{AccessEvent, AdmissionDecision, CachePolicy};
use crate::store::{Shard, ShardedStore};
use crate::task::notifier::Notification;
use crate::EvictionReason;

use fibre::mpsc;
use std::collections::HashSet;
use std::hash::{BuildHasher, Hash};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// The number of entries to sample from each shard on an expiration cleanup tick.
const JANITOR_EXPIRE_SAMPLE_SIZE: usize = 10;
/// The max number of access events to drain from a shard's buffer on a maintenance tick.
const MAINTENANCE_DRAIN_LIMIT: usize = 64;

/// A context object holding the thread-safe parts of the cache that the
/// janitor needs to access.
pub(crate) struct JanitorContext<K: Send, V: Send + Sync, H> {
  pub(crate) store: Arc<ShardedStore<K, V, H>>,
  pub(crate) metrics: Arc<Metrics>,
  pub(crate) cache_policy: Box<[Arc<dyn CachePolicy<K, V>>]>,
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
    // Then, clean up expired items from TTL and TTI.
    Self::cleanup_ttl(context);
    Self::cleanup_tti(context);
    // Finally, enforce capacity.
    Self::cleanup_capacity(context);
  }

  /// Removes expired items based on TTL.
  fn cleanup_ttl<K, V, H>(context: &JanitorContext<K, V, H>)
  where
    K: Eq + Hash + Clone + Send,
    V: Send + Sync,
    H: BuildHasher + Clone + Send + Sync,
  {
    // Iterate over each shard and advance its independent timer wheel.
    for (i, shard) in context.store.iter_shards().enumerate() {
      // Advance this shard's timer and get its expired keys.
      let expired_hashes = match &shard.timer_wheel {
        Some(wheel) => wheel.advance(),
        None => continue, // This shard has no timer, so skip.
      };

      if expired_hashes.is_empty() {
        continue;
      }

      let expired_set: HashSet<u64> = expired_hashes.into_iter().collect();
      let mut guard = shard.map.write();

      // Since these keys are from this shard's timer, we know they belong
      // in this shard's map.
      guard.retain(|key, entry| {
        let key_hash = crate::store::hash_key(&context.store.hasher, key);

        if expired_set.contains(&key_hash) {
          // This entry has expired.
          context.cache_policy[i].on_remove(key);
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
    if context.time_to_idle.is_some() {
      // We must enumerate the shards to get the index for the policy.
      for (i, shard) in context.store.iter_shards().enumerate() {
        
        let mut guard = shard.map.write();
        let mut victims = Vec::new();

        // We approximate random sampling by taking the first N items from the
        // shard's iterator. The order is arbitrary due to HashMap's internal layout.
        // This is much more efficient than a full scan.
        for (key, entry) in guard.iter().take(JANITOR_EXPIRE_SAMPLE_SIZE) {
          // Pass the TTI value to is_expired. This is the only check we need to do.
          if entry.is_expired(context.time_to_idle) {
            victims.push(key.clone());
          }
        }

        if victims.is_empty() {
          continue; // No victims found in this shard, move to the next.
        }

        for key in victims {
          // We must re-check that the entry still exists before removing,
          // as another thread could have invalidated it in the meantime.
          if let Some(entry) = guard.remove(&key) {
            // Select the policy for the correct shard index.
            context.cache_policy[i].on_remove(&key);

            context
              .metrics
              .evicted_by_tti
              .fetch_add(1, Ordering::Relaxed);
            context
              .metrics
              .current_cost
              .fetch_sub(entry.cost(), Ordering::Relaxed);

            // An item expired by TTI might still have a TTL timer. Cancel it.
            if let Some(wheel) = &shard.timer_wheel {
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
    if current_cost <= context.capacity {
      return;
    }

    let cost_to_free = current_cost - context.capacity;
    let num_shards = context.store.shards.len();
    // Calculate how much cost each shard's policy should try to free.
    let cost_to_free_per_shard = (cost_to_free as f64 / num_shards as f64).ceil() as u64;

    if cost_to_free_per_shard == 0 {
      return;
    }

    let mut total_cost_released = 0;
    let mut total_victims_count = 0;

    // Evict from each shard's policy in a deterministic order.
    for i in 0..num_shards {
      let (victims, cost_released) = context.cache_policy[i].evict(cost_to_free_per_shard);
      if victims.is_empty() {
        continue;
      }
      total_cost_released += cost_released;
      total_victims_count += victims.len();

      // Get the corresponding data shard and remove the victim keys from it.
      let shard = &context.store.shards[i];
      let mut guard = shard.map.write();
      for key in &victims {
        // The policy for shard `i` should only evict keys that belong to shard `i`.
        // We remove the key from the HashMap, and if successful, send a notification.
        if let Some(removed_entry) = guard.remove(key) {
          if let Some(sender) = &context.notification_sender {
            let _ = sender.try_send((key.clone(), removed_entry.value(), EvictionReason::Capacity));
          }
        }
      }
    }

    // Update global metrics once at the end.
    if total_cost_released > 0 {
      context
        .metrics
        .evicted_by_capacity
        .fetch_add(total_victims_count as u64, Ordering::Relaxed);
      context
        .metrics
        .current_cost
        .fetch_sub(total_cost_released, Ordering::Relaxed);
    }
  }

  /// Signals the janitor thread to stop.
  pub(crate) fn stop(self) {
    self.stop_flag.store(true, Ordering::Relaxed);
  }
}

/// Drains access event buffer for a single shard and applies them to the eviction policy.
/// This is now public within the crate so other parts of the cache can call it.
pub(crate) fn perform_shard_maintenance<K, V, H>(
  shard: &Shard<K, V, H>,
  shard_index: usize,
  context: &JanitorContext<K, V, H>,
) where
  K: Eq + Hash + Clone + Send,
  V: Send + Sync,
  H: BuildHasher + Clone,
{
  // Drain a bounded number of events to prevent this loop from running too long.
  for _ in 0..MAINTENANCE_DRAIN_LIMIT {
    match shard.event_buffer_rx.try_recv() {
      Ok(event) => match event {
        AccessEvent::Write(key, cost) => {
          let policy = &context.cache_policy[shard_index];
          let decision = policy.on_admit(&key, cost);

          if let AdmissionDecision::AdmitAndEvict(victims) = decision {
            let mut notifications_to_send = Vec::new();
            let mut total_cost_released = 0;

            for victim_key in victims {
              let victim_shard = context.store.get_shard(&victim_key);
              let mut guard = victim_shard.map.write();

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
      },
      Err(_) => {
        // The buffer is empty for this shard, we are done.
        break;
      }
    }
  }
}
