// channels/src/spmc/topic/left_right.rs

//! A minimal, custom implementation of a copy-on-write, single-writer,
//! multi-reader synchronization primitive.
//!
//! Read operations are lock-free: they increment an atomic reader counter for
//! the live slot, read, then decrement. Write operations acquire a mutex to
//! serialize writers, wait until the stale slot has zero active readers, copy
//! the live data into the stale slot, apply the change, then atomically publish.

use std::cell::UnsafeCell;
use std::ops::Deref;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use core::fmt;
use parking_lot::Mutex;

/// The shared state, containing two copies of the data, an atomic index to the
/// "live" copy, and per-slot reader counts used to guard the stale slot.
struct LeftRightShared<T> {
  live_idx: AtomicUsize,
  data: [UnsafeCell<T>; 2],
  /// Counts active readers on each slot. The writer spins on
  /// `active_readers[write_idx]` before overwriting that slot.
  active_readers: [AtomicUsize; 2],
}

impl<T> fmt::Debug for LeftRightShared<T> {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("LeftRightShared")
      .field("live_idx", &self.live_idx.load(Ordering::Relaxed))
      .finish_non_exhaustive()
  }
}

unsafe impl<T: Send> Send for LeftRightShared<T> {}
unsafe impl<T: Send + Sync> Sync for LeftRightShared<T> {}

/// The handle for reading data. This is `Send + Sync` and can be cloned freely.
#[derive(Debug)]
pub(crate) struct ReadHandle<T> {
  shared: Arc<LeftRightShared<T>>,
}

/// The handle for writing data. Only one can exist logically.
#[derive(Debug)]
pub(crate) struct WriteHandle<T> {
  shared: Arc<LeftRightShared<T>>,
  writer_lock: Mutex<()>,
}

/// A guard that provides access to a consistent snapshot of the data.
/// Holds a reader count on the slot until dropped.
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

