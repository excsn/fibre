//! A reader-writer lock with sync (parking) and async (waker) waiting.
//!
//! Contention strategy (matches the original Hybrid profile): spin with
//! yields first, barge freely, park only as a last resort, and on unlock
//! wake waiters to RE-CONTEND - the lock is always released before anyone
//! is woken, so ownership is never parked inside a suspended task and any
//! runnable thread (including a blocking waiter on an executor thread) can
//! always make progress.
//!
//! Correctness protocol (kept from the wait-list rework): waiter-owned
//! intrusive FIFO nodes, arm-then-recheck via RMWs so `Pending`/parking only
//! ever happens while a live holder is observed (a wake is always owed), and
//! cancellation unlinks under the list lock.
//!
//! Fairness: `WRITER_PENDING` is set only while a writer is actually queued
//! and gates new readers - the writer stays linked (keeping the gate up)
//! until it wins, which is what prevents reader streams from starving
//! writers. Pure-read workloads never set it and never touch the queue.

use super::wait_queue::{WaitList, Waiter, WaiterNode, WAITING, WOKEN};

use std::{
  cell::UnsafeCell,
  future::Future,
  ops::{Deref, DerefMut},
  pin::Pin,
  ptr,
  task::{Context, Poll},
};

// Sync primitives via the loom facade (see wait_queue.rs). Plain std re-exports
// in normal builds; loom's instrumented types only under channels' `--cfg loom`.
use crate::internal::sync::{thread, AtomicUsize, Ordering};

const WRITE_LOCKED: usize = 1;
const WRITER_PENDING: usize = 1 << 1;
const HAS_QUEUED: usize = 1 << 2;
const READER_UNIT: usize = 1 << 3;
const FLAGS: usize = WRITE_LOCKED | WRITER_PENDING | HAS_QUEUED;
const READERS: usize = !FLAGS;

/// Spin iterations (with `yield_now`) before parking - the original
/// Hybrid contention profile. Collapses to 1 under loom.
const SPIN_YIELDS: usize = if crate::internal::sync::IS_LOOM { 1 } else { 100 };
/// Lock-free acquisition attempts per async poll before queueing.
const POLL_ATTEMPTS: usize = if crate::internal::sync::IS_LOOM { 1 } else { 4 };

pub struct HybridRwLock<T> {
  state: AtomicUsize,
  waiters: WaitList,
  data: UnsafeCell<T>,
}

unsafe impl<T: Send + Sync> Sync for HybridRwLock<T> {}
unsafe impl<T: Send> Send for HybridRwLock<T> {}

impl<T> HybridRwLock<T> {
  pub fn new(data: T) -> Self {
    Self {
      state: AtomicUsize::new(0),
      waiters: WaitList::new(),
      data: UnsafeCell::new(data),
    }
  }

  /// Single lock-free read-acquisition attempt.
  #[inline]
  fn try_acquire_read(&self) -> bool {
    let s = self.state.load(Ordering::Relaxed);
    s & (WRITE_LOCKED | WRITER_PENDING) == 0
      && self
        .state
        .compare_exchange_weak(s, s + READER_UNIT, Ordering::Acquire, Ordering::Relaxed)
        .is_ok()
  }

  /// Single lock-free write-acquisition attempt (barges past the queue;
  /// flags are preserved).
  #[inline]
  fn try_acquire_write(&self) -> bool {
    let s = self.state.load(Ordering::Relaxed);
    s & (WRITE_LOCKED | READERS) == 0
      && self
        .state
        .compare_exchange(s, s | WRITE_LOCKED, Ordering::Acquire, Ordering::Relaxed)
        .is_ok()
  }

  // --- Read lock ---

