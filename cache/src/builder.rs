use crate::error::BuildError;
use crate::handles::{AsyncCache, Cache};
use crate::loader::Loader;
use crate::metrics::Metrics;
use crate::policy::tinylfu::TinyLfuPolicy;
use crate::policy::CachePolicy;
use crate::shared::CacheShared;
use crate::snapshot::CacheSnapshot;
use crate::store::{hash_key, ShardedStore};
use crate::task::janitor::{Janitor, JanitorContext};
use crate::task::notifier::Notifier;
use crate::task::timer::TimerWheel;
use crate::{time, EvictionListener, TaskSpawner};

use core::fmt;
use std::collections::HashMap;
use std::future::Future;
use std::hash::{BuildHasher, Hash};
use std::marker::PhantomData;
use std::pin::Pin;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;

/// Defines preset configurations for the cache's internal timer wheel,
/// which manages TTL and TTI expirations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimerWheelMode {
  /// A general-purpose configuration suitable for a wide range of workloads.
  ///
  /// - Granularity: 1 second
  /// - Wheel Size: 60 slots (1-minute cycle)
  Default,

  /// Optimized for caches where items have very short lifetimes (e.g., milliseconds).
  /// Provides high-precision expiration at the cost of slightly more overhead.
  ///
  /// - Granularity: 10 milliseconds
  /// - Wheel Size: 100 slots (1-second cycle)
  HighPrecisionShortLived,

  /// Optimized for caches where items have very long lifetimes (e.g., many minutes or hours).
  /// Reduces periodic work by using a coarse granularity.
  ///
  /// - Granularity: 30 seconds
  /// - Wheel Size: 120 slots (1-hour cycle)
  LowPrecisionLongLived,
}

/// A builder for creating `Cache` and `AsyncCache` instances.
pub struct CacheBuilder<K: Send, V: Send, H = ahash::RandomState> {
  pub(crate) capacity: u64,
  pub(crate) shards: usize,
  pub(crate) time_to_live: Option<Duration>,
  pub(crate) time_to_idle: Option<Duration>,
  pub(crate) hasher: H,
  pub(crate) stale_while_revalidate: Option<Duration>,
  pub(crate) janitor_tick_interval: Option<Duration>,
  timer_wheel_tick_duration: Option<Duration>,
  timer_wheel_size: Option<usize>,
  listener: Option<Arc<dyn EvictionListener<K, V>>>,
  cache_policy: Option<Arc<dyn CachePolicy<K, V>>>,
  loader: Option<Loader<K, V>>,
  spawner: Option<Arc<dyn TaskSpawner>>,
  _key_marker: PhantomData<K>,
  _value_marker: PhantomData<V>,
}

// Manual Debug implementation for CacheBuilder.
impl<K: Send, V: Send, H> fmt::Debug for CacheBuilder<K, V, H> {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("CacheBuilder")
      .field("capacity", &self.capacity)
      .field("shards", &self.shards)
      .field("time_to_live", &self.time_to_live)
      .field("time_to_idle", &self.time_to_idle)
      .field("has_listener", &self.listener.is_some())
      .finish_non_exhaustive()
  }
}

// --- General Configuration Methods ---
// This impl block has no restrictive bounds on K or V.
impl<K: Send, V: Send, H> CacheBuilder<K, V, H> {
  /// Sets the maximum total cost of the cache.
  pub fn capacity(mut self, capacity: u64) -> Self {
    self.capacity = capacity;
    self
  }

  /// Sets the cache to be "unbounded".
  pub fn unbounded(mut self) -> Self {
    self.capacity = u64::MAX;
    self
  }

  /// Sets the number of concurrent shards to use.
  pub fn shards(mut self, shards: usize) -> Self {
    // Ensure shards is at least 1 and a power of two for fast bitwise ANDing.
    self.shards = shards.max(1).next_power_of_two();
    self
  }

  /// Sets a time-to-live (TTL) for all entries in the cache.
  pub fn time_to_live(mut self, duration: Duration) -> Self {
    self.time_to_live = Some(duration);
    self
  }

  /// Sets a time-to-idle (TTI) for all entries in the cache.
  pub fn time_to_idle(mut self, duration: Duration) -> Self {
    self.time_to_idle = Some(duration);
    self
  }

  /// Sets the eviction listener for the cache.
  pub fn eviction_listener<Listener>(mut self, listener: Listener) -> Self
  where
    Listener: EvictionListener<K, V> + 'static,
  {
    self.listener = Some(Arc::new(listener));
    self
  }

  // In the general `impl<K, V, H> CacheBuilder` block, add this new method:
  /// Sets a custom eviction policy for the cache.
  ///
  /// By default, the cache uses a `TinyLfu` policy.
  pub fn cache_policy<Policy>(mut self, policy: Policy) -> Self
  where
    Policy: CachePolicy<K, V> + 'static,
  {
    self.cache_policy = Some(Arc::new(policy));
    self
  }

