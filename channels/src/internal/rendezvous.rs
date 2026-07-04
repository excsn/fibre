//! Shared core for the queued rendezvous channels (`mpsc::rendezvous` and
//! `mpmc::rendezvous`).
//!
//! A rendezvous channel has **no item buffer**. A send completes only when its
//! payload is handed directly to a receiver, and a receive completes only when
//! a sender hands it a payload.
//!
//! # Topology specialization via the receiver store
//!
//! The handshake logic is identical for MPSC and MPMC; they differ only in how
//! many receivers may be parked at once. The core is therefore generic over a
//! [`ReceiverStore`]:
//!
//! * MPMC uses [`VecDeque<RecvRec<T>>`] — many receivers may park in FIFO order.
//! * MPSC uses [`Option<RecvRec<T>>`] — a single consumer, so a one-slot store
//!   with no receiver-queue allocation.
//!
//! Both instantiations monomorphize the same audited handoff/cancel code; the
//! MPSC core is a genuinely distinct, single-slot type ([`MpscRvShared`]) while
//! sharing every data structure and function with MPMC ([`MpmcRvShared`]). The
//! sender side is always a FIFO `VecDeque`.
//!
//! # Transfer model: under-lock handoff ("Mode A")
//!
//! `plan.md` specifies "Mode B" (claim a waiter under the mutex, release the
//! lock, then move `T` outside the lock and publish `DONE`). Mode B is unsound
//! for **async** waiters: the waiter's state atomic and payload live inline in
//! the future to keep parking allocation-free, but if the future is dropped
//! while its node is `TAKING`, `Drop` must not block — so it frees the inline
//! atomic while the matching peer is still about to write `DONE` into it. That
//! is a use-after-free, and closing it would require either an `Arc` on the
//! normal parked path (violating the zero-allocation rule) or an unsound
//! `Drop`.
//!
//! This core therefore completes the entire handoff **while holding the
//! mutex**. The lock is the single linearization point that serializes
//! match-vs-cancel:
//!
//! * A waiter parks by pushing a record holding raw pointers to its inline
//!   `state` atomic and inline payload/destination slot, then waiting on the
//!   state atomic.
//! * A matching peer, under the lock, moves `T` through those pointers, stores
//!   the terminal `DONE` state, removes the record from the store, and only
//!   then (after releasing the lock) wakes the waiter.
//! * Cancellation (future `Drop`, `recv_timeout`) also takes the lock, so it
//!   either finds its still-`WAITING` record and removes it, or observes that
//!   the peer already completed the handoff under the lock.
//!
//! Because the peer only ever touches a waiter's inline memory while holding
//! the lock, and a cancelling waiter only frees that memory after re-acquiring
//! the same lock, there is no window for a use-after-free. The payload is never
//! visible to a receiver before the sender's operation resolves (no early
//! deposit, no buffering), so cancelling a pending send can never ghost-deliver
//! its item.

use crate::error::{RecvError, RecvErrorTimeout, SendError, TryRecvError, TrySendError};
use crate::internal::cache_padded::CachePadded;

use std::collections::VecDeque;
use std::fmt;
use std::task::{Context, Poll, Waker};
use std::time::{Duration, Instant};

use crate::internal::sync::{thread, AtomicU8, Mutex, Ordering, Thread};

// --- Waiter state machine -------------------------------------------------
//
// A record is in exactly one place at a time: either linked in a store (always
// `WAITING`) or removed (terminal). State transitions only ever happen while
// the mutex is held.

/// Parked and eligible to be matched. The owner still owns its payload/slot.
pub(crate) const WAITING: u8 = 0;
/// The handshake committed: the peer moved `T` and the owner may return.
pub(crate) const DONE: u8 = 1;
/// The owner reclaimed its record before any peer matched it.
pub(crate) const CANCELLED: u8 = 2;
/// The channel closed for this waiter; no handoff will occur.
pub(crate) const DISCONNECTED: u8 = 3;

