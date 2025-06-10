use crate::async_util::AtomicWaker;
use crate::error::{CloseError, RecvError, SendError, TryRecvError};
use crate::internal::cache_padded::CachePadded;
use crate::{sync_util, TrySendError};

use core::marker::PhantomPinned;
use futures_util::stream::Stream;
use std::cell::UnsafeCell;
use std::fmt;
use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
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
  pub(crate) consumer_parked: AtomicBool,
  pub(crate) consumer_thread: Mutex<Option<Thread>>,
  // Async waiter field
  consumer_waker: AtomicWaker,
  pub(crate) receiver_dropped: AtomicBool,

  pub(crate) sender_count: AtomicUsize,
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
      consumer_thread: Mutex::new(None),
      consumer_waker: AtomicWaker::new(),
      receiver_dropped: AtomicBool::new(false),
      sender_count: AtomicUsize::new(1),
      current_len: AtomicUsize::new(0),
    }
  }

  /// Wakes the consumer if it is parked, whether synchronously or asynchronously.
  #[inline]
  pub(crate) fn wake_consumer(&self) {
    self.consumer_waker.wake();

    if self
      .consumer_parked
      .compare_exchange(true, false, Ordering::AcqRel, Ordering::Acquire)
      .is_ok()
    {
      if let Some(thread_handle) = self.consumer_thread.lock().unwrap().take() {
        sync_util::unpark_thread(&thread_handle);
      }
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

  pub(crate) fn poll_recv_internal(&self, cx: &mut Context<'_>) -> Poll<Result<T, RecvError>> {
    loop {
      match self.try_recv_internal() {
        Ok(value) => return Poll::Ready(Ok(value)),
        Err(TryRecvError::Disconnected) => return Poll::Ready(Err(RecvError::Disconnected)),
        Err(TryRecvError::Empty) => {
          self.consumer_waker.register(cx.waker());
          match self.try_recv_internal() {
            Ok(value) => return Poll::Ready(Ok(value)),
            Err(TryRecvError::Disconnected) => return Poll::Ready(Err(RecvError::Disconnected)),
            Err(TryRecvError::Empty) => {
              if self.sender_count.load(Ordering::Acquire) == 0 {
                match self.try_recv_internal() {
                  Ok(value) => return Poll::Ready(Ok(value)),
                  _ => return Poll::Ready(Err(RecvError::Disconnected)),
                }
              }
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
    let mut current_ptr = unsafe { (*(*self.tail.get_mut())).next.load(Ordering::Relaxed) };
    while !current_ptr.is_null() {
      let node = unsafe { Box::from_raw(current_ptr) };
      current_ptr = node.next.load(Ordering::Relaxed);
    }
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
  pub(crate) closed: AtomicBool,
}

#[derive(Debug)]
pub struct AsyncSender<T: Send> {
  pub(crate) shared: Arc<MpscShared<T>>,
  pub(crate) closed: AtomicBool,
}

// Common logic for both producer types
pub(crate) fn send_internal<T: Send>(shared: &Arc<MpscShared<T>>, value: T) -> Result<(), T> {
  if shared.receiver_dropped.load(Ordering::Acquire) {
    return Err(value);
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

  shared.current_len.fetch_add(1, Ordering::Relaxed);
  shared.wake_consumer();
  Ok(())
}

impl<T: Send> Sender<T> {
  pub fn send(&self, value: T) -> Result<(), SendError> {
    if self.closed.load(Ordering::Relaxed) {
      return Err(SendError::Closed);
    }
    send_internal(&self.shared, value).map_err(|_| SendError::Closed)
  }

  /// Attempts to send a value on this channel without blocking.
  ///
  /// This method will either send the value immediately or return an error if
  /// the channel is disconnected. As this MPSC channel is unbounded, it will
  /// never be full.
  ///
  /// # Errors
  ///
  /// - `Err(TrySendError::Closed(value))` - The receiver has been dropped, or
  ///   this specific sender handle was closed via `close()`.
  pub fn try_send(&self, value: T) -> Result<(), TrySendError<T>> {
    if self.closed.load(Ordering::Relaxed) {
      return Err(TrySendError::Closed(value));
    }
    send_internal(&self.shared, value).map_err(TrySendError::Closed)
  }

  pub fn is_closed(&self) -> bool {
    self.shared.receiver_dropped.load(Ordering::Acquire)
  }

  pub fn close(&self) -> Result<(), CloseError> {
    if self
      .closed
      .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
      .is_ok()
    {
      self.close_internal();
      Ok(())
    } else {
      Err(CloseError)
    }
  }

  fn close_internal(&self) {
    if self.shared.sender_count.fetch_sub(1, Ordering::AcqRel) == 1 {
      self.shared.wake_consumer();
    }
  }

  pub fn len(&self) -> usize {
    self.shared.current_len.load(Ordering::Relaxed)
  }

  /// Returns `true` if the channel is approximately empty.
  ///
  /// # Warning
  ///
  /// This is a point-in-time approximation. Due to the concurrent nature
  /// of the channel, another sender may have sent a value immediately
  /// after this check, or the receiver may have consumed the last value.
  /// This method should not be used for synchronization and is provided
  /// for convenience and API consistency. For a guaranteed check, use the
  /// `is_empty()` method on the `Receiver`.
  pub fn is_empty(&self) -> bool {
    self.shared.current_len.load(Ordering::Relaxed) == 0
  }
  
  /// Converts this synchronous `Sender` into an asynchronous `AsyncSender`.
  pub fn to_async(self) -> AsyncSender<T> {
    let shared = unsafe { std::ptr::read(&self.shared) };
    std::mem::forget(self);
    
    AsyncSender {
      shared,
      closed: AtomicBool::new(false),
    }
  }
}

impl<T: Send> AsyncSender<T> {
  pub fn send(&self, value: T) -> SendFuture<'_, T> {
    SendFuture {
      producer: self,
      value: Some(value),
      _phantom: PhantomPinned,
    }
  }

  /// Attempts to send a value on this channel without blocking.
  ///
  /// This method will either send the value immediately or return an error if
  /// the channel is disconnected. As this MPSC channel is unbounded, it will
  /// never be full.
  ///
  /// # Errors
  ///
  /// - `Err(TrySendError::Closed(value))` - The receiver has been dropped, or
  ///   this specific sender handle was closed via `close()`.
  pub fn try_send(&self, value: T) -> Result<(), TrySendError<T>> {
    if self.closed.load(Ordering::Relaxed) {
      return Err(TrySendError::Closed(value));
    }
    send_internal(&self.shared, value).map_err(TrySendError::Closed)
  }

  pub fn is_closed(&self) -> bool {
    self.shared.receiver_dropped.load(Ordering::Acquire)
  }

  pub fn close(&self) -> Result<(), CloseError> {
    if self
      .closed
      .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
      .is_ok()
    {
      self.close_internal();
      Ok(())
    } else {
      Err(CloseError)
    }
  }

  fn close_internal(&self) {
    if self.shared.sender_count.fetch_sub(1, Ordering::AcqRel) == 1 {
      self.shared.wake_consumer();
    }
  }

  pub fn len(&self) -> usize {
    self.shared.current_len.load(Ordering::Relaxed)
  }

  /// Returns `true` if the channel is approximately empty.
  ///
  /// # Warning
  ///
  /// This is a point-in-time approximation. Due to the concurrent nature
  /// of the channel, another sender may have sent a value immediately
  /// after this check, or the receiver may have consumed the last value.
  /// This method should not be used for synchronization and is provided
  /// for convenience and API consistency. For a guaranteed check, use the
  /// `is_empty()` method on the `AsyncReceiver`.
  pub fn is_empty(&self) -> bool {
    self.shared.current_len.load(Ordering::Relaxed) == 0
  }
  
  /// Converts this asynchronous `AsyncSender` into a synchronous `Sender`.
  pub fn to_sync(self) -> Sender<T> {
    let shared = unsafe { std::ptr::read(&self.shared) };
    std::mem::forget(self);
    Sender {
      shared,
      closed: AtomicBool::new(false),
    }
  }
}

// --- Cloning and Dropping Senders ---
impl<T: Send> Clone for Sender<T> {
  fn clone(&self) -> Self {
    self.shared.sender_count.fetch_add(1, Ordering::Relaxed);
    Sender {
      shared: Arc::clone(&self.shared),
      closed: AtomicBool::new(false),
    }
  }
}
impl<T: Send> Drop for Sender<T> {
  fn drop(&mut self) {
    if !self.closed.swap(true, Ordering::AcqRel) {
      self.close_internal();
    }
  }
}
impl<T: Send> Clone for AsyncSender<T> {
  fn clone(&self) -> Self {
    self.shared.sender_count.fetch_add(1, Ordering::Relaxed);
    AsyncSender {
      shared: Arc::clone(&self.shared),
      closed: AtomicBool::new(false),
    }
  }
}
impl<T: Send> Drop for AsyncSender<T> {
  fn drop(&mut self) {
    if !self.closed.swap(true, Ordering::AcqRel) {
      self.close_internal();
    }
  }
}

// --- Receiver & AsyncReceiver ---
#[derive(Debug)]
pub struct Receiver<T: Send> {
  pub(crate) shared: Arc<MpscShared<T>>,
  pub(crate) closed: AtomicBool,
}
unsafe impl<T: Send> Send for Receiver<T> {}
#[derive(Debug)]
pub struct AsyncReceiver<T: Send> {
  pub(crate) shared: Arc<MpscShared<T>>,
  pub(crate) closed: AtomicBool,
}
unsafe impl<T: Send> Send for AsyncReceiver<T> {}

// --- Sync Receiver Implementation ---
impl<T: Send> Receiver<T> {
  pub fn try_recv(&self) -> Result<T, TryRecvError> {
    if self.closed.load(Ordering::Relaxed) {
      return Err(TryRecvError::Disconnected);
    }
    self.shared.try_recv_internal()
  }

  pub fn recv(&self) -> Result<T, RecvError> {
    if self.closed.load(Ordering::Relaxed) {
      return Err(RecvError::Disconnected);
    }
    loop {
      match self.try_recv() {
        Ok(value) => return Ok(value),
        Err(TryRecvError::Disconnected) => return Err(RecvError::Disconnected),
        Err(TryRecvError::Empty) => {}
      }

      *self.shared.consumer_thread.lock().unwrap() = Some(thread::current());
      self.shared.consumer_parked.store(true, Ordering::Release);

      match self.try_recv() {
        Ok(value) => {
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
          sync_util::park_thread();
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
    }
  }

  pub fn is_closed(&self) -> bool {
    self.shared.sender_count.load(Ordering::Acquire) == 0 && self.is_empty()
  }

  pub fn close(&self) -> Result<(), CloseError> {
    if self
      .closed
      .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
      .is_ok()
    {
      self.close_internal();
      Ok(())
    } else {
      Err(CloseError)
    }
  }

  fn close_internal(&self) {
    self.shared.receiver_dropped.store(true, Ordering::Release);
    while self.shared.try_recv_internal().is_ok() {
      // Keep draining
    }
  }

  pub fn len(&self) -> usize {
    self.shared.current_len.load(Ordering::Relaxed)
  }

  pub fn is_empty(&self) -> bool {
    if self.shared.current_len.load(Ordering::Relaxed) == 0 {
      let tail_node_ptr = unsafe { *self.shared.tail.get() };
      let next_node_ptr = unsafe { (*tail_node_ptr).next.load(Ordering::Acquire) };
      return next_node_ptr.is_null();
    }
    false
  }
  /// Converts this synchronous `Receiver` into an asynchronous `AsyncReceiver`.
  pub fn to_async(self) -> AsyncReceiver<T> {
    let shared = unsafe { std::ptr::read(&self.shared) };
    std::mem::forget(self);

    AsyncReceiver {
      shared,
      closed: AtomicBool::new(false),
    }
  }
}

// --- Async Receiver Implementation ---
impl<T: Send> AsyncReceiver<T> {
  pub fn try_recv(&mut self) -> Result<T, TryRecvError> {
    if self.closed.load(Ordering::Relaxed) {
      return Err(TryRecvError::Disconnected);
    }
    self.shared.try_recv_internal()
  }
  pub fn recv(&self) -> RecvFuture<'_, T> {
    RecvFuture { consumer: self }
  }

  pub fn is_closed(&self) -> bool {
    self.shared.sender_count.load(Ordering::Acquire) == 0 && self.is_empty()
  }

  pub fn close(&self) -> Result<(), CloseError> {
    if self
      .closed
      .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
      .is_ok()
    {
      self.close_internal();
      Ok(())
    } else {
      Err(CloseError)
    }
  }

  fn close_internal(&self) {
    self.shared.receiver_dropped.store(true, Ordering::Release);
    while self.shared.try_recv_internal().is_ok() {
      // Keep draining
    }
  }

  pub fn len(&self) -> usize {
    self.shared.current_len.load(Ordering::Relaxed)
  }

  pub fn is_empty(&self) -> bool {
    if self.shared.current_len.load(Ordering::Relaxed) == 0 {
      let tail_node_ptr = unsafe { *self.shared.tail.get() };
      let next_node_ptr = unsafe { (*tail_node_ptr).next.load(Ordering::Acquire) };
      return next_node_ptr.is_null();
    }
    false
  }

  /// Converts this asynchronous `AsyncReceiver` into a synchronous `Receiver`.
  pub fn to_sync(self) -> Receiver<T> {
    let shared = unsafe { std::ptr::read(&self.shared) };
    std::mem::forget(self);
    Receiver {
      shared,
      closed: AtomicBool::new(false),
    }
  }
}

// --- Dropping Receivers ---
impl<T: Send> Drop for Receiver<T> {
  fn drop(&mut self) {
    if !self.closed.swap(true, Ordering::AcqRel) {
      self.close_internal();
    }
  }
}
impl<T: Send> Drop for AsyncReceiver<T> {
  fn drop(&mut self) {
    if !self.closed.swap(true, Ordering::AcqRel) {
      self.close_internal();
    }
  }
}

// --- Future Implementations ---
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
    if this.producer.closed.load(Ordering::Relaxed) {
      return Poll::Ready(Err(SendError::Closed));
    }
    let value = this
      .value
      .take()
      .expect("SendFuture polled after completion");
    Poll::Ready(send_internal(&this.producer.shared, value).map_err(|_| SendError::Closed))
  }
}

#[must_use = "futures do nothing unless you .await or poll them"]
pub struct RecvFuture<'a, T: Send> {
  consumer: &'a AsyncReceiver<T>,
}

impl<'a, T: Send> Future for RecvFuture<'a, T> {
  type Output = Result<T, RecvError>;
  fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    if self.consumer.closed.load(Ordering::Relaxed) {
      return Poll::Ready(Err(RecvError::Disconnected));
    }
    self.consumer.shared.poll_recv_internal(cx)
  }
}

impl<T: Send> Stream for AsyncReceiver<T> {
  type Item = T;

  fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
    if self.closed.load(Ordering::Relaxed) {
      return Poll::Ready(None);
    }
    match self.shared.poll_recv_internal(cx) {
      Poll::Ready(Ok(value)) => Poll::Ready(Some(value)),
      Poll::Ready(Err(_)) => Poll::Ready(None),
      Poll::Pending => Poll::Pending,
    }
  }
}
