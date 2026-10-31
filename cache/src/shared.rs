use crate::entry::CacheEntry;
use crate::loader::{LoadFuture, Loader};
use crate::metrics::Metrics;
use crate::store::ShardedStore;
use crate::sync::HybridMutex;
use crate::task::janitor::Janitor;
use crate::task::notifier::{Notification, Notifier};
use crate::TaskSpawner;

use std::fmt;
use std::hash::{BuildHasher, Hash};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicUsize, Ordering};
use std::time::Duration;

use ahash::HashMap;
use fibre::{mpmc, mpsc};
use futures_util::FutureExt;


/// Stale refreshes waiting to run, first in first out, at most one per key since
/// a key with a pending load is not queued again.
pub(crate) struct RefreshQueue<K: Send, V: Send + Sync> {
  tx: mpmc::UnboundedSyncSender<(K, Arc<LoadFuture<V>>)>,
  rx: mpmc::UnboundedSyncReceiver<(K, Arc<LoadFuture<V>>)>,
  limit: usize,
  running: AtomicUsize,
  /// Sent and not yet received. Can dip below zero while a send is being counted.
  waiting: AtomicIsize,
  /// One thread starts refreshes at a time, so a spawner that drops its task on
  /// the spot does not recurse through the queue.
  pumping: AtomicBool,
}

impl<K: Send, V: Send + Sync> RefreshQueue<K, V> {
  pub(crate) fn new(limit: usize) -> Self {
    let (tx, rx) = mpmc::unbounded();
    Self { tx, rx, limit, running: AtomicUsize::new(0), waiting: AtomicIsize::new(0), pumping: AtomicBool::new(false) }
  }

  fn claim_slot(&self) -> bool {
    self.running.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| (n < self.limit).then_some(n + 1)).is_ok()
  }

  fn more_can_start(&self) -> bool {
    self.waiting.load(Ordering::SeqCst) > 0 && self.running.load(Ordering::SeqCst) < self.limit
  }
}

/// Held by an async load's task. Dropped with the task, including a task a dead
/// runtime dropped unpolled, so the load's waiters and refresh slot are not lost.
struct AsyncLoadGuard<K: Eq + Hash + Send, V: Send + Sync, H> {
  shared: Arc<CacheShared<K, V, H>>,
  key: K,
  index: usize,
  future: Arc<LoadFuture<V>>,
  /// Hands back a refresh's slot and starts the next one.
  refresh_done: Option<Box<dyn FnOnce() + Send>>,
  finished: bool,
}

impl<K: Eq + Hash + Send, V: Send + Sync, H> AsyncLoadGuard<K, V, H> {
  fn finish(&mut self) {
    self.finished = true;
  }
}

impl<K: Eq + Hash + Send, V: Send + Sync, H> Drop for AsyncLoadGuard<K, V, H> {
  fn drop(&mut self) {
    if !self.finished {
      self.future.drop_load();
      // A held lock leaves the entry for the next miss on the key to replace.
      if let Some(mut pending) = self.shared.pending_loads[self.index].try_lock() {
        if pending.get(&self.key).is_some_and(|load| Arc::ptr_eq(load, &self.future)) {
          pending.remove(&self.key);
        }
      }
    }
    if let Some(refresh_done) = self.refresh_done.take() {
      refresh_done();
    }
  }
}

/// The janitor or how to start it, for a cache that had no need of it when built.
pub(crate) enum JanitorState {
  Unstarted(Box<dyn FnOnce() -> Janitor + Send>),
  Running(Janitor),
  Stopped,
}

/// The internal, thread-safe core of the cache.
pub(crate) struct CacheShared<K: Send, V: Send + Sync, H> {
  pub(crate) store: Arc<ShardedStore<K, V, H>>,
  pub(crate) clock: Arc<crate::time::CoarseClock>,
  pub(crate) metrics: Arc<Metrics>,
  pub(crate) janitor: HybridMutex<JanitorState>,
  pub(crate) janitor_running: AtomicBool,
  pub(crate) notification_sender: Option<mpsc::BoundedSyncSender<Notification<K, V>>>,
  pub(crate) notifier: Option<Notifier<K, V>>,
  pub(crate) capacity: u64,
  pub(crate) time_to_live: Option<Duration>,
  pub(crate) time_to_idle: Option<Duration>,
  pub(crate) stale_while_revalidate: Option<Duration>,
  pub(crate) loader: Option<Loader<K, V>>,
  pub(crate) spawner: Option<Arc<dyn TaskSpawner>>,
  pub(crate) pending_loads: Box<[HybridMutex<HashMap<K, Arc<LoadFuture<V>>>>]>,
  pub(crate) load_pool: crate::task::load_pool::LoadPool,
  pub(crate) refreshes: RefreshQueue<K, V>,
  pub(crate) maintenance_on_introspection: bool,
  // False when no shard policy uses access events, letting reads skip the
  // access batcher entirely.
  pub(crate) track_reads: bool,
  pub(crate) builtin_reads: BuiltinReads,
}

