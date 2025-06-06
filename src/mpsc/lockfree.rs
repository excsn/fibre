use crate::async_util::AtomicWaker;
use crate::error::{RecvError, SendError, TryRecvError};
use crate::internal::cache_padded::CachePadded;
use crate::sync_util;

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

  // --- Consumer waiting state ---
  // Sync waiter fields
  consumer_parked: AtomicBool,
  consumer_thread: UnsafeCell<Option<Thread>>,
  // Async waiter field
  consumer_waker: AtomicWaker,

  sender_count: AtomicUsize,
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
  fn try_recv_internal(&self) -> Result<T, TryRecvError> {
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
      current_node_ptr = node_box.next.load(Ordering::Relaxed);
    }
  }
}

// --- Producer & AsyncProducer ---
#[derive(Debug)]
pub struct Producer<T: Send> {
  pub(crate) shared: Arc<MpscShared<T>>,
}
#[derive(Debug)]
pub struct AsyncProducer<T: Send> {
  pub(crate) shared: Arc<MpscShared<T>>,
}

// Common logic for both producer types
fn send_internal<T: Send>(shared: &Arc<MpscShared<T>>, value: T) -> Result<(), SendError> {
  let new_node = Box::new(Node {
    next: AtomicPtr::new(ptr::null_mut()),
    value: UnsafeCell::new(Some(value)),
  });
  let new_node_ptr = Box::into_raw(new_node);

  let old_head_ptr = shared.head.swap(new_node_ptr, Ordering::AcqRel);
  unsafe {
    (*old_head_ptr).next.store(new_node_ptr, Ordering::Release);
  }

  shared.wake_consumer();
  Ok(())
}

impl<T: Send> Producer<T> {
  pub fn send(&self, value: T) -> Result<(), SendError> {
    send_internal(&self.shared, value)
  }
}

impl<T: Send> AsyncProducer<T> {
  // For an unbounded MPSC channel, send is effectively non-blocking.
  // The future completes immediately.
  pub fn send(&self, value: T) -> SendFuture<'_, T> {
    SendFuture {
      producer: self,
      value: Some(value),
    }
  }
}

// --- Cloning and Dropping Producers ---
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
    if self.shared.sender_count.fetch_sub(1, Ordering::AcqRel) == 1 {
      self.shared.wake_consumer();
    }
  }
}
impl<T: Send> Clone for AsyncProducer<T> {
  fn clone(&self) -> Self {
    self.shared.sender_count.fetch_add(1, Ordering::Relaxed);
    AsyncProducer {
      shared: Arc::clone(&self.shared),
    }
  }
}
impl<T: Send> Drop for AsyncProducer<T> {
  fn drop(&mut self) {
    if self.shared.sender_count.fetch_sub(1, Ordering::AcqRel) == 1 {
      self.shared.wake_consumer();
    }
  }
}

// --- Consumer & AsyncConsumer ---
#[derive(Debug)]
pub struct Consumer<T: Send> {
  pub(crate) shared: Arc<MpscShared<T>>,
  pub(crate) _phantom: PhantomData<*mut ()>,
}
unsafe impl<T: Send> Send for Consumer<T> {}
#[derive(Debug)]
pub struct AsyncConsumer<T: Send> {
  pub(crate) shared: Arc<MpscShared<T>>,
  pub(crate) _phantom: PhantomData<*mut ()>,
}
unsafe impl<T: Send> Send for AsyncConsumer<T> {}

// --- Sync Consumer Implementation ---
impl<T: Send> Consumer<T> {
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

          if let Ok(value) = self.try_recv() {
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
            return Ok(value);
          }
          sync_util::park_thread();
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

// --- Async Consumer Implementation ---
impl<T: Send> AsyncConsumer<T> {
  pub fn try_recv(&mut self) -> Result<T, TryRecvError> {
    self.shared.try_recv_internal()
  }
  pub fn recv(&mut self) -> RecvFuture<'_, T> {
    RecvFuture { consumer: self }
  }
}

// --- Dropping Consumers ---
impl<T: Send> Drop for Consumer<T> {
  fn drop(&mut self) {
    while let Ok(_) = self.try_recv() {}
  }
}
impl<T: Send> Drop for AsyncConsumer<T> {
  fn drop(&mut self) {
    while let Ok(_) = self.try_recv() {}
  }
}

// --- Future Implementations ---

// SendFuture for MPSC is trivial because the send is lock-free and non-blocking.
#[must_use = "futures do nothing unless you .await or poll them"]
pub struct SendFuture<'a, T: Send> {
  producer: &'a AsyncProducer<T>,
  value: Option<T>,
}

// Add the `Unpin` bound here to allow mutable access inside poll.
impl<'a, T: Send + Unpin> Future for SendFuture<'a, T> {
  type Output = Result<(), SendError>;
  fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
    // Use self.as_mut() to get a mutable reference to the future's data.
    let value = self
      .as_mut()
      .value
      .take()
      .expect("SendFuture polled after completion");
    Poll::Ready(send_internal(&self.producer.shared, value))
  }
}

#[must_use = "futures do nothing unless you .await or poll them"]
pub struct RecvFuture<'a, T: Send> {
  consumer: &'a mut AsyncConsumer<T>,
}

impl<'a, T: Send> Future for RecvFuture<'a, T> {
  type Output = Result<T, RecvError>;
  fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    loop {
      match self.consumer.shared.try_recv_internal() {
        Ok(value) => return Poll::Ready(Ok(value)),
        Err(TryRecvError::Disconnected) => return Poll::Ready(Err(RecvError::Disconnected)),
        Err(TryRecvError::Empty) => {
          // Queue is empty. Register the waker.
          self.consumer.shared.consumer_waker.register(cx.waker());

          // Critical re-check to prevent lost wakeups.
          match self.consumer.shared.try_recv_internal() {
            Ok(value) => return Poll::Ready(Ok(value)),
            Err(TryRecvError::Disconnected) => return Poll::Ready(Err(RecvError::Disconnected)),
            Err(TryRecvError::Empty) => {
              // Still empty, waker is registered. It's safe to park.
              return Poll::Pending;
            }
          }
        }
      }
    }
  }
}
