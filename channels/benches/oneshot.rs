use bench_matrix::{
  criterion_runner::async_suite::AsyncBenchmarkSuite, AbstractCombination, MatrixCellValue,
};
use criterion::{criterion_group, criterion_main, Criterion, Throughput};
use std::{
  future::Future,
  hint::black_box,
  pin::Pin,
  sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Barrier,
  },
  thread,
  time::{Duration, Instant},
};
use tokio::runtime::Runtime;

use fibre::oneshot;

mod slot {
  use futures_util::task::AtomicWaker;
  use std::cell::UnsafeCell;
  use std::mem::MaybeUninit;
  use std::sync::atomic::{AtomicU8, Ordering};
  use std::task::{Context, Poll};

  const VALUE: u8 = 1;
  const SENDER_GONE: u8 = 2;
  const RECEIVER_GONE: u8 = 4;
  const TAKEN: u8 = 8;
  const SEALED: u8 = 16;
  const WAITING: u8 = 32;

  /// The error returned when the sending side went away without delivering a value.
  #[derive(Debug, PartialEq, Eq)]
  pub struct SenderGone;

  /// A single-shot slot that lives inside a caller's allocation instead of allocating
  /// its own, with one sender and one receiver.
  ///
  /// Discipline the embedding code must uphold: `send` and `close_sender` are called at
  /// most once between them, from one party; `poll_recv` and `close_receiver` belong to
  /// a single receiver. The state bits exist so whichever side finishes second cleans up
  /// the value; `Drop` covers a value nobody claimed.
  pub struct Slot<T> {
    state: AtomicU8,
    waker: AtomicWaker,
    value: UnsafeCell<MaybeUninit<T>>,
  }

  unsafe impl<T: Send> Sync for Slot<T> {}
  unsafe impl<T: Send> Send for Slot<T> {}

  #[allow(dead_code)]
  impl<T> Slot<T> {
    pub fn new() -> Self {
      Slot {
        state: AtomicU8::new(0),
        waker: AtomicWaker::new(),
        value: UnsafeCell::new(MaybeUninit::uninit()),
      }
    }

    /// Delivers the value, waking a parked receiver. Returns it back if the receiver is
    /// already gone. The cell write is exclusive because `send` is called at most once
    /// and the receiver reads only after observing `VALUE`.
    pub fn send(&self, value: T) -> Result<(), T> {
      unsafe { (*self.value.get()).write(value) };
      let prev = self.state.fetch_or(VALUE, Ordering::AcqRel);
      debug_assert_eq!(prev & (VALUE | SENDER_GONE), 0, "Slot::send called twice");
      if prev & RECEIVER_GONE != 0 {
        return Err(unsafe { (*self.value.get()).assume_init_read() });
      }
      self.waker.wake();
      Ok(())
    }

    /// Marks the sender gone without a value, waking a parked receiver into an error.
    pub fn close_sender(&self) {
      let prev = self.state.fetch_or(SENDER_GONE, Ordering::AcqRel);
      if prev & (VALUE | RECEIVER_GONE) == 0 {
        self.waker.wake();
      }
    }

    /// Marks the receiver gone so a later `send` drops its value immediately, and drops
    /// a value that was delivered but never taken.
    pub fn close_receiver(&self) {
      let prev = self.state.fetch_or(RECEIVER_GONE, Ordering::AcqRel);
      if prev & VALUE != 0 && prev & TAKEN == 0 {
        self.state.fetch_or(TAKEN, Ordering::Relaxed);
        unsafe { (*self.value.get()).assume_init_drop() };
      }
    }

    pub fn poll_recv(&self, cx: &mut Context<'_>) -> Poll<Result<T, SenderGone>> {
      let state = self.state.load(Ordering::Acquire);
      if state & VALUE != 0 {
        self.state.fetch_or(TAKEN, Ordering::Relaxed);
        return Poll::Ready(Ok(unsafe { (*self.value.get()).assume_init_read() }));
      }
      if state & SENDER_GONE != 0 {
        return Poll::Ready(Err(SenderGone));
      }

      self.waker.register(cx.waker());
      // Recheck after registering, or a send between the load and the register would
      // never wake this receiver.
      let state = self.state.load(Ordering::Acquire);
      if state & VALUE != 0 {
        self.state.fetch_or(TAKEN, Ordering::Relaxed);
        return Poll::Ready(Ok(unsafe { (*self.value.get()).assume_init_read() }));
      }
      if state & SENDER_GONE != 0 {
        return Poll::Ready(Err(SenderGone));
      }
      Poll::Pending
    }

    pub async fn recv(&self) -> Result<T, SenderGone> {
      std::future::poll_fn(|cx| self.poll_recv(cx)).await
    }

    /// `None` means no value yet with the sender still alive.
    pub fn try_recv(&self) -> Option<Result<T, SenderGone>> {
      let state = self.state.load(Ordering::Acquire);
      if state & VALUE != 0 {
        self.state.fetch_or(TAKEN, Ordering::Relaxed);
        return Some(Ok(unsafe { (*self.value.get()).assume_init_read() }));
      }
      if state & SENDER_GONE != 0 {
        return Some(Err(SenderGone));
      }
      None
    }

    pub fn is_receiver_gone(&self) -> bool {
      self.state.load(Ordering::Acquire) & RECEIVER_GONE != 0
    }

    pub fn is_disconnected(&self) -> bool {
      let state = self.state.load(Ordering::Acquire);
      state & VALUE == 0 && state & SENDER_GONE != 0
    }

    /// `send` for exit-tracked handle pairs where whichever side exits last frees the
    /// shared storage. The wake, and the `SEALED` fetch_or that lets a receiver-side
    /// freer wait out the wake, run only when the receiver has announced `WAITING`;
    /// otherwise the `VALUE` fetch_or is this side's final access to the slot's
    /// memory. `Err` means the receiver already exited: the value comes back and the
    /// caller must run the release.
    pub fn send_exit(&self, value: T) -> Result<(), T> {
      unsafe { (*self.value.get()).write(value) };
      let prev = self.state.fetch_or(VALUE, Ordering::AcqRel);
      debug_assert_eq!(prev & (VALUE | SENDER_GONE), 0, "Slot::send_exit called twice");
      if prev & RECEIVER_GONE != 0 {
        return Err(unsafe { (*self.value.get()).assume_init_read() });
      }
      if prev & WAITING != 0 {
        self.waker.wake();
        self.state.fetch_or(SEALED, Ordering::Release);
      }
      Ok(())
    }

    /// Returns true when the receiver already exited, making the caller responsible
    /// for releasing the shared storage. Same WAITING-gated wake/seal as `send_exit`.
    pub fn close_sender_exit(&self) -> bool {
      let prev = self.state.fetch_or(SENDER_GONE, Ordering::AcqRel);
      if prev & RECEIVER_GONE != 0 {
        return true;
      }
      if prev & WAITING != 0 {
        self.waker.wake();
        self.state.fetch_or(SEALED, Ordering::Release);
      }
      false
    }

    /// Cancel-path receiver exit. Returns true when the sender already exited (sent
    /// or closed), making the caller responsible for releasing the shared storage;
    /// the caller must first `spin_sealed` when its `WAITING` preceded the sender's
    /// exit. Value cleanup happens before the exit `fetch_or`; a value published
    /// between the load and the `fetch_or` is cleaned up by the slot's own `Drop`
    /// when the storage is released.
    pub fn close_receiver_exit(&self) -> bool {
      let state = self.state.load(Ordering::Acquire);
      if state & VALUE != 0 && state & TAKEN == 0 {
        self.state.fetch_or(TAKEN, Ordering::Relaxed);
        unsafe { (*self.value.get()).assume_init_drop() };
      }
      let prev = self.state.fetch_or(RECEIVER_GONE, Ordering::AcqRel);
      prev & (VALUE | SENDER_GONE) != 0
    }

    /// Pooled send: recycled storage needs no seal, and the wake is gated on the
    /// receiver's `WAITING` announcement (always made before its first registration),
    /// so a never-parked exchange touches the waker zero times. `Err` means the
    /// receiver already exited and the caller is the recycler.
    pub fn send_pooled(&self, value: T) -> Result<(), T> {
      unsafe { (*self.value.get()).write(value) };
      let prev = self.state.fetch_or(VALUE, Ordering::AcqRel);
      debug_assert_eq!(prev & (VALUE | SENDER_GONE), 0, "Slot::send_pooled called twice");
      if prev & RECEIVER_GONE != 0 {
        return Err(unsafe { (*self.value.get()).assume_init_read() });
      }
      if prev & WAITING != 0 {
        self.waker.wake();
      }
      Ok(())
    }

    /// Sender exit for recycled (never-freed) slots: `WAITING`-gated wake, and
    /// returns true when the receiver already exited, making the caller the recycler.
    pub fn close_sender_pooled(&self) -> bool {
      let prev = self.state.fetch_or(SENDER_GONE, Ordering::AcqRel);
      if prev & RECEIVER_GONE != 0 {
        return true;
      }
      if prev & WAITING != 0 {
        self.waker.wake();
      }
      false
    }

    /// Pooled receive poll: announces `WAITING` before the first registration and
    /// skips the `TAKEN` bit entirely; the handle's `done` flag is the taken-ness
    /// record, so the caller must not poll again after `Ready`.
    pub fn poll_recv_pooled(
      &self,
      cx: &mut Context<'_>,
      waiting_set: &mut bool,
    ) -> Poll<Result<T, SenderGone>> {
      let state = self.state.load(Ordering::Acquire);
      if state & VALUE != 0 {
        return Poll::Ready(Ok(unsafe { (*self.value.get()).assume_init_read() }));
      }
      if state & SENDER_GONE != 0 {
        return Poll::Ready(Err(SenderGone));
      }
      if !*waiting_set {
        self.state.fetch_or(WAITING, Ordering::AcqRel);
        *waiting_set = true;
      }
      self.waker.register(cx.waker());
      let state = self.state.load(Ordering::Acquire);
      if state & VALUE != 0 {
        return Poll::Ready(Ok(unsafe { (*self.value.get()).assume_init_read() }));
      }
      if state & SENDER_GONE != 0 {
        return Poll::Ready(Err(SenderGone));
      }
      Poll::Pending
    }

    /// Cancel-path receiver exit for pooled slots. Caller must be a receiver that
    /// never took the value (`!done`), which is why no `TAKEN` bit is needed: a
    /// present value is by definition unconsumed. A value published between the load
    /// and the exit `fetch_or` is caught by the second check (the sender saw no
    /// `RECEIVER_GONE`, so it returned `Ok` and will never touch the cell).
    pub fn close_receiver_pooled(&self) -> bool {
      let state = self.state.load(Ordering::Acquire);
      if state & VALUE != 0 {
        unsafe { (*self.value.get()).assume_init_drop() };
      }
      let prev = self.state.fetch_or(RECEIVER_GONE, Ordering::AcqRel);
      if prev & VALUE != 0 && state & VALUE == 0 {
        unsafe { (*self.value.get()).assume_init_drop() };
      }
      prev & (VALUE | SENDER_GONE) != 0
    }

    /// Recycler-only: the caller must hold the slot exclusively (just popped, or just
    /// determined to be last-out). The waker take is gated on `WAITING`: an announce
    /// always precedes any registration, so no announcement means no waker to clear.
    /// A stale sender may still be inside `wake`, which only touches the waker;
    /// concurrent take/wake is within AtomicWaker's contract and at worst spuriously
    /// wakes the slot's next user.
    pub fn reset(&self) {
      if self.state.load(Ordering::Relaxed) & WAITING != 0 {
        drop(self.waker.take());
      }
      self.state.store(0, Ordering::Relaxed);
    }

    /// Bounded: the sender seals immediately after the wake that `WAITING` gated in.
    pub fn spin_sealed(&self) {
      while self.state.load(Ordering::Acquire) & SEALED == 0 {
        std::hint::spin_loop();
      }
    }

    /// `poll_recv` for exit-tracked receivers: announces `WAITING` before the first
    /// registration. `seal_expected` records whether the announcement preceded the
    /// sender's exit (the total order of the two fetch_ors decides), which is exactly
    /// whether the sender will run the wake/seal path and the freeing receiver must
    /// `spin_sealed` first.
    pub fn poll_recv_exit(
      &self,
      cx: &mut Context<'_>,
      waiting_set: &mut bool,
      seal_expected: &mut bool,
    ) -> Poll<Result<T, SenderGone>> {
      let state = self.state.load(Ordering::Acquire);
      if state & VALUE != 0 {
        self.state.fetch_or(TAKEN, Ordering::Relaxed);
        return Poll::Ready(Ok(unsafe { (*self.value.get()).assume_init_read() }));
      }
      if state & SENDER_GONE != 0 {
        return Poll::Ready(Err(SenderGone));
      }
      if !*waiting_set {
        let prev = self.state.fetch_or(WAITING, Ordering::AcqRel);
        *waiting_set = true;
        *seal_expected = prev & (VALUE | SENDER_GONE) == 0;
      }
      self.waker.register(cx.waker());
      let state = self.state.load(Ordering::Acquire);
      if state & VALUE != 0 {
        self.state.fetch_or(TAKEN, Ordering::Relaxed);
        return Poll::Ready(Ok(unsafe { (*self.value.get()).assume_init_read() }));
      }
      if state & SENDER_GONE != 0 {
        return Poll::Ready(Err(SenderGone));
      }
      Poll::Pending
    }
  }

