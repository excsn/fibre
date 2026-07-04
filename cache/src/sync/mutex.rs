//! A mutex with sync (parking) and async (waker) waiting, built on the same
//! FIFO wait list as `HybridRwLock` (see wait_queue.rs) with the same
//! contention strategy: spin with yields, barge freely, park last, and wake
//! waiters to RE-CONTEND — the lock is always released before anyone is
//! woken, so ownership is never parked inside a suspended task. Wakes go to
//! the queue head one at a time; the node stays linked while its owner
//! re-contends, and a loser simply re-arms and waits for the next wake.

use super::wait_queue::{WaitList, Waiter, WaiterNode, WAITING, WOKEN};

use std::{
  cell::UnsafeCell,
  future::Future,
  ops::{Deref, DerefMut},
  pin::Pin,
  ptr,
  sync::atomic::{AtomicUsize, Ordering},
  task::{Context, Poll},
  thread,
};

const LOCKED: usize = 1;
const HAS_QUEUED: usize = 1 << 1;

/// Spin iterations (with `yield_now`) before parking.
const SPIN_YIELDS: usize = 100;
/// Lock-free acquisition attempts per async poll before queueing.
const POLL_ATTEMPTS: usize = 4;

pub struct HybridMutex<T> {
  state: AtomicUsize,
  waiters: WaitList,
  data: UnsafeCell<T>,
}

unsafe impl<T: Send> Send for HybridMutex<T> {}
unsafe impl<T: Send> Sync for HybridMutex<T> {}

impl<T> HybridMutex<T> {
  pub fn new(data: T) -> Self {
    Self {
      state: AtomicUsize::new(0),
      waiters: WaitList::new(),
      data: UnsafeCell::new(data),
    }
  }

  /// Single lock-free acquisition attempt (barges past the queue; the
  /// HAS_QUEUED flag is preserved).
  #[inline]
  fn try_acquire(&self) -> bool {
    let s = self.state.load(Ordering::Relaxed);
    s & LOCKED == 0
      && self
        .state
        .compare_exchange(s, s | LOCKED, Ordering::Acquire, Ordering::Relaxed)
        .is_ok()
  }

  #[inline]
  pub fn lock(&self) -> MutexGuard<'_, T> {
    if self.try_acquire() {
      return MutexGuard { lock: self };
    }
    self.lock_slow()
  }

  #[cold]
  fn lock_slow(&self) -> MutexGuard<'_, T> {
    let mut node = WaiterNode::new_sync(true);
    let node_ptr: *mut WaiterNode = &mut node;
    // The node stays linked while we re-contend; only we unlink it.
    let mut linked = false;
    loop {
      for _ in 0..SPIN_YIELDS {
        if self.try_acquire() {
          if linked {
            let mut g = self.waiters.lock();
            // SAFETY: we linked it and only we unlink it.
            unsafe { g.unlink(node_ptr) };
            self.fix_flags(&mut g);
          }
          return MutexGuard { lock: self };
        }
        thread::yield_now();
      }

      {
        let mut g = self.waiters.lock();
        // SAFETY: node lives on this stack frame.
        unsafe {
          g.rearm(node_ptr, Waiter::Thread(thread::current()));
          if !linked {
            g.link_back(node_ptr);
            linked = true;
          }
        }
        self.state.fetch_or(HAS_QUEUED, Ordering::Relaxed);
        // Post-RMW loads are coherent: if we observe the lock held here,
        // that holder's unlock will see HAS_QUEUED and wake us.
        loop {
          let s = self.state.load(Ordering::Relaxed);
          if s & LOCKED != 0 {
            break;
          }
          if self
            .state
            .compare_exchange(s, s | LOCKED, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
          {
            // SAFETY: linked above.
            unsafe { g.unlink(node_ptr) };
            self.fix_flags(&mut g);
            return MutexGuard { lock: self };
          }
        }
      }

      // SAFETY: the waker stops touching the node before (in list-lock
      // order) the WOKEN store we synchronize on.
      while unsafe { (*node_ptr).state.load(Ordering::Acquire) } == WAITING {
        thread::park();
      }
      // Woken: still linked; loop back and re-contend.
    }
  }

  #[inline]
  pub async fn lock_async(&self) -> MutexGuard<'_, T> {
    if self.try_acquire() {
      return MutexGuard { lock: self };
    }
    MutexFuture {
      lock: self,
      node: ptr::null_mut(),
      done: false,
    }
    .await
  }

  pub fn try_lock(&self) -> Option<MutexGuard<'_, T>> {
    if self.try_acquire() {
      Some(MutexGuard { lock: self })
    } else {
      None
    }
  }

  fn unlock(&self) {
    let prev = self.state.fetch_and(!LOCKED, Ordering::Release);
    if prev & HAS_QUEUED != 0 {
      self.wake_next();
    }
  }

  fn fix_flags(&self, g: &mut super::wait_queue::ListGuard<'_>) {
    if g.is_empty() {
      self.state.fetch_and(!HAS_QUEUED, Ordering::Relaxed);
    } else {
      self.state.fetch_or(HAS_QUEUED, Ordering::Relaxed);
    }
  }

  /// Wakes the queue head to re-contend, leaving it linked. The lock is
  /// already released when this runs.
  fn wake_next(&self) {
    let mut g = self.waiters.lock();
    let head = g.head();
    if head.is_null() {
      self.fix_flags(&mut g);
      return;
    }
    // SAFETY: linked node, valid under the list lock; `None` means the
    // owner is already awake re-contending — its arm-then-recheck covers
    // this release.
    let w = unsafe { g.take_and_mark_woken(head) };
    drop(g);
    if let Some(w) = w {
      w.wake();
    }
  }
}

