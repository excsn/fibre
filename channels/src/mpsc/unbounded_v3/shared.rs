//! Shared core of the unbounded v3 MPSC channel.
//!
//! Strict-FIFO Vyukov intrusive chain: producers publish with one
//! always-succeeding `swap` of `head` — the single shared atomic RMW per send,
//! which is the floor any linearizable multi-producer order requires. Every
//! other cost is handle-local: nodes are bump-allocated from per-handle slabs
//! (see `producer.rs`), `len()` is tracked with sharded monotonic counters,
//! and there is no send-waiter machinery at all because sends never block.

use crate::error::{RecvError, TryRecvError};
use crate::internal::cache_padded::CachePadded;

use std::cell::UnsafeCell;
use std::fmt;
use std::ptr;
use std::sync::atomic::{fence, AtomicBool, AtomicPtr, AtomicU32, AtomicUsize, Ordering};
use std::task::{Context, Poll, Waker};
use std::thread::Thread;

use parking_lot::Mutex;

/// Nodes per slab: one heap allocation per this many sends per handle.
pub(crate) const SLAB_NODES: usize = 128;

/// Fixed number of `sent` counter shards backing `len()`. Handles take a shard
/// round-robin at creation, so with up to this many live handles each producer
/// counts on a private cache line.
const LEN_SHARDS: usize = 16;

// --- Node & Slab ------------------------------------------------------------

pub(crate) struct Node<T> {
  pub(crate) next: AtomicPtr<Node<T>>,
  /// Owning slab, for retirement accounting; null for the stub node (a plain
  /// `Box` owned by the chain).
  slab: *mut Slab<T>,
  /// `Option` so the slab's drop glue can never double-drop a consumed value:
  /// the consumer `take()`s it out, leaving `None` behind.
  pub(crate) val: UnsafeCell<Option<T>>,
}

/// A flat run of nodes handed to exactly one producer handle for bump
/// allocation, freed once every node in it has been retired.
pub(crate) struct Slab<T> {
  /// Starts at `SLAB_NODES + 1`: one count per node plus one hold for the
  /// producer. The consumer releases one count per node it retires; the
  /// producer releases the hold — plus one count per never-used node — when it
  /// seals the slab (on exhaustion or handle close/drop). Whoever reaches zero
  /// frees the slab. Arc-style ordering: `fetch_sub(Release)` with an
  /// `Acquire` fence before the free.
  remaining: AtomicU32,
  nodes: Box<[Node<T>]>,
}

/// Allocates a fresh slab and returns it with the base pointer of its node
/// array (captured once, like `bounded_queue`'s `buf_start`).
pub(crate) fn alloc_slab<T>() -> (*mut Slab<T>, *mut Node<T>) {
  let slab = Box::into_raw(Box::new(Slab {
    remaining: AtomicU32::new(SLAB_NODES as u32 + 1),
    nodes: Box::default(),
  }));
  let mut nodes = Vec::with_capacity(SLAB_NODES);
  for _ in 0..SLAB_NODES {
    nodes.push(Node {
      next: AtomicPtr::new(ptr::null_mut()),
      slab,
      val: UnsafeCell::new(None),
    });
  }
  unsafe {
    (*slab).nodes = nodes.into_boxed_slice();
    let base = (*slab).nodes.as_mut_ptr();
    (slab, base)
  }
}

/// Retires a node the consumer has advanced past. The stub is a plain `Box`;
/// slab nodes release one slab count, freeing the slab on zero.
///
/// # Safety
/// `node` must be a chain node the caller exclusively owns the retirement of
/// (each node is retired exactly once, by the single consumer or by
/// `MpscShared::drop`).
pub(crate) unsafe fn retire_node<T>(node: *mut Node<T>) {
  unsafe {
    let slab = (*node).slab;
    if slab.is_null() {
      drop(Box::from_raw(node));
    } else if (*slab).remaining.fetch_sub(1, Ordering::Release) == 1 {
      fence(Ordering::Acquire);
      drop(Box::from_raw(slab));
    }
  }
}

/// Producer-side seal: releases the producer hold plus one count per
/// never-used node. Must be called exactly once per slab.
///
/// # Safety
/// `slab` must have been produced by `alloc_slab`, `used` must be the number
/// of nodes actually handed out from it, and no further nodes may be bumped
/// from it after sealing.
pub(crate) unsafe fn seal_slab<T>(slab: *mut Slab<T>, used: usize) {
  let release = (SLAB_NODES - used) as u32 + 1;
  unsafe {
    if (*slab).remaining.fetch_sub(release, Ordering::Release) == release {
      fence(Ordering::Acquire);
      drop(Box::from_raw(slab));
    }
  }
}

