//! Implementation of the synchronous, blocking send and receive logic for the MPMC channel.

use super::core::{SyncWaiter, STATE_CANCELLED, STATE_WAITING};
use super::{Receiver, Sender};
use crate::error::{
  BatchSendErrorReason, RecvError, SendBatchError, SendError, TryRecvError, TrySendError,
};
use crate::mpmc_v2::backoff;
use crate::RecvErrorTimeout;

use std::time::{Duration, Instant};

use crate::internal::sync::{thread, AtomicU8, Ordering};

/// The synchronous, blocking send operation.
pub(crate) fn send_sync<T: Send>(sender: &Sender<T>, item: T) -> Result<(), SendError> {
  let mut current_item_opt = Some(item);

  loop {
    let item_to_send = current_item_opt.take().unwrap();

    match sender.shared.try_send_core(item_to_send) {
      Ok(()) => return Ok(()),
      Err(TrySendError::Closed(_)) => return Err(SendError::Closed),
      Err(TrySendError::Full(returned_item)) => {
        current_item_opt = Some(returned_item);
      }
      Err(TrySendError::Sent(_)) => unreachable!(),
    }

    let done_flag = AtomicU8::new(STATE_WAITING);
    let done_ptr = &done_flag as *const AtomicU8;

    let waiter = SyncWaiter {
      thread: thread::current(),
      state: done_ptr,
    };

    {
      let mut guard = sender.shared.internal.lock();

      if !guard.is_full(sender.shared.capacity)
        && (!guard.waiting_async_receivers.is_empty()
          || !guard.waiting_sync_receivers.is_empty()
          || (sender.shared.capacity > 0 && guard.len() < sender.shared.capacity))
      {
        continue;
      }

      if guard.receiver_count == 0 {
        return Err(SendError::Closed);
      }

      guard.waiting_sync_senders.push_back(waiter);
    }

    // Bitwise wait loop: exits when FINISHED (Bit 0 / 0x01) is set
    backoff::adaptive_wait(|| {
      let st = done_flag.load(Ordering::Acquire);
      (st & 0x01) != 0
    });

    let final_state = done_flag.load(Ordering::Acquire);

    // Check if we closed (Bit 1 / 0x02 is 0)
    if (final_state & 0x02) == 0 {
      return Err(SendError::Closed);
    }

    // It was a success!
    // If it was rendezvous (Bit 2 / 0x04 is set), the receiver popped the waiter and stole the item.
    if (final_state & 0x04) != 0 {
      return Ok(());
    }

    // current_item_opt is still Some(item) locally. Loop back to retry.
  }
}

