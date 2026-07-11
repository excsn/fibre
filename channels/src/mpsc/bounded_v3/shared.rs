//! Shared core of Bounded MPSC V3 - the credit-BEFORE-claim MPSC.
//!
//! - **Chunk-table recycling.** A FIXED table of `N` chunk slots (each a reused
//!   allocation) is indexed by `chunk_id % N`; an `id`-epoch on each entry IS
//!   the generation, so there is no ABA, no `prev`/`next` links, no stale-hint
//!   hazard. Slots are cleared **reset-on-drain**: the consumer stores EMPTY as
//!   it takes each item, so a reused chunk is already EMPTY and the producer
//!   pays no reset burst on reuse (that ~1 store/item lands on the consumer's
//!   already-hot cache line instead of the producer's hot path). Credit-before-
//!   claim + `SLACK` size `N` so the physical chunk a producer reuses is already
//!   fully drained by the time its window opens (the `consumer_retired` spin is a
//!   safety net, not a hot wait). `chunk_cap` is sized to `cap`
//!   (`clamp(next_pow2(cap), floor, 1024)`, floor flavor-tuned - sync 128 /
//!   async 16); memory is a fixed ~`N × chunk_cap × size_of::<T>()`, i.e. a
//!   small multiple of `cap`.
//! - **Run-claim batching** - `claim_run` + `resolve_run` (send) and `deq_run`
//!   (recv): contiguous ticket runs, ascending slot writes, ONE `notify_receiver`
//!   per run, intra-run progress publishes.
//! - **Split dequeue counter** - `drained` mirrors `h.pos` at head-lock exit
//!   (single writer, one owned-line store per deq call). It is NOT the send
//!   window: `window_open`/`credit_ok` stay on the K-cadenced `progress`, so
//!   the hoard/drip release policy is untouched. Only `len`/`is_full` and the
//!   try_send cold path (`try_send_now_cold`/`claim_run_cold`) read it, making
//!   a reported `Full` exact instead of up-to-K stale.
//!
//! Everything here is performance-load-bearing and tuned against the bench suite:
//! credit-before-claim (`window_open`/`credit_ok`/`try_send_now`), the SKIP
//! tombstone + retry, wake metering (sync batch-by-freed / async exactly-one
//! drip), the K release cadence, SeqCst Dekker pairings, flush-before-wait, the
//! cap-1 pre-park spin, `finish_sync_*` stack guards, and `CachePadded`
//! placement. Never ever "simplify" any of it without re-benchmarking.

// Payload cells stay on std::cell::UnsafeCell (see internal/sync.rs - loom's
// closure-API cell is out of scope; miri covers payload races).
use std::cell::UnsafeCell;
use std::collections::VecDeque;
use std::task::Waker;

use crate::internal::sync::{
  fence, hint, Arc, AtomicBool, AtomicU8, AtomicUsize, Mutex, Ordering, Thread,
};

use crate::internal::cache_padded::CachePadded;

pub(crate) const EMPTY: u8 = 0;
pub(crate) const SET: u8 = 1;
/// Overshoot tombstone - the consumer skips it, still counts it as drained.
pub(crate) const SKIP: u8 = 2;

/// Whether to shrink the chunk table for model checkers. Under miri a small
/// table makes chunk RECYCLING/REUSE - the fresh unsafe surface (`ensure_resident`
/// CAS, reset-on-drain ordering) - reachable within a modest item budget instead
/// of only after hundreds of sends. Under loom it keeps each tiny model from
/// allocating hundreds of loom-tracked atomics it never touches. Correctness is
/// `chunk_cap`/`n`-agnostic, so shrinking only broadens coverage / cuts overhead.
const MODEL_CHECK: bool = cfg!(miri) || crate::internal::sync::IS_LOOM;

/// Extra live-ticket headroom (beyond `cap`) the chunk table must cover, to
/// absorb concurrent-claim overshoot. One SKIP per racing producer per attempt.
const SLACK: usize = if MODEL_CHECK { 8 } else { 64 };

pub(crate) const CACHE_FLUSH_CHUNK: usize = 64;

/// Chunk-size floors, flavor-tuned (see `Shared::new`). Sync favors bigger chunks (fewer
/// reuse-CAS handshakes for contending producers); async favors smaller chunks
/// (it runs ~2x faster and is L1-residency-bound). Collapsed to a small
/// power-of-two under miri/loom (see `MODEL_CHECK`).
pub(crate) const CHUNK_FLOOR_SYNC: usize = if MODEL_CHECK { 4 } else { 128 };
pub(crate) const CHUNK_FLOOR_ASYNC: usize = if MODEL_CHECK { 4 } else { 16 };

