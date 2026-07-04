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

const MUTEX_LOCKED: usize = 1;

pub struct HybridMutex<T> {
  state: AtomicUsize,
  waiters: WaitQueue,
  data: UnsafeCell<T>,
}

unsafe impl<T: Send> Send for HybridMutex<T> {}
unsafe impl<T: Send> Sync for HybridMutex<T> {}

impl<T> HybridMutex<T> {
  pub fn new(data: T) -> Self {
    Self {
      state: AtomicUsize::new(0),
      waiters: WaitQueue::new(),
      data: UnsafeCell::new(data),
    }
  }

  #[inline]
  pub fn lock(&self) -> MutexGuard<'_, T> {
    // Fast path: attempt to acquire the lock if it's free.
    if self
      .state
      .compare_exchange(0, MUTEX_LOCKED, Ordering::Acquire, Ordering::Relaxed)
      .is_ok()
    {
      return MutexGuard { lock: self };
    }
    self.lock_slow()
  }

  #[cold]
  fn lock_slow(&self) -> MutexGuard<'_, T> {
    let mut spin_count = 0;
    loop {
      // Spin for a short time before parking.
      if spin_count < 100 {
        if self
          .state
          .compare_exchange(0, MUTEX_LOCKED, Ordering::Acquire, Ordering::Relaxed)
          .is_ok()
        {
          return MutexGuard { lock: self };
        }
        spin_count += 1;
        thread::yield_now();
      } else {
        // Enqueue ourselves and park the thread.
        let node = Box::into_raw(Box::new(WaiterNode {
          waiter: Waiter::Thread(thread::current()),
          next: AtomicPtr::new(ptr::null_mut()),
        }));
        self.waiters.push(node);

        // Re-check state after pushing, before parking.
        if self
          .state
          .compare_exchange(0, MUTEX_LOCKED, Ordering::Acquire, Ordering::Relaxed)
          .is_ok()
        {
          // We got the lock! Our node might still be in the queue, but that's okay.
          // `unlock` will handle it.
          return MutexGuard { lock: self };
        }

        thread::park();
      }
    }
  }

  #[inline]
  pub async fn lock_async(&self) -> MutexGuard<'_, T> {
    if self
      .state
      .compare_exchange(0, MUTEX_LOCKED, Ordering::Acquire, Ordering::Relaxed)
      .is_ok()
    {
      return MutexGuard { lock: self };
    }
    MutexFuture { lock: self }.await
  }

  pub fn try_lock(&self) -> Option<MutexGuard<'_, T>> {
    // Attempt to grab the lock in a single, non-blocking operation.
    // If the current state is 0, set it to LOCKED. If not, fail.
    if self
      .state
      .compare_exchange(0, MUTEX_LOCKED, Ordering::Acquire, Ordering::Relaxed)
      .is_ok()
    {
      Some(MutexGuard { lock: self })
    } else {
      None
    }
  }

  fn unlock(&self) {
    // Unlock the state.
    self.state.store(0, Ordering::Release);

    // Wake up all waiters. They will re-contend for the lock.
    // This is simpler and fairer than trying to wake just one.
    for node in self.waiters.pop_all() {
      node.waiter.wake();
    }
  }
}

// === Mutex Guard ===
pub struct MutexGuard<'a, T> {
  lock: &'a HybridMutex<T>,
}

impl<T> Drop for MutexGuard<'_, T> {
  fn drop(&mut self) {
    self.lock.unlock();
  }
}

impl<T> Deref for MutexGuard<'_, T> {
  type Target = T;
  fn deref(&self) -> &T {
    unsafe { &*self.lock.data.get() }
  }
}

impl<T> DerefMut for MutexGuard<'_, T> {
  fn deref_mut(&mut self) -> &mut T {
    unsafe { &mut *self.lock.data.get() }
  }
}

// === Mutex Future ===
struct MutexFuture<'a, T> {
  lock: &'a HybridMutex<T>,
}

impl<'a, T> Future for MutexFuture<'a, T> {
  type Output = MutexGuard<'a, T>;

  fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    // Fast path check.
    if self
      .lock
      .state
      .compare_exchange(0, MUTEX_LOCKED, Ordering::Acquire, Ordering::Relaxed)
      .is_ok()
    {
      return Poll::Ready(MutexGuard { lock: self.lock });
    }

    // Enqueue the waker.
    let node = Box::into_raw(Box::new(WaiterNode {
      waiter: Waiter::Task(cx.waker().clone()),
      next: AtomicPtr::new(ptr::null_mut()),
    }));
    self.lock.waiters.push(node);

    // Re-check state after pushing.
    if self
      .lock
      .state
      .compare_exchange(0, MUTEX_LOCKED, Ordering::Acquire, Ordering::Relaxed)
      .is_ok()
    {
      return Poll::Ready(MutexGuard { lock: self.lock });
    }

    Poll::Pending
  }
}
