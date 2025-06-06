// src/mpsc/lockfree.rs

use crate::error::{RecvError, SendError, TryRecvError, TrySendError};
use crate::internal::cache_padded::CachePadded;
use crate::sync_util;

use core::fmt;
use core::sync::atomic;
use std::cell::UnsafeCell;
use std::marker::PhantomData;
use std::mem;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread::{self, Thread};

/// A node in our lock-free linked list.
struct Node<T> {
  next: AtomicPtr<Node<T>>,
  value: UnsafeCell<Option<T>>, // Use Option for the stub node. UnsafeCell for interior mutability.
}

/// The shared state of the MPSC channel.
pub(crate) struct MpscShared<T> {
  // Padded to avoid false sharing between producers and the consumer.
  head: CachePadded<AtomicPtr<Node<T>>>,
  tail: CachePadded<UnsafeCell<*mut Node<T>>>,

  // Consumer waiting state.
  consumer_parked: AtomicBool,
  consumer_thread: UnsafeCell<Option<Thread>>,

  // The number of active producers.
  sender_count: AtomicUsize,
}

impl<T> fmt::Debug for MpscShared<T> {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("MpscShared")
      .field("head", &self.head.load(Ordering::Relaxed))
      // Note: We cannot safely inspect the tail pointer here without `&mut self`
      // or taking the lock if we had one. So we just indicate its presence.
      .field("tail", &"<UnsafeCell>")
      .field(
        "consumer_parked",
        &self.consumer_parked.load(Ordering::Relaxed),
      )
      .field("sender_count", &self.sender_count.load(Ordering::Relaxed))
      .finish_non_exhaustive() // Indicates there are other, unprinted fields
  }
}

// It is safe to send MpscShared across threads if T is Send.
unsafe impl<T: Send> Send for MpscShared<T> {}
unsafe impl<T: Send> Sync for MpscShared<T> {}

impl<T> MpscShared<T> {
  /// Creates a new, empty MPSC channel.
  pub(crate) fn new() -> Self {
    // Create the initial stub node. It contains no value.
    let stub = Box::new(Node {
      next: AtomicPtr::new(ptr::null_mut()),
      value: UnsafeCell::new(None),
    });
    let stub_ptr = Box::into_raw(stub);

    MpscShared {
      head: CachePadded::new(AtomicPtr::new(stub_ptr)),
      tail: CachePadded::new(UnsafeCell::new(stub_ptr)),
      consumer_parked: AtomicBool::new(false),
      consumer_thread: UnsafeCell::new(None),
      sender_count: AtomicUsize::new(1),
    }
  }

  /// Wakes the consumer thread if it is parked.
  #[inline]
  fn wake_consumer(&self) {
    // Check the flag first with a relaxed load, which is fast.
    if self.consumer_parked.load(Ordering::Relaxed) {
      // If the flag is set, attempt to acquire it and wake the thread.
      // The fence ensures that our view of the thread handle is up-to-date.
      atomic::fence(Ordering::Acquire);
      if self
        .consumer_parked
        .compare_exchange(true, false, Ordering::AcqRel, Ordering::Relaxed)
        .is_ok()
      {
        // We successfully acquired the "right to unpark".
        // It is now safe to take the thread handle.
        if let Some(thread_handle) = unsafe { (*self.consumer_thread.get()).take() } {
          sync_util::unpark_thread(&thread_handle);
        }
      }
    }
  }
}

impl<T> Drop for MpscShared<T> {
  fn drop(&mut self) {
    // Deallocate all remaining nodes to prevent memory leaks.
    let mut current_node_ptr = *self.tail.get_mut();
    while !current_node_ptr.is_null() {
      let node_box = unsafe { Box::from_raw(current_node_ptr) };
      // The value inside the node (if any) is dropped when node_box is dropped.
      current_node_ptr = node_box.next.load(Ordering::Relaxed);
    }
  }
}

// --- Producer ---
#[derive(Debug)]
pub struct Producer<T: Send> {
  pub(crate) shared: Arc<MpscShared<T>>,
}