  #[inline]
  pub fn read(&self) -> ReadGuard<'_, T> {
    if self.try_acquire_read() {
      return ReadGuard { lock: self };
    }
    self.read_slow()
  }

  #[cold]
  fn read_slow(&self) -> ReadGuard<'_, T> {
    let mut node = WaiterNode::new_sync(false);
    let node_ptr: *mut WaiterNode = &mut node;
    loop {
      // Phase 1: spin. Readers are only ever woken after being unlinked, so
      // the node is never linked here.
      for _ in 0..SPIN_YIELDS {
        if self.try_acquire_read() {
          return ReadGuard { lock: self };
        }
        thread::yield_now();
      }

      // Phase 2: queue. Arm-then-recheck: after linking, acquisition is
      // re-attempted via RMWs under the list lock, so we can only proceed
      // to park while a live holder was observed - whose unlock will see
      // HAS_QUEUED and wake us.
      {
        let mut g = self.waiters.lock();
        // SAFETY: node lives on this stack frame and is unlinked here.
        unsafe {
          g.rearm(node_ptr, Waiter::Thread(thread::current()));
          g.link_back(node_ptr);
        }
        self.state.fetch_or(HAS_QUEUED, Ordering::Relaxed);
        loop {
          let s = self.state.load(Ordering::Relaxed);
          if s & (WRITE_LOCKED | WRITER_PENDING) != 0 {
            break;
          }
          if self
            .state
            .compare_exchange(s, s + READER_UNIT, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
          {
            // SAFETY: we linked it above.
            unsafe { g.unlink(node_ptr) };
            self.fix_flags(&mut g);
            return ReadGuard { lock: self };
          }
        }
      }

      // SAFETY: the waker stops touching the node before (in list-lock
      // order) the WOKEN store we synchronize on.
      while unsafe { (*node_ptr).state.load(Ordering::Acquire) } == WAITING {
        thread::park();
      }
      // Woken readers were unlinked by the waker; re-contend.
    }
  }

  #[inline]
  pub async fn read_async(&self) -> ReadGuard<'_, T> {
    if self.try_acquire_read() {
      return ReadGuard { lock: self };
    }
    ReadFuture {
      lock: self,
      node: ptr::null_mut(),
      done: false,
    }
    .await
  }

  // --- Write lock ---

  #[inline]
  pub fn write(&self) -> WriteGuard<'_, T> {
    if self.try_acquire_write() {
      return WriteGuard { lock: self };
    }
    self.write_slow()
  }

  #[cold]
  fn write_slow(&self) -> WriteGuard<'_, T> {
    let mut node = WaiterNode::new_sync(true);
    let node_ptr: *mut WaiterNode = &mut node;
    // Writers stay linked while re-contending (keeps WRITER_PENDING up so
    // readers cannot starve us); track linkage locally - only we unlink it.
    let mut linked = false;
    loop {
      for _ in 0..SPIN_YIELDS {
        if self.try_acquire_write() {
          if linked {
            let mut g = self.waiters.lock();
            // SAFETY: we linked it and only we unlink writer nodes.
            unsafe { g.unlink(node_ptr) };
            self.fix_flags(&mut g);
          }
          return WriteGuard { lock: self };
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
        self
          .state
          .fetch_or(HAS_QUEUED | WRITER_PENDING, Ordering::Relaxed);
        loop {
          let s = self.state.load(Ordering::Relaxed);
          if s & (WRITE_LOCKED | READERS) != 0 {
            break;
          }
          if self
            .state
            .compare_exchange(s, s | WRITE_LOCKED, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
          {
            // SAFETY: linked above.
            unsafe { g.unlink(node_ptr) };
            self.fix_flags(&mut g);
            return WriteGuard { lock: self };
          }
        }
      }

      while unsafe { (*node_ptr).state.load(Ordering::Acquire) } == WAITING {
        thread::park();
      }
      // Woken writers remain linked; loop back and re-contend.
    }
  }

  #[inline]
  pub async fn write_async(&self) -> WriteGuard<'_, T> {
    if self.try_acquire_write() {
      return WriteGuard { lock: self };
    }
    WriteFuture {
      lock: self,
      node: ptr::null_mut(),
      done: false,
    }
    .await
  }

  // --- Try variants ---

  pub fn try_read(&self) -> Option<ReadGuard<'_, T>> {
    let s = self.state.load(Ordering::Relaxed);
    if s & (WRITE_LOCKED | WRITER_PENDING) != 0 {
      return None;
    }
    match self
      .state
      .compare_exchange(s, s + READER_UNIT, Ordering::Acquire, Ordering::Relaxed)
    {
      Ok(_) => Some(ReadGuard { lock: self }),
      Err(_) => None,
    }
  }

  pub fn try_write(&self) -> Option<WriteGuard<'_, T>> {
    if self.try_acquire_write() {
      Some(WriteGuard { lock: self })
    } else {
      None
    }
  }

  // --- Unlock / wake ---

  fn unlock_read(&self) {
    let prev = self.state.fetch_sub(READER_UNIT, Ordering::Release);
    debug_assert!(prev & READERS >= READER_UNIT);
    if prev & READERS == READER_UNIT && prev & HAS_QUEUED != 0 {
      self.wake_waiters();
    }
  }

  fn unlock_write(&self) {
    let prev = self.state.fetch_and(!WRITE_LOCKED, Ordering::Release);
    if prev & HAS_QUEUED != 0 {
      self.wake_waiters();
    }
  }

  /// Recomputes WRITER_PENDING / HAS_QUEUED from queue contents. Must be
  /// called with the list lock held, after any link/unlink.
  fn fix_flags(&self, g: &mut super::wait_queue::ListGuard<'_>) {
    if g.queued_writers() == 0 {
      self.state.fetch_and(!WRITER_PENDING, Ordering::Relaxed);
    } else {
      self.state.fetch_or(WRITER_PENDING, Ordering::Relaxed);
    }
    if g.is_empty() {
      self.state.fetch_and(!HAS_QUEUED, Ordering::Relaxed);
    } else {
      self.state.fetch_or(HAS_QUEUED, Ordering::Relaxed);
    }
  }

  /// Write-preferring wake: if any writer is queued, wake exactly the first
  /// one and LEAVE IT LINKED (WRITER_PENDING stays up while it re-contends).
  /// Otherwise wake all queued readers (unlinked) to re-contend. The lock is
  /// already released when this runs; woken waiters race and re-queue if
  /// they lose.
  fn wake_waiters(&self) {
    let mut g = self.waiters.lock();
    if g.queued_writers() > 0 {
      let w_node = g.first_writer();
      debug_assert!(!w_node.is_null());
      // SAFETY: linked node, valid under the list lock; take_and_mark_woken
      // does not touch the node after the guard drops. `None` means the
      // writer is already awake re-contending - its arm-then-recheck
      // covers this release.
      let w = unsafe { g.take_and_mark_woken(w_node) };
      drop(g);
      if let Some(w) = w {
        w.wake();
      }
      return;
    }

    let mut wakes: Vec<Waiter> = Vec::new();
    let mut cur = g.head();
    while !cur.is_null() {
      // SAFETY: linked nodes are valid under the list lock.
      let next = unsafe { g.next_of(cur) };
      unsafe { g.unlink(cur) };
      if let Some(w) = unsafe { g.take_and_mark_woken(cur) } {
        wakes.push(w);
      }
      cur = next;
    }
    self.fix_flags(&mut g);
    drop(g);
    for w in wakes {
      w.wake();
    }
  }
}

