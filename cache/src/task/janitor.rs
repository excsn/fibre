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
  pub(crate) notification_sender: Option<mpsc::BoundedSyncSender<Notification<K, V>>>,
}

/// The background task responsible for periodic cleanup and maintenance of the cache.
pub(crate) struct Janitor {
  handle: JoinHandle<()>, // When janitor is dropped, thread is exited
  stop_flag: Arc<AtomicBool>,
}

impl Janitor {
  /// Spawns a new janitor thread.
  ///
  /// `signal_rx` carries shard indices from async paths requesting
  /// maintenance: async callers must never run drain/eviction work inline
  /// (blocking locks on an executor thread), so they signal and this thread
  /// does the work in sync context. The receive timeout doubles as the
  /// periodic tick sleep.
  pub(crate) fn spawn<K, V, H>(
    context: JanitorContext<K, V, H>,
    tick_interval: Duration,
    maintenance_probability_denominator: u32,
    signal_rx: std::sync::mpsc::Receiver<usize>,
  ) -> Self
  where
    K: Eq + Hash + Clone + Send + Sync + 'static,
    V: Send + Sync + 'static,
    H: BuildHasher + Clone + Send + Sync + 'static,
  {
    let stop_flag = Arc::new(AtomicBool::new(false));
    let stop_clone = stop_flag.clone();

    let handle = thread::spawn(move || {
      use std::sync::mpsc::RecvTimeoutError;
      loop {
        if stop_clone.load(Ordering::Relaxed) {
          break;
        }
        match signal_rx.recv_timeout(tick_interval) {
          Ok(shard_index) => {
            if stop_clone.load(Ordering::Relaxed) {
              break;
            }
            Self::signaled_maintenance(&context, shard_index, &signal_rx);
          }
          Err(RecvTimeoutError::Timeout) => {
            Self::cleanup(&context, maintenance_probability_denominator);
          }
          Err(RecvTimeoutError::Disconnected) => {
            // All senders gone: the cache is shutting down. Keep periodic
            // ticks until the stop flag lands.
            Self::cleanup(&context, maintenance_probability_denominator);
            thread::sleep(tick_interval);
          }
        }
      }
    });

    Self { handle, stop_flag }
  }

  /// Handles a burst of maintenance signals: dedups pending shard indices
  /// and runs one drain + capacity pass per shard. No probability gate -
  /// the sender side already gated.
  fn signaled_maintenance<K, V, H>(
    context: &JanitorContext<K, V, H>,
    first: usize,
    signal_rx: &std::sync::mpsc::Receiver<usize>,
  ) where
    K: Eq + Hash + Clone + Send + Sync + 'static,
    V: Send + Sync + 'static,
    H: BuildHasher + Clone + Send + Sync + 'static,
  {
    let num_shards = context.store.shards.len();
    let mut pending: Vec<usize> = Vec::with_capacity(8);
    pending.push(first);
    while let Ok(idx) = signal_rx.try_recv() {
      if !pending.contains(&idx) {
        pending.push(idx);
        if pending.len() >= num_shards {
          break;
        }
      }
    }

    for shard_index in pending {
      if shard_index >= num_shards {
        continue;
      }
      let shard = &context.store.shards[shard_index];
      // Contended means someone else is already maintaining this shard;
      // the signal is satisfied either way.
      if let Some(_guard) = shard.maintenance_lock.try_lock() {
        perform_shard_maintenance(shard, shard_index, context, JANITOR_MAINTENANCE_DRAIN_LIMIT);
        Self::cleanup_capacity_for_shard(shard, shard_index, context);
      }
    }
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
        Self::cleanup_capacity_for_shard(shard, shard_index, context);
      }
    }
  }

  /// Removes expired items based on TTL for a single shard.
  pub(crate) fn cleanup_ttl_for_shard<K, V, H>(
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
  pub(crate) fn cleanup_tti_for_shard<K, V, H>(
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

  /// Removes items for a single shard if the cache is over its cost capacity.
  /// Must be called while the shard's `maintenance_lock` is held so that the policy
  /// is guaranteed to be in sync with the map at the time eviction candidates are chosen.
  pub(crate) fn cleanup_capacity_for_shard<K, V, H>(
    shard: &Shard<K, V, H>,
    shard_index: usize,
    context: &JanitorContext<K, V, H>,
  ) where
    K: Eq + Hash + Clone + Send + Sync,
    V: Send + Sync,
    H: BuildHasher + Clone + Send + Sync,
  {
    let current_cost = context.metrics.current_cost.load(Ordering::Relaxed);
    if current_cost <= context.capacity {
      return;
    }
    let cost_to_free = current_cost - context.capacity;
    let (victims, cost_released) = context.cache_policy[shard_index].evict(cost_to_free);
    if victims.is_empty() {
      return;
    }
    {
      let mut guard = shard.map.write();
      for key in &victims {
        if let Some(removed) = guard.remove(key) {
          if let Some(sender) = &context.notification_sender {
            let _ = sender.try_send((key.clone(), removed.value(), EvictionReason::Capacity));
          }
        }
      }
    }
    context
      .metrics
      .evicted_by_capacity
      .fetch_add(victims.len() as u64, Ordering::Relaxed);
    context
      .metrics
      .current_cost
      .fetch_sub(cost_released, Ordering::Relaxed);
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

  // The write channel and the read batcher are separate streams with no
  // total order, and the batcher additionally coalesces, so a pass
  // reconstructs temporal order structurally:
  //
  //   1. collect (don't apply) this pass's write events;
  //   2. apply read accesses for keys with NO write pending in this pass -
  //      their inserts were admitted long ago and their reads happened
  //      before this pass's writes were drained;
  //   3. apply the writes (admissions / admission-driven evictions);
  //   4. apply read accesses for keys that WERE admitted in step 3 - an
  //      access can only be recorded after a map hit, and `insert()`
  //      enqueues the Write event before returning, so these reads
  //      necessarily postdate their key's insert.
  //
  // Both Left-Right batcher instances are drained: drain() flips sides, so
  // a record racing an earlier drain can sit in either - and flushes must
  // catch both. Residual advisory loss windows, self-correcting on later
  // hits: a reader hitting mid-insert (the map insert precedes the event
  // enqueue), a write backlog beyond `drain_limit`, and arbitrary ordering
  // WITHIN one read batch (the batcher coalesces). A sequence-stamped total
  // order was implemented and benched 2026-07-05: the per-hit fetch_add on
  // a shared per-shard counter cost too much read throughput and was
  // reverted in favor of this reconstruction.

  // 1. Collect this pass's write events.
  let mut writes: Vec<(K, u64)> = Vec::new();
  for _ in 0..drain_limit {
    match shard.event_buffer_rx.try_recv() {
      Ok(AccessEvent::Write(key, cost)) => writes.push((key, cost)),
      Err(_) => break,
    }
  }

  // 2. Apply accesses for keys without a pending write; defer the rest.
  let mut read_batch = shard.read_access_batcher.drain();
  read_batch.extend(shard.read_access_batcher.drain());
  let mut deferred_reads: Vec<(K, u64)> = Vec::new();
  {
    let pending_writes: HashSet<&K> = writes.iter().map(|(k, _)| k).collect();
    for (key, cost) in read_batch {
      if pending_writes.contains(&key) {
        deferred_reads.push((key, cost));
      } else {
        policy.on_access(&key, cost);
      }
    }
  }

  // 3. Apply the writes.
  for (key, cost) in writes {
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

  // 4. Apply the reads that had to wait for this pass's admissions.
  for (key, cost) in deferred_reads {
    policy.on_access(&key, cost);
  }
}
