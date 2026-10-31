use crate::EvictionReason;
use crate::entry::CacheEntry;
use crate::metrics::Metrics;
use crate::store::{ExpiredBy, Shard, ShardedStore};
use crate::task::notifier::Notification;

use fibre::mpsc;
use rand::Rng;
use std::hash::{BuildHasher, Hash};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

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

impl<K: Send, V: Send + Sync, H> JanitorContext<K, V, H> {
  /// Accounts for entries `ShardMap` removed for a TTL or TTI limit.
  pub(crate) fn account_expired(&self, expired: Vec<(K, CacheEntry<V>, ExpiredBy)>) {
    for (key, entry, by) in expired {
      let counter = match by {
        ExpiredBy::Ttl => &self.metrics.evicted_by_ttl,
        ExpiredBy::Tti => &self.metrics.evicted_by_tti,
      };
      counter.fetch_add(1, Ordering::Relaxed);
      self.metrics.current_cost.fetch_sub(entry.cost(), Ordering::Relaxed);
      if let Some(sender) = &self.notification_sender {
        let _ = sender.try_send((key, entry.value(), EvictionReason::Expired));
      }
    }
  }
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
    clock_interval: Duration,
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
      let mut since_cleanup = Duration::ZERO;
      loop {
        if stop_clone.load(Ordering::Relaxed) {
          break;
        }
        context.clock.refresh();
        thread::sleep(clock_interval);
        since_cleanup += clock_interval;
        if since_cleanup >= tick_interval {
          since_cleanup = Duration::ZERO;
          Self::cleanup(&context, maintenance_probability_denominator);
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

      if !shard.rng.should_run(maintenance_probability_denominator) {
        continue;
      }

      if let Some(_guard) = shard.maintenance_lock.try_lock() {
        // We got the lock. Perform all maintenance for this single shard.
        perform_shard_maintenance(shard);
      }
    }

    // Removal is not gated. Every shard with a timer due is swept each tick, and
    // every shard is scanned for idle entries when the cache has a time-to-idle.
    for shard in context.store.shards.iter() {
      Self::cleanup_expired_for_shard(shard, context);
      if context.time_to_idle.is_some() {
        Self::cleanup_idle_for_shard(shard, context);
      }
    }
  }

  /// Removes the entries whose timers are due on one shard.
  pub(crate) fn cleanup_expired_for_shard<K, V, H>(shard: &Shard<K, V, H>, context: &JanitorContext<K, V, H>)
  where
    K: Eq + Hash + Clone + Send,
    V: Send + Sync,
    H: BuildHasher + Clone + Send + Sync,
  {
    let now = crate::time::now_duration().as_nanos() as u64;
    if !shard.ttl_is_due(now) {
      return;
    }
    let expired = shard.map.write().expire_due(now);
    context.account_expired(expired);
  }

  fn cleanup_idle_for_shard<K, V, H>(shard: &Shard<K, V, H>, context: &JanitorContext<K, V, H>)
  where
    K: Eq + Hash + Clone + Send,
    V: Send + Sync,
    H: BuildHasher + Clone + Send + Sync,
  {
    // A long scan must not age the clock past what `idle_limit` allows for.
    context.clock.refresh();
    let now = crate::time::now_duration().as_nanos() as u64;
    let hashes = shard.map.read().idle_hashes(now);
    if hashes.is_empty() {
      return;
    }
    let expired = shard.map.write().remove_expired_hashes(hashes, now);
    context.account_expired(expired);
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
