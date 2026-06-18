//! The core shared data structures and logic for the MPMC channel.

use crate::error::{BatchSendErrorReason, TryRecvError, TrySendError};
use crate::internal::unsynchronized_ring::UnsynchronizedRingBuffer;
use crate::RecvError;
use core::task::{Context, Poll};
use parking_lot::Mutex;
use std::collections::VecDeque;
use std::fmt;
use std::sync::atomic::{AtomicU8, Ordering};
use std::task::Waker;
use std::thread::Thread;

// --- State Machine Constants ---
pub(crate) const STATE_WAITING: u8 = 0;
pub(crate) const STATE_SUCCESS_SPACE: u8 = 3;
pub(crate) const STATE_SUCCESS_HANDOFF: u8 = 7;
pub(crate) const STATE_CLOSED_BUFFERED: u8 = 1;
pub(crate) const STATE_CLOSED_RENDEZVOUS: u8 = 5;
pub(crate) const STATE_CANCELLED: u8 = 8;

// --- Storage Strategy Enum ---
/// Delegates channel storage.
///
/// Under unbounded configurations, it falls back to a standard library `VecDeque`
/// to allow heap growth. Under bounded configurations, it uses the high-performance
/// unsynchronized ring buffer.
pub(crate) enum Storage<T> {
  Bounded(UnsynchronizedRingBuffer<T>),
  Unbounded(VecDeque<T>),
  Rendezvous(Option<T>), // Stack-allocated single-slot handoff mailbox
}

impl<T> Storage<T> {
  #[inline]
  pub(crate) fn len(&self) -> usize {
    match self {
      Self::Bounded(ring) => ring.len(),
      Self::Unbounded(deque) => deque.len(),
      Self::Rendezvous(slot) => {
        if slot.is_some() {
          1
        } else {
          0
        }
      }
    }
  }

  #[inline]
  pub(crate) fn is_empty(&self) -> bool {
    match self {
      Self::Bounded(ring) => ring.is_empty(),
      Self::Unbounded(deque) => deque.is_empty(),
      Self::Rendezvous(slot) => slot.is_none(),
    }
  }

  #[inline]
  pub(crate) fn push_back(&mut self, item: T) -> Result<(), T> {
    match self {
      Self::Bounded(ring) => ring.push(item),
      Self::Unbounded(deque) => {
        deque.push_back(item);
        Ok(())
      }
      Self::Rendezvous(slot) => {
        if slot.is_none() {
          *slot = Some(item);
          Ok(())
        } else {
          Err(item) // Slot already occupied by an in-flight handoff
        }
      }
    }
  }

  #[inline]
  pub(crate) fn pop_front(&mut self) -> Option<T> {
    match self {
      Self::Bounded(ring) => ring.pop(),
      Self::Unbounded(deque) => deque.pop_front(),
      Self::Rendezvous(slot) => slot.take(), // Extract the in-flight handoff item
    }
  }

  #[inline]
  pub(crate) fn drain_into(&mut self, out: &mut Vec<T>, count: usize) {
    match self {
      Self::Bounded(ring) => {
        for _ in 0..count {
          if let Some(item) = ring.pop() {
            out.push(item);
          }
        }
      }
      Self::Unbounded(deque) => {
        out.extend(deque.drain(..count));
      }
      Self::Rendezvous(slot) => {
        if count > 0 {
          if let Some(item) = slot.take() {
            out.push(item);
          }
        }
      }
    }
  }

  #[inline]
  pub(crate) fn clear(&mut self) {
    match self {
      Self::Bounded(ring) => ring.clear(),
      Self::Unbounded(deque) => deque.clear(),
      Self::Rendezvous(slot) => {
        let _ = slot.take();
      }
    }
  }

  #[inline]
  pub(crate) fn is_full(&self) -> bool {
    match self {
      Self::Bounded(ring) => ring.is_full(),
      Self::Unbounded(_) => false, // Unbounded queue is never full
      Self::Rendezvous(slot) => slot.is_some(), // Full if the single-slot mailbox is occupied
    }
  }
}

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
pub(crate) struct MpmcChannelInternal<T> {
  /// The circular storage delegate for items in the channel.
  pub(crate) queue: Storage<T>,
  /// Cached count of items in the queue (prevents enum branching on invariants).
  pub(crate) queue_len: usize,
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

impl<T> fmt::Debug for MpmcChannelInternal<T> {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("MpmcChannelInternal")
      .field("queue_len", &self.queue.len())
      .field("waiting_sync_senders", &self.waiting_sync_senders.len())
      .field("waiting_async_senders", &self.waiting_async_senders.len())
      .field("waiting_sync_receivers", &self.waiting_sync_receivers.len())
      .field(
        "waiting_async_receivers",
        &self.waiting_async_receivers.len(),
      )
      .field("sender_count", &self.sender_count)
      .field("receiver_count", &self.receiver_count)
      .finish()
  }
}

