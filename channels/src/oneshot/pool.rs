//! Pooled oneshot channels.
//!
//! [`pair_pool()`] creates an [`OneshotPairPool`]: contiguous slot storage plus a
//! lock-free freelist. [`OneshotPairPool::pair`] pops a slot and returns owned
//! handles with the same contract as [`exclusive()`](super::exclusive): a consuming
//! `send`, `&mut self` receive methods, drop-closes. Slots are recycled, never
//! freed per channel, so the handles carry no reference counts, and the sender
//! never touches the waker unless the receiver announced it was about to park.
//! Whichever handle exits last pushes the slot back.
//!
//! The pool may be dropped while channels are in flight: storage release is
//! deferred until the last outstanding channel retires.
//!
//! [`OneshotHostPool`] is the fused variant: each pool cell is a whole caller
//! record with a [`PoolSlot`] reply channel embedded (projection supplied at
//! construction), so a request/reply exchange performs no allocation at all.
//!
//! # Examples
//!
//! ```
//! use fibre::oneshot;
//!
//! let pool = oneshot::pair_pool::<u32>(64);
//! let (tx, mut rx) = pool.pair().unwrap();
//!
//! tokio::runtime::Runtime::new().unwrap().block_on(async {
//!     tx.send(42).unwrap();
//!     assert_eq!(rx.recv().await.unwrap(), 42);
//! });
//! ```

use crate::error::{RecvError, TryRecvError, TrySendError};
use crate::internal::sync::{AtomicU32, AtomicU64, AtomicU8, AtomicUsize, AtomicWaker, Ordering};

use std::cell::UnsafeCell;
use std::fmt;
use std::future::Future;
use std::mem::MaybeUninit;
use std::pin::Pin;
use std::ptr::NonNull;
use std::task::{Context, Poll};

const VALUE: u8 = 1 << 0;
const SENDER_GONE: u8 = 1 << 1;
const RECEIVER_GONE: u8 = 1 << 2;
const WAITING: u8 = 1 << 3;
/// Set by a sender that saw `WAITING`, as its last access to the slot.
const SENDER_DONE: u8 = 1 << 4;

struct SenderGoneMark;

enum Sent<T> {
  Delivered,
  /// Delivered to a receiver that has already exited: the caller recycles.
  DeliveredLastOut,
  /// The receiver exited first: the value comes back and the caller recycles.
  Rejected(T),
}

/// One recycled channel core. The value cell is initialized iff `VALUE` is set
/// and the receiving side has not consumed it; taken-ness lives in the receiver
/// handle's `done` flag, not in the state word, because a cancelling receiver by
/// definition never took the value.
struct Slot<T> {
  state: AtomicU8,
  waker: AtomicWaker,
  value: UnsafeCell<MaybeUninit<T>>,
}

unsafe impl<T: Send> Send for Slot<T> {}
unsafe impl<T: Send> Sync for Slot<T> {}

impl<T> Slot<T> {
  fn new() -> Self {
    Slot {
      state: AtomicU8::new(0),
      waker: AtomicWaker::new(),
      value: UnsafeCell::new(MaybeUninit::uninit()),
    }
  }

  /// Publishes the value, then wakes a receiver that announced `WAITING`. The
  /// announcement always precedes the receiver's first waker registration, so
  /// a never-parked exchange touches the waker zero times.
  ///
  /// A sender that saw `WAITING` takes the waker out, sets `SENDER_DONE` as its
  /// last access to the slot and wakes with its own copy. A receiver that
  /// announced is not the recycler until it has seen `SENDER_DONE`.
  ///
  /// Takes a pointer because the receiver may free the slot's storage as soon
  /// as the sender's last access has landed.
  ///
  /// Safety: `this` is a live slot and the caller is its only sender.
  unsafe fn send(this: NonNull<Self>, value: T) -> Sent<T> {
    let slot = this.as_ptr();
    unsafe { (*(*slot).value.get()).write(value) };
    let prev = unsafe { (*slot).state.fetch_or(VALUE, Ordering::AcqRel) };
    debug_assert_eq!(prev & (VALUE | SENDER_GONE), 0, "pool Slot::send called twice");
    if prev & RECEIVER_GONE != 0 {
      return Sent::Rejected(unsafe { (*(*slot).value.get()).assume_init_read() });
    }
    if prev & WAITING == 0 {
      return Sent::Delivered;
    }
    let waker = unsafe { (*slot).waker.take() };
    let prev = unsafe { (*slot).state.fetch_or(SENDER_DONE, Ordering::AcqRel) };
    if let Some(waker) = waker {
      waker.wake();
    }
    if prev & RECEIVER_GONE != 0 {
      Sent::DeliveredLastOut
    } else {
      Sent::Delivered
    }
  }