// Under loom the spin budget collapses to 1 (every tracked op counts against
// loom's branch cap, and parking is the path we want modeled).
pub(crate) const SYNC_SPIN_LIMIT: usize = if crate::internal::sync::IS_LOOM { 1 } else { 200 };

/// Cap-1-only bounded spin on the wake flag before a sync park.
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

// --- chunked array (generic T); recycled via the chunk table ---

struct Slot<T> {
  state: AtomicU8,
  data: UnsafeCell<Option<T>>,
}

struct Chunk<T> {
  slots: Box<[Slot<T>]>,
}

impl<T> Chunk<T> {
  fn alloc(chunk_cap: usize) -> *mut Chunk<T> {
    let slots = (0..chunk_cap)
      .map(|_| Slot {
        state: AtomicU8::new(EMPTY),
        data: UnsafeCell::new(None),
      })
      .collect();
    Box::into_raw(Box::new(Chunk { slots }))
  }
}

/// One fixed slot of the chunk table. `chunk` is allocated once and reused; `id`
/// is the logical chunk id currently resident (monotonically increasing by `n`).
struct ChunkEntry<T> {
  id: AtomicUsize,
  chunk: *mut Chunk<T>,
}

// --- waiters: TICKETLESS (interchangeable, fibre-style) ---

pub(crate) struct SendWaiters<W> {
  queue: VecDeque<(u64, W, *const AtomicBool)>,
  next_id: u64,
}

impl<W> SendWaiters<W> {
  fn new() -> Self {
    SendWaiters {
      queue: VecDeque::new(),
      next_id: 0,
    }
  }
}

/// Consumer-owned drain cursor. `publish_chunk` (the flush cadence K) lives here
/// so runtime `to_async`/`to_sync` conversions can retune it.
struct Head {
  cid: usize,
  idx: usize,
  pos: usize,
  unpublished: usize,
  publish_chunk: usize,
}

pub(crate) enum Deq<T> {
  Got(T),
  InFlight,
  Empty,
}

pub struct Shared<T> {
  g_tail: CachePadded<AtomicUsize>,
  /// Consumer-published drain count (SETs + SKIPs). The window is open for a
  /// new claim iff `g_tail - progress < cap` (approximate pre-check; the claim
  /// itself re-verifies against a fresh `progress` and SKIPs on overshoot).
  progress: CachePadded<AtomicUsize>,
  /// Split dequeue counter: `h.pos` mirrored at every head-lock exit (single
  /// writer - the consumer). Unlike `progress` it is NOT the send-window gate,
  /// so it carries no release-cadence policy and can be fresh per drain: the
  /// hot send path never reads it, only `len`/`is_full` and the try_send cold
  /// path (verifying Full against reality instead of the K-stale window).
  drained: CachePadded<AtomicUsize>,
  /// Chunks fully drained by the consumer: `retired >= id + 1` means chunk `id`
  /// is safe to reuse. Producers spin on this before resetting a reused chunk.
  consumer_retired: CachePadded<AtomicUsize>,
  cap: usize,
  chunk_cap: usize,
  log2: u32,
  mask: usize,
  n: usize,
  table: Box<[ChunkEntry<T>]>,
  /// Producer-visible mirror of the consumer's `publish_chunk` K, used as the
  /// batch run cap (`MAX_RUN`). Updated alongside `Head::publish_chunk`.
  run_cap: CachePadded<AtomicUsize>,

  head: CachePadded<Mutex<Head>>,

  sender_count: CachePadded<AtomicUsize>,
  receiver_dropped: CachePadded<AtomicBool>,

  sync_send_waiters: Mutex<SendWaiters<Thread>>,
  async_send_waiters: Mutex<SendWaiters<Waker>>,
  sync_send_waiter_count: CachePadded<AtomicUsize>,
  async_send_waiter_count: CachePadded<AtomicUsize>,
  sync_recv_waiter: Mutex<Option<(Thread, *const AtomicBool)>>,
  async_recv_waiter: Mutex<Option<Waker>>,
  sync_recv_waiter_count: CachePadded<AtomicUsize>,
  async_recv_waiter_count: CachePadded<AtomicUsize>,
}

