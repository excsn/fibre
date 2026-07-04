//! Shared core of the unbounded MPMC channel.
//!
//! Producers publish lock-free onto the slab-backed Vyukov chain
//! (`internal::slab_chain`): one always-succeeding `swap` per send, nodes from
//! per-handle slabs, sends never block, no send waiters exist. The consumer
//! side is serialized by one small `parking_lot::Mutex` — the mechanism the
//! v2-vs-v3 contention benchmarks crowned — guarding only the chain cursor,
//! the recv-waiter queue, and the reclaimed-item buffer.
//!
//! **Eager handoff:** whoever holds the consumer mutex while waiters exist
//! delivers chain items directly into waiters' cells (in chain order, to
//! waiters in FIFO order) instead of the classic wake-then-repoll dance. This
//! is the load-bearing optimization from mpmc_v2's bounded core, applied on
//! the consumer side only; `EAGER_HANDOFF` is the compile-time kill-switch for
//! A/B measurement, following the proven `EAGER_HANDOFF_ENABLED` precedent.
//!
//! Waiter entries hold `Arc<WaiterCell>` rather than raw pointers into parked
//! stacks or pinned futures, so the delivery path cannot use-after-free a
//! cancelled waiter by construction.

use crate::error::{RecvError, TryRecvError};
use crate::internal::cache_padded::CachePadded;
use crate::internal::slab_chain::{alloc_stub, retire_node, ChainHead, Node, SlabPool};

use std::cell::UnsafeCell;
use std::collections::VecDeque;
use std::fmt;
use std::task::Waker;
use std::time::Instant;

use crate::internal::sync::{
  fence, thread, Arc, AtomicBool, AtomicU8, AtomicUsize, Mutex, Ordering, Thread,
};

/// Compile-time kill-switch for eager handoff (A/B: flip and re-bench).
pub(crate) const EAGER_HANDOFF: bool = false;

const LEN_SHARDS: usize = 16;

// --- Waiter cells -------------------------------------------------------------

pub(crate) const WAITER_WAITING: u8 = 0;
/// An item was deposited into the cell's slot (handoff delivery).
pub(crate) const WAITER_FULFILLED: u8 = 1;
/// Classic wake-one consumed the registration without delivering an item
/// (kill-switch-off path and disconnect wakes); the waiter must re-attempt.
pub(crate) const WAITER_NOTIFIED: u8 = 2;

/// A waiter's landing pad. `state` transitions are only ever performed while
/// holding the consumer mutex; the owner reads `slot` only after observing
/// `WAITER_FULFILLED` with `Acquire`, after which no deliverer touches the
/// cell again.
pub(crate) struct WaiterCell<T> {
  pub(crate) state: AtomicU8,
  slot: UnsafeCell<Option<T>>,
}

unsafe impl<T: Send> Send for WaiterCell<T> {}
unsafe impl<T: Send> Sync for WaiterCell<T> {}

impl<T: Send> WaiterCell<T> {
  pub(crate) fn new() -> Arc<Self> {
    Arc::new(WaiterCell {
      state: AtomicU8::new(WAITER_WAITING),
      slot: UnsafeCell::new(None),
    })
  }

  /// Rearm a cached cell for re-registration. Owner-only, while unregistered.
  pub(crate) fn rearm(&self) {
    self.state.store(WAITER_WAITING, Ordering::Relaxed);
  }

  /// Takes the delivered item. Owner-only, after observing FULFILLED.
  pub(crate) fn take(&self) -> Option<T> {
    unsafe { (*self.slot.get()).take() }
  }

  /// Deliverer-only, under the consumer mutex, on a WAITING cell.
  fn fulfill(&self, item: T) {
    unsafe { *self.slot.get() = Some(item) };
    self.state.store(WAITER_FULFILLED, Ordering::Release);
  }
}

enum WakeHandle {
  Thread(Thread),
  Task(Waker),
}

impl WakeHandle {
  fn wake(self) {
    match self {
      WakeHandle::Thread(t) => t.unpark(),
      WakeHandle::Task(w) => w.wake(),
    }
  }
}

