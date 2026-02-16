//! Implementation of the synchronous, blocking send and receive logic for the MPMC channel.

use super::core::{SyncReceiverWaiter, SyncSenderWaiter};
use super::{Receiver, Sender};
use crate::error::{RecvError, SendError, TryRecvError, TrySendError};
use crate::internal::cache_padded::CachePadded;
use crate::mpmc_exp::backoff;
use crate::RecvErrorTimeout;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

/// The synchronous, blocking send operation.
///
/// This function will attempt to send an item. If the channel is full, it will
/// park the current thread using an adaptive backoff strategy until space becomes
/// available or the channel is closed.
pub(crate) fn send_sync<T: Send>(sender: &Sender<T>, item: T) -> Result<(), SendError> {
  let mut item = Some(item);
  let is_rendezvous = sender.shared.capacity == 0;

  loop {
    // --- Phase 1: Fast path: try to send without blocking ---
    let current_item = item.take().unwrap();
    match sender.shared.try_send_core(current_item) {
      Ok(()) => return Ok(()), // Success!
      Err(TrySendError::Closed(_)) => return Err(SendError::Closed),
      Err(TrySendError::Sent(_)) => unreachable!(),
      Err(TrySendError::Full(returned)) => {
        item = Some(returned); // Keep ownership of the item
      }
    }

    // --- Phase 2: Slow path: lock, re-check, and potentially park ---
    let done_flag = CachePadded::new(AtomicBool::new(false));
    {
      let mut guard = sender.shared.internal.lock();

      // Re-check state under lock to prevent lost wakeups
      if !guard.waiting_async_receivers.is_empty() // A receiver is waiting for our item.
        || !guard.waiting_sync_receivers.is_empty() // A sync receiver is waiting.
        || (sender.shared.capacity > 0 // It's a buffered channel...
          && guard.queue.len() < sender.shared.capacity)
      {
        // State changed, retry without parking
        drop(guard);
        continue;
      }

      if guard.receiver_count == 0 {
        return Err(SendError::Closed);
      }

      // Commit to parking: create waiter and flag now.
      let waiter = SyncSenderWaiter {
        thread: thread::current(),
        data: if is_rendezvous { item.take() } else { None },
        done: &done_flag,
      };
      guard.waiting_sync_senders.push_back(waiter);
    }

    // Wait outside the lock.
    backoff::adaptive_wait(&done_flag);

    if is_rendezvous {
      // For rendezvous, the item was moved into the waiter. The send is complete.
      return if sender.is_closed() && !done_flag.load(Ordering::Acquire) {
        Err(SendError::Closed)
      } else {
        Ok(())
      };
    }
    // For buffered, `item` is still `Some`, so we loop to retry.
  }
}

/// The synchronous, blocking receive operation.
///
pub(crate) fn recv_sync<T: Send>(receiver: &Receiver<T>) -> Result<T, RecvError> {
  loop {
    // --- Phase 1: Attempt a non-blocking receive ---
    match receiver.shared.try_recv_core() {
      Ok(item) => return Ok(item), // Success!
      Err(TryRecvError::Disconnected) => return Err(RecvError::Disconnected),
      Err(TryRecvError::Empty) => {
        // Channel is empty, prepare to park.
      }
    }

    // --- Phase 2: Slow path: lock and decide whether to park ---
    let done_flag = CachePadded::new(AtomicBool::new(false));
    {
      let mut guard = receiver.shared.internal.lock();

      // Re-check state under lock. An item may have arrived.
      if !guard.queue.is_empty()
        || (receiver.shared.capacity == 0
          && (!guard.waiting_sync_senders.is_empty() || !guard.waiting_async_senders.is_empty()))
      {
        continue; // Loop to retry receive.
      }

      if guard.sender_count == 0 {
        return Err(RecvError::Disconnected);
      }

      // Commit to parking.
      let waiter = SyncReceiverWaiter {
        thread: thread::current(),
        done: &done_flag,
      };
      guard.waiting_sync_receivers.push_back(waiter);
    }

    // --- Phase 3: Wait outside the lock ---
    backoff::adaptive_wait(&done_flag);

    // --- Phase 4: Handle wake-up ---
    // Being woken means an item is likely available. Loop to the top to `try_recv_core` again.
  }
}

/// The synchronous, blocking receive operation with a timeout.
pub(crate) fn recv_timeout_sync<T: Send>(
  receiver: &Receiver<T>,
  timeout: Duration,
) -> Result<T, RecvErrorTimeout> {
  let start_time = Instant::now();

  // First, try a non-blocking receive.
  match receiver.shared.try_recv_core() {
    Ok(item) => return Ok(item),
    Err(TryRecvError::Disconnected) => return Err(RecvErrorTimeout::Disconnected),
    Err(TryRecvError::Empty) => { /* Continue to blocking path */ }
  }

  loop {
    let elapsed = start_time.elapsed();
    if elapsed >= timeout {
      return Err(RecvErrorTimeout::Timeout);
    }
    let remaining_timeout = timeout - elapsed;

    // --- Phase 2: Prepare to park ---
    let done_flag = CachePadded::new(AtomicBool::new(false));

    // --- Phase 3: Lock, re-check, and commit to parking ---
    {
      let mut guard = receiver.shared.internal.lock();

      // Re-check state under lock.
      if !guard.queue.is_empty()
        || (receiver.shared.capacity == 0
          && (!guard.waiting_sync_senders.is_empty() || !guard.waiting_async_senders.is_empty()))
      {
        drop(guard);
        // An item might be available, loop to try_recv_core again without parking
        match receiver.shared.try_recv_core() {
          Ok(item) => return Ok(item),
          Err(TryRecvError::Disconnected) => return Err(RecvErrorTimeout::Disconnected),
          Err(TryRecvError::Empty) => continue,
        }
      }

      if guard.sender_count == 0 {
        return Err(RecvErrorTimeout::Disconnected);
      }

      let waiter = SyncReceiverWaiter {
        thread: thread::current(),
        done: &done_flag,
      };
      guard.waiting_sync_receivers.push_back(waiter);
    }

    // --- Phase 4: Wait with timeout ---
    thread::park_timeout(remaining_timeout);

    // --- Phase 5: Handle wake-up ---
    // If we were woken, the flag will be set.
    if done_flag.load(Ordering::Acquire) {
      match receiver.shared.try_recv_core() {
        Ok(item) => return Ok(item),
        Err(TryRecvError::Disconnected) => return Err(RecvErrorTimeout::Disconnected),
        Err(TryRecvError::Empty) => continue, // Spurious wakeup, loop to check timeout and retry
      }
    }

    // If the flag is not set, it means we timed out. The loop will check the deadline
    // and exit or park again.
  }
}