// === Guards ===

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

// === Futures ===
//
// One heap waiter node per future, allocated on first queueing; freed only
// by the future. Wakes are re-contention signals, never ownership transfer,
// so a cancelled future only has to unlink - plus pass the wake token on if
// it was woken but never used it.

struct ReadFuture<'a, T> {
  lock: &'a HybridRwLock<T>,
  node: *mut WaiterNode,
  done: bool,
}

// SAFETY: the raw node pointer is exclusively owned by this future.
unsafe impl<T: Send + Sync> Send for ReadFuture<'_, T> {}

impl<'a, T> Future for ReadFuture<'a, T> {
  type Output = ReadGuard<'a, T>;

  fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    let this = self.get_mut();
    debug_assert!(!this.done);
    let lock = this.lock;

    for _ in 0..POLL_ATTEMPTS {
      if lock.try_acquire_read() {
        this.finish_node();
        this.done = true;
        return Poll::Ready(ReadGuard { lock });
      }
    }

    if this.node.is_null() {
      this.node = Box::into_raw(Box::new(WaiterNode::new_task(false, cx.waker().clone())));
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
      if s & (WRITE_LOCKED | WRITER_PENDING) != 0 {
        break;
      }
      if lock
        .state
        .compare_exchange(s, s + READER_UNIT, Ordering::Acquire, Ordering::Relaxed)
        .is_ok()
      {
        // SAFETY: linked above.
        unsafe { g.unlink(node) };
        lock.fix_flags(&mut g);
        drop(g);
        // SAFETY: unlinked; we exclusively own it again.
        unsafe { drop(Box::from_raw(node)) };
        this.node = ptr::null_mut();
        this.done = true;
        return Poll::Ready(ReadGuard { lock });
      }
    }
    drop(g);
    Poll::Pending
  }
}

