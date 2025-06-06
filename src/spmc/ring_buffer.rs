// src/spmc/ring_buffer.rs

use crate::async_util::AtomicWaker;
use crate::error::{RecvError, SendError, TryRecvError};
use crate::internal::cache_padded::CachePadded;
use crate::sync_util;

use std::cell::UnsafeCell;
use std::fmt;
use std::future::Future;
use std::marker::PhantomData;
use std::mem::MaybeUninit;
use std::pin::Pin;
use std::sync::atomic::{self, AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
use std::thread::{self, Thread};

// Helper function to create a Waker that unparks a specific thread.
// This is essential for the synchronous `recv` to park correctly.
fn sync_waker(thread: Thread) -> Waker {
  // This vtable defines how to clone, wake, and drop the waker's raw data.
  // The key is that `wake` and `wake_by_ref` call `thread.unpark()`.
  const VTABLE: RawWakerVTable = RawWakerVTable::new(
    |data| unsafe {
      // Clone: Create a new Box<Thread> and return a new RawWaker.
      let thread_ptr = Box::into_raw(Box::new((*(data as *const Thread)).clone()));
      RawWaker::new(thread_ptr as *const (), &VTABLE)
    },
    |data| unsafe {
      // Wake: Consumes the waker, so we must consume and drop the Box<Thread>.
      let thread = Box::from_raw(data as *mut Thread);
      thread.unpark();
    },
    |data| unsafe {
      // Wake_by_ref: Does not consume the waker, so we only unpark.
      (*(data as *const Thread)).unpark();
    },
    |data| unsafe {
      // Drop: The waker is dropped, so we must drop the Box<Thread>.
      drop(Box::from_raw(data as *mut Thread));
    },
  );
  let thread_ptr = Box::into_raw(Box::new(thread));
  unsafe { Waker::from_raw(RawWaker::new(thread_ptr as *const (), &VTABLE)) }
}

// --- Core Structs ---

pub(crate) struct Slot<T> {
  sequence: AtomicUsize,
  value: UnsafeCell<MaybeUninit<T>>,
  // A slot can have multiple consumers (sync or async) waiting for it.
  // We use a Mutex-guarded Vec to store all their wakers.
  wakers: Mutex<Vec<Waker>>,
}

impl<T> Drop for Slot<T> {
  fn drop(&mut self) {
    if *self.sequence.get_mut() % 2 == 1 {
      unsafe { self.value.get_mut().assume_init_drop() };
    }
  }
}

#[derive(Debug)]
struct ConsumerCursors {
  list: Vec<Arc<AtomicUsize>>,
}

pub(crate) struct SpmcShared<T: Send + Clone> {
  buffer: Box<[Slot<T>]>,
  capacity: usize,
  head: CachePadded<UnsafeCell<usize>>,
  tails: Mutex<ConsumerCursors>,
  producer_parked: AtomicBool,
  producer_thread: UnsafeCell<Option<Thread>>,
  producer_waker: AtomicWaker,
}

impl<T: Send + Clone> fmt::Debug for SpmcShared<T> {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("SpmcShared")
      .field("capacity", &self.capacity)
      .field("head", &unsafe { *self.head.get() })
      .field("tails", &self.tails.lock().unwrap())
      .finish_non_exhaustive()
  }
}

unsafe impl<T: Send + Clone> Send for SpmcShared<T> {}
unsafe impl<T: Send + Clone> Sync for SpmcShared<T> {}

impl<T: Send + Clone> SpmcShared<T> {
  fn new(capacity: usize) -> Self {
    let mut buffer = Vec::with_capacity(capacity);
    for i in 0..capacity {
      buffer.push(Slot {
        sequence: AtomicUsize::new(2 * i),
        value: UnsafeCell::new(MaybeUninit::uninit()),
        wakers: Mutex::new(Vec::new()),
      });
    }
    SpmcShared {
      buffer: buffer.into_boxed_slice(),
      capacity,
      head: CachePadded::new(UnsafeCell::new(0)),
      tails: Mutex::new(ConsumerCursors { list: Vec::new() }),
      producer_parked: AtomicBool::new(false),
      producer_thread: UnsafeCell::new(None),
      producer_waker: AtomicWaker::new(),
    }
  }

  fn wake_producer(&self) {
    if self.producer_parked.load(Ordering::Relaxed) {
      atomic::fence(Ordering::Acquire);
      if self
        .producer_parked
        .compare_exchange(true, false, Ordering::AcqRel, Ordering::Relaxed)
        .is_ok()
      {
        if let Some(thread) = unsafe { (*self.producer_thread.get()).take() } {
          thread.unpark();
        }
      }
    }
    self.producer_waker.wake();
  }
}

