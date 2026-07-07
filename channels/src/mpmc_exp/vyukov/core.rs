//! Core of the lock-free MPMC channel (`mpmc_v3`).
//!
//! # Design
//!
//! The data path is a canonical Vyukov bounded MPMC ring: a fixed,
//! heap-allocated array of seq-stamped slots, with monotonic `enqueue_pos` /
//! `dequeue_pos` cursors. `push`/`pop` are fully lock-free and never touch a
//! lock. Delivery is FIFO (the seq stamp fixes claim order) and there is no
//! per-call scan.
//!
//! Parking is the *only* place a lock is involved, and only when the ring is
//! transiently full (senders) or empty (receivers). A small `parking_lot::Mutex`
//! guards two per-role waiter queues. The fast path never acquires it: a producer
//! only wakes receivers if `recv_waiters != 0`, and vice-versa.
//!
//! ## Lost-wakeup safety
//!
//! The dangerous race is: a waiter decides to park while, concurrently, the other
//! side makes progress but doesn't see the registration and so doesn't wake it.
//! We close it with the classic two-fence (Dekker) pattern:
//!
//! - the waiter registers (bumps `*_waiters` under the lock), then
//!   `fence(SeqCst)`, then re-runs the lock-free op one last time before parking;
//! - the actor (after a successful `push`/`pop`) does `fence(SeqCst)`, then loads
//!   `*_waiters`; if non-zero it drains and wakes that role.
//!
//! Two `SeqCst` fences guarantee it is impossible for both "the actor missed the
//! registration" and "the waiter missed the progress" to be true at once - so at
//! least one side acts, and no wakeup is lost.
//!
//! ## Cancellation
//!
//! `push` claims, writes, and publishes in a single synchronous burst with no
//! suspension point, so a send/recv future can only ever be dropped while parked
//! for space/data - i.e. while holding no slot. Cancellation is therefore just
//! "unlink the parked waiter"; there is no in-buffer tombstone to resolve.

use std::cell::UnsafeCell;
use std::mem::MaybeUninit;
use std::ptr;
use std::sync::atomic::{fence, AtomicBool, AtomicUsize, Ordering};
use std::task::Waker;
use std::thread::Thread;

use parking_lot::Mutex;

use crate::internal::cache_padded::CachePadded;

// --- Ring slot ------------------------------------------------------------

struct Slot<T> {
  /// Vyukov sequence stamp. Initialized to the slot index. After a producer
  /// publishes position `p` it holds `p + 1`; after a consumer frees it it holds
  /// `p + capacity`.
  seq: AtomicUsize,
  val: UnsafeCell<MaybeUninit<T>>,
}

// --- Lock-free ring -------------------------------------------------------

pub(crate) struct Ring<T> {
  buf: Box<[CachePadded<Slot<T>>]>,
  mask: usize,
  /// Channel capacity: the requested `n`, `<= buf.len()`. The physical buffer
  /// (`mask + 1`) is rounded up to a power of two for index masking and is a
  /// separate quantity; fullness and `capacity()` both use this logical value.
  cap: usize,
  enqueue_pos: CachePadded<AtomicUsize>,
  dequeue_pos: CachePadded<AtomicUsize>,
}

impl<T> Ring<T> {
  fn new(capacity: usize) -> Self {
    assert!(capacity > 0, "mpmc_v3 capacity must be > 0");
    // Physical buffer: a power of two (>= 2) for fast index masking. The `>= 2`
    // floor also sidesteps the Vyukov stamp degeneracy at physical size 1. The
    // channel capacity (`capacity`) is enforced separately, so e.g. a requested
    // capacity of 1 yields a 2-slot buffer that still admits only 1 live item.
    let phys = capacity.next_power_of_two().max(2);
    let mut v = Vec::with_capacity(phys);
    for i in 0..phys {
      v.push(CachePadded::new(Slot {
        seq: AtomicUsize::new(i),
        val: UnsafeCell::new(MaybeUninit::uninit()),
      }));
    }
    Ring {
      buf: v.into_boxed_slice(),
      mask: phys - 1,
      cap: capacity,
      enqueue_pos: CachePadded::new(AtomicUsize::new(0)),
      dequeue_pos: CachePadded::new(AtomicUsize::new(0)),
    }
  }

  #[inline]
  pub(crate) fn capacity(&self) -> usize {
    self.cap
  }

