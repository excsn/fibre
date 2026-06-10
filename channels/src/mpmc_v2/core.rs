//! The core shared data structures and logic for the MPMC channel.

use crate::error::{BatchSendErrorReason, TryRecvError, TrySendError};
use crate::RecvError;
use core::task::{Context, Poll};
use parking_lot::Mutex;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU8, Ordering};
use std::task::Waker;
use std::thread::Thread;

// --- State Machine Constants ---
pub(crate) const STATE_WAITING: u8 = 0;
pub(crate) const STATE_SUCCESS: u8 = 1;
pub(crate) const STATE_CANCELLED: u8 = 2;

// --- Waiter & Data Structs ---

/// Data that a parked sender might hold, specifically for rendezvous channels where
/// the item is transferred directly without being buffered.
#[derive(Debug)]
pub(crate) enum WaiterData<T> {
  SenderItem(Option<T>),
}

/// Represents a parked synchronous thread waiting for an operation to complete.
#[derive(Debug)]
pub(crate) struct SyncWaiter<T> {
  /// The handle to the parked thread, used for `unpark()`.
  pub(crate) thread: Thread,
  /// A slot for a rendezvous item. `None` for buffered waiters.
  pub(crate) data: Option<WaiterData<T>>,
  /// Raw pointer to the atomic state flag on the waiting thread's stack frame.
  /// Safety: valid as long as the waiter is live in the queue (the thread is parked).
  pub(crate) state: *const AtomicU8,
}

impl<T> SyncWaiter<T> {
  /// Takes the item from a rendezvous sender's waiter slot.
  pub(crate) fn take_item_from_sender_slot(&mut self) -> Option<T> {
    if let Some(WaiterData::SenderItem(item_opt)) = self.data.as_mut() {
      item_opt.take()
    } else {
      None
    }
  }
}

unsafe impl<T: Send> Send for SyncWaiter<T> {}
unsafe impl<T: Send> Sync for SyncWaiter<T> {}

/// Represents a parked asynchronous task waiting for an operation to complete.
#[derive(Debug)]
pub(crate) struct AsyncWaiter<T> {
  /// The waker for the parked future, used for `wake()`.
  pub(crate) waker: Waker,
  /// A slot for a rendezvous item. `None` for buffered waiters.
  pub(crate) data: Option<WaiterData<T>>,
  /// Raw pointer to the atomic state flag within the pinned future or AsyncReceiver struct.
  /// Safety: valid as long as the waiter is live in the queue (the future is pinned/alive).
  pub(crate) state: *const AtomicU8,
}

impl<T> AsyncWaiter<T> {
  /// Takes the item from a rendezvous sender's waiter slot.
  pub(crate) fn take_item_from_sender_slot(&mut self) -> Option<T> {
    if let Some(WaiterData::SenderItem(item_opt)) = self.data.as_mut() {
      item_opt.take()
    } else {
      None
    }
  }
}

unsafe impl<T: Send> Send for AsyncWaiter<T> {}
unsafe impl<T: Send> Sync for AsyncWaiter<T> {}

/// A wake handle collected under the central mutex and woken after it is
/// released, so woken threads/tasks don't immediately contend on the lock.
pub(crate) enum WakeRef {
  Thread(Thread),
  Waker(Waker),
}

impl WakeRef {
  #[inline]
  pub(crate) fn wake(self) {
    match self {
      WakeRef::Thread(thread) => thread.unpark(),
      WakeRef::Waker(waker) => waker.wake(),
    }
  }
}

/// The core state of the MPMC channel, protected by a single `Mutex`.
#[derive(Debug)]
pub(crate) struct MpmcChannelInternal<T> {
  /// The primary buffer for items when the channel has a capacity > 0.
  pub(crate) queue: VecDeque<T>,
  /// Queue of parked synchronous senders.
  pub(crate) waiting_sync_senders: VecDeque<SyncWaiter<T>>,
  /// Queue of parked asynchronous senders.
  pub(crate) waiting_async_senders: VecDeque<AsyncWaiter<T>>,
  /// Queue of parked synchronous receivers.
  pub(crate) waiting_sync_receivers: VecDeque<SyncWaiter<T>>,
  /// Queue of parked asynchronous receivers.
  pub(crate) waiting_async_receivers: VecDeque<AsyncWaiter<T>>,
  /// The number of active `Sender` and `AsyncSender` handles.
  pub(crate) sender_count: usize,
  /// The number of active `Receiver` and `AsyncReceiver` handles.
  pub(crate) receiver_count: usize,
}