  /// Sets the synchronous loader function for the cache.
  ///
  /// The provided closure is called by `get_with` when a key is not present.
  /// It must return a tuple of `(value, cost)`.
  pub fn loader(mut self, f: impl Fn(K) -> (V, u64) + Send + Sync + 'static) -> Self {
    self.loader = Some(Loader::Sync(Arc::new(f)));
    self
  }

  /// Sets the asynchronous loader function for the cache.
  ///
  /// The provided closure is called by `get_with` when a key is not present.
  /// It must return a tuple of `(value, cost)`.
  pub fn async_loader<F, Fut>(mut self, f: F) -> Self
  where
    F: Fn(K) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = (V, u64)> + Send + 'static,
  {
    let loader_fn = move |key| Box::pin(f(key)) as Pin<Box<dyn Future<Output = (V, u64)> + Send>>;
    self.loader = Some(Loader::Async(Arc::new(loader_fn)));
    self
  }

  pub fn spawner(mut self, spawner: Arc<dyn TaskSpawner>) -> Self {
    self.spawner = Some(spawner);
    self
  }

  /// Sets the tick interval for the background cleanup task (janitor).
  /// (Primarily for testing purposes).
  #[doc(hidden)] // Hide from public docs unless we want to make it a first-class feature
  pub fn janitor_tick_interval(mut self, duration: Duration) -> Self {
    self.janitor_tick_interval = Some(duration);
    self
  }

  /// Sets the timer wheel configuration using a convenient preset.
  ///
  /// This will set both the `tick_duration` and `wheel_size` internally.
  /// Any subsequent calls to `.timer_tick_duration()` or `.timer_wheel_size()`
  /// will override the values set by this preset.
  pub fn timer_mode(mut self, mode: TimerWheelMode) -> Self {
    let (size, duration) = match mode {
      TimerWheelMode::Default => (60, Duration::from_secs(1)),
      TimerWheelMode::HighPrecisionShortLived => (100, Duration::from_millis(10)),
      TimerWheelMode::LowPrecisionLongLived => (120, Duration::from_secs(30)),
    };

    self.timer_wheel_size = Some(size);
    self.timer_wheel_tick_duration = Some(duration);
    self
  }

  /// Sets the granularity of the timer wheel for TTL/TTI expirations.
  ///
  /// This is the duration that each "tick" of the wheel represents. A smaller
  /// duration provides more precise expiration but may have slightly higher
  /// overhead.
  ///
  /// Defaults to `1 second` if not set.
  pub fn timer_tick_duration(mut self, duration: Duration) -> Self {
    self.timer_wheel_tick_duration = Some(duration);
    self
  }

  /// Sets the number of slots in the timer wheel for TTL/TTI expirations.
  ///
  /// The total cycle time of the wheel is `tick_duration * wheel_size`.
  /// A larger wheel can accommodate longer TTLs with fewer "laps," at the
  /// cost of slightly more memory.
  ///
  /// Defaults to `60` slots if not set.
  pub fn timer_wheel_size(mut self, size: usize) -> Self {
    self.timer_wheel_size = Some(size);
    self
  }
}

// --- Default Constructor ---
impl<K: Send, V: Send, H: BuildHasher + Default> CacheBuilder<K, V, H> {
  /// Creates a new `CacheBuilder` with default settings.
  pub fn new() -> Self {
    Self {
      capacity: u64::MAX,
      shards: (num_cpus::get() * 4).max(1).next_power_of_two(),
      time_to_live: None,
      time_to_idle: None,
      hasher: H::default(),
      stale_while_revalidate: None,
      janitor_tick_interval: None,
      timer_wheel_tick_duration: None, // Will default to 1s if not set
      timer_wheel_size: None,          // Will default to 60 if not set
      listener: None,
      cache_policy: None,
      loader: None,
      spawner: None,
      _key_marker: PhantomData,
      _value_marker: PhantomData,
    }
  }
}

impl<K: Send, V: Send> Default for CacheBuilder<K, V, ahash::RandomState> {
  fn default() -> Self {
    Self::new()
  }
}

impl<K: Send, V: Send> CacheBuilder<K, V, rapidhash::RapidRandomState> {
  #[cfg(feature = "rapidhash")]
  pub fn rapidhash() -> Self {
    Self::new()
  }
}