// --- Handles ---
#[derive(Debug)]
pub struct Producer<T: Send + Clone> {
  pub(crate) shared: Arc<SpmcShared<T>>,
  pub(crate) _phantom: PhantomData<*const ()>,
}
unsafe impl<T: Send + Clone> Send for Producer<T> {}
#[derive(Debug)]
pub struct AsyncProducer<T: Send + Clone> {
  pub(crate) shared: Arc<SpmcShared<T>>,
  pub(crate) _phantom: PhantomData<*const ()>,
}
unsafe impl<T: Send + Clone> Send for AsyncProducer<T> {}
#[derive(Debug)]
pub struct Receiver<T: Send + Clone> {
  pub(crate) shared: Arc<SpmcShared<T>>,
  pub(crate) tail: Arc<AtomicUsize>,
}
#[derive(Debug)]
pub struct AsyncReceiver<T: Send + Clone> {
  pub(crate) shared: Arc<SpmcShared<T>>,
  pub(crate) tail: Arc<AtomicUsize>,
}

// --- Channel Constructor ---
pub(crate) fn new_channel<T: Send + Clone>(capacity: usize) -> (Producer<T>, Receiver<T>) {
  assert!(capacity > 0, "SPMC channel capacity must be > 0");
  let shared = Arc::new(SpmcShared::new(capacity));
  let initial_tail = Arc::new(AtomicUsize::new(0));
  shared.tails.lock().unwrap().list.push(initial_tail.clone());
  (
    Producer {
      shared: Arc::clone(&shared),
      _phantom: PhantomData,
    },
    Receiver {
      shared,
      tail: initial_tail,
    },
  )
}

// --- Receiver Logic ---
fn try_recv_internal<T: Send + Clone>(
  shared: &SpmcShared<T>,
  tail_atomic: &AtomicUsize,
) -> Result<T, TryRecvError> {
  let tail = tail_atomic.load(Ordering::Relaxed);
  let slot_idx = tail % shared.capacity;
  let slot = &shared.buffer[slot_idx];
  let seq = slot.sequence.load(Ordering::Acquire);

  if seq == 2 * tail + 1 {
    let value = unsafe { (*slot.value.get()).assume_init_ref().clone() };
    tail_atomic.store(tail + 1, Ordering::Release);
    shared.wake_producer();
    Ok(value)
  } else {
    Err(TryRecvError::Empty)
  }
}

impl<T: Send + Clone> Receiver<T> {
  pub fn try_recv(&mut self) -> Result<T, TryRecvError> {
    try_recv_internal(&self.shared, &self.tail)
  }

  pub fn recv(&mut self) -> Result<T, RecvError> {
    loop {
      match self.try_recv() {
        Ok(value) => return Ok(value),
        Err(TryRecvError::Empty) => {
          let tail = self.tail.load(Ordering::Relaxed);
          let slot_idx = tail % self.shared.capacity;
          let slot = &self.shared.buffer[slot_idx];

          let waker = sync_waker(thread::current());
          slot.wakers.lock().unwrap().push(waker);

          if let Ok(value) = self.try_recv() {
            return Ok(value);
          }
          thread::park();
        }
        Err(TryRecvError::Disconnected) => unreachable!(),
      }
    }
  }
}
impl<T: Send + Clone> AsyncReceiver<T> {
  pub fn try_recv(&mut self) -> Result<T, TryRecvError> {
    try_recv_internal(&self.shared, &self.tail)
  }
  pub fn recv(&mut self) -> RecvFuture<'_, T> {
    RecvFuture { receiver: self }
  }
}

// --- Common Receiver Traits ---
fn clone_receiver<T: Send + Clone>(
  shared: &Arc<SpmcShared<T>>,
  tail: &Arc<AtomicUsize>,
) -> Arc<AtomicUsize> {
  let new_tail_val = tail.load(Ordering::Acquire);
  let new_tail = Arc::new(AtomicUsize::new(new_tail_val));
  shared.tails.lock().unwrap().list.push(new_tail.clone());
  new_tail
}
impl<T: Send + Clone> Clone for Receiver<T> {
  fn clone(&self) -> Self {
    Self {
      shared: Arc::clone(&self.shared),
      tail: clone_receiver(&self.shared, &self.tail),
    }
  }
}
impl<T: Send + Clone> Clone for AsyncReceiver<T> {
  fn clone(&self) -> Self {
    Self {
      shared: Arc::clone(&self.shared),
      tail: clone_receiver(&self.shared, &self.tail),
    }
  }
}
fn drop_receiver<T: Send + Clone>(shared: &SpmcShared<T>, tail: &Arc<AtomicUsize>) {
  let mut tails_guard = shared.tails.lock().unwrap();
  tails_guard.list.retain(|t| !Arc::ptr_eq(t, tail));
  shared.wake_producer();
}
impl<T: Send + Clone> Drop for Receiver<T> {
  fn drop(&mut self) {
    drop_receiver(&self.shared, &self.tail)
  }
}
impl<T: Send + Clone> Drop for AsyncReceiver<T> {
  fn drop(&mut self) {
    drop_receiver(&self.shared, &self.tail)
  }
}

