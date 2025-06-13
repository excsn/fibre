use crossbeam_utils::CachePadded;
use parking_lot::{Condvar, Mutex};
use std::cell::UnsafeCell;
use std::collections::VecDeque;
use std::future::Future;
use std::hint::spin_loop;
use std::ops::{Deref, DerefMut};
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll, Waker};

// --- Constants and State Representation ---
const UNLOCKED: usize = 0;
const WRITE_LOCKED: usize = usize::MAX;
const SPIN_LIMIT: u32 = 100;

// The internal state of our lock, protected by a Mutex for the slow path.
#[derive(Default, Debug)]
struct LockState {
  async_writer_waiters: VecDeque<Waker>,
}

/// A custom, high-performance, async- and sync-aware RwLock.
#[derive(Default, Debug)]
pub(crate) struct HybridRwLock<T> {
  // Wrap hot fields in CachePadded to prevent false sharing.
  state: CachePadded<AtomicUsize>,
  waiters: CachePadded<Mutex<LockState>>,

  // Condvars are less contended and don't need padding as much.
  sync_writer_cv: Condvar,
  sync_reader_cv: Condvar,
  data: UnsafeCell<T>,
}

// We must manually implement Send and Sync because of UnsafeCell.
// It is safe because we ensure exclusive access via our state logic.
unsafe impl<T: Send> Send for HybridRwLock<T> {}
unsafe impl<T: Send + Sync> Sync for HybridRwLock<T> {}

impl<T> HybridRwLock<T> {
  pub fn new(data: T) -> Self {
    Self {
      state: CachePadded::new(AtomicUsize::new(UNLOCKED)),
      waiters: CachePadded::new(Mutex::new(LockState::default())),
      sync_writer_cv: Condvar::new(),
      sync_reader_cv: Condvar::new(),
      data: UnsafeCell::new(data),
    }
  }

  // --- Public Read API ---
  pub fn read(&self) -> ReadGuard<'_, T> {
    for _ in 0..SPIN_LIMIT {
      let s = self.state.load(Ordering::Acquire);
      if s < WRITE_LOCKED - 1 {
        if self
          .state
          .compare_exchange_weak(s, s + 1, Ordering::Acquire, Ordering::Relaxed)
          .is_ok()
        {
          return ReadGuard { lock: self };
        }
      }
      spin_loop();
    }
    self.read_slow()
  }

  #[cold]
  fn read_slow(&self) -> ReadGuard<'_, T> {
    let mut waiters_guard = self.waiters.lock();
    loop {
      let s = self.state.load(Ordering::Acquire);
      if s < WRITE_LOCKED - 1 {
        if self
          .state
          .compare_exchange_weak(s, s + 1, Ordering::Acquire, Ordering::Relaxed)
          .is_ok()
        {
          return ReadGuard { lock: self };
        }
      } else {
        self.sync_reader_cv.wait(&mut waiters_guard);
      }
    }
  }

  // --- Public Write API ---
  pub fn write_sync(&self) -> WriteGuard<'_, T> {
    for _ in 0..SPIN_LIMIT {
      if self
        .state
        .compare_exchange(UNLOCKED, WRITE_LOCKED, Ordering::Acquire, Ordering::Relaxed)
        .is_ok()
      {
        return WriteGuard { lock: self };
      }
      spin_loop();
    }
    self.write_sync_slow()
  }

  #[cold]
  fn write_sync_slow(&self) -> WriteGuard<'_, T> {
    let mut waiters_guard = self.waiters.lock();
    loop {
      if self
        .state
        .compare_exchange(UNLOCKED, WRITE_LOCKED, Ordering::Acquire, Ordering::Relaxed)
        .is_ok()
      {
        return WriteGuard { lock: self };
      }
      self.sync_writer_cv.wait(&mut waiters_guard);
    }
  }

  pub fn write_async(&self) -> WriteFuture<'_, T> {
    WriteFuture { lock: self }
  }

  // --- Unlock Logic ---
  fn unlock_read(&self) {
    if self.state.fetch_sub(1, Ordering::Release) == 1 {
      let mut waiters_guard = self.waiters.lock();
      if let Some(waker) = waiters_guard.async_writer_waiters.pop_front() {
        drop(waiters_guard);
        waker.wake();
      } else {
        drop(waiters_guard);
        self.sync_writer_cv.notify_one();
      }
    }
  }

  fn unlock_write(&self) {
    self.state.store(UNLOCKED, Ordering::Release);

    let mut waiters_guard = self.waiters.lock();
    if let Some(waker) = waiters_guard.async_writer_waiters.pop_front() {
      drop(waiters_guard);
      waker.wake();
    } else {
      drop(waiters_guard);
      self.sync_writer_cv.notify_one();
      self.sync_reader_cv.notify_all();
    }
  }
}

// --- Guards (RAII pattern) ---
#[derive(Debug)]
pub(crate) struct WriteGuard<'a, T: 'a> {
  lock: &'a HybridRwLock<T>,
}
impl<T> Drop for WriteGuard<'_, T> {
  fn drop(&mut self) {
    self.lock.unlock_write();
  }
}
impl<T> Deref for WriteGuard<'_, T> {
  type Target = T;
  fn deref(&self) -> &T {
    unsafe { &*self.lock.data.get() }
  }
}
impl<T> DerefMut for WriteGuard<'_, T> {
  fn deref_mut(&mut self) -> &mut T {
    unsafe { &mut *self.lock.data.get() }
  }
}

#[derive(Debug)]
pub(crate) struct ReadGuard<'a, T: 'a> {
  lock: &'a HybridRwLock<T>,
}
impl<T> Drop for ReadGuard<'_, T> {
  fn drop(&mut self) {
    self.lock.unlock_read();
  }
}
impl<T> Deref for ReadGuard<'_, T> {
  type Target = T;
  fn deref(&self) -> &T {
    unsafe { &*self.lock.data.get() }
  }
}

// --- Future for Async Write Lock ---
#[must_use = "futures do nothing unless you .await or poll them"]
pub(crate) struct WriteFuture<'a, T> {
  lock: &'a HybridRwLock<T>,
}

impl<'a, T> Future for WriteFuture<'a, T> {
  type Output = WriteGuard<'a, T>;

  fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    if self
      .lock
      .state
      .compare_exchange(UNLOCKED, WRITE_LOCKED, Ordering::Acquire, Ordering::Relaxed)
      .is_ok()
    {
      return Poll::Ready(WriteGuard { lock: self.lock });
    }

    {
      let mut waiters_guard = self.lock.waiters.lock();
      if self
        .lock
        .state
        .compare_exchange(UNLOCKED, WRITE_LOCKED, Ordering::Acquire, Ordering::Relaxed)
        .is_ok()
      {
        return Poll::Ready(WriteGuard { lock: self.lock });
      }
      waiters_guard
        .async_writer_waiters
        .push_back(cx.waker().clone());
    }

    Poll::Pending
  }
}
