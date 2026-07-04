// channels/src/internal/left_right.rs

//! A self-contained, high-performance, and correct implementation of a copy-free,
//! single-writer, multi-reader synchronization primitive.
//!
//! Dual buffers are kept in sync by applying deterministic mutations twice,
//! eliminating the need to copy or clone data. Read operations are entirely
//! lock-free and protected from false sharing via cache-padding.
//!
//! ## Correctness: Dekker's Store-Load Race
//!
//! The reader path uses a store (`fetch_add`) followed by a load (`live_idx`).
//! Without sequential consistency, CPUs may reorder these, allowing a writer to
//! observe zero active readers and modify a slot that a reader is already inside.
//! All atomic operations on the critical reader/writer path therefore use
//! `Ordering::SeqCst` to form a total order over these events.
//!
//! ## Performance: Cache-Line Padding
//!
//! `live_idx` and each element of `active_readers` are wrapped in `CachePadded`
//! to ensure they occupy distinct cache lines. Without this, readers on different
//! cores would bounce the same cache line when incrementing their respective
//! per-slot counters, serialising what should be a parallel operation.
//!
//! ## Write Model: Double Execution
//!
//! Rather than cloning the live buffer into the stale slot on every write (O(N)),
//! the writer applies the same `FnMut` closure twice — once to each slot —
//! keeping both in sync at O(1) cost relative to data size.
//! **Requirement:** the closure must be deterministic and produce the same result
//! when applied to either slot (both slots are always in the same logical state
//! at the start of a `modify` call, which is the invariant preserved by the
//! double-execution strategy).

use crate::internal::cache_padded::CachePadded;
use core::fmt;
use std::cell::UnsafeCell;
use std::ops::Deref;

use crate::internal::sync::{hint, Arc, AtomicUsize, Mutex, Ordering};

/// Shared state: dual buffers, a padded live index, and per-slot padded reader counts.
struct LeftRightShared<T> {
  live_idx: CachePadded<AtomicUsize>,
  active_readers: [CachePadded<AtomicUsize>; 2],
  data_0: UnsafeCell<T>,
  data_1: UnsafeCell<T>,
}

impl<T> fmt::Debug for LeftRightShared<T> {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("LeftRightShared")
      .field("live_idx", &self.live_idx.load(Ordering::Relaxed))
      .finish_non_exhaustive()
  }
}

// Safety: access is synchronised via SeqCst barriers and the writer lock.
unsafe impl<T: Send> Send for LeftRightShared<T> {}
unsafe impl<T: Send + Sync> Sync for LeftRightShared<T> {}

/// Handle for lock-free read operations. Can be cloned freely.
#[derive(Debug)]
pub(crate) struct ReadHandle<T> {
  shared: Arc<LeftRightShared<T>>,
}

impl<T> Clone for ReadHandle<T> {
  fn clone(&self) -> Self {
    Self {
      shared: self.shared.clone(),
    }
  }
}

/// Handle for write mutations. Serialised internally; only one logical writer.
#[derive(Debug)]
pub(crate) struct WriteHandle<T> {
  shared: Arc<LeftRightShared<T>>,
  writer_lock: Mutex<()>,
}

/// RAII guard providing a consistent read snapshot until dropped.
pub(crate) struct ReadGuard<T> {
  shared: Arc<LeftRightShared<T>>,
  idx: usize,
  data: *const T,
}

impl<T> fmt::Debug for ReadGuard<T> {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("ReadGuard")
      .field("idx", &self.idx)
      .finish_non_exhaustive()
  }
}

// Safety: ReadGuard<T> provides shared read access (&T), same as &T.
unsafe impl<T: Send + Sync> Send for ReadGuard<T> {}
unsafe impl<T: Sync> Sync for ReadGuard<T> {}

/// Creates a new left-right pair. T only needs `Default`; cloning is never required.
pub(crate) fn new<T: Default>() -> (ReadHandle<T>, WriteHandle<T>) {
  let shared = Arc::new(LeftRightShared {
    live_idx: CachePadded::new(AtomicUsize::new(0)),
    active_readers: [
      CachePadded::new(AtomicUsize::new(0)),
      CachePadded::new(AtomicUsize::new(0)),
    ],
    data_0: UnsafeCell::new(T::default()),
    data_1: UnsafeCell::new(T::default()),
  });
  let rh = ReadHandle {
    shared: shared.clone(),
  };
  let wh = WriteHandle {
    shared,
    writer_lock: Mutex::new(()),
  };
  (rh, wh)
}

impl<T> ReadHandle<T> {
  /// Returns a guard pinning a reader count on the live slot.
  ///
  /// SeqCst on the fetch_add and the subsequent live_idx re-check closes the
  /// Dekker store-load window: the writer's SeqCst fence in `modify` will
  /// observe our incremented counter before it drains readers.
  pub(crate) fn enter(&self) -> ReadGuard<T> {
    loop {
      let idx = self.shared.live_idx.load(Ordering::SeqCst);

      self.shared.active_readers[idx].fetch_add(1, Ordering::SeqCst);

      // If live_idx is unchanged we are committed to this slot.
      if self.shared.live_idx.load(Ordering::SeqCst) == idx {
        let data = if idx == 0 {
          self.shared.data_0.get() as *const T
        } else {
          self.shared.data_1.get() as *const T
        };
        return ReadGuard {
          shared: self.shared.clone(),
          idx,
          data,
        };
      }

      // Writer swapped live_idx between our load and the increment — back off.
      self.shared.active_readers[idx].fetch_sub(1, Ordering::SeqCst);
    }
  }
}

