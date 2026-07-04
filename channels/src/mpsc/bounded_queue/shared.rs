//! Shared core of the bounded MPSC: the pre-allocated node pool, the intrusive
//! Vyukov MPSC list (`BoundedQueue`), the per-handle `LocalCache`, the
//! generational chunk-recycling stack (`ChunkStack`), waiter coordination, and
//! the common `publish_run` send tail. Producer/consumer handles live in
//! `super::producer` / `super::consumer`; miri custody scaffolding in
//! `super::miri`.
//!
//! The publish path is CAS-free: producers enqueue onto an intrusive Vyukov
//! MPSC linked list with a single always-succeeding `atomic::swap`, so
//! throughput stays flat as producers are added. All nodes are pre-allocated
//! up front in one flat heap array; the send path never allocates.
//!
//! Capacity is enforced by node exhaustion rather than a semaphore: a send
//! first acquires a free node from the shared pool (the `Mutex<LocalCache>`
//! all senders share), and "full" simply means no free node is available.
//! The receiver recycles consumed nodes through its own single-threaded cache
//! and flushes them back in chunks (up to `CACHE_FLUSH_CHUNK`) onto a
//! lock-free, generation-tagged index stack (`ChunkStack`), so the shared
//! free list is touched once per chunk instead of once per item. That flush
//! is also the wake point for senders blocked on capacity.
//!
//! Sync and async waiters live in entirely separate queues so OS threads and
//! tasks never contend on each other's mutex; sync senders are woken in
//! batches sized by how many nodes a flush released, async senders one at a
//! time (see `SyncSendWaiters` and `notify_sync_senders`).

// Payload cells stay on std::cell::UnsafeCell (see internal/sync.rs — loom's
// closure-API cell is out of scope for now; miri covers payload races).
use std::cell::UnsafeCell;
use std::collections::VecDeque;
use std::task::{Context, Poll, Waker};

use crate::internal::sync::{
  fence, hint, AtomicBool, AtomicPtr, AtomicU64, AtomicUsize, Mutex, Ordering, Thread, thread,
};

use crate::error::RecvError;
use crate::internal::cache_padded::CachePadded;

use super::miri::custody;
#[cfg(miri)]
use super::miri;

pub(crate) const SENTINEL_IDX: u32 = u32::MAX;

// Upper bound on how many recycled nodes a receiver hoards before flushing
// them back to the shared chunk stack. Without this cap the threshold
// scaled with the full queue capacity, so on large-capacity queues senders
// would starve waiting for the receiver to accumulate hundreds of freed
// nodes before releasing any of them back.
pub(crate) const CACHE_FLUSH_CHUNK: usize = 64;

// Bounded spin before a sync park, applied ONLY at capacity 1: cap-1
// ping-pong hits a full/empty boundary on every item, and the partner's next
// step lands inside a short spin window almost every time — a real park
// costs an unpark round trip per item instead (~15x throughput collapse;
// the original SYNC_SPIN_LIMIT was dropped by accident in f35e867's waiter
// rework). At larger capacities pre-park spinning measurably hurt contended
// throughput in past experiments, so it stays cap-1-only.
//
// Under loom the budget collapses to 1: every tracked op in a spin iteration
// counts against loom's branch cap, and parking is the path we want modeled.
pub(crate) const SYNC_SPIN_LIMIT: usize = if crate::internal::sync::IS_LOOM { 1 } else { 200 };

/// Cap-1-only bounded spin on the wake flag before a sync park. May observe
/// the wake early; the caller's usual `notified` handling covers both
/// outcomes (a skipped park leaves at most one pending park token, which the
/// retry loops already treat as a spurious wake).
#[inline]
pub(crate) fn spin_before_park_cap1(cap: usize, notified: &AtomicBool) {
  if cap != 1 {
    return;
  }
  for _ in 0..SYNC_SPIN_LIMIT {
    if notified.load(Ordering::Relaxed) {
      return;
    }
    hint::spin_loop();
  }
}

// --- Private Waiter Coordination ---
//
// Sync and async waiters live in entirely separate queues/slots (mirroring
// mpmc_v2's SyncWaiter/AsyncWaiter split) rather than one shared queue
// holding a Sync/Async enum. This means sync's OS threads and async's tasks
// never contend on the same mutex at all: sync senders only ever lock
// `sync_send_waiters`, async senders only ever lock `async_send_waiters`,
// and likewise on the receive side. Each side is free to pick its own wake
// policy later without any risk of it affecting the other.