/// The shared owner of the channel's internal state, designed to be wrapped in an `Arc`.
#[derive(Debug)]
pub(crate) struct MpmcShared<T> {
  pub(crate) internal: Mutex<MpmcChannelInternal<T>>,
  pub(crate) capacity: usize,
}

unsafe impl<T: Send> Send for MpmcShared<T> {}
unsafe impl<T: Send> Sync for MpmcShared<T> {}

impl<T: Send> MpmcShared<T> {
  /// Creates a new shared core for the channel with a given capacity.
  pub(crate) fn new(capacity: usize) -> Self {
    MpmcShared {
      internal: Mutex::new(MpmcChannelInternal {
        queue: VecDeque::with_capacity(if capacity == usize::MAX { 32 } else { capacity }),
        waiting_sync_senders: VecDeque::new(),
        waiting_async_senders: VecDeque::new(),
        waiting_sync_receivers: VecDeque::new(),
        waiting_async_receivers: VecDeque::new(),
        sender_count: 1,
        receiver_count: 1,
      }),
      capacity,
    }
  }

  /// The core logic for attempting to send an item.
  pub(crate) fn try_send_core(&self, item: T) -> Result<(), TrySendError<T>> {
    let mut guard = self.internal.lock();

    if guard.receiver_count == 0 {
      return Err(TrySendError::Closed(item));
    }

    // --- Priority 1: Wake a waiting receiver ---
    loop {
      match guard.waiting_async_receivers.pop_front() {
        None => break,
        Some(waiter) => {
          let waiter_state = unsafe { &*waiter.state };
          if waiter_state
            .compare_exchange(
              STATE_WAITING,
              STATE_SUCCESS,
              Ordering::SeqCst,
              Ordering::SeqCst,
            )
            .is_ok()
          {
            guard.queue.push_back(item);
            waiter.waker.wake();
            return Ok(());
          }
          // STATE_CANCELLED: discard and try next
        }
      }
    }

    loop {
      match guard.waiting_sync_receivers.pop_front() {
        None => break,
        Some(waiter) => {
          let waiter_state = unsafe { &*waiter.state };
          if waiter_state
            .compare_exchange(
              STATE_WAITING,
              STATE_SUCCESS,
              Ordering::SeqCst,
              Ordering::SeqCst,
            )
            .is_ok()
          {
            guard.queue.push_back(item);
            waiter.thread.unpark();
            return Ok(());
          }
          // STATE_CANCELLED: discard and try next
        }
      }
    }

    // --- Priority 2: Push to buffer if space is available ---
    if self.capacity == 0 {
      return Err(TrySendError::Full(item));
    }
    if self.capacity == usize::MAX || guard.queue.len() < self.capacity {
      guard.queue.push_back(item);
      return Ok(());
    }

    Err(TrySendError::Full(item))
  }