/// How to wake a parked waiter once its terminal state is published.
pub(crate) enum WakeHandle {
  Thread(Thread),
  Waker(Waker),
}

impl WakeHandle {
  #[inline]
  fn wake(self) {
    match self {
      WakeHandle::Thread(t) => t.unpark(),
      WakeHandle::Waker(w) => w.wake(),
    }
  }
}

/// A parked sender's channel-side record.
///
/// `state` and `src` point at memory owned by the sender (its stack frame for
/// blocking sends, or its pinned future for async sends). Both are only
/// dereferenced while the mutex is held.
pub(crate) struct SenderRec<T> {
  state: *const AtomicU8,
  /// Pointer to the sender's inline `Option<T>` payload slot. A matching
  /// receiver `take()`s the item out of it under the lock.
  src: *mut Option<T>,
  wake: WakeHandle,
}

/// A parked receiver's channel-side record.
///
/// `state` and `dest` point at memory owned by the receiver. Both are only
/// dereferenced while the mutex is held.
pub(crate) struct RecvRec<T> {
  state: *const AtomicU8,
  /// Pointer to the receiver's inline `Option<T>` destination slot. A matching
  /// sender writes the item into it under the lock.
  dest: *mut Option<T>,
  wake: WakeHandle,
}

// --- Receiver store -------------------------------------------------------

/// The container of parked receivers, abstracted so MPMC (many) and MPSC (one)
/// can share the same core. All methods run with the mutex held.
pub(crate) trait ReceiverStore<T> {
  fn empty() -> Self;
  /// Parks a receiver record (FIFO for MPMC; the single slot for MPSC).
  fn push_receiver(&mut self, rec: RecvRec<T>);
  /// Removes and returns the oldest parked receiver, if any.
  fn pop_receiver(&mut self) -> Option<RecvRec<T>>;
  /// Refreshes the wake handle of the parked receiver with the given state
  /// pointer. Returns `true` if it was found.
  fn refresh_receiver(&mut self, state_ptr: *const AtomicU8, wake: WakeHandle) -> bool;
  /// Removes the parked receiver with the given state pointer, if present.
  fn remove_receiver(&mut self, state_ptr: *const AtomicU8);
  /// Drains every parked receiver, publishing `DISCONNECTED` and collecting
  /// wake handles.
  fn disconnect_all(&mut self, wakes: &mut Vec<WakeHandle>);
}

impl<T> ReceiverStore<T> for VecDeque<RecvRec<T>> {
  #[inline]
  fn empty() -> Self {
    VecDeque::new()
  }
  #[inline]
  fn push_receiver(&mut self, rec: RecvRec<T>) {
    self.push_back(rec);
  }
  #[inline]
  fn pop_receiver(&mut self) -> Option<RecvRec<T>> {
    self.pop_front()
  }
  fn refresh_receiver(&mut self, state_ptr: *const AtomicU8, wake: WakeHandle) -> bool {
    if let Some(rec) = self.iter_mut().find(|r| r.state == state_ptr) {
      rec.wake = wake;
      true
    } else {
      false
    }
  }
  fn remove_receiver(&mut self, state_ptr: *const AtomicU8) {
    if let Some(pos) = self.iter().position(|r| r.state == state_ptr) {
      self.remove(pos);
    }
  }
  fn disconnect_all(&mut self, wakes: &mut Vec<WakeHandle>) {
    for rec in self.drain(..) {
      unsafe { (*rec.state).store(DISCONNECTED, Ordering::Release) };
      wakes.push(rec.wake);
    }
  }
}