// --- Shared State -----------------------------------------------------------

pub(crate) struct MpscShared<T> {
  /// Producers swap here — the single shared RMW per send.
  head: CachePadded<AtomicPtr<Node<T>>>,
  /// Consumer-only cursor. Exclusivity is guaranteed by the receiver handle
  /// rules: `Receiver` is `!Sync`, `AsyncReceiver` takes `&mut self` on every
  /// receive method, and both are `!Clone`.
  tail: CachePadded<UnsafeCell<*mut Node<T>>>,

  receiver_dropped: AtomicBool,
  sender_count: AtomicUsize,

  // Receiver wake machinery (same proven shape as `bounded_queue`; there are
  // no send waiters because sends never block).
  sync_recv_waiter: Mutex<Option<(Thread, *const AtomicBool)>>,
  async_recv_waiter: Mutex<Option<(Waker, *const AtomicBool)>>,
  sync_recv_waiter_count: CachePadded<AtomicUsize>,
  async_recv_waiter_count: CachePadded<AtomicUsize>,

  // len(): monotonic producer-side shards vs one monotonic consumer-side
  // counter. Approximate by design, like every mpsc `len()`.
  sent_shards: [CachePadded<AtomicUsize>; LEN_SHARDS],
  shard_cursor: AtomicUsize,
  consumed: CachePadded<AtomicUsize>,
}

unsafe impl<T: Send> Send for MpscShared<T> {}
unsafe impl<T: Send> Sync for MpscShared<T> {}

impl<T> fmt::Debug for MpscShared<T> {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("MpscShared")
      .field("sender_count", &self.sender_count.load(Ordering::Relaxed))
      .field(
        "receiver_dropped",
        &self.receiver_dropped.load(Ordering::Relaxed),
      )
      .finish_non_exhaustive()
  }
}

impl<T: Send> MpscShared<T> {
  pub(crate) fn new() -> Self {
    let stub = Box::into_raw(Box::new(Node {
      next: AtomicPtr::new(ptr::null_mut()),
      slab: ptr::null_mut(),
      val: UnsafeCell::new(None),
    }));
    MpscShared {
      head: CachePadded::new(AtomicPtr::new(stub)),
      tail: CachePadded::new(UnsafeCell::new(stub)),
      receiver_dropped: AtomicBool::new(false),
      sender_count: AtomicUsize::new(1),
      sync_recv_waiter: Mutex::new(None),
      async_recv_waiter: Mutex::new(None),
      sync_recv_waiter_count: CachePadded::new(AtomicUsize::new(0)),
      async_recv_waiter_count: CachePadded::new(AtomicUsize::new(0)),
      sent_shards: std::array::from_fn(|_| CachePadded::new(AtomicUsize::new(0))),
      shard_cursor: AtomicUsize::new(0),
      consumed: CachePadded::new(AtomicUsize::new(0)),
    }
  }

  // --- Handle bookkeeping ---------------------------------------------------

  pub(crate) fn next_shard(&self) -> usize {
    self.shard_cursor.fetch_add(1, Ordering::Relaxed) % LEN_SHARDS
  }

  #[inline]
  pub(crate) fn record_sent(&self, shard: usize, n: usize) {
    self.sent_shards[shard].fetch_add(n, Ordering::Relaxed);
  }

  #[inline]
  pub(crate) fn senders_alive(&self) -> bool {
    self.sender_count.load(Ordering::Acquire) != 0
  }

  #[inline]
  pub(crate) fn receivers_alive(&self) -> bool {
    !self.receiver_dropped.load(Ordering::Acquire)
  }

  pub(crate) fn sender_count(&self) -> usize {
    self.sender_count.load(Ordering::Relaxed)
  }

  pub(crate) fn add_sender(&self) {
    self.sender_count.fetch_add(1, Ordering::Relaxed);
  }

  pub(crate) fn drop_sender(&self) {
    if self.sender_count.fetch_sub(1, Ordering::AcqRel) == 1 {
      self.wake_all_receivers();
    }
  }

  pub(crate) fn drop_receiver(&self) {
    self.receiver_dropped.store(true, Ordering::Release);
    // Sends never block, so there are no senders to wake.
  }