// === Guard ===

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

// === Future ===

struct MutexFuture<'a, T> {
  lock: &'a HybridMutex<T>,
  node: *mut WaiterNode,
  done: bool,
}

// SAFETY: the raw node pointer is exclusively owned by this future.
unsafe impl<T: Send> Send for MutexFuture<'_, T> {}

impl<'a, T> Future for MutexFuture<'a, T> {
  type Output = MutexGuard<'a, T>;

  fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    let this = self.get_mut();
    debug_assert!(!this.done);
    let lock = this.lock;

    for _ in 0..POLL_ATTEMPTS {
      if lock.try_acquire() {
        this.finish_node();
        this.done = true;
        return Poll::Ready(MutexGuard { lock });
      }
    }

    if this.node.is_null() {
      this.node = Box::into_raw(Box::new(WaiterNode::new_task(true, cx.waker().clone())));
    }
    let node = this.node;
    let mut g = lock.waiters.lock();
    // SAFETY: we own the node; mutations are under the list lock.
    unsafe {
      g.rearm(node, Waiter::Task(cx.waker().clone()));
      if !g.is_linked(node) {
        g.link_back(node);
      }
    }
    lock.state.fetch_or(HAS_QUEUED, Ordering::Relaxed);
    loop {
      let s = lock.state.load(Ordering::Relaxed);
      if s & LOCKED != 0 {
        break;
      }
      if lock
        .state
        .compare_exchange(s, s | LOCKED, Ordering::Acquire, Ordering::Relaxed)
        .is_ok()
      {
        // SAFETY: linked above.
        unsafe { g.unlink(node) };
        lock.fix_flags(&mut g);
        drop(g);
        // SAFETY: unlinked; exclusively ours again.
        unsafe { drop(Box::from_raw(node)) };
        this.node = ptr::null_mut();
        this.done = true;
        return Poll::Ready(MutexGuard { lock });
      }
    }
    drop(g);
    Poll::Pending
  }
}

impl<T> MutexFuture<'_, T> {
  fn finish_node(&mut self) {
    if self.node.is_null() {
      return;
    }
    let node = self.node;
    let mut g = self.lock.waiters.lock();
    // SAFETY: we own the node.
    if unsafe { g.unlink(node) } {
      self.lock.fix_flags(&mut g);
    }
    drop(g);
    unsafe { drop(Box::from_raw(node)) };
    self.node = ptr::null_mut();
  }
}

