// src/mpsc/lockfree.rs

use crate::async_util::AtomicWaker;
use crate::error::{RecvError, SendError, TryRecvError};
use crate::internal::cache_padded::CachePadded;
use crate::sync_util;

use core::marker::PhantomPinned;
use std::cell::UnsafeCell;
use std::fmt;
use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use std::ptr;
use std::sync::atomic::{self, AtomicBool, AtomicPtr, AtomicUsize, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};
use std::thread::{self, Thread};

/// A node in our lock-free linked list.
struct Node<T> {
  next: AtomicPtr<Node<T>>,
  value: UnsafeCell<Option<T>>,
}

/// The shared state of the MPSC channel.
pub(crate) struct MpscShared<T> {
  head: CachePadded<AtomicPtr<Node<T>>>,
  tail: CachePadded<UnsafeCell<*mut Node<T>>>,

  // --- Receiver waiting state ---
  // Sync waiter fields
  consumer_parked: AtomicBool,
  consumer_thread: UnsafeCell<Option<Thread>>,
  // Async waiter field
  consumer_waker: AtomicWaker,

  sender_count: AtomicUsize,
  pub(crate) current_len: AtomicUsize, // New: for tracking approximate length
}

impl<T> fmt::Debug for MpscShared<T> {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("MpscShared")
      .field("head", &self.head.load(Ordering::Relaxed))
      .field("tail", &"<UnsafeCell>")
      .field(
        "consumer_parked",
        &self.consumer_parked.load(Ordering::Relaxed),
      )
      .field("consumer_waker", &self.consumer_waker) // AtomicWaker has a Debug impl
      .field("sender_count", &self.sender_count.load(Ordering::Relaxed))
      .field("current_len", &self.current_len.load(Ordering::Relaxed))
      .finish_non_exhaustive()
  }
}

// It is safe to send MpscShared across threads if T is Send.
unsafe impl<T: Send> Send for MpscShared<T> {}
unsafe impl<T: Send> Sync for MpscShared<T> {}

impl<T: Send> MpscShared<T> {
  /// Creates a new, empty MPSC channel.
  pub(crate) fn new() -> Self {
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
      consumer_waker: AtomicWaker::new(),
      sender_count: AtomicUsize::new(1),
      current_len: AtomicUsize::new(0),
    }
  }

  /// Wakes the consumer if it is parked, whether synchronously or asynchronously.
  #[inline]
  fn wake_consumer(&self) {
    // Wake the sync consumer if it's parked.
    if self.consumer_parked.load(Ordering::Relaxed) {
      atomic::fence(Ordering::Acquire);
      if self
        .consumer_parked
        .compare_exchange(true, false, Ordering::AcqRel, Ordering::Relaxed)
        .is_ok()
      {
        if let Some(thread_handle) = unsafe { (*self.consumer_thread.get()).take() } {
          sync_util::unpark_thread(&thread_handle);
        }
      }
    }
    // Always wake the async waker. It's cheap and handles its own state.
    self.consumer_waker.wake();
  }

  /// The core non-blocking receive logic, used by both sync and async consumers.
  pub(crate) fn try_recv_internal(&self) -> Result<T, TryRecvError> {
    unsafe {
      let tail_ptr = *self.tail.get();
      let next_ptr = (*tail_ptr).next.load(Ordering::Acquire);

      if next_ptr.is_null() {
        if self.sender_count.load(Ordering::Acquire) == 0 {
          Err(TryRecvError::Disconnected)
        } else {
          Err(TryRecvError::Empty)
        }
      } else {
        let value = (*(*next_ptr).value.get()).take().unwrap();
        *self.tail.get() = next_ptr;
        self.current_len.fetch_sub(1, Ordering::Relaxed); // Decrement length
        drop(Box::from_raw(tail_ptr));
        Ok(value)
      }
    }
  }
}