/// Iterator-driven core for the blocking batch send, shared by the by-value and
/// in-place (`_mut`) entry points. Sends every item, parking whenever the channel
/// is full, until done or all receivers drop. On close returns `(sent, pending)`,
/// where `pending` is the one item pulled from `iter` but not yet placed — callers
/// restore it ahead of whatever remains in `iter`.
fn send_batch_iter_sync<T: Send, I: Iterator<Item = T>>(
  sender: &Sender<T>,
  iter: &mut I,
  total: usize,
) -> Result<usize, (usize, Option<T>)> {
  let mut sent = 0;
  let mut pending: Option<T> = None;

  loop {
    if let Some(item) = pending.take() {
      let mut once = std::iter::once(item);
      let (k, reason) = sender.shared.try_send_batch_core(&mut once, 1);
      sent += k;
      if k == 0 {
        pending = once.next();
        if matches!(reason, Some(BatchSendErrorReason::Closed)) {
          return Err((sent, pending.take()));
        }
      }
    }
    if pending.is_none() {
      let (k, reason) = sender.shared.try_send_batch_core(iter, total - sent);
      sent += k;
      if sent == total {
        return Ok(total);
      }
      if matches!(reason, Some(BatchSendErrorReason::Closed)) {
        return Err((sent, None));
      }
    }

    let done_flag = AtomicU8::new(STATE_WAITING);
    let done_ptr = &done_flag as *const AtomicU8;

    let waiter = SyncWaiter {
      thread: thread::current(),
      state: done_ptr,
    };

    {
      let mut guard = sender.shared.internal.lock();

      if !guard.is_full(sender.shared.capacity)
        && (!guard.waiting_async_receivers.is_empty()
          || !guard.waiting_sync_receivers.is_empty()
          || (sender.shared.capacity > 0 && guard.len() < sender.shared.capacity))
      {
        continue;
      }

      if guard.receiver_count == 0 {
        return Err((sent, pending.take()));
      }

      guard.waiting_sync_senders.push_back(waiter);
    }

    backoff::adaptive_wait(|| {
      let st = done_flag.load(Ordering::Acquire);
      (st & 0x01) != 0
    });

    let final_state = done_flag.load(Ordering::Acquire);

    // Check if we closed (Bit 1 / 0x02 is 0)
    if (final_state & 0x02) == 0 {

      return Err((sent, pending.take()));
    }

    // It was a success!
    if (final_state & 0x04) != 0 {
      // Handoff complete
      sent += 1;
      if sent == total {
        return Ok(total);
      }
    }
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
  match send_batch_iter_sync(sender, &mut iter, total) {
    Ok(n) => Ok(n),
    Err((sent, pending)) => {
      let mut unsent: Vec<T> = pending.into_iter().collect();
      unsent.extend(iter);
      Err(SendBatchError { sent, unsent })
    }
  }
}

/// The synchronous, blocking in-place batch send. Drains the caller's buffer in
/// place so on success its allocation is retained (a `mem::take` hands it off
/// and leaves an empty zero-capacity Vec). On close the unconsumed tail is
/// collected back into `items` (Drain's Drop would destroy it), with the one
/// popped-but-unsent item (if any) restored ahead of it.
pub(crate) fn send_batch_mut_sync<T: Send>(
  sender: &Sender<T>,
  items: &mut Vec<T>,
) -> Result<usize, SendError> {
  if items.is_empty() {
    return Ok(0);
  }
  let total = items.len();
  let mut drain = items.drain(..);
  match send_batch_iter_sync(sender, &mut drain, total) {
    Ok(n) => Ok(n),
    Err((_sent, pending)) => {
      let rest: Vec<T> = drain.collect();
      *items = rest;
      if let Some(item) = pending {
        items.insert(0, item);
      }
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
    backoff::adaptive_wait(|| {
      let st = done_flag.load(Ordering::Acquire);
      (st & 0x01) != 0
    });
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
    backoff::adaptive_wait(|| {
      let st = done_flag.load(Ordering::Acquire);
      (st & 0x01) != 0
    });
  }
}

/// The synchronous, blocking receive operation with a timeout.
pub(crate) fn recv_timeout_sync<T: Send>(
  receiver: &Receiver<T>,
  timeout: Duration,
) -> Result<T, RecvErrorTimeout> {
  let start_time = Instant::now();

  loop {
    match receiver.shared.try_recv_core() {
      Ok(item) => return Ok(item),
      Err(TryRecvError::Disconnected) => return Err(RecvErrorTimeout::Disconnected),
      Err(TryRecvError::Empty) => {}
    }

    let done_flag = AtomicU8::new(STATE_WAITING);
    let done_ptr = &done_flag as *const AtomicU8;
    let waiter = SyncWaiter {
      thread: thread::current(),
      state: done_ptr,
    };

    {
      let mut guard = receiver.shared.internal.lock();

      if !guard.queue.is_empty()
        || (receiver.shared.capacity == 0
          && (!guard.waiting_sync_senders.is_empty() || !guard.waiting_async_senders.is_empty()))
      {
        continue;
      }

      if guard.sender_count == 0 {
        return Err(RecvErrorTimeout::Disconnected);
      }

      guard.waiting_sync_receivers.push_back(waiter);
    }

    while done_flag.load(Ordering::Acquire) == STATE_WAITING {
      let elapsed = start_time.elapsed();
      if elapsed < timeout {
        thread::park_timeout(timeout - elapsed);
        continue;
      }

      if done_flag
        .compare_exchange(
          STATE_WAITING,
          STATE_CANCELLED,
          Ordering::SeqCst,
          Ordering::SeqCst,
        )
        .is_ok()
      {
        let mut guard = receiver.shared.internal.lock();
        guard.waiting_sync_receivers.retain(|w| w.state != done_ptr);
        return Err(RecvErrorTimeout::Timeout);
      }
    }

    // Marked by a sender or a close, which also removed the entry. Past the
    // deadline one more attempt decides between the item and the timeout.
    if start_time.elapsed() >= timeout {
      return match receiver.shared.try_recv_core() {
        Ok(item) => Ok(item),
        Err(TryRecvError::Disconnected) => Err(RecvErrorTimeout::Disconnected),
        Err(TryRecvError::Empty) => Err(RecvErrorTimeout::Timeout),
      };
    }
  }
}
