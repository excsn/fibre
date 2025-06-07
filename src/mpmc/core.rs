// src/mpmc/core.rs

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
use crate::RecvError;
use parking_lot::Mutex;
use core::future::PollFn;
use core::task::{Context, Poll};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::task::Waker;
use std::thread::Thread;

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
  /// An atomic flag used by the adaptive backoff strategy to detect when the
  /// wait is over, preventing the thread from re-parking unnecessarily.
  pub(crate) done: Arc<AtomicBool>,
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

/// Represents a parked asynchronous task waiting for an operation to complete.
#[derive(Debug)]
pub(crate) struct AsyncWaiter<T> {
  /// The waker for the parked future, used for `wake()`.
  pub(crate) waker: Waker,
  /// A slot for a rendezvous item. `None` for buffered waiters.
  pub(crate) data: Option<WaiterData<T>>,
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
        waiting_sync_senders: VecDeque::new(),
        waiting_async_senders: VecDeque::new(),
        waiting_sync_receivers: VecDeque::new(),
        waiting_async_receivers: VecDeque::new(),
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
    // Prioritize async waiters as they are generally lower overhead to wake.
    if let Some(waiter) = guard.waiting_async_receivers.pop_front() {
      guard.queue.push_back(item); // Item is buffered for the receiver to pick up.
      waiter.waker.wake();
      return Ok(());
    }
    if let Some(waiter) = guard.waiting_sync_receivers.pop_front() {
      guard.queue.push_back(item);
      waiter.done.store(true, Ordering::Release);
      waiter.thread.unpark();
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

    // --- Priority 1: Check for a waiting rendezvous sender ---
    if let Some(mut waiter) = guard.waiting_async_senders.pop_front() {
      if let Some(item) = waiter.take_item_from_sender_slot() {
        waiter.waker.wake();
        return Ok(item);
      }
      // Not a rendezvous sender, put it back.
      guard.waiting_async_senders.push_front(waiter);
    }
    if let Some(mut waiter) = guard.waiting_sync_senders.pop_front() {
      if let Some(item) = waiter.take_item_from_sender_slot() {
        waiter.done.store(true, Ordering::Release);
        waiter.thread.unpark();
        return Ok(item);
      }
      // Not a rendezvous sender, put it back.
      guard.waiting_sync_senders.push_front(waiter);
    }

    // --- Priority 2: Check the main buffer ---
    if let Some(item) = guard.queue.pop_front() {
      // An item was in the queue. This might have freed up space for a buffered sender.
      // Wake one up if any are waiting.
      if let Some(waiter) = guard.waiting_async_senders.pop_front() {
        waiter.waker.wake();
      } else if let Some(waiter) = guard.waiting_sync_senders.pop_front() {
        waiter.done.store(true, Ordering::Release);
        waiter.thread.unpark();
      }
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

        // Safe to park.
        let waiter = AsyncWaiter {
          waker: cx.waker().clone(),
          data: None, // Receivers never hold data.
        };
        guard.waiting_async_receivers.push_back(waiter);
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
      guard.queue.clear(); // VecDeque's clear/drop handles dropping items.

      // Drain any parked senders, which might contain items for rendezvous channels.
      for mut waiter in guard.waiting_sync_senders.drain(..) {
        if let Some(_item) = waiter.take_item_from_sender_slot() {
          // Item from rendezvous waiter is dropped here.
        }
      }
      for mut waiter in guard.waiting_async_senders.drain(..) {
        if let Some(_item) = waiter.take_item_from_sender_slot() {
          // Item from rendezvous waiter is dropped here.
        }
      }
    }
    // If the lock is poisoned, the data inside might not be properly dropped,
    // but this is an unrecoverable panic state anyway.
  }
}
