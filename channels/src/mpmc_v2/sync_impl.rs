//! Implementation of the synchronous, blocking send and receive logic for the MPMC channel.

use super::core::{SyncWaiter, WaiterData, STATE_CANCELLED, STATE_SUCCESS, STATE_WAITING};
use super::{Receiver, Sender};
use crate::error::{
  BatchSendErrorReason, RecvError, SendBatchError, SendError, TryRecvError, TrySendError,
};
use crate::mpmc_v2::backoff;
use crate::RecvErrorTimeout;

use std::sync::atomic::{AtomicU8, Ordering};
use std::thread;
use std::time::{Duration, Instant};

/// The synchronous, blocking send operation.
pub(crate) fn send_sync<T: Send>(sender: &Sender<T>, item: T) -> Result<(), SendError> {
  let mut current_item_opt = Some(item);

  loop {
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
        current_item_opt = Some(returned_item);
      }
      Err(TrySendError::Sent(_)) => unreachable!(),
    }

    // --- Phase 2: Prepare to park (state on the stack, no heap allocation) ---
    let done_flag = AtomicU8::new(STATE_WAITING);
    let done_ptr = &done_flag as *const AtomicU8;
    let is_rendezvous = sender.shared.capacity == 0;

    let waiter = SyncWaiter {
      thread: thread::current(),
      data: if is_rendezvous {
        Some(WaiterData::SenderItem(current_item_opt.take()))
      } else {
        None
      },
      state: done_ptr,
    };

    // --- Phase 3: Lock, re-check, and commit to parking ---
    {
      let mut guard = sender.shared.internal.lock();

      if !guard.waiting_async_receivers.is_empty()
        || !guard.waiting_sync_receivers.is_empty()
        || (sender.shared.capacity > 0 && guard.queue.len() < sender.shared.capacity)
      {
        if is_rendezvous {
          let mut temp_waiter = waiter;
          current_item_opt = temp_waiter.take_item_from_sender_slot();
        }
        continue;
      }

      if guard.receiver_count == 0 {
        return Err(SendError::Closed);
      }

      guard.waiting_sync_senders.push_back(waiter);
    }

    // --- Phase 4: Wait ---
    // done_flag lives on the stack above; adaptive_wait borrows it by ref.
    // The stack frame stays alive until adaptive_wait returns (STATE_SUCCESS).
    backoff::adaptive_wait(|| done_flag.load(Ordering::Acquire) == STATE_SUCCESS);

    // --- Phase 5: Handle wake-up ---
    if is_rendezvous {
      if sender.is_closed() && done_flag.load(Ordering::Acquire) != STATE_SUCCESS {
        return Err(SendError::Closed);
      }
      return Ok(());
    }

    // For buffered channels, being woken means space may be available — loop to retry.
  }
}