  /// Returns true when the caller is the recycler: the receiver had already
  /// exited when this close set `SENDER_GONE`. A close that saw `WAITING`
  /// looks again when it sets `SENDER_DONE`.
  ///
  /// Safety: same contract as [`Slot::send`].
  unsafe fn close_sender(this: NonNull<Self>) -> bool {
    let slot = this.as_ptr();
    let prev = unsafe { (*slot).state.fetch_or(SENDER_GONE, Ordering::AcqRel) };
    if prev & RECEIVER_GONE != 0 {
      return true;
    }
    if prev & WAITING == 0 {
      return false;
    }
    let waker = unsafe { (*slot).waker.take() };
    let prev = unsafe { (*slot).state.fetch_or(SENDER_DONE, Ordering::AcqRel) };
    if let Some(waker) = waker {
      waker.wake();
    }
    prev & RECEIVER_GONE != 0
  }

  /// `waiting_set` records that the receiver announced before the sender
  /// published, which is when the sender takes the `SENDER_DONE` path. The
  /// announcement is a fetch_or made before the first registration. A sender
  /// ordered before it has already published, so its outcome is returned here
  /// and `waiting_set` stays false. A sender ordered after it sees `WAITING`,
  /// and the register-then-recheck below observes its value.
  fn poll_recv(&self, cx: &mut Context<'_>, waiting_set: &mut bool) -> Poll<Result<T, SenderGoneMark>> {
    let state = self.state.load(Ordering::Acquire);
    if state & VALUE != 0 {
      return Poll::Ready(Ok(unsafe { (*self.value.get()).assume_init_read() }));
    }
    if state & SENDER_GONE != 0 {
      return Poll::Ready(Err(SenderGoneMark));
    }
    if !*waiting_set {
      let prev = self.state.fetch_or(WAITING, Ordering::AcqRel);
      if prev & VALUE != 0 {
        return Poll::Ready(Ok(unsafe { (*self.value.get()).assume_init_read() }));
      }
      if prev & SENDER_GONE != 0 {
        return Poll::Ready(Err(SenderGoneMark));
      }
      *waiting_set = true;
    }
    self.waker.register(cx.waker());
    let state = self.state.load(Ordering::Acquire);
    if state & VALUE != 0 {
      return Poll::Ready(Ok(unsafe { (*self.value.get()).assume_init_read() }));
    }
    if state & SENDER_GONE != 0 {
      return Poll::Ready(Err(SenderGoneMark));
    }
    Poll::Pending
  }

  fn try_recv(&self) -> Option<Result<T, SenderGoneMark>> {
    let state = self.state.load(Ordering::Acquire);
    if state & VALUE != 0 {
      return Some(Ok(unsafe { (*self.value.get()).assume_init_read() }));
    }
    if state & SENDER_GONE != 0 {
      return Some(Err(SenderGoneMark));
    }
    None
  }

  /// Cancel-path receiver exit; the caller must be a receiver that never took
  /// the value, so a present value is by definition unconsumed and is dropped
  /// here before `RECEIVER_GONE` goes in. The compare-exchange leaves no window
  /// for a value to arrive in between: a sender that publishes later sees
  /// `RECEIVER_GONE` and takes its value back. Returns true when the caller is
  /// the recycler.
  ///
  /// Safety: `this` is a live slot and the caller is its only receiver.
  unsafe fn close_receiver(this: NonNull<Self>, waiting_set: bool) -> bool {
    let slot = this.as_ptr();
    let mut state = unsafe { (*slot).state.load(Ordering::Acquire) };
    let mut dropped = false;
    loop {
      if state & VALUE != 0 && !dropped {
        unsafe { (*(*slot).value.get()).assume_init_drop() };
        dropped = true;
      }
      match unsafe {
        (*slot).state.compare_exchange_weak(
          state,
          state | RECEIVER_GONE,
          Ordering::AcqRel,
          Ordering::Acquire,
        )
      } {
        Ok(_) => break,
        Err(now) => state = now,
      }
    }
    if state & (VALUE | SENDER_GONE) == 0 {
      return false;
    }
    !waiting_set || state & SENDER_DONE != 0
  }