impl<T> Deref for ReadGuard<T> {
  type Target = T;
  fn deref(&self) -> &Self::Target {
    unsafe { &*self.data }
  }
}

impl<T> Drop for ReadGuard<T> {
  fn drop(&mut self) {
    self.shared.active_readers[self.idx].fetch_sub(1, Ordering::SeqCst);
  }
}

impl<T> WriteHandle<T> {
  /// Applies `f` to both slots using the double-execution pattern.
  ///
  /// Protocol:
  /// 1. Apply `f` to the currently-stale slot (safe: no active readers there).
  /// 2. Publish the stale slot as the new live slot (SeqCst store).
  /// 3. Wait (spin) until every reader that had entered the old live slot has
  ///    dropped its guard — their SeqCst decrement is visible after the fence.
  /// 4. Apply the identical `f` to the old live slot, bringing it back in sync.
  ///
  /// After step 4, both slots hold the same logical state and the invariant is
  /// re-established for the next call.
  pub(crate) fn modify<F>(&self, mut f: F)
  where
    F: FnMut(&mut T),
  {
    let _lock = self.writer_lock.lock();
    let live_idx = self.shared.live_idx.load(Ordering::SeqCst);
    let write_idx = 1 - live_idx;

    // Step 1 — mutate the currently-stale slot.
    // Safety: writer lock held; active_readers[write_idx] == 0 is guaranteed by
    // the invariant (we waited for it in the previous modify call, and no new
    // readers can join a non-live slot).
    let stale = unsafe {
      if write_idx == 0 {
        &mut *self.shared.data_0.get()
      } else {
        &mut *self.shared.data_1.get()
      }
    };
    f(stale);

    // Step 2 — atomically publish the updated slot.
    self.shared.live_idx.store(write_idx, Ordering::SeqCst);

    // Step 3 — drain readers still inside the old live slot.
    // SeqCst store above + SeqCst loads below form a total order that prevents
    // the writer from missing a reader that incremented active_readers[live_idx]
    // before observing the new live_idx.
    while self.shared.active_readers[live_idx].load(Ordering::SeqCst) > 0 {
      hint::spin_loop();
    }

    // Step 4 — bring the old slot back in sync.
    // Safety: writer lock held; spin above guarantees active_readers[live_idx] == 0.
    let old = unsafe {
      if live_idx == 0 {
        &mut *self.shared.data_0.get()
      } else {
        &mut *self.shared.data_1.get()
      }
    };
    f(old);
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::sync::atomic::AtomicBool;
  use std::thread;
  use std::time::Duration;

  #[test]
  fn initial_state_is_default() {
    let (rh, _wh) = new::<i32>();
    assert_eq!(*rh.enter(), 0);
  }

  #[test]
  fn write_and_read_back() {
    let (rh, wh) = new::<String>();

    assert_eq!(*rh.enter(), "");

    wh.modify(|s| *s = "hello".to_string());
    assert_eq!(*rh.enter(), "hello");

    wh.modify(|s| s.push_str(" world"));
    assert_eq!(*rh.enter(), "hello world");
  }

  #[test]
  fn cloned_read_handle_sees_updates() {
    let (rh, wh) = new::<usize>();
    let rh2 = rh.clone();

    assert_eq!(*rh.enter(), 0);
    assert_eq!(*rh2.enter(), 0);

    wh.modify(|val| *val = 100);

    assert_eq!(*rh.enter(), 100);
    assert_eq!(*rh2.enter(), 100);
  }

  #[derive(Default, Debug, PartialEq, Eq)]
  struct TestData {
    version: usize,
    data: [usize; 8],
  }

  impl TestData {
    fn is_consistent(&self) -> bool {
      self.data.iter().all(|&x| x == self.version)
    }
  }

  #[test]
  fn concurrent_reads_and_writes_are_consistent() {
    let (rh, wh) = new::<TestData>();
    let rh = Arc::new(rh);
    let wh = Arc::new(wh);
    let writer_finished = Arc::new(AtomicBool::new(false));

    let mut reader_handles = Vec::new();
    for _ in 0..4 {
      let rh = rh.clone();
      let finished = writer_finished.clone();
      reader_handles.push(thread::spawn(move || {
        let mut last_version = 0;
        while !finished.load(Ordering::Acquire) {
          let guard = rh.enter();
          assert!(guard.is_consistent(), "Inconsistent read: {:?}", *guard);
          assert!(guard.version >= last_version, "Version went backwards!");
          last_version = guard.version;
          thread::yield_now();
        }
      }));
    }

    let writer = thread::spawn(move || {
      for i in 1..=100 {
        // Closure is deterministic: sets all fields to `i` regardless of prior state.
        wh.modify(|d| {
          d.version = i;
          d.data.iter_mut().for_each(|x| *x = i);
        });
        thread::sleep(Duration::from_micros(10));
      }
      writer_finished.store(true, Ordering::Release);
    });

    writer.join().expect("writer panicked");
    for h in reader_handles {
      h.join().expect("reader panicked");
    }

    let final_guard = rh.enter();
    assert_eq!(final_guard.version, 100);
    assert!(final_guard.is_consistent());
  }
}