// SAFETY: per-slot Release/Acquire handshake for payloads, a
// Mutex-guarded single-consumer head, id-epoch chunk reuse gated by
// `consumer_retired`, and fibre's finish_* protocol for the raw `notified` ptrs.
unsafe impl<T: Send> Send for Shared<T> {}
unsafe impl<T: Send> Sync for Shared<T> {}

impl<T> Shared<T> {
  pub(crate) fn new(cap: usize, publish_chunk: usize, chunk_floor: usize) -> Arc<Self> {
    let cap = cap.max(1);
    // `chunk_floor` is flavor-tuned (CHUNK_FLOOR_SYNC / CHUNK_FLOOR_ASYNC): sync
    // wants big chunks (less reuse-CAS churn under contention), async wants small
    // (L1 residency). A runtime `to_async`/`to_sync` conversion keeps whatever
    // floor the channel was BUILT with (the table can't be re-laid-out live).
    let chunk_cap = cap.next_power_of_two().clamp(chunk_floor, 1024);
    let log2 = chunk_cap.trailing_zeros();
    let mask = chunk_cap - 1;
    // Enough chunk slots to hold `cap + SLACK` live tickets plus two spare, so
    // the physical chunk a producer reuses is always already retired.
    let n = (cap + SLACK).div_ceil(chunk_cap) + 2;
    let table: Box<[ChunkEntry<T>]> = (0..n)
      .map(|i| ChunkEntry {
        id: AtomicUsize::new(i),
        chunk: Chunk::<T>::alloc(chunk_cap),
      })
      .collect();
    let publish_chunk = publish_chunk.clamp(1, cap);
    Arc::new(Shared {
      g_tail: CachePadded::new(AtomicUsize::new(0)),
      progress: CachePadded::new(AtomicUsize::new(0)),
      drained: CachePadded::new(AtomicUsize::new(0)),
      consumer_retired: CachePadded::new(AtomicUsize::new(0)),
      cap,
      chunk_cap,
      log2,
      mask,
      n,
      table,
      run_cap: CachePadded::new(AtomicUsize::new(publish_chunk)),
      head: CachePadded::new(Mutex::new(Head {
        cid: 0,
        idx: 0,
        pos: 0,
        unpublished: 0,
        publish_chunk,
      })),
      sender_count: CachePadded::new(AtomicUsize::new(1)),
      receiver_dropped: CachePadded::new(AtomicBool::new(false)),
      sync_send_waiters: Mutex::new(SendWaiters::new()),
      async_send_waiters: Mutex::new(SendWaiters::new()),
      sync_send_waiter_count: CachePadded::new(AtomicUsize::new(0)),
      async_send_waiter_count: CachePadded::new(AtomicUsize::new(0)),
      sync_recv_waiter: Mutex::new(None),
      async_recv_waiter: Mutex::new(None),
      sync_recv_waiter_count: CachePadded::new(AtomicUsize::new(0)),
      async_recv_waiter_count: CachePadded::new(AtomicUsize::new(0)),
    })
  }

  #[inline]
  pub(crate) fn cap(&self) -> usize {
    self.cap
  }

  /// `to_async` sets K = full cap (hoard-then-dump, arms the drip); `to_sync`
  /// sets K = min(cap, 64). Updates both the consumer cadence and
  /// the producer-visible run cap.
  pub(crate) fn set_publish_chunk(&self, v: usize) {
    let v = v.clamp(1, self.cap);
    self.head.lock().publish_chunk = v;
    self.run_cap.store(v, Ordering::Relaxed);
  }

  #[inline]
  fn credit_ok(&self, ticket: usize) -> bool {
    ticket.wrapping_sub(self.progress.load(Ordering::Acquire)) < self.cap
  }

  /// Approximate pre-claim gate: racy by design (the claim re-verifies).
  #[inline]
  pub(crate) fn window_open(&self) -> bool {
    self
      .g_tail
      .load(Ordering::Relaxed)
      .wrapping_sub(self.progress.load(Ordering::Acquire))
      < self.cap
  }

  // --- cold-path mirrors gated on `drained` instead of `progress` ---
  //
  // Same claim protocol (fetch_add ticket, re-verify, SKIP on overshoot), just
  // against the fresh split counter, so a `Full` from here means actually-full
  // at this instant rather than up-to-K stale. The `<= cap` occupancy bound and
  // the SLACK table sizing hold for ANY counter <= true drains, and slot-reuse
  // safety is carried by `ensure_resident`'s `consumer_retired` gate, not by
  // the credit counter. `progress` (the hot window and its release cadence) is
  // never read or written here.