  /// Receiver exit after it took the value or saw the disconnect. Returns true
  /// when the caller is the recycler. A receiver that never announced always
  /// is, with no atomic operation.
  ///
  /// Safety: same contract as [`Slot::close_receiver`].
  #[inline]
  unsafe fn finish_receiver(this: NonNull<Self>, waiting_set: bool) -> bool {
    if !waiting_set {
      return true;
    }
    let slot = this.as_ptr();
    if unsafe { (*slot).state.load(Ordering::Acquire) } & SENDER_DONE != 0 {
      return true;
    }
    let prev = unsafe { (*slot).state.fetch_or(RECEIVER_GONE, Ordering::AcqRel) };
    prev & SENDER_DONE != 0
  }

  /// Recycler-only: the caller holds the slot exclusively as this channel's
  /// last-out. Every exit path leaves the value cell uninitialized before
  /// reaching here. The waker take is gated on `WAITING` since no announcement
  /// means no registration ever happened.
  fn reset(&self) {
    if self.state.load(Ordering::Relaxed) & WAITING != 0 {
      drop(self.waker.take());
    }
    self.state.store(0, Ordering::Relaxed);
  }

  fn is_receiver_gone(&self) -> bool {
    self.state.load(Ordering::Acquire) & RECEIVER_GONE != 0
  }

  fn is_disconnected(&self) -> bool {
    let state = self.state.load(Ordering::Acquire);
    state & VALUE == 0 && state & SENDER_GONE != 0
  }
}

const NIL: u32 = u32::MAX;

/// Tagged Treiber stack over out-of-line links. The head packs
/// `(tag << 32) | index`; the tag bumps on every successful CAS so a pop/push
/// ABA on the same index cannot validate a stale link. `next` entries only ever
/// hold `NIL` or a valid index: they are written exclusively by `push` (from a
/// loaded head) and by construction.
struct Freelist {
  head: AtomicU64,
  next: Box<[AtomicU32]>,
}

impl Freelist {
  fn new(capacity: usize) -> Self {
    assert!(capacity < NIL as usize, "pool capacity must be below u32::MAX");
    let next = (0..capacity)
      .map(|i| AtomicU32::new(if i + 1 < capacity { i as u32 + 1 } else { NIL }))
      .collect();
    Freelist {
      head: AtomicU64::new(if capacity == 0 { NIL as u64 } else { 0 }),
      next,
    }
  }