struct WaiterEntry<T> {
  id: u64,
  wake: WakeHandle,
  cell: Arc<WaiterCell<T>>,
}

/// Deferred wakeups, fired after the consumer mutex is released.
pub(crate) struct WakeList(Vec<WakeHandle>);

impl WakeList {
  pub(crate) fn new() -> Self {
    WakeList(Vec::new())
  }
  pub(crate) fn fire(self) {
    for w in self.0 {
      w.wake();
    }
  }
}

// --- Consumer-side state (everything behind the one mutex) ---------------------

pub(crate) struct ConsumerState<T> {
  tail: *mut Node<T>,
  waiters: VecDeque<WaiterEntry<T>>,
  /// Items recovered from cancelled-but-already-fulfilled waiters. Drained
  /// before the chain so a recovered item re-enters as close to its original
  /// position as still possible.
  reclaimed: VecDeque<T>,
  next_id: u64,
}

unsafe impl<T: Send> Send for ConsumerState<T> {}

// --- Shared state ----------------------------------------------------------------

pub(crate) struct UnboundedShared<T> {
  /// Producers swap here — the single shared RMW per send; never locked.
  head: ChainHead<T>,
  /// Recycles retired slabs; its own `Arc` so sender handles (and the slabs
  /// themselves) can outlive-independently reference it.
  slab_pool: Arc<SlabPool<T>>,
  consumer: Mutex<ConsumerState<T>>,
  /// Producer-visible gate: producers only enter the consumer mutex when this
  /// is non-zero (paired SeqCst fences with register-then-recheck).
  recv_waiter_count: CachePadded<AtomicUsize>,

  sender_count: AtomicUsize,
  receiver_count: AtomicUsize,
  receiver_dropped: AtomicBool,

  sent_shards: [CachePadded<AtomicUsize>; LEN_SHARDS],
  shard_cursor: AtomicUsize,
  consumed: CachePadded<AtomicUsize>,
}

unsafe impl<T: Send> Send for UnboundedShared<T> {}
unsafe impl<T: Send> Sync for UnboundedShared<T> {}

impl<T> fmt::Debug for UnboundedShared<T> {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("UnboundedShared")
      .field("sender_count", &self.sender_count.load(Ordering::Relaxed))
      .field("receiver_count", &self.receiver_count.load(Ordering::Relaxed))
      .finish_non_exhaustive()
  }
}

impl<T: Send> UnboundedShared<T> {
  pub(crate) fn new() -> Self {
    let stub = alloc_stub::<T>();
    UnboundedShared {
      head: ChainHead::new(stub),
      slab_pool: Arc::new(SlabPool::new()),
      consumer: Mutex::new(ConsumerState {
        tail: stub,
        waiters: VecDeque::new(),
        reclaimed: VecDeque::new(),
        next_id: 0,
      }),
      recv_waiter_count: CachePadded::new(AtomicUsize::new(0)),
      sender_count: AtomicUsize::new(1),
      receiver_count: AtomicUsize::new(1),
      receiver_dropped: AtomicBool::new(false),
      sent_shards: std::array::from_fn(|_| CachePadded::new(AtomicUsize::new(0))),
      shard_cursor: AtomicUsize::new(0),
      consumed: CachePadded::new(AtomicUsize::new(0)),
    }
  }

  // --- Producer side ----------------------------------------------------------

  pub(crate) fn next_shard(&self) -> usize {
    self.shard_cursor.fetch_add(1, Ordering::Relaxed) % LEN_SHARDS
  }

  pub(crate) fn slab_pool(&self) -> Arc<SlabPool<T>> {
    Arc::clone(&self.slab_pool)
  }

  #[inline]
  pub(crate) fn record_sent(&self, shard: usize, n: usize) {
    self.sent_shards[shard].fetch_add(n, Ordering::Relaxed);
  }

  #[inline]
  pub(crate) fn publish(&self, first: *mut Node<T>, last: *mut Node<T>) {
    self.head.publish(first, last);
  }