pub(crate) struct SyncSendWaiters {
  queue: VecDeque<(u64, Thread, *const AtomicBool)>,
  next_id: u64,
}

pub(crate) struct AsyncSendWaiters {
  queue: VecDeque<(u64, Waker, *const AtomicBool)>,
  next_id: u64,
}

// --- Intrusive Node ---
pub struct Node<T> {
  next: AtomicPtr<Node<T>>,
  next_chunk: AtomicPtr<Node<T>>,
  pub(crate) chunk_len: u32,
  pub(crate) val: UnsafeCell<Option<T>>,
  /// Miri-only ownership canary, deliberately NON-atomic. The producer writes
  /// it when it grabs a node (`fill_pool`) and when it publishes one
  /// (`publish_run`); the consumer reads it in `follow_link`. On a legitimate
  /// follow the Acquire load of the junction synchronizes-with the producer's
  /// Release, so this read is happens-before the producer's writes and is
  /// race-free. On a ghost follow (the consumer's tail walking into
  /// producer-owned memory via a stale link) there is no such edge, so the
  /// non-atomic read/write pair is a data race miri reports at the exact
  /// instruction — no Relaxed-atomic false-negative window like the shadow
  /// table. Silence here therefore *is* meaningful (unlike the shadow checks).
  #[cfg(miri)]
  pub(crate) owner_canary: UnsafeCell<u8>,
}

// --- Generational Tagged Index Stack (Safe ABA Mitigation) ---
pub(crate) struct ChunkStack {
  // Packed 64-bit state: (generation: u32 << 32) | (head_index: u32)
  state: AtomicU64,
}

impl ChunkStack {
  fn new() -> Self {
    ChunkStack {
      state: AtomicU64::new(((0u64) << 32) | (SENTINEL_IDX as u64)),
    }
  }

  pub(crate) fn push<T>(&self, buf_start: *mut Node<T>, head_idx: u32) {
    let mut state = self.state.load(Ordering::Relaxed);
    loop {
      let generation_count = (state >> 32) as u32;
      let old_head = (state & 0xFFFF_FFFF) as u32;

      unsafe {
        let head_ptr = buf_start.add(head_idx as usize);
        (*head_ptr).next_chunk.store(
          if old_head == SENTINEL_IDX {
            std::ptr::null_mut()
          } else {
            buf_start.add(old_head as usize)
          },
          Ordering::Relaxed,
        );
      }

      let new_state = ((generation_count.wrapping_add(1) as u64) << 32) | (head_idx as u64);
      match self
        .state
        .compare_exchange_weak(state, new_state, Ordering::AcqRel, Ordering::Relaxed)
      {
        Ok(_) => break,
        Err(actual) => state = actual,
      }
    }
  }

  pub(crate) fn pop<T>(&self, buf_start: *mut Node<T>) -> Option<u32> {
    let mut state = self.state.load(Ordering::Acquire);
    loop {
      let generation_count = (state >> 32) as u32;
      let head_idx = (state & 0xFFFF_FFFF) as u32;

      if head_idx == SENTINEL_IDX {
        return None;
      }

      let head_ptr = unsafe { buf_start.add(head_idx as usize) };
      let next_ptr = unsafe { (*head_ptr).next_chunk.load(Ordering::Relaxed) };
      let next_idx = if next_ptr.is_null() {
        SENTINEL_IDX
      } else {
        unsafe { next_ptr.offset_from(buf_start) as u32 }
      };

      let new_state = ((generation_count.wrapping_add(1) as u64) << 32) | (next_idx as u64);
      match self
        .state
        // Failure ordering MUST be Acquire, not Relaxed: on a failed CAS the loop
        // re-reads `(*head_ptr).next_chunk` for the newly-observed head. Without
        // an Acquire here that read has no happens-before with the thread that
        // pushed the new head, so on a weak model it can return a stale
        // prior-lifetime `next_chunk` — the classic Treiber ABA. The CAS then
        // reinstalls a garbage stack link and two owners walk the same chunk
        // (surfaces as CHUNK SHORT / DOUBLE OWNERSHIP / the chunk_len data race).
        .compare_exchange_weak(state, new_state, Ordering::AcqRel, Ordering::Acquire)
      {
        Ok(_) => return Some(head_idx),
        Err(actual) => state = actual,
      }
    }
  }
}