  /// The core logic for attempting to receive an item.
  pub(crate) fn try_recv_core(&self) -> Result<T, TryRecvError> {
    let mut guard = self.internal.lock();

    // --- Priority 1: Check for a waiting rendezvous sender ---
    loop {
      if guard
        .waiting_async_senders
        .front()
        .map(|w| w.data.is_some())
        .unwrap_or(false)
      {
        let mut waiter = guard.waiting_async_senders.pop_front().unwrap();
        let waiter_state = unsafe { &*waiter.state };
        match waiter_state.compare_exchange(
          STATE_WAITING,
          STATE_SUCCESS,
          Ordering::SeqCst,
          Ordering::SeqCst,
        ) {
          Ok(_) => {
            let item = waiter.take_item_from_sender_slot().unwrap();
            waiter.waker.wake();
            return Ok(item);
          }
          Err(_) => {
            drop(waiter.data.take()); // CANCELLED: drop rendezvous payload, loop
          }
        }
      } else {
        break;
      }
    }

    loop {
      if guard
        .waiting_sync_senders
        .front()
        .map(|w| w.data.is_some())
        .unwrap_or(false)
      {
        let mut waiter = guard.waiting_sync_senders.pop_front().unwrap();
        let waiter_state = unsafe { &*waiter.state };
        match waiter_state.compare_exchange(
          STATE_WAITING,
          STATE_SUCCESS,
          Ordering::SeqCst,
          Ordering::SeqCst,
        ) {
          Ok(_) => {
            let item = waiter.take_item_from_sender_slot().unwrap();
            waiter.thread.unpark();
            return Ok(item);
          }
          Err(_) => {
            drop(waiter.data.take());
          }
        }
      } else {
        break;
      }
    }

    // --- Priority 2: Check the main buffer ---
    if let Some(item) = guard.queue.pop_front() {
      // Free buffer space exists. Only wake waiting senders if the channel is buffered.
      // Rendezvous senders (capacity == 0) are only ever woken from Priority 1 when their
      // specific payload is extracted; waking them here would cause their item to be silently
      // dropped (payload leak → deadlock).
      if self.capacity > 0 {
        let mut woke = false;
        loop {
          match guard.waiting_async_senders.pop_front() {
            None => break,
            Some(waiter) => {
              let waiter_state = unsafe { &*waiter.state };
              if waiter_state
                .compare_exchange(
                  STATE_WAITING,
                  STATE_SUCCESS,
                  Ordering::SeqCst,
                  Ordering::SeqCst,
                )
                .is_ok()
              {
                waiter.waker.wake();
                woke = true;
                break;
              }
            }
          }
        }

        if !woke {
          loop {
            match guard.waiting_sync_senders.pop_front() {
              None => break,
              Some(waiter) => {
                let waiter_state = unsafe { &*waiter.state };
                if waiter_state
                  .compare_exchange(
                    STATE_WAITING,
                    STATE_SUCCESS,
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                  )
                  .is_ok()
                {
                  waiter.thread.unpark();
                  break;
                }
              }
            }
          }
        }
      }

      return Ok(item);
    }

    // --- Priority 3: Check for disconnection ---
    if guard.sender_count == 0 {
      return Err(TryRecvError::Disconnected);
    }

    Err(TryRecvError::Empty)
  }

  /// The core non-blocking batch send: sends up to `limit` items from `iter`
  /// inside a single lock acquisition, waking satisfied receivers in one
  /// coalesced pass after the lock is released.
  ///
  /// Returns `(sent, reason)`: `reason` is `None` when all `limit` items were
  /// sent, `Some(Full)` when the channel ran out of space/receivers, and
  /// `Some(Closed)` when all receivers are gone (in which case `sent == 0`
  /// and no item was consumed from `iter`).
  pub(crate) fn try_send_batch_core<I: Iterator<Item = T>>(
    &self,
    iter: &mut I,
    limit: usize,
  ) -> (usize, Option<BatchSendErrorReason>) {
    if limit == 0 {
      return (0, None);
    }
    let mut to_wake: Vec<WakeRef> = Vec::new();
    let result = {
      let mut guard = self.internal.lock();

      if guard.receiver_count == 0 {
        (0, Some(BatchSendErrorReason::Closed))
      } else {
        let mut sent = 0;
        while sent < limit {
          // --- Priority 1: hand the item to a waiting receiver ---
          let mut handed = false;
          loop {
            match guard.waiting_async_receivers.pop_front() {
              None => break,
              Some(waiter) => {
                let waiter_state = unsafe { &*waiter.state };
                if waiter_state
                  .compare_exchange(
                    STATE_WAITING,
                    STATE_SUCCESS,
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                  )
                  .is_ok()
                {
                  guard.queue.push_back(iter.next().expect("iterator shorter than limit"));
                  to_wake.push(WakeRef::Waker(waiter.waker));
                  handed = true;
                  break;
                }
                // STATE_CANCELLED: discard and try next
              }
            }
          }
          if !handed {
            loop {
              match guard.waiting_sync_receivers.pop_front() {
                None => break,
                Some(waiter) => {
                  let waiter_state = unsafe { &*waiter.state };
                  if waiter_state
                    .compare_exchange(
                      STATE_WAITING,
                      STATE_SUCCESS,
                      Ordering::SeqCst,
                      Ordering::SeqCst,
                    )
                    .is_ok()
                  {
                    guard.queue.push_back(iter.next().expect("iterator shorter than limit"));
                    to_wake.push(WakeRef::Thread(waiter.thread));
                    handed = true;
                    break;
                  }
                }
              }
            }
          }
          if handed {
            sent += 1;
            continue;
          }

          // --- Priority 2: push to buffer if space is available ---
          if self.capacity == 0 {
            break; // Rendezvous: only direct handoffs above.
          }
          if self.capacity == usize::MAX || guard.queue.len() < self.capacity {
            guard.queue.push_back(iter.next().expect("iterator shorter than limit"));
            sent += 1;
          } else {
            break; // Full.
          }
        }

        let reason = if sent == limit {
          None
        } else {
          Some(BatchSendErrorReason::Full)
        };
        (sent, reason)
      }
    };
    // Wake outside the lock, in one coalesced pass.
    for w in to_wake {
      w.wake();
    }
    result
  }