  impl<T> Drop for Slot<T> {
    fn drop(&mut self) {
      let state = *self.state.get_mut();
      if state & VALUE != 0 && state & TAKEN == 0 {
        unsafe { (*self.value.get()).assume_init_drop() };
      }
    }
  }
}

use slot::Slot;

/// `oneshot::exclusive()` as it would be if `ExclusiveShared` were rewritten into `Slot`:
/// the same `Arc`, the same two handles and the same public surface, over the slot engine.
mod slot_exclusive {
  use super::slot::{SenderGone, Slot};
  use fibre::error::{RecvError, TryRecvError, TrySendError};
  use std::future::Future;
  use std::mem::ManuallyDrop;
  use std::pin::Pin;
  use std::sync::Arc;
  use std::task::{Context, Poll};

  pub fn channel<T>() -> (SlotSender<T>, SlotReceiver<T>) {
    let shared = Arc::new(Slot::new());
    (
      SlotSender {
        shared: ManuallyDrop::new(Arc::clone(&shared)),
      },
      SlotReceiver {
        shared,
        done: false,
      },
    )
  }

  pub struct SlotSender<T> {
    shared: ManuallyDrop<Arc<Slot<T>>>,
  }

  #[allow(dead_code)]
  impl<T> SlotSender<T> {
    pub fn send(mut self, value: T) -> Result<(), TrySendError<T>> {
      let shared = unsafe { ManuallyDrop::take(&mut self.shared) };
      std::mem::forget(self);
      shared.send(value).map_err(TrySendError::Closed)
    }

    pub fn close(self) {}

    pub fn is_closed(&self) -> bool {
      self.shared.is_receiver_gone()
    }
  }

  impl<T> Drop for SlotSender<T> {
    fn drop(&mut self) {
      self.shared.close_sender();
      unsafe { ManuallyDrop::drop(&mut self.shared) };
    }
  }

  pub struct SlotReceiver<T> {
    shared: Arc<Slot<T>>,
    done: bool,
  }

  #[allow(dead_code)]
  impl<T> SlotReceiver<T> {
    pub fn try_recv(&mut self) -> Result<T, TryRecvError> {
      if self.done {
        return Err(TryRecvError::Disconnected);
      }
      match self.shared.try_recv() {
        Some(Ok(value)) => {
          self.done = true;
          Ok(value)
        }
        Some(Err(SenderGone)) => {
          self.done = true;
          Err(TryRecvError::Disconnected)
        }
        None => Err(TryRecvError::Empty),
      }
    }

    pub fn recv(&mut self) -> SlotRecvFuture<'_, T> {
      SlotRecvFuture { receiver: self }
    }

    pub fn close(&mut self) {
      if self.done {
        return;
      }
      self.done = true;
      self.shared.close_receiver();
    }

    pub fn is_closed(&self) -> bool {
      self.done || self.shared.is_disconnected()
    }
  }

  impl<T> Drop for SlotReceiver<T> {
    fn drop(&mut self) {
      if !self.done {
        self.shared.close_receiver();
      }
    }
  }

  #[must_use = "futures do nothing unless you .await or poll them"]
  pub struct SlotRecvFuture<'a, T> {
    receiver: &'a mut SlotReceiver<T>,
  }

  impl<'a, T> Future for SlotRecvFuture<'a, T> {
    type Output = Result<T, RecvError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
      let rx = &mut *self.receiver;
      match rx.shared.poll_recv(cx) {
        Poll::Ready(Ok(value)) => {
          rx.done = true;
          Poll::Ready(Ok(value))
        }
        Poll::Ready(Err(SenderGone)) => {
          rx.done = true;
          Poll::Ready(Err(RecvError::Disconnected))
        }
        Poll::Pending => Poll::Pending,
      }
    }
  }
}

/// `exclusive()`'s handle contract with owned, `'static`, movable handles and NO
/// reference counting: creation consumes the storage owner (a `Box`ed record) into a
/// raw pointer, both handles carry a plain copy of pointer + release shim, and
/// whichever side's exit `fetch_or` on the slot's state word observes the other side's
/// exit bit frees the storage. Handles are parameterized on `T` alone.
mod slot_owned {
  use super::slot::{SenderGone, Slot};
  use fibre::error::{RecvError, TryRecvError, TrySendError};
  use std::future::Future;
  use std::pin::Pin;
  use std::ptr::NonNull;
  use std::task::{Context, Poll};

  #[derive(Clone, Copy)]
  struct Release {
    ptr: *const (),
    drop_fn: unsafe fn(*const ()),
  }

  // Points at a Box<H> with H: Send + 'static, per pair_boxed's bounds.
  unsafe impl Send for Release {}

  impl Release {
    /// Caller must be the side whose exit observed the other side's exit bit.
    unsafe fn run(self) {
      unsafe { (self.drop_fn)(self.ptr) };
    }
  }

  unsafe fn drop_boxed<H>(ptr: *const ()) {
    unsafe { drop(Box::from_raw(ptr as *const H as *mut H)) };
  }

  pub fn pair_boxed<H, T>(
    owner: Box<H>,
    project: fn(&H) -> &Slot<T>,
  ) -> (OwnedSender<T>, OwnedReceiver<T>)
  where
    H: Send + 'static,
    T: Send,
  {
    let raw = Box::into_raw(owner);
    let slot = NonNull::from(project(unsafe { &*raw }));
    let release = Release {
      ptr: raw as *const (),
      drop_fn: drop_boxed::<H>,
    };
    (
      OwnedSender { slot, release },
      OwnedReceiver {
        slot,
        release,
        done: false,
        exited: false,
        waiting_set: false,
        seal_expected: false,
      },
    )
  }

  pub fn channel<T: Send + 'static>() -> (OwnedSender<T>, OwnedReceiver<T>) {
    pair_boxed(Box::new(Slot::new()), |s| s)
  }

  pub struct OwnedSender<T: Send> {
    slot: NonNull<Slot<T>>,
    release: Release,
  }

  unsafe impl<T: Send> Send for OwnedSender<T> {}

  #[allow(dead_code)]
  impl<T: Send> OwnedSender<T> {
    pub fn send(self, value: T) -> Result<(), TrySendError<T>> {
      let slot = self.slot;
      let release = self.release;
      std::mem::forget(self);
      match unsafe { slot.as_ref() }.send_exit(value) {
        Ok(()) => Ok(()),
        Err(v) => {
          unsafe { release.run() };
          Err(TrySendError::Closed(v))
        }
      }
    }

    pub fn close(self) {}

    pub fn is_closed(&self) -> bool {
      unsafe { self.slot.as_ref() }.is_receiver_gone()
    }
  }

  impl<T: Send> Drop for OwnedSender<T> {
    fn drop(&mut self) {
      if unsafe { self.slot.as_ref() }.close_sender_exit() {
        unsafe { self.release.run() };
      }
    }
  }

  pub struct OwnedReceiver<T: Send> {
    slot: NonNull<Slot<T>>,
    release: Release,
    done: bool,
    exited: bool,
    waiting_set: bool,
    seal_expected: bool,
  }

  unsafe impl<T: Send> Send for OwnedReceiver<T> {}

  #[allow(dead_code)]
  impl<T: Send> OwnedReceiver<T> {
    pub fn try_recv(&mut self) -> Result<T, TryRecvError> {
      if self.done || self.exited {
        return Err(TryRecvError::Disconnected);
      }
      match unsafe { self.slot.as_ref() }.try_recv() {
        Some(Ok(value)) => {
          self.done = true;
          Ok(value)
        }
        Some(Err(SenderGone)) => {
          self.done = true;
          Err(TryRecvError::Disconnected)
        }
        None => Err(TryRecvError::Empty),
      }
    }

    pub fn recv(&mut self) -> OwnedRecvFuture<'_, T> {
      OwnedRecvFuture { receiver: self }
    }

    pub fn close(&mut self) {
      if self.exited {
        return;
      }
      self.exited = true;
      let slot = unsafe { self.slot.as_ref() };
      let sender_exited = if self.done {
        true
      } else {
        self.done = true;
        slot.close_receiver_exit()
      };
      if sender_exited {
        if self.waiting_set && self.seal_expected {
          slot.spin_sealed();
        }
        unsafe { self.release.run() };
      }
    }

    pub fn is_closed(&self) -> bool {
      self.done || self.exited || unsafe { self.slot.as_ref() }.is_disconnected()
    }
  }

  impl<T: Send> Drop for OwnedReceiver<T> {
    fn drop(&mut self) {
      if self.exited {
        return;
      }
      let slot = unsafe { self.slot.as_ref() };
      let sender_exited = if self.done {
        true
      } else {
        slot.close_receiver_exit()
      };
      if sender_exited {
        if self.waiting_set && self.seal_expected {
          slot.spin_sealed();
        }
        unsafe { self.release.run() };
      }
    }
  }

  #[must_use = "futures do nothing unless you .await or poll them"]
  pub struct OwnedRecvFuture<'a, T: Send> {
    receiver: &'a mut OwnedReceiver<T>,
  }

  impl<'a, T: Send> Future for OwnedRecvFuture<'a, T> {
    type Output = Result<T, RecvError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
      let rx = &mut *self.receiver;
      if rx.exited {
        return Poll::Ready(Err(RecvError::Disconnected));
      }
      let polled = unsafe { rx.slot.as_ref() }.poll_recv_exit(
        cx,
        &mut rx.waiting_set,
        &mut rx.seal_expected,
      );
      match polled {
        Poll::Ready(Ok(value)) => {
          rx.done = true;
          Poll::Ready(Ok(value))
        }
        Poll::Ready(Err(SenderGone)) => {
          rx.done = true;
          Poll::Ready(Err(RecvError::Disconnected))
        }
        Poll::Pending => Poll::Pending,
      }
    }
  }
}

/// Norm's pool shape: an instantiable pool owns contiguous recycled slots, and
/// `pair()` pops one; whichever handle exits last pushes it back. The lifetime
/// guarantee is the pool's: as long as the pool is around, the slots are around, so
/// there is no per-channel liveness machinery at all: no refcount, no seal, and a
/// wake landing on a recycled slot is a tolerated spurious wake of its next user.
mod slot_pooled {
  use super::slot::{SenderGone, Slot};
  use fibre::error::{RecvError, TryRecvError, TrySendError};
  use std::cell::UnsafeCell;
  use std::future::Future;
  use std::pin::Pin;
  use std::ptr::NonNull;
  use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
  use std::task::{Context, Poll};

  const NIL: u32 = u32::MAX;

  pub struct Pool<T: Send> {
    inner: Box<PoolInner<T>>,
  }

  struct PoolInner<T: Send> {
    /// Tagged Treiber head: (tag << 32) | index. The tag bumps on every CAS so a
    /// pop/push ABA on the same index cannot validate a stale next-link.
    head: AtomicU64,
    next: Vec<AtomicU32>,
    slots: Vec<Slot<T>>,
  }

  impl<T: Send> Pool<T> {
    pub fn new(capacity: usize) -> Self {
      let slots = (0..capacity).map(|_| Slot::new()).collect();
      let next = (0..capacity)
        .map(|i| AtomicU32::new(if i + 1 < capacity { i as u32 + 1 } else { NIL }))
        .collect();
      let head = AtomicU64::new(if capacity == 0 { NIL as u64 } else { 0 });
      Pool {
        inner: Box::new(PoolInner { head, next, slots }),
      }
    }

    pub fn pair(&self) -> (PooledSender<T>, PooledReceiver<T>) {
      let inner = NonNull::from(&*self.inner);
      let idx = self.inner.pop().expect("slot pool exhausted");
      let slot = NonNull::from(&self.inner.slots[idx as usize]);
      (
        PooledSender { slot, inner, idx },
        PooledReceiver {
          slot,
          inner,
          idx,
          done: false,
          exited: false,
          waiting_set: false,
        },
      )
    }
  }

  impl<T: Send> PoolInner<T> {
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

