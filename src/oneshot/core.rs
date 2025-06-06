// src/oneshot/core.rs

use crate::async_util::AtomicWaker;
use crate::error::{RecvError, TryRecvError, TrySendError}; // Assuming SendError is needed by Sender

use core::task::{Context, Poll};
use std::fmt;
use std::mem::MaybeUninit;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering}; // Added spin_loop_hint
use std::sync::Mutex;

// State constants for OneShotShared::state
pub(super) const STATE_EMPTY: usize = 0; // No value, receiver may be waiting. Initial state.
pub(super) const STATE_WRITING: usize = 1; // A sender is in the process of writing the value (critical section).
pub(super) const STATE_SENT: usize = 2; // Value has been written by a sender and is ready for receiver.
pub(super) const STATE_TAKEN: usize = 3; // Receiver has taken the value. Terminal state for value.
pub(super) const STATE_CLOSED: usize = 4; // Channel closed definitively (e.g. receiver dropped AND no value was sent, or all senders dropped AND no value sent). Terminal state.

pub(super) struct OneShotShared<T> {
  pub(crate) value_slot: Mutex<Option<MaybeUninit<T>>>,
  pub(crate) state: AtomicUsize, // STATE_EMPTY, STATE_WRITING, STATE_SENT, STATE_TAKEN, STATE_CLOSED
  receiver_waker: AtomicWaker,
  pub(crate) receiver_dropped: AtomicBool, // True if the Receiver struct itself has been dropped
  pub(crate) sender_count: AtomicUsize,    // Number of active Sender structs
}

impl<T> fmt::Debug for OneShotShared<T> {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    let state_val = self.state.load(Ordering::Relaxed);
    let state_str = match state_val {
      STATE_EMPTY => "Empty",
      STATE_WRITING => "Writing",
      STATE_SENT => "Sent",
      STATE_TAKEN => "Taken",
      STATE_CLOSED => "Closed",
      _ => "Unknown",
    };
    f.debug_struct("OneShotShared")
      .field("state", &state_str)
      .field("receiver_dropped", &self.receiver_dropped.load(Ordering::Relaxed))
      .field("sender_count", &self.sender_count.load(Ordering::Relaxed))
      .finish_non_exhaustive()
  }
}

unsafe impl<T: Send> Send for OneShotShared<T> {}
unsafe impl<T: Send> Sync for OneShotShared<T> {}

impl<T> OneShotShared<T> {
  pub(super) fn new() -> Self {
    OneShotShared {
      value_slot: Mutex::new(None),
      state: AtomicUsize::new(STATE_EMPTY),
      receiver_waker: AtomicWaker::new(),
      receiver_dropped: AtomicBool::new(false),
      sender_count: AtomicUsize::new(1),
    }
  }

  pub(super) fn increment_senders(&self) {
    // Only increment if not already closed due to receiver drop.
    // This prevents new senders if the receiver is already gone.
    if !self.receiver_dropped.load(Ordering::Relaxed) {
      self.sender_count.fetch_add(1, Ordering::Relaxed);
    }
    // If receiver is dropped, new senders are effectively inert.
  }

  pub(super) fn decrement_senders(&self) {
    if self.sender_count.fetch_sub(1, Ordering::AcqRel) == 1 {
      // This was the last sender.
      // If state is still EMPTY (meaning no value was ever successfully sent and committed),
      // then the channel is now disconnected from the sender side.
      // Try to transition EMPTY -> CLOSED.
      if self
        .state
        .compare_exchange(
          STATE_EMPTY,
          STATE_CLOSED,
          Ordering::AcqRel, // Ensure prior writes (if any) are visible, and this store is visible
          Ordering::Relaxed,
        )
        .is_ok()
      {
        self.receiver_waker.wake(); // Wake receiver to observe Disconnected state
      }
      // If state was WRITING, SENT, or TAKEN, the value path took precedence.
      // If it was ALREADY CLOSED (e.g. by receiver drop), this does nothing to state.
      // But we still wake the receiver in case it was pending on this last sender dropping.
      else if self.state.load(Ordering::Relaxed) != STATE_TAKEN && self.state.load(Ordering::Relaxed) != STATE_SENT {
        // Avoid waking if value is there or taken
        self.receiver_waker.wake();
      }
    }
  }

