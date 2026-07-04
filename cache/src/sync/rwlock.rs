use super::wait_queue::{WaitQueue, Waiter, WaiterNode};

use std::{
  cell::UnsafeCell,
  future::Future,
  ops::{Deref, DerefMut},
  pin::Pin,
  ptr,
  sync::atomic::{AtomicPtr, AtomicUsize, Ordering},
  task::{Context, Poll},
  thread,
};

// State bits for the RwLock
const RW_WRITE_LOCKED: usize = 1;
const RW_READ_LOCKED_SHIFT: usize = 1;

pub struct HybridRwLock<T> {
  state: AtomicUsize,
  waiters: WaitQueue,
  data: UnsafeCell<T>,
}

unsafe impl<T: Send + Sync> Sync for HybridRwLock<T> {}
unsafe impl<T: Send> Send for HybridRwLock<T> {}

impl<T> HybridRwLock<T> {
  pub fn new(data: T) -> Self {
    Self {
      state: AtomicUsize::new(0),
      waiters: WaitQueue::new(),
      data: UnsafeCell::new(data),
    }
  }

  // --- Read Lock ---
  #[inline]
  pub fn read(&self) -> ReadGuard<'_, T> {
    let s = self.state.load(Ordering::Relaxed);
    // Fast path: No writer, just increment reader count.
    if s & RW_WRITE_LOCKED == 0 {
      if self
        .state
        .compare_exchange_weak(
          s,
          s + (1 << RW_READ_LOCKED_SHIFT),
          Ordering::Acquire,
          Ordering::Relaxed,
        )
        .is_ok()
      {
        return ReadGuard { lock: self };
      }
    }
    self.read_slow()
  }

  #[cold]
  fn read_slow(&self) -> ReadGuard<'_, T> {
    let mut spin_count = 0;
    loop {
      let s = self.state.load(Ordering::Relaxed);
      if s & RW_WRITE_LOCKED == 0 {
        if self
          .state
          .compare_exchange_weak(
            s,
            s + (1 << RW_READ_LOCKED_SHIFT),
            Ordering::Acquire,
            Ordering::Relaxed,
          )
          .is_ok()
        {
          return ReadGuard { lock: self };
        }
      }

      if spin_count < 100 {
        spin_count += 1;
        thread::yield_now();
      } else {
        let node = Box::into_raw(Box::new(WaiterNode {
          waiter: Waiter::Thread(thread::current()),
          next: AtomicPtr::new(ptr::null_mut()),
        }));
        self.waiters.push(node);

        // Re-check after enqueueing
        let s = self.state.load(Ordering::Relaxed);
        if s & RW_WRITE_LOCKED == 0 {
          if self
            .state
            .compare_exchange_weak(
              s,
              s + (1 << RW_READ_LOCKED_SHIFT),
              Ordering::Acquire,
              Ordering::Relaxed,
            )
            .is_ok()
          {
            return ReadGuard { lock: self };
          }
        }

        thread::park();
      }
    }
  }

  #[inline]
  pub async fn read_async(&self) -> ReadGuard<'_, T> {
    let s = self.state.load(Ordering::Relaxed);
    if s & RW_WRITE_LOCKED == 0 {
      if self
        .state
        .compare_exchange_weak(
          s,
          s + (1 << RW_READ_LOCKED_SHIFT),
          Ordering::Acquire,
          Ordering::Relaxed,
        )
        .is_ok()
      {
        return ReadGuard { lock: self };
      }
    }
    ReadFuture { lock: self }.await
  }

  // --- Write Lock ---
  #[inline]
  pub fn write(&self) -> WriteGuard<'_, T> {
    if self
      .state
      .compare_exchange(0, RW_WRITE_LOCKED, Ordering::Acquire, Ordering::Relaxed)
      .is_ok()
    {
      return WriteGuard { lock: self };
    }
    self.write_slow()
  }

  #[cold]
  fn write_slow(&self) -> WriteGuard<'_, T> {
    let mut spin_count = 0;
    loop {
      if spin_count < 100 {
        if self
          .state
          .compare_exchange(0, RW_WRITE_LOCKED, Ordering::Acquire, Ordering::Relaxed)
          .is_ok()
        {
          return WriteGuard { lock: self };
        }
        spin_count += 1;
        thread::yield_now();
      } else {
        let node = Box::into_raw(Box::new(WaiterNode {
          waiter: Waiter::Thread(thread::current()),
          next: AtomicPtr::new(ptr::null_mut()),
        }));
        self.waiters.push(node);

        if self
          .state
          .compare_exchange(0, RW_WRITE_LOCKED, Ordering::Acquire, Ordering::Relaxed)
          .is_ok()
        {
          return WriteGuard { lock: self };
        }

        thread::park();
      }
    }
  }

  #[inline]
  pub async fn write_async(&self) -> WriteGuard<'_, T> {
    if self
      .state
      .compare_exchange(0, RW_WRITE_LOCKED, Ordering::Acquire, Ordering::Relaxed)
      .is_ok()
    {
      return WriteGuard { lock: self };
    }
    WriteFuture { lock: self }.await
  }

  pub fn try_read(&self) -> Option<ReadGuard<'_, T>> {
    let s = self.state.load(Ordering::Relaxed);

    // Fail immediately if a writer holds the lock.
    if (s & RW_WRITE_LOCKED) != 0 {
      return None;
    }

    // Attempt to atomically increment the reader count.
    // This will fail if the state changed (e.g., a writer just acquired the lock).
    match self.state.compare_exchange(
      s,
      s + (1 << RW_READ_LOCKED_SHIFT),
      Ordering::Acquire,
      Ordering::Relaxed,
    ) {
      Ok(_) => Some(ReadGuard { lock: self }),
      Err(_) => None, // State changed, indicating contention.
    }
  }

  pub fn try_write(&self) -> Option<WriteGuard<'_, T>> {
    // Attempt to grab the write lock in a single operation.
    // This only succeeds if the state is 0 (no readers and no writers).
    if self
      .state
      .compare_exchange(0, RW_WRITE_LOCKED, Ordering::Acquire, Ordering::Relaxed)
      .is_ok()
    {
      Some(WriteGuard { lock: self })
    } else {
      None
    }
  }

  // --- Unlock Logic ---
  fn unlock_read(&self) {
    let old_state = self
      .state
      .fetch_sub(1 << RW_READ_LOCKED_SHIFT, Ordering::Release);
    // If we were the last reader, check if writers are waiting.
    if old_state >> RW_READ_LOCKED_SHIFT == 1 {
      self.wake_waiters();
    }
  }

  fn unlock_write(&self) {
    self.state.store(0, Ordering::Release);
    self.wake_waiters();
  }

  fn wake_waiters(&self) {
    // The robust solution is to wake all waiters and let them re-contend for the lock.
    // This is simpler and guarantees no waiter is ever "lost".
    let waiters = self.waiters.pop_all();
    for node in waiters {
      node.waiter.wake();
    }
  }
}

