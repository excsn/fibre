//! Implementation of the synchronous, blocking send and receive logic for the MPMC channel.

use super::{Receiver, Sender};
use crate::error::{RecvError, SendError, TryRecvError, TrySendError};
use crate::internal::waiter::{AsyncWaiterNode, SyncWaiterNode};
use crate::mpmc_exp::backoff;
use crate::RecvErrorTimeout;

use std::pin::Pin;
use std::ptr::NonNull;
use std::sync::atomic::Ordering;
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

    // --- Prepare to park (optimistic allocation on stack) ---
    let mut node = SyncWaiterNode::new(thread::current());
    node.item = Some(item_to_send);

    // Pin the node on the stack.
    // Safety: We do not move `node` after this point until it is dropped/goes out of scope.
    let mut pinned_node = unsafe { Pin::new_unchecked(&mut node) };
    let node_ptr = unsafe { NonNull::from(pinned_node.as_mut().get_unchecked_mut()) };

    // --- Single Lock Scope ---
    {
      let mut guard = sender.shared.internal.lock();

      if guard.receiver_count == 0 {
        return Err(SendError::Closed);
      }

      // 1. Priority: Wake a waiting receiver (Direct Handoff)
      if let Some(mut waiter_ptr) = unsafe { guard.waiting_async_receivers.pop_front() } {
        let waiter = unsafe { waiter_ptr.as_mut() };
        // Take item from OUR node and give to RECEIVER node
        let item = unsafe { pinned_node.as_mut().get_unchecked_mut() }.item.take().unwrap();
        waiter.item = Some(item);
        drop(guard);
        waiter.wake();
        return Ok(());
      }
      if let Some(mut waiter_ptr) = unsafe { guard.waiting_sync_receivers.pop_front() } {
        let waiter = unsafe { waiter_ptr.as_mut() };
        // Take item from OUR node and give to RECEIVER node
        let item = unsafe { pinned_node.as_mut().get_unchecked_mut() }.item.take().unwrap();
        waiter.item = Some(item);
        drop(guard);
        waiter.wake();
        return Ok(());
      }

      // 2. Priority: Push to buffer
      if sender.shared.capacity > 0 && guard.queue.len() < sender.shared.capacity {
        let item = unsafe { pinned_node.as_mut().get_unchecked_mut() }.item.take().unwrap();
        guard.queue.push_back(item);
        drop(guard);
        return Ok(());
      }

      // 3. Fallback: Park
      unsafe {
        guard.waiting_sync_senders.push_back(node_ptr);
      }
    }

    // --- Phase 4: Wait ---
    // The adaptive wait will spin, then yield, then park until `done_flag` is true.
    backoff::adaptive_wait(|| pinned_node.notified.load(Ordering::Acquire));

    // --- Phase 5: Handle wake-up ---
    // Ensure we are removed from the queue.
    {
      let mut guard = sender.shared.internal.lock();
      unsafe {
        guard.waiting_sync_senders.remove(node_ptr);
      }
    }

    if pinned_node.item.is_none() {
      // Item was taken, send successful.
      return Ok(());
    } else {
      // Item was not taken (spurious wake or closure). Retrieve it.
      current_item_opt = unsafe { pinned_node.as_mut().get_unchecked_mut() }.item.take();
      if sender.is_closed() {
        return Err(SendError::Closed);
      }
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
    // --- Prepare to park ---
    let mut node = SyncWaiterNode::<T>::new(thread::current());
    let mut pinned_node = unsafe { Pin::new_unchecked(&mut node) };
    let node_ptr = unsafe { NonNull::from(pinned_node.as_mut().get_unchecked_mut()) };

    // --- Single Lock Scope ---
    {
      let mut guard = receiver.shared.internal.lock();

      // 1. Priority: Check buffer
      if let Some(item) = guard.queue.pop_front() {
        // Wake a buffered sender if we freed space
        let mut sender_to_wake: Option<NonNull<()>> = None;
        let mut is_async = false;
        if receiver.shared.capacity > 0 {
          if let Some(mut sender_ptr) = unsafe { guard.waiting_async_senders.pop_front() } {
            let sender = unsafe { sender_ptr.as_mut() };
            let sender_item = sender.item.take().expect("Sender must have an item");
            guard.queue.push_back(sender_item);
            sender_to_wake = Some(sender_ptr.cast::<()>());
            is_async = true;
          } else if let Some(mut sender_ptr) = unsafe { guard.waiting_sync_senders.pop_front() } {
            let sender = unsafe { sender_ptr.as_mut() };
            let sender_item = sender.item.take().expect("Sender must have an item");
            guard.queue.push_back(sender_item);
            sender_to_wake = Some(sender_ptr.cast::<()>());
            is_async = false;
          }
        }
        drop(guard);
        if let Some(sender_ptr) = sender_to_wake {
          unsafe {
            if is_async {
              let ptr = sender_ptr.cast::<AsyncWaiterNode<T>>();
              ptr.as_ref().wake();
            } else {
              let ptr = sender_ptr.cast::<SyncWaiterNode<T>>();
              ptr.as_ref().wake();
            }
          }
        }
        return Ok(item);
      }

      // 2. Priority: Rendezvous Handoff
      if let Some(mut sender_ptr) = unsafe { guard.waiting_async_senders.pop_front() } {
        let sender = unsafe { sender_ptr.as_mut() };
        let item = sender.item.take().expect("Sender must have an item");
        drop(guard);
        sender.wake();
        return Ok(item);
      }
      if let Some(mut sender_ptr) = unsafe { guard.waiting_sync_senders.pop_front() } {
        let sender = unsafe { sender_ptr.as_mut() };
        let item = sender.item.take().expect("Sender must have an item");
        drop(guard);
        sender.wake();
        return Ok(item);
      }

      // 3. Check Disconnect
      if guard.sender_count == 0 {
        return Err(RecvError::Disconnected);
      }

      // 4. Park
      unsafe {
        guard.waiting_sync_receivers.push_back(node_ptr);
      }
    }

    // --- Phase 4: Wait ---
    backoff::adaptive_wait(|| pinned_node.notified.load(Ordering::Acquire));

    // --- Phase 5: Handle wake-up ---
    {
      let mut guard = receiver.shared.internal.lock();
      unsafe {
        guard.waiting_sync_receivers.remove(node_ptr);
      }
    }

    // Check if we received an item via direct handoff
    if let Some(item) = unsafe { pinned_node.as_mut().get_unchecked_mut() }.item.take() {
      return Ok(item);
    }
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
    let mut node = SyncWaiterNode::<T>::new(thread::current());
    let mut pinned_node = unsafe { Pin::new_unchecked(&mut node) };
    let node_ptr = unsafe { NonNull::from(pinned_node.as_mut().get_unchecked_mut()) };

    // --- Phase 3: Lock, re-check, and commit to parking ---
    {
      let mut guard = receiver.shared.internal.lock();

      // Re-check state under lock.
      if !guard.queue.is_empty()
        || (receiver.shared.capacity == 0 && (!guard.waiting_sync_senders.is_empty() || !guard.waiting_async_senders.is_empty()))
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

      unsafe {
        guard.waiting_sync_receivers.push_back(node_ptr);
      }
    }

    // --- Phase 4: Wait with timeout ---
    thread::park_timeout(remaining_timeout);

    // --- Phase 5: Handle wake-up ---
    {
      let mut guard = receiver.shared.internal.lock();
      unsafe {
        guard.waiting_sync_receivers.remove(node_ptr);
      }
    }

    // Check if we received an item via direct handoff
    if let Some(item) = unsafe { pinned_node.as_mut().get_unchecked_mut() }.item.take() {
      return Ok(item);
    }

    match receiver.shared.try_recv_core() {
      Ok(item) => return Ok(item),
      Err(TryRecvError::Disconnected) => return Err(RecvErrorTimeout::Disconnected),
      Err(TryRecvError::Empty) => { /* Loop to check timeout and maybe park again */ }
    }
  }
}
