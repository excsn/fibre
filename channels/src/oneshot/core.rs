//! Shared core of the clonable oneshot channel.
//!
//! All channel state lives in ONE atomic word so every observer gets a
//! consistent snapshot of phase, sender count, and receiver liveness in a
//! single load. The word is laid out as:
//!
//! ```text
//! [ sender count (bits 4..) | RX_DROPPED (bit 3) | phase (bits 0..3) ]
//! ```
//!
//! Phase is strictly monotonic: EMPTY -> WRITING -> SENT -> TAKEN, with
//! EMPTY -> CLOSED as the no-value terminal. Only the WRITING owner advances
//! WRITING -> SENT, and only one claimant wins SENT -> TAKEN, so owner-driven
//! transitions are `fetch_add` deltas and contended ones are CAS loops that
//! preserve the count/flag bits.
//!
//! The value slot is a bare `UnsafeCell<MaybeUninit<T>>`; it is initialized
//! iff phase is SENT, and whoever moves phase off SENT (to TAKEN) owns the
//! value. Exclusivity for the write comes from holding WRITING.

use crate::async_util::AtomicWaker;
use crate::error::{RecvError, TryRecvError, TrySendError};
use crate::internal::sync::{AtomicUsize, Ordering};

use core::task::{Context, Poll};
use std::cell::UnsafeCell;
use std::fmt;
use std::mem::MaybeUninit;

pub(super) const STATE_EMPTY: usize = 0;
pub(super) const STATE_WRITING: usize = 1;
pub(super) const STATE_SENT: usize = 2;
pub(super) const STATE_TAKEN: usize = 3;
pub(super) const STATE_CLOSED: usize = 4;

const PHASE_MASK: usize = 0b111;
const RX_DROPPED: usize = 1 << 3;
const COUNT_SHIFT: usize = 4;
const COUNT_UNIT: usize = 1 << COUNT_SHIFT;

#[inline(always)]
fn phase(word: usize) -> usize {
  word & PHASE_MASK
}

#[inline(always)]
fn sender_count(word: usize) -> usize {
  word >> COUNT_SHIFT
}

#[inline(always)]
fn with_phase(word: usize, new_phase: usize) -> usize {
  (word & !PHASE_MASK) | new_phase
}

pub(super) struct OneShotShared<T> {
  state: AtomicUsize,
  value_slot: UnsafeCell<MaybeUninit<T>>,
  receiver_waker: AtomicWaker,
  #[cfg(test)]
  pub(super) hold_publish: crate::internal::sync::AtomicBool,
}

impl<T> fmt::Debug for OneShotShared<T> {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    let word = self.state.load(Ordering::Relaxed);
    let phase_str = match phase(word) {
      STATE_EMPTY => "Empty",
      STATE_WRITING => "Writing",
      STATE_SENT => "Sent",
      STATE_TAKEN => "Taken",
      STATE_CLOSED => "Closed",
      _ => "Unknown",
    };
    f.debug_struct("OneShotShared")
      .field("state", &phase_str)
      .field("receiver_dropped", &(word & RX_DROPPED != 0))
      .field("sender_count", &sender_count(word))
      .finish_non_exhaustive()
  }
}

unsafe impl<T: Send> Send for OneShotShared<T> {}
unsafe impl<T: Send> Sync for OneShotShared<T> {}

impl<T> OneShotShared<T> {
  pub(super) fn new() -> Self {
    OneShotShared {
      state: AtomicUsize::new(COUNT_UNIT | STATE_EMPTY),
      value_slot: UnsafeCell::new(MaybeUninit::uninit()),
      receiver_waker: AtomicWaker::new(),
      #[cfg(test)]
      hold_publish: crate::internal::sync::AtomicBool::new(false),
    }
  }

  unsafe fn take_value(&self) -> T {
    unsafe { (*self.value_slot.get()).assume_init_read() }
  }