impl<T> ReceiverStore<T> for Option<RecvRec<T>> {
  #[inline]
  fn empty() -> Self {
    None
  }
  #[inline]
  fn push_receiver(&mut self, rec: RecvRec<T>) {
    debug_assert!(self.is_none(), "MPSC rendezvous parked two receivers");
    *self = Some(rec);
  }
  #[inline]
  fn pop_receiver(&mut self) -> Option<RecvRec<T>> {
    self.take()
  }
  fn refresh_receiver(&mut self, state_ptr: *const AtomicU8, wake: WakeHandle) -> bool {
    match self {
      Some(rec) if rec.state == state_ptr => {
        rec.wake = wake;
        true
      }
      _ => false,
    }
  }
  fn remove_receiver(&mut self, state_ptr: *const AtomicU8) {
    if matches!(self, Some(rec) if rec.state == state_ptr) {
      *self = None;
    }
  }
  fn disconnect_all(&mut self, wakes: &mut Vec<WakeHandle>) {
    if let Some(rec) = self.take() {
      unsafe { (*rec.state).store(DISCONNECTED, Ordering::Release) };
      wakes.push(rec.wake);
    }
  }
}

// --- Core -----------------------------------------------------------------

struct Core<T, R: ReceiverStore<T>> {
  sender_waiters: VecDeque<SenderRec<T>>,
  receivers: R,
  sender_count: usize,
  receiver_count: usize,
}

/// The shared, `Arc`-wrapped state of a queued rendezvous channel.
pub(crate) struct RendezvousShared<T, R: ReceiverStore<T>> {
  core: CachePadded<Mutex<Core<T, R>>>,
}

/// MPMC rendezvous core: a FIFO queue of parked receivers.
pub(crate) type MpmcRvShared<T> = RendezvousShared<T, VecDeque<RecvRec<T>>>;
/// MPSC rendezvous core: a single parked-receiver slot (one consumer).
pub(crate) type MpscRvShared<T> = RendezvousShared<T, Option<RecvRec<T>>>;

impl<T, R: ReceiverStore<T>> fmt::Debug for RendezvousShared<T, R> {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("RendezvousShared").finish_non_exhaustive()
  }
}

// The raw pointers in the records always point at live waiter memory (the
// waiter cannot free it without re-taking the lock), and `T: Send` is required
// to move payloads across threads.
unsafe impl<T: Send, R: ReceiverStore<T>> Send for RendezvousShared<T, R> {}
unsafe impl<T: Send, R: ReceiverStore<T>> Sync for RendezvousShared<T, R> {}

impl<T: Send, R: ReceiverStore<T>> RendezvousShared<T, R> {
  pub(crate) fn new() -> Self {
    Self {
      core: CachePadded::new(Mutex::new(Core {
        sender_waiters: VecDeque::new(),
        receivers: R::empty(),
        sender_count: 1,
        receiver_count: 1,
      })),
    }
  }

  // --- Handle bookkeeping -------------------------------------------------

  pub(crate) fn add_sender(&self) {
    self.core.lock().sender_count += 1;
  }

  pub(crate) fn add_receiver(&self) {
    self.core.lock().receiver_count += 1;
  }

  pub(crate) fn receiver_count(&self) -> usize {
    self.core.lock().receiver_count
  }

  pub(crate) fn sender_count(&self) -> usize {
    self.core.lock().sender_count
  }

  /// Drops one sender handle. If it was the last, wakes every parked receiver
  /// with `DISCONNECTED`.
  pub(crate) fn drop_sender(&self) {
    let mut wakes = Vec::new();
    {
      let mut core = self.core.lock();
      core.sender_count -= 1;
      if core.sender_count == 0 {
        core.receivers.disconnect_all(&mut wakes);
      }
    }
    for w in wakes {
      w.wake();
    }
  }

  /// Drops one receiver handle. If it was the last, wakes every parked sender
  /// with `DISCONNECTED`.
  pub(crate) fn drop_receiver(&self) {
    let mut wakes = Vec::new();
    {
      let mut core = self.core.lock();
      core.receiver_count -= 1;
      if core.receiver_count == 0 {
        for rec in core.sender_waiters.drain(..) {
          unsafe { (*rec.state).store(DISCONNECTED, Ordering::Release) };
          wakes.push(rec.wake);
        }
      }
    }
    for w in wakes {
      w.wake();
    }
  }

