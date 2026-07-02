//! A screamingly high-performance Bounded MPSC Queue inspired by higher performing C++ MPSC Queues
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

use std::cell::{RefCell, UnsafeCell};
use std::collections::VecDeque;
use std::fmt;
use std::marker::PhantomPinned;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU64, AtomicUsize, Ordering, fence};
use std::task::{Context, Poll, Waker};
use std::thread::{self, Thread};
use std::time::{Duration, Instant};

use futures_core::Stream;
use parking_lot::Mutex;

use crate::error::{
  BatchSendErrorReason, CloseError, RecvError, RecvErrorTimeout, SendBatchError, SendError,
  TryRecvError, TrySendBatchError, TrySendError,
};
use crate::internal::cache_padded::CachePadded;
use crate::sync_util;

const SENTINEL_IDX: u32 = u32::MAX;

// Upper bound on how many recycled nodes a receiver hoards before flushing
// them back to the shared chunk stack. Without this cap the threshold
// scaled with the full queue capacity, so on large-capacity queues senders
// would starve waiting for the receiver to accumulate hundreds of freed
// nodes before releasing any of them back.
const CACHE_FLUSH_CHUNK: usize = 64;

// --- Private Waiter Coordination ---
//
// Sync and async waiters live in entirely separate queues/slots (mirroring
// mpmc_v2's SyncWaiter/AsyncWaiter split) rather than one shared queue
// holding a Sync/Async enum. This means sync's OS threads and async's tasks
// never contend on the same mutex at all: sync senders only ever lock
// `sync_send_waiters`, async senders only ever lock `async_send_waiters`,
// and likewise on the receive side. Each side is free to pick its own wake
// policy later without any risk of it affecting the other.

struct SyncSendWaiters {
  queue: VecDeque<(u64, Thread, *const AtomicBool)>,
  next_id: u64,
}

struct AsyncSendWaiters {
  queue: VecDeque<(u64, Waker, *const AtomicBool)>,
  next_id: u64,
}

// --- Intrusive Node ---
pub struct Node<T> {
  next: AtomicPtr<Node<T>>,
  next_chunk: AtomicPtr<Node<T>>,
  chunk_len: u32,
  val: UnsafeCell<Option<T>>,
}

// --- Generational Tagged Index Stack (Safe ABA Mitigation) ---
struct ChunkStack {
  // Packed 64-bit state: (generation: u32 << 32) | (head_index: u32)
  state: AtomicU64,
}

impl ChunkStack {
  fn new() -> Self {
    ChunkStack {
      state: AtomicU64::new(((0u64) << 32) | (SENTINEL_IDX as u64)),
    }
  }