impl<T> Drop for MutexFuture<'_, T> {
  fn drop(&mut self) {
    if self.done || self.node.is_null() {
      return;
    }
    let node = self.node;
    let mut g = self.lock.waiters.lock();
    // SAFETY: we own the node.
    if unsafe { g.unlink(node) } {
      self.lock.fix_flags(&mut g);
    }
    drop(g);
    let was_woken = unsafe { (*node).state.load(Ordering::Acquire) } == WOKEN;
    unsafe { drop(Box::from_raw(node)) };
    if was_woken {
      // We consumed a wake we will never use; pass it on.
      self.lock.wake_next();
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::sync::test_util::counting_waker;
  use std::sync::Arc;
  use std::task::Context;

  const THREADS: usize = 4;
  const ITERS: usize = if cfg!(miri) { 25 } else { 10_000 };

  #[test]
  fn lock_and_mutate() {
    let m = HybridMutex::new(5u32);
    {
      let mut g = m.lock();
      *g += 1;
    }
    assert_eq!(*m.lock(), 6);
  }

  #[test]
  fn try_lock_semantics() {
    let m = HybridMutex::new(());
    let g = m.lock();
    assert!(m.try_lock().is_none());
    drop(g);
    assert!(m.try_lock().is_some());
  }

  #[test]
  fn contended_counter_is_exact() {
    let m = Arc::new(HybridMutex::new(0usize));
    let handles: Vec<_> = (0..THREADS)
      .map(|_| {
        let m = m.clone();
        thread::spawn(move || {
          for _ in 0..ITERS {
            *m.lock() += 1;
          }
        })
      })
      .collect();
    for h in handles {
      h.join().unwrap();
    }
    assert_eq!(*m.lock(), THREADS * ITERS);
  }

  #[test]
  fn lock_async_uncontended() {
    let m = HybridMutex::new(3u32);
    futures_executor::block_on(async {
      let mut g = m.lock_async().await;
      *g += 1;
    });
    assert_eq!(*m.lock(), 4);
  }

  #[test]
  fn async_waiter_woken_on_unlock() {
    let m = HybridMutex::new(());
    let g = m.lock();

    let (counter, waker) = counting_waker();
    let mut cx = Context::from_waker(&waker);
    let mut fut = Box::pin(m.lock_async());

    assert!(fut.as_mut().poll(&mut cx).is_pending());
    assert_eq!(counter.count(), 0, "no wake while lock is held");

    drop(g);
    assert_eq!(counter.count(), 1, "unlock must wake the queued waiter");
    assert!(fut.as_mut().poll(&mut cx).is_ready());
  }

  #[test]
  fn cancelled_async_waiter_is_not_woken() {
    let m = HybridMutex::new(());
    let g = m.lock();

    let (counter, waker) = counting_waker();
    let mut cx = Context::from_waker(&waker);
    let mut fut = Box::pin(m.lock_async());
    assert!(fut.as_mut().poll(&mut cx).is_pending());

    drop(fut); // cancel: unlinks its node
    assert_eq!(counter.count(), 0);

    drop(g);
    assert_eq!(counter.count(), 0, "cancelled waiter must not be woken");
    // Lock must still be usable.
    assert!(m.try_lock().is_some());
  }

  #[test]
  fn repolling_reuses_single_waiter() {
    let m = HybridMutex::new(());
    let g = m.lock();

    let (counter, waker) = counting_waker();
    let mut cx = Context::from_waker(&waker);
    let mut fut = Box::pin(m.lock_async());

    assert!(fut.as_mut().poll(&mut cx).is_pending());
    assert!(fut.as_mut().poll(&mut cx).is_pending());
    assert!(fut.as_mut().poll(&mut cx).is_pending());

    drop(g);
    assert_eq!(counter.count(), 1, "one waiter node per future, one wake");
    assert!(fut.as_mut().poll(&mut cx).is_ready());
  }

  #[test]
  fn cancelled_woken_waiter_passes_the_wake_on() {
    // f1 is woken by the unlock but dropped before it re-acquires; its Drop
    // must pass the wake to f2 instead of letting it sleep forever.
    let m = HybridMutex::new(());
    let g = m.lock();

    let (c1, w1) = counting_waker();
    let (c2, w2) = counting_waker();
    let mut cx1 = Context::from_waker(&w1);
    let mut cx2 = Context::from_waker(&w2);

    let mut f1 = Box::pin(m.lock_async());
    let mut f2 = Box::pin(m.lock_async());
    assert!(f1.as_mut().poll(&mut cx1).is_pending());
    assert!(f2.as_mut().poll(&mut cx2).is_pending());

    drop(g); // wakes f1 (FIFO head)
    assert_eq!(c1.count(), 1);
    assert_eq!(c2.count(), 0);

    drop(f1); // cancelled after being woken: must wake f2
    assert_eq!(c2.count(), 1);
    assert!(f2.as_mut().poll(&mut cx2).is_ready());
  }
}
