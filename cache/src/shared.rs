use crate::backpressure::CostGate;
use crate::policy::CachePolicy;
use crate::metrics::Metrics;
use crate::store::ShardedStore;
use crate::task::janitor::Janitor;
use crate::task::notifier::{Notification, Notifier};

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use fibre::mpsc;

/// The internal, thread-safe core of the cache.
///
/// This struct is wrapped in an `Arc` and shared between the cache handles
/// (`Cache`, `AsyncCache`) and any background tasks.
pub(crate) struct CacheShared<K: Send, V: Send, H> {
  // These are the primary owners of the state.
  pub(crate) store: Arc<ShardedStore<K, V, H>>,
  pub(crate) metrics: Arc<Metrics>,

  // Components for managing the cache logic
  pub(crate) cost_gate: Arc<CostGate>,
  pub(crate) eviction_policy: Arc<dyn CachePolicy<K, V>>,

  pub(crate) janitor: Option<Janitor>,

  pub(crate) notification_sender: Option<mpsc::BoundedSender<Notification<K, V>>>,
  pub(crate) notifier: Option<Notifier>,

  // Configuration from the builder.
  pub(crate) capacity: u64,
  pub(crate) time_to_live: Option<Duration>,
  pub(crate) time_to_idle: Option<Duration>,
}

impl<K: Send, V: Send, H> fmt::Debug for CacheShared<K, V, H> {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("CacheShared")
      .field("capacity", &self.capacity)
      .field("cost_gate", &self.cost_gate)
      .field("time_to_live", &self.time_to_live)
      .field("time_to_idle", &self.time_to_idle)
      .field("metrics", &self.metrics.snapshot())
      .finish_non_exhaustive()
  }
}

impl<K: Send, V: Send, H> Drop for CacheShared<K, V, H> {
  fn drop(&mut self) {
    if let Some(janitor) = self.janitor.take() {
      janitor.stop();
    }

    if let Some(notifier) = self.notifier.take() {
      notifier.stop();
    }
  }
}