  pub(super) fn mark_receiver_dropped(&self) {
    self.receiver_dropped.store(true, Ordering::Release);
    // If no value has been sent and no senders are in the process of sending,
    // transition to CLOSED.
    if self
      .state
      .compare_exchange(STATE_EMPTY, STATE_CLOSED, Ordering::AcqRel, Ordering::Relaxed)
      .is_ok()
    {
      // Senders trying to send will now see receiver_dropped or state == CLOSED.
      // No specific waker for senders in oneshot, they fail fast.
    }
    // If state was STATE_WRITING, the sender might complete or see receiver_dropped.
    // If state was STATE_SENT, the value is now orphaned. It will be dropped by Receiver::drop.
    // If state was STATE_TAKEN or already CLOSED, no change.
  }

  pub(super) fn send(&self, value: T) -> Result<(), TrySendError<T>> {
    // Fast path check for receiver dropped or channel already terminally closed or value sent/taken.
    if self.receiver_dropped.load(Ordering::Acquire) {
      return Err(TrySendError::Closed(value));
    }
    let current_state = self.state.load(Ordering::Acquire);
    if current_state >= STATE_SENT {
      // SENT, TAKEN, or CLOSED
      return Err(TrySendError::Sent(value)); // Treat CLOSED as if already sent for send attempt
    }

    // Attempt to transition from EMPTY to WRITING. Only one sender will succeed.
    match self.state.compare_exchange(
      STATE_EMPTY,
      STATE_WRITING,
      Ordering::AcqRel,  // Acquire to sync with other CAS, Release for value_slot write
      Ordering::Acquire, // On failure, acquire to see latest state
    ) {
      Ok(_) => {
        // Successfully Acquired WRITING state
        // Double check receiver_dropped *after* acquiring WRITING lock.
        if self.receiver_dropped.load(Ordering::Acquire) {
          // Receiver dropped between initial check and acquiring write lock.
          // Revert state to EMPTY (or CLOSED if no other senders).
          // This logic makes it more complex, simpler might be to proceed and let send fail.
          // For now, let's assume this is rare and proceed, the send will fail if receiver is gone.
          // A better approach: if this CAS to WRITING fails because state is now CLOSED, handle that.
          // If it succeeds, but receiver_dropped is now true:
          self.state.store(STATE_EMPTY, Ordering::Release); // Backtrack
          return Err(TrySendError::Closed(value));
        }

        // We are the chosen sender.
        // Lock is held very briefly.
        let mut guard = self.value_slot.lock().expect("Oneshot value_slot mutex poisoned");
        *guard = Some(MaybeUninit::new(value));

        // Now transition from WRITING to SENT.
        // This must succeed as we are the only one in WRITING state.
        // Use swap to ensure it was WRITING.
        let prev_state = self.state.swap(STATE_SENT, Ordering::AcqRel);
        debug_assert_eq!(
          prev_state, STATE_WRITING,
          "Oneshot: State inconsistency during send, expected WRITING"
        );

        self.receiver_waker.wake();
        Ok(())
      }
      Err(observed_state_on_failure) => {
        // CAS failed. Another sender is writing, or value is already sent/taken/closed.
        if observed_state_on_failure >= STATE_SENT {
          // SENT, TAKEN, CLOSED
          Err(TrySendError::Sent(value))
        } else if observed_state_on_failure == STATE_WRITING {
          // Another sender is currently writing. Contend or treat as "already sent".
          // For oneshot, usually "first wins cleanly". So, treat as Sent.
          Err(TrySendError::Sent(value))
        } else {
          // Observed EMPTY again, but our CAS failed - means another thread did CAS(EMPTY, WRITING)
          // and maybe even completed or failed itself. Effectively, we lost the race.
          Err(TrySendError::Sent(value))
        }
      }
    }
  }