  /// The core non-blocking batch receive: collects up to `max` items inside a
  /// single lock acquisition — first extracting payloads from waiting
  /// rendezvous senders, then draining the buffer, then waking up to as many
  /// buffered senders as buffer slots were freed. All wakes happen after the
  /// lock is released.
  pub(crate) fn try_recv_batch_core(
    &self,
    out: &mut Vec<T>,
    max: usize,
  ) -> Result<usize, TryRecvError> {
    if max == 0 {
      return Ok(0);
    }
    let mut to_wake: Vec<WakeRef> = Vec::new();
    let result = {
      let mut guard = self.internal.lock();
      let mut got = 0;

      // --- Priority 1: extract payloads from waiting rendezvous senders ---
      while got < max {
        if guard
          .waiting_async_senders
          .front()
          .map(|w| w.data.is_some())
          .unwrap_or(false)
        {
          let mut waiter = guard.waiting_async_senders.pop_front().unwrap();
          let waiter_state = unsafe { &*waiter.state };
          match waiter_state.compare_exchange(
            STATE_WAITING,
            STATE_SUCCESS,
            Ordering::SeqCst,
            Ordering::SeqCst,
          ) {
            Ok(_) => {
              out.push(waiter.take_item_from_sender_slot().unwrap());
              to_wake.push(WakeRef::Waker(waiter.waker));
              got += 1;
            }
            Err(_) => {
              drop(waiter.data.take()); // CANCELLED: drop payload, try next.
            }
          }
          continue;
        }
        if guard
          .waiting_sync_senders
          .front()
          .map(|w| w.data.is_some())
          .unwrap_or(false)
        {
          let mut waiter = guard.waiting_sync_senders.pop_front().unwrap();
          let waiter_state = unsafe { &*waiter.state };
          match waiter_state.compare_exchange(
            STATE_WAITING,
            STATE_SUCCESS,
            Ordering::SeqCst,
            Ordering::SeqCst,
          ) {
            Ok(_) => {
              out.push(waiter.take_item_from_sender_slot().unwrap());
              to_wake.push(WakeRef::Thread(waiter.thread));
              got += 1;
            }
            Err(_) => {
              drop(waiter.data.take());
            }
          }
          continue;
        }
        break;
      }

      // --- Priority 2: drain the main buffer ---
      let from_buffer = (max - got).min(guard.queue.len());
      if from_buffer > 0 {
        out.extend(guard.queue.drain(..from_buffer));
        got += from_buffer;
      }

      // --- Priority 3: wake up to `from_buffer` buffered senders ---
      // (They hold no payload; they wake, retry, and refill the freed space.
      // Rendezvous senders are only ever woken from Priority 1.)
      if self.capacity > 0 && from_buffer > 0 {
        let mut woken = 0;
        while woken < from_buffer {
          match guard.waiting_async_senders.pop_front() {
            None => break,
            Some(waiter) => {
              let waiter_state = unsafe { &*waiter.state };
              if waiter_state
                .compare_exchange(
                  STATE_WAITING,
                  STATE_SUCCESS,
                  Ordering::SeqCst,
                  Ordering::SeqCst,
                )
                .is_ok()
              {
                to_wake.push(WakeRef::Waker(waiter.waker));
                woken += 1;
              }
            }
          }
        }
        while woken < from_buffer {
          match guard.waiting_sync_senders.pop_front() {
            None => break,
            Some(waiter) => {
              let waiter_state = unsafe { &*waiter.state };
              if waiter_state
                .compare_exchange(
                  STATE_WAITING,
                  STATE_SUCCESS,
                  Ordering::SeqCst,
                  Ordering::SeqCst,
                )
                .is_ok()
              {
                to_wake.push(WakeRef::Thread(waiter.thread));
                woken += 1;
              }
            }
          }
        }
      }

      if got == 0 {
        if guard.sender_count == 0 {
          Err(TryRecvError::Disconnected)
        } else {
          Err(TryRecvError::Empty)
        }
      } else {
        Ok(got)
      }
    };
    for w in to_wake {
      w.wake();
    }
    result
  }

