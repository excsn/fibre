// cache/src/task/access_batcher.rs
use crate::sync::HybridMutex;
use ahash::{HashMap, HashMapExt};
use std::hash::Hash;
use std::sync::atomic::{AtomicUsize, Ordering};

const BATCH_STRIPES: usize = 16; // Power of two for efficient bitmasking.

/// A self-contained, Left-Right striped batching mechanism for access events.
///
/// This structure allows many producer threads to record access events with
/// very low contention, and a single consumer thread to drain all events
/// in a non-blocking manner relative to the producers.
pub(crate) struct AccessBatcher<K> {
  active_idx: AtomicUsize,
  // Two independent sets of striped, coalescing buffers.
  instances: [Box<[HybridMutex<HashMap<K, u64>>]>; 2],
}

impl<K: Hash + Eq> AccessBatcher<K> {
  pub(crate) fn new() -> Self {
    let create_instance = || -> Box<[HybridMutex<HashMap<K, u64>>]> {
      (0..BATCH_STRIPES)
        .map(|_| HybridMutex::new(HashMap::new()))
        .collect()
    };
    Self {
      active_idx: AtomicUsize::new(0),
      instances: [create_instance(), create_instance()],
    }
  }

  /// Records an access event, blocking on the stripe lock. Called from the
  /// sync `get` hot path. Accepts a pre-computed hash and only clones the
  /// key if it is not already batched.
  #[inline]
  pub(crate) fn record_access(&self, key: &K, hash: u64, cost: u64)
  where
    K: Clone,
  {
    // 1. Find the active buffer set via a single atomic load.
    let idx = self.active_idx.load(Ordering::Relaxed);
    let stripes = &self.instances[idx];

    // 2. Use the pre-computed hash to select the correct stripe. The high bits
    //    are used because the low bits select the shard and are therefore
    //    constant for every key that reaches this batcher.
    let stripe_idx = (hash >> 32) as usize & (BATCH_STRIPES - 1);

    // 3. Lock only that single stripe and insert the key only if not already present.
    let mut guard = stripes[stripe_idx].lock();
    if !guard.contains_key(key) {
      guard.insert(key.clone(), cost);
    }
  }

  /// Non-blocking attempt to record an access event. Returns `false` if the
  /// stripe lock was contended and the event was NOT recorded - the caller
  /// must then fall back to [`record_access_async`]. Used from async hit
  /// paths while the shard guard is held (so no awaiting is possible here).
  #[inline]
  pub(crate) fn try_record_access(&self, key: &K, hash: u64, cost: u64) -> bool
  where
    K: Clone,
  {
    let idx = self.active_idx.load(Ordering::Relaxed);
    let stripes = &self.instances[idx];
    let stripe_idx = (hash >> 32) as usize & (BATCH_STRIPES - 1);

    match stripes[stripe_idx].try_lock() {
      Some(mut guard) => {
        if !guard.contains_key(key) {
          guard.insert(key.clone(), cost);
        }
        true
      }
      None => false,
    }
  }

  /// Waker-based recording for async hit paths, used as the fallback when
  /// [`try_record_access`] hit stripe contention. Takes the key by value:
  /// the caller clones it before releasing its shard guard, so nothing is
  /// borrowed across the await.
  pub(crate) async fn record_access_async(&self, key: K, hash: u64, cost: u64) {
    let idx = self.active_idx.load(Ordering::Relaxed);
    let stripes = &self.instances[idx];
    let stripe_idx = (hash >> 32) as usize & (BATCH_STRIPES - 1);

    let mut guard = stripes[stripe_idx].lock_async().await;
    if !guard.contains_key(&key) {
      guard.insert(key, cost);
    }
  }

  /// Swaps buffers and returns the entire coalesced, inactive batch.
  /// Called by the single consumer thread (the janitor).
  pub(crate) fn drain(&self) -> HashMap<K, u64> {
    // 1. Atomically flip the switch. New producers now write to the other instance.
    // This is non-blocking for producers.
    let inactive_idx = self.active_idx.swap(1, Ordering::AcqRel);
    let active_idx = 1 - inactive_idx;
    self.active_idx.store(active_idx, Ordering::Release);

    let inactive_stripes = &self.instances[inactive_idx];
    let mut final_batch = HashMap::new();

    // 2. We now have exclusive logical access to the inactive stripes. Drain them.
    for stripe_mutex in inactive_stripes.iter() {
      // Scope the lock to release it before extending final_batch, letting producers
      // unblock as soon as the stripe is drained. drain() preserves bucket capacity.
      let drained = {
        let mut guard = stripe_mutex.lock();
        guard.drain().collect::<Vec<_>>()
      };
      final_batch.extend(drained);
    }
    final_batch
  }
}
