use crate::backpressure::CostGate;
use crate::error::BuildError;
use crate::policy::tinylfu::TinyLfu;
use crate::policy::CachePolicy;
use crate::handles::{AsyncCache, Cache};
use crate::metrics::Metrics;
use crate::shared::CacheShared;
use crate::snapshot::CacheSnapshot;
use crate::store::ShardedStore;
use crate::task::janitor::{Janitor, JanitorContext};
use crate::task::notifier::Notifier;
use crate::{time, EvictionListener};

use core::fmt;
use std::hash::{BuildHasher, Hash};
use std::marker::PhantomData;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

/// A builder for creating `Cache` and `AsyncCache` instances.
#[derive(Clone)]
pub struct CacheBuilder<K: Send, V: Send, H = ahash::RandomState> {
  pub(crate) capacity: u64,
  pub(crate) shards: usize,
  pub(crate) time_to_live: Option<Duration>,
  pub(crate) time_to_idle: Option<Duration>,
  pub(crate) hasher: H,
  listener: Option<Arc<dyn EvictionListener<K, V>>>,
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
    self.shards = shards;
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
  pub fn eviction_listener(mut self, listener: Arc<dyn EvictionListener<K, V>>) -> Self {
    self.listener = Some(listener);
    self
  }
}

// --- Default Constructor ---
impl<K: Send, V: Send> CacheBuilder<K, V, ahash::RandomState> {
  /// Creates a new `CacheBuilder` with default settings.
  pub fn new() -> Self {
    Self {
      capacity: u64::MAX,
      shards: (num_cpus::get() * 4).max(1),
      time_to_live: None,
      time_to_idle: None,
      hasher: ahash::RandomState::new(),
      listener: None,
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

  /// Builds a synchronous `Cache`.
  pub fn build(&self) -> Result<Cache<K, V, H>, BuildError> {
    self.validate()?;
    let shared = self.build_shared_core(None);
    Ok(Cache { shared })
  }

  /// Builds an asynchronous `AsyncCache`.
  pub fn build_async(&self) -> Result<AsyncCache<K, V, H>, BuildError> {
    self.validate()?;
    let shared = self.build_shared_core(None);
    Ok(AsyncCache { shared })
  }

  /// Central logic to construct the shared core of the cache.
  pub(crate) fn build_shared_core(
    &self,
    snapshot: Option<CacheSnapshot<K, V>>,
  ) -> Arc<CacheShared<K, V, H>> {
    let store = Arc::new(ShardedStore::new(self.shards, self.hasher.clone()));
    let metrics = Arc::new(Metrics::new());
    let cost_gate = Arc::new(CostGate::new(self.capacity));
    let eviction_policy: Arc<dyn CachePolicy<K, V>> = Arc::new(TinyLfu::new(self.capacity));

    let (notifier, notification_sender) = if let Some(listener) = &self.listener {
      let (notifier, sender) = Notifier::spawn(listener.clone());
      (Some(notifier), Some(sender))
    } else {
      (None, None)
    };

    // --- Populate from Snapshot if it exists ---
    if let Some(snap) = snapshot {
      let now_duration = time::now_duration();
      for p_entry in snap.entries {
        let expires_at = p_entry.ttl_remaining.map(|ttl| now_duration + ttl);

        // Directly insert into the store. This bypasses admission/eviction
        // policies, which is correct for a cold start from a trusted source.
        let entry = crate::entry::CacheEntry::new_with_expiry(
          p_entry.value,
          p_entry.cost,
          expires_at,
          self.time_to_idle,
        );
        let shard = store.get_shard(&p_entry.key);
        shard.write_sync().insert(p_entry.key, Arc::new(entry));
        metrics
          .current_cost
          .fetch_add(p_entry.cost, Ordering::Relaxed);
      }
    }

    let janitor_context = JanitorContext {
      store: Arc::clone(&store),
      metrics: Arc::clone(&metrics),
      cost_gate: Arc::clone(&cost_gate),
      eviction_policy: Arc::clone(&eviction_policy),
      capacity: self.capacity,
      time_to_idle: self.time_to_idle,
      notification_sender: notification_sender.as_ref().map(|val| val.clone()),
    };

    let janitor =
      if self.time_to_live.is_some() || self.time_to_idle.is_some() || self.capacity != u64::MAX {
        let tick_interval = Duration::from_secs(1);
        Some(Janitor::spawn(janitor_context, tick_interval))
      } else {
        None
      };

    Arc::new(CacheShared {
      store,
      metrics,
      cost_gate,
      eviction_policy,
      janitor,
      capacity: self.capacity,
      time_to_live: self.time_to_live,
      time_to_idle: self.time_to_idle,
      notification_sender,
      notifier,
    })
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
