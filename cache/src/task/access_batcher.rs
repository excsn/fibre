// cache/src/task/access_batcher.rs
use crate::store::hash_key;
use crate::sync::HybridMutex;
use std::collections::HashMap;
use std::hash::{BuildHasher, Hash};
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

  /// Records an access event. Called by many producer threads from the `get` hot path.
  #[inline]
  pub(crate) fn record_access<H: BuildHasher>(&self, key: &K, cost: u64, hasher: &H)
  where
    K: Clone,
  {
    // 1. Instantly find the active buffer set. This is a single atomic load.
    let idx = self.active_idx.load(Ordering::Relaxed);
    let stripes = &self.instances[idx];

    // 2. Hash the key to pick a stripe, minimizing contention.
    let hash = hash_key(hasher, key);
    let stripe_idx = hash as usize & (BATCH_STRIPES - 1);

    // 3. Lock only that single stripe and insert the data.
    let mut guard = stripes[stripe_idx].lock();
    guard.insert(key.clone(), cost);
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
      let mut guard = stripe_mutex.lock();
      // `take()` is efficient, leaving an empty map behind for the next round.
      if !guard.is_empty() {
        final_batch.extend(std::mem::take(&mut *guard));
      }
    }
    final_batch
  }
}