  #[inline]
  fn credit_ok_cold(&self, ticket: usize) -> bool {
    ticket.wrapping_sub(self.drained.load(Ordering::Acquire)) < self.cap
  }

  #[inline]
  fn window_open_cold(&self) -> bool {
    self
      .g_tail
      .load(Ordering::Relaxed)
      .wrapping_sub(self.drained.load(Ordering::Acquire))
      < self.cap
  }

  /// Cold-path retry for `try_send` after `try_send_now` reported the window
  /// closed. See the section comment above.
  pub(crate) fn try_send_now_cold(&self, v: T) -> Result<(), T> {
    while self.window_open_cold() {
      let ticket = self.g_tail.fetch_add(1, Ordering::Relaxed);
      if self.credit_ok_cold(ticket) {
        self.write_slot(ticket, Some(v));
        return Ok(());
      }
      self.write_slot(ticket, None);
    }
    Err(v)
  }

  /// Cold-path mirror of `claim_run` for the try-batch paths. See the section
  /// comment above.
  pub(crate) fn claim_run_cold(&self, remaining: usize) -> (usize, usize, usize) {
    let run_cap = self.run_cap.load(Ordering::Relaxed).max(1);
    let g = self.g_tail.load(Ordering::Relaxed);
    let d = self.drained.load(Ordering::Acquire);
    let slack = self.cap.saturating_sub(g.wrapping_sub(d));
    let m = remaining.min(slack).min(run_cap);
    if m == 0 {
      return (0, 0, 0);
    }
    let t = self.g_tail.fetch_add(m, Ordering::Relaxed);
    let d2 = self.drained.load(Ordering::Acquire);
    let valid = m.min((d2 + self.cap).saturating_sub(t));
    (t, valid, m)
  }