/// The synchronous, blocking batch send: sends every item, parking whenever
/// the channel is full, until done or all receivers drop.
pub(crate) fn send_batch_sync<T: Send>(
  sender: &Sender<T>,
  items: Vec<T>,
) -> Result<usize, SendBatchError<T>> {
  let total = items.len();
  if total == 0 {
    return Ok(0);
  }
  let mut iter = items.into_iter();
  let mut sent = 0;
  let is_rendezvous = sender.shared.capacity == 0;
  // Holds an item reclaimed from an un-queued rendezvous waiter so it is
  // retried before the iterator is consumed further.
  let mut pending: Option<T> = None;

  loop {
    // --- Phase 1: non-blocking batch send (pending item first) ---
    if let Some(item) = pending.take() {
      let mut once = std::iter::once(item);
      let (k, reason) = sender.shared.try_send_batch_core(&mut once, 1);
      sent += k;
      if k == 0 {
        pending = once.next();
        if matches!(reason, Some(BatchSendErrorReason::Closed)) {
          let mut unsent: Vec<T> = pending.take().into_iter().collect();
          unsent.extend(iter.by_ref());
          return Err(SendBatchError { sent, unsent });
        }
      }
    }
    if pending.is_none() {
      let (k, reason) = sender.shared.try_send_batch_core(&mut iter, total - sent);
      sent += k;
      if sent == total {
        return Ok(total);
      }
      if matches!(reason, Some(BatchSendErrorReason::Closed)) {
        return Err(SendBatchError {
          sent,
          unsent: iter.collect(),
        });
      }
    }

    // --- Phase 2: prepare to park (rendezvous waiters carry the next item) ---
    let done_flag = AtomicU8::new(STATE_WAITING);
    let done_ptr = &done_flag as *const AtomicU8;

    let waiter = SyncWaiter {
      thread: thread::current(),
      data: if is_rendezvous {
        Some(WaiterData::SenderItem(
          pending.take().or_else(|| iter.next()),
        ))
      } else {
        None
      },
      state: done_ptr,
    };

    // --- Phase 3: Lock, re-check, and commit to parking ---
    {
      let mut guard = sender.shared.internal.lock();

      if !guard.waiting_async_receivers.is_empty()
        || !guard.waiting_sync_receivers.is_empty()
        || (sender.shared.capacity > 0 && guard.queue.len() < sender.shared.capacity)
      {
        if is_rendezvous {
          let mut temp_waiter = waiter;
          pending = temp_waiter.take_item_from_sender_slot();
        }
        continue;
      }

      if guard.receiver_count == 0 {
        let mut temp_waiter = waiter;
        let mut unsent: Vec<T> = temp_waiter
          .take_item_from_sender_slot()
          .into_iter()
          .collect();
        unsent.extend(iter.by_ref());
        return Err(SendBatchError { sent, unsent });
      }

      guard.waiting_sync_senders.push_back(waiter);
    }

    // --- Phase 4: Wait ---
    backoff::adaptive_wait(|| done_flag.load(Ordering::Acquire) == STATE_SUCCESS);

    // --- Phase 5: Handle wake-up ---
    if is_rendezvous {
      // The waiter (and its payload) was taken by a receiver, or by a close
      // path; either way it left our hands. Mirror the single-item `send_sync`
      // optimism: count it as sent. If the channel closed, the next loop
      // iteration reports Closed for the remainder.
      sent += 1;
      if sent == total {
        return Ok(total);
      }
    }
    // For buffered channels, being woken means space may be available — loop to retry.
  }
}

/// The synchronous, blocking in-place batch send. The vector is exclusively
/// borrowed for the whole call, so this delegates to the by-value path and
/// restores the unsent remainder into `items` on closure — observationally
/// identical to draining sent items from the front as they go.
pub(crate) fn send_batch_mut_sync<T: Send>(
  sender: &Sender<T>,
  items: &mut Vec<T>,
) -> Result<usize, SendError> {
  if items.is_empty() {
    return Ok(0);
  }
  let batch = std::mem::take(items);
  match send_batch_sync(sender, batch) {
    Ok(n) => Ok(n),
    Err(e) => {
      *items = e.unsent;
      Err(SendError::Closed)
    }
  }
}

/// The synchronous, blocking batch receive: blocks until at least one item is
/// available, then drains up to `max` without further waiting.
pub(crate) fn recv_batch_sync<T: Send>(
  receiver: &Receiver<T>,
  out: &mut Vec<T>,
  max: usize,
) -> Result<usize, RecvError> {
  if max == 0 {
    return Ok(0);
  }
  loop {
    // --- Phase 1: Attempt a non-blocking batch receive ---
    match receiver.shared.try_recv_batch_core(out, max) {
      Ok(k) => return Ok(k),
      Err(TryRecvError::Disconnected) => return Err(RecvError::Disconnected),
      Err(TryRecvError::Empty) => {}
    }

    // --- Phase 2: Prepare to park ---
    let done_flag = AtomicU8::new(STATE_WAITING);
    let done_ptr = &done_flag as *const AtomicU8;
    let waiter = SyncWaiter {
      thread: thread::current(),
      data: None,
      state: done_ptr,
    };

    // --- Phase 3: Lock, re-check, and commit to parking ---
    {
      let mut guard = receiver.shared.internal.lock();

      if !guard.queue.is_empty()
        || (receiver.shared.capacity == 0
          && (!guard.waiting_sync_senders.is_empty() || !guard.waiting_async_senders.is_empty()))
      {
        continue; // Loop to retry receive.
      }

      if guard.sender_count == 0 {
        return Err(RecvError::Disconnected);
      }

      guard.waiting_sync_receivers.push_back(waiter);
    }

    // --- Phase 4: Wait, then loop to drain ---
    backoff::adaptive_wait(|| done_flag.load(Ordering::Acquire) == STATE_SUCCESS);
  }
}

