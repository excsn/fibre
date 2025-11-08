use crate::EvictionReason;
use crate::metrics::Metrics;
use crate::policy::{AccessEvent, AdmissionDecision, CachePolicy};
use crate::store::{Shard, ShardedStore};
use crate::task::notifier::Notification;

use fibre::mpsc;
use rand::Rng;
use std::collections::HashSet;
use std::hash::{BuildHasher, Hash};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// The number of entries to sample from each shard on an expiration cleanup tick.
const JANITOR_EXPIRE_SAMPLE_SIZE: usize = 10;

/// The max number of access events to drain from a shard's buffer during
/// cooperative maintenance (e.g., on an insert). This should be small
/// to keep the user-facing operation fast.
pub(crate) const COOPERATIVE_MAINTENANCE_DRAIN_LIMIT: usize = 16;

/// The max number of access events to drain from a shard's buffer during
/// a janitor run. This can be larger as it runs on a background thread.
const JANITOR_MAINTENANCE_DRAIN_LIMIT: usize = 256;

/// The number of random shards the janitor will attempt to check on each tick.
const JANITOR_CHECKS_PER_TICK: usize = 2;

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
  handle: JoinHandle<()>, // When janitor is dropped, thread is exited
  stop_flag: Arc<AtomicBool>,
}

impl Janitor {
  /// Spawns a new janitor thread.
  pub(crate) fn spawn<K, V, H>(
    context: JanitorContext<K, V, H>,
    tick_interval: Duration,
    maintenance_probability_denominator: u32,
  ) -> Self
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
        Self::cleanup(&context, maintenance_probability_denominator);