  fn push<T>(&self, buf_start: *mut Node<T>, head_idx: u32) {
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

  fn pop<T>(&self, buf_start: *mut Node<T>) -> Option<u32> {
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
        .compare_exchange_weak(state, new_state, Ordering::AcqRel, Ordering::Relaxed)
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
  buf: *mut [Node<T>],
  buf_start: *mut Node<T>,
  cap: usize,

  // Intrusive MPSC cursors
  head: CachePadded<AtomicPtr<Node<T>>>,
  tail: CachePadded<UnsafeCell<*mut Node<T>>>,

  // Tagged Chunk Recycling Stack
  chunk_stack: ChunkStack,

  // Handle coordination. Sync and async waiters are kept in entirely
  // separate queues/slots (see the comment above `SyncSendWaiters`) so
  // neither kind ever contends on the other's mutex.
  sender_count: CachePadded<AtomicUsize>,
  receiver_dropped: CachePadded<AtomicBool>,
  sync_send_waiters: Mutex<SyncSendWaiters>,
  async_send_waiters: Mutex<AsyncSendWaiters>,
  sync_send_waiter_count: CachePadded<AtomicUsize>,
  async_send_waiter_count: CachePadded<AtomicUsize>,
  sync_recv_waiter: Mutex<Option<(Thread, *const AtomicBool)>>,
  async_recv_waiter: Mutex<Option<(Waker, *const AtomicBool)>>,
  sync_recv_waiter_count: CachePadded<AtomicUsize>,
  async_recv_waiter_count: CachePadded<AtomicUsize>,
  current_len: CachePadded<AtomicUsize>,

  // Shared sender cache. Capacity is a single chunk, so every Sender/
  // AsyncSender clone locks this directly instead of being assigned a bucket.
  cache: CachePadded<Mutex<LocalCache<T>>>,
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
  fn ptr_to_idx(&self, ptr: *mut Node<T>) -> u32 {
    unsafe { ptr.offset_from(self.buf_start) as u32 }
  }

  #[inline]
  fn idx_to_ptr(&self, idx: u32) -> *mut Node<T> {
    if idx == SENTINEL_IDX {
      std::ptr::null_mut()
    } else {
      unsafe { self.buf_start.add(idx as usize) }
    }
  }

  fn release_chunk(&self, chunk_head: *mut Node<T>) {
    // Read chunk_len before pushing: once it's on chunk_stack another
    // thread can pop (and even re-split, via take_nodes) it immediately.
    let released = unsafe { (*chunk_head).chunk_len } as usize;
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

  fn register_sync_send(&self, prev_id: Option<u64>, thread: Thread, notified: *const AtomicBool) -> u64 {
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

  fn unregister_sync_send(&self, id: u64) {
    let mut g = self.sync_send_waiters.lock();
    let prev = g.queue.len();
    g.queue.retain(|(sid, _, _)| *sid != id);
    if g.queue.len() != prev {
      self
        .sync_send_waiter_count
        .store(g.queue.len(), Ordering::Release);
    }
  }

  fn register_async_send(&self, prev_id: Option<u64>, waker: Waker, notified: *const AtomicBool) -> u64 {
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

  fn unregister_async_send(&self, id: u64) {
    let mut g = self.async_send_waiters.lock();
    let prev = g.queue.len();
    g.queue.retain(|(sid, _, _)| *sid != id);
    if g.queue.len() != prev {
      self
        .async_send_waiter_count
        .store(g.queue.len(), Ordering::Release);
    }
  }

  fn register_sync_recv(&self, thread: Thread, notified: *const AtomicBool) {
    *self.sync_recv_waiter.lock() = Some((thread, notified));
    self.sync_recv_waiter_count.store(1, Ordering::Release);
  }

  fn unregister_sync_recv(&self) {
    *self.sync_recv_waiter.lock() = None;
    self.sync_recv_waiter_count.store(0, Ordering::Release);
  }

  fn register_async_recv(&self, waker: Waker, notified: *const AtomicBool) {
    *self.async_recv_waiter.lock() = Some((waker, notified));
    self.async_recv_waiter_count.store(1, Ordering::Release);
  }

  fn unregister_async_recv(&self) {
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
      let next = unsafe { (*tail).next.load(Ordering::Acquire) };

      if !next.is_null() {
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
        let next_check = unsafe { (*tail).next.load(Ordering::Acquire) };
        if !next_check.is_null() {
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

      let next_after_register = unsafe { (*tail).next.load(Ordering::Acquire) };
      if !next_after_register.is_null() {
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
}

unsafe impl<T: Send> Send for LocalCache<T> {}

impl<T> Default for LocalCache<T> {
  fn default() -> Self {
    Self {
      head: std::ptr::null_mut(),
      count: 0,
    }
  }
}

impl<T: Send> LocalCache<T> {
  #[inline]
  fn fill_pool(&mut self, shared: &BoundedQueue<T>) -> bool {
    if self.count == 0 {
      if let Some(head_idx) = shared.chunk_stack.pop(shared.buf_start) {
        let ptr = shared.idx_to_ptr(head_idx);
        self.head = ptr;
        self.count = unsafe { (*ptr).chunk_len as usize };
        return true;
      }
      return false;
    }
    true
  }

  #[inline]
  fn pop_node(&mut self, shared: &BoundedQueue<T>) -> Option<*mut Node<T>> {
    if !self.fill_pool(shared) {
      return None;
    }
    let curr = self.head;
    unsafe {
      self.head = (*curr).next.load(Ordering::Relaxed);
    }
    self.count -= 1;
    Some(curr)
  }

  #[inline]
  fn push_node(&mut self, node: *mut Node<T>) {
    unsafe {
      (*node).next.store(self.head, Ordering::Relaxed);
      self.head = node;
      self.count += 1;
    }
  }

  #[inline]
  pub(crate) fn flush(&mut self, shared: &BoundedQueue<T>) {
    if self.count > 0 {
      unsafe {
        (*self.head).chunk_len = self.count as u32;
      }
      shared.release_chunk(self.head);
      self.head = std::ptr::null_mut();
      self.count = 0;
    }
  }
}

// --- Handles ---

pub struct Sender<T: Send> {
  shared: Arc<BoundedQueue<T>>,
  closed: AtomicBool,
}

// Deliberately `RefCell`, not `Mutex`: Receiver is intentionally restricted to
// single-thread access (see the `Send`-only impl below, no `Sync`). Sharing a
// `Receiver` across threads is a compile error now (RefCell is never Sync),
// rather than a silent footgun protected only by a lock nothing contends.
pub struct Receiver<T: Send> {
  shared: Arc<BoundedQueue<T>>,
  closed: AtomicBool,
  pub(crate) cache: RefCell<LocalCache<T>>,
}

pub struct AsyncSender<T: Send> {
  shared: Arc<BoundedQueue<T>>,
  closed: AtomicBool,
}

pub struct AsyncReceiver<T: Send> {
  shared: Arc<BoundedQueue<T>>,
  closed: AtomicBool,
  is_registered: bool,
  pub(crate) cache: Mutex<LocalCache<T>>,
}

impl<T: Send> fmt::Debug for Sender<T> {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("Sender")
      .field("capacity", &self.capacity())
      .field("len", &self.len())
      .field("closed", &self.closed.load(Ordering::Relaxed))
      .finish()
  }
}

impl<T: Send> fmt::Debug for Receiver<T> {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("Receiver")
      .field("capacity", &self.capacity())
      .field("len", &self.len())
      .field("closed", &self.closed.load(Ordering::Relaxed))
      .finish()
  }
}

impl<T: Send> fmt::Debug for AsyncSender<T> {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("AsyncSender")
      .field("capacity", &self.capacity())
      .field("len", &self.len())
      .field("closed", &self.closed.load(Ordering::Relaxed))
      .finish()
  }
}

impl<T: Send> fmt::Debug for AsyncReceiver<T> {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("AsyncReceiver")
      .field("capacity", &self.capacity())
      .field("len", &self.len())
      .field("closed", &self.closed.load(Ordering::Relaxed))
      .finish()
  }
}

unsafe impl<T: Send> Send for Sender<T> {}
unsafe impl<T: Send> Send for Receiver<T> {}
unsafe impl<T: Send> Send for AsyncSender<T> {}
unsafe impl<T: Send> Sync for AsyncSender<T> {}
unsafe impl<T: Send> Send for AsyncReceiver<T> {}
unsafe impl<T: Send> Sync for AsyncReceiver<T> {}

impl<T: Send> Sender<T> {
  #[inline]
  unsafe fn enqueue_node(&self, node: *mut Node<T>, item: T) {
    unsafe {
      (*node).val.get().write(Some(item));
      (*node).next.store(std::ptr::null_mut(), Ordering::Relaxed);
      let old_head = self.shared.head.swap(node, Ordering::AcqRel);
      (*old_head).next.store(node, Ordering::Release);
    }
    self.shared.current_len.fetch_add(1, Ordering::Relaxed);
    self.shared.notify_receiver();
  }

  /// Blocks until a node is available. Never holds the shared bucket's lock
  /// while parked: each iteration locks just long enough to check, then drops
  /// it before potentially parking, so other senders sharing this bucket
  /// aren't frozen out for the duration of our wait.
  #[inline]
  fn allocate_node(&self) -> Result<*mut Node<T>, SendError> {
    let mut is_registered = false;
    let mut my_id = None;
    let notified = AtomicBool::new(false);
    let notified_ptr = &notified as *const AtomicBool;

    loop {
      if self.closed.load(Ordering::Relaxed) || !self.shared.receivers_alive() {
        if let Some(id) = my_id {
          self.shared.unregister_sync_send(id);
        }
        return Err(SendError::Closed);
      }

      {
        let mut pool = self.shared.cache.lock();
        if let Some(node) = pool.pop_node(&self.shared) {
          if let Some(id) = my_id {
            self.shared.unregister_sync_send(id);
          }
          return Ok(node);
        }
      }

      if is_registered {
        sync_util::park_thread();
        if notified.swap(false, Ordering::Acquire) {
          is_registered = false;
          my_id = None;
        }
        continue;
      }

      let id = self
        .shared
        .register_sync_send(None, thread::current(), notified_ptr);
      is_registered = true;
      my_id = Some(id);
      fence(Ordering::SeqCst);
    }
  }

  pub fn send(&self, item: T) -> Result<(), SendError> {
    if self.closed.load(Ordering::Relaxed) || !self.shared.receivers_alive() {
      return Err(SendError::Closed);
    }

    // 1. Try to get a node from local cache or chunk stack. The lock is
    // held only long enough to pop the pointer; the Vyukov swap and the
    // receiver wake happen after it's released.
    let node_ptr = {
      let mut pool = self.shared.cache.lock();
      pool.pop_node(&self.shared)
    };
    if let Some(node) = node_ptr {
      unsafe { self.enqueue_node(node, item) };
      return Ok(());
    }

    // 2. Blocking Path: park immediately rather than spinning.
    let node_ptr = self.allocate_node()?;
    unsafe { self.enqueue_node(node_ptr, item) };
    Ok(())
  }

  pub fn try_send(&self, item: T) -> Result<(), TrySendError<T>> {
    if self.closed.load(Ordering::Relaxed) || !self.shared.receivers_alive() {
      return Err(TrySendError::Closed(item));
    }
    let node_ptr = {
      let mut pool = self.shared.cache.lock();
      pool.pop_node(&self.shared)
    };
    let curr = match node_ptr {
      Some(n) => n,
      None => return Err(TrySendError::Full(item)),
    };

    unsafe { self.enqueue_node(curr, item) };
    Ok(())
  }

  pub fn send_batch(&self, items: Vec<T>) -> Result<usize, SendBatchError<T>> {
    let total = items.len();
    if total == 0 {
      return Ok(0);
    }
    if self.closed.load(Ordering::Relaxed) || !self.shared.receivers_alive() {
      return Err(SendBatchError {
        sent: 0,
        unsent: items,
      });
    }

    let mut iter = items.into_iter();
    let mut sent = 0;

    while sent < total {
      // Detach a run of nodes from the pool under the lock (pure pointer
      // bookkeeping); the lock is released before writing items or
      // publishing, so it's never held across the Vyukov swap or wake.
      let (first_node, k) = loop {
        let mut pool = self.shared.cache.lock();
        if pool.count == 0 {
          // Drop the lock before calling allocate_node, which may park: see
          // the comment on allocate_node for why it must never be held while
          // blocked.
          drop(pool);
          match self.allocate_node() {
            Ok(ptr) => {
              self.shared.cache.lock().push_node(ptr);
            }
            Err(_) => {
              return Err(SendBatchError {
                sent,
                unsent: iter.collect(),
              });
            }
          }
          continue;
        }

        let k = (total - sent).min(pool.count);
        let first_node = pool.head;
        let mut curr = first_node;
        unsafe {
          for _ in 0..(k - 1) {
            curr = (*curr).next.load(Ordering::Relaxed);
          }
          pool.head = (*curr).next.load(Ordering::Relaxed);
        }
        pool.count -= k;
        break (first_node, k);
      };

      unsafe {
        let mut curr = first_node;
        for _ in 0..(k - 1) {
          let next = (*curr).next.load(Ordering::Relaxed);
          let item_val = iter.next().unwrap();
          (*curr).val.get().write(Some(item_val));
          curr = next;
        }
        let item_val = iter.next().unwrap();
        (*curr).val.get().write(Some(item_val));
        (*curr).next.store(std::ptr::null_mut(), Ordering::Relaxed);

        let old_head = self.shared.head.swap(curr, Ordering::AcqRel);
        (*old_head).next.store(first_node, Ordering::Release);
      }

      self.shared.current_len.fetch_add(k, Ordering::Relaxed);
      self.shared.notify_receiver();
      sent += k;
    }

    Ok(total)
  }

  pub fn try_send_batch(&self, items: Vec<T>) -> Result<usize, TrySendBatchError<T>> {
    let total = items.len();
    if total == 0 {
      return Ok(0);
    }
    if self.closed.load(Ordering::Relaxed) || !self.shared.receivers_alive() {
      return Err(TrySendBatchError {
        sent: 0,
        unsent: items,
        reason: BatchSendErrorReason::Closed,
      });
    }

    let mut iter = items.into_iter();
    let mut sent = 0;

    while sent < total {
      // Detach a run of nodes from the pool under the lock (pure pointer
      // bookkeeping); the lock is released before writing items or
      // publishing, so it's never held across the Vyukov swap or wake.
      let detached = {
        let mut pool = self.shared.cache.lock();
        if !pool.fill_pool(&self.shared) {
          None
        } else {
          let k = (total - sent).min(pool.count);
          let first_node = pool.head;
          let mut curr = first_node;
          unsafe {
            for _ in 0..(k - 1) {
              curr = (*curr).next.load(Ordering::Relaxed);
            }
            pool.head = (*curr).next.load(Ordering::Relaxed);
          }
          pool.count -= k;
          Some((first_node, k))
        }
      };

      let (first_node, k) = match detached {
        Some(v) => v,
        None => break,
      };

      unsafe {
        let mut curr = first_node;
        for _ in 0..(k - 1) {
          let next = (*curr).next.load(Ordering::Relaxed);
          let item_val = iter.next().unwrap();
          (*curr).val.get().write(Some(item_val));
          curr = next;
        }
        let item_val = iter.next().unwrap();
        (*curr).val.get().write(Some(item_val));
        (*curr).next.store(std::ptr::null_mut(), Ordering::Relaxed);

        let old_head = self.shared.head.swap(curr, Ordering::AcqRel);
        (*old_head).next.store(first_node, Ordering::Release);
      }

      self.shared.current_len.fetch_add(k, Ordering::Relaxed);
      self.shared.notify_receiver();
      sent += k;
    }

    if sent == total {
      Ok(total)
    } else {
      Err(TrySendBatchError {
        sent,
        unsent: iter.collect(),
        reason: BatchSendErrorReason::Full,
      })
    }
  }

  pub fn send_batch_mut(&self, items: &mut Vec<T>) -> Result<usize, SendError> {
    if items.is_empty() {
      return Ok(0);
    }
    if self.closed.load(Ordering::Relaxed) || !self.shared.receivers_alive() {
      return Err(SendError::Closed);
    }
    let batch = std::mem::take(items);
    match self.send_batch(batch) {
      Ok(n) => Ok(n),
      Err(e) => {
        *items = e.unsent;
        Err(SendError::Closed)
      }
    }
  }

  pub fn try_send_batch_mut(&self, items: &mut Vec<T>) -> Result<usize, SendError> {
    if items.is_empty() {
      return Ok(0);
    }
    if self.closed.load(Ordering::Relaxed) || !self.shared.receivers_alive() {
      return Err(SendError::Closed);
    }
    let batch = std::mem::take(items);
    match self.try_send_batch(batch) {
      Ok(n) => Ok(n),
      Err(e) => {
        let (sent, reason) = (e.sent, e.reason);
        *items = e.unsent;
        if sent == 0 && matches!(reason, BatchSendErrorReason::Closed) {
          Err(SendError::Closed)
        } else {
          Ok(sent)
        }
      }
    }
  }

  pub fn close(&self) -> Result<(), CloseError> {
    if self
      .closed
      .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
      .is_ok()
    {
      self.shared.drop_sender();
      Ok(())
    } else {
      Err(CloseError)
    }
  }

  pub fn is_closed(&self) -> bool {
    self.closed.load(Ordering::Relaxed) || !self.shared.receivers_alive()
  }

  pub fn capacity(&self) -> usize {
    self.shared.capacity()
  }

  pub fn len(&self) -> usize {
    self.shared.len()
  }

  pub fn is_empty(&self) -> bool {
    self.shared.is_empty()
  }

  pub fn is_full(&self) -> bool {
    self.shared.is_full()
  }

  pub fn to_async(self) -> AsyncSender<T> {
    let shared = unsafe { std::ptr::read(&self.shared) };
    let closed = self.closed.load(Ordering::Relaxed);
    std::mem::forget(self);
    AsyncSender {
      shared,
      closed: AtomicBool::new(closed),
    }
  }
}

impl<T: Send> Clone for Sender<T> {
  fn clone(&self) -> Self {
    self.shared.add_sender();
    Sender {
      shared: Arc::clone(&self.shared),
      closed: AtomicBool::new(false),
    }
  }
}

impl<T: Send> Drop for Sender<T> {
  fn drop(&mut self) {
    let _ = self.close();
  }
}

impl<T: Send> Receiver<T> {
  #[inline]
  fn recycle_node(&self, node_ptr: *mut Node<T>, pool: &mut LocalCache<T>) {
    pool.push_node(node_ptr);
    if pool.count >= self.shared.capacity().min(CACHE_FLUSH_CHUNK) {
      pool.flush(&self.shared);
    }
  }

  pub fn recv(&self) -> Result<T, RecvError> {
    let mut is_registered = false;
    let notified = AtomicBool::new(false);
    let notified_ptr = &notified as *const AtomicBool;

    if self.closed.load(Ordering::Relaxed) {
      return Err(RecvError::Disconnected);
    }

    let mut pool = self.cache.borrow_mut();

    loop {
      let tail = unsafe { *self.shared.tail.get() };
      let next = unsafe { (*tail).next.load(Ordering::Acquire) };

      if !next.is_null() {
        if is_registered {
          self.shared.unregister_sync_recv();
        }
        // see poll_pop: must leave the slot None, or the queue's drop glue
        // double-drops a stale value left in a never-resent free-list node
        let value = unsafe { (&mut *(*next).val.get()).take().unwrap() };
        unsafe {
          *self.shared.tail.get() = next;
        }
        self.shared.current_len.fetch_sub(1, Ordering::Relaxed);
        self.recycle_node(tail, &mut pool);
        return Ok(value);
      }

      pool.flush(&self.shared);

      if !self.shared.senders_alive() {
        let next_check = unsafe { (*tail).next.load(Ordering::Acquire) };
        if !next_check.is_null() {
          continue;
        }
        if is_registered {
          self.shared.unregister_sync_recv();
        }
        return Err(RecvError::Disconnected);
      }

      if is_registered {
        sync_util::park_thread();
        if notified.swap(false, Ordering::Acquire) {
          is_registered = false;
        }
        continue;
      }

      self
        .shared
        .register_sync_recv(thread::current(), notified_ptr);
      is_registered = true;
      fence(Ordering::SeqCst);
    }
  }

  pub fn recv_timeout(&self, timeout: Duration) -> Result<T, RecvErrorTimeout> {
    let deadline = Instant::now().checked_add(timeout);
    let mut is_registered = false;
    let notified = AtomicBool::new(false);
    let notified_ptr = &notified as *const AtomicBool;

    let mut pool = self.cache.borrow_mut();

    loop {
      let tail = unsafe { *self.shared.tail.get() };
      let next = unsafe { (*tail).next.load(Ordering::Acquire) };

      if !next.is_null() {
        if is_registered {
          self.shared.unregister_sync_recv();
        }
        // see poll_pop: must leave the slot None, or the queue's drop glue
        // double-drops a stale value left in a never-resent free-list node
        let value = unsafe { (&mut *(*next).val.get()).take().unwrap() };
        unsafe {
          *self.shared.tail.get() = next;
        }
        self.shared.current_len.fetch_sub(1, Ordering::Relaxed);
        self.recycle_node(tail, &mut pool);
        return Ok(value);
      }

      pool.flush(&self.shared);

      if !self.shared.senders_alive() {
        let next_check = unsafe { (*tail).next.load(Ordering::Acquire) };
        if !next_check.is_null() {
          continue;
        }
        if is_registered {
          self.shared.unregister_sync_recv();
        }
        return Err(RecvErrorTimeout::Disconnected);
      }

      let now = Instant::now();
      let remaining = match deadline {
        Some(d) if d > now => d - now,
        _ => {
          if is_registered {
            self.shared.unregister_sync_recv();
          }
          return Err(RecvErrorTimeout::Timeout);
        }
      };

      if is_registered {
        sync_util::park_thread_timeout(remaining);
        if notified.swap(false, Ordering::Acquire) {
          is_registered = false;
        }
        continue;
      }

      self
        .shared
        .register_sync_recv(thread::current(), notified_ptr);
      is_registered = true;
      fence(Ordering::SeqCst);
    }
  }

  pub fn try_recv(&self) -> Result<T, TryRecvError> {
    if self.closed.load(Ordering::Relaxed) {
      return Err(TryRecvError::Disconnected);
    }

    let mut pool = self.cache.borrow_mut();
    let tail = unsafe { *self.shared.tail.get() };
    let next = unsafe { (*tail).next.load(Ordering::Acquire) };

    if !next.is_null() {
      // see poll_pop: must leave the slot None, or the queue's drop glue
      // double-drops a stale value left in a never-resent free-list node
      let value = unsafe { (&mut *(*next).val.get()).take().unwrap() };
      unsafe {
        *self.shared.tail.get() = next;
      }
      self.shared.current_len.fetch_sub(1, Ordering::Relaxed);
      self.recycle_node(tail, &mut *pool);
      Ok(value)
    } else {
      pool.flush(&self.shared);
      if !self.shared.senders_alive() {
        Err(TryRecvError::Disconnected)
      } else {
        Err(TryRecvError::Empty)
      }
    }
  }

  fn pop_batch_lock_free(&self, out: &mut Vec<T>, max: usize, pool: &mut LocalCache<T>) -> usize {
    let mut popped = 0;
    unsafe {
      let mut tail = *self.shared.tail.get();
      while popped < max {
        let next = (*tail).next.load(Ordering::Acquire);
        if next.is_null() {
          break;
        }
        // see poll_pop: must leave the slot None, or the queue's drop glue
        // double-drops a stale value left in a never-resent free-list node
        let value = (&mut *(*next).val.get()).take().unwrap();
        *self.shared.tail.get() = next;
        self.recycle_node(tail, pool);
        tail = next;
        out.push(value);
        popped += 1;
      }
    }
    if popped > 0 {
      self.shared.current_len.fetch_sub(popped, Ordering::Relaxed);
    }
    popped
  }

  pub fn recv_batch(&self, max: usize) -> Result<Vec<T>, RecvError> {
    let mut out = Vec::new();
    self.recv_batch_mut(&mut out, max)?;
    Ok(out)
  }

  pub fn recv_batch_mut(&self, out: &mut Vec<T>, max: usize) -> Result<usize, RecvError> {
    if max == 0 {
      return Ok(0);
    }
    if self.closed.load(Ordering::Relaxed) {
      return Err(RecvError::Disconnected);
    }

    let mut is_registered = false;
    let notified = AtomicBool::new(false);
    let notified_ptr = &notified as *const AtomicBool;
    let mut pool = self.cache.borrow_mut();

    loop {
      let k = self.pop_batch_lock_free(out, max, &mut *pool);
      if k > 0 {
        if is_registered {
          self.shared.unregister_sync_recv();
        }
        pool.flush(&self.shared);
        return Ok(k);
      }

      pool.flush(&self.shared);

      if !self.shared.senders_alive() {
        let k_end = self.pop_batch_lock_free(out, max, &mut pool);
        if k_end > 0 {
          if is_registered {
            self.shared.unregister_sync_recv();
          }
          pool.flush(&self.shared);
          return Ok(k_end);
        }
        if is_registered {
          self.shared.unregister_sync_recv();
        }
        return Err(RecvError::Disconnected);
      }

      if is_registered {
        sync_util::park_thread();
        if notified.swap(false, Ordering::Acquire) {
          is_registered = false;
        }
        continue;
      }

      self
        .shared
        .register_sync_recv(thread::current(), notified_ptr);
      is_registered = true;
      fence(Ordering::SeqCst);
    }
  }

  pub fn try_recv_batch(&self, max: usize) -> Result<Vec<T>, TryRecvError> {
    let mut out = Vec::new();
    self.try_recv_batch_mut(&mut out, max)?;
    Ok(out)
  }

  pub fn try_recv_batch_mut(&self, out: &mut Vec<T>, max: usize) -> Result<usize, TryRecvError> {
    if max == 0 {
      return Ok(0);
    }
    if self.closed.load(Ordering::Relaxed) {
      return Err(TryRecvError::Disconnected);
    }

    let mut pool = self.cache.borrow_mut();
    let k = self.pop_batch_lock_free(out, max, &mut *pool);
    if k > 0 {
      pool.flush(&self.shared);
      return Ok(k);
    }
    pool.flush(&self.shared);

    if !self.shared.senders_alive() {
      let k_end = self.pop_batch_lock_free(out, max, &mut *pool);
      if k_end > 0 {
        pool.flush(&self.shared);
        return Ok(k_end);
      }
      return Err(TryRecvError::Disconnected);
    }
    Err(TryRecvError::Empty)
  }

  pub fn close(&self) -> Result<(), CloseError> {
    if self
      .closed
      .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
      .is_ok()
    {
      self.shared.drop_receiver();
      Ok(())
    } else {
      Err(CloseError)
    }
  }

  pub fn is_closed(&self) -> bool {
    self.closed.load(Ordering::Relaxed) || (!self.shared.senders_alive() && self.shared.is_empty())
  }

  pub fn capacity(&self) -> usize {
    self.shared.capacity()
  }

  pub fn len(&self) -> usize {
    self.shared.len()
  }

  pub fn is_empty(&self) -> bool {
    self.shared.is_empty()
  }

  pub fn is_full(&self) -> bool {
    self.shared.is_full()
  }

  pub fn to_async(mut self) -> AsyncReceiver<T> {
    let shared = unsafe { std::ptr::read(&self.shared) };
    let closed = self.closed.load(Ordering::Relaxed);
    let cache_val = std::mem::take(self.cache.get_mut());
    std::mem::forget(self);
    AsyncReceiver {
      shared,
      closed: AtomicBool::new(closed),
      is_registered: false,
      cache: Mutex::new(cache_val),
    }
  }
}

impl<T: Send> Drop for Receiver<T> {
  fn drop(&mut self) {
    self.cache.get_mut().flush(&self.shared);
    let _ = self.close();
  }
}

impl<T: Send> AsyncSender<T> {
  pub fn send(&self, item: T) -> SendFuture<'_, T> {
    SendFuture::new(self, item)
  }

  pub fn try_send(&self, item: T) -> Result<(), TrySendError<T>> {
    if self.closed.load(Ordering::Relaxed) || !self.shared.receivers_alive() {
      return Err(TrySendError::Closed(item));
    }

    let mut pool = self.shared.cache.lock();
    let curr = match pool.pop_node(&self.shared) {
      Some(node) => node,
      None => return Err(TrySendError::Full(item)),
    };

    unsafe {
      (*curr).val.get().write(Some(item));
      (*curr).next.store(std::ptr::null_mut(), Ordering::Relaxed);
    }

    let old_head = self.shared.head.swap(curr, Ordering::AcqRel);
    unsafe {
      (*old_head).next.store(curr, Ordering::Release);
    }

    self.shared.current_len.fetch_add(1, Ordering::Relaxed);
    self.shared.notify_receiver();
    Ok(())
  }

  pub fn close(&self) -> Result<(), CloseError> {
    if self
      .closed
      .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
      .is_ok()
    {
      self.shared.drop_sender();
      Ok(())
    } else {
      Err(CloseError)
    }
  }

  pub fn is_closed(&self) -> bool {
    self.closed.load(Ordering::Relaxed) || !self.shared.receivers_alive()
  }

  pub fn capacity(&self) -> usize {
    self.shared.capacity()
  }

  pub fn len(&self) -> usize {
    self.shared.len()
  }

  pub fn is_empty(&self) -> bool {
    self.shared.is_empty()
  }

  pub fn is_full(&self) -> bool {
    self.shared.is_full()
  }

  pub fn to_sync(self) -> Sender<T> {
    let shared = unsafe { std::ptr::read(&self.shared) };
    let closed = self.closed.load(Ordering::Relaxed);
    std::mem::forget(self);
    Sender {
      shared,
      closed: AtomicBool::new(closed),
    }
  }

  pub fn try_send_batch(&self, items: Vec<T>) -> Result<usize, TrySendBatchError<T>> {
    let total = items.len();
    if total == 0 {
      return Ok(0);
    }
    if self.closed.load(Ordering::Relaxed) || !self.shared.receivers_alive() {
      return Err(TrySendBatchError {
        sent: 0,
        unsent: items,
        reason: BatchSendErrorReason::Closed,
      });
    }

    let mut iter = items.into_iter();
    let mut sent = 0;
    let mut pool = self.shared.cache.lock();

    while sent < total {
      if !pool.fill_pool(&self.shared) {
        break;
      }

      let k = (total - sent).min(pool.count);
      unsafe {
        let first_node = pool.head;
        let mut curr = first_node;
        for _ in 0..(k - 1) {
          let next = (*curr).next.load(Ordering::Relaxed);
          let item_val = iter.next().unwrap();
          (*curr).val.get().write(Some(item_val));
          curr = next;
        }
        let next_free = (*curr).next.load(Ordering::Relaxed);
        let item_val = iter.next().unwrap();
        (*curr).val.get().write(Some(item_val));
        (*curr).next.store(std::ptr::null_mut(), Ordering::Relaxed);

        pool.head = next_free;
        pool.count -= k;

        let old_head = self.shared.head.swap(curr, Ordering::AcqRel);
        (*old_head).next.store(first_node, Ordering::Release);
      }

      self.shared.current_len.fetch_add(k, Ordering::Relaxed);
      self.shared.notify_receiver();
      sent += k;
    }

    if sent == total {
      Ok(total)
    } else {
      Err(TrySendBatchError {
        sent,
        unsent: iter.collect(),
        reason: BatchSendErrorReason::Full,
      })
    }
  }

  pub fn try_send_batch_mut(&self, items: &mut Vec<T>) -> Result<usize, SendError> {
    if items.is_empty() {
      return Ok(0);
    }
    if self.closed.load(Ordering::Relaxed) || !self.shared.receivers_alive() {
      return Err(SendError::Closed);
    }
    let batch = std::mem::take(items);
    match self.try_send_batch(batch) {
      Ok(n) => Ok(n),
      Err(e) => {
        let (sent, reason) = (e.sent, e.reason);
        *items = e.unsent;
        if sent == 0 && matches!(reason, BatchSendErrorReason::Closed) {
          Err(SendError::Closed)
        } else {
          Ok(sent)
        }
      }
    }
  }
}

impl<T: Send> Clone for AsyncSender<T> {
  fn clone(&self) -> Self {
    self.shared.add_sender();
    AsyncSender {
      shared: Arc::clone(&self.shared),
      closed: AtomicBool::new(false),
    }
  }
}

impl<T: Send> Drop for AsyncSender<T> {
  fn drop(&mut self) {
    let _ = self.close();
  }
}

impl<T: Send> AsyncReceiver<T> {
  #[inline]
  fn recycle_node(&self, node_ptr: *mut Node<T>, pool: &mut LocalCache<T>) {
    pool.push_node(node_ptr);
    if pool.count >= self.shared.capacity() {
      pool.flush(&self.shared);
    }
  }

  pub fn recv(&self) -> RecvFuture<'_, T> {
    RecvFuture::new(self)
  }

  pub fn try_recv(&self) -> Result<T, TryRecvError> {
    if self.closed.load(Ordering::Relaxed) {
      return Err(TryRecvError::Disconnected);
    }

    let mut pool = self.cache.lock();
    let tail = unsafe { *self.shared.tail.get() };
    let next = unsafe { (*tail).next.load(Ordering::Acquire) };

    if !next.is_null() {
      // see poll_pop: must leave the slot None, or the queue's drop glue
      // double-drops a stale value left in a never-resent free-list node
      let value = unsafe { (&mut *(*next).val.get()).take().unwrap() };
      unsafe {
        *self.shared.tail.get() = next;
      }
      self.shared.current_len.fetch_sub(1, Ordering::Relaxed);
      self.recycle_node(tail, &mut *pool);
      Ok(value)
    } else {
      pool.flush(&self.shared);
      if !self.shared.senders_alive() {
        Err(TryRecvError::Disconnected)
      } else {
        Err(TryRecvError::Empty)
      }
    }
  }

  pub fn close(&self) -> Result<(), CloseError> {
    if self
      .closed
      .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
      .is_ok()
    {
      self.shared.drop_receiver();
      Ok(())
    } else {
      Err(CloseError)
    }
  }

  pub fn is_closed(&self) -> bool {
    self.closed.load(Ordering::Relaxed) || (!self.shared.senders_alive() && self.shared.is_empty())
  }

  pub fn capacity(&self) -> usize {
    self.shared.capacity()
  }

  pub fn len(&self) -> usize {
    self.shared.len()
  }

  pub fn is_empty(&self) -> bool {
    self.shared.is_empty()
  }

  pub fn is_full(&self) -> bool {
    self.shared.is_full()
  }

  pub fn to_sync(mut self) -> Receiver<T> {
    if self.is_registered {
      self.shared.unregister_async_recv();
    }
    let shared = unsafe { std::ptr::read(&self.shared) };
    let closed = self.closed.load(Ordering::Relaxed);
    let cache_val = std::mem::take(self.cache.get_mut());
    std::mem::forget(self);
    Receiver {
      shared,
      closed: AtomicBool::new(closed),
      cache: RefCell::new(cache_val),
    }
  }

  pub fn try_recv_batch(&self, max: usize) -> Result<Vec<T>, TryRecvError> {
    let mut out = Vec::new();
    self.try_recv_batch_mut(&mut out, max)?;
    Ok(out)
  }

  pub fn try_recv_batch_mut(&self, out: &mut Vec<T>, max: usize) -> Result<usize, TryRecvError> {
    if max == 0 {
      return Ok(0);
    }
    if self.closed.load(Ordering::Relaxed) {
      return Err(TryRecvError::Disconnected);
    }
    let mut pool = self.cache.lock();
    let k = self.pop_batch_lock_free(out, max, &mut *pool);
    if k > 0 {
      pool.flush(&self.shared);
      return Ok(k);
    }
    pool.flush(&self.shared);
    if !self.shared.senders_alive() {
      let k_end = self.pop_batch_lock_free(out, max, &mut *pool);
      if k_end > 0 {
        pool.flush(&self.shared);
        return Ok(k_end);
      }
      return Err(TryRecvError::Disconnected);
    }
    Err(TryRecvError::Empty)
  }

  fn pop_batch_lock_free(&self, out: &mut Vec<T>, max: usize, pool: &mut LocalCache<T>) -> usize {
    let mut popped = 0;
    unsafe {
      let mut tail = *self.shared.tail.get();
      while popped < max {
        let next = (*tail).next.load(Ordering::Acquire);
        if next.is_null() {
          break;
        }
        // see poll_pop: must leave the slot None, or the queue's drop glue
        // double-drops a stale value left in a never-resent free-list node
        let value = (&mut *(*next).val.get()).take().unwrap();
        *self.shared.tail.get() = next;
        self.recycle_node(tail, pool);
        tail = next;
        out.push(value);
        popped += 1;
      }
    }
    if popped > 0 {
      self.shared.current_len.fetch_sub(popped, Ordering::Relaxed);
    }
    popped
  }
}

impl<T: Send> Drop for AsyncReceiver<T> {
  fn drop(&mut self) {
    if self.is_registered {
      self.shared.unregister_async_recv();
    }
    self.cache.get_mut().flush(&self.shared);
    let _ = self.close();
  }
}

// --- Futures ---

#[must_use = "futures do nothing unless you .await or poll them"]
pub struct SendFuture<'a, T: Send> {
  sender: &'a AsyncSender<T>,
  item: Option<T>,
  my_id: Option<u64>,
  _phantom: PhantomPinned,
}

impl<'a, T: Send> SendFuture<'a, T> {
  fn new(sender: &'a AsyncSender<T>, item: T) -> Self {
    SendFuture {
      sender,
      item: Some(item),
      my_id: None,
      _phantom: PhantomPinned,
    }
  }

  fn unregister(&mut self) {
    if let Some(id) = self.my_id.take() {
      self.sender.shared.unregister_async_send(id);
    }
  }
}

impl<'a, T: Send> Future for SendFuture<'a, T> {
  type Output = Result<(), SendError>;

  fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    let this = unsafe { self.as_mut().get_unchecked_mut() };
    let shared = &this.sender.shared;

    let mut pool = this.sender.shared.cache.lock();

    loop {
      let item_val = match this.item.take() {
        Some(it) => it,
        None => return Poll::Ready(Ok(())),
      };

      if this.sender.closed.load(Ordering::Relaxed) || !shared.receivers_alive() {
        this.unregister();
        return Poll::Ready(Err(SendError::Closed));
      }

      if !pool.fill_pool(shared) {
        let id = shared.register_async_send(
          this.my_id,
          cx.waker().clone(),
          std::ptr::null(),
        );
        this.my_id = Some(id);
        shared.pre_park_fence();

        if this.sender.closed.load(Ordering::Relaxed) || !shared.receivers_alive() {
          shared.unregister_async_send(id);
          this.my_id = None;
          this.item = Some(item_val);
          return Poll::Ready(Err(SendError::Closed));
        }

        if !pool.fill_pool(shared) {
          this.item = Some(item_val);
          return Poll::Pending;
        }
      }

      unsafe {
        let curr = pool.pop_node(shared).unwrap();
        (*curr).val.get().write(Some(item_val));
        (*curr).next.store(std::ptr::null_mut(), Ordering::Relaxed);

        let old_head = shared.head.swap(curr, Ordering::AcqRel);
        (*old_head).next.store(curr, Ordering::Release);
      }

      shared.current_len.fetch_add(1, Ordering::Relaxed);
      shared.notify_receiver();

      if let Some(id) = this.my_id.take() {
        shared.unregister_async_send(id);
      }
      return Poll::Ready(Ok(()));
    }
  }
}

impl<'a, T: Send> Drop for SendFuture<'a, T> {
  fn drop(&mut self) {
    self.unregister();
  }
}

#[must_use = "futures do nothing unless you .await or poll them"]
pub struct RecvFuture<'a, T: Send> {
  receiver: &'a AsyncReceiver<T>,
  is_registered: bool,
}