  unsafe fn drop_value(&self) {
    unsafe { (*self.value_slot.get()).assume_init_drop() };
  }

  pub(super) fn increment_senders(&self) {
    self.state.fetch_add(COUNT_UNIT, Ordering::Relaxed);
  }

  pub(super) fn decrement_senders(&self) {
    // RMWs on `state` are totally ordered, so `prev` is the true current word:
    // it cannot miss an RX_DROPPED or phase change that ordered before it.
    let prev = self.state.fetch_sub(COUNT_UNIT, Ordering::AcqRel);
    debug_assert!(sender_count(prev) >= 1, "oneshot: sender count underflow");
    if sender_count(prev) != 1 {
      return;
    }

    let mut cur = prev - COUNT_UNIT;
    loop {
      match phase(cur) {
        STATE_EMPTY => {
          match self.state.compare_exchange_weak(
            cur,
            with_phase(cur, STATE_CLOSED),
            Ordering::AcqRel,
            Ordering::Acquire,
          ) {
            Ok(_) => {
              self.receiver_waker.wake();
              return;
            }
            Err(w) => cur = w,
          }
        }
        STATE_SENT => {
          if cur & RX_DROPPED == 0 {
            // Receiver is alive; it will take the value or reclaim it on drop.
            return;
          }
          // Receiver dropped while we were WRITING, so it could not claim the
          // value then. Claim and drop it here to prevent a leak.
          match self.state.compare_exchange_weak(
            cur,
            with_phase(cur, STATE_TAKEN),
            Ordering::AcqRel,
            Ordering::Acquire,
          ) {
            Ok(_) => {
              unsafe { self.drop_value() };
              return;
            }
            Err(w) => cur = w,
          }
        }
        _ => return,
      }
    }
  }

  /// Marks the receiver as gone and performs all receiver-side cleanup:
  /// closes an EMPTY channel, or claims and drops a SENT-but-untaken value.
  pub(super) fn mark_receiver_dropped(&self) {
    let prev = self.state.fetch_or(RX_DROPPED, Ordering::AcqRel);
    let mut cur = prev | RX_DROPPED;
    loop {
      match phase(cur) {
        STATE_EMPTY => {
          match self.state.compare_exchange_weak(
            cur,
            with_phase(cur, STATE_CLOSED),
            Ordering::AcqRel,
            Ordering::Acquire,
          ) {
            Ok(_) => return,
            Err(w) => cur = w,
          }
        }
        STATE_SENT => {
          match self.state.compare_exchange_weak(
            cur,
            with_phase(cur, STATE_TAKEN),
            Ordering::AcqRel,
            Ordering::Acquire,
          ) {
            Ok(_) => {
              unsafe { self.drop_value() };
              return;
            }
            Err(w) => cur = w,
          }
        }
        // WRITING: the in-flight sender publishes SENT, then the last sender
        // drop observes SENT | RX_DROPPED and reclaims the value.
        _ => return,
      }
    }
  }

  pub(super) fn send(&self, value: T) -> Result<(), TrySendError<T>> {
    let mut cur = self.state.load(Ordering::Acquire);
    loop {
      if cur & RX_DROPPED != 0 {
        return Err(TrySendError::Closed(value));
      }
      if phase(cur) != STATE_EMPTY {
        return Err(TrySendError::Sent(value));
      }
      match self.state.compare_exchange_weak(
        cur,
        with_phase(cur, STATE_WRITING),
        Ordering::AcqRel,
        Ordering::Acquire,
      ) {
        Ok(_) => break,
        Err(w) => cur = w,
      }
    }

    if self.state.load(Ordering::Acquire) & RX_DROPPED != 0 {
      self
        .state
        .fetch_sub(STATE_WRITING - STATE_EMPTY, Ordering::AcqRel);
      return Err(TrySendError::Closed(value));
    }

    unsafe {
      (*self.value_slot.get()).write(value);
    }

    #[cfg(test)]
    while self.hold_publish.load(Ordering::Acquire) {
      std::thread::yield_now();
    }

    let prev = self
      .state
      .fetch_add(STATE_SENT - STATE_WRITING, Ordering::AcqRel);
    debug_assert_eq!(
      phase(prev),
      STATE_WRITING,
      "oneshot: publish from non-WRITING phase"
    );

    self.receiver_waker.wake();
    Ok(())
  }