impl<T> Drop for MpscShared<T> {
  fn drop(&mut self) {
    let mut current_node_ptr = *self.tail.get_mut();
    while !current_node_ptr.is_null() {
      let node_box = unsafe { Box::from_raw(current_node_ptr) };
      // If node_box.value contains Some(T), it will be dropped here.
      // current_len is not decremented here as it's an approximate count and
      // the shared state is being destroyed.
      current_node_ptr = node_box.next.load(Ordering::Relaxed);
    }
  }
}

// --- Sender & AsyncPSender ---
#[derive(Debug)]
pub struct Sender<T: Send> {
  pub(crate) shared: Arc<MpscShared<T>>,
}
#[derive(Debug)]
pub struct AsyncSender<T: Send> {
  pub(crate) shared: Arc<MpscShared<T>>,
}

// Common logic for both producer types
fn send_internal<T: Send>(shared: &Arc<MpscShared<T>>, value: T) -> Result<(), SendError> {
  // Check if consumer has dropped before attempting to send.
  // This is a best-effort check; consumer could drop after this check but before node is linked.
  // MPSC typically allows sends even if consumer is gone; the items queue up and are dropped
  // when MpscShared drops. However, for API consistency with other channel types,
  // we can return SendError::Closed if sender_count > 0 but consumer is known to be gone.
  // For MPSC, the `sender_count` is for senders. The "closed" state for senders
  // is usually when *they* cannot send (e.g. oneshot).
  // Here, the primary error is receiver dropping. MPSC doesn't have a direct "receiver_dropped" flag.
  // The MPSC contract usually means senders can send until they all drop,
  // and the receiver gets Disconnected then.
  // For now, MPSC send will succeed unless it's a shared error, not typically returning SendError::Closed.
  // If we wanted to detect a dropped receiver and fail sends, MpscShared would need a consumer_dropped flag.

  let new_node = Box::new(Node {
    next: AtomicPtr::new(ptr::null_mut()),
    value: UnsafeCell::new(Some(value)),
  });
  let new_node_ptr = Box::into_raw(new_node);

  let old_head_ptr = shared.head.swap(new_node_ptr, Ordering::AcqRel);
  unsafe {
    (*old_head_ptr).next.store(new_node_ptr, Ordering::Release);
  }

  shared.current_len.fetch_add(1, Ordering::Relaxed); // Increment length
  shared.wake_consumer();
  Ok(())
}

impl<T: Send> Sender<T> {
  pub fn send(&self, value: T) -> Result<(), SendError> {
    send_internal(&self.shared, value)
  }

  /// Returns the approximate number of items currently in the channel.
  pub fn len(&self) -> usize {
    self.shared.current_len.load(Ordering::Relaxed)
  }
}

impl<T: Send> AsyncSender<T> {
  // For an unbounded MPSC channel, send is effectively non-blocking.
  // The future completes immediately.
  pub fn send(&self, value: T) -> SendFuture<'_, T> {
    SendFuture {
      producer: self,
      value: Some(value),
      _phantom: PhantomPinned,
    }
  }

  /// Returns the approximate number of items currently in the channel.
  pub fn len(&self) -> usize {
    self.shared.current_len.load(Ordering::Relaxed)
  }
}

// --- Cloning and Dropping Senders ---
impl<T: Send> Clone for Sender<T> {
  fn clone(&self) -> Self {
    self.shared.sender_count.fetch_add(1, Ordering::Relaxed);
    Sender {
      shared: Arc::clone(&self.shared),
    }
  }
}
impl<T: Send> Drop for Sender<T> {
  fn drop(&mut self) {
    if self.shared.sender_count.fetch_sub(1, Ordering::AcqRel) == 1 {
      // This was the last sender. Wake the consumer so it can see Disconnected.
      self.shared.wake_consumer();
    }
  }
}
impl<T: Send> Clone for AsyncSender<T> {
  fn clone(&self) -> Self {
    self.shared.sender_count.fetch_add(1, Ordering::Relaxed);
    AsyncSender {
      shared: Arc::clone(&self.shared),
    }
  }
}
impl<T: Send> Drop for AsyncSender<T> {
  fn drop(&mut self) {
    if self.shared.sender_count.fetch_sub(1, Ordering::AcqRel) == 1 {
      // This was the last sender. Wake the consumer so it can see Disconnected.
      self.shared.wake_consumer();
    }
  }
}