impl<'a, T: Send> RecvFuture<'a, T> {
  fn new(receiver: &'a AsyncReceiver<T>) -> Self {
    RecvFuture {
      receiver,
      is_registered: false,
    }
  }
}

impl<'a, T: Send> Future for RecvFuture<'a, T> {
  type Output = Result<T, RecvError>;

  fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    let this = unsafe { self.get_unchecked_mut() };
    let shared = &this.receiver.shared;

    if this.receiver.closed.load(Ordering::Relaxed) {
      return Poll::Ready(Err(RecvError::Disconnected));
    }

    loop {
      let was_registered = this.is_registered;
      match shared.poll_pop(cx, &mut this.is_registered) {
        Poll::Ready(Ok((tail, value))) => {
          let mut pool = this.receiver.cache.lock();
          this.receiver.recycle_node(tail, &mut *pool);
          return Poll::Ready(Ok(value));
        }
        Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
        Poll::Pending => {
          if !was_registered {
            this.receiver.cache.lock().flush(shared);
          }
          return Poll::Pending;
        }
      }
    }
  }
}

impl<'a, T: Send> Drop for RecvFuture<'a, T> {
  fn drop(&mut self) {
    if self.is_registered {
      self.receiver.shared.unregister_async_recv();
    }
  }
}

impl<T: Send> Stream for AsyncReceiver<T> {
  type Item = T;

  fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
    let this = unsafe { self.as_mut().get_unchecked_mut() };
    let shared = &this.shared;

    if this.closed.load(Ordering::Relaxed) {
      return Poll::Ready(None);
    }

    loop {
      let was_registered = this.is_registered;
      match shared.poll_pop(cx, &mut this.is_registered) {
        Poll::Ready(Ok((tail, value))) => {
          let mut pool = this.cache.lock();
          this.recycle_node(tail, &mut *pool);
          return Poll::Ready(Some(value));
        }
        Poll::Ready(Err(_)) => return Poll::Ready(None),
        Poll::Pending => {
          if !was_registered {
            this.cache.lock().flush(shared);
          }
          return Poll::Pending;
        }
      }
    }
  }
}

// --- Async Batch Futures ---

#[must_use = "futures do nothing unless you .await or poll them"]
pub struct BoundedSendBatchFuture<'a, T: Send> {
  sender: &'a AsyncSender<T>,
  iter: std::vec::IntoIter<T>,
  total: usize,
  sent: usize,
  my_id: Option<u64>,
  _phantom: PhantomPinned,
}

impl<'a, T: Send> Future for BoundedSendBatchFuture<'a, T> {
  type Output = Result<usize, SendBatchError<T>>;

  fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    let this = unsafe { self.as_mut().get_unchecked_mut() };
    let shared = &this.sender.shared;
    let mut pool = this.sender.shared.cache.lock();