/// Creates a new `left-right` pair.
pub(crate) fn new<T: Clone + Default>() -> (ReadHandle<T>, WriteHandle<T>) {
  let shared = Arc::new(LeftRightShared {
    live_idx: AtomicUsize::new(0),
    data: [UnsafeCell::new(T::default()), UnsafeCell::new(T::default())],
    active_readers: [AtomicUsize::new(0), AtomicUsize::new(0)],
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
  /// Enters the read-side, returning a guard that pins a reader count on the
  /// live slot. Lock-free except for the atomic increment/decrement.
  pub(crate) fn enter(&self) -> ReadGuard<T> {
    loop {
      let idx = self.shared.live_idx.load(Ordering::Acquire);

      // Arm the reader count on this slot.
      self.shared.active_readers[idx].fetch_add(1, Ordering::SeqCst);

      // Double-check: if live_idx is still the same we are committed to this slot.
      if self.shared.live_idx.load(Ordering::Relaxed) == idx {
        let data = unsafe { &*self.shared.data[idx].get() } as *const T;
        return ReadGuard {
          shared: self.shared.clone(),
          idx,
          data,
        };
      }

      // Writer swapped the index between our load and the increment — back off
      // and retry on the new live slot.
      self.shared.active_readers[idx].fetch_sub(1, Ordering::SeqCst);
    }
  }
}

impl<T> Clone for ReadHandle<T> {
  fn clone(&self) -> Self {
    Self {
      shared: self.shared.clone(),
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

impl<T: Clone> WriteHandle<T> {
  /// Modifies the data using a copy-on-write strategy.
  ///
  /// Acquires the writer lock, waits until the stale slot has no active readers,
  /// clones the live data into the stale slot, applies `f`, then publishes.
  pub(crate) fn modify<F>(&self, f: F)
  where
    F: FnOnce(&mut T),
  {
    let _lock = self.writer_lock.lock();
    let live_idx = self.shared.live_idx.load(Ordering::Relaxed);
    let write_idx = 1 - live_idx;

    // Wait until every reader that pinned the stale slot has dropped its guard.
    while self.shared.active_readers[write_idx].load(Ordering::Acquire) > 0 {
      std::hint::spin_loop();
    }

    // Safety: writer lock held; active_readers[write_idx] == 0 guarantees no
    // concurrent readers on this slot.
    let live_data = unsafe { &*self.shared.data[live_idx].get() };
    let write_data = unsafe { &mut *self.shared.data[write_idx].get() };

    write_data.clone_from(live_data);
    f(write_data);

    // Release: makes write_data modifications visible to readers that acquire
    // on live_idx.
    self.shared.live_idx.store(write_idx, Ordering::Release);
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::thread;
  use std::time::Duration;

  #[test]
  fn initial_state_is_default() {
    let (rh, _wh) = new::<i32>();
    let guard = rh.enter();
    assert_eq!(*guard, 0); // Default for i32 is 0
  }

  #[test]
  fn write_and_read_back() {
    let (rh, wh) = new::<String>();

    // Initial read
    assert_eq!(*rh.enter(), "");

    // First modification
    wh.modify(|s| *s = "hello".to_string());
    assert_eq!(*rh.enter(), "hello");

    // Second modification
    wh.modify(|s| s.push_str(" world"));
    assert_eq!(*rh.enter(), "hello world");
  }

  #[test]
  fn cloned_read_handle_sees_updates() {
    let (rh, wh) = new::<usize>();
    let rh_clone = rh.clone();

    // Both handles see the initial state
    assert_eq!(*rh.enter(), 0);
    assert_eq!(*rh_clone.enter(), 0);

    // Modify the data
    wh.modify(|val| *val = 100);

    // Both handles should see the updated state
    assert_eq!(*rh.enter(), 100);
    assert_eq!(*rh_clone.enter(), 100);
  }

  #[derive(Clone, Default, Debug, PartialEq, Eq)]
  struct TestData {
    version: usize,
    // Using a small array to make consistency checks more robust
    data: [usize; 8],
  }

  impl TestData {
    // A check to ensure the data is in a consistent state
    fn is_consistent(&self) -> bool {
      self.data.iter().all(|&x| x == self.version)
    }
  }

  #[test]
  fn concurrent_reads_and_writes_are_consistent() {
    let (rh, wh) = new::<TestData>();
    let rh = Arc::new(rh);
    let wh = Arc::new(wh);
    let writer_finished = Arc::new(std::sync::atomic::AtomicBool::new(false));

    let mut reader_handles = Vec::new();

    // Spawn multiple reader threads
    for _ in 0..4 {
      let rh_clone = rh.clone();
      let writer_finished_clone = writer_finished.clone();
      reader_handles.push(thread::spawn(move || {
        let mut last_seen_version = 0;
        // Readers spin as long as the writer is active
        while !writer_finished_clone.load(Ordering::Acquire) {
          let guard = rh_clone.enter();
          // The key assertion: we must always see a consistent state
          assert!(
            guard.is_consistent(),
            "Inconsistent read detected: {:?}",
            *guard
          );
          // Version should only ever increase
          assert!(
            guard.version >= last_seen_version,
            "Version went backwards!"
          );
          last_seen_version = guard.version;
          thread::yield_now();
        }
      }));
    }

    // Spawn a single writer thread
    let writer_handle = thread::spawn(move || {
      for i in 1..=100 {
        wh.modify(|d| {
          d.version = i;
          // Set all elements to the new version number
          d.data.iter_mut().for_each(|item| *item = i);
        });
        thread::sleep(Duration::from_micros(10)); // Allow readers to interleave
      }
      // Signal readers that we are done
      writer_finished.store(true, Ordering::Release);
    });

    writer_handle.join().expect("Writer thread panicked");
    for handle in reader_handles {
      handle.join().expect("Reader thread panicked");
    }

    // Final check on the data
    let final_guard = rh.enter();
    assert_eq!(final_guard.version, 100);
    assert!(final_guard.is_consistent());
  }
}
