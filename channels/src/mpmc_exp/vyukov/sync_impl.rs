//! Blocking sync send/recv for `mpmc_v3`.
//!
//! The fast path is a lock-free `ring.push` / `ring.pop`. On full/empty we use
//! the register → `SeqCst` fence → recheck → `thread::park` discipline from
//! [`super::core`] to park without losing wakeups. `thread::park`/`unpark` is
//! token-based, so an `unpark` that arrives before we `park` is not lost.

use std::cell::Cell;
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use crate::error::{
  BatchSendErrorReason, RecvError, RecvErrorTimeout, SendBatchError, SendError, TryRecvError,
};

use super::core::{Role, Shared, WakeRef};

// Adaptive spin backoff: spin up to THREAD_SPIN_LIMIT iterations, then
// register as a waiter. Success rewards (+1 up to SPIN_MAX), forced park
// penalises (-1 down to SPIN_MIN). No yield phase: yielding before
// registration causes scheduler starvation under thread oversubscription.
const SPIN_INITIAL: u32 = 16;
const SPIN_MIN: u32 = 2;
const SPIN_MAX: u32 = 64;

thread_local! {
  static THREAD_SPIN_LIMIT: Cell<u32> = Cell::new(SPIN_INITIAL);
}

pub(crate) fn send_sync<T: Send>(shared: &Arc<Shared<T>>, item: T) -> Result<(), SendError> {
  let mut item = item;
  // `my_id` is `Some` only while we are actually linked in the waiter slab. A
  // legitimate wake (`notified` set by `wake_one`) means we were already popped,
  // so we clear `my_id` and skip the redundant re-register + unregister - that
  // churn is what dominated the lock traffic under contention.
  let mut my_id: Option<u64> = None;
  let notified = AtomicBool::new(false);
  let notified_ptr = &notified as *const AtomicBool;
  let spin_limit = THREAD_SPIN_LIMIT.with(|c| c.get());

  // Closure check must precede the push: an empty bounded ring would otherwise
  // accept a value even though every receiver is gone.
  if !shared.receivers_alive() {
    return Err(SendError::Closed);
  }

  // Fast path.
  match shared.ring.push(item) {
    Ok(()) => {
      shared.notify_receivers();
      return Ok(());
    }
    Err(returned) => item = returned,
  }

  let mut backoff: u32 = 0;
  loop {
    if !shared.receivers_alive() {
      if let Some(id) = my_id {
        shared.unregister(Role::Send, id);
      }
      return Err(SendError::Closed);
    }

    // Lock-free retry first - covers the initial slow entry, every backoff step,
    // and every wake.
    match shared.ring.push(item) {
      Ok(()) => {
        if let Some(id) = my_id {
          shared.unregister(Role::Send, id);
        }
        shared.notify_receivers();
        THREAD_SPIN_LIMIT.with(|c| { let cur = c.get(); c.set((cur + 1).min(SPIN_MAX)); });
        return Ok(());
      }
      Err(returned) => item = returned,
    }

    if my_id.is_some() {
      // Already linked (the Dekker recheck above failed): park.
      THREAD_SPIN_LIMIT.with(|c| { let cur = c.get(); c.set((cur / 2).max(SPIN_MIN)); });
      thread::park();
      if notified.swap(false, Ordering::Acquire) {
        // We were popped by `wake_one`; no longer linked. Back off again.
        my_id = None;
        backoff = 0;
      }
      continue;
    }

    if backoff < spin_limit {
      std::hint::spin_loop();
      backoff += 1;
      continue;
    }

    // Register, fence, then loop for the Dekker recheck before parking.
    let id = shared.register(Role::Send, None, WakeRef::Thread(thread::current()), notified_ptr);
    my_id = Some(id);
    shared.pre_park_fence();
  }
}

