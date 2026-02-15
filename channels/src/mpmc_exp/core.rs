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
//!     two distinct queues: `waiting_senders` and `waiting_receivers`. These queues use
//!     an intrusive linked list (`WaiterQueue`) to hold both synchronous and asynchronous waiters.
//! 3.  **Wake-up Priority**: When an operation frees up a resource (e.g., a `send`
//!     provides an item, or a `recv` creates space), the logic prioritizes waking an
//!     existing parked task/thread over performing another action (like pushing to the
//!     buffer). Async waiters are generally prioritized as they are lower overhead to wake.

use crate::error::{TryRecvError, TrySendError};
use crate::internal::waiter::{AsyncWaiterNode, SyncWaiterNode, WaiterQueue};
use parking_lot::Mutex;
use std::collections::VecDeque;

/// The core state of the MPMC channel, protected by a single `Mutex`.
#[derive(Debug)]
pub(crate) struct MpmcChannelInternal<T> {
  /// The primary buffer for items when the channel has a capacity > 0.
  pub(crate) queue: VecDeque<T>,
  /// Queue of parked synchronous senders.
  pub(crate) waiting_sync_senders: WaiterQueue<SyncWaiterNode<T>>,
  /// Queue of parked asynchronous senders.
  pub(crate) waiting_async_senders: WaiterQueue<AsyncWaiterNode<T>>,
  /// Queue of parked synchronous receivers.
  pub(crate) waiting_sync_receivers: WaiterQueue<SyncWaiterNode<T>>,
  /// Queue of parked asynchronous receivers.
  pub(crate) waiting_async_receivers: WaiterQueue<AsyncWaiterNode<T>>,
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

// It is safe to send MpmcShared across threads if T is Send.
// The internal Mutex ensures that access to the shared state is properly synchronized.
unsafe impl<T: Send> Send for MpmcShared<T> {}
unsafe impl<T: Send> Sync for MpmcShared<T> {}

impl<T: Send> MpmcShared<T> {
  /// Creates a new shared core for the channel with a given capacity.
  /// `usize::MAX` is used to signify an "unbounded" channel.
  pub(crate) fn new(capacity: usize) -> Self {
    MpmcShared {
      internal: Mutex::new(MpmcChannelInternal {
        queue: VecDeque::with_capacity(if capacity == usize::MAX { 32 } else { capacity }),
        waiting_sync_senders: WaiterQueue::new(),
        waiting_async_senders: WaiterQueue::new(),
        waiting_sync_receivers: WaiterQueue::new(),
        waiting_async_receivers: WaiterQueue::new(),
        sender_count: 1, // Starts with one producer and one consumer
        receiver_count: 1,
      }),
      capacity,
    }
  }

  /// The core logic for attempting to send an item. This will try, in order:
  /// 1. Hand the item to a waiting receiver (async first, then sync).
  /// 2. Push the item into the buffer if there is space.
  /// If neither is possible, it returns a `TrySendError`.
  pub(crate) fn try_send_core(&self, item: T) -> Result<(), TrySendError<T>> {
    let mut guard = self.internal.lock();

    if guard.receiver_count == 0 {
      return Err(TrySendError::Closed(item));
    }

    // --- Priority 1: Wake a waiting receiver ---
    // If a receiver is waiting, we can hand off the item directly (via the queue).
    // This works for both buffered and rendezvous channels.
    if let Some(mut waiter_ptr) = unsafe { guard.waiting_async_receivers.pop_front() } {
      let waiter = unsafe { waiter_ptr.as_mut() };
      waiter.item = Some(item); // Direct handoff
      drop(guard); // Wake outside lock
      waiter.wake();
      return Ok(());
    }
    if let Some(mut waiter_ptr) = unsafe { guard.waiting_sync_receivers.pop_front() } {
      let waiter = unsafe { waiter_ptr.as_mut() };
      waiter.item = Some(item); // Direct handoff
      drop(guard); // Wake outside lock
      waiter.wake();
      return Ok(());
    }

    // --- Priority 2: Push to buffer if space is available ---
    if self.capacity == 0 {
      // A rendezvous channel is "full" if no receivers are waiting.
      return Err(TrySendError::Full(item));
    }
    if self.capacity == usize::MAX || guard.queue.len() < self.capacity {
      guard.queue.push_back(item);
      return Ok(());
    }

    // --- Fallback: Buffer is full and no one is waiting ---
    Err(TrySendError::Full(item))
  }