    fn push(&self, idx: u32) {
      self.slots[idx as usize].reset();
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

  pub struct PooledSender<T: Send> {
    slot: NonNull<Slot<T>>,
    inner: NonNull<PoolInner<T>>,
    idx: u32,
  }

  unsafe impl<T: Send> Send for PooledSender<T> {}

  #[allow(dead_code)]
  impl<T: Send> PooledSender<T> {
    pub fn send(self, value: T) -> Result<(), TrySendError<T>> {
      let slot = self.slot;
      let inner = self.inner;
      let idx = self.idx;
      std::mem::forget(self);
      match unsafe { slot.as_ref() }.send_pooled(value) {
        Ok(()) => Ok(()),
        Err(v) => {
          unsafe { inner.as_ref() }.push(idx);
          Err(TrySendError::Closed(v))
        }
      }
    }

    pub fn close(self) {}

    pub fn is_closed(&self) -> bool {
      unsafe { self.slot.as_ref() }.is_receiver_gone()
    }
  }

  impl<T: Send> Drop for PooledSender<T> {
    fn drop(&mut self) {
      if unsafe { self.slot.as_ref() }.close_sender_pooled() {
        unsafe { self.inner.as_ref() }.push(self.idx);
      }
    }
  }

  pub struct PooledReceiver<T: Send> {
    slot: NonNull<Slot<T>>,
    inner: NonNull<PoolInner<T>>,
    idx: u32,
    done: bool,
    exited: bool,
    waiting_set: bool,
  }

  unsafe impl<T: Send> Send for PooledReceiver<T> {}

  #[allow(dead_code)]
  impl<T: Send> PooledReceiver<T> {
    pub fn try_recv(&mut self) -> Result<T, TryRecvError> {
      if self.done || self.exited {
        return Err(TryRecvError::Disconnected);
      }
      match unsafe { self.slot.as_ref() }.try_recv() {
        Some(Ok(value)) => {
          self.done = true;
          Ok(value)
        }
        Some(Err(SenderGone)) => {
          self.done = true;
          Err(TryRecvError::Disconnected)
        }
        None => Err(TryRecvError::Empty),
      }
    }

    pub fn recv(&mut self) -> PooledRecvFuture<'_, T> {
      PooledRecvFuture { receiver: self }
    }

    pub fn close(&mut self) {
      if self.exited {
        return;
      }
      self.exited = true;
      let sender_exited = if self.done {
        true
      } else {
        self.done = true;
        unsafe { self.slot.as_ref() }.close_receiver_pooled()
      };
      if sender_exited {
        unsafe { self.inner.as_ref() }.push(self.idx);
      }
    }

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
        true
      } else {
        unsafe { self.slot.as_ref() }.close_receiver_pooled()
      };
      if sender_exited {
        unsafe { self.inner.as_ref() }.push(self.idx);
      }
    }
  }

  #[must_use = "futures do nothing unless you .await or poll them"]
  pub struct PooledRecvFuture<'a, T: Send> {
    receiver: &'a mut PooledReceiver<T>,
  }

  impl<'a, T: Send> Future for PooledRecvFuture<'a, T> {
    type Output = Result<T, RecvError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
      let rx = &mut *self.receiver;
      if rx.done || rx.exited {
        return Poll::Ready(Err(RecvError::Disconnected));
      }
      let polled = unsafe { rx.slot.as_ref() }.poll_recv_pooled(cx, &mut rx.waiting_set);
      match polled {
        Poll::Ready(Ok(value)) => {
          rx.done = true;
          Poll::Ready(Ok(value))
        }
        Poll::Ready(Err(SenderGone)) => {
          rx.done = true;
          Poll::Ready(Err(RecvError::Disconnected))
        }
        Poll::Pending => Poll::Pending,
      }
    }
  }

  /// The fused shape: the pool's cells are whole caller records with the slot
  /// embedded, so a request pays zero allocations: one pop hands out the record and
  /// its reply channel together. Cell mutation is exclusive between pop and push
  /// (the freelist is the ownership token); `init` runs in that window.
  pub struct HostPool<H: Send + Sync, T: Send> {
    inner: Box<HostPoolInner<H, T>>,
  }

  struct HostPoolInner<H: Send + Sync, T: Send> {
    head: AtomicU64,
    next: Vec<AtomicU32>,
    cells: Vec<UnsafeCell<H>>,
    project: fn(&H) -> &Slot<T>,
  }

  unsafe impl<H: Send + Sync, T: Send> Send for HostPoolInner<H, T> {}
  unsafe impl<H: Send + Sync, T: Send> Sync for HostPoolInner<H, T> {}

  impl<H: Send + Sync, T: Send> HostPool<H, T> {
    pub fn new(capacity: usize, make: impl Fn() -> H, project: fn(&H) -> &Slot<T>) -> Self {
      let cells = (0..capacity).map(|_| UnsafeCell::new(make())).collect();
      let next = (0..capacity)
        .map(|i| AtomicU32::new(if i + 1 < capacity { i as u32 + 1 } else { NIL }))
        .collect();
      let head = AtomicU64::new(if capacity == 0 { NIL as u64 } else { 0 });
      HostPool {
        inner: Box::new(HostPoolInner {
          head,
          next,
          cells,
          project,
        }),
      }
    }

    pub fn pair_init(
      &self,
      init: impl FnOnce(&mut H),
    ) -> (HostSender<H, T>, HostReceiver<H, T>) {
      let inner = NonNull::from(&*self.inner);
      let idx = self.inner.pop().expect("host pool exhausted");
      let cell = &self.inner.cells[idx as usize];
      init(unsafe { &mut *cell.get() });
      let slot = NonNull::from((self.inner.project)(unsafe { &*cell.get() }));
      (
        HostSender { slot, inner, idx },
        HostReceiver {
          cell: NonNull::from(cell),
          slot,
          inner,
          idx,
          done: false,
          exited: false,
          waiting_set: false,
        },
      )
    }
  }

  impl<H: Send + Sync, T: Send> HostPoolInner<H, T> {
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

    fn push(&self, idx: u32) {
      (self.project)(unsafe { &*self.cells[idx as usize].get() }).reset();
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

  pub struct HostSender<H: Send + Sync, T: Send> {
    slot: NonNull<Slot<T>>,
    inner: NonNull<HostPoolInner<H, T>>,
    idx: u32,
  }

  unsafe impl<H: Send + Sync, T: Send> Send for HostSender<H, T> {}

  #[allow(dead_code)]
  impl<H: Send + Sync, T: Send> HostSender<H, T> {
    pub fn send(self, value: T) -> Result<(), TrySendError<T>> {
      let slot = self.slot;
      let inner = self.inner;
      let idx = self.idx;
      std::mem::forget(self);
      match unsafe { slot.as_ref() }.send_pooled(value) {
        Ok(()) => Ok(()),
        Err(v) => {
          unsafe { inner.as_ref() }.push(idx);
          Err(TrySendError::Closed(v))
        }
      }
    }

    pub fn close(self) {}

    pub fn is_closed(&self) -> bool {
      unsafe { self.slot.as_ref() }.is_receiver_gone()
    }
  }

  impl<H: Send + Sync, T: Send> Drop for HostSender<H, T> {
    fn drop(&mut self) {
      if unsafe { self.slot.as_ref() }.close_sender_pooled() {
        unsafe { self.inner.as_ref() }.push(self.idx);
      }
    }
  }

  pub struct HostReceiver<H: Send + Sync, T: Send> {
    cell: NonNull<UnsafeCell<H>>,
    slot: NonNull<Slot<T>>,
    inner: NonNull<HostPoolInner<H, T>>,
    idx: u32,
    done: bool,
    exited: bool,
    waiting_set: bool,
  }

  unsafe impl<H: Send + Sync, T: Send> Send for HostReceiver<H, T> {}

  #[allow(dead_code)]
  impl<H: Send + Sync, T: Send> HostReceiver<H, T> {
    pub fn host(&self) -> &H {
      unsafe { &*self.cell.as_ref().get() }
    }

    pub fn try_recv(&mut self) -> Result<T, TryRecvError> {
      if self.done || self.exited {
        return Err(TryRecvError::Disconnected);
      }
      match unsafe { self.slot.as_ref() }.try_recv() {
        Some(Ok(value)) => {
          self.done = true;
          Ok(value)
        }
        Some(Err(SenderGone)) => {
          self.done = true;
          Err(TryRecvError::Disconnected)
        }
        None => Err(TryRecvError::Empty),
      }
    }

    pub fn recv(&mut self) -> HostRecvFuture<'_, H, T> {
      HostRecvFuture { receiver: self }
    }

    pub fn close(&mut self) {
      if self.exited {
        return;
      }
      self.exited = true;
      let sender_exited = if self.done {
        true
      } else {
        self.done = true;
        unsafe { self.slot.as_ref() }.close_receiver_pooled()
      };
      if sender_exited {
        unsafe { self.inner.as_ref() }.push(self.idx);
      }
    }

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
        true
      } else {
        unsafe { self.slot.as_ref() }.close_receiver_pooled()
      };
      if sender_exited {
        unsafe { self.inner.as_ref() }.push(self.idx);
      }
    }
  }

  #[must_use = "futures do nothing unless you .await or poll them"]
  pub struct HostRecvFuture<'a, H: Send + Sync, T: Send> {
    receiver: &'a mut HostReceiver<H, T>,
  }

  impl<'a, H: Send + Sync, T: Send> Future for HostRecvFuture<'a, H, T> {
    type Output = Result<T, RecvError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
      let rx = &mut *self.receiver;
      if rx.done || rx.exited {
        return Poll::Ready(Err(RecvError::Disconnected));
      }
      let polled = unsafe { rx.slot.as_ref() }.poll_recv_pooled(cx, &mut rx.waiting_set);
      match polled {
        Poll::Ready(Ok(value)) => {
          rx.done = true;
          Poll::Ready(Ok(value))
        }
        Poll::Ready(Err(SenderGone)) => {
          rx.done = true;
          Poll::Ready(Err(RecvError::Disconnected))
        }
        Poll::Pending => Poll::Pending,
      }
    }
  }
}

/// The embeddable shape: `Slot` stays inside the caller's allocation, and `pair` hands
/// out borrowed one-shot handles enforcing the discipline that raw `Slot` leaves to the
/// caller.
mod slot_split {
  use super::slot::{SenderGone, Slot};
  use fibre::error::{RecvError, TryRecvError, TrySendError};
  use std::future::Future;
  use std::pin::Pin;
  use std::task::{Context, Poll};

  pub fn pair<T>(slot: &Slot<T>) -> (SplitSender<'_, T>, SplitReceiver<'_, T>) {
    (SplitSender { slot }, SplitReceiver { slot, done: false })
  }

  pub struct SplitSender<'a, T> {
    slot: &'a Slot<T>,
  }

  #[allow(dead_code)]
  impl<'a, T> SplitSender<'a, T> {
    pub fn send(self, value: T) -> Result<(), TrySendError<T>> {
      let slot = self.slot;
      std::mem::forget(self);
      slot.send(value).map_err(TrySendError::Closed)
    }

    pub fn close(self) {}

    pub fn is_closed(&self) -> bool {
      self.slot.is_receiver_gone()
    }
  }

  impl<'a, T> Drop for SplitSender<'a, T> {
    fn drop(&mut self) {
      self.slot.close_sender();
    }
  }

  pub struct SplitReceiver<'a, T> {
    slot: &'a Slot<T>,
    done: bool,
  }

  #[allow(dead_code)]
  impl<'a, T> SplitReceiver<'a, T> {
    pub fn try_recv(&mut self) -> Result<T, TryRecvError> {
      if self.done {
        return Err(TryRecvError::Disconnected);
      }
      match self.slot.try_recv() {
        Some(Ok(value)) => {
          self.done = true;
          Ok(value)
        }
        Some(Err(SenderGone)) => {
          self.done = true;
          Err(TryRecvError::Disconnected)
        }
        None => Err(TryRecvError::Empty),
      }
    }

    pub fn recv(&mut self) -> SplitRecvFuture<'_, 'a, T> {
      SplitRecvFuture { receiver: self }
    }

    pub fn close(&mut self) {
      if self.done {
        return;
      }
      self.done = true;
      self.slot.close_receiver();
    }

    pub fn is_closed(&self) -> bool {
      self.done || self.slot.is_disconnected()
    }
  }

  impl<'a, T> Drop for SplitReceiver<'a, T> {
    fn drop(&mut self) {
      if !self.done {
        self.slot.close_receiver();
      }
    }
  }

  #[must_use = "futures do nothing unless you .await or poll them"]
  pub struct SplitRecvFuture<'r, 'a, T> {
    receiver: &'r mut SplitReceiver<'a, T>,
  }

  impl<'r, 'a, T> Future for SplitRecvFuture<'r, 'a, T> {
    type Output = Result<T, RecvError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
      let rx = &mut *self.receiver;
      match rx.slot.poll_recv(cx) {
        Poll::Ready(Ok(value)) => {
          rx.done = true;
          Poll::Ready(Ok(value))
        }
        Poll::Ready(Err(SenderGone)) => {
          rx.done = true;
          Poll::Ready(Err(RecvError::Disconnected))
        }
        Poll::Pending => Poll::Pending,
      }
    }
  }
}

const ITEM_VALUE: u64 = 42;

/// Stand-in for an in-flight request record, in the arm that embeds the slot in it.
struct ReqWithSlot {
  id: u64,
  slot: Slot<u64>,
}

/// The same record in the arms whose channel carries its own allocation.
struct Req {
  id: u64,
}

#[derive(Debug, Clone)]
struct OneshotBenchConfig {
  num_items: usize,
}

#[derive(Default, Debug)]
struct BenchContext {
  items_processed_total: usize,
}

struct OneshotAsyncState {
  _marker: (),
}

fn extract_oneshot_config(combo: &AbstractCombination) -> Result<OneshotBenchConfig, String> {
  Ok(OneshotBenchConfig {
    num_items: combo.get_u64(0)? as usize,
  })
}

fn setup_fn_oneshot_async(
  _runtime: &Runtime,
  _cfg: &OneshotBenchConfig,
) -> Pin<Box<dyn Future<Output = Result<(BenchContext, OneshotAsyncState), String>> + Send>> {
  Box::pin(async move { Ok((BenchContext::default(), OneshotAsyncState { _marker: () })) })
}

fn teardown_oneshot_async(
  _ctx: BenchContext,
  _state: OneshotAsyncState,
  _runtime: &Runtime,
  _cfg: &OneshotBenchConfig,
) -> Pin<Box<dyn Future<Output = ()> + Send>> {
  Box::pin(async move {})
}

type LogicFuture = Pin<Box<dyn Future<Output = (BenchContext, OneshotAsyncState, Duration)> + Send>>;

// --- Clonable oneshot() ---

fn logic_clonable_full(
  mut ctx: BenchContext,
  state: OneshotAsyncState,
  cfg: &OneshotBenchConfig,
) -> LogicFuture {
  let cfg = cfg.clone();
  Box::pin(async move {
    let start = Instant::now();
    for _ in 0..cfg.num_items {
      let (tx, rx) = oneshot::oneshot();
      tx.send(ITEM_VALUE).expect("send failed");
      let _ = rx.recv().await.unwrap();
    }
    let duration = start.elapsed();
    ctx.items_processed_total += cfg.num_items;
    (ctx, state, duration)
  })
}

