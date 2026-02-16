//! The core shared data structures and logic for the MPMC channel.
//!
//! This module contains the `MpmcShared` struct which holds the central mutex-protected
//! state of the channel. To support mixed synchronous and asynchronous operations
//! (e.g., a sync producer sending to an async consumer), the design separates waiters
//! into distinct queues based on their type.
//!
//! ### Design Principles:
//!
//! 1.  **Central Mutex**: A `parking_lot::Mutex` guards all state changes, ensuring safety
//!     and correctness at the cost of some contention in highly parallel scenarios.
//! 2.  **Separate Waiter Queues**: Instead of a single queue with a `Waiter` enum, we have
//!     four distinct queues: `waiting_sync_senders`, `waiting_async_senders`,
//!     `waiting_sync_receivers`, and `waiting_async_receivers`. This allows the core
//!     logic to wake the correct type of parker (`thread::unpark` for sync, `waker.wake()`
//!     for async) without ambiguity.
//! 3.  **Wake-up Priority**: When an operation frees up a resource (e.g., a `send`
//!     provides an item, or a `recv` creates space), the logic prioritizes waking an
//!     existing parked task/thread over performing another action (like pushing to the
//!     buffer). Async waiters are generally prioritized as they are lower overhead to wake.

use crate::error::{TryRecvError, TrySendError};
use crate::internal::blocked_deque::BlockedVecDeque;
use crate::internal::cache_padded::CachePadded;
use crate::RecvError;
use core::task::{Context, Poll};
use std::collections::VecDeque;
use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::task::Waker;
use std::thread::Thread;

// --- Waiter & Data Structs ---

/// Represents a parked synchronous sender waiting for space or a receiver.
#[derive(Debug)]
pub(crate) struct SyncSenderWaiter<T> {
  /// The handle to the parked thread, used for `unpark()`.
  pub(crate) thread: Thread,
  /// A slot for a rendezvous item. `None` for buffered waiters.
  pub(crate) data: Option<T>,
  /// An atomic flag used by the adaptive backoff strategy to detect when the
  /// wait is over, preventing the thread from re-parking unnecessarily.
  pub(crate) done: *const CachePadded<AtomicBool>,
}

// The raw pointer `done` is safe to send across threads because the thread that owns
// the data pointed to is blocked and cannot return until the pointer is used by the
// waking thread.
unsafe impl<T: Send> Send for SyncSenderWaiter<T> {}
unsafe impl<T: Send> Sync for SyncSenderWaiter<T> {}

impl<T> SyncSenderWaiter<T> {
  /// Takes the item from a rendezvous sender's waiter slot.
  pub(crate) fn take_item_from_sender_slot(&mut self) -> Option<T> {
    self.data.take()
  }
}

/// Represents a parked synchronous receiver waiting for an item.
#[derive(Debug)]
pub(crate) struct SyncReceiverWaiter {
  pub(crate) thread: Thread,
  pub(crate) done: *const CachePadded<AtomicBool>,
}

// See SyncSenderWaiter for safety justification.
unsafe impl Send for SyncReceiverWaiter {}
unsafe impl Sync for SyncReceiverWaiter {}

/// Represents a parked asynchronous sender waiting for space or a receiver.
#[derive(Debug)]
pub(crate) struct AsyncSenderWaiter<T> {
  /// The waker for the parked future, used for `wake()`.
  pub(crate) waker: Waker,
  /// A slot for a rendezvous item. `None` for buffered waiters.
  pub(crate) data: Option<T>,
}

impl<T> AsyncSenderWaiter<T> {
  /// Takes the item from a rendezvous sender's waiter slot.
  pub(crate) fn take_item_from_sender_slot(&mut self) -> Option<T> {
    self.data.take()
  }
}

/// Represents a parked asynchronous receiver waiting for an item.
#[derive(Debug)]
pub(crate) struct AsyncReceiverWaiter {
  pub(crate) waker: Waker,
}

