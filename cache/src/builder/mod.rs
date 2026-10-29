/// A set of presets for configuring the frequency of opportunistic maintenance.
///
/// These values are used with the [`CacheBuilder::maintenance_chance()`] method.
pub mod maintenance_frequency;

use crate::error::BuildError;
use crate::handles::{AsyncCache, Cache};
use crate::loader::Loader;
use crate::metrics::Metrics;
use crate::policy::CachePolicy;
use crate::shared::CacheShared;
#[cfg(feature = "serde")]
use crate::snapshot::CacheSnapshot;
use crate::store::{hash_key, ShardedStore};
use crate::task::janitor::{Janitor, JanitorContext};
use crate::task::notifier::Notifier;
use crate::{time, EvictionListener, TaskSpawner};

use core::fmt;
use std::future::Future;
use std::hash::{BuildHasher, Hash};
use std::marker::PhantomData;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

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
  policy_factory: Option<Arc<dyn Fn() -> Box<dyn CachePolicy<K, V>> + Send + Sync>>,
  builtin_choice: crate::select::BuiltinChoice,
  loader: Option<Loader<K, V>>,
  spawner: Option<Arc<dyn TaskSpawner>>,
  maintenance_probability_denominator: u32,
  maintenance_on_introspection: bool,
  shards_set: bool,
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
    self.shards_set = true;
    self
  }

  /// Sets a time-to-live (TTL) for all entries in the cache.
  ///
  /// Reads check expiry against a clock the janitor refreshes once per tick
  /// (1 s by default), so `get`, `fetch` and `peek` can return an entry for up
  /// to one tick past its deadline. `iter()` and snapshots use the exact clock.
  /// See [`janitor_tick_interval`](Self::janitor_tick_interval).
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

  /// Sets the eviction policy for the cache using a factory function.
  ///
  /// The provided factory closure will be called once for each cache shard.
  /// It must return a `Box<dyn CachePolicy<...>>`, signifying that the cache
  /// takes ownership of the created policy instance. This is the most flexible
  /// and safest way to configure a custom policy.
  ///
  /// # Example
  /// ```ignore
  /// let cache = CacheBuilder::new()
  ///     .capacity(1000)
  ///     .shards(4)
  ///     .cache_policy_factory(|| {
  ///         // This closure creates a new LruPolicy for each shard.
  ///         Box::new(LruPolicy::new())
  ///     })
  ///     .build();
  /// ```
  pub fn cache_policy_factory<F>(mut self, factory: F) -> Self
  where
    F: Fn() -> Box<dyn CachePolicy<K, V>> + Send + Sync + 'static,
  {
    self.policy_factory = Some(Arc::new(factory));
    self
  }

  /// Sets the eviction policy to S3-FIFO, run inside each shard. This is the
  /// default for a bounded cache; the call replaces any `cache_policy_factory`
  /// or `tinylfu` set before it.
  pub fn s3fifo(mut self) -> Self {
    self.policy_factory = None;
    self.builtin_choice = crate::select::BuiltinChoice::S3Fifo;
    self
  }

  /// Sets the eviction policy to W-TinyLFU, run inside each shard. Replaces any
  /// `cache_policy_factory` or `s3fifo` set before it.
  pub fn tinylfu(mut self) -> Self {
    self.policy_factory = None;
    self.builtin_choice = crate::select::BuiltinChoice::TinyLfu;
    self
  }

  /// Sets the eviction policy to the no-op `NullPolicy`, which never evicts.
  pub fn null_policy(self) -> Self
  where
    K: Sync,
    V: Sync,
  {
    self.cache_policy_factory(|| Box::new(crate::policy::null::NullPolicy))
  }

  /// Sets the synchronous loader function for the cache.
  ///
  /// The provided closure is called by `fetch_with` when a key is not present.
  /// It must return a tuple of `(value, cost)`.
  pub fn loader(mut self, f: impl Fn(K) -> (V, u64) + Send + Sync + 'static) -> Self {
    self.loader = Some(Loader::Sync(Arc::new(f)));
    self
  }

  /// Sets the asynchronous loader function for the cache.
  ///
  /// The provided closure is called by `fetch_with` when a key is not present.
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

  /// Sets how often the janitor wakes to refresh the clock reads check expiry
  /// against and to run cleanup. Defaults to 1 s.
  ///
  /// Reads can return an entry for up to one tick past its TTL or TTI
  /// deadline. A shorter tick tightens that bound at the cost of more frequent
  /// background work.
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

  /// Sets the probability of running opportunistic maintenance on each `insert`.
  ///
  /// Maintenance is run with a chance of `1 / denominator`. For example, a
  /// value of `16` means there is a 1-in-16 chance that any given `insert`
  /// operation will trigger a maintenance cycle for its shard.
  ///
  /// The default is `16`, which is suitable for most workloads. For convenience,
  /// this crate provides presets in the [`maintenance_frequency`] module, such as
  /// [`maintenance_frequency::RESPONSIVE`] (the default) and
  /// [`maintenance_frequency::THROUGHPUT`].
  ///
  /// A lower value (e.g., `RESPONSIVE`) increases the frequency of maintenance,
  /// keeping memory usage smoother. A higher value (e.g., `THROUGHPUT`) reduces
  /// the CPU overhead of maintenance checks, which can be beneficial in
  /// extremely write-heavy workloads.
  ///
  /// The provided denominator will be rounded up to the next power of two for
  /// efficient internal calculations.
  ///
  /// # Panics
  ///
  /// Panics if `denominator` is `0`.
  ///
  /// # Example
  /// ```
  /// use fibre_cache::CacheBuilder;
  /// use fibre_cache::builder::maintenance_frequency;
  ///
  /// // Configure the cache for maximum insert throughput.
  /// let cache = CacheBuilder::<u64, u64>::new()
  ///     .maintenance_chance(maintenance_frequency::THROUGHPUT)
  ///     .build();
  /// ```
  pub fn maintenance_chance(mut self, denominator: u32) -> Self {
    assert!(
      denominator > 0,
      "maintenance chance denominator cannot be zero"
    );
    // Ensure the denominator is a power of two for efficient bitmasking.
    self.maintenance_probability_denominator = denominator.next_power_of_two();
    self
  }

  /// When enabled, the cache automatically runs a full maintenance pass before
  /// returning metrics, iterators, snapshots, or other introspection results.
  ///
  /// This ensures that pending write admissions and read access events are
  /// reflected in policy state before the caller observes cache internals.
  /// Particularly useful for deterministic testing.
  pub fn maintenance_on_introspection(mut self, enabled: bool) -> Self {
    self.maintenance_on_introspection = enabled;
    self
  }
}