fn logic_clonable_xfer(
  mut ctx: BenchContext,
  state: OneshotAsyncState,
  cfg: &OneshotBenchConfig,
) -> LogicFuture {
  let cfg = cfg.clone();
  Box::pin(async move {
    let mut pairs = Vec::with_capacity(cfg.num_items);
    for _ in 0..cfg.num_items {
      pairs.push(oneshot::oneshot());
    }
    let start = Instant::now();
    for (tx, rx) in pairs {
      tx.send(ITEM_VALUE).expect("send failed");
      let _ = rx.recv().await.unwrap();
    }
    let duration = start.elapsed();
    ctx.items_processed_total += cfg.num_items;
    (ctx, state, duration)
  })
}

// --- exclusive() ---

fn logic_exclusive_full(
  mut ctx: BenchContext,
  state: OneshotAsyncState,
  cfg: &OneshotBenchConfig,
) -> LogicFuture {
  let cfg = cfg.clone();
  Box::pin(async move {
    let start = Instant::now();
    for _ in 0..cfg.num_items {
      let (tx, mut rx) = oneshot::exclusive();
      tx.send(ITEM_VALUE).expect("send failed");
      let _ = rx.recv().await.unwrap();
    }
    let duration = start.elapsed();
    ctx.items_processed_total += cfg.num_items;
    (ctx, state, duration)
  })
}

fn logic_exclusive_xfer(
  mut ctx: BenchContext,
  state: OneshotAsyncState,
  cfg: &OneshotBenchConfig,
) -> LogicFuture {
  let cfg = cfg.clone();
  Box::pin(async move {
    let mut pairs = Vec::with_capacity(cfg.num_items);
    for _ in 0..cfg.num_items {
      pairs.push(oneshot::exclusive());
    }
    let start = Instant::now();
    for (tx, mut rx) in pairs {
      tx.send(ITEM_VALUE).expect("send failed");
      let _ = rx.recv().await.unwrap();
    }
    let duration = start.elapsed();
    ctx.items_processed_total += cfg.num_items;
    (ctx, state, duration)
  })
}

// --- tokio::sync::oneshot ---

fn logic_tokio_full(
  mut ctx: BenchContext,
  state: OneshotAsyncState,
  cfg: &OneshotBenchConfig,
) -> LogicFuture {
  let cfg = cfg.clone();
  Box::pin(async move {
    let start = Instant::now();
    for _ in 0..cfg.num_items {
      let (tx, rx) = tokio::sync::oneshot::channel();
      tx.send(ITEM_VALUE).expect("send failed");
      let _ = rx.await.unwrap();
    }
    let duration = start.elapsed();
    ctx.items_processed_total += cfg.num_items;
    (ctx, state, duration)
  })
}

fn logic_tokio_xfer(
  mut ctx: BenchContext,
  state: OneshotAsyncState,
  cfg: &OneshotBenchConfig,
) -> LogicFuture {
  let cfg = cfg.clone();
  Box::pin(async move {
    let mut pairs = Vec::with_capacity(cfg.num_items);
    for _ in 0..cfg.num_items {
      pairs.push(tokio::sync::oneshot::channel());
    }
    let start = Instant::now();
    for (tx, rx) in pairs {
      tx.send(ITEM_VALUE).expect("send failed");
      let _ = rx.await.unwrap();
    }
    let duration = start.elapsed();
    ctx.items_processed_total += cfg.num_items;
    (ctx, state, duration)
  })
}

// --- Carried by a request record: one allocation vs two ---

fn logic_clonable_record(
  mut ctx: BenchContext,
  state: OneshotAsyncState,
  cfg: &OneshotBenchConfig,
) -> LogicFuture {
  let cfg = cfg.clone();
  Box::pin(async move {
    let start = Instant::now();
    let mut sum = 0u64;
    for i in 0..cfg.num_items {
      let core = Arc::new(Req { id: i as u64 });
      let (tx, rx) = oneshot::oneshot();
      tx.send(core.id).expect("send failed");
      sum += rx.recv().await.unwrap();
      black_box(&core);
    }
    let duration = start.elapsed();
    black_box(sum);
    ctx.items_processed_total += cfg.num_items;
    (ctx, state, duration)
  })
}

fn logic_slot_record(
  mut ctx: BenchContext,
  state: OneshotAsyncState,
  cfg: &OneshotBenchConfig,
) -> LogicFuture {
  let cfg = cfg.clone();
  Box::pin(async move {
    let start = Instant::now();
    let mut sum = 0u64;
    for i in 0..cfg.num_items {
      let core = Arc::new(ReqWithSlot {
        id: i as u64,
        slot: Slot::new(),
      });
      core.slot.send(core.id).expect("send failed");
      sum += core.slot.recv().await.unwrap();
    }
    let duration = start.elapsed();
    black_box(sum);
    ctx.items_processed_total += cfg.num_items;
    (ctx, state, duration)
  })
}

fn logic_exclusive_record(
  mut ctx: BenchContext,
  state: OneshotAsyncState,
  cfg: &OneshotBenchConfig,
) -> LogicFuture {
  let cfg = cfg.clone();
  Box::pin(async move {
    let start = Instant::now();
    let mut sum = 0u64;
    for i in 0..cfg.num_items {
      let core = Arc::new(Req { id: i as u64 });
      let (tx, mut rx) = oneshot::exclusive();
      tx.send(core.id).expect("send failed");
      sum += rx.recv().await.unwrap();
      black_box(&core);
    }
    let duration = start.elapsed();
    black_box(sum);
    ctx.items_processed_total += cfg.num_items;
    (ctx, state, duration)
  })
}

fn logic_tokio_record(
  mut ctx: BenchContext,
  state: OneshotAsyncState,
  cfg: &OneshotBenchConfig,
) -> LogicFuture {
  let cfg = cfg.clone();
  Box::pin(async move {
    let start = Instant::now();
    let mut sum = 0u64;
    for i in 0..cfg.num_items {
      let core = Arc::new(Req { id: i as u64 });
      let (tx, rx) = tokio::sync::oneshot::channel();
      tx.send(core.id).expect("send failed");
      sum += rx.await.unwrap();
      black_box(&core);
    }
    let duration = start.elapsed();
    black_box(sum);
    ctx.items_processed_total += cfg.num_items;
    (ctx, state, duration)
  })
}

fn logic_slot_exclusive_full(
  mut ctx: BenchContext,
  state: OneshotAsyncState,
  cfg: &OneshotBenchConfig,
) -> LogicFuture {
  let cfg = cfg.clone();
  Box::pin(async move {
    let start = Instant::now();
    let mut sum = 0u64;
    for _ in 0..cfg.num_items {
      let (tx, mut rx) = slot_exclusive::channel::<u64>();
      tx.send(ITEM_VALUE).expect("send failed");
      sum += rx.recv().await.unwrap();
    }
    let duration = start.elapsed();
    black_box(sum);
    ctx.items_processed_total += cfg.num_items;
    (ctx, state, duration)
  })
}

fn logic_slot_exclusive_xfer(
  mut ctx: BenchContext,
  state: OneshotAsyncState,
  cfg: &OneshotBenchConfig,
) -> LogicFuture {
  let cfg = cfg.clone();
  Box::pin(async move {
    let mut pairs = Vec::with_capacity(cfg.num_items);
    for _ in 0..cfg.num_items {
      pairs.push(slot_exclusive::channel::<u64>());
    }
    let start = Instant::now();
    let mut sum = 0u64;
    for (tx, mut rx) in pairs {
      tx.send(ITEM_VALUE).expect("send failed");
      sum += rx.recv().await.unwrap();
    }
    let duration = start.elapsed();
    black_box(sum);
    ctx.items_processed_total += cfg.num_items;
    (ctx, state, duration)
  })
}

fn logic_slot_exclusive_record(
  mut ctx: BenchContext,
  state: OneshotAsyncState,
  cfg: &OneshotBenchConfig,
) -> LogicFuture {
  let cfg = cfg.clone();
  Box::pin(async move {
    let start = Instant::now();
    let mut sum = 0u64;
    for i in 0..cfg.num_items {
      let core = Arc::new(Req { id: i as u64 });
      let (tx, mut rx) = slot_exclusive::channel::<u64>();
      tx.send(core.id).expect("send failed");
      sum += rx.recv().await.unwrap();
      black_box(&core);
    }
    let duration = start.elapsed();
    black_box(sum);
    ctx.items_processed_total += cfg.num_items;
    (ctx, state, duration)
  })
}

fn logic_slot_full(
  mut ctx: BenchContext,
  state: OneshotAsyncState,
  cfg: &OneshotBenchConfig,
) -> LogicFuture {
  let cfg = cfg.clone();
  Box::pin(async move {
    let start = Instant::now();
    let mut sum = 0u64;
    for _ in 0..cfg.num_items {
      let slot = Arc::new(Slot::<u64>::new());
      slot.send(ITEM_VALUE).expect("send failed");
      sum += slot.recv().await.unwrap();
    }
    let duration = start.elapsed();
    black_box(sum);
    ctx.items_processed_total += cfg.num_items;
    (ctx, state, duration)
  })
}

fn logic_slot_xfer(
  mut ctx: BenchContext,
  state: OneshotAsyncState,
  cfg: &OneshotBenchConfig,
) -> LogicFuture {
  let cfg = cfg.clone();
  Box::pin(async move {
    let slots: Vec<Slot<u64>> = (0..cfg.num_items).map(|_| Slot::new()).collect();
    let start = Instant::now();
    let mut sum = 0u64;
    for s in &slots {
      s.send(ITEM_VALUE).expect("send failed");
      sum += s.recv().await.unwrap();
    }
    let duration = start.elapsed();
    black_box(sum);
    ctx.items_processed_total += cfg.num_items;
    (ctx, state, duration)
  })
}

fn logic_slot_split_full(
  mut ctx: BenchContext,
  state: OneshotAsyncState,
  cfg: &OneshotBenchConfig,
) -> LogicFuture {
  let cfg = cfg.clone();
  Box::pin(async move {
    let start = Instant::now();
    let mut sum = 0u64;
    for _ in 0..cfg.num_items {
      let slot = Slot::<u64>::new();
      let (tx, mut rx) = slot_split::pair(&slot);
      tx.send(ITEM_VALUE).expect("send failed");
      sum += rx.recv().await.unwrap();
    }
    let duration = start.elapsed();
    black_box(sum);
    ctx.items_processed_total += cfg.num_items;
    (ctx, state, duration)
  })
}

fn logic_slot_split_xfer(
  mut ctx: BenchContext,
  state: OneshotAsyncState,
  cfg: &OneshotBenchConfig,
) -> LogicFuture {
  let cfg = cfg.clone();
  Box::pin(async move {
    let slots: Vec<Slot<u64>> = (0..cfg.num_items).map(|_| Slot::new()).collect();
    let start = Instant::now();
    let mut sum = 0u64;
    for s in &slots {
      let (tx, mut rx) = slot_split::pair(s);
      tx.send(ITEM_VALUE).expect("send failed");
      sum += rx.recv().await.unwrap();
    }
    let duration = start.elapsed();
    black_box(sum);
    ctx.items_processed_total += cfg.num_items;
    (ctx, state, duration)
  })
}

fn logic_slot_split_record(
  mut ctx: BenchContext,
  state: OneshotAsyncState,
  cfg: &OneshotBenchConfig,
) -> LogicFuture {
  let cfg = cfg.clone();
  Box::pin(async move {
    let start = Instant::now();
    let mut sum = 0u64;
    for i in 0..cfg.num_items {
      let core = Arc::new(ReqWithSlot {
        id: i as u64,
        slot: Slot::new(),
      });
      let (tx, mut rx) = slot_split::pair(&core.slot);
      tx.send(core.id).expect("send failed");
      sum += rx.recv().await.unwrap();
    }
    let duration = start.elapsed();
    black_box(sum);
    ctx.items_processed_total += cfg.num_items;
    (ctx, state, duration)
  })
}

fn logic_slot_owned_full(
  mut ctx: BenchContext,
  state: OneshotAsyncState,
  cfg: &OneshotBenchConfig,
) -> LogicFuture {
  let cfg = cfg.clone();
  Box::pin(async move {
    let start = Instant::now();
    let mut sum = 0u64;
    for _ in 0..cfg.num_items {
      let (tx, mut rx) = slot_owned::channel::<u64>();
      tx.send(ITEM_VALUE).expect("send failed");
      sum += rx.recv().await.unwrap();
    }
    let duration = start.elapsed();
    black_box(sum);
    ctx.items_processed_total += cfg.num_items;
    (ctx, state, duration)
  })
}

fn logic_slot_owned_xfer(
  mut ctx: BenchContext,
  state: OneshotAsyncState,
  cfg: &OneshotBenchConfig,
) -> LogicFuture {
  let cfg = cfg.clone();
  Box::pin(async move {
    let mut pairs = Vec::with_capacity(cfg.num_items);
    for _ in 0..cfg.num_items {
      pairs.push(slot_owned::channel::<u64>());
    }
    let start = Instant::now();
    let mut sum = 0u64;
    for (tx, mut rx) in pairs {
      tx.send(ITEM_VALUE).expect("send failed");
      sum += rx.recv().await.unwrap();
    }
    let duration = start.elapsed();
    black_box(sum);
    ctx.items_processed_total += cfg.num_items;
    (ctx, state, duration)
  })
}