  #[inline]
  pub(crate) fn len(&self) -> usize {
    // Two independent monotonic counters can't be snapshotted atomically, so
    // read `head`, then `tail`, then `head` again and retry until `head` is
    // stable across the `tail` read. That makes the pair reflect one real
    // instant - where `tail >= head` and `tail - head` is in `[0, cap]` - so the
    // result never underflows (torn read) or overshoots capacity. (Same approach
    // as crossbeam's `ArrayQueue::len`.)
    loop {
      let head = self.dequeue_pos.load(Ordering::Acquire);
      let tail = self.enqueue_pos.load(Ordering::Acquire);
      if head == self.dequeue_pos.load(Ordering::Acquire) {
        return tail.wrapping_sub(head);
      }
    }
  }

  #[inline]
  pub(crate) fn is_empty(&self) -> bool {
    self.len() == 0
  }

  #[inline]
  pub(crate) fn is_full(&self) -> bool {
    self.len() >= self.cap
  }

  /// Lock-free enqueue. Returns `Err(item)` if the ring is full.
  pub(crate) fn push(&self, item: T) -> Result<(), T> {
    let mut pos = self.enqueue_pos.load(Ordering::Relaxed);
    loop {
      // Enforce the channel capacity, which is independent of (and <=) the
      // physical buffer size. `head` is loaded Acquire and only ever advances,
      // so a stale read is conservative (at worst a spurious Full, resolved by
      // the caller's register+recheck).
      let head = self.dequeue_pos.load(Ordering::Acquire);
      if pos.wrapping_sub(head) >= self.cap {
        return Err(item);
      }
      let slot = &self.buf[pos & self.mask];
      let seq = slot.seq.load(Ordering::Acquire);
      let diff = seq as isize - pos as isize;
      if diff == 0 {
        // Slot is free for this lap; try to claim position `pos`.
        match self.enqueue_pos.compare_exchange_weak(
          pos,
          pos.wrapping_add(1),
          Ordering::Relaxed,
          Ordering::Relaxed,
        ) {
          Ok(_) => {
            unsafe { (*slot.val.get()).write(item) };
            slot.seq.store(pos.wrapping_add(1), Ordering::Release);
            return Ok(());
          }
          Err(actual) => pos = actual,
        }
      } else if diff < 0 {
        // Slot still holds an unconsumed item from the previous lap: full.
        return Err(item);
      } else {
        // Another producer claimed it; reload and retry.
        pos = self.enqueue_pos.load(Ordering::Relaxed);
      }
    }
  }

  /// Lock-free dequeue. Returns `None` if the ring is empty.
  pub(crate) fn pop(&self) -> Option<T> {
    let mut pos = self.dequeue_pos.load(Ordering::Relaxed);
    loop {
      let slot = &self.buf[pos & self.mask];
      let seq = slot.seq.load(Ordering::Acquire);
      let diff = seq as isize - (pos.wrapping_add(1)) as isize;
      if diff == 0 {
        match self.dequeue_pos.compare_exchange_weak(
          pos,
          pos.wrapping_add(1),
          Ordering::Relaxed,
          Ordering::Relaxed,
        ) {
          Ok(_) => {
            let item = unsafe { (*slot.val.get()).assume_init_read() };
            // Free the slot for the next lap (position `pos + capacity`).
            slot.seq.store(
              pos.wrapping_add(self.mask).wrapping_add(1),
              Ordering::Release,
            );
            return Some(item);
          }
          Err(actual) => pos = actual,
        }
      } else if diff < 0 {
        // Slot not yet published for this lap: empty (or a producer is mid-publish).
        return None;
      } else {
        pos = self.dequeue_pos.load(Ordering::Relaxed);
      }
    }
  }
}

impl<T> Drop for Ring<T> {
  fn drop(&mut self) {
    // Drop any remaining buffered values exactly once.
    while self.pop().is_some() {}
  }
}

// --- Waiters --------------------------------------------------------------

/// A wake handle: either a parked sync thread or an async task's waker.
#[derive(Clone)]
pub(crate) enum WakeRef {
  Thread(Thread),
  Waker(Waker),
}

impl WakeRef {
  #[inline]
  pub(crate) fn wake(self) {
    match self {
      WakeRef::Thread(t) => t.unpark(),
      WakeRef::Waker(w) => w.wake(),
    }
  }
}