    loop {
      if this.sent == this.total {
        if let Some(id) = this.my_id.take() {
          shared.unregister_async_send(id);
        }
        return Poll::Ready(Ok(this.total));
      }

      if this.sender.closed.load(Ordering::Relaxed) || !shared.receivers_alive() {
        if let Some(id) = this.my_id.take() {
          shared.unregister_async_send(id);
        }
        return Poll::Ready(Err(SendBatchError {
          sent: this.sent,
          unsent: this.iter.by_ref().collect(),
        }));
      }

      if !pool.fill_pool(shared) {
        let id = shared.register_async_send(
          this.my_id,
          cx.waker().clone(),
          std::ptr::null(),
        );
        this.my_id = Some(id);
        shared.pre_park_fence();

        if this.sender.closed.load(Ordering::Relaxed) || !shared.receivers_alive() {
          shared.unregister_async_send(id);
          this.my_id = None;
          return Poll::Ready(Err(SendBatchError {
            sent: this.sent,
            unsent: this.iter.by_ref().collect(),
          }));
        }

        if !pool.fill_pool(shared) {
          return Poll::Pending;
        }
      }

      let remaining = this.total - this.sent;
      let local_available = pool.count;
      let k = remaining.min(local_available);

      unsafe {
        let first_node = pool.head;
        let mut curr = first_node;
        for _ in 0..(k - 1) {
          let next = (*curr).next.load(Ordering::Relaxed);
          let item_val = this.iter.next().unwrap();
          (*curr).val.get().write(Some(item_val));
          curr = next;
        }
        let next_free = (*curr).next.load(Ordering::Relaxed);
        let item_val = this.iter.next().unwrap();
        (*curr).val.get().write(Some(item_val));
        (*curr).next.store(std::ptr::null_mut(), Ordering::Relaxed);

        pool.head = next_free;
        pool.count -= k;

        let old_head = shared.head.swap(curr, Ordering::AcqRel);
        (*old_head).next.store(first_node, Ordering::Release);
      }

      shared.current_len.fetch_add(k, Ordering::Relaxed);
      shared.notify_receiver();
      this.sent += k;
    }
  }
}