fn logic_slot_owned_record(
  mut ctx: BenchContext,
  state: OneshotAsyncState,
  cfg: &OneshotBenchConfig,
) -> LogicFuture {
  let cfg = cfg.clone();
  Box::pin(async move {
    let start = Instant::now();
    let mut sum = 0u64;
    for i in 0..cfg.num_items {
      let id = i as u64;
      let (tx, mut rx) = slot_owned::pair_boxed(
        Box::new(ReqWithSlot {
          id,
          slot: Slot::new(),
        }),
        |r| &r.slot,
      );
      tx.send(id).expect("send failed");
      sum += rx.recv().await.unwrap();
    }
    let duration = start.elapsed();
    black_box(sum);
    ctx.items_processed_total += cfg.num_items;
    (ctx, state, duration)
  })
}

fn logic_slot_pooled_host_record(
  mut ctx: BenchContext,
  state: OneshotAsyncState,
  cfg: &OneshotBenchConfig,
) -> LogicFuture {
  let cfg = cfg.clone();
  Box::pin(async move {
    let pool = slot_pooled::HostPool::<ReqWithSlot, u64>::new(
      64,
      || ReqWithSlot {
        id: 0,
        slot: Slot::new(),
      },
      |r| &r.slot,
    );
    let start = Instant::now();
    let mut sum = 0u64;
    for i in 0..cfg.num_items {
      let (tx, mut rx) = pool.pair_init(|r| r.id = i as u64);
      tx.send(rx.host().id).expect("send failed");
      sum += rx.recv().await.unwrap();
    }
    let duration = start.elapsed();
    black_box(sum);
    ctx.items_processed_total += cfg.num_items;
    (ctx, state, duration)
  })
}

fn logic_slot_pooled_full(
  mut ctx: BenchContext,
  state: OneshotAsyncState,
  cfg: &OneshotBenchConfig,
) -> LogicFuture {
  let cfg = cfg.clone();
  Box::pin(async move {
    let pool = slot_pooled::Pool::<u64>::new(64);
    let start = Instant::now();
    let mut sum = 0u64;
    for _ in 0..cfg.num_items {
      let (tx, mut rx) = pool.pair();
      tx.send(ITEM_VALUE).expect("send failed");
      sum += rx.recv().await.unwrap();
    }
    let duration = start.elapsed();
    black_box(sum);
    ctx.items_processed_total += cfg.num_items;
    (ctx, state, duration)
  })
}

fn logic_slot_pooled_xfer(
  mut ctx: BenchContext,
  state: OneshotAsyncState,
  cfg: &OneshotBenchConfig,
) -> LogicFuture {
  let cfg = cfg.clone();
  Box::pin(async move {
    let pool = slot_pooled::Pool::<u64>::new(cfg.num_items + 4);
    let mut pairs = Vec::with_capacity(cfg.num_items);
    for _ in 0..cfg.num_items {
      pairs.push(pool.pair());
    }
    let start = Instant::now();
    let mut sum = 0u64;
    for (tx, mut rx) in pairs {
      tx.send(ITEM_VALUE).expect("send failed");
      sum += rx.recv().await.unwrap();
    }
    let duration = start.elapsed();
    black_box(sum);
    ctx.items_processed_total += cfg.num_items;
    (ctx, state, duration)
  })
}

fn logic_slot_pooled_record(
  mut ctx: BenchContext,
  state: OneshotAsyncState,
  cfg: &OneshotBenchConfig,
) -> LogicFuture {
  let cfg = cfg.clone();
  Box::pin(async move {
    let pool = slot_pooled::Pool::<u64>::new(64);
    let start = Instant::now();
    let mut sum = 0u64;
    for i in 0..cfg.num_items {
      let core = Arc::new(Req { id: i as u64 });
      let (tx, mut rx) = pool.pair();
      tx.send(core.id).expect("send failed");
      sum += rx.recv().await.unwrap();
      black_box(&core);
    }
    let duration = start.elapsed();
    black_box(sum);
    ctx.items_processed_total += cfg.num_items;
    (ctx, state, duration)
  })
}

// --- Two-thread handoff: the receiver can actually park ---

fn logic_clonable_handoff(
  mut ctx: BenchContext,
  state: OneshotAsyncState,
  cfg: &OneshotBenchConfig,
) -> LogicFuture {
  let cfg = cfg.clone();
  Box::pin(async move {
    let mut senders = Vec::with_capacity(cfg.num_items);
    let mut receivers = Vec::with_capacity(cfg.num_items);
    for _ in 0..cfg.num_items {
      let (tx, rx) = oneshot::oneshot::<u64>();
      senders.push(tx);
      receivers.push(rx);
    }
    let barrier = Arc::new(Barrier::new(2));

    let sender = {
      let barrier = Arc::clone(&barrier);
      thread::spawn(move || {
        barrier.wait();
        for tx in senders {
          tx.send(ITEM_VALUE).expect("send failed");
        }
      })
    };

    barrier.wait();
    let start = Instant::now();
    let mut sum = 0u64;
    for rx in receivers {
      sum += rx.recv().await.unwrap();
    }
    let duration = start.elapsed();

    sender.join().expect("clonable sender thread panicked");
    black_box(sum);
    ctx.items_processed_total += cfg.num_items;
    (ctx, state, duration)
  })
}

fn logic_slot_exclusive_handoff(
  mut ctx: BenchContext,
  state: OneshotAsyncState,
  cfg: &OneshotBenchConfig,
) -> LogicFuture {
  let cfg = cfg.clone();
  Box::pin(async move {
    let mut senders = Vec::with_capacity(cfg.num_items);
    let mut receivers = Vec::with_capacity(cfg.num_items);
    for _ in 0..cfg.num_items {
      let (tx, rx) = slot_exclusive::channel::<u64>();
      senders.push(tx);
      receivers.push(rx);
    }
    let barrier = Arc::new(Barrier::new(2));

    let sender = {
      let barrier = Arc::clone(&barrier);
      thread::spawn(move || {
        barrier.wait();
        for tx in senders {
          tx.send(ITEM_VALUE).expect("send failed");
        }
      })
    };

    barrier.wait();
    let start = Instant::now();
    let mut sum = 0u64;
    for mut rx in receivers {
      sum += rx.recv().await.unwrap();
    }
    let duration = start.elapsed();

    sender.join().expect("slot-exclusive sender thread panicked");
    black_box(sum);
    ctx.items_processed_total += cfg.num_items;
    (ctx, state, duration)
  })
}

fn logic_slot_handoff(
  mut ctx: BenchContext,
  state: OneshotAsyncState,
  cfg: &OneshotBenchConfig,
) -> LogicFuture {
  let cfg = cfg.clone();
  Box::pin(async move {
    let slots: Arc<Vec<Slot<u64>>> = Arc::new((0..cfg.num_items).map(|_| Slot::new()).collect());
    let barrier = Arc::new(Barrier::new(2));

    let sender = {
      let slots = Arc::clone(&slots);
      let barrier = Arc::clone(&barrier);
      thread::spawn(move || {
        barrier.wait();
        for s in slots.iter() {
          s.send(ITEM_VALUE).expect("send failed");
        }
      })
    };

    barrier.wait();
    let start = Instant::now();
    let mut sum = 0u64;
    for s in slots.iter() {
      sum += s.recv().await.unwrap();
    }
    let duration = start.elapsed();

    sender.join().expect("slot sender thread panicked");
    black_box(sum);
    ctx.items_processed_total += cfg.num_items;
    (ctx, state, duration)
  })
}

fn logic_slot_split_handoff(
  mut ctx: BenchContext,
  state: OneshotAsyncState,
  cfg: &OneshotBenchConfig,
) -> LogicFuture {
  let cfg = cfg.clone();
  Box::pin(async move {
    // Leaked so the borrowed handles are 'static and can cross into thread::spawn,
    // keeping the thread/barrier/timing shape identical to the other handoff arms;
    // reclaimed after the join.
    let slots: &'static [Slot<u64>] = Box::leak(
      (0..cfg.num_items)
        .map(|_| Slot::new())
        .collect::<Vec<_>>()
        .into_boxed_slice(),
    );
    let mut senders = Vec::with_capacity(cfg.num_items);
    let mut receivers = Vec::with_capacity(cfg.num_items);
    for s in slots {
      let (tx, rx) = slot_split::pair(s);
      senders.push(tx);
      receivers.push(rx);
    }
    let barrier = Arc::new(Barrier::new(2));

    let sender = {
      let barrier = Arc::clone(&barrier);
      thread::spawn(move || {
        barrier.wait();
        for tx in senders {
          tx.send(ITEM_VALUE).expect("send failed");
        }
      })
    };

    barrier.wait();
    let start = Instant::now();
    let mut sum = 0u64;
    for mut rx in receivers {
      sum += rx.recv().await.unwrap();
    }
    let duration = start.elapsed();

    sender.join().expect("slot-split sender thread panicked");
    // Safety: the sender thread joined and every handle was consumed above, so no
    // borrow of the leaked storage remains.
    unsafe { drop(Box::from_raw(slots as *const [Slot<u64>] as *mut [Slot<u64>])) };
    black_box(sum);
    ctx.items_processed_total += cfg.num_items;
    (ctx, state, duration)
  })
}

fn logic_slot_owned_handoff(
  mut ctx: BenchContext,
  state: OneshotAsyncState,
  cfg: &OneshotBenchConfig,
) -> LogicFuture {
  let cfg = cfg.clone();
  Box::pin(async move {
    let mut senders = Vec::with_capacity(cfg.num_items);
    let mut receivers = Vec::with_capacity(cfg.num_items);
    for _ in 0..cfg.num_items {
      let (tx, rx) = slot_owned::channel::<u64>();
      senders.push(tx);
      receivers.push(rx);
    }
    let barrier = Arc::new(Barrier::new(2));

    let sender = {
      let barrier = Arc::clone(&barrier);
      thread::spawn(move || {
        barrier.wait();
        for tx in senders {
          tx.send(ITEM_VALUE).expect("send failed");
        }
      })
    };

    barrier.wait();
    let start = Instant::now();
    let mut sum = 0u64;
    for mut rx in receivers {
      sum += rx.recv().await.unwrap();
    }
    let duration = start.elapsed();

    sender.join().expect("slot-owned sender thread panicked");
    black_box(sum);
    ctx.items_processed_total += cfg.num_items;
    (ctx, state, duration)
  })
}

fn logic_slot_pooled_handoff(
  mut ctx: BenchContext,
  state: OneshotAsyncState,
  cfg: &OneshotBenchConfig,
) -> LogicFuture {
  let cfg = cfg.clone();
  Box::pin(async move {
    let pool = slot_pooled::Pool::<u64>::new(cfg.num_items + 4);
    let mut senders = Vec::with_capacity(cfg.num_items);
    let mut receivers = Vec::with_capacity(cfg.num_items);
    for _ in 0..cfg.num_items {
      let (tx, rx) = pool.pair();
      senders.push(tx);
      receivers.push(rx);
    }
    let barrier = Arc::new(Barrier::new(2));

    let sender = {
      let barrier = Arc::clone(&barrier);
      thread::spawn(move || {
        barrier.wait();
        for tx in senders {
          tx.send(ITEM_VALUE).expect("send failed");
        }
      })
    };

    barrier.wait();
    let start = Instant::now();
    let mut sum = 0u64;
    for mut rx in receivers {
      sum += rx.recv().await.unwrap();
    }
    let duration = start.elapsed();

    sender.join().expect("slot-pooled sender thread panicked");
    black_box(sum);
    ctx.items_processed_total += cfg.num_items;
    (ctx, state, duration)
  })
}

fn logic_exclusive_handoff(
  mut ctx: BenchContext,
  state: OneshotAsyncState,
  cfg: &OneshotBenchConfig,
) -> LogicFuture {
  let cfg = cfg.clone();
  Box::pin(async move {
    let mut senders = Vec::with_capacity(cfg.num_items);
    let mut receivers = Vec::with_capacity(cfg.num_items);
    for _ in 0..cfg.num_items {
      let (tx, rx) = oneshot::exclusive::<u64>();
      senders.push(tx);
      receivers.push(rx);
    }
    let barrier = Arc::new(Barrier::new(2));

    let sender = {
      let barrier = Arc::clone(&barrier);
      thread::spawn(move || {
        barrier.wait();
        for tx in senders {
          tx.send(ITEM_VALUE).expect("send failed");
        }
      })
    };

    barrier.wait();
    let start = Instant::now();
    let mut sum = 0u64;
    for mut rx in receivers {
      sum += rx.recv().await.unwrap();
    }
    let duration = start.elapsed();

    sender.join().expect("exclusive sender thread panicked");
    black_box(sum);
    ctx.items_processed_total += cfg.num_items;
    (ctx, state, duration)
  })
}

fn logic_tokio_handoff(
  mut ctx: BenchContext,
  state: OneshotAsyncState,
  cfg: &OneshotBenchConfig,
) -> LogicFuture {
  let cfg = cfg.clone();
  Box::pin(async move {
    let mut senders = Vec::with_capacity(cfg.num_items);
    let mut receivers = Vec::with_capacity(cfg.num_items);
    for _ in 0..cfg.num_items {
      let (tx, rx) = tokio::sync::oneshot::channel::<u64>();
      senders.push(tx);
      receivers.push(rx);
    }
    let barrier = Arc::new(Barrier::new(2));

    let sender = {
      let barrier = Arc::clone(&barrier);
      thread::spawn(move || {
        barrier.wait();
        for tx in senders {
          tx.send(ITEM_VALUE).expect("send failed");
        }
      })
    };

    barrier.wait();
    let start = Instant::now();
    let mut sum = 0u64;
    for rx in receivers {
      sum += rx.await.unwrap();
    }
    let duration = start.elapsed();

    sender.join().expect("tokio sender thread panicked");
    black_box(sum);
    ctx.items_processed_total += cfg.num_items;
    (ctx, state, duration)
  })
}