// --- Producer Logic ---
impl<T: Send + Clone> Producer<T> {
  pub fn send(&mut self, value: T) -> Result<(), SendError> {
    loop {
      let head = unsafe { *self.shared.head.get() };
      let tails_guard = self.shared.tails.lock().unwrap();
      if tails_guard.list.is_empty() {
        return Err(SendError::Closed);
      }
      let min_tail = tails_guard
        .list
        .iter()
        .map(|t| t.load(Ordering::Acquire))
        .min()
        .unwrap();
      drop(tails_guard);

      if head - min_tail < self.shared.capacity {
        break;
      }

      unsafe { *self.shared.producer_thread.get() = Some(thread::current()) };
      self.shared.producer_parked.store(true, Ordering::Release);

      let tails_guard = self.shared.tails.lock().unwrap();
      if tails_guard.list.is_empty() {
        return Err(SendError::Closed);
      }
      let min_tail = tails_guard
        .list
        .iter()
        .map(|t| t.load(Ordering::Acquire))
        .min()
        .unwrap();
      drop(tails_guard);

      if head - min_tail < self.shared.capacity {
        if self
          .shared
          .producer_parked
          .compare_exchange(true, false, Ordering::AcqRel, Ordering::Relaxed)
          .is_ok()
        {
          unsafe {
            *self.shared.producer_thread.get() = None;
          }
        }
        continue;
      }
      sync_util::park_thread();
    }

    unsafe {
      let head = *self.shared.head.get();
      let slot_idx = head % self.shared.capacity;
      let slot = &self.shared.buffer[slot_idx];

      (*slot.value.get()).write(value);
      slot.sequence.store(2 * head + 1, Ordering::Release);

      for waker in slot.wakers.lock().unwrap().drain(..) {
        waker.wake();
      }

      *self.shared.head.get() = head + 1;
    }
    Ok(())
  }
}

impl<T: Send + Clone> AsyncProducer<T> {
  pub fn send(&mut self, value: T) -> SendFuture<'_, T> {
    SendFuture {
      producer: self,
      value: Some(value),
    }
  }
}

// --- Futures ---
#[must_use]
pub struct SendFuture<'a, T: Send + Clone> {
  producer: &'a mut AsyncProducer<T>,
  value: Option<T>,
}

impl<'a, T: Send + Clone + Unpin> Future for SendFuture<'a, T> {
  type Output = Result<(), SendError>;
  fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    let s = self.as_mut().get_mut();
    loop {
      let shared = &s.producer.shared;
      let head = unsafe { *shared.head.get() };

      let tails_guard = shared.tails.lock().unwrap();
      if tails_guard.list.is_empty() {
        s.value = None;
        return Poll::Ready(Err(SendError::Closed));
      }
      let min_tail = tails_guard
        .list
        .iter()
        .map(|t| t.load(Ordering::Acquire))
        .min()
        .unwrap();
      drop(tails_guard);

      if head - min_tail >= shared.capacity {
        shared.producer_waker.register(cx.waker());
        let tails_guard = shared.tails.lock().unwrap();
        if tails_guard.list.is_empty() {
          s.value = None;
          return Poll::Ready(Err(SendError::Closed));
        }
        let new_min_tail = tails_guard
          .list
          .iter()
          .map(|t| t.load(Ordering::Acquire))
          .min()
          .unwrap();
        drop(tails_guard);
        if head - new_min_tail < shared.capacity {
          continue;
        }
        return Poll::Pending;
      }

      let value_to_write = s.value.take().expect("SendFuture polled after completion");
      let slot_idx = head % shared.capacity;
      let slot = &shared.buffer[slot_idx];
      unsafe {
        (*slot.value.get()).write(value_to_write);
        slot.sequence.store(2 * head + 1, Ordering::Release);

        for waker in slot.wakers.lock().unwrap().drain(..) {
          waker.wake();
        }

        *shared.head.get() = head + 1;
      }
      return Poll::Ready(Ok(()));
    }
  }
}

#[must_use]
pub struct RecvFuture<'a, T: Send + Clone> {
  receiver: &'a mut AsyncReceiver<T>,
}

impl<'a, T: Send + Clone> Future for RecvFuture<'a, T> {
  type Output = Result<T, RecvError>;
  fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    let tail = self.receiver.tail.load(Ordering::Relaxed);
    let slot_idx = tail % self.receiver.shared.capacity;
    let slot = &self.receiver.shared.buffer[slot_idx];

    let seq = slot.sequence.load(Ordering::Acquire);
    if seq == 2 * tail + 1 {
      let value = unsafe { (*slot.value.get()).assume_init_ref().clone() };
      self.receiver.tail.store(tail + 1, Ordering::Release);
      self.receiver.shared.wake_producer();
      return Poll::Ready(Ok(value));
    }

    slot.wakers.lock().unwrap().push(cx.waker().clone());

    if slot.sequence.load(Ordering::Acquire) == 2 * tail + 1 {
      let value = unsafe { (*slot.value.get()).assume_init_ref().clone() };
      self.receiver.tail.store(tail + 1, Ordering::Release);
      self.receiver.shared.wake_producer();
      return Poll::Ready(Ok(value));
    }
    Poll::Pending
  }
}
