//! The core shared data structures and logic for the MPMC channel.

use crate::error::{BatchSendErrorReason, TryRecvError, TrySendError};
use crate::internal::unsynchronized_ring::UnsynchronizedRingBuffer;
use crate::RecvError;
use core::task::{Context, Poll};
use std::collections::VecDeque;
use std::fmt;
use std::task::Waker;

use crate::internal::sync::{AtomicU8, Ordering, Thread};
use crate::sync::HybridMutex as Mutex;

// --- State Machine Constants ---
pub(crate) const STATE_WAITING: u8 = 0;
pub(crate) const STATE_SUCCESS_SPACE: u8 = 3;
pub(crate) const STATE_CLOSED_BUFFERED: u8 = 1;
pub(crate) const STATE_CLOSED_RENDEZVOUS: u8 = 5;
pub(crate) const STATE_CANCELLED: u8 = 8;

// --- Waiter Structs ---

/// Represents a parked synchronous thread waiting for an operation to complete.
#[derive(Debug)]
pub(crate) struct SyncWaiter {
  pub(crate) thread: Thread,
  pub(crate) state: *const AtomicU8,
}

unsafe impl Send for SyncWaiter {}
unsafe impl Sync for SyncWaiter {}

/// Represents a parked asynchronous task waiting for an operation to complete.
#[derive(Debug)]
pub(crate) struct AsyncWaiter {
  pub(crate) waker: Waker,
  pub(crate) state: *const AtomicU8,
}

unsafe impl Send for AsyncWaiter {}
unsafe impl Sync for AsyncWaiter {}

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
  /// The circular ring buffer storage for items in the channel.
  pub(crate) queue: UnsynchronizedRingBuffer<T>,
  /// Cached count of items in the queue (prevents enum branching on invariants).
  pub(crate) queue_len: usize,
  /// Queue of parked synchronous senders.
  pub(crate) waiting_sync_senders: VecDeque<SyncWaiter>,
  /// Queue of parked asynchronous senders.
  pub(crate) waiting_async_senders: VecDeque<AsyncWaiter>,
  /// Queue of parked synchronous receivers.
  pub(crate) waiting_sync_receivers: VecDeque<SyncWaiter>,
  /// Queue of parked asynchronous receivers.
  pub(crate) waiting_async_receivers: VecDeque<AsyncWaiter>,
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
    match self.queue.push(item) {
      Ok(()) => {
        self.queue_len += 1;
        Ok(())
      }
      Err(returned_item) => Err(returned_item),
    }
  }

  #[inline]
  pub(crate) fn pop_front(&mut self) -> Option<T> {
    if let Some(item) = self.queue.pop() {
      self.queue_len -= 1;
      Some(item)
    } else {
      None
    }
  }

  #[inline]
  pub(crate) fn drain_into(&mut self, out: &mut Vec<T>, count: usize) {
    for _ in 0..count {
      if let Some(item) = self.queue.pop() {
        out.push(item);
      }
    }
    self.queue_len -= count;
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
  pub(crate) fn new(capacity: usize) -> Self {
    let queue = UnsynchronizedRingBuffer::new(capacity);

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
    if guard.len() < self.capacity {
      guard.push_back(item).ok().expect("MPMC push_back failed");
      return Ok(());
    }

    Err(TrySendError::Full(item))
  }

  pub(crate) fn try_recv_core(&self) -> Result<T, TryRecvError> {
    let mut guard = self.internal.lock();

    // --- Priority 1: Check the main buffer first (Preserves strict FIFO) ---
    if let Some(item) = guard.pop_front() {
      if self.capacity > 0 {
        let mut i = 0;
        while i < guard.waiting_async_senders.len() {
          let waiter = &guard.waiting_async_senders[i];
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
            let waiter = guard.waiting_async_senders.remove(i).unwrap();
            waiter.waker.wake();
            return Ok(item);
          } else {
            i += 1;
          }
        }

        let mut i = 0;
        while i < guard.waiting_sync_senders.len() {
          let waiter = &guard.waiting_sync_senders[i];
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
            let waiter = guard.waiting_sync_senders.remove(i).unwrap();
            waiter.thread.unpark();
            return Ok(item);
          } else {
            i += 1;
          }
        }
      }

      return Ok(item);
    }

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
    let mut guard = self.internal.lock();

    if guard.receiver_count == 0 {
      return (0, Some(BatchSendErrorReason::Closed));
    }

    let mut sent = 0;
    while sent < limit {
      // --- Priority 1: hand the item to a waiting receiver (wake inside lock) ---
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
              waiter.waker.wake();
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
                waiter.thread.unpark();
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
      if guard.len() < self.capacity {
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

  pub(crate) fn try_recv_batch_core(
    &self,
    out: &mut Vec<T>,
    max: usize,
  ) -> Result<usize, TryRecvError> {
    if max == 0 {
      return Ok(0);
    }
    let mut guard = self.internal.lock();
    let mut got = 0;

    // --- Priority 1: Drain the main buffer first (FIFO preserved) ---
    let from_buffer = (max - got).min(guard.len());
    if from_buffer > 0 {
      guard.drain_into(out, from_buffer);
      got += from_buffer;
    }

    // --- Priority 2: Wake buffered senders (inside the lock) ---
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
            STATE_SUCCESS_SPACE,
            Ordering::SeqCst,
            Ordering::SeqCst,
          )
          .is_ok()
        {
          let waiter = guard.waiting_async_senders.remove(i).unwrap();
          waiter.waker.wake();
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
            STATE_SUCCESS_SPACE,
            Ordering::SeqCst,
            Ordering::SeqCst,
          )
          .is_ok()
        {
          let waiter = guard.waiting_sync_senders.remove(i).unwrap();
          waiter.thread.unpark();
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
    guard.waiting_sync_senders.clear();
    guard.waiting_async_senders.clear();
  }
}