// --- Ping-pong: each side must wait on the other, so both park every round ---

fn logic_clonable_pingpong(
  mut ctx: BenchContext,
  state: OneshotAsyncState,
  cfg: &OneshotBenchConfig,
) -> LogicFuture {
  let cfg = cfg.clone();
  Box::pin(async move {
    let n = cfg.num_items;
    let (mut in_tx, mut in_rx) = (Vec::with_capacity(n), Vec::with_capacity(n));
    let (mut out_tx, mut out_rx) = (Vec::with_capacity(n), Vec::with_capacity(n));
    for _ in 0..n {
      let (tx, rx) = oneshot::oneshot::<u64>();
      in_tx.push(tx);
      in_rx.push(rx);
      let (tx, rx) = oneshot::oneshot::<u64>();
      out_tx.push(tx);
      out_rx.push(rx);
    }
    let barrier = Arc::new(Barrier::new(2));

    let peer = {
      let barrier = Arc::clone(&barrier);
      thread::spawn(move || {
        barrier.wait();
        for (tx, rx) in in_tx.into_iter().zip(out_rx) {
          tx.send(ITEM_VALUE).expect("send failed");
          futures_executor::block_on(rx.recv()).unwrap();
        }
      })
    };

    barrier.wait();
    let start = Instant::now();
    let mut sum = 0u64;
    for (rx, tx) in in_rx.into_iter().zip(out_tx) {
      sum += rx.recv().await.unwrap();
      tx.send(ITEM_VALUE).expect("send failed");
    }
    let duration = start.elapsed();

    peer.join().expect("clonable ping-pong peer thread panicked");
    black_box(sum);
    ctx.items_processed_total += n;
    (ctx, state, duration)
  })
}

fn logic_slot_exclusive_pingpong(
  mut ctx: BenchContext,
  state: OneshotAsyncState,
  cfg: &OneshotBenchConfig,
) -> LogicFuture {
  let cfg = cfg.clone();
  Box::pin(async move {
    let n = cfg.num_items;
    let (mut in_tx, mut in_rx) = (Vec::with_capacity(n), Vec::with_capacity(n));
    let (mut out_tx, mut out_rx) = (Vec::with_capacity(n), Vec::with_capacity(n));
    for _ in 0..n {
      let (tx, rx) = slot_exclusive::channel::<u64>();
      in_tx.push(tx);
      in_rx.push(rx);
      let (tx, rx) = slot_exclusive::channel::<u64>();
      out_tx.push(tx);
      out_rx.push(rx);
    }
    let barrier = Arc::new(Barrier::new(2));

    let peer = {
      let barrier = Arc::clone(&barrier);
      thread::spawn(move || {
        barrier.wait();
        for (tx, mut rx) in in_tx.into_iter().zip(out_rx) {
          tx.send(ITEM_VALUE).expect("send failed");
          futures_executor::block_on(rx.recv()).unwrap();
        }
      })
    };

    barrier.wait();
    let start = Instant::now();
    let mut sum = 0u64;
    for (mut rx, tx) in in_rx.into_iter().zip(out_tx) {
      sum += rx.recv().await.unwrap();
      tx.send(ITEM_VALUE).expect("send failed");
    }
    let duration = start.elapsed();

    peer.join().expect("slot-exclusive ping-pong peer thread panicked");
    black_box(sum);
    ctx.items_processed_total += n;
    (ctx, state, duration)
  })
}

fn logic_slot_pingpong(
  mut ctx: BenchContext,
  state: OneshotAsyncState,
  cfg: &OneshotBenchConfig,
) -> LogicFuture {
  let cfg = cfg.clone();
  Box::pin(async move {
    let n = cfg.num_items;
    let inbound: Arc<Vec<Slot<u64>>> = Arc::new((0..n).map(|_| Slot::new()).collect());
    let outbound: Arc<Vec<Slot<u64>>> = Arc::new((0..n).map(|_| Slot::new()).collect());
    let barrier = Arc::new(Barrier::new(2));

    let peer = {
      let inbound = Arc::clone(&inbound);
      let outbound = Arc::clone(&outbound);
      let barrier = Arc::clone(&barrier);
      thread::spawn(move || {
        barrier.wait();
        for i in 0..n {
          inbound[i].send(ITEM_VALUE).expect("send failed");
          futures_executor::block_on(outbound[i].recv()).unwrap();
        }
      })
    };

    barrier.wait();
    let start = Instant::now();
    let mut sum = 0u64;
    for i in 0..n {
      sum += inbound[i].recv().await.unwrap();
      outbound[i].send(ITEM_VALUE).expect("send failed");
    }
    let duration = start.elapsed();

    peer.join().expect("slot ping-pong peer thread panicked");
    black_box(sum);
    ctx.items_processed_total += n;
    (ctx, state, duration)
  })
}

fn logic_slot_split_pingpong(
  mut ctx: BenchContext,
  state: OneshotAsyncState,
  cfg: &OneshotBenchConfig,
) -> LogicFuture {
  let cfg = cfg.clone();
  Box::pin(async move {
    let n = cfg.num_items;
    // Leaked for the same reason as logic_slot_split_handoff; reclaimed after the join.
    let inbound: &'static [Slot<u64>] =
      Box::leak((0..n).map(|_| Slot::new()).collect::<Vec<_>>().into_boxed_slice());
    let outbound: &'static [Slot<u64>] =
      Box::leak((0..n).map(|_| Slot::new()).collect::<Vec<_>>().into_boxed_slice());
    let (mut in_tx, mut in_rx) = (Vec::with_capacity(n), Vec::with_capacity(n));
    let (mut out_tx, mut out_rx) = (Vec::with_capacity(n), Vec::with_capacity(n));
    for s in inbound {
      let (tx, rx) = slot_split::pair(s);
      in_tx.push(tx);
      in_rx.push(rx);
    }
    for s in outbound {
      let (tx, rx) = slot_split::pair(s);
      out_tx.push(tx);
      out_rx.push(rx);
    }
    let barrier = Arc::new(Barrier::new(2));

    let peer = {
      let barrier = Arc::clone(&barrier);
      thread::spawn(move || {
        barrier.wait();
        for (tx, mut rx) in in_tx.into_iter().zip(out_rx) {
          tx.send(ITEM_VALUE).expect("send failed");
          futures_executor::block_on(rx.recv()).unwrap();
        }
      })
    };

    barrier.wait();
    let start = Instant::now();
    let mut sum = 0u64;
    for (mut rx, tx) in in_rx.into_iter().zip(out_tx) {
      sum += rx.recv().await.unwrap();
      tx.send(ITEM_VALUE).expect("send failed");
    }
    let duration = start.elapsed();

    peer.join().expect("slot-split ping-pong peer thread panicked");
    // Safety: the peer thread joined and every handle was consumed above, so no
    // borrow of the leaked storage remains.
    unsafe {
      drop(Box::from_raw(inbound as *const [Slot<u64>] as *mut [Slot<u64>]));
      drop(Box::from_raw(outbound as *const [Slot<u64>] as *mut [Slot<u64>]));
    }
    black_box(sum);
    ctx.items_processed_total += n;
    (ctx, state, duration)
  })
}

fn logic_slot_owned_pingpong(
  mut ctx: BenchContext,
  state: OneshotAsyncState,
  cfg: &OneshotBenchConfig,
) -> LogicFuture {
  let cfg = cfg.clone();
  Box::pin(async move {
    let n = cfg.num_items;
    let (mut in_tx, mut in_rx) = (Vec::with_capacity(n), Vec::with_capacity(n));
    let (mut out_tx, mut out_rx) = (Vec::with_capacity(n), Vec::with_capacity(n));
    for _ in 0..n {
      let (tx, rx) = slot_owned::channel::<u64>();
      in_tx.push(tx);
      in_rx.push(rx);
      let (tx, rx) = slot_owned::channel::<u64>();
      out_tx.push(tx);
      out_rx.push(rx);
    }
    let barrier = Arc::new(Barrier::new(2));

    let peer = {
      let barrier = Arc::clone(&barrier);
      thread::spawn(move || {
        barrier.wait();
        for (tx, mut rx) in in_tx.into_iter().zip(out_rx) {
          tx.send(ITEM_VALUE).expect("send failed");
          futures_executor::block_on(rx.recv()).unwrap();
        }
      })
    };

    barrier.wait();
    let start = Instant::now();
    let mut sum = 0u64;
    for (mut rx, tx) in in_rx.into_iter().zip(out_tx) {
      sum += rx.recv().await.unwrap();
      tx.send(ITEM_VALUE).expect("send failed");
    }
    let duration = start.elapsed();

    peer.join().expect("slot-owned ping-pong peer thread panicked");
    black_box(sum);
    ctx.items_processed_total += n;
    (ctx, state, duration)
  })
}

fn logic_slot_pooled_pingpong(
  mut ctx: BenchContext,
  state: OneshotAsyncState,
  cfg: &OneshotBenchConfig,
) -> LogicFuture {
  let cfg = cfg.clone();
  Box::pin(async move {
    let n = cfg.num_items;
    let pool = slot_pooled::Pool::<u64>::new(n * 2 + 4);
    let (mut in_tx, mut in_rx) = (Vec::with_capacity(n), Vec::with_capacity(n));
    let (mut out_tx, mut out_rx) = (Vec::with_capacity(n), Vec::with_capacity(n));
    for _ in 0..n {
      let (tx, rx) = pool.pair();
      in_tx.push(tx);
      in_rx.push(rx);
      let (tx, rx) = pool.pair();
      out_tx.push(tx);
      out_rx.push(rx);
    }
    let barrier = Arc::new(Barrier::new(2));

    let peer = {
      let barrier = Arc::clone(&barrier);
      thread::spawn(move || {
        barrier.wait();
        for (tx, mut rx) in in_tx.into_iter().zip(out_rx) {
          tx.send(ITEM_VALUE).expect("send failed");
          futures_executor::block_on(rx.recv()).unwrap();
        }
      })
    };

    barrier.wait();
    let start = Instant::now();
    let mut sum = 0u64;
    for (mut rx, tx) in in_rx.into_iter().zip(out_tx) {
      sum += rx.recv().await.unwrap();
      tx.send(ITEM_VALUE).expect("send failed");
    }
    let duration = start.elapsed();

    peer.join().expect("slot-pooled ping-pong peer thread panicked");
    black_box(sum);
    ctx.items_processed_total += n;
    (ctx, state, duration)
  })
}

fn logic_exclusive_pingpong(
  mut ctx: BenchContext,
  state: OneshotAsyncState,
  cfg: &OneshotBenchConfig,
) -> LogicFuture {
  let cfg = cfg.clone();
  Box::pin(async move {
    let n = cfg.num_items;
    let (mut in_tx, mut in_rx) = (Vec::with_capacity(n), Vec::with_capacity(n));
    let (mut out_tx, mut out_rx) = (Vec::with_capacity(n), Vec::with_capacity(n));
    for _ in 0..n {
      let (tx, rx) = oneshot::exclusive::<u64>();
      in_tx.push(tx);
      in_rx.push(rx);
      let (tx, rx) = oneshot::exclusive::<u64>();
      out_tx.push(tx);
      out_rx.push(rx);
    }
    let barrier = Arc::new(Barrier::new(2));

    let peer = {
      let barrier = Arc::clone(&barrier);
      thread::spawn(move || {
        barrier.wait();
        for (tx, mut rx) in in_tx.into_iter().zip(out_rx) {
          tx.send(ITEM_VALUE).expect("send failed");
          futures_executor::block_on(rx.recv()).unwrap();
        }
      })
    };

    barrier.wait();
    let start = Instant::now();
    let mut sum = 0u64;
    for (mut rx, tx) in in_rx.into_iter().zip(out_tx) {
      sum += rx.recv().await.unwrap();
      tx.send(ITEM_VALUE).expect("send failed");
    }
    let duration = start.elapsed();

    peer.join().expect("exclusive ping-pong peer thread panicked");
    black_box(sum);
    ctx.items_processed_total += n;
    (ctx, state, duration)
  })
}

fn logic_tokio_pingpong(
  mut ctx: BenchContext,
  state: OneshotAsyncState,
  cfg: &OneshotBenchConfig,
) -> LogicFuture {
  let cfg = cfg.clone();
  Box::pin(async move {
    let n = cfg.num_items;
    let (mut in_tx, mut in_rx) = (Vec::with_capacity(n), Vec::with_capacity(n));
    let (mut out_tx, mut out_rx) = (Vec::with_capacity(n), Vec::with_capacity(n));
    for _ in 0..n {
      let (tx, rx) = tokio::sync::oneshot::channel::<u64>();
      in_tx.push(tx);
      in_rx.push(rx);
      let (tx, rx) = tokio::sync::oneshot::channel::<u64>();
      out_tx.push(tx);
      out_rx.push(rx);
    }
    let barrier = Arc::new(Barrier::new(2));

    let peer = {
      let barrier = Arc::clone(&barrier);
      thread::spawn(move || {
        barrier.wait();
        for (tx, rx) in in_tx.into_iter().zip(out_rx) {
          tx.send(ITEM_VALUE).expect("send failed");
          futures_executor::block_on(rx).unwrap();
        }
      })
    };

    barrier.wait();
    let start = Instant::now();
    let mut sum = 0u64;
    for (rx, tx) in in_rx.into_iter().zip(out_tx) {
      sum += rx.await.unwrap();
      tx.send(ITEM_VALUE).expect("send failed");
    }
    let duration = start.elapsed();

    peer.join().expect("tokio ping-pong peer thread panicked");
    black_box(sum);
    ctx.items_processed_total += n;
    (ctx, state, duration)
  })
}