/// The core state of the MPMC channel, protected by a single `Mutex`.
#[derive(Debug)]
pub(crate) struct MpmcChannelInternal<T> {
  /// The primary buffer for items when the channel has a capacity > 0.
  pub(crate) queue: VecDeque<T>,
  /// Queue of parked synchronous senders.
  pub(crate) waiting_sync_senders: BlockedVecDeque<SyncSenderWaiter<T>, 16>,
  /// Queue of parked asynchronous senders.
  pub(crate) waiting_async_senders: BlockedVecDeque<AsyncSenderWaiter<T>, 16>,
  /// Queue of parked synchronous receivers.
  pub(crate) waiting_sync_receivers: BlockedVecDeque<SyncReceiverWaiter, 16>,
  /// Queue of parked asynchronous receivers.
  pub(crate) waiting_async_receivers: BlockedVecDeque<AsyncReceiverWaiter, 16>,
  /// The number of active `Sender` and `AsyncSender` handles.
  pub(crate) sender_count: usize,
  /// The number of active `Receiver` and `AsyncReceiver` handles.
  pub(crate) receiver_count: usize,
}

type TrySendFn<T> = fn(&MpmcShared<T>, T) -> Result<(), TrySendError<T>>;
type TryRecvFn<T> = fn(&MpmcShared<T>) -> Result<T, TryRecvError>;

/// The shared owner of the channel's internal state, designed to be wrapped in an `Arc`.
#[derive(Debug)]
pub(crate) struct MpmcShared<T> {
  pub(crate) internal: CachePadded<Mutex<MpmcChannelInternal<T>>>,
  pub(crate) capacity: usize,
  pub(crate) try_send: TrySendFn<T>,
  pub(crate) try_recv: TryRecvFn<T>,
  pub(crate) sender_count: AtomicUsize,
}

// It is safe to send MpmcShared across threads if T is Send.
// The internal Mutex ensures that access to the shared state is properly synchronized.
unsafe impl<T: Send> Send for MpmcShared<T> {}
unsafe impl<T: Send> Sync for MpmcShared<T> {}

impl<T: Send> MpmcShared<T> {
  /// Creates a new shared core for the channel with a given capacity.
  /// `usize::MAX` is used to signify an "unbounded" channel.
  pub(crate) fn new(capacity: usize) -> Self {
    let (try_send, try_recv) = if capacity == 0 {
      (try_send_rendezvous::<T> as TrySendFn<T>, try_recv_rendezvous::<T> as TryRecvFn<T>)
    } else if capacity == usize::MAX {
      (try_send_unbounded::<T> as TrySendFn<T>, try_recv_unbounded::<T> as TryRecvFn<T>)
    } else {
      (try_send_buffered::<T> as TrySendFn<T>, try_recv_buffered::<T> as TryRecvFn<T>)
    };

    const PREALLOC_LIMIT: usize = 10240;

    let queue = {
      if capacity > 0 && capacity < PREALLOC_LIMIT {
        VecDeque::with_capacity(capacity)
      } else {
        VecDeque::new()
      }
    };

    MpmcShared {
      internal: CachePadded::new(Mutex::new(MpmcChannelInternal {
        queue,
        waiting_sync_senders: BlockedVecDeque::new(),
        waiting_async_senders: BlockedVecDeque::new(),
        waiting_sync_receivers: BlockedVecDeque::new(),
        waiting_async_receivers: BlockedVecDeque::new(),
        sender_count: 1, // Starts with one producer and one consumer
        receiver_count: 1,
      })),
      capacity,
      try_send,
      try_recv,
      sender_count: AtomicUsize::new(1),
    }
  }

  /// The core logic for attempting to send an item. This will try, in order:
  /// 1. Hand the item to a waiting receiver (async first, then sync).
  /// 2. Push the item into the buffer if there is space.
  /// If neither is possible, it returns a `TrySendError`.
  pub(crate) fn try_send_core(&self, item: T) -> Result<(), TrySendError<T>> {
    (self.try_send)(self, item)
  }