/// The synchronous, blocking receive operation.
pub(crate) fn recv_sync<T: Send>(receiver: &Receiver<T>) -> Result<T, RecvError> {
  loop {
    // --- Phase 1: Attempt a non-blocking receive ---
    match receiver.shared.try_recv_core() {
      Ok(item) => return Ok(item), // Success!
      Err(TryRecvError::Disconnected) => return Err(RecvError::Disconnected),
      Err(TryRecvError::Empty) => {}
    }

    // --- Phase 2: Prepare to park ---
    let done_flag = AtomicU8::new(STATE_WAITING);
    let done_ptr = &done_flag as *const AtomicU8;
    let waiter = SyncWaiter {
      thread: thread::current(),
      data: None, // Receivers never hold data in their waiter struct.
      state: done_ptr,
    };

    // --- Phase 3: Lock, re-check, and commit to parking ---
    {
      let mut guard = receiver.shared.internal.lock();

      if !guard.queue.is_empty()
        || (receiver.shared.capacity == 0
          && (!guard.waiting_sync_senders.is_empty() || !guard.waiting_async_senders.is_empty()))
      {
        continue; // Loop to retry receive.
      }

      if guard.sender_count == 0 {
        return Err(RecvError::Disconnected);
      }

      guard.waiting_sync_receivers.push_back(waiter);
    }

    // --- Phase 4: Wait ---
    backoff::adaptive_wait(|| done_flag.load(Ordering::Acquire) == STATE_SUCCESS);

    // --- Phase 5: Handle wake-up ---
    // Being woken means an item is likely available. Loop to the top to try_recv_core.
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

  // Declare state and waiter exactly once on the stack — no per-iteration allocation.
  let done_flag = AtomicU8::new(STATE_WAITING);
  let done_ptr = &done_flag as *const AtomicU8;
  let waiter = SyncWaiter {
    thread: thread::current(),
    data: None,
    state: done_ptr,
  };

  // Lock once to enqueue the waiter, with a final pre-park re-check.
  {
    let mut guard = receiver.shared.internal.lock();

    if !guard.queue.is_empty()
      || (receiver.shared.capacity == 0
        && (!guard.waiting_sync_senders.is_empty() || !guard.waiting_async_senders.is_empty()))
    {
      drop(guard);
      match receiver.shared.try_recv_core() {
        Ok(item) => return Ok(item),
        Err(TryRecvError::Disconnected) => return Err(RecvErrorTimeout::Disconnected),
        Err(TryRecvError::Empty) => {}
      }
    } else {
      if guard.sender_count == 0 {
        return Err(RecvErrorTimeout::Disconnected);
      }

      guard.waiting_sync_receivers.push_back(waiter);
    }
  }

  loop {
    let elapsed = start_time.elapsed();
    if elapsed >= timeout {
      // Attempt atomic cancellation.
      match done_flag.compare_exchange(
        STATE_WAITING,
        STATE_CANCELLED,
        Ordering::SeqCst,
        Ordering::SeqCst,
      ) {
        Ok(_) => {
          // Eagerly unlink so the stack frame can safely return.
          let mut guard = receiver.shared.internal.lock();
          guard.waiting_sync_receivers.retain(|w| w.state != done_ptr);
          return Err(RecvErrorTimeout::Timeout);
        }
        Err(_) => {
          // STATE_SUCCESS: a sender committed the handoff concurrently. Must complete.
          match receiver.shared.try_recv_core() {
            Ok(item) => return Ok(item),
            Err(TryRecvError::Disconnected) => return Err(RecvErrorTimeout::Disconnected),
            Err(TryRecvError::Empty) => unreachable!("state was SUCCESS but channel empty"),
          }
        }
      }
    }

    let remaining_timeout = timeout - elapsed;
    thread::park_timeout(remaining_timeout);

    // Check if a sender committed the handoff.
    if done_flag.load(Ordering::Acquire) == STATE_SUCCESS {
      match receiver.shared.try_recv_core() {
        Ok(item) => return Ok(item),
        Err(TryRecvError::Disconnected) => return Err(RecvErrorTimeout::Disconnected),
        Err(TryRecvError::Empty) => {} // Spurious wakeup — loop to re-check timeout
      }
    }
    // Spurious wakeup with no handoff committed — loop to re-check timeout without
    // re-acquiring the lock or re-enqueuing.
  }
}
