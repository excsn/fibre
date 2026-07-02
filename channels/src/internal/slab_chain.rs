//! The slab-backed Vyukov intrusive chain shared by the unbounded MPSC and
//! MPMC channels.
//!
//! Producers publish with one always-succeeding `swap` of the chain head — the
//! single shared atomic RMW that any strict (linearizable) multi-producer FIFO
//! requires — and bump-allocate nodes from per-handle slabs so concurrent
//! producers write into disjoint memory. Each slab is released once every node
//! in it has been retired, via an Arc-style refcount, so the retirement side
//! is safe for one consumer or many. Fully-retired slabs recycle through a
//! small per-channel [`SlabPool`] instead of round-tripping the allocator:
//! flamegraphs showed malloc/free (producer-allocates, consumer-frees) as a
//! dominant multi-producer cost, and the clean A/B (2026-07-02, both sides at
//! 512 nodes) showed removing the pool regressed mpmc async 1P by 13-17%.
//!
//! The consumer-side release is a bare pointer push — it can run under the
//! mpmc consumer mutex, a measured lock-hold amplifier — while the node
//! re-arm walk runs on the producer side in `acquire`.

use crate::internal::cache_padded::CachePadded;

use std::cell::UnsafeCell;
use std::ptr;
use std::sync::atomic::{fence, AtomicPtr, AtomicU32, Ordering};
use std::sync::Arc;

use parking_lot::Mutex;

/// Nodes per slab: one slab acquisition per this many sends per handle.
/// Size sweep (2026-07-02): mpmc is flat across 256-1024 but its 1P/14C
/// convoy collapses at 128 (frequent consumer-side release under the consumer
/// mutex); mpsc multi-producer degrades at 512 (larger hot working set).
/// 256 is the point aimed at satisfying both.
pub(crate) const SLAB_NODES: usize = 128;

/// Retired slabs kept for reuse; beyond this the allocator gets them back, so
/// a queue-depth burst doesn't pin its high-water memory forever.
const SLAB_POOL_CAP: usize = 8;

pub(crate) struct Node<T> {
  pub(crate) next: AtomicPtr<Node<T>>,
  /// Owning slab, for retirement accounting; null for a stub node (a plain
  /// `Box` owned by the chain).
  slab: *mut Slab<T>,
  /// `Option` so the slab's drop glue can never double-drop a consumed value:
  /// the consumer `take()`s it out, leaving `None` behind.
  pub(crate) val: UnsafeCell<Option<T>>,
}

/// A flat run of nodes handed to exactly one producer handle for bump
/// allocation, released to its pool once every node in it has been retired.
pub(crate) struct Slab<T> {
  /// Starts at `SLAB_NODES + 1`: one count per node plus one hold for the
  /// producer. Retirement releases one count per node; the producer releases
  /// the hold — plus one count per never-used node — when it seals the slab
  /// (on exhaustion or handle close/drop). Whoever reaches zero releases the
  /// slab. Arc-style ordering: `fetch_sub(Release)` with an `Acquire` fence
  /// before the release. The decrements are atomic, so retirement is safe
  /// from a single consumer (mpsc) or from multiple (mpmc).
  remaining: AtomicU32,
  /// The pool this slab returns to when fully retired. Points into the
  /// pool's own `Arc` allocation (never into channel shared state, so the
  /// shared state's `Drop` drain doesn't alias `&mut self`), and every
  /// retire/seal runs while something still holds that `Arc` — a sender
  /// handle or the channel's shared state — so it cannot dangle.
  pool: *const SlabPool<T>,
  nodes: Box<[Node<T>]>,
}

// --- Slab pool --------------------------------------------------------------------

/// Per-channel recycling pool for fully-retired slabs. Pool operations happen
/// once per `SLAB_NODES` sends, so a plain mutex is amortized to noise.
pub(crate) struct SlabPool<T> {
  slabs: Mutex<Vec<*mut Slab<T>>>,
}

unsafe impl<T: Send> Send for SlabPool<T> {}
unsafe impl<T: Send> Sync for SlabPool<T> {}

impl<T> SlabPool<T> {
  pub(crate) fn new() -> Self {
    SlabPool {
      slabs: Mutex::new(Vec::new()),
    }
  }
}

impl<T: Send> SlabPool<T> {
  /// Hands out a slab: a pooled one (re-armed) if available, else fresh.
  fn acquire(&self) -> (*mut Slab<T>, *mut Node<T>) {
    let recycled = self.slabs.lock().pop();
    match recycled {
      Some(slab) => unsafe {
        (*slab).remaining.store(SLAB_NODES as u32 + 1, Ordering::Relaxed);
        let base = (*slab).nodes.as_mut_ptr();
        for i in 0..SLAB_NODES {
          let node = base.add(i);
          (*node).next.store(ptr::null_mut(), Ordering::Relaxed);
          // Every retired node's value was already taken (or never written);
          // assigning (rather than skipping) keeps that an invariant instead
          // of a soundness requirement.
          *(*node).val.get() = None;
        }
        (slab, base)
      },
      None => alloc_slab(self),
    }
  }
}

impl<T> Drop for SlabPool<T> {
  fn drop(&mut self) {
    for slab in self.slabs.get_mut().drain(..) {
      unsafe { drop(Box::from_raw(slab)) };
    }
  }
}

/// Returns a fully-retired slab to its pool, or frees it if the pool is full.
///
/// # Safety
/// The slab's refcount must have reached zero, with the caller owning the
/// final release.
unsafe fn release_slab<T>(slab: *mut Slab<T>) {
  unsafe {
    let pool = &*(*slab).pool;
    let mut pooled = pool.slabs.lock();
    if pooled.len() < SLAB_POOL_CAP {
      pooled.push(slab);
    } else {
      drop(pooled);
      drop(Box::from_raw(slab));
    }
  }
}

