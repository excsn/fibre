//! Intrusive FIFO wait list shared by `HybridMutex` and `HybridRwLock`.
//!
//! Nodes are OWNED by their waiters — a stack slot for sync waiters, a single
//! stable heap allocation for async futures — and the list only ever holds
//! raw pointers; it never allocates and never frees.
//!
//! Protocol (wake-and-recontend): the lock is always RELEASED before waiters
//! are woken; a wake is a re-contention signal, never an ownership transfer,
//! so a lock can never be owned by a suspended task. A waker takes the
//! node's `Waiter` handle and stores `WOKEN` — both under the list lock, so
//! the node is never touched after the lock is released (its owner may
//! observe `WOKEN` and immediately reuse or free it). `Pending`/parking is
//! only ever entered after the node is armed+linked and a live holder was
//! observed by a post-RMW load, so a wake is always owed — there is no
//! state in which a linked waiter can be forgotten.
//!
//! All node fields except `state` are accessed only while holding the list
//! spinlock. The spinlock is held for a handful of instructions and never
//! across user code, parking, or `.await`.

use std::cell::UnsafeCell;
use std::ptr;
use std::task::Waker;

// Sync primitives via the loom facade so `HybridMutex`/`HybridRwLock` are
// loom-modelable in channels' loom tests. Under normal builds these are plain
// std re-exports (zero cost); only channels' `--cfg loom` test build swaps in
// loom's instrumented types. The data cell stays std (loom's closure-cell can't
// back a guard's Deref).
use crate::internal::sync::{hint, thread, AtomicBool, AtomicU8, Ordering, Thread};

pub(super) const WAITING: u8 = 0;
pub(super) const WOKEN: u8 = 1;

pub(super) enum Waiter {
  Thread(Thread),
  Task(Waker),
}

impl Waiter {
  pub(super) fn wake(self) {
    match self {
      Waiter::Thread(t) => t.unpark(),
      Waiter::Task(w) => w.wake(),
    }
  }
}

pub(super) struct WaiterNode {
  prev: *mut WaiterNode,
  next: *mut WaiterNode,
  is_writer: bool,
  linked: bool,
  waiter: Option<Waiter>,
  /// Written under the list lock (Release) by the granter, read without it
  /// (Acquire) by the owner. The grant handoff synchronizes through this.
  pub(super) state: AtomicU8,
}

impl WaiterNode {
  /// A node for the calling thread (sync waiters).
  pub(super) fn new_sync(is_writer: bool) -> Self {
    Self {
      prev: ptr::null_mut(),
      next: ptr::null_mut(),
      is_writer,
      linked: false,
      waiter: Some(Waiter::Thread(thread::current())),
      state: AtomicU8::new(WAITING),
    }
  }

  /// A node for an async task (waker registered up front, before linking).
  pub(super) fn new_task(is_writer: bool, waker: Waker) -> Self {
    Self {
      prev: ptr::null_mut(),
      next: ptr::null_mut(),
      is_writer,
      linked: false,
      waiter: Some(Waiter::Task(waker)),
      state: AtomicU8::new(WAITING),
    }
  }
}

struct ListInner {
  head: *mut WaiterNode,
  tail: *mut WaiterNode,
  /// Number of queued writer nodes (drives the lock's WRITER_PENDING flag).
  writers: usize,
  len: usize,
}

pub(super) struct WaitList {
  locked: AtomicBool,
  inner: UnsafeCell<ListInner>,
}

// SAFETY: `inner` is only accessed while holding the `locked` spinlock; the
// raw node pointers it stores are kept alive by their owning waiters, which
// never free a node while it is linked.
unsafe impl Send for WaitList {}
unsafe impl Sync for WaitList {}

impl WaitList {
  pub(super) fn new() -> Self {
    Self {
      locked: AtomicBool::new(false),
      inner: UnsafeCell::new(ListInner {
        head: ptr::null_mut(),
        tail: ptr::null_mut(),
        writers: 0,
        len: 0,
      }),
    }
  }

  /// Acquires the list spinlock. Critical sections are a few instructions;
  /// this spins (never parks, never yields to an executor).
  pub(super) fn lock(&self) -> ListGuard<'_> {
    loop {
      if !self.locked.swap(true, Ordering::Acquire) {
        return ListGuard { list: self };
      }
      while self.locked.load(Ordering::Relaxed) {
        hint::spin_loop();
      }
    }
  }
}