// --- BoundedQueue Shared Core ---
pub struct BoundedQueue<T: Send> {
  // Flat up-front heap allocation, held raw (`Box::into_raw`) and freed in
  // `Drop`. Storing the owning `Box` here would be UB under Stacked Borrows:
  // every move of a `Box` (into this struct, then into the `Arc`) is a fresh
  // Unique retag that invalidates `buf_start` and every node pointer derived
  // from it.
  pub(crate) buf: *mut [Node<T>],
  pub(crate) buf_start: *mut Node<T>,
  pub(crate) cap: usize,

  // Intrusive MPSC cursors
  pub(crate) head: CachePadded<AtomicPtr<Node<T>>>,
  pub(crate) tail: CachePadded<UnsafeCell<*mut Node<T>>>,

  /// Shadow custody table (miri only): per-node state, 0=FREE (pool/free
  /// chain), 1=PUBLISHED, 2=CONSUMED, updated with Relaxed atomics so it adds
  /// NO happens-before edges. On every legitimate transition the real
  /// protocol's own Release/Acquire also carries the shadow store, so a stale
  /// shadow read is only possible when synchronization is absent — the check
  /// panics at the FIRST custody violation (ghost follow, double publish,
  /// double free) with the node and its state, instead of the downstream
  /// symptom (val race / wrong value / truncated free chain).
  #[cfg(miri)]
  pub(crate) shadow: Box<[std::sync::atomic::AtomicU32]>,

  // Tagged Chunk Recycling Stack
  pub(crate) chunk_stack: ChunkStack,

  // Handle coordination. Sync and async waiters are kept in entirely
  // separate queues/slots (see the comment above `SyncSendWaiters`) so
  // neither kind ever contends on the other's mutex.
  pub(crate) sender_count: CachePadded<AtomicUsize>,
  pub(crate) receiver_dropped: CachePadded<AtomicBool>,
  pub(crate) sync_send_waiters: Mutex<SyncSendWaiters>,
  pub(crate) async_send_waiters: Mutex<AsyncSendWaiters>,
  /// Serializes sync producers entering the blocking path, at CAP-1 ONLY:
  /// at most one sync sender is ever registered/parked in
  /// `sync_send_waiters`; the rest queue here, waiting on each other instead
  /// of on the consumer. At cap-1 each release frees one node, so this keeps
  /// the consumer's per-item `notify_sync_senders` at a single relaxed load
  /// of a 0-or-1 count (250 K/s -> ~1 M/s at Cap-1/Prod-14). At caps >= 2 a
  /// release frees a whole chunk and the batch wake wants that many
  /// producers unparked in parallel — gating there serializes the wakes into
  /// a producer-to-producer cond_signal chain (see `allocate_node`).
  /// Sync-only: async senders and the consumer never touch it.
  pub(crate) park_gate: Mutex<()>,
  pub(crate) sync_send_waiter_count: CachePadded<AtomicUsize>,
  pub(crate) async_send_waiter_count: CachePadded<AtomicUsize>,
  pub(crate) sync_recv_waiter: Mutex<Option<(Thread, *const AtomicBool)>>,
  pub(crate) async_recv_waiter: Mutex<Option<(Waker, *const AtomicBool)>>,
  pub(crate) sync_recv_waiter_count: CachePadded<AtomicUsize>,
  pub(crate) async_recv_waiter_count: CachePadded<AtomicUsize>,
  pub(crate) current_len: CachePadded<AtomicUsize>,

  // Shared sender cache. Capacity is a single chunk, so every Sender/
  // AsyncSender clone locks this directly instead of being assigned a bucket.
  pub(crate) cache: CachePadded<Mutex<LocalCache<T>>>,
}

unsafe impl<T: Send> Send for BoundedQueue<T> {}
unsafe impl<T: Send> Sync for BoundedQueue<T> {}

