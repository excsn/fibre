//! A reader-biased reader-writer lock: readers that write no shared word.
//!
//! Readers normally count themselves in one of `SLOTS` per-thread slots, each
//! on its own cache line, and check a `visible` flag; a writer takes the inner
//! lock, clears `visible` and waits for every slot to read zero. Readers that
//! find `visible` clear take the inner lock's reader count instead and count a
//! fallback; the reader that completes `REVISIT` batches of `FALLBACK_BATCH`
//! fallbacks on its own thread turns the slots back on. Dice and Kogan,
//! "BRAVO: Biased Locking for Reader-Writer Locks", USENIX ATC 2019, with a
//! batched count in place of the timer.
//!
//! The inner lock's reader fast path is a single `fetch_add`, backed out only
//! when a writer holds the lock or is queued for it; a queued writer counts
//! itself in the state word so a reader stream cannot starve it, while a
//! writer that is still spinning lets readers barge. Waiting
//! parks on the intrusive `WaitList` shared with `HybridRwLock`, under the
//! same protocol: the lock is released before anyone is woken, a wake is a
//! re-contention signal, and `Pending` or parking only follows a post-link
//! re-check that saw a live holder.
//!
//! Slot order: a reader's slot increment and its `visible` load are SeqCst, as
//! are the writer's `visible` store and its slot loads, so either the writer
//! sees the slot or the reader sees the cleared flag.

use super::wait_queue::{WAITING, WOKEN, WaitList, Waiter, WaiterNode};
use crate::internal::cache_padded::CachePadded;
use crate::internal::sync::{AtomicBool, AtomicUsize, IS_LOOM, Ordering, hint, thread};

use std::{
  cell::{Cell, UnsafeCell},
  future::Future,
  ops::{Deref, DerefMut},
  pin::Pin,
  ptr,
  task::{Context, Poll},
};

const WRITE_LOCKED: usize = 1;
const HAS_QUEUED: usize = 1 << 1;
const PENDING_UNIT: usize = 1 << 2;
const PENDING_MASK: usize = 0xff << 2;
const READER_UNIT: usize = 1 << 10;
const READERS: usize = !(READER_UNIT - 1);

const SLOTS: usize = if IS_LOOM { 2 } else { 64 };
/// Spin iterations (with `yield_now`) before parking.
const SPIN_YIELDS: usize = if IS_LOOM { 1 } else { 100 };
/// Lock-free acquisition attempts per async poll before queueing.
const POLL_ATTEMPTS: usize = if IS_LOOM { 1 } else { 4 };
/// Polls an async waiter yields back to its runtime before it queues.
const ASYNC_YIELDS: u8 = if IS_LOOM { 1 } else { 8 };
/// Pure spins before a slot-draining writer starts yielding.
const DRAIN_SPINS: usize = if IS_LOOM { 1 } else { 64 };
/// Fallback reads counted on the reader's own thread before one shared increment.
const FALLBACK_BATCH: u32 = if IS_LOOM { 1 } else { 64 };
/// Shared increments before the slots come back on.
const REVISIT: usize = if IS_LOOM { 1 } else { 64 };

#[inline]
fn slot_index() -> usize {
  static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
  thread_local!(static SLOT: usize = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed) % SLOTS);
  SLOT.with(|s| *s)
}

/// True once per `FALLBACK_BATCH` calls on this thread.
#[inline]
fn fallback_batch_full() -> bool {
  thread_local!(static N: Cell<u32> = const { Cell::new(0) });
  N.with(|n| {
    let v = n.get() + 1;
    if v >= FALLBACK_BATCH {
      n.set(0);
      true
    } else {
      n.set(v);
      false
    }
  })
}

pub struct HybridRwLock<T> {
  state: CachePadded<AtomicUsize>,
  visible: CachePadded<AtomicBool>,
  fallbacks: CachePadded<AtomicUsize>,
  slots: Box<[CachePadded<AtomicUsize>]>,
  waiters: WaitList,
  data: UnsafeCell<T>,
}