/// Sets a waiter's stack-owned "notified" flag, if it has one. MUST be called
/// while holding the parkers lock: a waiter only frees this flag after its own
/// `unregister` (which needs that lock), so the pointer is live while held.
#[inline]
fn set_notified(notified: *const AtomicBool) {
  if !notified.is_null() {
    unsafe { (*notified).store(true, Ordering::Release) };
  }
}

/// Sentinel "no link" index for the slab's intrusive lists.
const NIL: u32 = u32::MAX;

struct SlabEntry {
  /// `Some` while the slot is live (registered); `None` while it sits on the
  /// free list.
  wake: Option<WakeRef>,
  /// Raw pointer to the waiter's stack-owned "notified" flag, or null. Set by
  /// `wake_one`/`wake_all` (a real wake) *under the parkers lock*, so the woken
  /// waiter can tell it was already removed and skip a redundant `unregister`.
  /// Not touched by `remove` (cancellation is not a wake). Sound because the
  /// waiter always `unregister`s (taking the same lock) before its frame goes
  /// away - see `set_notified`.
  notified: *const AtomicBool,
  /// Intrusive doubly-linked FIFO order links (live slots), or free-list `next`
  /// (recycled slots). `NIL` terminates a list.
  prev: u32,
  next: u32,
  /// Bumped on every free so any outstanding handle to a recycled slot is
  /// rejected by `remove` - makes post-wake `unregister` a safe no-op.
  _gen: u32,
}

/// O(1) FIFO waiter queue. A slab of `SlabEntry`s threaded by two intrusive
/// lists: the live FIFO order (`head`..`tail`) and a free list (`free_head`).
/// Handles are packed `(gen << 32 | index)` `u64`s. Every operation -
/// `push_back`, `pop_front`, `remove` - is O(1), so the parking critical section
/// no longer scales with the number of waiters.
struct WaiterSlab {
  entries: Vec<SlabEntry>,
  free_head: u32,
  head: u32,
  tail: u32,
  len: usize,
}

impl Default for WaiterSlab {
  fn default() -> Self {
    WaiterSlab {
      entries: Vec::new(),
      free_head: NIL,
      head: NIL,
      tail: NIL,
      len: 0,
    }
  }
}

impl WaiterSlab {
  #[inline]
  fn len(&self) -> usize {
    self.len
  }

  /// Registers `wake` (with its `notified` flag) at the FIFO tail and returns
  /// its packed handle.
  fn push_back(&mut self, wake: WakeRef, notified: *const AtomicBool) -> u64 {
    let idx = if self.free_head != NIL {
      let i = self.free_head;
      self.free_head = self.entries[i as usize].next;
      let e = &mut self.entries[i as usize];
      e.wake = Some(wake);
      e.notified = notified;
      e.prev = self.tail;
      e.next = NIL;
      i
    } else {
      let i = self.entries.len() as u32;
      self.entries.push(SlabEntry {
        wake: Some(wake),
        notified,
        prev: self.tail,
        next: NIL,
        _gen: 0,
      });
      i
    };
    if self.tail != NIL {
      self.entries[self.tail as usize].next = idx;
    } else {
      self.head = idx;
    }
    self.tail = idx;
    self.len += 1;
    let _gen = self.entries[idx as usize]._gen;
    ((_gen as u64) << 32) | idx as u64
  }

  /// Unlinks `idx` from the FIFO order, recycles it, bumps its generation, and
  /// returns its wake handle plus the waiter's `notified` flag pointer.
  fn remove_index(&mut self, idx: u32) -> Option<(WakeRef, *const AtomicBool)> {
    let (prev, next) = {
      let e = &self.entries[idx as usize];
      (e.prev, e.next)
    };
    if prev != NIL {
      self.entries[prev as usize].next = next;
    } else {
      self.head = next;
    }
    if next != NIL {
      self.entries[next as usize].prev = prev;
    } else {
      self.tail = prev;
    }
    let e = &mut self.entries[idx as usize];
    let wake = e.wake.take();
    let notified = e.notified;
    e.notified = ptr::null();
    e._gen = e._gen.wrapping_add(1);
    e.prev = NIL;
    e.next = self.free_head;
    self.free_head = idx;
    self.len -= 1;
    wake.map(|w| (w, notified))
  }

  #[inline]
  fn pop_front(&mut self) -> Option<(WakeRef, *const AtomicBool)> {
    if self.head == NIL {
      None
    } else {
      self.remove_index(self.head)
    }
  }