impl<T: Send> BoundedQueue<T> {
  pub fn new(capacity: usize) -> Self {
    assert!(capacity > 0, "Queue capacity must be greater than 0");
    let total_nodes = capacity + 1; // +1 for the Vyukov stub node

    let mut nodes = Vec::with_capacity(total_nodes);
    for _ in 0..total_nodes {
      nodes.push(Node {
        next: AtomicPtr::new(std::ptr::null_mut()),
        next_chunk: AtomicPtr::new(std::ptr::null_mut()),
        chunk_len: 0,
        val: UnsafeCell::new(None),
        #[cfg(miri)]
        owner_canary: UnsafeCell::new(miri::OWNER_FREE),
      });
    }

    let buf = Box::into_raw(nodes.into_boxed_slice());
    let buf_start = buf as *mut Node<T>;

    let stub_ptr = buf_start;
    let chunk_stack = ChunkStack::new();

    // The whole capacity is a single chunk, starting right after the stub node.
    let start_idx = 1u32;
    let end_idx = capacity as u32;
    for i in start_idx..end_idx {
      unsafe {
        (*buf_start.add(i as usize))
          .next
          .store(buf_start.add(i as usize + 1), Ordering::Relaxed);
      }
    }
    unsafe {
      (*buf_start.add(start_idx as usize)).chunk_len = capacity as u32;
    }
    chunk_stack.push(buf_start, start_idx);

    BoundedQueue {
      buf,
      buf_start,
      cap: capacity,
      #[cfg(miri)]
      shadow: (0..total_nodes)
        .map(|_| std::sync::atomic::AtomicU32::new(0))
        .collect(),
      head: CachePadded::new(AtomicPtr::new(stub_ptr)),
      tail: CachePadded::new(UnsafeCell::new(stub_ptr)),
      chunk_stack,
      sender_count: CachePadded::new(AtomicUsize::new(1)),
      receiver_dropped: CachePadded::new(AtomicBool::new(false)),
      sync_send_waiters: Mutex::new(SyncSendWaiters {
        queue: VecDeque::new(),
        next_id: 0,
      }),
      async_send_waiters: Mutex::new(AsyncSendWaiters {
        queue: VecDeque::new(),
        next_id: 0,
      }),
      park_gate: Mutex::new(()),
      sync_send_waiter_count: CachePadded::new(AtomicUsize::new(0)),
      async_send_waiter_count: CachePadded::new(AtomicUsize::new(0)),
      sync_recv_waiter: Mutex::new(None),
      async_recv_waiter: Mutex::new(None),
      sync_recv_waiter_count: CachePadded::new(AtomicUsize::new(0)),
      async_recv_waiter_count: CachePadded::new(AtomicUsize::new(0)),
      current_len: CachePadded::new(AtomicUsize::new(0)),
      cache: CachePadded::new(Mutex::new(LocalCache::default())),
    }
  }

  #[inline]
  pub(crate) fn ptr_to_idx(&self, ptr: *mut Node<T>) -> u32 {
    unsafe { ptr.offset_from(self.buf_start) as u32 }
  }

  #[inline]
  pub(crate) fn idx_to_ptr(&self, idx: u32) -> *mut Node<T> {
    if idx == SENTINEL_IDX {
      std::ptr::null_mut()
    } else {
      unsafe { self.buf_start.add(idx as usize) }
    }
  }

  /// Producer-side free-chain hop.
  #[inline]
  pub(crate) unsafe fn next_of(&self, node: *mut Node<T>) -> *mut Node<T> {
    unsafe { (*node).next.load(Ordering::Relaxed) }
  }

  /// Consumer-only. Returns the next live node if `tail` has been linked
  /// forward, else None.
  #[inline]
  pub(crate) unsafe fn follow_link(&self, tail: *mut Node<T>) -> Option<*mut Node<T>> {
    let next = unsafe { (*tail).next.load(Ordering::Acquire) };
    if next.is_null() {
      return None;
    }
    custody!(
      "follow tail={} -> next={}",
      self.ptr_to_idx(tail),
      self.ptr_to_idx(next)
    );
    // Two distinct miri custody probes (see super::miri): the logical shadow
    // assertion (PUBLISHED -> CONSUMED) and the genuine data-race canary probe.
    #[cfg(miri)]
    miri::check_follow_shadow(self, tail, next);
    #[cfg(miri)]
    miri::check_follow_canary(self, tail, next);
    Some(next)
  }

  /// Consumer-only peek for the re-check paths.
  #[inline]
  pub(crate) unsafe fn link_is_fresh(&self, tail: *mut Node<T>) -> bool {
    !unsafe { (*tail).next.load(Ordering::Acquire) }.is_null()
  }