impl<'a, T: Send> Drop for BoundedSendBatchFuture<'a, T> {
  fn drop(&mut self) {
    if let Some(id) = self.my_id.take() {
      self.sender.shared.unregister_async_send(id);
    }
  }
}

#[must_use = "futures do nothing unless you .await or poll them"]
pub struct BoundedSendBatchMutFuture<'a, T: Send> {
  sender: &'a AsyncSender<T>,
  items: &'a mut Vec<T>,
  sent: usize,
  my_id: Option<u64>,
  _phantom: PhantomPinned,
}

impl<'a, T: Send> Future for BoundedSendBatchMutFuture<'a, T> {
  type Output = Result<usize, SendError>;

  fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    let this = unsafe { self.as_mut().get_unchecked_mut() };
    let shared = &this.sender.shared;
    let mut pool = this.sender.shared.cache.lock();

    loop {
      let remaining = this.items.len();
      if remaining == 0 {
        if let Some(id) = this.my_id.take() {
          shared.unregister_async_send(id);
        }
        return Poll::Ready(Ok(this.sent));
      }

      if this.sender.closed.load(Ordering::Relaxed) || !shared.receivers_alive() {
        if let Some(id) = this.my_id.take() {
          shared.unregister_async_send(id);
        }
        return Poll::Ready(Err(SendError::Closed));
      }

      if !pool.fill_pool(shared) {
        let id = shared.register_async_send(
          this.my_id,
          cx.waker().clone(),
          std::ptr::null(),
        );
        this.my_id = Some(id);
        shared.pre_park_fence();

        if this.sender.closed.load(Ordering::Relaxed) || !shared.receivers_alive() {
          shared.unregister_async_send(id);
          this.my_id = None;
          return Poll::Ready(Err(SendError::Closed));
        }

        if !pool.fill_pool(shared) {
          return Poll::Pending;
        }
      }

      let local_available = pool.count;
      let k = remaining.min(local_available);

      unsafe {
        let first_node = pool.head;
        let mut curr = first_node;
        let mut drain = this.items.drain(..k);
        for _ in 0..(k - 1) {
          let next = (*curr).next.load(Ordering::Relaxed);
          let item_val = drain.next().unwrap();
          (*curr).val.get().write(Some(item_val));
          curr = next;
        }
        let next_free = (*curr).next.load(Ordering::Relaxed);
        let item_val = drain.next().unwrap();
        (*curr).val.get().write(Some(item_val));
        (*curr).next.store(std::ptr::null_mut(), Ordering::Relaxed);
        drop(drain);

        pool.head = next_free;
        pool.count -= k;

        let old_head = shared.head.swap(curr, Ordering::AcqRel);
        (*old_head).next.store(first_node, Ordering::Release);
      }

      shared.current_len.fetch_add(k, Ordering::Relaxed);
      shared.notify_receiver();
      this.sent += k;
    }
  }
}

impl<'a, T: Send> Drop for BoundedSendBatchMutFuture<'a, T> {
  fn drop(&mut self) {
    if let Some(id) = self.my_id.take() {
      self.sender.shared.unregister_async_send(id);
    }
  }
}

#[must_use = "futures do nothing unless you .await or poll them"]
pub struct BoundedRecvBatchFuture<'a, T: Send> {
  receiver: &'a AsyncReceiver<T>,
  max: usize,
  is_registered: bool,
}

impl<'a, T: Send> Future for BoundedRecvBatchFuture<'a, T> {
  type Output = Result<Vec<T>, RecvError>;

  fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    let this = unsafe { self.get_unchecked_mut() };
    if this.max == 0 {
      return Poll::Ready(Ok(Vec::new()));
    }
    let mut out = Vec::new();
    match poll_recv_batch_async_impl(
      this.receiver,
      cx,
      &mut out,
      this.max,
      &mut this.is_registered,
    ) {
      Poll::Ready(Ok(_)) => Poll::Ready(Ok(out)),
      Poll::Ready(Err(e)) => Poll::Ready(Err(e)),
      Poll::Pending => Poll::Pending,
    }
  }
}

impl<'a, T: Send> Drop for BoundedRecvBatchFuture<'a, T> {
  fn drop(&mut self) {
    if self.is_registered {
      self.receiver.shared.unregister_async_recv();
    }
  }
}

#[must_use = "futures do nothing unless you .await or poll them"]
pub struct BoundedRecvBatchMutFuture<'a, T: Send> {
  receiver: &'a AsyncReceiver<T>,
  out: &'a mut Vec<T>,
  max: usize,
  is_registered: bool,
}

impl<'a, T: Send> Future for BoundedRecvBatchMutFuture<'a, T> {
  type Output = Result<usize, RecvError>;

  fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    let this = unsafe { self.as_mut().get_unchecked_mut() };
    if this.max == 0 {
      return Poll::Ready(Ok(0));
    }
    let max = this.max;
    poll_recv_batch_async_impl(this.receiver, cx, this.out, max, &mut this.is_registered)
  }
}

impl<'a, T: Send> Drop for BoundedRecvBatchMutFuture<'a, T> {
  fn drop(&mut self) {
    if self.is_registered {
      self.receiver.shared.unregister_async_recv();
    }
  }
}

fn poll_recv_batch_async_impl<T: Send>(
  receiver: &AsyncReceiver<T>,
  cx: &mut Context<'_>,
  out: &mut Vec<T>,
  max: usize,
  is_registered: &mut bool,
) -> Poll<Result<usize, RecvError>> {
  if receiver.closed.load(Ordering::Relaxed) {
    return Poll::Ready(Err(RecvError::Disconnected));
  }
  let shared = &receiver.shared;

  let mut pool = receiver.cache.lock();

  loop {
    let k = receiver.pop_batch_lock_free(out, max, &mut *pool);
    if k > 0 {
      if *is_registered {
        shared.unregister_async_recv();
        *is_registered = false;
      }
      pool.flush(shared);
      return Poll::Ready(Ok(k));
    }

    pool.flush(shared);

    if !shared.senders_alive() {
      let k_end = receiver.pop_batch_lock_free(out, max, &mut *pool);
      if k_end > 0 {
        if *is_registered {
          shared.unregister_async_recv();
          *is_registered = false;
        }
        pool.flush(shared);
        return Poll::Ready(Ok(k_end));
      }
      if *is_registered {
        shared.unregister_async_recv();
        *is_registered = false;
      }
      return Poll::Ready(Err(RecvError::Disconnected));
    }

    shared.register_async_recv(cx.waker().clone(), std::ptr::null());
    *is_registered = true;
    shared.pre_park_fence();

    let k_after = receiver.pop_batch_lock_free(out, max, &mut pool);
    if k_after > 0 {
      shared.unregister_async_recv();
      *is_registered = false;
      pool.flush(shared);
      return Poll::Ready(Ok(k_after));
    }
    if !shared.senders_alive() {
      continue;
    }

    return Poll::Pending;
  }
}