  pub(crate) fn len(&self) -> usize {
    let sent: usize = self
      .sent_shards
      .iter()
      .map(|s| s.load(Ordering::Relaxed))
      .sum();
    sent.saturating_sub(self.consumed.load(Ordering::Relaxed))
  }

  /// Accurate emptiness from the consumer's side. Consumer-exclusive (see the
  /// `tail` field docs).
  pub(crate) fn consumer_is_empty(&self) -> bool {
    unsafe {
      let tail = *self.tail.get();
      (*tail).next.load(Ordering::Acquire).is_null()
    }
  }

  // --- Publish (producer side) ----------------------------------------------

  /// Publishes a pre-linked run of nodes `first..=last` (a single node passes
  /// itself as both). This is v1/bounded's exact Vyukov pair: the
  /// always-succeeding swap orders the run into the global FIFO, then the old
  /// head is linked to it.
  pub(crate) fn publish(&self, first: *mut Node<T>, last: *mut Node<T>) {
    let old = self.head.swap(last, Ordering::AcqRel);
    unsafe { (*old).next.store(first, Ordering::Release) };
  }

  // --- Consume (consumer-exclusive) ------------------------------------------

  /// Pops one item, or `None` if the chain looks empty from the consumer's
  /// view (a producer mid-publish counts as empty; callers re-check per the
  /// register-then-recheck discipline).
  unsafe fn pop_node(&self) -> Option<T> {
    unsafe {
      let tail = *self.tail.get();
      let next = (*tail).next.load(Ordering::Acquire);
      if next.is_null() {
        return None;
      }
      let value = (*(*next).val.get()).take().unwrap();
      *self.tail.get() = next;
      retire_node(tail);
      self.consumed.fetch_add(1, Ordering::Relaxed);
      Some(value)
    }
  }

  pub(crate) fn try_recv_internal(&self) -> Result<T, TryRecvError> {
    if let Some(v) = unsafe { self.pop_node() } {
      return Ok(v);
    }
    if !self.senders_alive() {
      // A sender may have published right before dropping; drain once more.
      if let Some(v) = unsafe { self.pop_node() } {
        return Ok(v);
      }
      return Err(TryRecvError::Disconnected);
    }
    Err(TryRecvError::Empty)
  }

  pub(crate) fn try_recv_batch_internal(
    &self,
    out: &mut Vec<T>,
    max: usize,
  ) -> Result<usize, TryRecvError> {
    if max == 0 {
      return Ok(0);
    }
    let _ = out.try_reserve_exact(max);
    let k = self.pop_batch(out, max);
    if k > 0 {
      return Ok(k);
    }
    if !self.senders_alive() {
      let k = self.pop_batch(out, max);
      if k > 0 {
        return Ok(k);
      }
      return Err(TryRecvError::Disconnected);
    }
    Err(TryRecvError::Empty)
  }

  fn pop_batch(&self, out: &mut Vec<T>, max: usize) -> usize {
    let mut got = 0;
    while got < max {
      match unsafe { self.pop_node() } {
        Some(v) => {
          out.push(v);
          got += 1;
        }
        None => break,
      }
    }
    got
  }

  pub(crate) fn poll_recv_internal(
    &self,
    cx: &mut Context<'_>,
    is_registered: &mut bool,
  ) -> Poll<Result<T, RecvError>> {
    loop {
      if let Some(v) = unsafe { self.pop_node() } {
        if *is_registered {
          self.unregister_async_recv();
          *is_registered = false;
        }
        return Poll::Ready(Ok(v));
      }
      if !self.senders_alive() {
        if let Some(v) = unsafe { self.pop_node() } {
          if *is_registered {
            self.unregister_async_recv();
            *is_registered = false;
          }
          return Poll::Ready(Ok(v));
        }
        if *is_registered {
          self.unregister_async_recv();
          *is_registered = false;
        }
        return Poll::Ready(Err(RecvError::Disconnected));
      }

      self.register_async_recv(cx.waker().clone(), ptr::null());
      *is_registered = true;
      self.pre_park_fence();

      if let Some(v) = unsafe { self.pop_node() } {
        self.unregister_async_recv();
        *is_registered = false;
        return Poll::Ready(Ok(v));
      }
      if !self.senders_alive() {
        continue;
      }
      return Poll::Pending;
    }
  }