/// Allocates a fresh slab owned by `pool` and returns it with the base
/// pointer of its node array (captured once, like `bounded_queue`'s
/// `buf_start`).
fn alloc_slab<T>(pool: &SlabPool<T>) -> (*mut Slab<T>, *mut Node<T>) {
  let slab = Box::into_raw(Box::new(Slab {
    remaining: AtomicU32::new(SLAB_NODES as u32 + 1),
    pool: pool as *const SlabPool<T>,
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

/// Allocates the initial stub node (null `slab`), installed as head+tail.
pub(crate) fn alloc_stub<T>() -> *mut Node<T> {
  Box::into_raw(Box::new(Node {
    next: AtomicPtr::new(ptr::null_mut()),
    slab: ptr::null_mut(),
    val: UnsafeCell::new(None),
  }))
}

/// Retires a node the consumer side has advanced past. The stub is a plain
/// `Box`; slab nodes release one slab count, recycling the slab on zero.
///
/// # Safety
/// `node` must be a chain node the caller exclusively owns the retirement of
/// (each node is retired exactly once).
pub(crate) unsafe fn retire_node<T>(node: *mut Node<T>) {
  unsafe {
    let slab = (*node).slab;
    if slab.is_null() {
      drop(Box::from_raw(node));
    } else if (*slab).remaining.fetch_sub(1, Ordering::Release) == 1 {
      fence(Ordering::Acquire);
      release_slab(slab);
    }
  }
}

/// Producer-side seal: releases the producer hold plus one count per
/// never-used node. Must be called exactly once per slab.
///
/// # Safety
/// `slab` must come from this module's allocation path, `used` must be the
/// number of nodes actually handed out from it, and no further nodes may be
/// bumped from it after sealing.
unsafe fn seal_slab<T>(slab: *mut Slab<T>, used: usize) {
  let release = (SLAB_NODES - used) as u32 + 1;
  unsafe {
    if (*slab).remaining.fetch_sub(release, Ordering::Release) == release {
      fence(Ordering::Acquire);
      release_slab(slab);
    }
  }
}

/// Producer-private bump state, owned directly by each sender handle. The
/// send methods take `&mut self`, so the borrow checker provides the
/// exclusivity (and per-handle FIFO) that previously required a per-handle
/// lock.
pub(crate) struct ProducerSlab<T> {
  slab: *mut Slab<T>,
  base: *mut Node<T>,
  pos: usize,
  /// Shared with the channel and every sibling handle; keeps the pool alive
  /// for as long as this handle can still seal a slab into it.
  pool: Arc<SlabPool<T>>,
}

unsafe impl<T: Send> Send for ProducerSlab<T> {}

impl<T: Send> ProducerSlab<T> {
  pub(crate) fn new(pool: Arc<SlabPool<T>>) -> Self {
    ProducerSlab {
      slab: ptr::null_mut(),
      base: ptr::null_mut(),
      pos: 0,
      pool,
    }
  }

  /// Hands out the next free node (fresh: `next == null`, `val == None`,
  /// slab back-pointer set), sealing/acquiring slabs as needed.
  pub(crate) fn bump(&mut self) -> *mut Node<T> {
    if self.slab.is_null() || self.pos == SLAB_NODES {
      if !self.slab.is_null() {
        // Exhausted: release only the producer hold.
        unsafe { seal_slab(self.slab, SLAB_NODES) };
      }
      let (slab, base) = self.pool.acquire();
      self.slab = slab;
      self.base = base;
      self.pos = 0;
    }
    let node = unsafe { self.base.add(self.pos) };
    self.pos += 1;
    node
  }

  /// Bumps and pre-links `count` nodes with their values written (Relaxed
  /// link stores; runs may span slabs). Returns `(first, last)` for a single
  /// `publish`. The iterator must yield at least `count` items.
  pub(crate) fn bump_batch(
    &mut self,
    iter: &mut impl Iterator<Item = T>,
    count: usize,
  ) -> (*mut Node<T>, *mut Node<T>) {
    debug_assert!(count > 0);
    let expect_msg = "bump_batch: iterator yielded fewer than `count` items";
    let first = self.bump();
    unsafe { *(*first).val.get() = Some(iter.next().expect(expect_msg)) };
    let mut prev = first;
    for _ in 1..count {
      let node = self.bump();
      unsafe {
        *(*node).val.get() = Some(iter.next().expect(expect_msg));
        (*prev).next.store(node, Ordering::Relaxed);
      }
      prev = node;
    }
    (first, prev)
  }

  /// Handle close/drop: seals the partial slab exactly once. Safe to call on
  /// an already-sealed (null) state.
  pub(crate) fn seal(&mut self) {
    if !self.slab.is_null() {
      unsafe { seal_slab(self.slab, self.pos) };
      self.slab = ptr::null_mut();
      self.base = ptr::null_mut();
      self.pos = 0;
    }
  }
}

/// The producers' end of the chain.
pub(crate) struct ChainHead<T> {
  head: CachePadded<AtomicPtr<Node<T>>>,
}

impl<T> ChainHead<T> {
  pub(crate) fn new(stub: *mut Node<T>) -> Self {
    ChainHead {
      head: CachePadded::new(AtomicPtr::new(stub)),
    }
  }

  /// Publishes a pre-linked run of nodes `first..=last` (a single node passes
  /// itself as both): an always-succeeding swap orders the run into the
  /// global FIFO, then the old head is linked to it.
  pub(crate) fn publish(&self, first: *mut Node<T>, last: *mut Node<T>) {
    let old = self.head.swap(last, Ordering::AcqRel);
    unsafe { (*old).next.store(first, Ordering::Release) };
  }
}