  /// Post-publish notify. The common case (no waiters) touches nothing shared
  /// beyond the gate load; with waiters, the producer enters the consumer
  /// mutex and runs a handoff session (or classic wake-one under the
  /// kill-switch) — which also removes the wake-then-lose-race respawning.
  pub(crate) fn notify_receivers(&self) {
    fence(Ordering::SeqCst);
    if self.recv_waiter_count.load(Ordering::Relaxed) == 0 {
      return;
    }
    let mut wakes = WakeList::new();
    {
      let mut c = self.consumer.lock();
      if EAGER_HANDOFF {
        self.handoff_session(&mut c, &mut wakes);
      } else if let Some(e) = c.waiters.pop_front() {
        e.cell.state.store(WAITER_NOTIFIED, Ordering::Release);
        wakes.0.push(e.wake);
      }
      self.store_waiter_count(&c);
    }
    wakes.fire();
  }

  // --- Handle bookkeeping ------------------------------------------------------

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
      // Disconnect: wake every waiter so it can observe Disconnected.
      let mut wakes = WakeList::new();
      {
        let mut c = self.consumer.lock();
        while let Some(e) = c.waiters.pop_front() {
          e.cell.state.store(WAITER_NOTIFIED, Ordering::Release);
          wakes.0.push(e.wake);
        }
        self.store_waiter_count(&c);
      }
      wakes.fire();
    }
  }

  pub(crate) fn add_receiver(&self) {
    self.receiver_count.fetch_add(1, Ordering::Relaxed);
  }

  pub(crate) fn drop_receiver(&self) {
    if self.receiver_count.fetch_sub(1, Ordering::AcqRel) == 1 {
      self.receiver_dropped.store(true, Ordering::Release);
      // Sends never block, so there are no senders to wake.
    }
  }

  pub(crate) fn len(&self) -> usize {
    let sent: usize = self
      .sent_shards
      .iter()
      .map(|s| s.load(Ordering::Relaxed))
      .sum();
    sent.saturating_sub(self.consumed.load(Ordering::Relaxed))
  }

  // --- Consumer core (all under the consumer mutex) -----------------------------

  fn store_waiter_count(&self, c: &ConsumerState<T>) {
    self
      .recv_waiter_count
      .store(c.waiters.len(), Ordering::Release);
  }

  /// Pops one item — reclaimed items first, then the chain.
  fn pop_locked(&self, c: &mut ConsumerState<T>) -> Option<T> {
    if let Some(v) = c.reclaimed.pop_front() {
      self.consumed.fetch_add(1, Ordering::Relaxed);
      return Some(v);
    }
    unsafe {
      let tail = c.tail;
      let next = (*tail).next.load(Ordering::Acquire);
      if next.is_null() {
        return None;
      }
      let value = (*(*next).val.get()).take().unwrap();
      c.tail = next;
      retire_node(tail);
      self.consumed.fetch_add(1, Ordering::Relaxed);
      Some(value)
    }
  }

  /// Delivers items (in FIFO order) directly into waiter cells (in FIFO
  /// order) while both exist. Wakes are deferred to after unlock.
  fn handoff_session(&self, c: &mut ConsumerState<T>, wakes: &mut WakeList) {
    while !c.waiters.is_empty() {
      let Some(item) = self.pop_locked(c) else { break };
      let e = c.waiters.pop_front().unwrap();
      e.cell.fulfill(item);
      wakes.0.push(e.wake);
    }
  }

  /// Runs a handoff session if enabled and profitable; callers hold the lock.
  fn maybe_handoff(&self, c: &mut ConsumerState<T>, wakes: &mut WakeList) {
    if EAGER_HANDOFF && !c.waiters.is_empty() {
      self.handoff_session(c, wakes);
      self.store_waiter_count(c);
    }
  }

  fn register_waiter(&self, c: &mut ConsumerState<T>, wake: WakeHandle, cell: Arc<WaiterCell<T>>) -> u64 {
    let id = c.next_id;
    c.next_id = c.next_id.wrapping_add(1);
    c.waiters.push_back(WaiterEntry { id, wake, cell });
    self.store_waiter_count(c);
    id
  }

  fn remove_waiter(&self, c: &mut ConsumerState<T>, id: u64) -> bool {
    let before = c.waiters.len();
    c.waiters.retain(|e| e.id != id);
    let removed = c.waiters.len() != before;
    if removed {
      self.store_waiter_count(c);
    }
    removed
  }

  fn update_waker(&self, c: &mut ConsumerState<T>, id: u64, waker: &Waker) -> bool {
    for e in c.waiters.iter_mut() {
      if e.id == id {
        e.wake = WakeHandle::Task(waker.clone());
        return true;
      }
    }
    false
  }

  /// Recovers an item from a cancelled-but-fulfilled waiter: closest-possible
  /// FIFO reinsertion, delivered onward immediately if someone is waiting so
  /// it can never strand while consumers sleep.
  fn reclaim(&self, c: &mut ConsumerState<T>, item: T, wakes: &mut WakeList) {
    c.reclaimed.push_front(item);
    // The reclaimed item was already counted consumed once; popping it again
    // would double-count. Compensate here, where it re-enters the queue.
    self.consumed.fetch_sub(1, Ordering::Relaxed);
    if EAGER_HANDOFF {
      self.handoff_session(c, wakes);
      self.store_waiter_count(c);
    } else if let Some(e) = c.waiters.pop_front() {
      e.cell.state.store(WAITER_NOTIFIED, Ordering::Release);
      wakes.0.push(e.wake);
      self.store_waiter_count(c);
    }
  }

  // --- Non-blocking API cores ----------------------------------------------------

  pub(crate) fn try_recv_internal(&self) -> Result<T, TryRecvError> {
    let mut wakes = WakeList::new();
    let res;
    {
      let mut c = self.consumer.lock();
      res = match self.pop_locked(&mut c) {
        Some(v) => Ok(v),
        None => {
          if !self.senders_alive() {
            // A sender may have published right before dropping.
            match self.pop_locked(&mut c) {
              Some(v) => Ok(v),
              None => Err(TryRecvError::Disconnected),
            }
          } else {
            Err(TryRecvError::Empty)
          }
        }
      };
      if res.is_ok() {
        self.maybe_handoff(&mut c, &mut wakes);
      }
    }
    wakes.fire();
    res
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
    let mut wakes = WakeList::new();
    let res;
    {
      let mut c = self.consumer.lock();
      let mut got = 0;
      while got < max {
        match self.pop_locked(&mut c) {
          Some(v) => {
            out.push(v);
            got += 1;
          }
          None => break,
        }
      }
      res = if got > 0 {
        Ok(got)
      } else if !self.senders_alive() {
        match self.pop_locked(&mut c) {
          Some(v) => {
            out.push(v);
            Ok(1)
          }
          None => Err(TryRecvError::Disconnected),
        }
      } else {
        Err(TryRecvError::Empty)
      };
      if res.is_ok() {
        self.maybe_handoff(&mut c, &mut wakes);
      }
    }
    wakes.fire();
    res
  }

  // --- Blocking receive -----------------------------------------------------------

  /// Blocking receive with optional deadline. `deadline: None` waits forever.
  ///
  /// `cell` is the caller's landing pad (the receiver handle caches one so the
  /// hot path never allocates). Its state may be stale from a previous wait;
  /// registration below rearms it before any deliverer can see it.
  pub(crate) fn recv_sync_internal(
    &self,
    cell: &Arc<WaiterCell<T>>,
    deadline: Option<Instant>,
  ) -> Result<T, RecvTimeoutOutcome> {
    let mut registered: Option<u64> = None;

    loop {
      // Locked attempt (register-then-recheck with the SC fence pairing the
      // producers' publish→fence→gate-load).
      let mut wakes = WakeList::new();
      let attempt: Option<Result<T, RecvTimeoutOutcome>>;
      {
        let mut c = self.consumer.lock();
        if let Some(v) = self.pop_locked(&mut c) {
          if let Some(id) = registered.take() {
            self.remove_waiter(&mut c, id);
          }
          self.maybe_handoff(&mut c, &mut wakes);
          attempt = Some(Ok(v));
        } else if !self.senders_alive() {
          if let Some(id) = registered.take() {
            self.remove_waiter(&mut c, id);
          }
          attempt = Some(match self.pop_locked(&mut c) {
            Some(v) => Ok(v),
            None => Err(RecvTimeoutOutcome::Disconnected),
          });
        } else {
          if registered.is_none() {
            cell.rearm();
            registered = Some(self.register_waiter(
              &mut c,
              WakeHandle::Thread(thread::current()),
              Arc::clone(cell),
            ));
          }
          fence(Ordering::SeqCst);
          if let Some(v) = self.pop_locked(&mut c) {
            let id = registered.take().unwrap();
            self.remove_waiter(&mut c, id);
            self.maybe_handoff(&mut c, &mut wakes);
            attempt = Some(Ok(v));
          } else {
            attempt = None;
          }
        }
      }
      wakes.fire();
      if let Some(res) = attempt {
        return res;
      }

      // Park until a terminal cell state or deadline.
      loop {
        match deadline {
          None => thread::park(),
          Some(d) => {
            let now = Instant::now();
            if now >= d {
              return self.timeout_finish(cell, registered.take());
            }
            thread::park_timeout(d - now);
          }
        }
        match cell.state.load(Ordering::Acquire) {
          WAITER_FULFILLED => {
            return Ok(cell.take().expect("fulfilled waiter cell must hold an item"));
          }
          WAITER_NOTIFIED => {
            // Registration consumed without delivery: retry from the top.
            registered = None;
            break;
          }
          _ => {
            // Spurious wake: still registered; check deadline and re-park.
            if let Some(d) = deadline {
              if Instant::now() >= d {
                return self.timeout_finish(cell, registered.take());
              }
            }
          }
        }
      }
    }
  }

  /// Resolves a timeout race: if the registration is gone, a deliverer beat
  /// us — honor its outcome (state writes happen under the lock we now hold).
  fn timeout_finish(
    &self,
    cell: &Arc<WaiterCell<T>>,
    registered: Option<u64>,
  ) -> Result<T, RecvTimeoutOutcome> {
    let Some(id) = registered else {
      return Err(RecvTimeoutOutcome::Timeout);
    };
    let mut c = self.consumer.lock();
    if self.remove_waiter(&mut c, id) {
      return Err(RecvTimeoutOutcome::Timeout);
    }
    drop(c);
    match cell.state.load(Ordering::Acquire) {
      WAITER_FULFILLED => Ok(cell.take().expect("fulfilled waiter cell must hold an item")),
      _ => Err(RecvTimeoutOutcome::Timeout),
    }
  }

  // --- Async receive ---------------------------------------------------------------

  /// One poll step for a single-item async receive. `ctx` carries the
  /// future's registration state across polls.
  pub(crate) fn poll_recv_internal(
    &self,
    waker: &Waker,
    ctx: &mut AsyncWaitCtx<T>,
  ) -> std::task::Poll<Result<T, RecvError>> {
    use std::task::Poll;

    // A previous handoff may have completed us before this poll.
    match ctx.take_terminal() {
      Some(TerminalState::Fulfilled(v)) => return Poll::Ready(Ok(v)),
      Some(TerminalState::Notified) | None => {}
    }

    let mut wakes = WakeList::new();
    let out;
    {
      let mut c = self.consumer.lock();
      if let Some(v) = self.pop_locked(&mut c) {
        if let Some(id) = ctx.registered.take() {
          self.remove_waiter(&mut c, id);
        }
        self.maybe_handoff(&mut c, &mut wakes);
        out = Poll::Ready(Ok(v));
      } else if !self.senders_alive() {
        if let Some(id) = ctx.registered.take() {
          self.remove_waiter(&mut c, id);
        }
        out = Poll::Ready(match self.pop_locked(&mut c) {
          Some(v) => Ok(v),
          None => Err(RecvError::Disconnected),
        });
      } else {
        match ctx.registered {
          Some(id) => {
            if !self.update_waker(&mut c, id, waker) {
              // Entry vanished: terminal state was set before the deliverer
              // released the lock; handle it on the next iteration.
              ctx.registered = None;
            }
          }
          None => {
            ctx.cell.rearm();
            ctx.registered = Some(self.register_waiter(
              &mut c,
              WakeHandle::Task(waker.clone()),
              Arc::clone(&ctx.cell),
            ));
          }
        }
        fence(Ordering::SeqCst);
        if let Some(v) = self.pop_locked(&mut c) {
          if let Some(id) = ctx.registered.take() {
            self.remove_waiter(&mut c, id);
          }
          self.maybe_handoff(&mut c, &mut wakes);
          out = Poll::Ready(Ok(v));
        } else if !self.senders_alive() {
          if let Some(id) = ctx.registered.take() {
            self.remove_waiter(&mut c, id);
          }
          out = Poll::Ready(match self.pop_locked(&mut c) {
            Some(v) => Ok(v),
            None => Err(RecvError::Disconnected),
          });
        } else if ctx.registered.is_none() {
          // Lost the entry to a terminal transition mid-poll; resolve it now.
          out = match ctx.take_terminal() {
            Some(TerminalState::Fulfilled(v)) => Poll::Ready(Ok(v)),
            _ => Poll::Pending, // Notified: we re-register on the next poll
          };
          if matches!(out, Poll::Pending) {
            // Ensure we get polled again promptly.
            waker.wake_by_ref();
          }
        } else {
          out = Poll::Pending;
        }
      }
    }
    wakes.fire();
    out
  }

  /// Cancel-safety: called by future/stream drops while registered.
  pub(crate) fn cancel_wait(&self, ctx: &mut AsyncWaitCtx<T>) {
    let Some(id) = ctx.registered.take() else {
      return;
    };
    let mut wakes = WakeList::new();
    {
      let mut c = self.consumer.lock();
      if !self.remove_waiter(&mut c, id) {
        // A deliverer won: terminal state is already set (it happened under
        // this lock). Recover a fulfilled item so it is never lost, and rearm
        // the cell: the ctx outlives this wait (handles cache it), and a
        // stale FULFILLED with an emptied slot would panic the next poll.
        if ctx.cell.state.load(Ordering::Acquire) == WAITER_FULFILLED {
          if let Some(item) = ctx.cell.take() {
            self.reclaim(&mut c, item, &mut wakes);
          }
          ctx.cell.rearm();
        }
      }
    }
    wakes.fire();
  }
}

