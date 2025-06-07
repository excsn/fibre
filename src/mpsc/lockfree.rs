use crate::async_util::AtomicWaker;
use crate::error::{RecvError, SendError, TryRecvError};
use crate::internal::cache_padded::CachePadded;
use crate::sync_util;

use core::marker::PhantomPinned;
use futures_util::stream::Stream;
use std::cell::UnsafeCell;
use std::fmt;
use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex}; // <--- ADDED Mutex
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
  consumer_thread: Mutex<Option<Thread>>, // <--- CORRECTED: Use a Mutex to prevent data races.
  // Async waiter field
  consumer_waker: AtomicWaker,
  receiver_dropped: AtomicBool,

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
      consumer_thread: Mutex::new(None), // <--- CORRECTED
      consumer_waker: AtomicWaker::new(),
      receiver_dropped: AtomicBool::new(false),
      sender_count: AtomicUsize::new(1),
      current_len: AtomicUsize::new(0),
    }
  }

  /// Wakes the consumer if it is parked, whether synchronously or asynchronously.
  #[inline]
  fn wake_consumer(&self) {
    // Always wake the async waker. It handles its own state and is cheap.
    self.consumer_waker.wake();

    // For the synchronous consumer:
    // Attempt to transition `consumer_parked` from `true` to `false`.
    // If successful, this thread takes responsibility for unparking.
    if self
      .consumer_parked
      .compare_exchange(true, false, Ordering::AcqRel, Ordering::Acquire)
      .is_ok()
    {
      // <--- CORRECTED: Lock mutex to safely access the thread handle.
      // Successfully acquired the "right to unpark" by setting flag to false.
      if let Some(thread_handle) = self.consumer_thread.lock().unwrap().take() {
        // Unpark the thread. It's fine to do this after the lock is released.
        sync_util::unpark_thread(&thread_handle);
      }
      // If thread_handle was None, it's a logic error, but we've prevented a data race.
    }
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

  // ADD THIS NEW, INTERNAL POLLING FUNCTION
  pub(crate) fn poll_recv_internal(&self, cx: &mut Context<'_>) -> Poll<Result<T, RecvError>> {
    loop {
      match self.try_recv_internal() {
        Ok(value) => return Poll::Ready(Ok(value)),
        Err(TryRecvError::Disconnected) => return Poll::Ready(Err(RecvError::Disconnected)),
        Err(TryRecvError::Empty) => {
          // Queue is empty. Register the waker.
          self.consumer_waker.register(cx.waker());

          // Critical re-check to prevent lost wakeups.
          match self.try_recv_internal() {
            Ok(value) => return Poll::Ready(Ok(value)),
            Err(TryRecvError::Disconnected) => return Poll::Ready(Err(RecvError::Disconnected)),
            Err(TryRecvError::Empty) => {
              // Still empty. Check if all senders are gone *after* confirming empty.
              if self.sender_count.load(Ordering::Acquire) == 0 {
                // Re-check queue one last time if all senders are gone
                match self.try_recv_internal() {
                  Ok(value) => return Poll::Ready(Ok(value)),
                  _ => return Poll::Ready(Err(RecvError::Disconnected)),
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

impl<T> Drop for MpscShared<T> {
  fn drop(&mut self) {
    // Now that sends are blocked by `receiver_dropped` before this can run,
    // we can safely clean up the list.
    let mut current_ptr = unsafe { (*(*self.tail.get_mut())).next.load(Ordering::Relaxed) };
    while !current_ptr.is_null() {
      let node = unsafe { Box::from_raw(current_ptr) };
      current_ptr = node.next.load(Ordering::Relaxed);
      // node and its value are dropped here
    }

    // Finally, drop the stub node itself.
    let stub_ptr = *self.tail.get_mut();
    if !stub_ptr.is_null() {
      unsafe {
        drop(Box::from_raw(stub_ptr));
      }
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
  if shared.receiver_dropped.load(Ordering::Acquire) {
    // The receiver is gone or being dropped, so we can't send.
    // This check prevents us from modifying the queue during receiver cleanup.
    return Err(SendError::Closed);
  }

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
}
unsafe impl<T: Send> Send for Receiver<T> {}
#[derive(Debug)]
pub struct AsyncReceiver<T: Send> {
  pub(crate) shared: Arc<MpscShared<T>>,
}
unsafe impl<T: Send> Send for AsyncReceiver<T> {}

// --- Sync Receiver Implementation ---
impl<T: Send> Receiver<T> {
  pub fn try_recv(&mut self) -> Result<T, TryRecvError> {
    self.shared.try_recv_internal()
  }
  pub fn recv(&mut self) -> Result<T, RecvError> {
    loop {
      // First attempt to receive. This is the hot path.
      match self.try_recv() {
        Ok(value) => return Ok(value),
        Err(TryRecvError::Disconnected) => return Err(RecvError::Disconnected),
        Err(TryRecvError::Empty) => {
          // Queue is empty, prepare to park.
        }
      }

      // --- Parking Logic ---
      // Prepare for parking by setting the thread handle and flag.
      *self.shared.consumer_thread.lock().unwrap() = Some(thread::current());
      self.shared.consumer_parked.store(true, Ordering::Release);

      // **CRITICAL RE-CHECK** after arming the park.
      // This is the only re-check needed. By calling the full try_recv, we avoid race conditions.
      match self.try_recv() {
        Ok(value) => {
          // A value arrived before we parked. Disarm and return the value.
          if self
            .shared
            .consumer_parked
            .compare_exchange(true, false, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
          {
            *self.shared.consumer_thread.lock().unwrap() = None;
          }
          return Ok(value);
        }
        Err(TryRecvError::Disconnected) => {
          // Channel became disconnected. Disarm and return error.
          if self
            .shared
            .consumer_parked
            .compare_exchange(true, false, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
          {
            *self.shared.consumer_thread.lock().unwrap() = None;
          }
          return Err(RecvError::Disconnected);
        }
        Err(TryRecvError::Empty) => {
          // Still empty. It is now safe to park.
          sync_util::park_thread();

          // After waking up, disarm the parker state just in case of a spurious wakeup.
          // The waker already set the flag to false, but this handles other cases.
          if self
            .shared
            .consumer_parked
            .compare_exchange(true, false, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
          {
            *self.shared.consumer_thread.lock().unwrap() = None;
          }
        }
      }
      // Loop back to the top to try receiving again.
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
  pub fn recv(&self) -> RecvFuture<'_, T> {
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
    // 1. Signal to all producers that the receiver is gone.
    //    This must happen *before* we start modifying the queue.
    self.shared.receiver_dropped.store(true, Ordering::Release);
    // Drain the queue. try_recv_internal handles decrementing current_len.
    while self.shared.try_recv_internal().is_ok() {
      // Keep draining
    }
  }
}
impl<T: Send> Drop for AsyncReceiver<T> {
  fn drop(&mut self) {
    self.shared.receiver_dropped.store(true, Ordering::Release);
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
  consumer: &'a AsyncReceiver<T>,
}

impl<'a, T: Send> Future for RecvFuture<'a, T> {
  type Output = Result<T, RecvError>;
  fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    self.consumer.shared.poll_recv_internal(cx)
  }
}

impl<T: Send> Stream for AsyncReceiver<T> {
  type Item = T;

  fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
    match self.shared.poll_recv_internal(cx) {
      Poll::Ready(Ok(value)) => Poll::Ready(Some(value)),
      Poll::Ready(Err(_)) => Poll::Ready(None),
      Poll::Pending => Poll::Pending,
    }
  }
}
