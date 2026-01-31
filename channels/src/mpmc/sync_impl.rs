//! Implementation of the synchronous, blocking send and receive logic for the MPMC channel.

use super::core::{SyncWaiter, WaiterData};
use super::{Receiver, Sender};
use crate::error::{RecvError, SendError, TryRecvError, TrySendError};
use crate::mpmc::backoff;
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
  // Use an Option to manage ownership of the item across loop iterations.
  let mut current_item_opt = Some(item);

  loop {
    // We must have an item to send at the start of the loop.
    let item_to_send = current_item_opt
      .take()
      .expect("Item should always exist at the start of the loop");

    // --- Phase 1: Attempt a non-blocking send ---
    match sender.shared.try_send_core(item_to_send) {
      Ok(()) => return Ok(()), // Success!
      Err(TrySendError::Closed(_)) => {
        return Err(SendError::Closed);
      }
      Err(TrySendError::Full(returned_item)) => {
        // Channel is full, must park. Put the item back into our Option.
        current_item_opt = Some(returned_item);
      }
      Err(TrySendError::Sent(_)) => unreachable!(),
    }

    // --- Phase 2: Prepare to park ---
    let done_flag = Arc::new(AtomicBool::new(false));
    let is_rendezvous = sender.shared.capacity == 0;

    // Create the waiter struct. For rendezvous channels, we move the item into it.
    let waiter = SyncWaiter {
      thread: thread::current(),
      data: if is_rendezvous {
        Some(WaiterData::SenderItem(current_item_opt.take()))
      } else {
        None
      },
      done: done_flag.clone(),
    };

    // --- Phase 3: Lock, re-check, and commit to parking ---
    {
      let mut guard = sender.shared.internal.lock();

      // Re-check state under lock to prevent lost wakeups.
      // An item can be sent if a receiver is waiting, or if there's buffer space.
      if !guard.waiting_async_receivers.is_empty() // A receiver is waiting for our item.
        || !guard.waiting_sync_receivers.is_empty() // A sync receiver is waiting.
        || (sender.shared.capacity > 0 // It's a buffered channel...
          && guard.queue.len() < sender.shared.capacity)
      {
        // State changed. Don't park. Retrieve our item if it was for rendezvous.
        if is_rendezvous {
          let mut temp_waiter = waiter;
          current_item_opt = temp_waiter.take_item_from_sender_slot();
        }
        continue; // Loop again to retry the send.
      }

      // Check for closure one last time under the lock.
      if guard.receiver_count == 0 {
        return Err(SendError::Closed);
      }

      // All checks passed. It's safe to park. Add ourselves to the wait queue.
      guard.waiting_sync_senders.push_back(waiter);
    }

    // --- Phase 4: Wait ---
    // The adaptive wait will spin, then yield, then park until `done_flag` is true.
    backoff::adaptive_wait(|| done_flag.load(Ordering::Acquire));

    // --- Phase 5: Handle wake-up ---
    if is_rendezvous {
      // For rendezvous, if we are woken, the send is considered complete.
      // The receiver took the item directly from our `WaiterData`.
      // We double-check for closure in case we were woken by a dropping receiver.
      if sender.is_closed() && !done_flag.load(Ordering::Acquire) {
        return Err(SendError::Closed);
      }
      return Ok(());
    }

    // For buffered channels, being woken just means there might be space.
    // The item is still in `current_item_opt`, so we loop to try sending again.
  }
}

/// The synchronous, blocking receive operation.
///
/// This function will attempt to receive an item. If the channel is empty, it will
/// park the current thread using an adaptive backoff strategy until an item
/// is available or the channel is disconnected.
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

    // --- Phase 2: Prepare to park ---
    let done_flag = Arc::new(AtomicBool::new(false));
    let waiter = SyncWaiter {
      thread: thread::current(),
      data: None, // Receivers never hold data in their waiter struct.
      done: done_flag.clone(),
    };

    // --- Phase 3: Lock, re-check, and commit to parking ---
    {
      let mut guard = receiver.shared.internal.lock();

      // Re-check state under lock. An item may have arrived.
      // An item is available if it's in the queue, or for rendezvous channels, if a sender is waiting.
      if !guard.queue.is_empty()
        || (receiver.shared.capacity == 0
          && (!guard.waiting_sync_senders.is_empty() || !guard.waiting_async_senders.is_empty()))
      {
        continue; // Loop to retry receive.
      }

      // Check for disconnection under the lock.
      if guard.sender_count == 0 {
        return Err(RecvError::Disconnected);
      }

      // Safe to park.
      guard.waiting_sync_receivers.push_back(waiter);
    }

    // --- Phase 4: Wait ---
    backoff::adaptive_wait(|| done_flag.load(Ordering::Acquire));

    // --- Phase 5: Handle wake-up ---
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
    let done_flag = Arc::new(AtomicBool::new(false));
    let waiter = SyncWaiter {
      thread: thread::current(),
      data: None, // Receivers never hold data.
      done: done_flag.clone(),
    };

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

      guard.waiting_sync_receivers.push_back(waiter);
    }

    // --- Phase 4: Wait with timeout ---
    thread::park_timeout(remaining_timeout);

    // --- Phase 5: Handle wake-up ---
    // After waking (or timeout), try again. The loop will check the timeout.
    match receiver.shared.try_recv_core() {
      Ok(item) => return Ok(item),
      Err(TryRecvError::Disconnected) => return Err(RecvErrorTimeout::Disconnected),
      Err(TryRecvError::Empty) => { /* Loop to check timeout and maybe park again */ }
    }
  }
}