fn run_suite(
  c: &mut Criterion,
  rt: &Runtime,
  name: &str,
  logic: fn(BenchContext, OneshotAsyncState, &OneshotBenchConfig) -> LogicFuture,
  ops: &[u64],
) {
  let parameter_axes: Vec<Vec<MatrixCellValue>> =
    vec![ops.iter().copied().map(MatrixCellValue::Unsigned).collect()];
  let parameter_names = vec!["Ops".to_string()];

  AsyncBenchmarkSuite::new(
    c,
    rt,
    name.to_string(),
    Some(parameter_names),
    parameter_axes,
    Box::new(extract_oneshot_config),
    setup_fn_oneshot_async,
    logic,
    teardown_oneshot_async,
  )
  .throughput(|cfg: &OneshotBenchConfig| Throughput::Elements(cfg.num_items as u64))
  .run();
}

/// Reports what fraction of the handoff receives actually parked, so the handoff numbers
/// can be read. Gated on `ONESHOT_PARK_DIAG` because the counting wrapper allocates per
/// receive; it never runs inside a timed region.
fn report_parked_fraction(rt: &Runtime) {
  const OPS: usize = 10_000;

  async fn counting<F: Future>(fut: F, parks: &AtomicUsize) -> F::Output {
    let mut fut = Box::pin(fut);
    std::future::poll_fn(|cx| {
      let polled = fut.as_mut().poll(cx);
      if polled.is_pending() {
        parks.fetch_add(1, Ordering::Relaxed);
      }
      polled
    })
    .await
  }

  let parks = AtomicUsize::new(0);
  rt.block_on(async {
    let slots: Arc<Vec<Slot<u64>>> = Arc::new((0..OPS).map(|_| Slot::new()).collect());
    let barrier = Arc::new(Barrier::new(2));
    let sender = {
      let slots = Arc::clone(&slots);
      let barrier = Arc::clone(&barrier);
      thread::spawn(move || {
        barrier.wait();
        for s in slots.iter() {
          s.send(ITEM_VALUE).expect("send failed");
        }
      })
    };
    barrier.wait();
    for s in slots.iter() {
      counting(s.recv(), &parks).await.unwrap();
    }
    sender.join().unwrap();
  });
  eprintln!(
    "OneshotSlotHandoff: {}/{} receives parked",
    parks.load(Ordering::Relaxed),
    OPS
  );

  let parks = AtomicUsize::new(0);
  rt.block_on(async {
    let mut senders = Vec::with_capacity(OPS);
    let mut receivers = Vec::with_capacity(OPS);
    for _ in 0..OPS {
      let (tx, rx) = slot_exclusive::channel::<u64>();
      senders.push(tx);
      receivers.push(rx);
    }
    let barrier = Arc::new(Barrier::new(2));
    let sender = {
      let barrier = Arc::clone(&barrier);
      thread::spawn(move || {
        barrier.wait();
        for tx in senders {
          tx.send(ITEM_VALUE).expect("send failed");
        }
      })
    };
    barrier.wait();
    for mut rx in receivers {
      counting(rx.recv(), &parks).await.unwrap();
    }
    sender.join().unwrap();
  });
  eprintln!(
    "OneshotSlotExclusiveHandoff: {}/{} receives parked",
    parks.load(Ordering::Relaxed),
    OPS
  );

  let parks = AtomicUsize::new(0);
  rt.block_on(async {
    let slots: &'static [Slot<u64>] = Box::leak(
      (0..OPS)
        .map(|_| Slot::new())
        .collect::<Vec<_>>()
        .into_boxed_slice(),
    );
    let mut senders = Vec::with_capacity(OPS);
    let mut receivers = Vec::with_capacity(OPS);
    for s in slots {
      let (tx, rx) = slot_split::pair(s);
      senders.push(tx);
      receivers.push(rx);
    }
    let barrier = Arc::new(Barrier::new(2));
    let sender = {
      let barrier = Arc::clone(&barrier);
      thread::spawn(move || {
        barrier.wait();
        for tx in senders {
          tx.send(ITEM_VALUE).expect("send failed");
        }
      })
    };
    barrier.wait();
    for mut rx in receivers {
      counting(rx.recv(), &parks).await.unwrap();
    }
    sender.join().unwrap();
    unsafe { drop(Box::from_raw(slots as *const [Slot<u64>] as *mut [Slot<u64>])) };
  });
  eprintln!(
    "OneshotSlotSplitHandoff: {}/{} receives parked",
    parks.load(Ordering::Relaxed),
    OPS
  );

  let parks = AtomicUsize::new(0);
  rt.block_on(async {
    let mut senders = Vec::with_capacity(OPS);
    let mut receivers = Vec::with_capacity(OPS);
    for _ in 0..OPS {
      let (tx, rx) = slot_owned::channel::<u64>();
      senders.push(tx);
      receivers.push(rx);
    }
    let barrier = Arc::new(Barrier::new(2));
    let sender = {
      let barrier = Arc::clone(&barrier);
      thread::spawn(move || {
        barrier.wait();
        for tx in senders {
          tx.send(ITEM_VALUE).expect("send failed");
        }
      })
    };
    barrier.wait();
    for mut rx in receivers {
      counting(rx.recv(), &parks).await.unwrap();
    }
    sender.join().unwrap();
  });
  eprintln!(
    "OneshotSlotOwnedHandoff: {}/{} receives parked",
    parks.load(Ordering::Relaxed),
    OPS
  );

  let parks = AtomicUsize::new(0);
  rt.block_on(async {
    let pool = slot_pooled::Pool::<u64>::new(OPS + 4);
    let mut senders = Vec::with_capacity(OPS);
    let mut receivers = Vec::with_capacity(OPS);
    for _ in 0..OPS {
      let (tx, rx) = pool.pair();
      senders.push(tx);
      receivers.push(rx);
    }
    let barrier = Arc::new(Barrier::new(2));
    let sender = {
      let barrier = Arc::clone(&barrier);
      thread::spawn(move || {
        barrier.wait();
        for tx in senders {
          tx.send(ITEM_VALUE).expect("send failed");
        }
      })
    };
    barrier.wait();
    for mut rx in receivers {
      counting(rx.recv(), &parks).await.unwrap();
    }
    sender.join().unwrap();
  });
  eprintln!(
    "OneshotSlotPooledHandoff: {}/{} receives parked",
    parks.load(Ordering::Relaxed),
    OPS
  );

  let parks = AtomicUsize::new(0);
  rt.block_on(async {
    let mut senders = Vec::with_capacity(OPS);
    let mut receivers = Vec::with_capacity(OPS);
    for _ in 0..OPS {
      let (tx, rx) = oneshot::exclusive::<u64>();
      senders.push(tx);
      receivers.push(rx);
    }
    let barrier = Arc::new(Barrier::new(2));
    let sender = {
      let barrier = Arc::clone(&barrier);
      thread::spawn(move || {
        barrier.wait();
        for tx in senders {
          tx.send(ITEM_VALUE).expect("send failed");
        }
      })
    };
    barrier.wait();
    for mut rx in receivers {
      counting(rx.recv(), &parks).await.unwrap();
    }
    sender.join().unwrap();
  });
  eprintln!(
    "OneshotExclusiveHandoff: {}/{} receives parked",
    parks.load(Ordering::Relaxed),
    OPS
  );

  let parks = AtomicUsize::new(0);
  rt.block_on(async {
    let mut senders = Vec::with_capacity(OPS);
    let mut receivers = Vec::with_capacity(OPS);
    for _ in 0..OPS {
      let (tx, rx) = tokio::sync::oneshot::channel::<u64>();
      senders.push(tx);
      receivers.push(rx);
    }
    let barrier = Arc::new(Barrier::new(2));
    let sender = {
      let barrier = Arc::clone(&barrier);
      thread::spawn(move || {
        barrier.wait();
        for tx in senders {
          tx.send(ITEM_VALUE).expect("send failed");
        }
      })
    };
    barrier.wait();
    for rx in receivers {
      counting(rx, &parks).await.unwrap();
    }
    sender.join().unwrap();
  });
  eprintln!(
    "OneshotTokioHandoff: {}/{} receives parked",
    parks.load(Ordering::Relaxed),
    OPS
  );

  let parks = AtomicUsize::new(0);
  rt.block_on(async {
    let mut senders = Vec::with_capacity(OPS);
    let mut receivers = Vec::with_capacity(OPS);
    for _ in 0..OPS {
      let (tx, rx) = oneshot::oneshot::<u64>();
      senders.push(tx);
      receivers.push(rx);
    }
    let barrier = Arc::new(Barrier::new(2));
    let sender = {
      let barrier = Arc::clone(&barrier);
      thread::spawn(move || {
        barrier.wait();
        for tx in senders {
          tx.send(ITEM_VALUE).expect("send failed");
        }
      })
    };
    barrier.wait();
    for rx in receivers {
      counting(rx.recv(), &parks).await.unwrap();
    }
    sender.join().unwrap();
  });
  eprintln!(
    "OneshotHandoff: {}/{} receives parked",
    parks.load(Ordering::Relaxed),
    OPS
  );

  let parks = AtomicUsize::new(0);
  rt.block_on(async {
    let (mut in_tx, mut in_rx) = (Vec::with_capacity(OPS), Vec::with_capacity(OPS));
    let (mut out_tx, mut out_rx) = (Vec::with_capacity(OPS), Vec::with_capacity(OPS));
    for _ in 0..OPS {
      let (tx, rx) = oneshot::oneshot::<u64>();
      in_tx.push(tx);
      in_rx.push(rx);
      let (tx, rx) = oneshot::oneshot::<u64>();
      out_tx.push(tx);
      out_rx.push(rx);
    }
    let barrier = Arc::new(Barrier::new(2));
    let peer = {
      let barrier = Arc::clone(&barrier);
      thread::spawn(move || {
        barrier.wait();
        for (tx, rx) in in_tx.into_iter().zip(out_rx) {
          tx.send(ITEM_VALUE).expect("send failed");
          futures_executor::block_on(rx.recv()).unwrap();
        }
      })
    };
    barrier.wait();
    for (rx, tx) in in_rx.into_iter().zip(out_tx) {
      counting(rx.recv(), &parks).await.unwrap();
      tx.send(ITEM_VALUE).expect("send failed");
    }
    peer.join().unwrap();
  });
  eprintln!(
    "OneshotPingPong: {}/{} receives parked",
    parks.load(Ordering::Relaxed),
    OPS
  );

  let parks = AtomicUsize::new(0);
  rt.block_on(async {
    let inbound: Arc<Vec<Slot<u64>>> = Arc::new((0..OPS).map(|_| Slot::new()).collect());
    let outbound: Arc<Vec<Slot<u64>>> = Arc::new((0..OPS).map(|_| Slot::new()).collect());
    let barrier = Arc::new(Barrier::new(2));
    let peer = {
      let inbound = Arc::clone(&inbound);
      let outbound = Arc::clone(&outbound);
      let barrier = Arc::clone(&barrier);
      thread::spawn(move || {
        barrier.wait();
        for i in 0..OPS {
          inbound[i].send(ITEM_VALUE).expect("send failed");
          futures_executor::block_on(outbound[i].recv()).unwrap();
        }
      })
    };
    barrier.wait();
    for i in 0..OPS {
      counting(inbound[i].recv(), &parks).await.unwrap();
      outbound[i].send(ITEM_VALUE).expect("send failed");
    }
    peer.join().unwrap();
  });
  eprintln!(
    "OneshotSlotPingPong: {}/{} receives parked",
    parks.load(Ordering::Relaxed),
    OPS
  );

  let parks = AtomicUsize::new(0);
  rt.block_on(async {
    let inbound: &'static [Slot<u64>] = Box::leak(
      (0..OPS)
        .map(|_| Slot::new())
        .collect::<Vec<_>>()
        .into_boxed_slice(),
    );
    let outbound: &'static [Slot<u64>] = Box::leak(
      (0..OPS)
        .map(|_| Slot::new())
        .collect::<Vec<_>>()
        .into_boxed_slice(),
    );
    let (mut in_tx, mut in_rx) = (Vec::with_capacity(OPS), Vec::with_capacity(OPS));
    let (mut out_tx, mut out_rx) = (Vec::with_capacity(OPS), Vec::with_capacity(OPS));
    for s in inbound {
      let (tx, rx) = slot_split::pair(s);
      in_tx.push(tx);
      in_rx.push(rx);
    }
    for s in outbound {
      let (tx, rx) = slot_split::pair(s);
      out_tx.push(tx);
      out_rx.push(rx);
    }
    let barrier = Arc::new(Barrier::new(2));
    let peer = {
      let barrier = Arc::clone(&barrier);
      thread::spawn(move || {
        barrier.wait();
        for (tx, mut rx) in in_tx.into_iter().zip(out_rx) {
          tx.send(ITEM_VALUE).expect("send failed");
          futures_executor::block_on(rx.recv()).unwrap();
        }
      })
    };
    barrier.wait();
    for (mut rx, tx) in in_rx.into_iter().zip(out_tx) {
      counting(rx.recv(), &parks).await.unwrap();
      tx.send(ITEM_VALUE).expect("send failed");
    }
    peer.join().unwrap();
    unsafe {
      drop(Box::from_raw(inbound as *const [Slot<u64>] as *mut [Slot<u64>]));
      drop(Box::from_raw(outbound as *const [Slot<u64>] as *mut [Slot<u64>]));
    }
  });
  eprintln!(
    "OneshotSlotSplitPingPong: {}/{} receives parked",
    parks.load(Ordering::Relaxed),
    OPS
  );

  let parks = AtomicUsize::new(0);
  rt.block_on(async {
    let (mut in_tx, mut in_rx) = (Vec::with_capacity(OPS), Vec::with_capacity(OPS));
    let (mut out_tx, mut out_rx) = (Vec::with_capacity(OPS), Vec::with_capacity(OPS));
    for _ in 0..OPS {
      let (tx, rx) = slot_owned::channel::<u64>();
      in_tx.push(tx);
      in_rx.push(rx);
      let (tx, rx) = slot_owned::channel::<u64>();
      out_tx.push(tx);
      out_rx.push(rx);
    }
    let barrier = Arc::new(Barrier::new(2));
    let peer = {
      let barrier = Arc::clone(&barrier);
      thread::spawn(move || {
        barrier.wait();
        for (tx, mut rx) in in_tx.into_iter().zip(out_rx) {
          tx.send(ITEM_VALUE).expect("send failed");
          futures_executor::block_on(rx.recv()).unwrap();
        }
      })
    };
    barrier.wait();
    for (mut rx, tx) in in_rx.into_iter().zip(out_tx) {
      counting(rx.recv(), &parks).await.unwrap();
      tx.send(ITEM_VALUE).expect("send failed");
    }
    peer.join().unwrap();
  });
  eprintln!(
    "OneshotSlotOwnedPingPong: {}/{} receives parked",
    parks.load(Ordering::Relaxed),
    OPS
  );

  let parks = AtomicUsize::new(0);
  rt.block_on(async {
    let pool = slot_pooled::Pool::<u64>::new(OPS * 2 + 4);
    let (mut in_tx, mut in_rx) = (Vec::with_capacity(OPS), Vec::with_capacity(OPS));
    let (mut out_tx, mut out_rx) = (Vec::with_capacity(OPS), Vec::with_capacity(OPS));
    for _ in 0..OPS {
      let (tx, rx) = pool.pair();
      in_tx.push(tx);
      in_rx.push(rx);
      let (tx, rx) = pool.pair();
      out_tx.push(tx);
      out_rx.push(rx);
    }
    let barrier = Arc::new(Barrier::new(2));
    let peer = {
      let barrier = Arc::clone(&barrier);
      thread::spawn(move || {
        barrier.wait();
        for (tx, mut rx) in in_tx.into_iter().zip(out_rx) {
          tx.send(ITEM_VALUE).expect("send failed");
          futures_executor::block_on(rx.recv()).unwrap();
        }
      })
    };
    barrier.wait();
    for (mut rx, tx) in in_rx.into_iter().zip(out_tx) {
      counting(rx.recv(), &parks).await.unwrap();
      tx.send(ITEM_VALUE).expect("send failed");
    }
    peer.join().unwrap();
  });
  eprintln!(
    "OneshotSlotPooledPingPong: {}/{} receives parked",
    parks.load(Ordering::Relaxed),
    OPS
  );

  let parks = AtomicUsize::new(0);
  rt.block_on(async {
    let (mut in_tx, mut in_rx) = (Vec::with_capacity(OPS), Vec::with_capacity(OPS));
    let (mut out_tx, mut out_rx) = (Vec::with_capacity(OPS), Vec::with_capacity(OPS));
    for _ in 0..OPS {
      let (tx, rx) = slot_exclusive::channel::<u64>();
      in_tx.push(tx);
      in_rx.push(rx);
      let (tx, rx) = slot_exclusive::channel::<u64>();
      out_tx.push(tx);
      out_rx.push(rx);
    }
    let barrier = Arc::new(Barrier::new(2));
    let peer = {
      let barrier = Arc::clone(&barrier);
      thread::spawn(move || {
        barrier.wait();
        for (tx, mut rx) in in_tx.into_iter().zip(out_rx) {
          tx.send(ITEM_VALUE).expect("send failed");
          futures_executor::block_on(rx.recv()).unwrap();
        }
      })
    };
    barrier.wait();
    for (mut rx, tx) in in_rx.into_iter().zip(out_tx) {
      counting(rx.recv(), &parks).await.unwrap();
      tx.send(ITEM_VALUE).expect("send failed");
    }
    peer.join().unwrap();
  });
  eprintln!(
    "OneshotSlotExclusivePingPong: {}/{} receives parked",
    parks.load(Ordering::Relaxed),
    OPS
  );

  let parks = AtomicUsize::new(0);
  rt.block_on(async {
    let (mut in_tx, mut in_rx) = (Vec::with_capacity(OPS), Vec::with_capacity(OPS));
    let (mut out_tx, mut out_rx) = (Vec::with_capacity(OPS), Vec::with_capacity(OPS));
    for _ in 0..OPS {
      let (tx, rx) = oneshot::exclusive::<u64>();
      in_tx.push(tx);
      in_rx.push(rx);
      let (tx, rx) = oneshot::exclusive::<u64>();
      out_tx.push(tx);
      out_rx.push(rx);
    }
    let barrier = Arc::new(Barrier::new(2));
    let peer = {
      let barrier = Arc::clone(&barrier);
      thread::spawn(move || {
        barrier.wait();
        for (tx, mut rx) in in_tx.into_iter().zip(out_rx) {
          tx.send(ITEM_VALUE).expect("send failed");
          futures_executor::block_on(rx.recv()).unwrap();
        }
      })
    };
    barrier.wait();
    for (mut rx, tx) in in_rx.into_iter().zip(out_tx) {
      counting(rx.recv(), &parks).await.unwrap();
      tx.send(ITEM_VALUE).expect("send failed");
    }
    peer.join().unwrap();
  });
  eprintln!(
    "OneshotExclusivePingPong: {}/{} receives parked",
    parks.load(Ordering::Relaxed),
    OPS
  );

  let parks = AtomicUsize::new(0);
  rt.block_on(async {
    let (mut in_tx, mut in_rx) = (Vec::with_capacity(OPS), Vec::with_capacity(OPS));
    let (mut out_tx, mut out_rx) = (Vec::with_capacity(OPS), Vec::with_capacity(OPS));
    for _ in 0..OPS {
      let (tx, rx) = tokio::sync::oneshot::channel::<u64>();
      in_tx.push(tx);
      in_rx.push(rx);
      let (tx, rx) = tokio::sync::oneshot::channel::<u64>();
      out_tx.push(tx);
      out_rx.push(rx);
    }
    let barrier = Arc::new(Barrier::new(2));
    let peer = {
      let barrier = Arc::clone(&barrier);
      thread::spawn(move || {
        barrier.wait();
        for (tx, rx) in in_tx.into_iter().zip(out_rx) {
          tx.send(ITEM_VALUE).expect("send failed");
          futures_executor::block_on(rx).unwrap();
        }
      })
    };
    barrier.wait();
    for (rx, tx) in in_rx.into_iter().zip(out_tx) {
      counting(rx, &parks).await.unwrap();
      tx.send(ITEM_VALUE).expect("send failed");
    }
    peer.join().unwrap();
  });
  eprintln!(
    "OneshotTokioPingPong: {}/{} receives parked",
    parks.load(Ordering::Relaxed),
    OPS
  );
}