  /// Batch counterpart of `poll_recv_internal`: resolves with 1..=max items
  /// once anything is available, registering/updating the receiver waiter
  /// otherwise (same register-then-recheck protocol).
  pub(crate) fn poll_recv_batch_internal(
    &self,
    cx: &mut Context<'_>,
    state_ptr: *const AtomicU8,
    out: &mut Vec<T>,
    max: usize,
  ) -> Poll<Result<usize, RecvError>> {
    if max == 0 {
      return Poll::Ready(Ok(0));
    }
    'poll_loop: loop {
      // --- Phase 1: Try to receive without parking ---
      match self.try_recv_batch_core(out, max) {
        Ok(k) => return Poll::Ready(Ok(k)),
        Err(TryRecvError::Disconnected) => return Poll::Ready(Err(RecvError::Disconnected)),
        Err(TryRecvError::Empty) => { /* Proceed to park */ }
      }

      // --- Phase 2: Lock, re-check, and commit to parking ---
      {
        let mut guard = self.internal.lock();

        if !guard.queue.is_empty()
          || (self.capacity == 0
            && (!guard.waiting_sync_senders.is_empty() || !guard.waiting_async_senders.is_empty()))
        {
          drop(guard);
          continue 'poll_loop;
        }

        if guard.sender_count == 0 {
          return Poll::Ready(Err(RecvError::Disconnected));
        }

        let new_waker = cx.waker();

        if let Some(existing_waiter) = guard
          .waiting_async_receivers
          .iter_mut()
          .find(|w| w.state == state_ptr)
        {
          existing_waiter.waker = new_waker.clone();
        } else {
          unsafe { (*state_ptr).store(STATE_WAITING, Ordering::SeqCst) };
          let waiter = AsyncWaiter {
            waker: new_waker.clone(),
            data: None,
            state: state_ptr,
          };
          guard.waiting_async_receivers.push_back(waiter);
        }

        return Poll::Pending;
      }
    }
  }

  /// Inner polling logic used by both `RecvFuture` and `Stream for AsyncReceiver`.
  pub(crate) fn poll_recv_internal(
    &self,
    cx: &mut Context<'_>,
    state_ptr: *const AtomicU8,
  ) -> Poll<Result<T, RecvError>> {
    'poll_loop: loop {
      // --- Phase 1: Try to receive without parking ---
      match self.try_recv_core() {
        Ok(item) => {
          return Poll::Ready(Ok(item));
        }
        Err(TryRecvError::Disconnected) => return Poll::Ready(Err(RecvError::Disconnected)),
        Err(TryRecvError::Empty) => { /* Proceed to park */ }
      }

      // --- Phase 2: Lock, re-check, and commit to parking ---
      {
        let mut guard = self.internal.lock();

        if !guard.queue.is_empty()
          || (self.capacity == 0
            && (!guard.waiting_sync_senders.is_empty() || !guard.waiting_async_senders.is_empty()))
        {
          drop(guard);
          continue 'poll_loop;
        }

        if guard.sender_count == 0 {
          return Poll::Ready(Err(RecvError::Disconnected));
        }

        let new_waker = cx.waker();

        if let Some(existing_waiter) = guard
          .waiting_async_receivers
          .iter_mut()
          .find(|w| w.state == state_ptr)
        {
          existing_waiter.waker = new_waker.clone();
        } else {
          // Reset state to WAITING under lock before registering.
          // This clears any stale STATE_SUCCESS from a previous wake-up cycle
          // so the next sender's CAS(WAITING→SUCCESS) on this slot will succeed.
          unsafe { (*state_ptr).store(STATE_WAITING, Ordering::SeqCst) };
          let waiter = AsyncWaiter {
            waker: new_waker.clone(),
            data: None,
            state: state_ptr,
          };
          guard.waiting_async_receivers.push_back(waiter);
        }

        return Poll::Pending;
      }
    }
  }
}

impl<T> Drop for MpmcShared<T> {
  fn drop(&mut self) {
    // Safely bypass lock overhead using exclusive mutable access
    let guard = self.internal.get_mut();

    guard.queue.clear();
    for mut waiter in guard.waiting_sync_senders.drain(..) {
      let _ = waiter.take_item_from_sender_slot();
    }
    for mut waiter in guard.waiting_async_senders.drain(..) {
      let _ = waiter.take_item_from_sender_slot();
    }
  }
}