impl<T: Send> AsyncSender<T> {
  pub fn send_batch(&self, items: Vec<T>) -> BoundedSendBatchFuture<'_, T> {
    let total = items.len();
    BoundedSendBatchFuture {
      sender: self,
      iter: items.into_iter(),
      total,
      sent: 0,
      my_id: None,
      _phantom: PhantomPinned,
    }
  }

  pub fn send_batch_mut<'a>(&'a self, items: &'a mut Vec<T>) -> BoundedSendBatchMutFuture<'a, T> {
    BoundedSendBatchMutFuture {
      sender: self,
      items,
      sent: 0,
      my_id: None,
      _phantom: PhantomPinned,
    }
  }
}

impl<T: Send> AsyncReceiver<T> {
  pub fn recv_batch(&self, max: usize) -> BoundedRecvBatchFuture<'_, T> {
    BoundedRecvBatchFuture {
      receiver: self,
      max,
      is_registered: false,
    }
  }

  pub fn recv_batch_mut<'a>(
    &'a self,
    out: &'a mut Vec<T>,
    max: usize,
  ) -> BoundedRecvBatchMutFuture<'a, T> {
    BoundedRecvBatchMutFuture {
      receiver: self,
      out,
      max,
      is_registered: false,
    }
  }
}

pub fn bounded_async<T: Send>(capacity: usize) -> (AsyncSender<T>, AsyncReceiver<T>) {
  let shared = Arc::new(BoundedQueue::new(capacity));
  let sender = AsyncSender {
    shared: Arc::clone(&shared),
    closed: AtomicBool::new(false),
  };
  let receiver = AsyncReceiver {
    shared,
    closed: AtomicBool::new(false),
    is_registered: false,
    cache: Mutex::new(LocalCache::default()),
  };
  (sender, receiver)
}

pub fn bounded<T: Send>(capacity: usize) -> (Sender<T>, Receiver<T>) {
  let shared = Arc::new(BoundedQueue::new(capacity));
  let sender = Sender {
    shared: Arc::clone(&shared),
    closed: AtomicBool::new(false),
  };
  let receiver = Receiver {
    shared,
    closed: AtomicBool::new(false),
    cache: RefCell::new(LocalCache::default()),
  };
  (sender, receiver)
}

// --- Test Suite ---

#[cfg(test)]
mod tests {
  use super::*;
  use std::sync::Arc;
  use std::sync::atomic::AtomicUsize as TestAtomicUsize;
  use std::task::{RawWaker, RawWakerVTable};

  struct SimpleRng {
    state: u32,
  }

  impl SimpleRng {
    fn new(seed: u32) -> Self {
      Self { state: seed }
    }

    fn next_u32(&mut self) -> u32 {
      self.state = self.state.wrapping_mul(1664525).wrapping_add(1013904223);
      self.state
    }

    fn gen_range(&mut self, min: u32, max: u32) -> u32 {
      assert!(min < max);
      min + (self.next_u32() % (max - min))
    }
  }

  struct DropTracker {
    counter: Arc<TestAtomicUsize>,
  }

  impl Drop for DropTracker {
    fn drop(&mut self) {
      self.counter.fetch_add(1, Ordering::SeqCst);
    }
  }

  fn mock_waker() -> Waker {
    unsafe fn clone(_: *const ()) -> RawWaker {
      RawWaker::new(std::ptr::null(), &VTABLE)
    }
    unsafe fn wake(_: *const ()) {}
    unsafe fn wake_by_ref(_: *const ()) {}
    unsafe fn drop_raw(_: *const ()) {}
    static VTABLE: RawWakerVTable = RawWakerVTable::new(clone, wake, wake_by_ref, drop_raw);
    unsafe { Waker::from_raw(RawWaker::new(std::ptr::null(), &VTABLE)) }
  }

  // --- Helper Functions for Benchmarking ---

  fn print_benchmark_row(label: &str, elapsed_ms: f64, throughput_melem: f64) {
    println!(
      "Configuration: {:<15} | Time: {:<8.3} ms | Throughput: {:<8.3} Melem/s",
      label, elapsed_ms, throughput_melem
    );
  }

  fn run_sync_benchmark(prod_count: usize, total_items: usize, capacity: usize) -> (f64, f64) {
    let (tx, rx) = bounded::<usize>(capacity);
    let items_per_producer = total_items / prod_count;
    let actual_total_items = items_per_producer * prod_count;

    let start = Instant::now();

    // 1. Spawn Consumer Thread
    let consumer_handle = thread::spawn(move || {
      let mut count = 0;
      while count < actual_total_items {
        if rx.recv().is_ok() {
          count += 1;
        }
      }
      count
    });

    // 2. Spawn Producer Threads
    let mut producers = Vec::new();
    for _ in 0..prod_count {
      let tx_clone = tx.clone();
      producers.push(thread::spawn(move || {
        for seq in 0..items_per_producer {
          let _ = tx_clone.send(seq);
        }
      }));
    }
    drop(tx); // Close original handle to ensure clean termination on completion

    // 3. Await all completions
    for p in producers {
      p.join().unwrap();
    }
    let received = consumer_handle.join().unwrap();
    let elapsed = start.elapsed();

    assert_eq!(received, actual_total_items);

    let elapsed_secs = elapsed.as_secs_f64();
    let throughput = (actual_total_items as f64 / elapsed_secs) / 1_000_000.0;
    (elapsed_secs * 1000.0, throughput)
  }

  async fn run_async_benchmark(
    prod_count: usize,
    total_items: usize,
    capacity: usize,
  ) -> (f64, f64) {
    let (tx, rx) = bounded_async::<usize>(capacity);
    let items_per_producer = total_items / prod_count;
    let actual_total_items = items_per_producer * prod_count;

    let start = Instant::now();

    // 1. Spawn Consumer Task
    let consumer_handle = tokio::spawn(async move {
      let mut count = 0;
      while count < actual_total_items {
        if rx.recv().await.is_ok() {
          count += 1;
        }
      }
      count
    });

    // 2. Spawn Producer Tasks
    let mut producers = Vec::new();
    for _ in 0..prod_count {
      let tx_clone = tx.clone();
      producers.push(tokio::spawn(async move {
        for seq in 0..items_per_producer {
          let _ = tx_clone.send(seq).await;
        }
      }));
    }
    drop(tx);

    // 3. Await all completions
    for p in producers {
      p.await.unwrap();
    }
    let received = consumer_handle.await.unwrap();
    let elapsed = start.elapsed();

    assert_eq!(received, actual_total_items);

    let elapsed_secs = elapsed.as_secs_f64();
    let throughput = (actual_total_items as f64 / elapsed_secs) / 1_000_000.0;
    (elapsed_secs * 1000.0, throughput)
  }

  // --- Basic Functionality Tests ---

  #[test]
  fn test_queue_initial_state() {
    let q = BoundedQueue::<i32>::new(4);
    assert_eq!(q.capacity(), 4); // Dynamically scaled chunk size means capacity is exactly 4!
    assert_eq!(q.len(), 0);
    assert!(q.is_empty());
    assert!(!q.is_full());
  }

  #[test]
  fn test_try_push_pop_sequence() {
    let (tx, rx) = bounded::<i32>(2);
    tx.try_send(10).unwrap();
    assert_eq!(tx.len(), 1);
    assert!(!tx.is_empty());

    tx.try_send(20).unwrap();
    assert_eq!(tx.len(), 2);
    assert!(tx.is_full());

    // 3rd push must return Full since capacity is exactly 2
    assert!(matches!(tx.try_send(30), Err(TrySendError::Full(30))));

    assert_eq!(rx.try_recv().unwrap(), 10);
    assert_eq!(rx.try_recv().unwrap(), 20);
    assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
  }

  #[test]
  fn test_push_pop_blocking_coordination() {
    let (tx, rx) = bounded(1);
    tx.send(100).unwrap();

    let tx_clone = tx.clone();
    let producer_thread = thread::spawn(move || {
      tx_clone.send(200).unwrap();
    });

    thread::sleep(Duration::from_millis(50));
    assert!(!producer_thread.is_finished());

    assert_eq!(rx.recv().unwrap(), 100);
    producer_thread.join().unwrap();
    assert_eq!(rx.recv().unwrap(), 200);
  }

  #[test]
  fn test_pop_timeout_behavior() {
    let (_tx, rx) = bounded::<i32>(1);
    let start = Instant::now();
    let res = rx.recv_timeout(Duration::from_millis(50));
    assert!(matches!(res, Err(RecvErrorTimeout::Timeout)));
    assert!(start.elapsed() >= Duration::from_millis(45));
  }

  #[test]
  fn test_receiver_drop_unblocks_pushers() {
    let (tx, rx) = bounded::<i32>(1);
    tx.send(1).unwrap();

    let tx_clone = tx.clone();
    let handle = thread::spawn(move || tx_clone.send(2));

    thread::sleep(Duration::from_millis(50));
    assert!(!handle.is_finished());

    drop(rx);

    let res = handle.join().unwrap();
    assert!(matches!(res, Err(SendError::Closed)));
  }

  // --- Async Manual Polling Validation ---