impl<T> ReadFuture<'_, T> {
  /// Frees the node after a successful lock-free acquisition (unlinking it
  /// first if it is still queued).
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

impl<T> Drop for ReadFuture<'_, T> {
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
      self.lock.wake_waiters();
    }
  }
}

struct WriteFuture<'a, T> {
  lock: &'a HybridRwLock<T>,
  node: *mut WaiterNode,
  done: bool,
}

// SAFETY: as for ReadFuture.
unsafe impl<T: Send + Sync> Send for WriteFuture<'_, T> {}

impl<'a, T> Future for WriteFuture<'a, T> {
  type Output = WriteGuard<'a, T>;

  fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    let this = self.get_mut();
    debug_assert!(!this.done);
    let lock = this.lock;

    for _ in 0..POLL_ATTEMPTS {
      if lock.try_acquire_write() {
        this.finish_node();
        this.done = true;
        return Poll::Ready(WriteGuard { lock });
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
    lock
      .state
      .fetch_or(HAS_QUEUED | WRITER_PENDING, Ordering::Relaxed);
    loop {
      let s = lock.state.load(Ordering::Relaxed);
      if s & (WRITE_LOCKED | READERS) != 0 {
        break;
      }
      if lock
        .state
        .compare_exchange(s, s | WRITE_LOCKED, Ordering::Acquire, Ordering::Relaxed)
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
        return Poll::Ready(WriteGuard { lock });
      }
    }
    drop(g);
    Poll::Pending
  }
}

impl<T> WriteFuture<'_, T> {
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