unsafe impl<T: Send + Sync> Sync for HybridRwLock<T> {}
unsafe impl<T: Send> Send for HybridRwLock<T> {}

impl<T> HybridRwLock<T> {
  pub fn new(data: T) -> Self {
    Self {
      state: CachePadded::new(AtomicUsize::new(0)),
      visible: CachePadded::new(AtomicBool::new(true)),
      fallbacks: CachePadded::new(AtomicUsize::new(0)),
      slots: (0..SLOTS).map(|_| CachePadded::new(AtomicUsize::new(0))).collect(),
      waiters: WaitList::new(),
      data: UnsafeCell::new(data),
    }
  }

  // --- Slot readers ---

  #[inline]
  fn try_slot(&self) -> Option<usize> {
    if !self.visible.load(Ordering::Relaxed) {
      return None;
    }
    let i = slot_index();
    self.slots[i].fetch_add(1, Ordering::SeqCst);
    if self.visible.load(Ordering::SeqCst) {
      return Some(i);
    }
    self.slots[i].fetch_sub(1, Ordering::Release);
    None
  }

  #[inline]
  fn slots_clear(&self) -> bool {
    self.slots.iter().all(|s| s.load(Ordering::SeqCst) == 0)
  }

  /// Called with the inner write lock held.
  fn drain_slots(&self) {
    if !self.visible.load(Ordering::Relaxed) {
      return;
    }
    self.visible.store(false, Ordering::SeqCst);
    let mut spins = 0;
    while !self.slots_clear() {
      if spins < DRAIN_SPINS {
        spins += 1;
        hint::spin_loop();
      } else {
        thread::yield_now();
      }
    }
  }

  // --- Inner lock, lock-free attempts ---

  /// Counts a fallback read and turns the slots back on once enough have
  /// accumulated. Called by a reader holding the inner lock, so no writer
  /// holds it.
  #[inline]
  fn count_fallback(&self) {
    if fallback_batch_full() && self.fallbacks.fetch_add(1, Ordering::Relaxed) + 1 >= REVISIT {
      self.fallbacks.store(0, Ordering::Relaxed);
      self.visible.store(true, Ordering::SeqCst);
    }
  }

  /// One reader attempt on the inner lock: a `fetch_add`, backed out when a
  /// writer holds or waits. Never called with the list lock held, since the
  /// back-out may wake.
  #[inline]
  fn try_inner_read(&self) -> bool {
    if self.state.load(Ordering::Relaxed) & (WRITE_LOCKED | PENDING_MASK) != 0 {
      return false;
    }
    let s = self.state.fetch_add(READER_UNIT, Ordering::Acquire);
    if s & (WRITE_LOCKED | PENDING_MASK) == 0 {
      self.count_fallback();
      return true;
    }
    // A writer may have parked on the transient count.
    let prev = self.state.fetch_sub(READER_UNIT, Ordering::Release);
    if prev & READERS == READER_UNIT && prev & HAS_QUEUED != 0 {
      self.wake_waiters();
    }
    false
  }

  /// The reader attempt made under the list lock: a compare-exchange with no
  /// transient count, so it never has to wake anyone.
  #[inline]
  fn try_inner_read_queued(&self) -> bool {
    loop {
      let s = self.state.load(Ordering::Relaxed);
      if s & (WRITE_LOCKED | PENDING_MASK) != 0 {
        return false;
      }
      if self
        .state
        .compare_exchange(s, s + READER_UNIT, Ordering::Acquire, Ordering::Relaxed)
        .is_ok()
      {
        self.count_fallback();
        return true;
      }
    }
  }

  /// A barging writer attempt: no readers, no holder; flags are preserved.
  #[inline]
  fn try_inner_write(&self) -> bool {
    let s = self.state.load(Ordering::Relaxed);
    s & (WRITE_LOCKED | READERS) == 0
      && self
        .state
        .compare_exchange(s, s | WRITE_LOCKED, Ordering::Acquire, Ordering::Relaxed)
        .is_ok()
  }