  #[inline]
  fn pop(&self) -> Option<u32> {
    loop {
      let head = self.head.load(Ordering::Acquire);
      let idx = head as u32;
      if idx == NIL {
        return None;
      }
      let next = self.next[idx as usize].load(Ordering::Relaxed);
      let new = ((head >> 32).wrapping_add(1)) << 32 | next as u64;
      if self
        .head
        .compare_exchange_weak(head, new, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
      {
        return Some(idx);
      }
    }
  }

  /// All-or-nothing pop of `n` indices into `out`. `false` means fewer than `n`
  /// slots were observed on a stable head; concurrent churn can make this
  /// conservatively false even when pushes land mid-walk.
  #[inline]
  fn pop_chain(&self, n: usize, out: &mut Vec<u32>) -> bool {
    if n == 0 {
      return true;
    }
    'retry: loop {
      out.clear();
      let head = self.head.load(Ordering::Acquire);
      let mut idx = head as u32;
      for _ in 0..n {
        if idx == NIL {
          if self.head.load(Ordering::Acquire) == head {
            return false;
          }
          continue 'retry;
        }
        out.push(idx);
        idx = self.next[idx as usize].load(Ordering::Relaxed);
      }
      let new = ((head >> 32).wrapping_add(1)) << 32 | idx as u64;
      if self
        .head
        .compare_exchange(head, new, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
      {
        return true;
      }
    }
  }

  #[inline]
  fn push(&self, idx: u32) {
    loop {
      let head = self.head.load(Ordering::Relaxed);
      self.next[idx as usize].store(head as u32, Ordering::Relaxed);
      let new = ((head >> 32).wrapping_add(1)) << 32 | idx as u64;
      if self
        .head
        .compare_exchange_weak(head, new, Ordering::Release, Ordering::Relaxed)
        .is_ok()
      {
        return;
      }
    }
  }
}

/// Lifecycle word: `outstanding_channels * 2 | CLOSED`. Each live channel holds
/// one stake, taken at pair time and settled by its retire; the pool handle's
/// drop sets `CLOSED`. Whoever observes the word emptying (the pool drop seeing
/// zero stakes, or the last retire seeing `CLOSED`) frees the inner.
const CLOSED: usize = 1;
const STAKE: usize = 2;

struct PairInner<T: Send> {
  list: Freelist,
  slots: Box<[Slot<T>]>,
  lifecycle: AtomicUsize,
}

impl<T: Send> PairInner<T> {
  /// Settles one channel's stake after recycling its slot.
  ///
  /// Safety: the caller must be the channel's last-out handle, and `inner` must
  /// originate from this pool's `Box::into_raw`.
  #[inline]
  unsafe fn retire(inner: NonNull<Self>, idx: u32) {
    let r = unsafe { inner.as_ref() };
    r.slots[idx as usize].reset();
    r.list.push(idx);
    if r.lifecycle.fetch_sub(STAKE, Ordering::AcqRel) == STAKE | CLOSED {
      drop(unsafe { Box::from_raw(inner.as_ptr()) });
    }
  }
}

/// A pool of recycled oneshot channels; see the [module docs](self).
pub struct OneshotPairPool<T: Send> {
  inner: NonNull<PairInner<T>>,
}

unsafe impl<T: Send> Send for OneshotPairPool<T> {}
unsafe impl<T: Send> Sync for OneshotPairPool<T> {}

/// Creates a pool of `capacity` recycled oneshot channels.
pub fn pair_pool<T: Send>(capacity: usize) -> OneshotPairPool<T> {
  OneshotPairPool::new(capacity)
}

impl<T: Send> OneshotPairPool<T> {
  pub fn new(capacity: usize) -> Self {
    let inner = Box::new(PairInner {
      list: Freelist::new(capacity),
      slots: (0..capacity).map(|_| Slot::new()).collect(),
      lifecycle: AtomicUsize::new(0),
    });
    OneshotPairPool {
      inner: unsafe { NonNull::new_unchecked(Box::into_raw(inner)) },
    }
  }

  pub fn capacity(&self) -> usize {
    unsafe { self.inner.as_ref() }.slots.len()
  }

  /// Pops a slot and returns the channel over it, or `None` when the pool is
  /// exhausted. The handles are owned and `Send`; the channel retires its slot
  /// back to the pool when the second handle finishes.
  pub fn pair(&self) -> Option<(PooledSender<T>, PooledReceiver<T>)> {
    let inner = unsafe { self.inner.as_ref() };
    let idx = inner.list.pop()?;
    inner.lifecycle.fetch_add(STAKE, Ordering::Relaxed);
    Some(self.assemble(idx))
  }

  /// All-or-nothing batch of `n` channels in one freelist operation, or `None`
  /// when fewer than `n` slots were available at observation time.
  pub fn pair_batch(&self, n: usize) -> Option<Vec<(PooledSender<T>, PooledReceiver<T>)>> {
    let inner = unsafe { self.inner.as_ref() };
    let mut indices = Vec::with_capacity(n);
    if !inner.list.pop_chain(n, &mut indices) {
      return None;
    }
    inner.lifecycle.fetch_add(STAKE * n, Ordering::Relaxed);
    Some(indices.into_iter().map(|idx| self.assemble(idx)).collect())
  }

  fn assemble(&self, idx: u32) -> (PooledSender<T>, PooledReceiver<T>) {
    let inner = unsafe { self.inner.as_ref() };
    let slot = NonNull::from(&inner.slots[idx as usize]);
    (
      PooledSender {
        slot,
        inner: self.inner,
        idx,
      },
      PooledReceiver {
        slot,
        inner: self.inner,
        idx,
        done: false,
        exited: false,
        waiting_set: false,
      },
    )
  }
}

impl<T: Send> Drop for OneshotPairPool<T> {
  fn drop(&mut self) {
    let prev = unsafe { self.inner.as_ref() }
      .lifecycle
      .fetch_or(CLOSED, Ordering::AcqRel);
    if prev == 0 {
      drop(unsafe { Box::from_raw(self.inner.as_ptr()) });
    }
  }
}

impl<T: Send> fmt::Debug for OneshotPairPool<T> {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("OneshotPairPool")
      .field("capacity", &self.capacity())
      .finish_non_exhaustive()
  }
}