impl<T> MpmcChannelInternal<T> {
  #[inline]
  pub(crate) fn len(&self) -> usize {
    self.queue_len
  }

  #[inline]
  pub(crate) fn is_empty(&self) -> bool {
    self.queue_len == 0
  }

  #[inline]
  pub(crate) fn is_full(&self, capacity: usize) -> bool {
    if capacity == 0 {
      self.queue_len == 1 // Rendezvous is full if the single-slot mailbox is occupied
    } else {
      self.queue_len == capacity
    }
  }

  #[inline]
  pub(crate) fn push_back(&mut self, item: T) -> Result<(), T> {
    match self.queue.push_back(item) {
      Ok(()) => {
        self.queue_len += 1;
        Ok(())
      }
      Err(returned_item) => Err(returned_item),
    }
  }

  #[inline]
  pub(crate) fn pop_front(&mut self) -> Option<T> {
    if let Some(item) = self.queue.pop_front() {
      self.queue_len -= 1;
      Some(item)
    } else {
      None
    }
  }

  #[inline]
  pub(crate) fn drain_into(&mut self, out: &mut Vec<T>, count: usize) {
    self.queue.drain_into(out, count);
    self.queue_len -= count;
  }

  #[inline]
  pub(crate) fn clear(&mut self) {
    self.queue.clear();
    self.queue_len = 0;
  }
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
  /// Creates a new shared core for the channel with a given capacity.
  pub(crate) fn new(capacity: usize) -> Self {
    let queue = if capacity == usize::MAX {
      Storage::Unbounded(VecDeque::new())
    } else if capacity == 0 {
      Storage::Rendezvous(None) // <-- Allocates ZERO heap memory
    } else {
      Storage::Bounded(UnsynchronizedRingBuffer::new(capacity))
    };

    MpmcShared {
      internal: Mutex::new(MpmcChannelInternal {
        queue,
        queue_len: 0,
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

  pub(crate) fn try_send_core(&self, item: T) -> Result<(), TrySendError<T>> {
    let mut guard = self.internal.lock();

    if guard.receiver_count == 0 {
      return Err(TrySendError::Closed(item));
    }

    // --- Priority 1: Wake a waiting receiver ---
    let mut i = 0;
    while i < guard.waiting_async_receivers.len() && !guard.is_full(self.capacity) {
      let waiter = &guard.waiting_async_receivers[i];
      let waiter_state = unsafe { &*waiter.state };
      if waiter_state
        .compare_exchange(
          STATE_WAITING,
          STATE_SUCCESS_SPACE,
          Ordering::SeqCst,
          Ordering::SeqCst,
        )
        .is_ok()
      {
        let waiter = guard.waiting_async_receivers.remove(i).unwrap();
        guard.push_back(item).ok().expect("MPMC push_back failed");
        waiter.waker.wake();
        return Ok(());
      } else {
        i += 1;
      }
    }

    let mut i = 0;
    while i < guard.waiting_sync_receivers.len() && !guard.is_full(self.capacity) {
      let waiter = &guard.waiting_sync_receivers[i];
      let waiter_state = unsafe { &*waiter.state };
      if waiter_state
        .compare_exchange(
          STATE_WAITING,
          STATE_SUCCESS_SPACE,
          Ordering::SeqCst,
          Ordering::SeqCst,
        )
        .is_ok()
      {
        let waiter = guard.waiting_sync_receivers.remove(i).unwrap();
        guard.push_back(item).ok().expect("MPMC push_back failed");
        waiter.thread.unpark();
        return Ok(());
      } else {
        i += 1;
      }
    }

    // --- Priority 2: Push to buffer if space is available ---
    if self.capacity == 0 {
      return Err(TrySendError::Full(item));
    }
    if self.capacity == usize::MAX || guard.len() < self.capacity {
      guard.push_back(item).ok().expect("MPMC push_back failed");
      return Ok(());
    }

    Err(TrySendError::Full(item))
  }

  pub(crate) fn try_recv_core(&self) -> Result<T, TryRecvError> {
    let mut guard = self.internal.lock();

    // --- Priority 1: Check the main buffer first (Preserves strict FIFO) ---
    if let Some(item) = guard.pop_front() {
      let mut to_wake = None;
      if self.capacity > 0 {
        let mut i = 0;
        while i < guard.waiting_async_senders.len() {
          let waiter = &guard.waiting_async_senders[i];
          let waiter_state = unsafe { &*waiter.state };
          if waiter_state
            .compare_exchange(
              STATE_WAITING,
              STATE_SUCCESS_HANDOFF, // Item successfully taken
              Ordering::SeqCst,
              Ordering::SeqCst,
            )
            .is_ok()
          {
            let mut waiter = guard.waiting_async_senders.remove(i).unwrap();
            if let Some(sender_item) = waiter.take_item_from_sender_slot() {
              guard
                .push_back(sender_item)
                .ok()
                .expect("MPMC push_back failed");
            }
            to_wake = Some(WakeRef::Waker(waiter.waker));
            break;
          } else {
            i += 1;
          }
        }

        if to_wake.is_none() {
          let mut i = 0;
          while i < guard.waiting_sync_senders.len() {
            let waiter = &guard.waiting_sync_senders[i];
            let waiter_state = unsafe { &*waiter.state };
            if waiter_state
              .compare_exchange(
                STATE_WAITING,
                STATE_SUCCESS_HANDOFF, // Item successfully taken
                Ordering::SeqCst,
                Ordering::SeqCst,
              )
              .is_ok()
            {
              let mut waiter = guard.waiting_sync_senders.remove(i).unwrap();
              if let Some(sender_item) = waiter.take_item_from_sender_slot() {
                guard
                  .push_back(sender_item)
                  .ok()
                  .expect("MPMC push_back failed");
              }
              to_wake = Some(WakeRef::Thread(waiter.thread));
              break;
            } else {
              i += 1;
            }
          }
        }
      }

      drop(guard);
      if let Some(w) = to_wake {
        w.wake();
      }
      return Ok(item);
    }

    // --- Priority 2: Check for a waiting rendezvous sender (Direct Handoff) ---
    let mut i = 0;
    while i < guard.waiting_async_senders.len() {
      let waiter = &guard.waiting_async_senders[i];
      if waiter.data.is_some() {
        let waiter_state = unsafe { &*waiter.state };
        if waiter_state
          .compare_exchange(
            STATE_WAITING,
            STATE_SUCCESS_HANDOFF,
            Ordering::SeqCst,
            Ordering::SeqCst,
          )
          .is_ok()
        {
          let mut waiter = guard.waiting_async_senders.remove(i).unwrap();
          let item = waiter.take_item_from_sender_slot().unwrap();
          drop(guard);
          waiter.waker.wake();
          return Ok(item);
        }
      }
      i += 1;
    }

    let mut i = 0;
    while i < guard.waiting_sync_senders.len() {
      let waiter = &guard.waiting_sync_senders[i];
      if waiter.data.is_some() {
        let waiter_state = unsafe { &*waiter.state };
        if waiter_state
          .compare_exchange(
            STATE_WAITING,
            STATE_SUCCESS_HANDOFF,
            Ordering::SeqCst,
            Ordering::SeqCst,
          )
          .is_ok()
        {
          let mut waiter = guard.waiting_sync_senders.remove(i).unwrap();
          let item = waiter.take_item_from_sender_slot().unwrap();
          drop(guard);
          waiter.thread.unpark();
          return Ok(item);
        }
      }
      i += 1;
    }

    // --- Priority 3: Check for disconnection ---
    if guard.sender_count == 0 {
      return Err(TryRecvError::Disconnected);
    }

    Err(TryRecvError::Empty)
  }

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
            if guard.is_full(self.capacity) {
              break;
            }

            match guard.waiting_async_receivers.pop_front() {
              None => break,
              Some(waiter) => {
                let waiter_state = unsafe { &*waiter.state };
                if waiter_state
                  .compare_exchange(
                    STATE_WAITING,
                    STATE_SUCCESS_SPACE,
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                  )
                  .is_ok()
                {
                  let item = iter.next().expect("iterator shorter than limit");
                  guard.push_back(item).ok().expect("MPMC push_back failed");
                  to_wake.push(WakeRef::Waker(waiter.waker));
                  handed = true;
                  break;
                }
              }
            }
          }
          if !handed {
            loop {
              if guard.is_full(self.capacity) {
                break;
              }

              match guard.waiting_sync_receivers.pop_front() {
                None => break,
                Some(waiter) => {
                  let waiter_state = unsafe { &*waiter.state };
                  if waiter_state
                    .compare_exchange(
                      STATE_WAITING,
                      STATE_SUCCESS_SPACE,
                      Ordering::SeqCst,
                      Ordering::SeqCst,
                    )
                    .is_ok()
                  {
                    let item = iter.next().expect("iterator shorter than limit");
                    guard.push_back(item).ok().expect("MPMC push_back failed");
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
            break;
          }
          if self.capacity == usize::MAX || guard.len() < self.capacity {
            let item = iter.next().expect("iterator shorter than limit");
            guard.push_back(item).ok().expect("MPMC push_back failed");
            sent += 1;
          } else {
            break;
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

    for w in to_wake {
      w.wake();
    }
    result
  }

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

      // --- Priority 1: Drain the main buffer first (FIFO preserved) ---
      let from_buffer = (max - got).min(guard.len());
      if from_buffer > 0 {
        guard.drain_into(out, from_buffer);
        got += from_buffer;
      }

      // --- Priority 2: Extract payloads from waiting rendezvous senders ---
      let mut i = 0;
      while got < max && i < guard.waiting_async_senders.len() {
        let waiter = &guard.waiting_async_senders[i];
        if waiter.data.is_some() {
          let waiter_state = unsafe { &*waiter.state };
          if waiter_state
            .compare_exchange(
              STATE_WAITING,
              STATE_SUCCESS_HANDOFF,
              Ordering::SeqCst,
              Ordering::SeqCst,
            )
            .is_ok()
          {
            let mut waiter = guard.waiting_async_senders.remove(i).unwrap();
            out.push(waiter.take_item_from_sender_slot().unwrap());
            to_wake.push(WakeRef::Waker(waiter.waker));
            got += 1;
            continue;
          }
        }
        i += 1;
      }

      let mut i = 0;
      while got < max && i < guard.waiting_sync_senders.len() {
        let waiter = &guard.waiting_sync_senders[i];
        if waiter.data.is_some() {
          let waiter_state = unsafe { &*waiter.state };
          if waiter_state
            .compare_exchange(
              STATE_WAITING,
              STATE_SUCCESS_HANDOFF,
              Ordering::SeqCst,
              Ordering::SeqCst,
            )
            .is_ok()
          {
            let mut waiter = guard.waiting_sync_senders.remove(i).unwrap();
            out.push(waiter.take_item_from_sender_slot().unwrap());
            to_wake.push(WakeRef::Thread(waiter.thread));
            got += 1;
            continue;
          }
        }
        i += 1;
      }

      // --- Priority 3: Wake and backfill from buffered senders ---
      if self.capacity > 0 && from_buffer > 0 {
        let mut woken = 0;
        let max_to_wake = from_buffer + (max - got);

        let mut i = 0;
        while i < guard.waiting_async_senders.len() && woken < max_to_wake {
          let waiter = &guard.waiting_async_senders[i];
          let waiter_state = unsafe { &*waiter.state };
          if waiter_state
            .compare_exchange(
              STATE_WAITING,
              STATE_SUCCESS_HANDOFF,
              Ordering::SeqCst,
              Ordering::SeqCst,
            )
            .is_ok()
          {
            let mut waiter = guard.waiting_async_senders.remove(i).unwrap();
            if let Some(sender_item) = waiter.take_item_from_sender_slot() {
              if got < max {
                out.push(sender_item);
                got += 1;
              } else {
                guard
                  .push_back(sender_item)
                  .ok()
                  .expect("MPMC push_back failed");
              }
            }
            to_wake.push(WakeRef::Waker(waiter.waker));
            woken += 1;
          } else {
            i += 1;
          }
        }

        let mut i = 0;
        while i < guard.waiting_sync_senders.len() && woken < max_to_wake {
          let waiter = &guard.waiting_sync_senders[i];
          let waiter_state = unsafe { &*waiter.state };
          if waiter_state
            .compare_exchange(
              STATE_WAITING,
              STATE_SUCCESS_HANDOFF,
              Ordering::SeqCst,
              Ordering::SeqCst,
            )
            .is_ok()
          {
            let mut waiter = guard.waiting_sync_senders.remove(i).unwrap();
            if let Some(sender_item) = waiter.take_item_from_sender_slot() {
              if got < max {
                out.push(sender_item);
                got += 1;
              } else {
                guard
                  .push_back(sender_item)
                  .ok()
                  .expect("MPMC push_back failed");
              }
            }
            to_wake.push(WakeRef::Thread(waiter.thread));
            woken += 1;
          } else {
            i += 1;
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

  /// Batch counterpart of `poll_recv_internal`.
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
      match self.try_recv_batch_core(out, max) {
        Ok(k) => return Poll::Ready(Ok(k)),
        Err(TryRecvError::Disconnected) => return Poll::Ready(Err(RecvError::Disconnected)),
        Err(TryRecvError::Empty) => {}
      }

      {
        let mut guard = self.internal.lock();

        if !guard.is_empty()
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
      match self.try_recv_core() {
        Ok(item) => {
          return Poll::Ready(Ok(item));
        }
        Err(TryRecvError::Disconnected) => return Poll::Ready(Err(RecvError::Disconnected)),
        Err(TryRecvError::Empty) => {}
      }

      {
        let mut guard = self.internal.lock();

        if !guard.is_empty()
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
}

impl<T> Drop for MpmcShared<T> {
  fn drop(&mut self) {
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