impl<K: Send, V: Send + Sync, H> fmt::Debug for CacheShared<K, V, H> {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("CacheShared")
      .field("capacity", &self.capacity)
      .field("time_to_live", &self.time_to_live)
      .field("time_to_idle", &self.time_to_idle)
      .field("metrics", &self.metrics.snapshot())
      .finish_non_exhaustive()
  }
}

impl<K: Send, V: Send + Sync, H> Drop for CacheShared<K, V, H> {
  fn drop(&mut self) {
    if let JanitorState::Running(janitor) = std::mem::replace(self.janitor.get_mut(), JanitorState::Stopped) {
      janitor.stop();
    }
    if let Some(notifier) = self.notifier.take() {
      notifier.stop();
    }
  }
}

impl<K, V, H> CacheShared<K, V, H>
where
  K: Eq + Hash + Clone + Send,
  V: Send + Sync,
  H: BuildHasher + Clone,
{
  /// Flushes pending read-access events for all shards.
  /// Called by introspection methods when `maintenance_on_introspection` is enabled.
  pub(crate) fn flush_for_introspection(&self) {
    if !self.maintenance_on_introspection {
      return;
    }
    for shard in self.store.shards.iter() {
      let _guard = shard.maintenance_lock.lock();
      crate::task::janitor::perform_shard_maintenance(shard);
    }
  }

  /// The flush for a synchronous call on an async handle. A shard whose lock is
  /// held is skipped, since waiting would stop the runtime thread the holder
  /// may need; its reads reach the policy on its next write.
  pub(crate) fn try_flush_for_introspection(&self) {
    if !self.maintenance_on_introspection {
      return;
    }
    for shard in self.store.shards.iter() {
      if let Some(_guard) = shard.maintenance_lock.try_lock() {
        if let Some(mut map) = shard.map.try_write() {
          map.apply_reads();
        }
      }
    }
  }

  pub(crate) async fn flush_for_introspection_async(&self) {
    if !self.maintenance_on_introspection {
      return;
    }
    for shard in self.store.shards.iter() {
      let _guard = shard.maintenance_lock.lock_async().await;
      shard.map.write_async().await.apply_reads();
    }
  }
}

impl<K: Send, V: Send + Sync, H> CacheShared<K, V, H> {

  /// Starts the janitor if it is not running. Never waits, so an async path may
  /// call it: a caller that finds the lock held leaves the start to its holder.
  #[inline]
  pub(crate) fn ensure_janitor(&self) {
    if !self.janitor_running.load(Ordering::Relaxed) {
      self.start_janitor();
    }
  }

  #[cold]
  fn start_janitor(&self) {
    if let Some(mut state) = self.janitor.try_lock() {
      if let JanitorState::Unstarted(start) = std::mem::replace(&mut *state, JanitorState::Stopped) {
        *state = JanitorState::Running(start());
      }
      self.janitor_running.store(true, Ordering::Relaxed);
    }
  }

  #[inline]
  pub fn get_shard_index_from_hash(&self, hash: u64) -> usize {
    hash as usize & (self.store.shards.len() - 1)
  }