  #[test]
  fn test_manual_async_polling_state_machine() {
    // Capacity 16 fills exactly 1 chunk
    let (tx, rx) = bounded_async::<i32>(16);
    let waker = mock_waker();
    let mut cx = Context::from_waker(&waker);

    // Fill all 16 slots (one chunk)
    for i in 0..16 {
      let mut fut = Box::pin(tx.send(i));
      assert!(fut.as_mut().poll(&mut cx).is_ready());
    }

    // Now the 17th push must return Pending!
    let mut blocked_fut = Box::pin(tx.send(999));
    assert!(blocked_fut.as_mut().poll(&mut cx).is_pending());

    // Drain all 16 items so the receiver's local recycle pool fills and
    // releases a complete chunk back to the global stack.
    for i in 0..16 {
      let mut recv_fut = Box::pin(rx.recv());
      let recv_res = recv_fut.as_mut().poll(&mut cx);
      assert!(matches!(recv_res, Poll::Ready(Ok(v)) if v == i));
    }

    // Re-poll original pending push; it should now complete
    assert!(blocked_fut.as_mut().poll(&mut cx).is_ready());
  }

  // --- Concurrent Stress Fuzzing ---

  #[test]
  fn test_concurrent_differential_fuzz() {
    const TOTAL_ITEMS: usize = 50_000;
    const CAPACITY: usize = 128;
    const PRODUCER_COUNT: usize = 4;

    let (tx, rx) = bounded::<usize>(CAPACITY);
    let mut producer_handles = Vec::new();

    // Spawn Producers
    for thread_idx in 0..PRODUCER_COUNT {
      let tx_clone = tx.clone();
      producer_handles.push(thread::spawn(move || {
        let mut rng = SimpleRng::new(1337 + thread_idx as u32);
        let items_to_send = TOTAL_ITEMS / PRODUCER_COUNT;

        for seq in 0..items_to_send {
          let msg = (thread_idx << 32) | seq;

          if rng.gen_range(0, 100) < 30 {
            while let Err(e) = tx_clone.try_send(msg) {
              match e {
                TrySendError::Full(_) => thread::yield_now(),
                TrySendError::Closed(_) => panic!("Queue unexpectedly closed"),
                TrySendError::Sent(_) => {
                  unreachable!("TrySendError::Sent is not possible on a bounded queue")
                }
              }
            }
          } else {
            tx_clone.send(msg).unwrap();
          }

          if rng.gen_range(0, 100) < 10 {
            thread::yield_now();
          }
        }
      }));
    }
    drop(tx);

    // Spawn Consumer
    let consumer_handle = thread::spawn(move || {
      let mut expected_seqs = vec![0usize; PRODUCER_COUNT];
      let mut received_count = 0;
      let mut rng = SimpleRng::new(9999);

      while received_count < (TOTAL_ITEMS / PRODUCER_COUNT) * PRODUCER_COUNT {
        let pop_res = if rng.gen_range(0, 100) < 20 {
          match rx.try_recv() {
            Ok(msg) => Some(msg),
            Err(TryRecvError::Empty) => {
              thread::yield_now();
              None
            }
            Err(TryRecvError::Disconnected) => panic!("Queue disconnected prematurely"),
          }
        } else if rng.gen_range(0, 100) < 40 {
          match rx.recv_timeout(Duration::from_micros(10)) {
            Ok(msg) => Some(msg),
            Err(RecvErrorTimeout::Timeout) => None,
            Err(RecvErrorTimeout::Disconnected) => panic!("Queue disconnected prematurely"),
          }
        } else {
          Some(rx.recv().unwrap())
        };

        if let Some(msg) = pop_res {
          let origin_thread = msg >> 32;
          let seq = msg & 0xFFFF_FFFF;

          assert_eq!(
            seq, expected_seqs[origin_thread],
            "Out of order item received from thread {}: expected {}, got {}",
            origin_thread, expected_seqs[origin_thread], seq
          );

          expected_seqs[origin_thread] += 1;
          received_count += 1;
        }
      }
      received_count
    });

    for h in producer_handles {
      h.join().unwrap();
    }

    let total_received = consumer_handle.join().unwrap();
    assert_eq!(
      total_received,
      (TOTAL_ITEMS / PRODUCER_COUNT) * PRODUCER_COUNT
    );
  }

  // --- Discrete Synchronous & Asynchronous Performance Benchmarks ---

  #[test]
  fn test_mpsc_sync_performance_profile() {
    let profiles = [
      (1, "1 to 1 (SPSC)"),
      (4, "4 to 1 (MPSC)"),
      (14, "14 to 1 (MPSC)"),
    ];

    const TOTAL_ITEMS: usize = 2_000_000;
    const CAPACITY: usize = 128;

    println!("\n=== MPSC_queue Concurrency Benchmark (Sync) ===");
    println!(
      "Workload Size: {} elements | Capacity: {}",
      TOTAL_ITEMS, CAPACITY
    );

    for &(prod_count, label) in &profiles {
      let (elapsed_ms, throughput) = run_sync_benchmark(prod_count, TOTAL_ITEMS, CAPACITY);
      print_benchmark_row(label, elapsed_ms, throughput);
    }
    println!("========================================================\n");
  }

  #[tokio::test]
  async fn test_mpsc_async_performance_profile() {
    let profiles = [
      (1, "1 to 1 (SPSC)"),
      (4, "4 to 1 (MPSC)"),
      (14, "14 to 1 (MPSC)"),
    ];

    const TOTAL_ITEMS: usize = 2_000_000;
    const CAPACITY: usize = 128;

    println!("\n=== Ported MPSC_queue Concurrency Benchmark (Async) ===");
    println!(
      "Workload Size: {} elements | Capacity: {}",
      TOTAL_ITEMS, CAPACITY
    );

    for &(prod_count, label) in &profiles {
      let (elapsed_ms, throughput) = run_async_benchmark(prod_count, TOTAL_ITEMS, CAPACITY).await;
      print_benchmark_row(label, elapsed_ms, throughput);
    }
    println!("========================================================\n");
  }
}

#[cfg(test)]
mod sync_batch_tests {
  use super::*;

  #[test]
  fn test_sync_send_batch_recv_batch() {
    let (tx, rx) = bounded::<i32>(32);

    let batch = vec![1, 2, 3, 4, 5];
    tx.send_batch(batch).unwrap();

    let values = rx.recv_batch(5).unwrap();
    assert_eq!(values, vec![1, 2, 3, 4, 5]);
  }

  #[test]
  fn test_sync_send_batch_mut_recv_batch_mut() {
    let (tx, rx) = bounded::<i32>(32);

    let mut batch = vec![10, 20, 30];
    tx.send_batch_mut(&mut batch).unwrap();
    assert!(batch.is_empty());

    let mut out = Vec::new();
    let count = rx.recv_batch_mut(&mut out, 3).unwrap();
    assert_eq!(count, 3);
    assert_eq!(out, vec![10, 20, 30]);
  }

  #[test]
  fn test_sync_batch_partial_completions() {
    let (tx, rx) = bounded::<i32>(16);

    let mut fill = vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];
    tx.send_batch_mut(&mut fill).unwrap();

    let tx_clone = tx.clone();
    let blocked_send = std::thread::spawn(move || tx_clone.send_batch(vec![17, 18, 19]));

    std::thread::sleep(Duration::from_millis(50));
    assert!(!blocked_send.is_finished());

    let mut out = Vec::new();
    let popped = rx.recv_batch_mut(&mut out, 3).unwrap();
    assert_eq!(popped, 3);
    assert_eq!(out, vec![1, 2, 3]);

    let send_result = blocked_send.join().unwrap().unwrap();
    assert_eq!(send_result, 3);
  }
}

#[cfg(test)]
mod batch_tests {
  use super::*;

  #[tokio::test]
  async fn test_async_send_batch_recv_batch() {
    let (tx, rx) = bounded_async::<i32>(32);

    let batch = vec![10, 20, 30, 40, 50];
    tx.send_batch(batch).await.unwrap();

    let values = rx.recv_batch(5).await.unwrap();
    assert_eq!(values, vec![10, 20, 30, 40, 50]);
  }

  #[tokio::test]
  async fn test_async_send_batch_mut_recv_batch_mut() {
    let (tx, rx) = bounded_async::<i32>(32);

    let mut batch = vec![100, 200, 300];
    tx.send_batch_mut(&mut batch).await.unwrap();
    assert!(batch.is_empty());

    let mut out = Vec::new();
    let count = rx.recv_batch_mut(&mut out, 3).await.unwrap();
    assert_eq!(count, 3);
    assert_eq!(out, vec![100, 200, 300]);
  }

  #[tokio::test]
  async fn test_async_batch_partial_completions() {
    let (tx, rx) = bounded_async::<i32>(16);

    let mut fill = vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];
    tx.send_batch_mut(&mut fill).await.unwrap();

    let tx_clone = tx.clone();
    let blocked_send = tokio::spawn(async move { tx_clone.send_batch(vec![17, 18, 19]).await });

    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!blocked_send.is_finished());

    let mut out = Vec::new();
    let popped = rx.recv_batch_mut(&mut out, 3).await.unwrap();
    assert_eq!(popped, 3);
    assert_eq!(out, vec![1, 2, 3]);

    let send_result = blocked_send.await.unwrap().unwrap();
    assert_eq!(send_result, 3);
  }
}