  pub(super) fn try_recv(&self) -> Result<T, TryRecvError> {
    let mut cur = self.state.load(Ordering::Acquire);
    loop {
      match phase(cur) {
        STATE_SENT => {
          match self.state.compare_exchange_weak(
            cur,
            with_phase(cur, STATE_TAKEN),
            Ordering::AcqRel,
            Ordering::Acquire,
          ) {
            Ok(_) => return Ok(unsafe { self.take_value() }),
            Err(w) => cur = w,
          }
        }
        STATE_TAKEN | STATE_WRITING => return Err(TryRecvError::Empty),
        STATE_CLOSED => return Err(TryRecvError::Disconnected),
        _ => {
          if sender_count(cur) > 0 {
            return Err(TryRecvError::Empty);
          }
          // Disconnected only once EMPTY -> CLOSED actually commits; a failed
          // CAS means the phase advanced (a value landed) and we re-dispatch.
          match self.state.compare_exchange_weak(
            cur,
            with_phase(cur, STATE_CLOSED),
            Ordering::AcqRel,
            Ordering::Acquire,
          ) {
            Ok(_) => return Err(TryRecvError::Disconnected),
            Err(w) => cur = w,
          }
        }
      }
    }
  }

  pub(super) fn poll_recv(&self, cx: &mut Context<'_>) -> Poll<Result<T, RecvError>> {
    match self.try_recv() {
      Ok(value) => return Poll::Ready(Ok(value)),
      Err(TryRecvError::Disconnected) => return Poll::Ready(Err(RecvError::Disconnected)),
      Err(TryRecvError::Empty) => {}
    }
    // TAKEN reports Empty from try_recv, but no wake will ever follow it; a
    // future polled past the take must resolve rather than hang.
    if phase(self.state.load(Ordering::Acquire)) == STATE_TAKEN {
      return Poll::Ready(Err(RecvError::Disconnected));
    }

    self.receiver_waker.register(cx.waker());

    match self.try_recv() {
      Ok(value) => Poll::Ready(Ok(value)),
      Err(TryRecvError::Disconnected) => Poll::Ready(Err(RecvError::Disconnected)),
      Err(TryRecvError::Empty) => {
        if phase(self.state.load(Ordering::Acquire)) == STATE_TAKEN {
          return Poll::Ready(Err(RecvError::Disconnected));
        }
        Poll::Pending
      }
    }
  }

  pub(super) fn is_receiver_dropped(&self) -> bool {
    self.state.load(Ordering::Acquire) & RX_DROPPED != 0
  }

  pub(super) fn is_sent(&self) -> bool {
    let p = phase(self.state.load(Ordering::Acquire));
    p == STATE_SENT || p == STATE_TAKEN
  }

  pub(super) fn is_closed_for_receiver(&self) -> bool {
    let word = self.state.load(Ordering::Acquire);
    match phase(word) {
      STATE_TAKEN | STATE_CLOSED => true,
      STATE_EMPTY | STATE_WRITING => sender_count(word) == 0,
      _ => false,
    }
  }

  #[cfg(test)]
  pub(super) fn test_phase(&self) -> usize {
    phase(self.state.load(Ordering::Acquire))
  }

  #[cfg(test)]
  pub(super) fn test_sender_count(&self) -> usize {
    sender_count(self.state.load(Ordering::Acquire))
  }
}

impl<T> Drop for OneShotShared<T> {
  fn drop(&mut self) {
    if phase(self.state.load(Ordering::Acquire)) == STATE_SENT {
      unsafe { self.drop_value() };
    }
  }
}