// --- Build Methods ---
// This impl block contains the full set of trait bounds required to actually
// construct the cache, including `K: Clone` for the janitor.
impl<K, V, H> CacheBuilder<K, V, H>
where
  K: Eq + Hash + Clone + Send + Sync + 'static,
  V: Send + Sync + 'static,
  H: BuildHasher + Clone + Send + Sync + 'static,
{
  /// Sets the hasher for the cache.
  pub fn hasher(mut self, hasher: H) -> Self {
    self.hasher = hasher;
    self
  }

  /// Sets a duration after an entry's TTL expires during which the cache can
  /// still serve the stale value while asynchronously refreshing it in the
  /// background.
  ///
  /// This feature requires a `loader` or `async_loader` to be configured.
  pub fn stale_while_revalidate(mut self, duration: Duration) -> Self {
    self.stale_while_revalidate = Some(duration);
    self
  }

  /// Builds a synchronous `Cache`.
  pub fn build(mut self) -> Result<Cache<K, V, H>, BuildError> {
    self.validate()?;
    let shared = self.build_shared_core(None)?;
    Ok(Cache { shared })
  }

  /// Builds an asynchronous `AsyncCache`.
  pub fn build_async(mut self) -> Result<AsyncCache<K, V, H>, BuildError> {
    self.validate()?;
    let shared = self.build_shared_core(None)?;
    Ok(AsyncCache { shared })
  }

  /// Central logic to construct the shared core of the cache.
  pub(crate) fn build_shared_core(
    &mut self,
    snapshot: Option<CacheSnapshot<K, V>>,
  ) -> Result<Arc<CacheShared<K, V, H>>, BuildError> {
    let mut spawner = self.spawner.take();
    if matches!(self.loader, Some(Loader::Async(_))) && spawner.is_none() {
      #[cfg(feature = "tokio")]
      {
        spawner = Some(Arc::new(crate::runtime::TokioSpawner::new()));
      }
      #[cfg(not(feature = "tokio"))]
      {
        return Err(BuildError::SpawnerRequired);
      }
    }

    let store = Arc::new(ShardedStore::new(self.shards, self.hasher.clone()));
    let metrics = Arc::new(Metrics::new());
    let cache_policy = self.cache_policy.take().unwrap_or_else(|| {
      // If the cache is unbounded, use a NullPolicy that does nothing.
      // Otherwise, default to TinyLfu which is designed for bounded caches.
      if self.capacity == u64::MAX {
        Arc::new(crate::policy::null::NullPolicy)
      } else {
        Arc::new(TinyLfuPolicy::new(self.capacity))
      }
    });

    let (notifier, notification_sender) = if let Some(listener) = &self.listener {
      let (notifier, sender) = Notifier::spawn(listener.clone());
      (Some(notifier), Some(sender))
    } else {
      (None, None)
    };

    let timer_wheel = if self.time_to_live.is_some() || self.time_to_idle.is_some() {
      let tick_duration = self
        .timer_wheel_tick_duration
        .unwrap_or(Duration::from_secs(1));
      let wheel_size = self.timer_wheel_size.unwrap_or(60);

      Some(Arc::new(TimerWheel::new(
        wheel_size,
        tick_duration,
      )))
    } else {
      None
    };

    // --- Populate from Snapshot if it exists ---
    if let Some(snap) = snapshot {
      let now_duration = time::now_duration();
      let mut total_cost = 0;
      // Group entries by shard to minimize locking.
      let mut entries_by_shard: Vec<HashMap<K, Arc<crate::entry::CacheEntry<V>>, H>> = (0..self
        .shards)
        .map(|_| HashMap::with_hasher(self.hasher.clone()))
        .collect();

      for p_entry in snap.entries {
        let expires_at = p_entry.ttl_remaining.map(|ttl| now_duration + ttl);
        let entry = crate::entry::CacheEntry::new_with_expiry(
          p_entry.value,
          p_entry.cost,
          expires_at,
          self.time_to_idle,
        );
        total_cost += p_entry.cost;

        let hash = hash_key(&self.hasher, &p_entry.key);
        let index = hash as usize % self.shards;
        entries_by_shard[index].insert(p_entry.key, Arc::new(entry));
      }
      metrics.current_cost.store(total_cost, Ordering::Relaxed);

      // Populate the store shard by shard.
      for (i, entries) in entries_by_shard.into_iter().enumerate() {
        if !entries.is_empty() {
          let mut guard = store.shards[i].map.write();
          *guard = entries;
        }
      }
    }

    let janitor_context = JanitorContext {
      store: Arc::clone(&store),
      metrics: Arc::clone(&metrics),
      eviction_policy: Arc::clone(&cache_policy),
      timer_wheel: timer_wheel.as_ref().map(Clone::clone),
      capacity: self.capacity,
      time_to_idle: self.time_to_idle,
      notification_sender: notification_sender.as_ref().map(|val| val.clone()),
    };

    let janitor =
      if self.time_to_live.is_some() || self.time_to_idle.is_some() || self.capacity != u64::MAX {
        let tick_interval = self.janitor_tick_interval.unwrap_or(Duration::from_secs(1));
        Some(Janitor::spawn(janitor_context, tick_interval))
      } else {
        None
      };

    Ok(Arc::new(CacheShared {
      store,
      metrics,
      eviction_policy: cache_policy,
      timer_wheel,
      janitor,
      capacity: self.capacity,
      time_to_live: self.time_to_live,
      time_to_idle: self.time_to_idle,
      stale_while_revalidate: self.stale_while_revalidate,
      notification_sender,
      notifier,
      loader: self.loader.take(),
      pending_loads: crate::sync::HybridMutex::new(Default::default()),
      spawner,
    }))
  }

  /// Validates the builder configuration.
  pub(crate) fn validate(&self) -> Result<(), BuildError> {
    if self.capacity == 0 {
      return Err(BuildError::ZeroCapacity);
    }
    if self.shards == 0 {
      return Err(BuildError::ZeroShards);
    }
    Ok(())
  }
}