  /// A writer attempt by a writer already counted in `PENDING_MASK`; the
  /// count drops with the acquisition.
  #[inline]
  fn try_inner_write_pending(&self) -> bool {
    let s = self.state.load(Ordering::Relaxed);
    s & (WRITE_LOCKED | READERS) == 0
      && self
        .state
        .compare_exchange(
          s,
          (s - PENDING_UNIT) | WRITE_LOCKED,
          Ordering::Acquire,
          Ordering::Relaxed,
        )
        .is_ok()
  }

  // --- Read lock ---

  #[inline]
  pub fn read(&self) -> ReadGuard<'_, T> {
    if let Some(slot) = self.try_slot() {
      return ReadGuard { lock: self, slot };
    }
    self.read_slow()
  }

  #[cold]
  fn read_slow(&self) -> ReadGuard<'_, T> {
    let mut node = WaiterNode::new_sync(false);
    let node_ptr: *mut WaiterNode = &mut node;
    loop {
      for _ in 0..SPIN_YIELDS {
        if let Some(slot) = self.try_slot() {
          return ReadGuard { lock: self, slot };
        }
        if self.try_inner_read() {
          return ReadGuard { lock: self, slot: SLOTS };
        }
        thread::yield_now();
      }

      {
        let mut g = self.waiters.lock();
        // SAFETY: node lives on this stack frame and is unlinked here.
        unsafe {
          g.rearm(node_ptr, Waiter::Thread(thread::current()));
          g.link_back(node_ptr);
        }
        self.state.fetch_or(HAS_QUEUED, Ordering::Relaxed);
        // Arm-then-recheck: a holder observed here owes us a wake.
        if self.try_inner_read_queued() {
          // SAFETY: linked above.
          unsafe { g.unlink(node_ptr) };
          self.fix_flags(&mut g);
          return ReadGuard { lock: self, slot: SLOTS };
        }
      }

      // SAFETY: the waker stops touching the node before (in list-lock
      // order) the WOKEN store we synchronize on.
      while unsafe { (*node_ptr).state.load(Ordering::Acquire) } == WAITING {
        thread::park();
      }
    }
  }

  #[inline]
  pub async fn read_async(&self) -> ReadGuard<'_, T> {
    if let Some(slot) = self.try_slot() {
      return ReadGuard { lock: self, slot };
    }
    ReadFuture {
      lock: self,
      node: ptr::null_mut(),
      yields: 0,
      done: false,
    }
    .await
  }

  // --- Write lock ---

  #[inline]
  pub fn write(&self) -> WriteGuard<'_, T> {
    if !self.try_inner_write() {
      self.write_slow();
    }
    self.drain_slots();
    WriteGuard { lock: self }
  }

  /// Acquires the inner write lock; the slots are drained by the caller. A
  /// writer counts itself pending, which turns new readers away, only once it
  /// queues: while it spins, readers still barge, as with `HybridRwLock`.
  #[cold]
  fn write_slow(&self) {
    let mut node = WaiterNode::new_sync(true);
    let node_ptr: *mut WaiterNode = &mut node;
    let mut linked = false;
    loop {
      for _ in 0..SPIN_YIELDS {
        let got = if linked { self.try_inner_write_pending() } else { self.try_inner_write() };
        if got {
          if linked {
            let mut g = self.waiters.lock();
            // SAFETY: we linked it and only we unlink writer nodes.
            unsafe { g.unlink(node_ptr) };
            self.fix_flags(&mut g);
          }
          return;
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
            self.state.fetch_add(PENDING_UNIT, Ordering::Relaxed);
          }
        }
        self.state.fetch_or(HAS_QUEUED, Ordering::Relaxed);
        if self.try_inner_write_pending() {
          // SAFETY: linked above.
          unsafe { g.unlink(node_ptr) };
          self.fix_flags(&mut g);
          return;
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
    WriteFuture {
      lock: self,
      node: ptr::null_mut(),
      phase: if self.try_inner_write() { Phase::Drain } else { Phase::Acquire },
      pending: false,
      draining: false,
      yields: 0,
      done: false,
    }
    .await
  }

  // --- Try variants ---

  pub fn try_read(&self) -> Option<ReadGuard<'_, T>> {
    if let Some(slot) = self.try_slot() {
      return Some(ReadGuard { lock: self, slot });
    }
    if self.try_inner_read() {
      return Some(ReadGuard { lock: self, slot: SLOTS });
    }
    None
  }

  /// Takes the lock when no writer holds it and no reader holds the inner
  /// count. Readers in slots are drained, which waits for their reads to end.
  pub fn try_write(&self) -> Option<WriteGuard<'_, T>> {
    if self.try_inner_write() {
      self.drain_slots();
      Some(WriteGuard { lock: self })
    } else {
      None
    }
  }

  // --- Unlock / wake ---

  fn unlock_read(&self, slot: usize) {
    if slot < SLOTS {
      // Only the increment pairs with the `visible` load; the writer's SeqCst
      // slot loads see this release.
      self.slots[slot].fetch_sub(1, Ordering::Release);
      return;
    }
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

  /// Recomputes HAS_QUEUED from the list. Must be called with the list lock
  /// held, after any link or unlink.
  fn fix_flags(&self, g: &mut super::wait_queue::ListGuard<'_>) {
    if g.is_empty() {
      self.state.fetch_and(!HAS_QUEUED, Ordering::Relaxed);
    } else {
      self.state.fetch_or(HAS_QUEUED, Ordering::Relaxed);
    }
  }

  /// Write-preferring wake, as in `HybridRwLock`: the first queued writer is
  /// woken and left linked; with no writer queued every reader is woken and
  /// unlinked. The lock is already released when this runs.
  fn wake_waiters(&self) {
    let mut g = self.waiters.lock();
    if g.queued_writers() > 0 {
      let w_node = g.first_writer();
      debug_assert!(!w_node.is_null());
      // SAFETY: linked node, valid under the list lock.
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
  /// `SLOTS` when the inner reader count is held instead of a slot.
  slot: usize,
}

impl<T> Drop for ReadGuard<'_, T> {
  fn drop(&mut self) {
    self.lock.unlock_read(self.slot);
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
// One heap waiter node per future, allocated on first queueing and freed only
// by the future, as in `HybridRwLock`.

struct ReadFuture<'a, T> {
  lock: &'a HybridRwLock<T>,
  node: *mut WaiterNode,
  yields: u8,
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
      if let Some(slot) = lock.try_slot() {
        this.finish_node();
        this.done = true;
        return Poll::Ready(ReadGuard { lock, slot });
      }
      if lock.try_inner_read() {
        this.finish_node();
        this.done = true;
        return Poll::Ready(ReadGuard { lock, slot: SLOTS });
      }
    }

    if this.yields < ASYNC_YIELDS {
      this.yields += 1;
      cx.waker().wake_by_ref();
      return Poll::Pending;
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
    if lock.try_inner_read_queued() {
      // SAFETY: linked above.
      unsafe { g.unlink(node) };
      lock.fix_flags(&mut g);
      drop(g);
      // SAFETY: unlinked; we exclusively own it again.
      unsafe { drop(Box::from_raw(node)) };
      this.node = ptr::null_mut();
      this.done = true;
      return Poll::Ready(ReadGuard { lock, slot: SLOTS });
    }
    drop(g);
    Poll::Pending
  }
}

impl<T> ReadFuture<'_, T> {
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
      self.lock.wake_waiters();
    }
  }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
  /// Waiting for the inner write lock.
  Acquire,
  /// Holding the inner write lock, waiting for the slots to empty.
  Drain,
}