// === RwLock Guards ===
pub struct ReadGuard<'a, T> {
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

pub struct WriteGuard<'a, T> {
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

// === RwLock Futures ===
struct ReadFuture<'a, T> {
  lock: &'a HybridRwLock<T>,
}

impl<'a, T> Future for ReadFuture<'a, T> {
  type Output = ReadGuard<'a, T>;
  fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    let s = self.lock.state.load(Ordering::Relaxed);
    if s & RW_WRITE_LOCKED == 0 {
      if self
        .lock
        .state
        .compare_exchange_weak(
          s,
          s + (1 << RW_READ_LOCKED_SHIFT),
          Ordering::Acquire,
          Ordering::Relaxed,
        )
        .is_ok()
      {
        return Poll::Ready(ReadGuard { lock: self.lock });
      }
    }

    let node = Box::into_raw(Box::new(WaiterNode {
      waiter: Waiter::Task(cx.waker().clone()),
      next: AtomicPtr::new(ptr::null_mut()),
    }));
    self.lock.waiters.push(node);

    let s = self.lock.state.load(Ordering::Relaxed);
    if s & RW_WRITE_LOCKED == 0 {
      if self
        .lock
        .state
        .compare_exchange_weak(
          s,
          s + (1 << RW_READ_LOCKED_SHIFT),
          Ordering::Acquire,
          Ordering::Relaxed,
        )
        .is_ok()
      {
        return Poll::Ready(ReadGuard { lock: self.lock });
      }
    }

    Poll::Pending
  }
}

struct WriteFuture<'a, T> {
  lock: &'a HybridRwLock<T>,
}

impl<'a, T> Future for WriteFuture<'a, T> {
  type Output = WriteGuard<'a, T>;
  fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    if self
      .lock
      .state
      .compare_exchange(0, RW_WRITE_LOCKED, Ordering::Acquire, Ordering::Relaxed)
      .is_ok()
    {
      return Poll::Ready(WriteGuard { lock: self.lock });
    }

    let node = Box::into_raw(Box::new(WaiterNode {
      waiter: Waiter::Task(cx.waker().clone()),
      next: AtomicPtr::new(ptr::null_mut()),
    }));
    self.lock.waiters.push(node);

    if self
      .lock
      .state
      .compare_exchange(0, RW_WRITE_LOCKED, Ordering::Acquire, Ordering::Relaxed)
      .is_ok()
    {
      return Poll::Ready(WriteGuard { lock: self.lock });
    }

    Poll::Pending
  }
}