  // --- close surface (fibre) ---

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
    self.wake_all_senders();
  }

  #[inline]
  pub(crate) fn senders_alive(&self) -> bool {
    self.sender_count.load(Ordering::Acquire) != 0
  }

  #[inline]
  pub(crate) fn receivers_alive(&self) -> bool {
    !self.receiver_dropped.load(Ordering::Acquire)
  }

  // --- len/is_empty/is_full: split-counter snapshot, clamped to [0, cap] ---

  pub(crate) fn capacity(&self) -> usize {
    self.cap
  }

  pub(crate) fn len(&self) -> usize {
    let tail = self.g_tail.load(Ordering::Acquire);
    let drained = self.drained.load(Ordering::Acquire);
    tail.wrapping_sub(drained).min(self.cap)
  }

  pub(crate) fn is_empty(&self) -> bool {
    self.len() == 0
  }

  pub(crate) fn is_full(&self) -> bool {
    self.len() >= self.cap
  }

  // --- ticketless send-waiter registry (fibre protocol) ---

  pub(crate) fn register_sync_send(&self, thread: Thread, notified: *const AtomicBool) -> u64 {
    let mut g = self.sync_send_waiters.lock();
    let id = g.next_id;
    g.next_id = g.next_id.wrapping_add(1);
    g.queue.push_back((id, thread, notified));
    self
      .sync_send_waiter_count
      .store(g.queue.len(), Ordering::Release);
    id
  }

  fn unregister_sync_send(&self, id: u64) -> bool {
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

  /// Leave the send-waiter set on the way out. If a notifier already dequeued us
  /// (`unregister` returns false) it holds our raw `notified` pointer and will
  /// `store(true)` into it; we MUST NOT let this frame (and `notified`) die until
  /// that write lands. Callers must NOT reuse a `notified` across a `finish` -
  /// give each register/park lifetime its own (see producer `wait_for_window`).
  pub(crate) fn finish_sync_send(&self, id: u64, notified: &AtomicBool) {
    if !self.unregister_sync_send(id) {
      while !notified.load(Ordering::Acquire) {
        hint::spin_loop();
      }
    }
  }

  pub(crate) fn register_async_send(&self, prev_id: Option<u64>, waker: Waker) -> u64 {
    let mut g = self.async_send_waiters.lock();
    if let Some(id) = prev_id {
      g.queue.retain(|(sid, _, _)| *sid != id);
    }
    let id = g.next_id;
    g.next_id = g.next_id.wrapping_add(1);
    g.queue.push_back((id, waker, std::ptr::null()));
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

  /// fibre's wake policies verbatim: sync = batch wake sized by freed credits;
  /// async = the H2 metered drip (exactly one). Caller has published `progress`.
  fn notify_senders(&self, freed: usize) {
    fence(Ordering::SeqCst);
    if self.sync_send_waiter_count.load(Ordering::Relaxed) != 0 {
      let mut g = self.sync_send_waiters.lock();
      let mut to_wake = Vec::with_capacity(g.queue.len().min(freed));
      while to_wake.len() < freed {
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
      for t in to_wake {
        t.unpark();
      }
    }
    if self.async_send_waiter_count.load(Ordering::Relaxed) != 0 {
      let mut g = self.async_send_waiters.lock();
      if let Some((_id, waker, _)) = g.queue.pop_front() {
        self
          .async_send_waiter_count
          .store(g.queue.len(), Ordering::Release);
        drop(g);
        waker.wake();
      }
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
      while let Some((_id, waker, _)) = g.queue.pop_front() {
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

  // --- recv-waiter slots ---

  pub(crate) fn register_sync_recv(&self, thread: Thread, notified: *const AtomicBool) {
    *self.sync_recv_waiter.lock() = Some((thread, notified));
    self.sync_recv_waiter_count.store(1, Ordering::Release);
  }

  fn unregister_sync_recv(&self) -> bool {
    let had = self.sync_recv_waiter.lock().take().is_some();
    self.sync_recv_waiter_count.store(0, Ordering::Release);
    had
  }

  pub(crate) fn finish_sync_recv(&self, notified: &AtomicBool) {
    if !self.unregister_sync_recv() {
      while !notified.load(Ordering::Acquire) {
        hint::spin_loop();
      }
    }
  }

  pub(crate) fn register_async_recv(&self, waker: Waker) {
    *self.async_recv_waiter.lock() = Some(waker);
    self.async_recv_waiter_count.store(1, Ordering::Release);
  }

  pub(crate) fn unregister_async_recv(&self) {
    *self.async_recv_waiter.lock() = None;
    self.async_recv_waiter_count.store(0, Ordering::Release);
  }

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
      if let Some(waker) = g.take() {
        self.async_recv_waiter_count.store(0, Ordering::Release);
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
    if let Some(waker) = self.async_recv_waiter.lock().take() {
      self.async_recv_waiter_count.store(0, Ordering::Release);
      waker.wake();
    }
  }

  // --- chunk table ---

  /// Resolve the chunk resident for logical id `cid`, performing the id-epoch
  /// reuse handshake if the physical slot still holds an older chunk. There is
  /// NO reset step here: the consumer resets each slot to EMPTY as it drains
  /// (reset-on-drain), so a reused chunk is already EMPTY. We only wait for the
  /// old occupant to be fully retired, then CAS `id` from `cur` to `cid` (winner
  /// returns; losers observe `id == cid` and return). The `consumer_retired`
  /// Release→Acquire gate orders the consumer's EMPTY stores before our reuse,
  /// and the winning CAS's Release propagates that to the other producers of
  /// this chunk. Credit-before-claim + `SLACK` make the gate a near-noop.
  fn ensure_resident(&self, cid: usize) -> *mut Chunk<T> {
    let entry = &self.table[cid % self.n];
    loop {
      let cur = entry.id.load(Ordering::Acquire);
      if cur == cid {
        return entry.chunk;
      }
      debug_assert!(cur < cid, "chunk table slot advanced past a live ticket");
      while self.consumer_retired.load(Ordering::Acquire) < cur + 1 {
        hint::spin_loop();
      }
      match entry
        .id
        .compare_exchange(cur, cid, Ordering::AcqRel, Ordering::Acquire)
      {
        Ok(_) => return entry.chunk,
        Err(_) => continue, // lost the race; re-load (someone installed cid)
      }
    }
  }

  fn write_slot(&self, ticket: usize, v: Option<T>) {
    let cid = ticket >> self.log2;
    let idx = ticket & self.mask;
    let chunk = self.ensure_resident(cid);
    unsafe {
      let slot = (&(*chunk).slots).get_unchecked(idx);
      match v {
        Some(val) => {
          *slot.data.get() = Some(val);
          slot.state.store(SET, Ordering::Release);
        }
        None => {
          slot.state.store(SKIP, Ordering::Release);
        }
      }
    }
    self.notify_receiver();
  }

  /// The credit-before-claim attempt: `Ok(())` = written; `Err(v)` = window
  /// closed, caller keeps the value to park/retry. On a check-then-claim race
  /// the overshoot slot is SKIP-tombstoned and the attempt retried (bounded).
  pub(crate) fn try_send_now(&self, v: T) -> Result<(), T> {
    while self.window_open() {
      let ticket = self.g_tail.fetch_add(1, Ordering::Relaxed);
      if self.credit_ok(ticket) {
        self.write_slot(ticket, Some(v));
        return Ok(());
      }
      self.write_slot(ticket, None);
    }
    Err(v)
  }

  // --- batch send: run-claim ---

  /// Claim a contiguous ticket run of up to `min(remaining, slack, run_cap)`.
  /// Returns `(t, valid, m)`: tickets `t..t+m` are CLAIMED, `t..t+valid` should
  /// be filled with values, `t+valid..t+m` must be SKIP-tombstoned (overshoot).
  /// `m == 0` means the window was closed (nothing claimed).
  pub(crate) fn claim_run(&self, remaining: usize) -> (usize, usize, usize) {
    let run_cap = self.run_cap.load(Ordering::Relaxed).max(1);
    let g = self.g_tail.load(Ordering::Relaxed);
    let p = self.progress.load(Ordering::Acquire);
    let slack = self.cap.saturating_sub(g.wrapping_sub(p));
    let m = remaining.min(slack).min(run_cap);
    if m == 0 {
      return (0, 0, 0);
    }
    let t = self.g_tail.fetch_add(m, Ordering::Relaxed);
    let p2 = self.progress.load(Ordering::Acquire);
    let valid = m.min((p2 + self.cap).saturating_sub(t));
    (t, valid, m)
  }

  /// Resolve a claimed run: fill `t..t+valid` from `iter` (ascending SETs),
  /// SKIP-tombstone `t+valid..t+m`, then a SINGLE `notify_receiver`.
  /// `iter` must yield at least `valid` items.
  pub(crate) fn resolve_run(
    &self,
    t: usize,
    valid: usize,
    m: usize,
    iter: &mut impl Iterator<Item = T>,
  ) {
    let mut written = 0;
    while written < valid {
      let ticket = t + written;
      let idx = ticket & self.mask;
      let cid = ticket >> self.log2;
      let chunk = self.ensure_resident(cid);
      let seg = (self.chunk_cap - idx).min(valid - written);
      unsafe {
        for j in 0..seg {
          let slot = (&(*chunk).slots).get_unchecked(idx + j);
          *slot.data.get() = Some(iter.next().expect("resolve_run: iter shorter than valid"));
          slot.state.store(SET, Ordering::Release);
        }
      }
      written += seg;
    }
    let mut skipped = valid;
    while skipped < m {
      let ticket = t + skipped;
      let idx = ticket & self.mask;
      let cid = ticket >> self.log2;
      let chunk = self.ensure_resident(cid);
      let seg = (self.chunk_cap - idx).min(m - skipped);
      unsafe {
        for j in 0..seg {
          (&(*chunk).slots).get_unchecked(idx + j).state.store(SKIP, Ordering::Release);
        }
      }
      skipped += seg;
    }
    self.notify_receiver();
  }

  // --- consumer core (table-walking) ---

  pub(crate) fn deq_once(&self) -> Deq<T> {
    let mut h = self.head.lock();
    loop {
      let entry = &self.table[h.cid % self.n];
      if entry.id.load(Ordering::Acquire) != h.cid {
        self.drained.store(h.pos, Ordering::Release);
        return if h.pos < self.g_tail.load(Ordering::Acquire) {
          Deq::InFlight
        } else {
          Deq::Empty
        };
      }
      let chunk = entry.chunk;
      if h.idx == self.chunk_cap {
        // Finished this chunk: retire it (enables producer reuse) and advance.
        self.consumer_retired.store(h.cid + 1, Ordering::Release);
        h.cid += 1;
        h.idx = 0;
        continue;
      }
      let slot = unsafe { (&(*chunk).slots).get_unchecked(h.idx) };
      match slot.state.load(Ordering::Acquire) {
        SET => {
          let v = unsafe { (*slot.data.get()).take().unwrap() };
          // reset-on-drain: leave the slot EMPTY so reuse of this chunk needs no
          // producer-side reset burst (see ensure_resident). Relaxed is safe -
          // the `consumer_retired` Release orders it before any producer reuse.
          slot.state.store(EMPTY, Ordering::Relaxed);
          h.idx += 1;
          h.pos += 1;
          h.unpublished += 1;
          if h.unpublished >= h.publish_chunk {
            self.publish_progress(&mut h);
          }
          self.drained.store(h.pos, Ordering::Release);
          return Deq::Got(v);
        }
        SKIP => {
          slot.state.store(EMPTY, Ordering::Relaxed);
          h.idx += 1;
          h.pos += 1;
          h.unpublished += 1;
          if h.unpublished >= h.publish_chunk {
            self.publish_progress(&mut h);
          }
          continue;
        }
        _ => {
          self.drained.store(h.pos, Ordering::Release);
          return if h.pos < self.g_tail.load(Ordering::Acquire) {
            Deq::InFlight
          } else {
            Deq::Empty
          };
        }
      }
    }
  }

  /// Drain up to `max` items in one head-lock hold. Publishes progress
  /// every K crossed inside the run so large batches never sit on credits.
  /// Returns the number of items pushed to `out`.
  pub(crate) fn deq_run(&self, out: &mut Vec<T>, max: usize) -> usize {
    if max == 0 {
      return 0;
    }
    let mut h = self.head.lock();
    let mut got = 0;
    while got < max {
      let entry = &self.table[h.cid % self.n];
      if entry.id.load(Ordering::Acquire) != h.cid {
        break;
      }
      let chunk = entry.chunk;
      if h.idx == self.chunk_cap {
        self.consumer_retired.store(h.cid + 1, Ordering::Release);
        h.cid += 1;
        h.idx = 0;
        continue;
      }
      let slot = unsafe { (&(*chunk).slots).get_unchecked(h.idx) };
      match slot.state.load(Ordering::Acquire) {
        SET => {
          let v = unsafe { (*slot.data.get()).take().unwrap() };
          slot.state.store(EMPTY, Ordering::Relaxed); // reset-on-drain (see deq_once)
          out.push(v);
          got += 1;
          h.idx += 1;
          h.pos += 1;
          h.unpublished += 1;
          if h.unpublished >= h.publish_chunk {
            self.publish_progress(&mut h);
          }
        }
        SKIP => {
          slot.state.store(EMPTY, Ordering::Relaxed); // reset-on-drain
          h.idx += 1;
          h.pos += 1;
          h.unpublished += 1;
          if h.unpublished >= h.publish_chunk {
            self.publish_progress(&mut h);
          }
        }
        _ => break,
      }
    }
    self.drained.store(h.pos, Ordering::Release);
    got
  }

  /// Cold-path helper closing the check-empty-then-check-disconnected race: a
  /// `deq_once` that ran BEFORE the caller observed `sender_count == 0` may have
  /// missed the last producer's final `SET` (ordered-before that observation).
  /// Exactly one more drain is required (and sufficient) before declaring the
  /// stream terminally drained. `Some(v)` = a straggler was found.
  pub(crate) fn drain_straggler(&self) -> Option<T> {
    match self.deq_once() {
      Deq::Got(v) => Some(v),
      _ => None,
    }
  }

  fn publish_progress(&self, h: &mut Head) {
    let freed = h.unpublished;
    h.unpublished = 0;
    // `drained` first so no observer ever sees `drained < progress`.
    self.drained.store(h.pos, Ordering::Release);
    self.progress.store(h.pos, Ordering::Release);
    self.notify_senders(freed);
  }

  pub(crate) fn flush_progress(&self) {
    let mut h = self.head.lock();
    if h.unpublished > 0 {
      self.publish_progress(&mut h);
    }
  }
}

impl<T> Drop for Shared<T> {
  fn drop(&mut self) {
    // Free every table chunk. A chunk's slot array is dropped by field glue: any
    // SET-but-undrained slot still holds `Some(T)` (drained slots are `None`
    // after `take()`), so undrained values are dropped exactly once. Reused
    // physical slots hold at most one `Some` at a time, so no double-drop. No
    // producer can be mid-write: Shared drops only after all handles are gone.
    for entry in self.table.iter() {
      unsafe {
        drop(Box::from_raw(entry.chunk));
      }
    }
  }
}