  pub(crate) fn poll_recv_batch_internal(
    &self,
    cx: &mut Context<'_>,
    out: &mut Vec<T>,
    max: usize,
    is_registered: &mut bool,
  ) -> Poll<Result<usize, RecvError>> {
    if max == 0 {
      return Poll::Ready(Ok(0));
    }
    match self.try_recv_batch_internal(out, max) {
      Ok(k) => {
        if *is_registered {
          self.unregister_async_recv();
          *is_registered = false;
        }
        Poll::Ready(Ok(k))
      }
      Err(TryRecvError::Disconnected) => {
        if *is_registered {
          self.unregister_async_recv();
          *is_registered = false;
        }
        Poll::Ready(Err(RecvError::Disconnected))
      }
      Err(TryRecvError::Empty) => {
        self.register_async_recv(cx.waker().clone(), ptr::null());
        *is_registered = true;
        self.pre_park_fence();
        match self.try_recv_batch_internal(out, max) {
          Ok(k) => {
            self.unregister_async_recv();
            *is_registered = false;
            Poll::Ready(Ok(k))
          }
          Err(TryRecvError::Disconnected) => {
            self.unregister_async_recv();
            *is_registered = false;
            Poll::Ready(Err(RecvError::Disconnected))
          }
          Err(TryRecvError::Empty) => Poll::Pending,
        }
      }
    }
  }

  // --- Waiter machinery (receiver side only) ----------------------------------

  pub(crate) fn register_sync_recv(&self, thread: Thread, notified: *const AtomicBool) {
    *self.sync_recv_waiter.lock() = Some((thread, notified));
    self.sync_recv_waiter_count.store(1, Ordering::Release);
  }

  pub(crate) fn unregister_sync_recv(&self) {
    *self.sync_recv_waiter.lock() = None;
    self.sync_recv_waiter_count.store(0, Ordering::Release);
  }

  fn register_async_recv(&self, waker: Waker, notified: *const AtomicBool) {
    *self.async_recv_waiter.lock() = Some((waker, notified));
    self.async_recv_waiter_count.store(1, Ordering::Release);
  }

  pub(crate) fn unregister_async_recv(&self) {
    *self.async_recv_waiter.lock() = None;
    self.async_recv_waiter_count.store(0, Ordering::Release);
  }

  /// Called by every sender after publishing. Only one of the two slots is
  /// ever populated (there's one receiver), so the other check is a cheap
  /// Relaxed-load no-op.
  pub(crate) fn notify_receiver(&self) {
    fence(Ordering::SeqCst);
    if self.sync_recv_waiter_count.load(Ordering::Relaxed) != 0 {
      let mut g = self.sync_recv_waiter.lock();
      if let Some((thread, notified)) = g.take() {
        self.sync_recv_waiter_count.store(0, Ordering::Release);
        if !notified.is_null() {
          unsafe { (*notified).store(true, Ordering::Release) };
        }
        drop(g);
        thread.unpark();
      }
    }
    if self.async_recv_waiter_count.load(Ordering::Relaxed) != 0 {
      let mut g = self.async_recv_waiter.lock();
      if let Some((waker, notified)) = g.take() {
        self.async_recv_waiter_count.store(0, Ordering::Release);
        if !notified.is_null() {
          unsafe { (*notified).store(true, Ordering::Release) };
        }
        drop(g);
        waker.wake();
      }
    }
  }

  fn wake_all_receivers(&self) {
    if let Some((thread, notified)) = self.sync_recv_waiter.lock().take() {
      self.sync_recv_waiter_count.store(0, Ordering::Release);
      if !notified.is_null() {
        unsafe { (*notified).store(true, Ordering::Release) };
      }
      thread.unpark();
    }
    if let Some((waker, notified)) = self.async_recv_waiter.lock().take() {
      self.async_recv_waiter_count.store(0, Ordering::Release);
      if !notified.is_null() {
        unsafe { (*notified).store(true, Ordering::Release) };
      }
      waker.wake();
    }
  }

  #[inline]
  pub(crate) fn pre_park_fence(&self) {
    fence(Ordering::SeqCst);
  }
}

impl<T> Drop for MpscShared<T> {
  fn drop(&mut self) {
    // Drain every published item (dropping values, retiring nodes and slabs),
    // then retire the final tail node — retirement always runs one behind, so
    // exactly one node remains when the chain is empty.
    unsafe {
      loop {
        let tail = *self.tail.get();
        let next = (*tail).next.load(Ordering::Relaxed);
        if next.is_null() {
          break;
        }
        drop((*(*next).val.get()).take());
        *self.tail.get() = next;
        retire_node(tail);
      }
      retire_node(*self.tail.get());
    }
  }
}