  /// The core logic for attempting to receive an item. This will try, in order:
  /// 1. Take an item from a waiting rendezvous sender (async first, then sync).
  /// 2. Take an item from the buffer. If successful, it may wake a waiting buffered sender.
  /// If neither is possible, it returns a `TryRecvError`.
  pub(crate) fn try_recv_core(&self) -> Result<T, TryRecvError> {
    (self.try_recv)(self)
  }

  pub(crate) fn poll_recv_internal(&self, cx: &mut Context<'_>) -> Poll<Result<T, RecvError>> {
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

        // Re-check under lock. An item might have appeared.
        // An item is available if:
        // 1. The queue is not empty (buffered or rendezvous after hand-off)
        // 2. It's a rendezvous channel and a sender is waiting to hand an item over.
        if !guard.queue.is_empty()
          || (self.capacity == 0
            && (!guard.waiting_sync_senders.is_empty() || !guard.waiting_async_senders.is_empty()))
        {
          drop(guard);
          continue 'poll_loop; // Retry immediately.
        }

        if guard.sender_count == 0 {
          return Poll::Ready(Err(RecvError::Disconnected));
        }

        let new_waker = cx.waker();

        let mut found_existing = false;
        for existing_waiter in guard.waiting_async_receivers.iter_mut() {
          if existing_waiter.waker.will_wake(new_waker) {
            existing_waiter.waker = new_waker.clone(); // Update in case it changed
            found_existing = true;
            break;
          }
        }
        if !found_existing {
          let waiter = AsyncReceiverWaiter {
            waker: new_waker.clone(),
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
    // When the Arc<MpmcShared> is finally dropped (i.e., all Senders and
    // Receivers are gone), we must drop any items remaining in the channel
    // to prevent memory leaks.
    if let Some(mut guard) = self.internal.try_lock() {
      // Drain the main queue.
      guard.queue.clear();

      // Drain any parked senders, which might contain items for rendezvous channels.
      // Collect into a Vec to ensure all items are processed and dropped.
      let _ = guard.waiting_sync_senders.drain().collect::<Vec<_>>();
      let _ = guard.waiting_async_senders.drain().collect::<Vec<_>>();
    }
    // If the lock is poisoned, the data inside might not be properly dropped,
    // but this is an unrecoverable panic state anyway.
  }
}

// --- Strategy Implementations ---

fn try_send_rendezvous<T: Send>(shared: &MpmcShared<T>, item: T) -> Result<(), TrySendError<T>> {
  let mut guard = shared.internal.lock();
  if guard.receiver_count == 0 {
    return Err(TrySendError::Closed(item));
  }
  // Priority 1: Wake receiver
  if let Some(waiter) = guard.waiting_async_receivers.pop_front() {
    guard.queue.push_back(item);
    waiter.waker.wake();
    return Ok(());
  }
  if let Some(waiter) = guard.waiting_sync_receivers.pop_front() {
    guard.queue.push_back(item);
    // Safety: The receiving thread is parked and waiting on this flag. The stack
    // frame it's on is guaranteed to be valid.
    unsafe { (*waiter.done).store(true, Ordering::Release) };
    waiter.thread.unpark();
    return Ok(());
  }
  // Priority 2: Buffer (none for rendezvous)
  Err(TrySendError::Full(item))
}

fn try_send_buffered<T: Send>(shared: &MpmcShared<T>, item: T) -> Result<(), TrySendError<T>> {
  let mut guard = shared.internal.lock();
  if guard.receiver_count == 0 {
    return Err(TrySendError::Closed(item));
  }

  // Determine if we can send: either a receiver is waiting, or there's space.
  let can_send = !guard.waiting_async_receivers.is_empty()
    || !guard.waiting_sync_receivers.is_empty()
    || guard.queue.len() < shared.capacity;

  if can_send {
    // Wake a receiver if one is waiting.
    if let Some(waiter) = guard.waiting_async_receivers.pop_front() {
      waiter.waker.wake();
    } else if let Some(waiter) = guard.waiting_sync_receivers.pop_front() {
      unsafe { (*waiter.done).store(true, Ordering::Release) };
      waiter.thread.unpark();
    }

    // Push the item to the queue.
    guard.queue.push_back(item);
    Ok(())
  } else {
    Err(TrySendError::Full(item))
  }
}

fn try_send_unbounded<T: Send>(shared: &MpmcShared<T>, item: T) -> Result<(), TrySendError<T>> {
  let mut guard = shared.internal.lock();
  if guard.receiver_count == 0 {
    return Err(TrySendError::Closed(item));
  }

  // If a receiver is waiting, wake them. The item will be pushed to the queue for them.
  if let Some(waiter) = guard.waiting_async_receivers.pop_front() {
    waiter.waker.wake();
  } else if let Some(waiter) = guard.waiting_sync_receivers.pop_front() {
    unsafe { (*waiter.done).store(true, Ordering::Release) };
    waiter.thread.unpark();
  }

  // For an unbounded channel, the item is always accepted. It's either for a
  // receiver we just woke, or for a future receiver.
  guard.queue.push_back(item);
  Ok(())
}

fn try_recv_rendezvous<T: Send>(shared: &MpmcShared<T>) -> Result<T, TryRecvError> {
  let mut guard = shared.internal.lock();
  // Priority 1: Check senders
  // We check async senders first, then sync senders.
  // If we find a valid item, we wake the sender and return the item.
  if let Some(mut waiter) = guard.waiting_async_senders.pop_front() {
      if let Some(item) = waiter.take_item_from_sender_slot() {
          waiter.waker.wake();
          return Ok(item);
      }
      guard.waiting_async_senders.push_front(waiter);
  } else if let Some(mut waiter) = guard.waiting_sync_senders.pop_front() {
      if let Some(item) = waiter.take_item_from_sender_slot() {
          unsafe { (*waiter.done).store(true, Ordering::Release) };
          waiter.thread.unpark();
          return Ok(item);
      }
      guard.waiting_sync_senders.push_front(waiter);
  }

  // Priority 2: Check buffer (rendezvous can have items in queue from handoff)
  if let Some(item) = guard.queue.pop_front() {
    return Ok(item);
  }

  if guard.sender_count == 0 {
    return Err(TryRecvError::Disconnected);
  }
  Err(TryRecvError::Empty)
}

fn try_recv_unbounded<T: Send>(shared: &MpmcShared<T>) -> Result<T, TryRecvError> {
  let mut guard = shared.internal.lock();
  // Priority 1: Check buffer
  if let Some(item) = guard.queue.pop_front() {
    // Unbounded channels never have parked senders waiting for space.
    return Ok(item);
  }

  if guard.sender_count == 0 {
    return Err(TryRecvError::Disconnected);
  }
  Err(TryRecvError::Empty)
}

fn try_recv_buffered<T: Send>(shared: &MpmcShared<T>) -> Result<T, TryRecvError> {
  let mut guard = shared.internal.lock();

  // Priority 1: Check buffer
  if let Some(item) = guard.queue.pop_front() {
    // If there are senders waiting for space, wake one up.
    // We prioritize async senders over sync senders.
    if let Some(waiter) = guard.waiting_async_senders.pop_front() {
      waiter.waker.wake();
    } else if let Some(waiter) = guard.waiting_sync_senders.pop_front() {
      unsafe { (*waiter.done).store(true, Ordering::Release) };
      waiter.thread.unpark();
    }
    return Ok(item);

  } else if guard.sender_count == 0 {
    return Err(TryRecvError::Disconnected);
  }
  Err(TryRecvError::Empty)
}