/// Smallest cost per shard the default shard count allows for a bounded cache.
pub const MIN_COST_PER_SHARD: u64 = 64;

// --- Default Constructor ---
impl<K: Send, V: Send, H: BuildHasher + Default> CacheBuilder<K, V, H> {
  /// Creates a new `CacheBuilder` with default settings.
  pub fn new() -> Self {
    Self {
      capacity: u64::MAX,
      shards: (num_cpus::get() * 8).max(1).next_power_of_two(),
      time_to_live: None,
      time_to_idle: None,
      hasher: H::default(),
      stale_while_revalidate: None,
      janitor_tick_interval: None,
      timer_wheel_tick_duration: None, // Will default to 1s if not set
      timer_wheel_size: None,          // Will default to 60 if not set
      listener: None,
      policy_factory: None,
      builtin_choice: crate::select::BuiltinChoice::S3Fifo,
      loader: None,
      spawner: None,
      maintenance_probability_denominator: maintenance_frequency::RESPONSIVE,
      maintenance_on_introspection: false,
      shards_set: false,
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

#[cfg(feature = "rapidhash")]
impl<K: Send, V: Send> CacheBuilder<K, V, rapidhash::RapidRandomState> {
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
    #[cfg(feature = "serde")]
    snapshot: Option<CacheSnapshot<K, V>>,
    #[cfg(not(feature = "serde"))]
    _snapshot: Option<()>,
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

    let has_timer_logic = self.time_to_live.is_some() || self.time_to_idle.is_some();
    let tick_duration = self
      .timer_wheel_tick_duration
      .unwrap_or(Duration::from_secs(1));
    let wheel_size = self.timer_wheel_size.unwrap_or(60);

    if !self.shards_set && self.capacity != u64::MAX {
      let max_shards = (self.capacity / MIN_COST_PER_SHARD).max(1);
      let max_shards = if max_shards.is_power_of_two() {
        max_shards
      } else {
        max_shards.next_power_of_two() / 2
      };
      self.shards = self.shards.min(max_shards as usize);
    }

    let metrics = Arc::new(Metrics::new(self.shards));
    // Create the per-shard eviction policies using the factory.
    let has_custom_policy = self.policy_factory.is_some();
    let builtin_capacity = (!has_custom_policy && self.capacity != u64::MAX)
      .then(|| (self.capacity as f64 / self.shards as f64).ceil() as u64);
    if builtin_capacity.is_some() {
      self.policy_factory = Some(Arc::new(|| Box::new(crate::policy::null::NullPolicy)));
    }
    let factory = self
      .policy_factory
      .take()
      .unwrap_or_else(|| Arc::new(|| Box::new(crate::policy::null::NullPolicy)));

    // Call the factory `N` times to create `N` independent policy instances.
    // The `Box` is converted to an `Arc` for internal sharing with the janitor.
    let cache_policy: Box<[Arc<dyn CachePolicy<K, V>>]> = (0..self.shards)
      .map(|_| Arc::from(factory()))
      .collect::<Vec<_>>()
      .into_boxed_slice();

    let track_reads = cache_policy.iter().any(|p| p.uses_access_events());
    let (notifier, notification_sender) = if let Some(listener) = &self.listener {
      let (notifier, sender) = Notifier::spawn(listener.clone());
      (Some(notifier), Some(sender))
    } else {
      (None, None)
    };
    let store = Arc::new(ShardedStore::new(
      self.shards,
      self.hasher.clone(),
      wheel_size,
      tick_duration,
      has_timer_logic,
      self.capacity,
      &cache_policy,
      builtin_capacity.map(|c| (self.builtin_choice, c)),
      Arc::new(crate::store::EvictCtx {
        notify: notification_sender.as_ref().map(|s| {
          let s = s.clone();
          Box::new(move |k: K, v: Arc<V>| {
            let _ = s.try_send((k, v, crate::listener::EvictionReason::Capacity));
          }) as Box<dyn Fn(K, Arc<V>) + Send + Sync>
        }),
      }),
    ));
    metrics.attach_shards(store.shards.iter().map(|s| s.counters.clone()).collect());



    // --- Populate from Snapshot if it exists ---
    #[cfg(feature = "serde")]
    if let Some(snap) = snapshot {
      let now_duration = time::now_duration();
      // Group entries by shard to minimize locking.
      let mut entries_by_shard: Vec<Vec<(K, crate::entry::CacheEntry<V>)>> = (0..self.shards).map(|_| Vec::new()).collect();

      for p_entry in snap.entries {
        let expires_at = p_entry.ttl_remaining.map(|ttl| now_duration + ttl);
        let mut entry = crate::entry::CacheEntry::new_with_expiry(
          p_entry.value,
          p_entry.cost,
          expires_at,
          self.time_to_idle,
        );

        let hash = hash_key(&self.hasher, &p_entry.key);
        entry.key_hash = hash;
        let index = hash as usize % self.shards;
        entries_by_shard[index].push((p_entry.key, entry));
      }

      // Populate the store shard by shard.
      for (i, entries) in entries_by_shard.into_iter().enumerate() {
        if !entries.is_empty() {
          let mut guard = store.shards[i].map.write();
          guard.replace_all(entries);
        }
      }
    }

    let clock = Arc::new(crate::time::CoarseClock::default());
    let janitor_context = JanitorContext {
      store: Arc::clone(&store),
      clock: Arc::clone(&clock),
      metrics: Arc::clone(&metrics),
      time_to_idle: self.time_to_idle,
      notification_sender: notification_sender.as_ref().map(|val| val.clone()),
    };

    let maintenance_probability_denominator = self.maintenance_probability_denominator;
    // The janitor also runs for custom policies on unbounded caches: async
    // paths never do maintenance inline (they signal instead), so a policy
    // that wants access/write events needs this thread to apply them.
    let needs_janitor = self.time_to_live.is_some()
      || self.time_to_idle.is_some()
      || self.capacity != u64::MAX
      || has_custom_policy;
    let (janitor, maintenance_signal) = if needs_janitor {
      let tick_interval = self.janitor_tick_interval.unwrap_or(Duration::from_secs(1));
      let (signal_tx, signal_rx) = std::sync::mpsc::sync_channel(self.shards.max(16));
      let janitor = Janitor::spawn(
        janitor_context,
        tick_interval,
        maintenance_probability_denominator,
        signal_rx,
      );
      (Some(janitor), Some(signal_tx))
    } else {
      (None, None)
    };

    let pending_loads = (0..self.shards)
      .map(|_| crate::sync::HybridMutex::new(Default::default()))
      .collect::<Vec<_>>()
      .into_boxed_slice();

    Ok(Arc::new(CacheShared {
      store,
      clock,
      metrics,
      janitor,
      capacity: self.capacity,
      time_to_live: self.time_to_live,
      time_to_idle: self.time_to_idle,
      stale_while_revalidate: self.stale_while_revalidate,
      notification_sender,
      notifier,
      loader: self.loader.take(),
      pending_loads,
      spawner,
      maintenance_probability_denominator,
      maintenance_on_introspection: self.maintenance_on_introspection,
      track_reads,
      builtin_reads: match builtin_capacity {
        None => crate::shared::BuiltinReads::None,
        Some(_) => crate::select::reads(self.builtin_choice),
      },
      maintenance_signal,
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