pub(crate) fn recv_sync<T: Send>(shared: &Arc<Shared<T>>) -> Result<T, RecvError> {
  let mut my_id: Option<u64> = None;
  let notified = AtomicBool::new(false);
  let notified_ptr = &notified as *const AtomicBool;

  if let Some(item) = shared.ring.pop() {
    shared.notify_senders();
    return Ok(item);
  }

  let spin_limit = THREAD_SPIN_LIMIT.with(|c| c.get());
  let mut backoff: u32 = 0;
  loop {
    // Lock-free retry first - covers the initial slow entry, every backoff step,
    // and every wake.
    if let Some(item) = shared.ring.pop() {
      if let Some(id) = my_id {
        shared.unregister(Role::Recv, id);
      }
      shared.notify_senders();
      THREAD_SPIN_LIMIT.with(|c| { let cur = c.get(); c.set((cur + 1).min(SPIN_MAX)); });
      return Ok(item);
    }

    // Empty: disconnected only if no senders remain AND still nothing buffered.
    if !shared.senders_alive() {
      // One last drain in case a sender published then dropped.
      if let Some(item) = shared.ring.pop() {
        if let Some(id) = my_id {
          shared.unregister(Role::Recv, id);
        }
        shared.notify_senders();
        return Ok(item);
      }
      if let Some(id) = my_id {
        shared.unregister(Role::Recv, id);
      }
      return Err(RecvError::Disconnected);
    }

    if my_id.is_some() {
      // Already linked (the Dekker recheck above failed): park.
      THREAD_SPIN_LIMIT.with(|c| { let cur = c.get(); c.set((cur / 2).max(SPIN_MIN)); });
      thread::park();
      if notified.swap(false, Ordering::Acquire) {
        my_id = None;
        backoff = 0;
      }
      continue;
    }

    if backoff < spin_limit {
      std::hint::spin_loop();
      backoff += 1;
      continue;
    }

    // Register, fence, then loop for the Dekker recheck before parking.
    let id = shared.register(Role::Recv, None, WakeRef::Thread(thread::current()), notified_ptr);
    my_id = Some(id);
    shared.pre_park_fence();
  }
}

pub(crate) fn recv_timeout_sync<T: Send>(
  shared: &Arc<Shared<T>>,
  timeout: Duration,
) -> Result<T, RecvErrorTimeout> {
  let deadline = Instant::now().checked_add(timeout);
  let mut my_id: Option<u64> = None;

  if let Some(item) = shared.ring.pop() {
    shared.notify_senders();
    return Ok(item);
  }

  loop {
    if !shared.senders_alive() {
      if let Some(item) = shared.ring.pop() {
        shared.notify_senders();
        return Ok(item);
      }
      return Err(RecvErrorTimeout::Disconnected);
    }

    let now = Instant::now();
    let remaining = match deadline {
      Some(d) if d > now => d - now,
      // Either the deadline passed or `timeout` overflowed `Instant`. If it
      // overflowed (deadline == None) treat it as "effectively forever".
      Some(_) => {
        if let Some(id) = my_id.take() {
          shared.unregister(Role::Recv, id);
        }
        return Err(RecvErrorTimeout::Timeout);
      }
      None => Duration::from_secs(3600),
    };

    let id = shared.register(Role::Recv, my_id, WakeRef::Thread(thread::current()), ptr::null());
    my_id = Some(id);
    shared.pre_park_fence();

    if let Some(item) = shared.ring.pop() {
      shared.unregister(Role::Recv, id);
      shared.notify_senders();
      return Ok(item);
    }
    if !shared.senders_alive() {
      shared.unregister(Role::Recv, id);
      if let Some(item) = shared.ring.pop() {
        shared.notify_senders();
        return Ok(item);
      }
      return Err(RecvErrorTimeout::Disconnected);
    }

    thread::park_timeout(remaining);
  }
}

pub(crate) fn try_send<T: Send>(shared: &Arc<Shared<T>>, item: T) -> Result<(), (T, bool)> {
  // Err carries (item, closed): closed == true means receivers are gone.
  if !shared.receivers_alive() {
    return Err((item, true));
  }
  match shared.ring.push(item) {
    Ok(()) => {
      shared.notify_receivers();
      Ok(())
    }
    Err(returned) => Err((returned, false)),
  }
}

// --- Batch ---------------------------------------------------------------