fn oneshot_async_benches(c: &mut Criterion) {
  let rt = Runtime::new().unwrap();

  if std::env::var_os("ONESHOT_PARK_DIAG").is_some() {
    report_parked_fraction(&rt);
  }

  const OPS: &[u64] = &[100, 1000];
  const HANDOFF_OPS: &[u64] = &[1000, 10_000];

  run_suite(c, &rt, "OneshotAsync", logic_clonable_full, OPS);
  run_suite(c, &rt, "OneshotAsyncXfer", logic_clonable_xfer, OPS);
  run_suite(c, &rt, "OneshotExclusiveAsync", logic_exclusive_full, OPS);
  run_suite(c, &rt, "OneshotExclusiveAsyncXfer", logic_exclusive_xfer, OPS);
  run_suite(c, &rt, "OneshotTokioAsync", logic_tokio_full, OPS);
  run_suite(c, &rt, "OneshotTokioAsyncXfer", logic_tokio_xfer, OPS);

  run_suite(c, &rt, "OneshotRecordAsync", logic_clonable_record, OPS);
  run_suite(c, &rt, "OneshotSlotRecordAsync", logic_slot_record, OPS);
  run_suite(
    c,
    &rt,
    "OneshotExclusiveRecordAsync",
    logic_exclusive_record,
    OPS,
  );
  run_suite(c, &rt, "OneshotTokioRecordAsync", logic_tokio_record, OPS);
  run_suite(
    c,
    &rt,
    "OneshotSlotExclusiveRecordAsync",
    logic_slot_exclusive_record,
    OPS,
  );
  run_suite(
    c,
    &rt,
    "OneshotSlotSplitRecordAsync",
    logic_slot_split_record,
    OPS,
  );
  run_suite(
    c,
    &rt,
    "OneshotSlotOwnedRecordAsync",
    logic_slot_owned_record,
    OPS,
  );
  run_suite(
    c,
    &rt,
    "OneshotSlotPooledRecordAsync",
    logic_slot_pooled_record,
    OPS,
  );
  run_suite(
    c,
    &rt,
    "OneshotSlotPooledHostRecordAsync",
    logic_slot_pooled_host_record,
    OPS,
  );
  run_suite(
    c,
    &rt,
    "OneshotSlotExclusiveAsync",
    logic_slot_exclusive_full,
    OPS,
  );
  run_suite(
    c,
    &rt,
    "OneshotSlotExclusiveAsyncXfer",
    logic_slot_exclusive_xfer,
    OPS,
  );
  run_suite(c, &rt, "OneshotSlotSplitAsync", logic_slot_split_full, OPS);
  run_suite(
    c,
    &rt,
    "OneshotSlotSplitAsyncXfer",
    logic_slot_split_xfer,
    OPS,
  );
  run_suite(c, &rt, "OneshotSlotOwnedAsync", logic_slot_owned_full, OPS);
  run_suite(
    c,
    &rt,
    "OneshotSlotOwnedAsyncXfer",
    logic_slot_owned_xfer,
    OPS,
  );
  run_suite(c, &rt, "OneshotSlotPooledAsync", logic_slot_pooled_full, OPS);
  run_suite(
    c,
    &rt,
    "OneshotSlotPooledAsyncXfer",
    logic_slot_pooled_xfer,
    OPS,
  );
  run_suite(c, &rt, "OneshotSlotAsync", logic_slot_full, OPS);
  run_suite(c, &rt, "OneshotSlotAsyncXfer", logic_slot_xfer, OPS);

  run_suite(c, &rt, "OneshotHandoff", logic_clonable_handoff, HANDOFF_OPS);
  run_suite(
    c,
    &rt,
    "OneshotSlotExclusiveHandoff",
    logic_slot_exclusive_handoff,
    HANDOFF_OPS,
  );
  run_suite(c, &rt, "OneshotSlotHandoff", logic_slot_handoff, HANDOFF_OPS);
  run_suite(
    c,
    &rt,
    "OneshotSlotSplitHandoff",
    logic_slot_split_handoff,
    HANDOFF_OPS,
  );
  run_suite(
    c,
    &rt,
    "OneshotSlotOwnedHandoff",
    logic_slot_owned_handoff,
    HANDOFF_OPS,
  );
  run_suite(
    c,
    &rt,
    "OneshotSlotPooledHandoff",
    logic_slot_pooled_handoff,
    HANDOFF_OPS,
  );
  run_suite(
    c,
    &rt,
    "OneshotExclusiveHandoff",
    logic_exclusive_handoff,
    HANDOFF_OPS,
  );
  run_suite(
    c,
    &rt,
    "OneshotTokioHandoff",
    logic_tokio_handoff,
    HANDOFF_OPS,
  );

  run_suite(c, &rt, "OneshotPingPong", logic_clonable_pingpong, HANDOFF_OPS);
  run_suite(
    c,
    &rt,
    "OneshotSlotExclusivePingPong",
    logic_slot_exclusive_pingpong,
    HANDOFF_OPS,
  );
  run_suite(c, &rt, "OneshotSlotPingPong", logic_slot_pingpong, HANDOFF_OPS);
  run_suite(
    c,
    &rt,
    "OneshotSlotSplitPingPong",
    logic_slot_split_pingpong,
    HANDOFF_OPS,
  );
  run_suite(
    c,
    &rt,
    "OneshotSlotOwnedPingPong",
    logic_slot_owned_pingpong,
    HANDOFF_OPS,
  );
  run_suite(
    c,
    &rt,
    "OneshotSlotPooledPingPong",
    logic_slot_pooled_pingpong,
    HANDOFF_OPS,
  );
  run_suite(
    c,
    &rt,
    "OneshotExclusivePingPong",
    logic_exclusive_pingpong,
    HANDOFF_OPS,
  );
  run_suite(
    c,
    &rt,
    "OneshotTokioPingPong",
    logic_tokio_pingpong,
    HANDOFF_OPS,
  );
}

criterion_group!(benches, oneshot_async_benches);
criterion_main!(benches);