// --- Receiver & AsyncReceiver ---
#[derive(Debug)]
pub struct Receiver<T: Send> {
  pub(crate) shared: Arc<MpscShared<T>>,
  pub(crate) _phantom: PhantomData<*mut ()>, // Makes it !Sync
}
unsafe impl<T: Send> Send for Receiver<T> {}
#[derive(Debug)]
pub struct AsyncReceiver<T: Send> {
  pub(crate) shared: Arc<MpscShared<T>>,
  pub(crate) _phantom: PhantomData<*mut ()>, // Makes it !Sync
}
unsafe impl<T: Send> Send for AsyncReceiver<T> {}

// --- Sync Receiver Implementation ---
impl<T: Send> Receiver<T> {
  pub fn try_recv(&mut self) -> Result<T, TryRecvError> {
    self.shared.try_recv_internal()
  }
  pub fn recv(&mut self) -> Result<T, RecvError> {
    loop {
      match self.try_recv() {
        Ok(value) => return Ok(value),
        Err(TryRecvError::Disconnected) => return Err(RecvError::Disconnected),
        Err(TryRecvError::Empty) => {
          unsafe {
            *self.shared.consumer_thread.get() = Some(thread::current());
          }
          self.shared.consumer_parked.store(true, Ordering::Release);

          // Critical re-check to prevent lost wakeups.
          // If an item arrived (or channel disconnected) between try_recv and parking setup,
          // we must handle it without parking.
          if let Ok(value) = self.try_recv() {
            // Item became available. Try to unarm parking.
            if self
              .shared
              .consumer_parked
              .compare_exchange(true, false, Ordering::AcqRel, Ordering::Relaxed)
              .is_ok()
            {
              // Successfully un-armed. Clear the thread handle.
              unsafe {
                *self.shared.consumer_thread.get() = None;
              }
            }
            return Ok(value);
          }
          // Check for disconnection again after setting parked flag
          if self.shared.sender_count.load(Ordering::Acquire) == 0 {
            let tail_ptr = unsafe { *self.shared.tail.get() };
            let next_ptr = unsafe { (*tail_ptr).next.load(Ordering::Acquire) };
            if next_ptr.is_null() {
              // Still empty
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
              return Err(RecvError::Disconnected);
            }
            // Item appeared, loop to get it
          }

          sync_util::park_thread();

          // After unparking, clear the flag if it was still set by us.
          // This handles spurious wakeups or cases where waker clears flag before we run.
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

  /// Returns the approximate number of items currently in the channel.
  pub fn len(&self) -> usize {
    self.shared.current_len.load(Ordering::Relaxed)
  }

  /// Returns `true` if the channel is currently empty from this receiver's perspective.
  /// This is an approximation; new items might be added concurrently.
  pub fn is_empty(&self) -> bool {
    // Check the atomic length counter first for a quick path.
    if self.shared.current_len.load(Ordering::Relaxed) == 0 {
      // If length is 0, verify by checking the actual list structure.
      let tail_node_ptr = unsafe { *self.shared.tail.get() };
      let next_node_ptr = unsafe { (*tail_node_ptr).next.load(Ordering::Acquire) };
      return next_node_ptr.is_null();
    }
    false
  }
}

// --- Async Receiver Implementation ---
impl<T: Send> AsyncReceiver<T> {
  pub fn try_recv(&mut self) -> Result<T, TryRecvError> {
    self.shared.try_recv_internal()
  }
  pub fn recv(&mut self) -> RecvFuture<'_, T> {
    RecvFuture { consumer: self }
  }

  /// Returns the approximate number of items currently in the channel.
  pub fn len(&self) -> usize {
    self.shared.current_len.load(Ordering::Relaxed)
  }

  /// Returns `true` if the channel is currently empty from this receiver's perspective.
  /// This is an approximation; new items might be added concurrently.
  pub fn is_empty(&self) -> bool {
    if self.shared.current_len.load(Ordering::Relaxed) == 0 {
      let tail_node_ptr = unsafe { *self.shared.tail.get() };
      let next_node_ptr = unsafe { (*tail_node_ptr).next.load(Ordering::Acquire) };
      return next_node_ptr.is_null();
    }
    false
  }
}

// --- Dropping Receivers ---
// When the Receiver (sync or async) is dropped, we need to ensure any
// items remaining in the queue are also dropped to prevent memory leaks.
impl<T: Send> Drop for Receiver<T> {
  fn drop(&mut self) {
    // Drain the queue. try_recv_internal handles decrementing current_len.
    while self.shared.try_recv_internal().is_ok() {
      // Keep draining
    }
    // Any remaining nodes in the MpscShared list (stub node) will be cleaned by MpscShared::drop
  }
}
impl<T: Send> Drop for AsyncReceiver<T> {
  fn drop(&mut self) {
    // Drain the queue
    while self.shared.try_recv_internal().is_ok() {
      // Keep draining
    }
  }
}

// --- Future Implementations ---

// SendFuture for MPSC is trivial because the send is lock-free and non-blocking.
#[must_use = "futures do nothing unless you .await or poll them"]
pub struct SendFuture<'a, T: Send> {
  producer: &'a AsyncSender<T>,
  value: Option<T>,
  _phantom: PhantomPinned,
}

impl<'a, T: Send> Future for SendFuture<'a, T> {
  type Output = Result<(), SendError>;
  fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
    let this = unsafe { self.as_mut().get_unchecked_mut() };
    let value = this
      .value
      .take()
      .expect("SendFuture polled after completion");
    Poll::Ready(send_internal(&this.producer.shared, value))
  }
}

#[must_use = "futures do nothing unless you .await or poll them"]
pub struct RecvFuture<'a, T: Send> {
  consumer: &'a mut AsyncReceiver<T>,
}