  // --- Non-blocking operations -------------------------------------------

  /// Attempts an immediate handoff to a waiting receiver.
  pub(crate) fn try_send(&self, item: T) -> Result<(), TrySendError<T>> {
    let mut core = self.core.lock();
    if core.receiver_count == 0 {
      return Err(TrySendError::Closed(item));
    }
    match core.receivers.pop_receiver() {
      Some(rec) => {
        let wake = fulfill_receiver(rec, item);
        drop(core);
        wake.wake();
        Ok(())
      }
      // No receiver was parked: a rendezvous send cannot make progress now.
      None => Err(TrySendError::Full(item)),
    }
  }

  /// Attempts an immediate handoff from a waiting sender.
  pub(crate) fn try_recv(&self) -> Result<T, TryRecvError> {
    let mut core = self.core.lock();
    match core.sender_waiters.pop_front() {
      Some(rec) => {
        let (item, wake) = fulfill_sender(rec);
        drop(core);
        wake.wake();
        Ok(item)
      }
      None => {
        if core.sender_count == 0 {
          Err(TryRecvError::Disconnected)
        } else {
          Err(TryRecvError::Empty)
        }
      }
    }
  }

  // --- Blocking operations -----------------------------------------------

  pub(crate) fn send_blocking(&self, item: T) -> Result<(), SendError> {
    let state = AtomicU8::new(WAITING);
    let mut slot = Some(item);
    let state_ptr = &state as *const AtomicU8;

    {
      let mut core = self.core.lock();
      if core.receiver_count == 0 {
        return Err(SendError::Closed);
      }
      // Fast path: a receiver is already parked — hand off directly.
      if let Some(rec) = core.receivers.pop_receiver() {
        let wake = fulfill_receiver(rec, slot.take().unwrap());
        drop(core);
        wake.wake();
        return Ok(());
      }
      // Park: publish a record pointing at our stack-local state and slot.
      core.sender_waiters.push_back(SenderRec {
        state: state_ptr,
        src: &mut slot as *mut Option<T>,
        wake: WakeHandle::Thread(thread::current()),
      });
    }

    // Wait for a receiver to take the item (or for disconnection). The record
    // is removed from the queue under the lock before either terminal state is
    // published, so once we observe it the pointers are no longer aliased.
    park_until_terminal(&state);
    match state.load(Ordering::Acquire) {
      DONE => Ok(()),
      _ => Err(SendError::Closed),
    }
  }

  pub(crate) fn recv_blocking(&self) -> Result<T, RecvError> {
    let state = AtomicU8::new(WAITING);
    let mut dest: Option<T> = None;
    let state_ptr = &state as *const AtomicU8;

    {
      let mut core = self.core.lock();
      // Fast path: a sender is already parked — take its item directly.
      if let Some(rec) = core.sender_waiters.pop_front() {
        let (item, wake) = fulfill_sender(rec);
        drop(core);
        wake.wake();
        return Ok(item);
      }
      if core.sender_count == 0 {
        return Err(RecvError::Disconnected);
      }
      core.receivers.push_receiver(RecvRec {
        state: state_ptr,
        dest: &mut dest as *mut Option<T>,
        wake: WakeHandle::Thread(thread::current()),
      });
    }

    park_until_terminal(&state);
    match state.load(Ordering::Acquire) {
      DONE => Ok(dest.take().expect("DONE implies the sender wrote the item")),
      _ => Err(RecvError::Disconnected),
    }
  }