/// The sending side of a pooled oneshot channel. Not clonable.
pub struct PooledSender<T: Send> {
  slot: NonNull<Slot<T>>,
  inner: NonNull<PairInner<T>>,
  idx: u32,
}

unsafe impl<T: Send> Send for PooledSender<T> {}

impl<T: Send> PooledSender<T> {
  /// Sends a value, consuming the sender.
  ///
  /// Fails with [`TrySendError::Closed`] returning the value if the receiver
  /// was dropped or explicitly closed.
  pub fn send(self, value: T) -> Result<(), TrySendError<T>> {
    let slot = self.slot;
    let inner = self.inner;
    let idx = self.idx;
    std::mem::forget(self);
    match unsafe { Slot::send(slot, value) } {
      Sent::Delivered => Ok(()),
      Sent::DeliveredLastOut => {
        unsafe { PairInner::retire(inner, idx) };
        Ok(())
      }
      Sent::Rejected(v) => {
        unsafe { PairInner::retire(inner, idx) };
        Err(TrySendError::Closed(v))
      }
    }
  }

  /// Closes this sender without sending. Equivalent to dropping it.
  pub fn close(self) {}

  /// Checks if the channel's receiver has been dropped or closed.
  pub fn is_closed(&self) -> bool {
    unsafe { self.slot.as_ref() }.is_receiver_gone()
  }
}

impl<T: Send> Drop for PooledSender<T> {
  fn drop(&mut self) {
    if unsafe { Slot::close_sender(self.slot) } {
      unsafe { PairInner::retire(self.inner, self.idx) };
    }
  }
}

impl<T: Send> fmt::Debug for PooledSender<T> {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("PooledSender")
      .field("is_closed", &self.is_closed())
      .finish_non_exhaustive()
  }
}

/// The receiving side of a pooled oneshot channel. Not clonable.
pub struct PooledReceiver<T: Send> {
  slot: NonNull<Slot<T>>,
  inner: NonNull<PairInner<T>>,
  idx: u32,
  done: bool,
  exited: bool,
  waiting_set: bool,
}

unsafe impl<T: Send> Send for PooledReceiver<T> {}

impl<T: Send> PooledReceiver<T> {
  /// Attempts to receive the value non-blockingly.
  ///
  /// Returns `Err(TryRecvError::Empty)` while the sender is alive and has not
  /// sent, and `Err(TryRecvError::Disconnected)` once the value has been taken,
  /// the sender dropped without sending, or this handle was closed.
  pub fn try_recv(&mut self) -> Result<T, TryRecvError> {
    if self.done || self.exited {
      return Err(TryRecvError::Disconnected);
    }
    match unsafe { self.slot.as_ref() }.try_recv() {
      Some(Ok(value)) => {
        self.done = true;
        Ok(value)
      }
      Some(Err(SenderGoneMark)) => {
        self.done = true;
        Err(TryRecvError::Disconnected)
      }
      None => Err(TryRecvError::Empty),
    }
  }

  /// Waits asynchronously for the value.
  pub fn recv(&mut self) -> PooledReceiveFuture<'_, T> {
    PooledReceiveFuture { receiver: self }
  }

  /// Blocking receive for synchronous callers: parks the thread until the
  /// value arrives or the channel disconnects.
  pub fn recv_blocking(&mut self) -> Result<T, RecvError> {
    crate::sync_util::block_on(self.recv())
  }

  /// Closes the receiving end. After this, `send` fails and returns the value.
  ///
  /// This is an explicit alternative to `drop`. If a value was already sent
  /// but not yet received, it is dropped.
  pub fn close(&mut self) {
    if self.exited {
      return;
    }
    self.exited = true;
    let sender_exited = if self.done {
      unsafe { Slot::finish_receiver(self.slot, self.waiting_set) }
    } else {
      self.done = true;
      unsafe { Slot::close_receiver(self.slot, self.waiting_set) }
    };
    if sender_exited {
      unsafe { PairInner::retire(self.inner, self.idx) };
    }
  }

  /// Checks if no value can ever be received anymore: the value was taken,
  /// this handle was closed, or the sender dropped without sending.
  pub fn is_closed(&self) -> bool {
    self.done || self.exited || unsafe { self.slot.as_ref() }.is_disconnected()
  }
}