pub(super) struct ListGuard<'a> {
  list: &'a WaitList,
}

impl Drop for ListGuard<'_> {
  fn drop(&mut self) {
    self.list.locked.store(false, Ordering::Release);
  }
}

impl ListGuard<'_> {
  fn inner(&mut self) -> &mut ListInner {
    // SAFETY: we hold the spinlock.
    unsafe { &mut *self.list.inner.get() }
  }

  pub(super) fn is_empty(&mut self) -> bool {
    self.inner().len == 0
  }

  pub(super) fn queued_writers(&mut self) -> usize {
    self.inner().writers
  }

  pub(super) fn head(&mut self) -> *mut WaiterNode {
    self.inner().head
  }

  /// # Safety
  /// `node` must be a valid, unlinked node owned by a live waiter, and must
  /// stay alive until it is unlinked again.
  pub(super) unsafe fn link_back(&mut self, node: *mut WaiterNode) {
    let inner = self.inner();
    unsafe {
      debug_assert!(!(*node).linked);
      (*node).prev = inner.tail;
      (*node).next = ptr::null_mut();
      (*node).linked = true;
      if inner.tail.is_null() {
        inner.head = node;
      } else {
        (*inner.tail).next = node;
      }
      inner.tail = node;
      if (*node).is_writer {
        inner.writers += 1;
      }
    }
    inner.len += 1;
  }

  /// Unlinks `node` if it is currently linked. Returns whether it was.
  ///
  /// # Safety
  /// `node` must be a valid node pointer owned by a live waiter.
  pub(super) unsafe fn unlink(&mut self, node: *mut WaiterNode) -> bool {
    let inner = self.inner();
    unsafe {
      if !(*node).linked {
        return false;
      }
      let prev = (*node).prev;
      let next = (*node).next;
      if prev.is_null() {
        inner.head = next;
      } else {
        (*prev).next = next;
      }
      if next.is_null() {
        inner.tail = prev;
      } else {
        (*next).prev = prev;
      }
      (*node).prev = ptr::null_mut();
      (*node).next = ptr::null_mut();
      (*node).linked = false;
      if (*node).is_writer {
        inner.writers -= 1;
      }
    }
    inner.len -= 1;
    true
  }

  /// # Safety
  /// `node` must be a valid node pointer owned by a live waiter.
  pub(super) unsafe fn is_linked(&mut self, node: *mut WaiterNode) -> bool {
    let _ = self.inner();
    unsafe { (*node).linked }
  }

  /// # Safety
  /// `node` must be a valid node pointer owned by a live waiter.
  pub(super) unsafe fn next_of(&mut self, node: *mut WaiterNode) -> *mut WaiterNode {
    let _ = self.inner();
    unsafe { (*node).next }
  }

  /// Re-arms a node for another waiting round: registers a fresh waiter
  /// handle and resets the state to WAITING.
  ///
  /// # Safety
  /// `node` must be a valid node pointer owned by a live waiter.
  pub(super) unsafe fn rearm(&mut self, node: *mut WaiterNode, waiter: Waiter) {
    let _ = self.inner();
    unsafe {
      (*node).waiter = Some(waiter);
      (*node).state.store(WAITING, Ordering::Relaxed);
    }
  }

  /// Takes the waiter handle and marks the node WOKEN — both under the list
  /// lock, so the node is never touched after the lock is released (its
  /// owner may observe WOKEN and immediately reuse or free it). Returns
  /// `None` if the waiter was already consumed (the owner is awake and
  /// re-contending; its arm-then-recheck covers this release).
  ///
  /// # Safety
  /// `node` must be a valid node pointer owned by a live waiter.
  pub(super) unsafe fn take_and_mark_woken(&mut self, node: *mut WaiterNode) -> Option<Waiter> {
    let _ = self.inner();
    unsafe {
      let w = (*node).waiter.take();
      (*node).state.store(WOKEN, Ordering::Release);
      w
    }
  }

  /// Returns the first writer node in FIFO order, if any.
  pub(super) fn first_writer(&mut self) -> *mut WaiterNode {
    let mut cur = self.inner().head;
    while !cur.is_null() {
      // SAFETY: linked nodes are valid while we hold the list lock.
      unsafe {
        if (*cur).is_writer {
          return cur;
        }
        cur = (*cur).next;
      }
    }
    ptr::null_mut()
  }
}