  pub(crate) fn recv_timeout(&self, timeout: Duration) -> Result<T, RecvErrorTimeout> {
    let deadline = Instant::now().checked_add(timeout);
    let state = AtomicU8::new(WAITING);
    let mut dest: Option<T> = None;
    let state_ptr = &state as *const AtomicU8;

    {
      let mut core = self.core.lock();
      if let Some(rec) = core.sender_waiters.pop_front() {
        let (item, wake) = fulfill_sender(rec);
        drop(core);
        wake.wake();
        return Ok(item);
      }
      if core.sender_count == 0 {
        return Err(RecvErrorTimeout::Disconnected);
      }
      core.receivers.push_receiver(RecvRec {
        state: state_ptr,
        dest: &mut dest as *mut Option<T>,
        wake: WakeHandle::Thread(thread::current()),
      });
    }

    loop {
      let st = state.load(Ordering::Acquire);
      if st != WAITING {
        break;
      }
      match deadline {
        Some(dl) => {
          let now = Instant::now();
          if now >= dl {
            // Timed out: try to cancel. If the CAS fails, a sender committed
            // the handoff concurrently and we must honor the delivery.
            if self.cancel_receiver(state_ptr, &state) {
              return Err(RecvErrorTimeout::Timeout);
            }
            break;
          }
          thread::park_timeout(dl - now);
        }
        None => thread::park(),
      }
    }

    match state.load(Ordering::Acquire) {
      DONE => Ok(dest.take().expect("DONE implies the sender wrote the item")),
      CANCELLED => Err(RecvErrorTimeout::Timeout),
      _ => Err(RecvErrorTimeout::Disconnected),
    }
  }

  // --- Async polling ------------------------------------------------------

  /// Polls a send. `slot` is the future's inline payload; `state` is its inline
  /// state atomic.
  pub(crate) fn poll_send(
    &self,
    cx: &mut Context<'_>,
    state: &AtomicU8,
    slot: &mut Option<T>,
    registered: &mut bool,
  ) -> Poll<Result<(), SendError>> {
    let state_ptr = state as *const AtomicU8;

    if *registered {
      match state.load(Ordering::Acquire) {
        WAITING => {
          // Spurious poll: refresh the waker in place.
          let mut core = self.core.lock();
          if core
            .sender_waiters
            .iter_mut()
            .find(|r| r.state == state_ptr)
            .map(|rec| rec.wake = WakeHandle::Waker(cx.waker().clone()))
            .is_some()
          {
            return Poll::Pending;
          }
          // The record left the queue between our load and the lock: a peer
          // committed the handoff (or close fired) under the lock. Re-read the
          // now-terminal state rather than re-attempting a fresh send (our
          // payload slot may already be empty).
          drop(core);
          *registered = false;
          return match state.load(Ordering::Acquire) {
            DONE => Poll::Ready(Ok(())),
            _ => Poll::Ready(Err(SendError::Closed)),
          };
        }
        DONE => {
          *registered = false;
          return Poll::Ready(Ok(()));
        }
        _ => {
          *registered = false;
          return Poll::Ready(Err(SendError::Closed));
        }
      }
    }

    let mut core = self.core.lock();
    if core.receiver_count == 0 {
      return Poll::Ready(Err(SendError::Closed));
    }
    if let Some(rec) = core.receivers.pop_receiver() {
      let item = slot.take().expect("poll_send called without a payload");
      let wake = fulfill_receiver(rec, item);
      drop(core);
      wake.wake();
      return Poll::Ready(Ok(()));
    }
    state.store(WAITING, Ordering::Release);
    *registered = true;
    core.sender_waiters.push_back(SenderRec {
      state: state_ptr,
      src: slot as *mut Option<T>,
      wake: WakeHandle::Waker(cx.waker().clone()),
    });
    Poll::Pending
  }