/// Registration state a recv future/stream carries across polls.
pub(crate) struct AsyncWaitCtx<T> {
  pub(crate) cell: Arc<WaiterCell<T>>,
  pub(crate) registered: Option<u64>,
}

enum TerminalState<T> {
  Fulfilled(T),
  Notified,
}

impl<T: Send> AsyncWaitCtx<T> {
  pub(crate) fn new() -> Self {
    AsyncWaitCtx {
      cell: WaiterCell::new(),
      registered: None,
    }
  }

  fn take_terminal(&mut self) -> Option<TerminalState<T>> {
    match self.cell.state.load(Ordering::Acquire) {
      WAITER_FULFILLED => {
        self.registered = None;
        self.cell.rearm();
        Some(TerminalState::Fulfilled(
          self.cell.take().expect("fulfilled waiter cell must hold an item"),
        ))
      }
      WAITER_NOTIFIED => {
        self.registered = None;
        self.cell.rearm();
        Some(TerminalState::Notified)
      }
      _ => None,
    }
  }
}

/// Internal outcome for the blocking path; mapped to `RecvError` /
/// `RecvErrorTimeout` by the handles.
pub(crate) enum RecvTimeoutOutcome {
  Disconnected,
  Timeout,
}

impl<T> Drop for UnboundedShared<T> {
  fn drop(&mut self) {
    // Drain every published item, then retire the final tail node (retirement
    // runs one behind). Reclaimed items drop with the VecDeque.
    let c = self.consumer.get_mut();
    unsafe {
      loop {
        let tail = c.tail;
        let next = (*tail).next.load(Ordering::Relaxed);
        if next.is_null() {
          break;
        }
        drop((*(*next).val.get()).take());
        c.tail = next;
        retire_node(tail);
      }
      retire_node(c.tail);
    }
  }
}