impl<T: Send> Drop for PooledReceiver<T> {
  fn drop(&mut self) {
    if self.exited {
      return;
    }
    let sender_exited = if self.done {
      unsafe { Slot::finish_receiver(self.slot, self.waiting_set) }
    } else {
      unsafe { Slot::close_receiver(self.slot, self.waiting_set) }
    };
    if sender_exited {
      unsafe { PairInner::retire(self.inner, self.idx) };
    }
  }
}

impl<T: Send> fmt::Debug for PooledReceiver<T> {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("PooledReceiver")
      .field("done", &self.done)
      .finish_non_exhaustive()
  }
}

#[must_use = "futures do nothing unless you .await or poll them"]
pub struct PooledReceiveFuture<'a, T: Send> {
  receiver: &'a mut PooledReceiver<T>,
}

impl<'a, T: Send> Future for PooledReceiveFuture<'a, T> {
  type Output = Result<T, RecvError>;

  fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    let rx = &mut *self.receiver;
    if rx.done || rx.exited {
      return Poll::Ready(Err(RecvError::Disconnected));
    }
    let polled = unsafe { rx.slot.as_ref() }.poll_recv(cx, &mut rx.waiting_set);
    match polled {
      Poll::Ready(Ok(value)) => {
        rx.done = true;
        Poll::Ready(Ok(value))
      }
      Poll::Ready(Err(SenderGoneMark)) => {
        rx.done = true;
        Poll::Ready(Err(RecvError::Disconnected))
      }
      Poll::Pending => Poll::Pending,
    }
  }
}

/// The reply slot a pooled host record embeds. Opaque: it only becomes a
/// channel through the projection an [`OneshotHostPool`] is constructed with.
pub struct PoolSlot<T> {
  slot: Slot<T>,
}

impl<T> PoolSlot<T> {
  pub fn new() -> Self {
    PoolSlot { slot: Slot::new() }
  }
}

impl<T> Default for PoolSlot<T> {
  fn default() -> Self {
    Self::new()
  }
}

impl<T> fmt::Debug for PoolSlot<T> {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("PoolSlot").finish_non_exhaustive()
  }
}

struct HostInner<H: Send + Sync, T: Send> {
  list: Freelist,
  cells: Box<[UnsafeCell<H>]>,
  project: fn(&H) -> &PoolSlot<T>,
  lifecycle: AtomicUsize,
}

unsafe impl<H: Send + Sync, T: Send> Send for HostInner<H, T> {}
unsafe impl<H: Send + Sync, T: Send> Sync for HostInner<H, T> {}

impl<H: Send + Sync, T: Send> HostInner<H, T> {
  fn slot(&self, idx: u32) -> &Slot<T> {
    &(self.project)(unsafe { &*self.cells[idx as usize].get() }).slot
  }

  /// Safety: same contract as [`PairInner::retire`].
  unsafe fn retire(inner: NonNull<Self>, idx: u32) {
    let r = unsafe { inner.as_ref() };
    r.slot(idx).reset();
    r.list.push(idx);
    if r.lifecycle.fetch_sub(STAKE, Ordering::AcqRel) == STAKE | CLOSED {
      drop(unsafe { Box::from_raw(inner.as_ptr()) });
    }
  }
}

/// A pool whose cells are whole caller records with the reply channel embedded;
/// see the [module docs](self). Cell mutation is exclusive between pop and
/// push, the freelist being the ownership token, and `pair_init` runs its
/// closure inside that window.
pub struct OneshotHostPool<H: Send + Sync, T: Send> {
  inner: NonNull<HostInner<H, T>>,
}

unsafe impl<H: Send + Sync, T: Send> Send for OneshotHostPool<H, T> {}
unsafe impl<H: Send + Sync, T: Send> Sync for OneshotHostPool<H, T> {}

impl<H: Send + Sync, T: Send> OneshotHostPool<H, T> {
  /// `make` builds each cell's record once at construction; `project` locates
  /// the record's embedded [`PoolSlot`] and must always return the same field.
  pub fn new(capacity: usize, mut make: impl FnMut() -> H, project: fn(&H) -> &PoolSlot<T>) -> Self {
    let inner = Box::new(HostInner {
      list: Freelist::new(capacity),
      cells: (0..capacity).map(|_| UnsafeCell::new(make())).collect(),
      project,
      lifecycle: AtomicUsize::new(0),
    });
    OneshotHostPool {
      inner: unsafe { NonNull::new_unchecked(Box::into_raw(inner)) },
    }
  }

