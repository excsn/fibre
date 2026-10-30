use crate::EvictionReason;
use crate::metrics::Metrics;
use crate::store::{Shard, ShardedStore};
use crate::task::notifier::Notification;

use fibre::mpsc;
use rand::Rng;
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


/// The number of random shards the janitor will attempt to check on each tick.
const JANITOR_CHECKS_PER_TICK: usize = 2;

/// A context object holding the thread-safe parts of the cache that the
/// janitor needs to access.
pub(crate) struct JanitorContext<K: Send, V: Send + Sync, H> {
  pub(crate) store: Arc<ShardedStore<K, V, H>>,
  pub(crate) clock: Arc<crate::time::CoarseClock>,
  pub(crate) metrics: Arc<Metrics>,
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
        context.clock.refresh();
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
        perform_shard_maintenance(shard);
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
        perform_shard_maintenance(shard);
        Self::cleanup_tti_for_shard(shard, shard_index, context);
      }
    }

    // TTL removal is not gated: every shard with a timer due is swept each tick.
    for (shard_index, shard) in context.store.shards.iter().enumerate() {
      Self::cleanup_ttl_for_shard(shard, shard_index, context);
    }
  }

  /// Removes expired items based on TTL for a single shard.
  pub(crate) fn cleanup_ttl_for_shard<K, V, H>(
    shard: &Shard<K, V, H>,
    _shard_index: usize,
    context: &JanitorContext<K, V, H>,
  ) where
    K: Eq + Hash + Clone + Send,
    V: Send + Sync,
    H: BuildHasher + Clone + Send + Sync,
  {
    let now = crate::time::now_duration().as_nanos() as u64;
    if !shard.ttl_is_due(now) {
      return;
    }
    let expired = shard.map.write().expire_due(now);
    for (key, entry) in expired {
      context.metrics.evicted_by_ttl.fetch_add(1, Ordering::Relaxed);
      context.metrics.current_cost.fetch_sub(entry.cost(), Ordering::Relaxed);
      if let Some(sender) = &context.notification_sender {
        let _ = sender.try_send((key, entry.value(), EvictionReason::Expired));
      }
    }
  }

  /// Removes expired items based on TTI for a single shard by sampling.
  pub(crate) fn cleanup_tti_for_shard<K, V, H>(
    shard: &Shard<K, V, H>,
    _shard_index: usize,
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
        context
          .metrics
          .evicted_by_tti
          .fetch_add(1, Ordering::Relaxed);
        context
          .metrics
          .current_cost
          .fetch_sub(entry.cost(), Ordering::Relaxed);

        if let Some(sender) = &context.notification_sender {
          let _ = sender.try_send((key, entry.value(), EvictionReason::Expired));
        }
      }
    }
  }

  /// Signals the janitor thread to stop.
  pub(crate) fn stop(self) {
    self.stop_flag.store(true, Ordering::Relaxed);
  }
}

/// Applies a shard's recorded reads to its policy under the shard's write lock.
/// Writes reach the policy on insert, so there is no write stream to drain.
pub(crate) fn perform_shard_maintenance<K, V, H>(shard: &Shard<K, V, H>)
where
  K: Eq + Hash + Clone + Send,
  H: BuildHasher,
{
  shard.map.write().apply_reads();
}