        // Sleep for the remaining duration of the tick interval.
        if let Some(remaining) = tick_interval.checked_sub(sleep_start.elapsed()) {
          thread::sleep(remaining);
        }
      }
    });

    Self { handle, stop_flag }
  }

  /// The main cleanup and maintenance routine, probabilistic.
  fn cleanup<K, V, H>(context: &JanitorContext<K, V, H>, maintenance_probability_denominator: u32)
  where
    K: Eq + Hash + Clone + Send + Sync + 'static,
    V: Send + Sync + 'static,
    H: BuildHasher + Clone + Send + Sync + 'static,
  {
    let num_shards = context.store.shards.len();
    if num_shards == 0 {
      return;
    }

    let mut rng = rand::rng();

    // On each janitor tick, we check a few random shards.
    for _ in 0..JANITOR_CHECKS_PER_TICK.min(num_shards) {
      let shard_index = rng.random_range(0..num_shards);
      let shard = &context.store.shards[shard_index];

      // The check is now a simple, fast method call on the shard's FastRng.
      if !shard.rng.should_run(maintenance_probability_denominator) {
        continue;
      }

      // Use the same probability as opportunistic maintenance.
      if let Some(_guard) = shard.maintenance_lock.try_lock() {
        // We got the lock. Perform all maintenance for this single shard.
        perform_shard_maintenance(shard, shard_index, context, JANITOR_MAINTENANCE_DRAIN_LIMIT);
        Self::cleanup_ttl_for_shard(shard, shard_index, context);
        Self::cleanup_tti_for_shard(shard, shard_index, context);
      }
    }

    // The capacity check remains global and runs on every tick to enforce
    // the hard memory limit reliably.
    Self::cleanup_capacity(context);
  }

  /// Removes expired items based on TTL for a single shard.
  fn cleanup_ttl_for_shard<K, V, H>(
    shard: &Shard<K, V, H>,
    shard_index: usize,
    context: &JanitorContext<K, V, H>,
  ) where
    K: Eq + Hash + Clone + Send,
    V: Send + Sync,
    H: BuildHasher + Clone + Send + Sync,
  {
    let expired_hashes = match &shard.timer_wheel {
      Some(wheel) => wheel.advance(),
      None => return,
    };

    if expired_hashes.is_empty() {
      return;
    }

    let expired_set: HashSet<u64> = expired_hashes.into_iter().collect();
    let mut guard = shard.map.write();

    guard.retain(|key, entry| {
      let key_hash = crate::store::hash_key(&context.store.hasher, key);

      if expired_set.contains(&key_hash) {
        context.cache_policy[shard_index].on_remove(key);
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

  /// Removes expired items based on TTI for a single shard by sampling.
  fn cleanup_tti_for_shard<K, V, H>(
    shard: &Shard<K, V, H>,
    shard_index: usize,
    context: &JanitorContext<K, V, H>,
  ) where
    K: Eq + Hash + Clone + Send,
    V: Send + Sync,
    H: BuildHasher + Clone + Send + Sync,
  {
    if context.time_to_idle.is_none() {
      return;
    }

    let mut guard = shard.map.write();
    let mut victims = Vec::new();

    for (key, entry) in guard.iter().take(JANITOR_EXPIRE_SAMPLE_SIZE) {
      if entry.is_expired(context.time_to_idle) {
        victims.push(key.clone());
      }
    }

    if victims.is_empty() {
      return;
    }

    for key in victims {
      if let Some(entry) = guard.remove(&key) {
        context.cache_policy[shard_index].on_remove(&key);
        context
          .metrics
          .evicted_by_tti
          .fetch_add(1, Ordering::Relaxed);
        context
          .metrics
          .current_cost
          .fetch_sub(entry.cost(), Ordering::Relaxed);

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

  /// Removes items if the cache is over its cost capacity. (Unchanged - remains global)
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

    let mut cost_to_free = current_cost - context.capacity;
    let num_shards = context.store.shards.len();
    let mut total_cost_released = 0;
    let mut total_victims_count = 0;

    // Iterate through each shard to distribute the eviction load.
    for i in 0..num_shards {
      // If we've already freed enough space, stop.
      if cost_to_free == 0 {
        break;
      }

      // Calculate a proportional amount for this shard to free.
      let amount_for_this_shard = (cost_to_free as f64 / (num_shards - i) as f64).ceil() as u64;

      let (victims, cost_released) = context.cache_policy[i].evict(amount_for_this_shard);
      if victims.is_empty() {
        continue;
      }

      total_cost_released += cost_released;
      total_victims_count += victims.len();
      cost_to_free = cost_to_free.saturating_sub(cost_released);

      let shard = &context.store.shards[i];
      let mut guard = shard.map.write();
      for key in &victims {
        if let Some(removed_entry) = guard.remove(key) {
          if let Some(sender) = &context.notification_sender {
            let _ = sender.try_send((key.clone(), removed_entry.value(), EvictionReason::Capacity));
          }
        }
      }
    }

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
  drain_limit: usize,
) where
  K: Eq + Hash + Clone + Send,
  V: Send + Sync,
  H: BuildHasher + Clone,
{
  let policy = &context.cache_policy[shard_index];

  // 1. Drain the lock-free read batcher and apply all access events.
  let read_batch = shard.read_access_batcher.drain();
  if !read_batch.is_empty() {
    for (key, cost) in read_batch {
      policy.on_access(&key, cost);
    }
  }

  // Drain a bounded number of events to prevent this loop from running too long.
  for _ in 0..drain_limit {
    match shard.event_buffer_rx.try_recv() {
      Ok(event) => match event {
        AccessEvent::Write(key, cost) => {
          let policy = &context.cache_policy[shard_index];
          let decision = policy.on_admit(&key, cost);

          if let AdmissionDecision::AdmitAndEvict(victims) = decision {
            let mut notifications_to_send = Vec::new();
            let mut total_cost_released = 0;

            for victim_key in victims {
              // The victim could be in another shard, so we must look it up.
              let victim_shard_index = context.store.get_shard_index(&victim_key);
              let victim_shard = &context.store.shards[victim_shard_index];

              let mut guard = victim_shard.map.write();

              if let Some(removed_entry) = guard.remove(&victim_key) {
                // If we evicted a key, we must also tell its original policy.
                context.cache_policy[victim_shard_index].on_remove(&victim_key);

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