  pub(super) fn try_recv(&self) -> Result<T, TryRecvError> {
    let current_state = self.state.load(Ordering::Acquire);

    if current_state == STATE_SENT {
      // Attempt to transition from SENT to TAKEN. Only one receiver poll will succeed.
      if self
        .state
        .compare_exchange(
          STATE_SENT,
          STATE_TAKEN,
          Ordering::AcqRel, // Acquire to sync with SENT, Release for value_slot change (though Mutex handles that)
          Ordering::Acquire, // On failure, re-read state
        )
        .is_ok()
      {
        // Successfully claimed the value.
        let mut guard = self.value_slot.lock().expect("Oneshot value_slot mutex poisoned");
        match guard.take() {
          Some(mu_value) => unsafe { Ok(mu_value.assume_init()) },
          None => {
            // Should not happen: state was SENT but value_slot was None. Logic error.
            // This implies state machine corruption.
            // To be safe, treat as if disconnected now.
            self.state.store(STATE_CLOSED, Ordering::Relaxed); // Mark corrupted state as closed
            Err(TryRecvError::Disconnected) // Or a specific "InternalError"
          }
        }
      } else {
        // CAS failed: state changed from SENT to something else (likely TAKEN by another poll, or CLOSED).
        // Re-evaluate current state.
        let new_state_after_cas_fail = self.state.load(Ordering::Acquire);
        if new_state_after_cas_fail == STATE_TAKEN {
          Err(TryRecvError::Empty) // Already taken by this logical receiver, now appears empty
        } else if new_state_after_cas_fail == STATE_CLOSED || self.sender_count.load(Ordering::Relaxed) == 0 {
          Err(TryRecvError::Disconnected)
        } else {
          Err(TryRecvError::Empty) // Still SENT (but our CAS failed), or EMPTY/WRITING
        }
      }
    } else if current_state == STATE_TAKEN {
      Err(TryRecvError::Empty) // Already taken, effectively empty for subsequent calls
    } else if current_state == STATE_CLOSED {
      Err(TryRecvError::Disconnected)
    } else {
      // EMPTY or WRITING
      // If empty and all senders are gone, it's disconnected.
      if current_state == STATE_EMPTY && self.sender_count.load(Ordering::Acquire) == 0 {
        // Attempt to transition to CLOSED if not already done by last sender drop
        self
          .state
          .compare_exchange(STATE_EMPTY, STATE_CLOSED, Ordering::Relaxed, Ordering::Relaxed)
          .ok();
        Err(TryRecvError::Disconnected)
      } else {
        Err(TryRecvError::Empty) // Not ready yet, or senders still active / writing
      }
    }
  }

  pub(super) fn poll_recv(&self, cx: &mut Context<'_>) -> Poll<Result<T, RecvError>> {
    loop {
      // Loop to handle state changes after waker registration
      match self.try_recv() {
        Ok(value) => return Poll::Ready(Ok(value)),
        Err(TryRecvError::Disconnected) => return Poll::Ready(Err(RecvError::Disconnected)),
        Err(TryRecvError::Empty) => {
          // Value not ready. State is EMPTY or WRITING, and senders might still exist.
          // Or state was SENT but CAS to TAKEN failed (another poll is racing).

          // If already terminally closed or taken by another concurrent poll, future should resolve.
          let current_state = self.state.load(Ordering::Acquire);
          if current_state == STATE_TAKEN || current_state == STATE_CLOSED {
            // If taken, it means another poll instance of *this same receiver* got it.
            // This poll attempt should then effectively see it as "empty" leading to Disconnected if senders gone.
            if self.sender_count.load(Ordering::Acquire) == 0 && current_state != STATE_SENT {
              return Poll::Ready(Err(RecvError::Disconnected));
            }
            // if state is TAKEN but senders still exist, it's like Empty for this poll.
            // We need to re-evaluate based on this, so loop or register.
            // This path indicates a race, safer to register and re-poll.
          }
          // Check again if all senders dropped AFTER deciding it's Empty
          if current_state == STATE_EMPTY && self.sender_count.load(Ordering::Acquire) == 0 {
            self
              .state
              .compare_exchange(STATE_EMPTY, STATE_CLOSED, Ordering::Relaxed, Ordering::Relaxed)
              .ok();
            return Poll::Ready(Err(RecvError::Disconnected));
          }

          self.receiver_waker.register(cx.waker());

          // Critical re-check after registering waker.
          // This is to see if the state changed *while* we were registering.
          match self.try_recv() {
            // Try again immediately
            Ok(value) => {
              // Value became available
              // Waker was registered, but we got the value.
              // It's fine, next wake on this waker will be a no-op for this future.
              return Poll::Ready(Ok(value));
            }
            Err(TryRecvError::Disconnected) => {
              // Became disconnected
              return Poll::Ready(Err(RecvError::Disconnected));
            }
            Err(TryRecvError::Empty) => {
              // Still empty
              // Waker is correctly registered for the current "empty" state.
              return Poll::Pending;
            }
          }
        }
      }
    }
  }
}