impl<'a, T: Send> Future for RecvFuture<'a, T> {
  type Output = Result<T, RecvError>;
  fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    loop {
      // Loop to handle state changes after waker registration
      match self.consumer.shared.try_recv_internal() {
        Ok(value) => return Poll::Ready(Ok(value)),
        Err(TryRecvError::Disconnected) => return Poll::Ready(Err(RecvError::Disconnected)),
        Err(TryRecvError::Empty) => {
          // Queue is empty. Register the waker.
          self.consumer.shared.consumer_waker.register(cx.waker());

          // Critical re-check to prevent lost wakeups.
          // This ensures that if an item arrived *between* the first try_recv_internal
          // and the waker registration, we pick it up.
          match self.consumer.shared.try_recv_internal() {
            Ok(value) => {
              // Item became available. Waker was registered but we got the value. Fine.
              return Poll::Ready(Ok(value));
            }
            Err(TryRecvError::Disconnected) => {
              // Became disconnected.
              return Poll::Ready(Err(RecvError::Disconnected));
            }
            Err(TryRecvError::Empty) => {
              // Still empty. Check if all senders are gone *after* confirming empty.
              if self.consumer.shared.sender_count.load(Ordering::Acquire) == 0 {
                // Re-check queue one last time if all senders are gone,
                // as a sender might have sent an item then immediately updated sender_count.
                match self.consumer.shared.try_recv_internal() {
                  Ok(value) => return Poll::Ready(Ok(value)),
                  Err(TryRecvError::Disconnected) => {
                    return Poll::Ready(Err(RecvError::Disconnected))
                  }
                  Err(TryRecvError::Empty) => return Poll::Ready(Err(RecvError::Disconnected)), // All senders gone and still empty
                }
              }
              // Senders still active, waker is registered. It's safe to park.
              return Poll::Pending;
            }
          }
        }
      }
    }
  }
}