  /// Consumer's pre-register probe: watches `tail`'s link for a fresh
  /// publish for one bounded window; true = something arrived, retry the
  /// pop instead of registering.
  ///
  /// Wait primitive is keyed on CAPACITY alone (producer count benched as a
  /// misleading proxy): what matters is whether the partner the consumer is
  /// waiting on is spinning or parked. Small cap (<= 4): producers respond
  /// in ns (they're in their own spin probes), so busy-spin — the consumer
  /// stays on-core and reacts instantly; every yield hands its core to a
  /// producer that can't progress without it (Cap-4/Prod-14: yield arm
  /// ~590 K/s vs spin ~2 M/s). Large cap: producers PARK between big
  /// recycle chunks, so the probe must bridge their ~1-3us unpark latency —
  /// only the yield arm's long window does (Cap-128 1P/4P benched −26-46%
  /// in the spin arm, 2026-07-02).
  #[inline]
  pub(crate) unsafe fn probe_for_publish(&self, tail: *mut Node<T>) -> bool {
    if self.cap <= 4 {
      for _ in 0..SYNC_SPIN_LIMIT {
        if unsafe { self.link_is_fresh(tail) } {
          return true;
        }
        hint::spin_loop();
      }
    } else {
      for _ in 0..SYNC_SPIN_LIMIT {
        if unsafe { self.link_is_fresh(tail) } {
          return true;
        }
        thread::yield_now();
      }
    }
    false
  }

  fn release_chunk(&self, chunk_head: *mut Node<T>) {
    // Read chunk_len before pushing: once it's on chunk_stack another
    // thread can pop (and even re-split, via take_nodes) it immediately.
    let released = unsafe { (*chunk_head).chunk_len } as usize;
    // Reachability check (miri): every node about to become producer-grabbable
    // must be strictly behind the consumer — never the current tail nor the
    // live `tail.next`. Panics naming a premature release BEFORE it hits the
    // chunk stack. Runs on the (single) consumer thread.
    #[cfg(miri)]
    miri::check_release_reachability(self, chunk_head, released);
    let head_idx = self.ptr_to_idx(chunk_head);
    self.chunk_stack.push(self.buf_start, head_idx);
    // One fence covers both checks below; each is otherwise just a Relaxed
    // load that returns immediately if that kind's queue is empty (the
    // common case when only one kind is in use on this queue). Sync gets a
    // batch wake bounded by how many nodes actually came back (real OS
    // threads parking means waking only one per release leaves the rest
    // sitting parked until further releases trickle through to reach them
    // individually); async is untouched and keeps waking exactly one, since
    // its wake is cheap in-process rescheduling and over-waking would just
    // cost extra poll/reschedule cycles for tasks that can't proceed.
    fence(Ordering::SeqCst);
    self.notify_sync_senders(released);
    self.notify_async_senders();
  }

  pub fn add_sender(&self) {
    self.sender_count.fetch_add(1, Ordering::Relaxed);
  }

  pub fn drop_sender(&self) {
    if self.sender_count.fetch_sub(1, Ordering::AcqRel) == 1 {
      self.wake_all_receivers();
    }
  }

  pub fn drop_receiver(&self) {
    self.receiver_dropped.store(true, Ordering::Release);
    self.wake_all_senders();
  }

  #[inline]
  pub fn senders_alive(&self) -> bool {
    self.sender_count.load(Ordering::Acquire) != 0
  }

  #[inline]
  pub fn receivers_alive(&self) -> bool {
    !self.receiver_dropped.load(Ordering::Acquire)
  }

  pub(crate) fn register_sync_send(&self, prev_id: Option<u64>, thread: Thread, notified: *const AtomicBool) -> u64 {
    let mut g = self.sync_send_waiters.lock();
    if let Some(id) = prev_id {
      g.queue.retain(|(sid, _, _)| *sid != id);
    }
    let id = g.next_id;
    g.next_id = g.next_id.wrapping_add(1);
    g.queue.push_back((id, thread, notified));
    self
      .sync_send_waiter_count
      .store(g.queue.len(), Ordering::Release);
    id
  }

  /// Returns true iff this call actually removed our entry. False means a waker
  /// already dequeued us and is (or is about to be) mid-write to our stack
  /// `notified` — see `finish_sync_send`.
  pub(crate) fn unregister_sync_send(&self, id: u64) -> bool {
    let mut g = self.sync_send_waiters.lock();
    let prev = g.queue.len();
    g.queue.retain(|(sid, _, _)| *sid != id);
    let removed = g.queue.len() != prev;
    if removed {
      self
        .sync_send_waiter_count
        .store(g.queue.len(), Ordering::Release);
    }
    removed
  }