  /// Removes the waiter named by `id`. Returns `true` only if the handle was
  /// live (index in range and generation current); a stale handle is ignored.
  fn remove(&mut self, id: u64) -> bool {
    let idx = (id & 0xFFFF_FFFF) as u32;
    let _gen = (id >> 32) as u32;
    match self.entries.get(idx as usize) {
      Some(e) if e._gen == _gen && e.wake.is_some() => {
        self.remove_index(idx);
        true
      }
      _ => false,
    }
  }

  /// Drains every live waiter in FIFO order (close-time broadcast).
  fn drain_all(&mut self) -> Vec<(WakeRef, *const AtomicBool)> {
    let mut out = Vec::with_capacity(self.len);
    while let Some(w) = self.pop_front() {
      out.push(w);
    }
    out
  }
}

#[derive(Default)]
struct Parkers {
  send: WaiterSlab,
  recv: WaiterSlab,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Role {
  Send,
  Recv,
}

// --- Shared ---------------------------------------------------------------

pub(crate) struct Shared<T> {
  pub(crate) ring: Ring<T>,
  pub(crate) sender_count: AtomicUsize,
  pub(crate) receiver_count: AtomicUsize,
  parkers: Mutex<Parkers>,
  /// Cheap fast-path gates: number of parked waiters per role. Read (relaxed,
  /// after a SeqCst fence) by actors to decide whether to take the lock at all.
  send_waiters: CachePadded<AtomicUsize>,
  recv_waiters: CachePadded<AtomicUsize>,
}

unsafe impl<T: Send> Send for Shared<T> {}
unsafe impl<T: Send> Sync for Shared<T> {}

impl<T> Shared<T> {
  pub(crate) fn new(capacity: usize) -> Self {
    Shared {
      ring: Ring::new(capacity),
      sender_count: AtomicUsize::new(1),
      receiver_count: AtomicUsize::new(1),
      parkers: Mutex::new(Parkers::default()),
      send_waiters: CachePadded::new(AtomicUsize::new(0)),
      recv_waiters: CachePadded::new(AtomicUsize::new(0)),
    }
  }

  #[inline]
  pub(crate) fn senders_alive(&self) -> bool {
    self.sender_count.load(Ordering::Acquire) != 0
  }

  #[inline]
  pub(crate) fn receivers_alive(&self) -> bool {
    self.receiver_count.load(Ordering::Acquire) != 0
  }

  // --- Backend surface: buffer queries (so callers never touch the ring) -----

  #[inline]
  pub(crate) fn capacity(&self) -> usize {
    self.ring.capacity()
  }

  #[inline]
  pub(crate) fn len(&self) -> usize {
    self.ring.len()
  }

  #[inline]
  pub(crate) fn is_empty(&self) -> bool {
    self.ring.is_empty()
  }

  #[inline]
  pub(crate) fn is_full(&self) -> bool {
    self.ring.is_full()
  }

  /// Cancels a parked receiver waiter by id (backend-surface wrapper so callers
  /// don't name `Role`).
  #[inline]
  pub(crate) fn unregister_recv(&self, id: u64) {
    self.unregister(Role::Recv, id);
  }

  /// Diagnostic: (parked send waiters, parked recv waiters).
  #[inline]
  pub(crate) fn debug_waiters(&self) -> (usize, usize) {
    (
      self.send_waiters.load(Ordering::Relaxed),
      self.recv_waiters.load(Ordering::Relaxed),
    )
  }

  /// Registers a waiter for `role`, replacing any prior registration with the
  /// same `prev` id. Returns the new id. Updates the per-role gate count.
  pub(crate) fn register(
    &self,
    role: Role,
    prev: Option<u64>,
    wake: WakeRef,
    notified: *const AtomicBool,
  ) -> u64 {
    let mut g = self.parkers.lock();
    let (slab, count) = match role {
      Role::Send => (&mut g.send, &self.send_waiters),
      Role::Recv => (&mut g.recv, &self.recv_waiters),
    };
    if let Some(p) = prev {
      slab.remove(p);
    }
    let id = slab.push_back(wake, notified);
    // Relaxed store under the lock; ordered against the actor's load by the
    // SeqCst fence the caller issues next (see module docs).
    count.store(slab.len(), Ordering::Relaxed);
    id
  }