/// Pushes items in order, as many as fit. Returns `(sent, unsent, reason)`:
/// `reason == None` means every item was sent; `Some(Full)` / `Some(Closed)`
/// means it stopped early with `unsent` holding the in-order remainder.
/// Wakes receivers once if anything was pushed.
pub(crate) fn try_send_batch_core<T: Send>(
  shared: &Arc<Shared<T>>,
  items: Vec<T>,
) -> (usize, Vec<T>, Option<BatchSendErrorReason>) {
  if items.is_empty() {
    return (0, items, None);
  }
  if !shared.receivers_alive() {
    return (0, items, Some(BatchSendErrorReason::Closed));
  }
  let mut sent = 0usize;
  let mut iter = items.into_iter();
  while let Some(item) = iter.next() {
    match shared.ring.push(item) {
      Ok(()) => {
        sent += 1;
        shared.notify_receivers(); // FIX: Call per item to wake up to `sent` receivers
      }
      Err(returned) => {
        let mut unsent = Vec::with_capacity(iter.len() + 1);
        unsent.push(returned);
        unsent.extend(iter);
        return (sent, unsent, Some(BatchSendErrorReason::Full));
      }
    }
  }
  if sent > 0 {
    shared.notify_receivers();
  }
  (sent, Vec::new(), None)
}

/// Blocking batch send: places every item, parking on a full ring, until done
/// or all receivers drop.
pub(crate) fn send_batch_sync<T: Send>(
  shared: &Arc<Shared<T>>,
  items: Vec<T>,
) -> Result<usize, SendBatchError<T>> {
  let mut iter = items.into_iter();
  let mut sent = 0usize;
  let mut my_id: Option<u64> = None;

  while let Some(mut item) = iter.next() {
    loop {
      if !shared.receivers_alive() {
        if let Some(id) = my_id.take() {
          shared.unregister(Role::Send, id);
        }
        let mut unsent = Vec::with_capacity(iter.len() + 1);
        unsent.push(item);
        unsent.extend(iter);
        return Err(SendBatchError { sent, unsent });
      }
      match shared.ring.push(item) {
        Ok(()) => {
          sent += 1;
          shared.notify_receivers();
          break;
        }
        Err(returned) => item = returned,
      }

      let id = shared.register(Role::Send, my_id, WakeRef::Thread(thread::current()), ptr::null());
      my_id = Some(id);
      shared.pre_park_fence();

      match shared.ring.push(item) {
        Ok(()) => {
          sent += 1;
          shared.notify_receivers();
          break;
        }
        Err(returned) => item = returned,
      }
      if !shared.receivers_alive() {
        continue;
      }
      thread::park();
    }
  }

  if let Some(id) = my_id.take() {
    shared.unregister(Role::Send, id);
  }
  Ok(sent)
}

/// Non-blocking batch receive: drains up to `max` into `out`. Wakes senders per
/// drained item.
pub(crate) fn try_recv_batch_core<T: Send>(
  shared: &Arc<Shared<T>>,
  out: &mut Vec<T>,
  max: usize,
) -> Result<usize, TryRecvError> {
  if max == 0 {
    return Ok(0);
  }
  let mut got = 0usize;
  while got < max {
    match shared.ring.pop() {
      Some(item) => {
        out.push(item);
        got += 1;
        shared.notify_senders();
      }
      None => break,
    }
  }
  if got > 0 {
    Ok(got)
  } else if !shared.senders_alive() {
    if let Some(item) = shared.ring.pop() {
      out.push(item);
      shared.notify_senders();
      Ok(1)
    } else {
      Err(TryRecvError::Disconnected)
    }
  } else {
    Err(TryRecvError::Empty)
  }
}

/// Blocking batch receive: blocks until at least one item is available, then
/// drains up to `max` without further blocking.
pub(crate) fn recv_batch_sync<T: Send>(
  shared: &Arc<Shared<T>>,
  out: &mut Vec<T>,
  max: usize,
) -> Result<usize, RecvError> {
  if max == 0 {
    return Ok(0);
  }
  let first = recv_sync(shared)?;
  out.push(first);
  let mut got = 1usize;
  while got < max {
    match shared.ring.pop() {
      Some(item) => {
        out.push(item);
        got += 1;
        shared.notify_senders();
      }
      None => break,
    }
  }
  Ok(got)
}

pub(crate) fn try_recv<T: Send>(shared: &Arc<Shared<T>>) -> Result<T, TryRecvError> {
  match shared.ring.pop() {
    Some(item) => {
      shared.notify_senders();
      Ok(item)
    }
    None => {
      if !shared.senders_alive() {
        // Re-check the ring once: a sender may have published then dropped.
        if let Some(item) = shared.ring.pop() {
          shared.notify_senders();
          return Ok(item);
        }
        Err(TryRecvError::Disconnected)
      } else {
        Err(TryRecvError::Empty)
      }
    }
  }
}