  /// Leave the send-waiter set on the way out of `allocate_node`. If a waker
  /// already dequeued us (`unregister` returns false), it holds our raw
  /// `notified` pointer and will `store(true)` into it; we MUST NOT let this
  /// frame (and `notified`) die until that write lands, or the waker's store is
  /// a use-after-free on reclaimed stack. Spin until we observe it.
  pub(crate) fn finish_sync_send(&self, id: u64, notified: &AtomicBool) {
    if !self.unregister_sync_send(id) {
      while !notified.load(Ordering::Acquire) {
        hint::spin_loop();
      }
    }
  }

  pub(crate) fn register_async_send(&self, prev_id: Option<u64>, waker: Waker, notified: *const AtomicBool) -> u64 {
    let mut g = self.async_send_waiters.lock();
    if let Some(id) = prev_id {
      g.queue.retain(|(sid, _, _)| *sid != id);
    }
    let id = g.next_id;
    g.next_id = g.next_id.wrapping_add(1);
    g.queue.push_back((id, waker, notified));
    self
      .async_send_waiter_count
      .store(g.queue.len(), Ordering::Release);
    id
  }

  pub(crate) fn unregister_async_send(&self, id: u64) {
    let mut g = self.async_send_waiters.lock();
    let prev = g.queue.len();
    g.queue.retain(|(sid, _, _)| *sid != id);
    if g.queue.len() != prev {
      self
        .async_send_waiter_count
        .store(g.queue.len(), Ordering::Release);
    }
  }

  pub(crate) fn register_sync_recv(&self, thread: Thread, notified: *const AtomicBool) {
    *self.sync_recv_waiter.lock() = Some((thread, notified));
    self.sync_recv_waiter_count.store(1, Ordering::Release);
  }

  /// Returns true iff our waiter slot was still present (we removed it). False
  /// means a waker (`notify_receiver`/`wake_all_receivers`) already took it and
  /// is mid-write to our stack `notified` — see `finish_sync_recv`.
  pub(crate) fn unregister_sync_recv(&self) -> bool {
    let had = self.sync_recv_waiter.lock().take().is_some();
    self.sync_recv_waiter_count.store(0, Ordering::Release);
    had
  }

  /// Symmetric to `finish_sync_send`: if a waker already took our recv slot it
  /// will `store(true)` into our raw `notified`; don't let this frame die until
  /// that write lands.
  pub(crate) fn finish_sync_recv(&self, notified: &AtomicBool) {
    if !self.unregister_sync_recv() {
      while !notified.load(Ordering::Acquire) {
        hint::spin_loop();
      }
    }
  }

  pub(crate) fn register_async_recv(&self, waker: Waker, notified: *const AtomicBool) {
    *self.async_recv_waiter.lock() = Some((waker, notified));
    self.async_recv_waiter_count.store(1, Ordering::Release);
  }

  pub(crate) fn unregister_async_recv(&self) {
    *self.async_recv_waiter.lock() = None;
    self.async_recv_waiter_count.store(0, Ordering::Release);
  }