  pub fn capacity(&self) -> usize {
    unsafe { self.inner.as_ref() }.cells.len()
  }

  /// Pops a record cell, runs `init` on it exclusively, and returns the channel
  /// over its embedded slot, or `None` when the pool is exhausted. The record
  /// stays readable through [`HostReceiver::host`] for the channel's lifetime.
  pub fn pair_init(&self, init: impl FnOnce(&mut H)) -> Option<(HostSender<H, T>, HostReceiver<H, T>)> {
    let inner = unsafe { self.inner.as_ref() };
    let idx = inner.list.pop()?;
    inner.lifecycle.fetch_add(STAKE, Ordering::Relaxed);
    init(unsafe { &mut *inner.cells[idx as usize].get() });
    Some(self.assemble(idx))
  }

  /// All-or-nothing batch variant of [`pair_init`](Self::pair_init); `init`
  /// runs once per popped record.
  pub fn pair_init_batch(
    &self,
    n: usize,
    mut init: impl FnMut(&mut H),
  ) -> Option<Vec<(HostSender<H, T>, HostReceiver<H, T>)>> {
    let inner = unsafe { self.inner.as_ref() };
    let mut indices = Vec::with_capacity(n);
    if !inner.list.pop_chain(n, &mut indices) {
      return None;
    }
    inner.lifecycle.fetch_add(STAKE * n, Ordering::Relaxed);
    Some(
      indices
        .into_iter()
        .map(|idx| {
          init(unsafe { &mut *inner.cells[idx as usize].get() });
          self.assemble(idx)
        })
        .collect(),
    )
  }

  fn assemble(&self, idx: u32) -> (HostSender<H, T>, HostReceiver<H, T>) {
    let inner = unsafe { self.inner.as_ref() };
    let slot = NonNull::from(inner.slot(idx));
    let cell = NonNull::from(&inner.cells[idx as usize]);
    (
      HostSender {
        slot,
        inner: self.inner,
        idx,
      },
      HostReceiver {
        cell,
        slot,
        inner: self.inner,
        idx,
        done: false,
        exited: false,
        waiting_set: false,
      },
    )
  }
}

impl<H: Send + Sync, T: Send> Drop for OneshotHostPool<H, T> {
  fn drop(&mut self) {
    let prev = unsafe { self.inner.as_ref() }
      .lifecycle
      .fetch_or(CLOSED, Ordering::AcqRel);
    if prev == 0 {
      drop(unsafe { Box::from_raw(self.inner.as_ptr()) });
    }
  }
}

impl<H: Send + Sync, T: Send> fmt::Debug for OneshotHostPool<H, T> {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("OneshotHostPool")
      .field("capacity", &self.capacity())
      .finish_non_exhaustive()
  }
}

/// The sending side of a hosted pooled oneshot channel. Not clonable.
pub struct HostSender<H: Send + Sync, T: Send> {
  slot: NonNull<Slot<T>>,
  inner: NonNull<HostInner<H, T>>,
  idx: u32,
}

unsafe impl<H: Send + Sync, T: Send> Send for HostSender<H, T> {}

impl<H: Send + Sync, T: Send> HostSender<H, T> {
  /// Sends a value, consuming the sender; see [`PooledSender::send`].
  pub fn send(self, value: T) -> Result<(), TrySendError<T>> {
    let slot = self.slot;
    let inner = self.inner;
    let idx = self.idx;
    std::mem::forget(self);
    match unsafe { Slot::send(slot, value) } {
      Sent::Delivered => Ok(()),
      Sent::DeliveredLastOut => {
        unsafe { HostInner::retire(inner, idx) };
        Ok(())
      }
      Sent::Rejected(v) => {
        unsafe { HostInner::retire(inner, idx) };
        Err(TrySendError::Closed(v))
      }
    }
  }

  /// Closes this sender without sending. Equivalent to dropping it.
  pub fn close(self) {}

  /// Checks if the channel's receiver has been dropped or closed.
  pub fn is_closed(&self) -> bool {
    unsafe { self.slot.as_ref() }.is_receiver_gone()
  }
}