  /// Runs a sync loader on this thread and finishes `future` with its result.
  pub(crate) fn run_sync_load(
    &self,
    loader: &(dyn Fn(K) -> (V, u64) + Send + Sync),
    key: K,
    future: &LoadFuture<V>,
  ) -> std::thread::Result<Arc<V>>
  where
    K: Eq + Hash + Clone + Send + Sync,
    V: Send + Sync,
    H: BuildHasher + Clone,
  {
    let hash = crate::store::hash_key(&self.store.hasher, &key);
    let index = hash as usize & (self.pending_loads.len() - 1);
    let loaded = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| loader(key.clone())));
    match loaded {
      Ok((value, cost)) => {
        let entry = CacheEntry::new(value, cost, self.time_to_live, self.time_to_idle);
        let value = entry.value();
        let _replaced = self.store.get_shard(&key).map.write().insert(key.clone(), entry);
        self.pending_loads[index].lock().remove(&key);
        future.complete(value.clone());
        Ok(value)
      }
      Err(panic) => {
        self.pending_loads[index].lock().remove(&key);
        future.fail();
        Err(panic)
      }
    }
  }

  /// Queues a stale refresh. Never waits, so an async path may call it.
  pub(crate) fn enqueue_refresh(shared: &Arc<Self>, key: K, future: Arc<LoadFuture<V>>)
  where
    K: Eq + Hash + Clone + Send + Sync + 'static,
    V: Send + Sync + 'static,
    H: BuildHasher + Clone + Send + Sync + 'static,
  {
    if shared.refreshes.tx.clone().send((key, future)).is_ok() {
      shared.refreshes.waiting.fetch_add(1, Ordering::SeqCst);
      Self::start_refreshes(shared);
    }
  }

  fn refresh_done(shared: &Arc<Self>)
  where
    K: Eq + Hash + Clone + Send + Sync + 'static,
    V: Send + Sync + 'static,
    H: BuildHasher + Clone + Send + Sync + 'static,
  {
    shared.refreshes.running.fetch_sub(1, Ordering::SeqCst);
    Self::start_refreshes(shared);
  }

  fn start_refreshes(shared: &Arc<Self>)
  where
    K: Eq + Hash + Clone + Send + Sync + 'static,
    V: Send + Sync + 'static,
    H: BuildHasher + Clone + Send + Sync + 'static,
  {
    let queue = &shared.refreshes;
    loop {
      if queue.pumping.swap(true, Ordering::SeqCst) {
        return;
      }
      while queue.claim_slot() {
        let Ok((key, future)) = queue.rx.try_recv() else {
          queue.running.fetch_sub(1, Ordering::SeqCst);
          break;
        };
        queue.waiting.fetch_sub(1, Ordering::SeqCst);
        if future.claim() {
          Self::spawn_loader_task(Arc::clone(shared), key, future, true);
        } else {
          queue.running.fetch_sub(1, Ordering::SeqCst);
        }
      }
      queue.pumping.store(false, Ordering::SeqCst);
      // A caller that found `pumping` set left its work to this check.
      if !queue.more_can_start() {
        return;
      }
    }
  }

  /// Starts a load off the caller's thread. `refresh` frees a refresh slot when it ends.
  pub(crate) fn spawn_loader_task(shared: Arc<Self>, key: K, future: Arc<LoadFuture<V>>, refresh: bool)
  where
    K: Eq + Hash + Clone + Send + Sync + 'static,
    V: Send + Sync + 'static,
    H: BuildHasher + Clone + Send + Sync + 'static,
  {
    let loader = shared.loader.as_ref().expect("caller checked for a loader").clone();
    match loader {
      Loader::Sync(sync_loader) => {
        let job_shared = shared.clone();
        shared.load_pool.submit(Box::new(move || {
          let _ = job_shared.run_sync_load(&*sync_loader, key, &future);
          if refresh {
            Self::refresh_done(&job_shared);
          }
        }));
      }
      Loader::Async(async_loader) => {
        let spawner = shared
          .spawner
          .as_ref()
          .expect("Spawner must exist for async loader");
        let hash = crate::store::hash_key(&shared.store.hasher, &key);
        let index = hash as usize & (shared.pending_loads.len() - 1);
        let refresh_done = refresh.then(|| {
          let shared = shared.clone();
          Box::new(move || Self::refresh_done(&shared)) as Box<dyn FnOnce() + Send>
        });
        let mut guard = AsyncLoadGuard { shared: shared.clone(), key: key.clone(), index, future: future.clone(), refresh_done, finished: false };
        let task = async move {
          let shared = guard.shared.clone();
          let load_key = key.clone();
          let loaded = std::panic::AssertUnwindSafe(async move { async_loader(load_key).await }).catch_unwind().await;
          match loaded {
            Ok((value, cost)) => {
              let entry = CacheEntry::new(value, cost, shared.time_to_live, shared.time_to_idle);
              let value = entry.value();
              let _replaced = shared.store.get_shard(&key).map.write_async().await.insert(key.clone(), entry);
              shared.pending_loads[index].lock_async().await.remove(&key);
              future.complete_async(value).await;
            }
            Err(_) => {
              shared.pending_loads[index].lock_async().await.remove(&key);
              future.fail_async().await;
            }
          }
          guard.finish();
        };
        spawner.spawn(Box::pin(task));
      }
    }
  }
}

/// How a hit reaches the shard's built-in policy, if there is one.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum BuiltinReads {
  None,
  Ring,
  Freq,
}