  /// Removes a registration (cancellation / success / timeout cleanup).
  pub(crate) fn unregister(&self, role: Role, id: u64) {
    let mut g = self.parkers.lock();
    let (slab, count) = match role {
      Role::Send => (&mut g.send, &self.send_waiters),
      Role::Recv => (&mut g.recv, &self.recv_waiters),
    };
    if slab.remove(id) {
      count.store(slab.len(), Ordering::Relaxed);
    }
  }

  /// Wakes exactly one parked waiter of `role`, in arrival (FIFO) order. Used
  /// after a single `push`/`pop` frees one unit of work - waking the whole pool
  /// would be a thundering herd, since only one waiter can claim that unit.
  fn wake_one(&self, role: Role) {
    let wake = {
      let mut g = self.parkers.lock();
      let (slab, count) = match role {
        Role::Send => (&mut g.send, &self.send_waiters),
        Role::Recv => (&mut g.recv, &self.recv_waiters),
      };
      match slab.pop_front() {
        Some((wake, notified)) => {
          count.store(slab.len(), Ordering::Relaxed);
          // Flag "you were removed" so the waiter skips a redundant
          // `unregister`. Done *under the lock*: a waiter only frees this flag
          // after its own `unregister`, which needs this same lock, so the
          // pointer is guaranteed live here. Release pairs with the waiter's
          // Acquire `swap` after it parks.
          set_notified(notified);
          Some(wake)
        }
        None => None,
      }
    };
    if let Some(wake) = wake {
      wake.wake();
    }
  }

  /// Wakes (broadcast, draining) every parked waiter of `role`. Used only on
  /// close, where every waiter must observe disconnection.
  fn wake_all(&self, role: Role) {
    let drained = {
      let mut g = self.parkers.lock();
      let (slab, count) = match role {
        Role::Send => (&mut g.send, &self.send_waiters),
        Role::Recv => (&mut g.recv, &self.recv_waiters),
      };
      if slab.len() == 0 {
        return;
      }
      count.store(0, Ordering::Relaxed);
      // Flag every waiter under the lock (see `wake_one`), then wake outside it.
      slab
        .drain_all()
        .into_iter()
        .map(|(wake, notified)| {
          set_notified(notified);
          wake
        })
        .collect::<Vec<_>>()
    };
    for wake in drained {
      wake.wake();
    }
  }

  /// Called by a producer after a successful `push`: wake the front parked
  /// receiver. The `SeqCst` fence pairs with the receiver's pre-park fence so a
  /// freshly registered receiver is never missed.
  #[inline]
  pub(crate) fn notify_receivers(&self) {
    fence(Ordering::SeqCst);
    if self.recv_waiters.load(Ordering::Relaxed) != 0 {
      self.wake_one(Role::Recv);
    }
  }

  /// Called by a consumer after a successful `pop`: wake the front parked sender.
  #[inline]
  pub(crate) fn notify_senders(&self) {
    fence(Ordering::SeqCst);
    if self.send_waiters.load(Ordering::Relaxed) != 0 {
      self.wake_one(Role::Send);
    }
  }

  /// The pre-park `SeqCst` fence; call after `register` and before the final
  /// recheck of the lock-free op.
  #[inline]
  pub(crate) fn pre_park_fence(&self) {
    fence(Ordering::SeqCst);
  }

  // --- Close ---------------------------------------------------------------

  /// Drops one sender. If it was the last, wakes all parked receivers so they
  /// observe disconnection.
  pub(crate) fn drop_sender(&self) {
    if self.sender_count.fetch_sub(1, Ordering::AcqRel) == 1 {
      self.wake_all(Role::Recv);
    }
  }

  /// Drops one receiver. If it was the last, wakes all parked senders so they
  /// observe closure.
  pub(crate) fn drop_receiver(&self) {
    if self.receiver_count.fetch_sub(1, Ordering::AcqRel) == 1 {
      self.wake_all(Role::Send);
    }
  }

  pub(crate) fn add_sender(&self) {
    self.sender_count.fetch_add(1, Ordering::AcqRel);
  }

  pub(crate) fn add_receiver(&self) {
    self.receiver_count.fetch_add(1, Ordering::AcqRel);
  }
}

impl<T: Send> std::fmt::Debug for Shared<T> {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_struct("Shared")
      .field("capacity", &self.capacity())
      .field("len", &self.len())
      .finish_non_exhaustive()
  }
}