  /// Polls a receive. `dest` is the future's inline destination slot.
  pub(crate) fn poll_recv(
    &self,
    cx: &mut Context<'_>,
    state: &AtomicU8,
    dest: &mut Option<T>,
    registered: &mut bool,
  ) -> Poll<Result<T, RecvError>> {
    let state_ptr = state as *const AtomicU8;

    if *registered {
      match state.load(Ordering::Acquire) {
        WAITING => {
          let mut core = self.core.lock();
          if core
            .receivers
            .refresh_receiver(state_ptr, WakeHandle::Waker(cx.waker().clone()))
          {
            return Poll::Pending;
          }
          // The record left the queue between our load and the lock: a sender
          // committed the handoff (or close fired) under the lock. Re-read the
          // now-terminal state rather than re-attempting a fresh recv (the item
          // may already sit in our `dest`).
          drop(core);
          *registered = false;
          return match state.load(Ordering::Acquire) {
            DONE => Poll::Ready(Ok(
              dest.take().expect("DONE implies the sender wrote the item"),
            )),
            _ => Poll::Ready(Err(RecvError::Disconnected)),
          };
        }
        DONE => {
          *registered = false;
          return Poll::Ready(Ok(
            dest.take().expect("DONE implies the sender wrote the item")
          ));
        }
        _ => {
          *registered = false;
          return Poll::Ready(Err(RecvError::Disconnected));
        }
      }
    }

    let mut core = self.core.lock();
    if let Some(rec) = core.sender_waiters.pop_front() {
      let (item, wake) = fulfill_sender(rec);
      drop(core);
      wake.wake();
      return Poll::Ready(Ok(item));
    }
    if core.sender_count == 0 {
      return Poll::Ready(Err(RecvError::Disconnected));
    }
    state.store(WAITING, Ordering::Release);
    *registered = true;
    core.receivers.push_receiver(RecvRec {
      state: state_ptr,
      dest: dest as *mut Option<T>,
      wake: WakeHandle::Waker(cx.waker().clone()),
    });
    Poll::Pending
  }

  // --- Cancellation -------------------------------------------------------

  /// Cancels a parked sender. Returns `true` if the record was still `WAITING`
  /// and was removed (the owner keeps its payload); `false` if the handoff
  /// already committed (delivery stands).
  pub(crate) fn cancel_sender(&self, state_ptr: *const AtomicU8, state: &AtomicU8) -> bool {
    if state
      .compare_exchange(WAITING, CANCELLED, Ordering::SeqCst, Ordering::SeqCst)
      .is_err()
    {
      return false;
    }
    let mut core = self.core.lock();
    if let Some(pos) = core.sender_waiters.iter().position(|r| r.state == state_ptr) {
      core.sender_waiters.remove(pos);
    }
    true
  }

  /// Cancels a parked receiver. Returns `true` if the record was still
  /// `WAITING` and was removed; `false` if a sender already committed the
  /// handoff (the item now sits in the receiver's `dest`).
  pub(crate) fn cancel_receiver(&self, state_ptr: *const AtomicU8, state: &AtomicU8) -> bool {
    if state
      .compare_exchange(WAITING, CANCELLED, Ordering::SeqCst, Ordering::SeqCst)
      .is_err()
    {
      return false;
    }
    self.core.lock().receivers.remove_receiver(state_ptr);
    true
  }
}

// --- Record-level transfer helpers ----------------------------------------
//
// These perform the actual unsafe move of `T` through a record's pointers and
// publish the terminal state. They run with the mutex held.

/// Writes `item` into a parked receiver's inline destination and publishes
/// `DONE`, returning its wake handle.
fn fulfill_receiver<T>(rec: RecvRec<T>, item: T) -> WakeHandle {
  unsafe {
    *rec.dest = Some(item);
    (*rec.state).store(DONE, Ordering::Release);
  }
  rec.wake
}

/// Takes the payload from a parked sender's inline slot and publishes `DONE`,
/// returning the item and its wake handle.
fn fulfill_sender<T>(rec: SenderRec<T>) -> (T, WakeHandle) {
  let item = unsafe {
    let item = (*rec.src)
      .take()
      .expect("a parked sender always holds an item");
    (*rec.state).store(DONE, Ordering::Release);
    item
  };
  (item, rec.wake)
}

/// Blocks the current thread until `state` leaves `WAITING`, tolerating
/// spurious wakeups.
fn park_until_terminal(state: &AtomicU8) {
  while state.load(Ordering::Acquire) == WAITING {
    thread::park();
  }
}