  // Called by every sender regardless of kind, since a sender doesn't know
  // whether the current receiver handle is sync or async. Only one of the
  // two slots is ever actually populated at a time (there's only ever one
  // receiver), so the other check is a cheap Relaxed-load no-op.
  fn notify_receiver(&self) {
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

  // Wakes up to `released` parked sync senders instead of just one: real OS
  // threads pay a full park/unpark round trip, so waking only one per
  // release regardless of how many nodes came back left the rest sitting
  // parked until enough further releases trickled through to reach them
  // individually. Uses `pop_front` (oldest first) rather than rebuilding the
  // VecDeque, so `sync_send_waiters` keeps its accumulated capacity instead
  // of forcing the next burst of registrations to reallocate from scratch.
  fn notify_sync_senders(&self, released: usize) {
    if self.sync_send_waiter_count.load(Ordering::Relaxed) == 0 {
      return;
    }
    let mut g = self.sync_send_waiters.lock();
    let mut to_wake = Vec::with_capacity(g.queue.len().min(released));
    while to_wake.len() < released {
      let Some((_id, thread, notified)) = g.queue.pop_front() else {
        break;
      };
      if !notified.is_null() {
        unsafe { (*notified).store(true, Ordering::Release) };
      }
      to_wake.push(thread);
    }
    self
      .sync_send_waiter_count
      .store(g.queue.len(), Ordering::Release);
    drop(g);
    for thread in to_wake {
      thread.unpark();
    }
  }

  fn notify_async_senders(&self) {
    if self.async_send_waiter_count.load(Ordering::Relaxed) != 0 {
      let mut g = self.async_send_waiters.lock();
      if let Some((_id, waker, notified)) = g.queue.pop_front() {
        self
          .async_send_waiter_count
          .store(g.queue.len(), Ordering::Release);
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

  fn wake_all_senders(&self) {
    let mut sync_to_wake = Vec::new();
    {
      let mut g = self.sync_send_waiters.lock();
      while let Some((_id, thread, notified)) = g.queue.pop_front() {
        if !notified.is_null() {
          unsafe { (*notified).store(true, Ordering::Release) };
        }
        sync_to_wake.push(thread);
      }
      self.sync_send_waiter_count.store(0, Ordering::Release);
    }
    let mut async_to_wake = Vec::new();
    {
      let mut g = self.async_send_waiters.lock();
      while let Some((_id, waker, notified)) = g.queue.pop_front() {
        if !notified.is_null() {
          unsafe { (*notified).store(true, Ordering::Release) };
        }
        async_to_wake.push(waker);
      }
      self.async_send_waiter_count.store(0, Ordering::Release);
    }
    for t in sync_to_wake {
      t.unpark();
    }
    for w in async_to_wake {
      w.wake();
    }
  }

  pub fn poll_pop(
    &self,
    cx: &mut Context<'_>,
    is_registered: &mut bool,
  ) -> Poll<Result<(*mut Node<T>, T), RecvError>> {
    loop {
      let tail = unsafe { *self.tail.get() };

      if let Some(next) = unsafe { self.follow_link(tail) } {
        if *is_registered {
          self.unregister_async_recv();
          *is_registered = false;
        }
        // Option::take, not ptr::read: must leave the slot as None. A node
        // popped here can sit unused in the chunk free-list until the queue
        // itself drops; Node has no custom Drop, so the auto-derived drop glue
        // for `buf` would double-drop a stale Some(T) left by a bare ptr::read.
        let value = unsafe { (&mut *(*next).val.get()).take().unwrap() };
        unsafe {
          *self.tail.get() = next;
        }
        self.current_len.fetch_sub(1, Ordering::Relaxed);
        return Poll::Ready(Ok((tail, value)));
      }

      if !self.senders_alive() {
        if unsafe { self.link_is_fresh(tail) } {
          continue;
        }
        if *is_registered {
          self.unregister_async_recv();
          *is_registered = false;
        }
        return Poll::Ready(Err(RecvError::Disconnected));
      }

      self.register_async_recv(cx.waker().clone(), std::ptr::null());
      *is_registered = true;
      self.pre_park_fence();

      if unsafe { self.link_is_fresh(tail) } {
        continue;
      }
      if !self.senders_alive() {
        continue;
      }

      return Poll::Pending;
    }
  }

  #[inline]
  pub fn pre_park_fence(&self) {
    fence(Ordering::SeqCst);
  }

  #[inline]
  pub fn capacity(&self) -> usize {
    self.cap
  }

  #[inline]
  pub fn len(&self) -> usize {
    self.current_len.load(Ordering::Relaxed)
  }

  #[inline]
  pub fn is_empty(&self) -> bool {
    self.len() == 0
  }

  #[inline]
  pub fn is_full(&self) -> bool {
    self.len() >= self.cap
  }
}

impl<T: Send> Drop for BoundedQueue<T> {
  fn drop(&mut self) {
    self.wake_all_senders();
    self.wake_all_receivers();
    // Reconstitute the node allocation; its drop glue drops any values still
    // buffered in the nodes (`val: Option<T>`).
    unsafe { drop(Box::from_raw(self.buf)) };
  }
}

// --- Local Cache ---

pub(crate) struct LocalCache<T> {
  pub(crate) head: *mut Node<T>,
  pub(crate) count: usize,
  /// Miri-only: exact membership of this cache's free chain. Every access to
  /// a LocalCache is single-threaded (shared pool under its mutex, receiver
  /// cache under RefCell), so this set is race-free and has no cross-thread
  /// visibility caveat — double-free, chunk overlap, and short chunks panic
  /// deterministically at the violating call.
  #[cfg(miri)]
  pub(crate) members: std::collections::HashSet<u32>,
}

unsafe impl<T: Send> Send for LocalCache<T> {}

impl<T> Default for LocalCache<T> {
  fn default() -> Self {
    Self {
      head: std::ptr::null_mut(),
      count: 0,
      #[cfg(miri)]
      members: std::collections::HashSet::new(),
    }
  }
}

impl<T: Send> LocalCache<T> {
  #[inline]
  pub(crate) fn fill_pool(&mut self, shared: &BoundedQueue<T>) -> bool {
    if self.count == 0 {
      if let Some(head_idx) = shared.chunk_stack.pop(shared.buf_start) {
        let ptr = shared.idx_to_ptr(head_idx);
        self.head = ptr;
        self.count = unsafe { (*ptr).chunk_len as usize };
        custody!("chunk_pop head={} len={}", head_idx, self.count);
        // Single-threaded under this cache's owner (mutex/RefCell), so no
        // visibility caveat: verify the popped chunk really contains
        // `chunk_len` distinct, not-yet-member nodes and mark each grabbed.
        #[cfg(miri)]
        {
          let count = self.count;
          let mut curr = ptr;
          for i in 0..count {
            miri::fill_pool_verify_node(shared, self, head_idx, count, curr, i);
            curr = unsafe { shared.next_of(curr) };
          }
        }
        return true;
      }
      return false;
    }
    true
  }

  #[inline]
  pub(crate) fn pop_node(&mut self, shared: &BoundedQueue<T>) -> Option<*mut Node<T>> {
    if !self.fill_pool(shared) {
      return None;
    }
    let curr = self.head;
    unsafe {
      self.head = shared.next_of(curr);
    }
    self.count -= 1;
    custody!("pool_pop {}", shared.ptr_to_idx(curr));
    #[cfg(miri)]
    miri::pool_pop_remove_member(shared, self, curr);
    Some(curr)
  }

  #[inline]
  #[cfg_attr(not(miri), allow(unused_variables))]
  pub(crate) fn push_node(&mut self, shared: &BoundedQueue<T>, node: *mut Node<T>) {
    custody!("free {}", shared.ptr_to_idx(node));
    #[cfg(miri)]
    miri::push_node_check(shared, self, node);
    unsafe {
      (*node).next.store(self.head, Ordering::Relaxed);
    }
    self.head = node;
    self.count += 1;
  }

  #[inline]
  pub(crate) fn flush(&mut self, shared: &BoundedQueue<T>) {
    if self.count > 0 {
      unsafe {
        (*self.head).chunk_len = self.count as u32;
      }
      // Bisect the recycle-chunk double-ownership: before this chunk becomes
      // producer-grabbable, every node in the [head, count) free-chain must be
      // FREE. See super::miri::flush_verify_chain.
      #[cfg(miri)]
      miri::flush_verify_chain(shared, self);
      custody!("flush chunk head={} len={}", shared.ptr_to_idx(self.head), self.count);
      shared.release_chunk(self.head);
      self.head = std::ptr::null_mut();
      self.count = 0;
      #[cfg(miri)]
      self.members.clear();
    }
  }
}

/// Publishes a detached free-chain run of `k >= 1` nodes starting at `first`:
/// writes the values, terminates the run, and splices it in with the Release
/// junction store. The caller must already have advanced its pool cursor past
/// the run and must feed an iterator with at least `k` items.
pub(crate) unsafe fn publish_run<T: Send>(
  shared: &BoundedQueue<T>,
  first: *mut Node<T>,
  k: usize,
  fill: &mut impl Iterator<Item = T>,
) {
  debug_assert!(k >= 1);
  custody!("pub_run first={} k={}", shared.ptr_to_idx(first), k);
  unsafe {
    let mut curr = first;
    for _ in 0..(k - 1) {
      let next = (*curr).next.load(Ordering::Relaxed);
      // PRODUCER -> PUBLISHED + owner canary (miri); see super::miri::shadow_publish.
      #[cfg(miri)]
      miri::shadow_publish(shared, curr);
      (*curr).val.get().write(Some(fill.next().unwrap()));
      curr = next;
    }
    #[cfg(miri)]
    miri::shadow_publish(shared, curr);
    (*curr).val.get().write(Some(fill.next().unwrap()));
    (*curr).next.store(std::ptr::null_mut(), Ordering::Relaxed);

    let old_head = shared.head.swap(curr, Ordering::AcqRel);
    (*old_head).next.store(first, Ordering::Release);
  }
  shared.current_len.fetch_add(k, Ordering::Relaxed);
  shared.notify_receiver();
}