impl<H: Send + Sync, T: Send> Drop for HostSender<H, T> {
  fn drop(&mut self) {
    if unsafe { Slot::close_sender(self.slot) } {
      unsafe { HostInner::retire(self.inner, self.idx) };
    }
  }
}

impl<H: Send + Sync, T: Send> fmt::Debug for HostSender<H, T> {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("HostSender")
      .field("is_closed", &self.is_closed())
      .finish_non_exhaustive()
  }
}

/// The receiving side of a hosted pooled oneshot channel. Not clonable.
pub struct HostReceiver<H: Send + Sync, T: Send> {
  cell: NonNull<UnsafeCell<H>>,
  slot: NonNull<Slot<T>>,
  inner: NonNull<HostInner<H, T>>,
  idx: u32,
  done: bool,
  exited: bool,
  waiting_set: bool,
}

unsafe impl<H: Send + Sync, T: Send> Send for HostReceiver<H, T> {}

impl<H: Send + Sync, T: Send> HostReceiver<H, T> {
  /// The record this channel rides in, readable until the receiver finishes.
  pub fn host(&self) -> &H {
    unsafe { &*self.cell.as_ref().get() }
  }

  /// See [`PooledReceiver::try_recv`].
  pub fn try_recv(&mut self) -> Result<T, TryRecvError> {
    if self.done || self.exited {
      return Err(TryRecvError::Disconnected);
    }
    match unsafe { self.slot.as_ref() }.try_recv() {
      Some(Ok(value)) => {
        self.done = true;
        Ok(value)
      }
      Some(Err(SenderGoneMark)) => {
        self.done = true;
        Err(TryRecvError::Disconnected)
      }
      None => Err(TryRecvError::Empty),
    }
  }

  /// Waits asynchronously for the value.
  pub fn recv(&mut self) -> HostReceiveFuture<'_, H, T> {
    HostReceiveFuture { receiver: self }
  }

  /// Blocking receive for synchronous callers: parks the thread until the
  /// value arrives or the channel disconnects.
  pub fn recv_blocking(&mut self) -> Result<T, RecvError> {
    crate::sync_util::block_on(self.recv())
  }

  /// See [`PooledReceiver::close`].
  pub fn close(&mut self) {
    if self.exited {
      return;
    }
    self.exited = true;
    let sender_exited = if self.done {
      unsafe { Slot::finish_receiver(self.slot, self.waiting_set) }
    } else {
      self.done = true;
      unsafe { Slot::close_receiver(self.slot, self.waiting_set) }
    };
    if sender_exited {
      unsafe { HostInner::retire(self.inner, self.idx) };
    }
  }

  /// See [`PooledReceiver::is_closed`].
  pub fn is_closed(&self) -> bool {
    self.done || self.exited || unsafe { self.slot.as_ref() }.is_disconnected()
  }
}

impl<H: Send + Sync, T: Send> Drop for HostReceiver<H, T> {
  fn drop(&mut self) {
    if self.exited {
      return;
    }
    let sender_exited = if self.done {
      unsafe { Slot::finish_receiver(self.slot, self.waiting_set) }
    } else {
      unsafe { Slot::close_receiver(self.slot, self.waiting_set) }
    };
    if sender_exited {
      unsafe { HostInner::retire(self.inner, self.idx) };
    }
  }
}

impl<H: Send + Sync, T: Send> fmt::Debug for HostReceiver<H, T> {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("HostReceiver")
      .field("done", &self.done)
      .finish_non_exhaustive()
  }
}

#[must_use = "futures do nothing unless you .await or poll them"]
pub struct HostReceiveFuture<'a, H: Send + Sync, T: Send> {
  receiver: &'a mut HostReceiver<H, T>,
}

impl<'a, H: Send + Sync, T: Send> Future for HostReceiveFuture<'a, H, T> {
  type Output = Result<T, RecvError>;

  fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    let rx = &mut *self.receiver;
    if rx.done || rx.exited {
      return Poll::Ready(Err(RecvError::Disconnected));
    }
    let polled = unsafe { rx.slot.as_ref() }.poll_recv(cx, &mut rx.waiting_set);
    match polled {
      Poll::Ready(Ok(value)) => {
        rx.done = true;
        Poll::Ready(Ok(value))
      }
      Poll::Ready(Err(SenderGoneMark)) => {
        rx.done = true;
        Poll::Ready(Err(RecvError::Disconnected))
      }
      Poll::Pending => Poll::Pending,
    }
  }
}