  /// The core logic for attempting to receive an item. This will try, in order:
  /// 1. Take an item from a waiting rendezvous sender (async first, then sync).
  /// 2. Take an item from the buffer. If successful, it may wake a waiting buffered sender.
  /// If neither is possible, it returns a `TryRecvError`.
  pub(crate) fn try_recv_core(&self) -> Result<T, TryRecvError> {
    let mut guard = self.internal.lock();

    // --- Priority 1: Check the main buffer ---
    if let Some(item) = guard.queue.pop_front() {
      // An item was in the queue. This might have freed up space for a buffered sender.
      // If this is a buffered channel, we should move a waiting sender's item into the queue.
      let mut async_sender_to_wake = None;
      let mut sync_sender_to_wake = None;

      if self.capacity > 0 {
        if let Some(mut sender_ptr) = unsafe { guard.waiting_async_senders.pop_front() } {
          let sender = unsafe { sender_ptr.as_mut() };
          let sender_item = sender.item.take().expect("Sender must have an item");
          guard.queue.push_back(sender_item);
          async_sender_to_wake = Some(sender_ptr);
        } else if let Some(mut sender_ptr) = unsafe { guard.waiting_sync_senders.pop_front() } {
          let sender = unsafe { sender_ptr.as_mut() };
          let sender_item = sender.item.take().expect("Sender must have an item");
          guard.queue.push_back(sender_item);
          sync_sender_to_wake = Some(sender_ptr);
        }
      }
      drop(guard); // Wake outside lock
      if let Some(sender_ptr) = async_sender_to_wake {
        unsafe { sender_ptr.as_ref().wake() };
      }
      if let Some(sender_ptr) = sync_sender_to_wake {
        unsafe { sender_ptr.as_ref().wake() };
      }
      return Ok(item);
    }

    // --- Priority 2: Check for a waiting rendezvous sender ---
    // If the queue is empty, we might still have a sender waiting if it's a rendezvous channel.
    // (For buffered channels, queue empty implies no senders are waiting).
    if let Some(mut sender_ptr) = unsafe { guard.waiting_async_senders.pop_front() } {
      // Direct hand-off from sender to receiver.
      let sender = unsafe { sender_ptr.as_mut() };
      let item = sender.item.take().expect("Sender must have an item");
      drop(guard); // Wake outside lock
      sender.wake();
      // We don't push to queue here; we take it directly.
      return Ok(item);
    }
    if let Some(mut sender_ptr) = unsafe { guard.waiting_sync_senders.pop_front() } {
      let sender = unsafe { sender_ptr.as_mut() };
      let item = sender.item.take().expect("Sender must have an item");
      drop(guard);
      sender.wake();
      return Ok(item);
    }

    // --- Priority 3: Check for disconnection ---
    // This check happens only after confirming the channel is truly empty
    // (no buffered items, no waiting rendezvous senders).
    if guard.sender_count == 0 {
      return Err(TryRecvError::Disconnected);
    }

    // --- Fallback: Channel is not disconnected but is temporarily empty ---
    Err(TryRecvError::Empty)
  }
}

impl<T> Drop for MpmcShared<T> {
  fn drop(&mut self) {
    // When the Arc<MpmcShared> is finally dropped (i.e., all Senders and
    // Receivers are gone), we must drop any items remaining in the channel
    // to prevent memory leaks.
    if let Some(mut guard) = self.internal.try_lock() {
      // Drain the main queue.
      guard.queue.clear(); // VecDeque's clear/drop handles dropping items.

      // Note: We do not need to drain `waiting_senders` or `waiting_receivers`.
      // If `MpmcShared` is dropping, it means all `Sender` and `Receiver` handles are gone.
      // Therefore, there can be no active waiters in the queues.
    }
    // If the lock is poisoned, the data inside might not be properly dropped,
    // but this is an unrecoverable panic state anyway.
  }
}