struct WriteFuture<'a, T> {
  lock: &'a HybridRwLock<T>,
  node: *mut WaiterNode,
  phase: Phase,
  /// Counted in `PENDING_MASK`; dropped with the acquisition or on cancel.
  pending: bool,
  /// This future cleared `visible` and owes a scan until the slots are empty.
  draining: bool,
  yields: u8,
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

    if this.phase == Phase::Acquire {
      let mut acquired = false;
      for _ in 0..POLL_ATTEMPTS {
        let got = if this.pending { lock.try_inner_write_pending() } else { lock.try_inner_write() };
        if got {
          acquired = true;
          break;
        }
      }
      if !acquired && this.yields < ASYNC_YIELDS {
        this.yields += 1;
        cx.waker().wake_by_ref();
        return Poll::Pending;
      }
      if !acquired {
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
            // Pending, which turns new readers away, only once queued.
            if !this.pending {
              lock.state.fetch_add(PENDING_UNIT, Ordering::Relaxed);
              this.pending = true;
            }
          }
        }
        lock.state.fetch_or(HAS_QUEUED, Ordering::Relaxed);
        if lock.try_inner_write_pending() {
          // SAFETY: linked above.
          unsafe { g.unlink(node) };
          lock.fix_flags(&mut g);
          acquired = true;
        }
        drop(g);
        if !acquired {
          return Poll::Pending;
        }
      }
      this.pending = false;
      this.finish_node();
      this.phase = Phase::Drain;
    }

    // Holding the inner lock. Slots are off and empty unless `visible` is set:
    // it only turns on under an inner read hold, and the writer that turned it
    // off drained them. Slot readers hold the lock for a read's duration and
    // never park anyone, so the drain yields to the runtime instead of queueing.
    if !lock.visible.load(Ordering::Relaxed) && !this.draining {
      this.done = true;
      return Poll::Ready(WriteGuard { lock });
    }
    if !this.draining {
      lock.visible.store(false, Ordering::SeqCst);
      this.draining = true;
    }
    for _ in 0..DRAIN_SPINS {
      if lock.slots_clear() {
        this.done = true;
        return Poll::Ready(WriteGuard { lock });
      }
      hint::spin_loop();
    }
    cx.waker().wake_by_ref();
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
    if self.done {
      return;
    }
    if self.phase == Phase::Drain {
      // Cancelled while holding the inner lock: release it as a guard would.
      self.lock.unlock_write();
      return;
    }
    if self.pending {
      self.lock.state.fetch_sub(PENDING_UNIT, Ordering::Relaxed);
    }
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

  /// Polls through the yield phase until the future has queued; every poll
  /// must be pending. Returns the wake count at that point, since each yield
  /// wakes the task once.
  fn queue<F: Future + ?Sized>(fut: &mut Pin<Box<F>>, cx: &mut Context<'_>, counter: &crate::sync::test_util::CountingWaker) -> usize {
    for _ in 0..=ASYNC_YIELDS {
      assert!(fut.as_mut().poll(cx).is_pending());
    }
    counter.count()
  }

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
    drop(r);
    let w = l.try_write().unwrap();
    assert!(l.try_read().is_none());
    assert!(l.try_write().is_none());
    drop(w);
    assert!(l.try_read().is_some());
  }

  #[test]
  fn slots_turn_off_for_a_writer_and_back_on_after_fallbacks() {
    let l = HybridRwLock::new(0u32);
    assert!(l.visible.load(Ordering::SeqCst));
    {
      let r = l.read();
      assert!(r.slot < SLOTS);
    }
    drop(l.write());
    assert!(!l.visible.load(Ordering::SeqCst));
    {
      let r = l.read();
      assert_eq!(r.slot, SLOTS, "a read after a write takes the inner lock");
    }
    for _ in 0..(FALLBACK_BATCH as usize * REVISIT) {
      drop(l.read());
    }
    assert!(l.visible.load(Ordering::SeqCst), "fallback reads turn the slots back on");
    assert!(l.read().slot < SLOTS);
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
    let readers: Vec<_> = (0..THREADS)
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
  fn async_writer_drains_slot_readers() {
    let l = HybridRwLock::new(());
    let r1 = l.read();
    let r2 = l.read();
    assert!(r1.slot < SLOTS && r2.slot < SLOTS);

    let (counter, waker) = counting_waker();
    let mut cx = Context::from_waker(&waker);
    let mut fut = Box::pin(l.write_async());
    assert!(fut.as_mut().poll(&mut cx).is_pending());
    assert!(!l.visible.load(Ordering::SeqCst), "the writer holds the inner lock and has cleared visible");
    assert!(counter.count() >= 1, "a draining writer re-polls itself");

    drop(r1);
    assert!(fut.as_mut().poll(&mut cx).is_pending());
    drop(r2);
    assert!(fut.as_mut().poll(&mut cx).is_ready());
  }

  #[test]
  fn async_writer_woken_when_last_inner_reader_leaves() {
    let l = HybridRwLock::new(());
    drop(l.write());
    let r1 = l.read();
    let r2 = l.read();
    assert_eq!(r1.slot, SLOTS);

    let (counter, waker) = counting_waker();
    let mut cx = Context::from_waker(&waker);
    let mut fut = Box::pin(l.write_async());
    let base = queue(&mut fut, &mut cx, &counter);

    drop(r1);
    assert_eq!(counter.count(), base, "only the LAST reader out wakes waiters");
    drop(r2);
    assert_eq!(counter.count(), base + 1);
    assert!(fut.as_mut().poll(&mut cx).is_ready());
  }

  #[test]
  fn async_reader_woken_on_write_unlock() {
    let l = HybridRwLock::new(());
    let w = l.write();

    let (counter, waker) = counting_waker();
    let mut cx = Context::from_waker(&waker);
    let mut fut = Box::pin(l.read_async());
    let base = queue(&mut fut, &mut cx, &counter);

    drop(w);
    assert_eq!(counter.count(), base + 1);
    assert!(fut.as_mut().poll(&mut cx).is_ready());
  }

  #[test]
  fn cancelled_read_future_is_not_woken() {
    let l = HybridRwLock::new(());
    let w = l.write();

    let (counter, waker) = counting_waker();
    let mut cx = Context::from_waker(&waker);
    let mut fut = Box::pin(l.read_async());
    let base = queue(&mut fut, &mut cx, &counter);

    drop(fut);
    drop(w);
    assert_eq!(counter.count(), base, "cancelled waiter must not be woken");
    assert!(l.try_write().is_some());
  }

  #[test]
  fn cancelled_write_future_releases_its_pending_count() {
    let l = HybridRwLock::new(());
    let w = l.write();

    let (counter, waker) = counting_waker();
    let mut cx = Context::from_waker(&waker);
    let mut fut = Box::pin(l.write_async());
    queue(&mut fut, &mut cx, &counter);
    assert_ne!(l.state.load(Ordering::SeqCst) & PENDING_MASK, 0, "a queued writer is pending");
    drop(fut);
    assert_eq!(l.state.load(Ordering::SeqCst) & PENDING_MASK, 0);
    drop(w);
    assert!(l.try_read().is_some());
  }

  #[test]
  fn cancelled_draining_write_future_releases_the_lock() {
    let l = HybridRwLock::new(());
    drop(l.write());
    for _ in 0..(FALLBACK_BATCH as usize * REVISIT) {
      drop(l.read());
    }
    let r = l.read();
    assert!(r.slot < SLOTS);

    let (_counter, waker) = counting_waker();
    let mut cx = Context::from_waker(&waker);
    let mut fut = Box::pin(l.write_async());
    assert!(fut.as_mut().poll(&mut cx).is_pending());
    drop(fut);
    drop(r);
    assert!(l.try_write().is_some(), "a cancelled draining writer must release the inner lock");
  }

  #[test]
  fn repolling_reuses_single_waiter() {
    let l = HybridRwLock::new(());
    let w = l.write();

    let (counter, waker) = counting_waker();
    let mut cx = Context::from_waker(&waker);
    let mut fut = Box::pin(l.read_async());
    let base = queue(&mut fut, &mut cx, &counter);
    assert!(fut.as_mut().poll(&mut cx).is_pending());
    assert!(fut.as_mut().poll(&mut cx).is_pending());

    drop(w);
    assert_eq!(counter.count(), base + 1, "one waiter node per future, one wake");
    assert!(fut.as_mut().poll(&mut cx).is_ready());
  }

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
        assert!(elapsed < Duration::from_secs(1), "writer acquired but took {elapsed:?}");
      }
      Err(_) => panic!("writer starved for 3s under overlapping readers"),
    }
  }
}