impl<T: Send> Producer<T> {
  /// Sends a value into the channel.
  pub fn send(&self, value: T) -> Result<(), SendError> {
    let new_node = Box::new(Node {
      next: AtomicPtr::new(ptr::null_mut()),
      value: UnsafeCell::new(Some(value)),
    });
    let new_node_ptr = Box::into_raw(new_node);

    let old_head_ptr = self.shared.head.swap(new_node_ptr, Ordering::AcqRel);
    unsafe {
      (*old_head_ptr).next.store(new_node_ptr, Ordering::Release);
    }

    // After successfully sending, wake the consumer if it's parked.
    self.shared.wake_consumer();

    Ok(())
  }
}

impl<T: Send> Clone for Producer<T> {
  fn clone(&self) -> Self {
    self.shared.sender_count.fetch_add(1, Ordering::Relaxed);
    Producer {
      shared: Arc::clone(&self.shared),
    }
  }
}

impl<T: Send> Drop for Producer<T> {
  fn drop(&mut self) {
    // If we are the last producer, we must wake the consumer to signal disconnection.
    if self.shared.sender_count.fetch_sub(1, Ordering::AcqRel) == 1 {
      self.shared.wake_consumer();
    }
  }
}

// --- Consumer ---
#[derive(Debug)]
pub struct Consumer<T: Send> {
  pub(crate) shared: Arc<MpscShared<T>>,
  pub(crate) _phantom: PhantomData<*mut ()>,
}

// This tells the compiler that even though Consumer contains a raw pointer marker
// (making it !Sync), it is still safe to SEND the consumer handle to another thread.
// This is true because the underlying shared data is managed by an Arc and atomics.
unsafe impl<T: Send> Send for Consumer<T> {}

impl<T: Send> Consumer<T> {
  pub fn try_recv(&mut self) -> Result<T, TryRecvError> {
    unsafe {
      let tail_ptr = *self.shared.tail.get();
      let next_ptr = (*tail_ptr).next.load(Ordering::Acquire);

      if next_ptr.is_null() {
        if self.shared.sender_count.load(Ordering::Acquire) == 0 {
          Err(TryRecvError::Disconnected)
        } else {
          Err(TryRecvError::Empty)
        }
      } else {
        // There's a message. First, take the value out.
        // This is safe because we are the only consumer.
        let value = (*(*next_ptr).value.get()).take().unwrap();

        // Now, advance the tail pointer.
        *self.shared.tail.get() = next_ptr;

        // The old tail was the stub. Deallocate it.
        drop(Box::from_raw(tail_ptr));

        Ok(value)
      }
    }
  }

  pub fn recv(&mut self) -> Result<T, RecvError> {
    loop {
      match self.try_recv() {
        Ok(value) => return Ok(value),
        Err(TryRecvError::Disconnected) => return Err(RecvError::Disconnected),
        Err(TryRecvError::Empty) => {
          // Queue is empty, prepare to park.
          unsafe {
            // Store our thread handle.
            *self.shared.consumer_thread.get() = Some(thread::current());
          }
          // Set the park flag. Release ordering ensures the thread handle is visible before the flag.
          self.shared.consumer_parked.store(true, Ordering::Release);

          // **Critical Re-check**: A producer might have sent a message *between* our
          // `try_recv` and us setting the park flag. We must check again to avoid
          // a lost wakeup and indefinite sleep.
          if let Ok(value) = self.try_recv() {
            // A message appeared! We need to un-park ourselves.
            // We try to unset the flag. If we succeed, we can continue.
            // If we fail, it means a producer already saw the flag and is
            // trying to unpark us, which is also fine.
            if self
              .shared
              .consumer_parked
              .compare_exchange(true, false, Ordering::AcqRel, Ordering::Relaxed)
              .is_ok()
            {
              // We successfully took back the "right to unpark".
              // Clear the thread handle we just stored.
              unsafe {
                *self.shared.consumer_thread.get() = None;
              }
            }
            return Ok(value);
          }

          // No message on re-check. Park the thread and wait.
          sync_util::park_thread();

          // After waking up, clear the parked flag if it's still set by us.
          // This handles spurious wakeups.
          if self
            .shared
            .consumer_parked
            .compare_exchange(true, false, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
          {
            unsafe {
              *self.shared.consumer_thread.get() = None;
            }
          }
        }
      }
    }
  }
}

impl<T: Send> Drop for Consumer<T> {
  fn drop(&mut self) {
    // Drain the queue and drop any remaining values.
    while let Ok(_) = self.try_recv() {}
  }
}