impl<T> Drop for WriteFuture<'_, T> {
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
      self.lock.wake_waiters();
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::sync::test_util::counting_waker;
  use std::sync::Arc;
  use std::sync::atomic::AtomicBool;
  use std::task::Context;
  use std::time::{Duration, Instant};

  const THREADS: usize = 4;
  const ITERS: usize = if cfg!(miri) { 25 } else { 10_000 };

  #[test]
  fn multiple_readers_coexist() {
    let l = HybridRwLock::new(7u32);
    let r1 = l.read();
    let r2 = l.read();
    assert_eq!(*r1, 7);
    assert_eq!(*r2, 7);
  }

  #[test]
  fn try_semantics() {
    let l = HybridRwLock::new(());
    let r = l.read();
    assert!(l.try_read().is_some());
    assert!(l.try_write().is_none());
    drop(r);
    // The try_read guard above was dropped immediately; only `r` was held.
    let w = l.try_write().unwrap();
    assert!(l.try_read().is_none());
    assert!(l.try_write().is_none());
    drop(w);
    assert!(l.try_read().is_some());
  }

  #[test]
  fn contended_writer_counter_is_exact() {
    let l = Arc::new(HybridRwLock::new(0usize));
    let handles: Vec<_> = (0..THREADS)
      .map(|_| {
        let l = l.clone();
        thread::spawn(move || {
          for _ in 0..ITERS {
            *l.write() += 1;
          }
        })
      })
      .collect();
    for h in handles {
      h.join().unwrap();
    }
    assert_eq!(*l.read(), THREADS * ITERS);
  }

  #[test]
  fn readers_never_observe_torn_writes() {
    // Writers keep the pair equal under the write lock; any reader observing
    // an unequal pair means a reader ran during a write.
    let l = Arc::new(HybridRwLock::new((0u64, 0u64)));
    let writers: Vec<_> = (0..2)
      .map(|_| {
        let l = l.clone();
        thread::spawn(move || {
          for _ in 0..ITERS {
            let mut g = l.write();
            g.0 += 1;
            g.1 += 1;
          }
        })
      })
      .collect();
    let readers: Vec<_> = (0..2)
      .map(|_| {
        let l = l.clone();
        thread::spawn(move || {
          for _ in 0..ITERS {
            let g = l.read();
            assert_eq!(g.0, g.1, "reader observed a torn write");
          }
        })
      })
      .collect();
    for h in writers.into_iter().chain(readers) {
      h.join().unwrap();
    }
    assert_eq!(*l.read(), (2 * ITERS as u64, 2 * ITERS as u64));
  }

  #[test]
  fn async_writer_woken_when_last_reader_leaves() {
    let l = HybridRwLock::new(());
    let r1 = l.read();
    let r2 = l.read();

    let (counter, waker) = counting_waker();
    let mut cx = Context::from_waker(&waker);
    let mut fut = Box::pin(l.write_async());
    assert!(fut.as_mut().poll(&mut cx).is_pending());

    drop(r1);
    assert_eq!(counter.count(), 0, "only the LAST reader out wakes waiters");
    drop(r2);
    assert_eq!(counter.count(), 1);
    assert!(fut.as_mut().poll(&mut cx).is_ready());
  }

  #[test]
  fn async_reader_woken_on_write_unlock() {
    let l = HybridRwLock::new(());
    let w = l.write();

    let (counter, waker) = counting_waker();
    let mut cx = Context::from_waker(&waker);
    let mut fut = Box::pin(l.read_async());
    assert!(fut.as_mut().poll(&mut cx).is_pending());

    drop(w);
    assert_eq!(counter.count(), 1);
    assert!(fut.as_mut().poll(&mut cx).is_ready());
  }

  #[test]
  fn cancelled_read_future_is_not_woken() {
    let l = HybridRwLock::new(());
    let w = l.write();

    let (counter, waker) = counting_waker();
    let mut cx = Context::from_waker(&waker);
    let mut fut = Box::pin(l.read_async());
    assert!(fut.as_mut().poll(&mut cx).is_pending());

    drop(fut); // cancel: unlinks its node
    drop(w);
    assert_eq!(counter.count(), 0, "cancelled waiter must not be woken");
    // Lock must still be usable.
    assert!(l.try_write().is_some());
  }

  #[test]
  fn repolling_reuses_single_waiter() {
    let l = HybridRwLock::new(());
    let w = l.write();

    let (counter, waker) = counting_waker();
    let mut cx = Context::from_waker(&waker);
    let mut fut = Box::pin(l.read_async());
    assert!(fut.as_mut().poll(&mut cx).is_pending());
    assert!(fut.as_mut().poll(&mut cx).is_pending());
    assert!(fut.as_mut().poll(&mut cx).is_pending());

    drop(w);
    assert_eq!(counter.count(), 1, "one waiter node per future, one wake");
    assert!(fut.as_mut().poll(&mut cx).is_ready());
  }

  /// Regression test for the P0-3 writer-starvation defect: two readers with
  /// phase-shifted overlapping holds used to keep the reader count above
  /// zero forever, so a parked writer was never woken. With WRITER_PENDING
  /// staying up while the queued writer re-contends, it must get through
  /// promptly.
  #[test]
  #[cfg_attr(miri, ignore = "timing-based; wall-clock sleeps")]
  fn writer_is_not_starved_by_overlapping_readers() {
    let l = Arc::new(HybridRwLock::new(0u64));
    let stop = Arc::new(AtomicBool::new(false));

    let mut readers = Vec::new();
    for offset_ms in [0u64, 1] {
      let l = l.clone();
      let stop = stop.clone();
      readers.push(thread::spawn(move || {
        thread::sleep(Duration::from_millis(offset_ms));
        while !stop.load(Ordering::Relaxed) {
          let _g = l.read();
          thread::sleep(Duration::from_millis(2));
        }
      }));
    }

    // Give the readers time to establish the overlap.
    thread::sleep(Duration::from_millis(10));

    let (tx, rx) = std::sync::mpsc::channel();
    let writer = {
      let l = l.clone();
      thread::spawn(move || {
        let start = Instant::now();
        let _g = l.write();
        let _ = tx.send(start.elapsed());
      })
    };

    let result = rx.recv_timeout(Duration::from_secs(3));

    stop.store(true, Ordering::Relaxed);
    for r in readers {
      r.join().unwrap();
    }
    writer.join().unwrap();

    match result {
      Ok(elapsed) => {
        assert!(
          elapsed < Duration::from_secs(1),
          "writer acquired but took {elapsed:?}"
        );
      }
      Err(_) => panic!("writer starved for 3s under overlapping readers"),
    }
  }
}
