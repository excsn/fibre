use crate::error::RecvError;
use crate::internal::cache_padded::CachePadded;
use core::cell::UnsafeCell;
use core::fmt;
use core::mem::MaybeUninit;
use core::task::{Context, Poll, Waker};
use std::sync::atomic::{fence, AtomicBool, AtomicUsize, Ordering};
use std::thread::{self, Thread};

use parking_lot::Mutex;

// --- Ring slot ------------------------------------------------------------

struct Slot<T> {
  /// Vyukov sequence stamp. Initialized to the slot index. After a producer
  /// publishes position `p` it holds `p + 1`; after a consumer frees it it holds
  /// `p + capacity`.
  seq: AtomicUsize,
  val: UnsafeCell<MaybeUninit<T>>,
}

// --- Lock-free ring -------------------------------------------------------

pub(crate) struct Ring<T> {
  buf: Box<[CachePadded<Slot<T>>]>,
  mask: usize,
  /// Channel capacity: the requested `n`, `<= buf.len()`. The physical buffer
  /// (`mask + 1`) is rounded up to a power of two for index masking and is a
  /// separate quantity; fullness and `capacity()` both use this logical value.
  cap: usize,
  enqueue_pos: CachePadded<AtomicUsize>,
  dequeue_pos: CachePadded<AtomicUsize>,
}

impl<T> Ring<T> {
  fn new(capacity: usize) -> Self {
    assert!(capacity > 0, "Spsc capacity must be > 0");
    // Physical buffer: a power of two (>= 2) for fast index masking.
    let phys = capacity.next_power_of_two().max(2);
    let mut v = Vec::with_capacity(phys);
    for i in 0..phys {
      v.push(CachePadded::new(Slot {
        seq: AtomicUsize::new(i),
        val: UnsafeCell::new(MaybeUninit::uninit()),
      }));
    }
    Ring {
      buf: v.into_boxed_slice(),
      mask: phys - 1,
      cap: capacity,
      enqueue_pos: CachePadded::new(AtomicUsize::new(0)),
      dequeue_pos: CachePadded::new(AtomicUsize::new(0)),
    }
  }

  #[inline]
  pub(crate) fn capacity(&self) -> usize {
    self.cap
  }

  #[inline]
  pub(crate) fn len(&self) -> usize {
    loop {
      let head = self.dequeue_pos.load(Ordering::Acquire);
      let tail = self.enqueue_pos.load(Ordering::Acquire);
      if head == self.dequeue_pos.load(Ordering::Acquire) {
        return tail.wrapping_sub(head);
      }
    }
  }

  #[inline]
  pub(crate) fn is_empty(&self) -> bool {
    self.len() == 0
  }

  #[inline]
  pub(crate) fn is_full(&self) -> bool {
    self.len() >= self.cap
  }

  /// Lock-free enqueue. Returns `Err(item)` if the ring is full.
  pub(crate) fn push(&self, item: T) -> Result<(), T> {
    let mut pos = self.enqueue_pos.load(Ordering::Relaxed);
    loop {
      let head = self.dequeue_pos.load(Ordering::Acquire);
      if pos.wrapping_sub(head) >= self.cap {
        return Err(item);
      }
      let slot = &self.buf[pos & self.mask];
      let seq = slot.seq.load(Ordering::Acquire);
      let diff = seq as isize - pos as isize;
      if diff == 0 {
        match self.enqueue_pos.compare_exchange_weak(
          pos,
          pos.wrapping_add(1),
          Ordering::Relaxed,
          Ordering::Relaxed,
        ) {
          Ok(_) => {
            unsafe { (*slot.val.get()).write(item) };
            slot.seq.store(pos.wrapping_add(1), Ordering::Release);
            return Ok(());
          }
          Err(actual) => pos = actual,
        }
      } else if diff < 0 {
        return Err(item);
      } else {
        pos = self.enqueue_pos.load(Ordering::Relaxed);
      }
    }
  }

  /// Lock-free dequeue. Returns `None` if the ring is empty.
  pub(crate) fn pop(&self) -> Option<T> {
    let mut pos = self.dequeue_pos.load(Ordering::Relaxed);
    loop {
      let slot = &self.buf[pos & self.mask];
      let seq = slot.seq.load(Ordering::Acquire);
      let diff = seq as isize - (pos.wrapping_add(1)) as isize;
      if diff == 0 {
        match self.dequeue_pos.compare_exchange_weak(
          pos,
          pos.wrapping_add(1),
          Ordering::Relaxed,
          Ordering::Relaxed,
        ) {
          Ok(_) => {
            let item = unsafe { (*slot.val.get()).assume_init_read() };
            slot.seq.store(
              pos.wrapping_add(self.mask).wrapping_add(1),
              Ordering::Release,
            );
            return Some(item);
          }
          Err(actual) => pos = actual,
        }
      } else if diff < 0 {
        return None;
      } else {
        pos = self.dequeue_pos.load(Ordering::Relaxed);
      }
    }
  }
}

impl<T> Drop for Ring<T> {
  fn drop(&mut self) {
    while self.pop().is_some() {}
  }
}

// --- Waiters --------------------------------------------------------------

#[derive(Clone)]
pub(crate) enum WakeRef {
  Thread(Thread),
  Waker(Waker),
}

impl WakeRef {
  #[inline]
  pub(crate) fn wake(self) {
    match self {
      WakeRef::Thread(t) => t.unpark(),
      WakeRef::Waker(w) => w.wake(),
    }
  }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Role {
  Send,
  Recv,
}

// --- Shared State Core ----------------------------------------------------

pub struct SpscShared<T> {
  pub(crate) ring: Ring<T>,
  pub(crate) sender_count: AtomicUsize,
  pub(crate) receiver_count: AtomicUsize,
  pub(crate) producer_dropped: AtomicBool,
  pub(crate) consumer_dropped: AtomicBool,

  // SPSC only has one sender and consumer; we only need a single-slot waiter structure.
  producer_waiter: Mutex<Option<(WakeRef, *const AtomicBool)>>,
  consumer_waiter: Mutex<Option<(WakeRef, *const AtomicBool)>>,

  // Cheap gates for fast-path check bypass
  send_waiters: CachePadded<AtomicUsize>,
  recv_waiters: CachePadded<AtomicUsize>,
}

unsafe impl<T: Send> Send for SpscShared<T> {}
unsafe impl<T: Send> Sync for SpscShared<T> {}

impl<T> SpscShared<T> {
  pub(crate) fn new_internal(capacity: usize) -> Self {
    SpscShared {
      ring: Ring::new(capacity),
      sender_count: AtomicUsize::new(1),
      receiver_count: AtomicUsize::new(1),
      producer_dropped: AtomicBool::new(false),
      consumer_dropped: AtomicBool::new(false),
      producer_waiter: Mutex::new(None),
      consumer_waiter: Mutex::new(None),
      send_waiters: CachePadded::new(AtomicUsize::new(0)),
      recv_waiters: CachePadded::new(AtomicUsize::new(0)),
    }
  }

  #[inline]
  pub(crate) fn senders_alive(&self) -> bool {
    self.sender_count.load(Ordering::Acquire) != 0
  }

  #[inline]
  pub(crate) fn receivers_alive(&self) -> bool {
    self.receiver_count.load(Ordering::Acquire) != 0
  }

  #[inline]
  pub(crate) fn capacity(&self) -> usize {
    self.ring.capacity()
  }

  #[inline]
  pub(crate) fn len(&self) -> usize {
    self.ring.len()
  }

  #[inline]
  pub(crate) fn is_empty(&self) -> bool {
    self.ring.is_empty()
  }

  #[inline]
  pub(crate) fn is_full(&self) -> bool {
    self.ring.is_full()
  }

  #[inline]
  pub(crate) fn available_space(&self) -> usize {
    self.capacity() - self.len()
  }

  // --- Registration -------------------------------------------------------

  pub(crate) fn register(&self, role: Role, wake: WakeRef, notified: *const AtomicBool) {
    let g = match role {
      Role::Send => {
        let mut guard = self.producer_waiter.lock();
        *guard = Some((wake, notified));
        self.send_waiters.store(1, Ordering::Relaxed);
        guard
      }
      Role::Recv => {
        let mut guard = self.consumer_waiter.lock();
        *guard = Some((wake, notified));
        self.recv_waiters.store(1, Ordering::Relaxed);
        guard
      }
    };
    drop(g);
  }

  pub(crate) fn unregister(&self, role: Role) {
    let g = match role {
      Role::Send => {
        let mut guard = self.producer_waiter.lock();
        *guard = None;
        self.send_waiters.store(0, Ordering::Relaxed);
        guard
      }
      Role::Recv => {
        let mut guard = self.consumer_waiter.lock();
        *guard = None;
        self.recv_waiters.store(0, Ordering::Relaxed);
        guard
      }
    };
    drop(g);
  }

  pub(crate) fn unregister_recv(&self) {
    self.unregister(Role::Recv);
  }

  fn wake_one(&self, role: Role) {
    let wake = match role {
      Role::Send => {
        let mut guard = self.producer_waiter.lock();
        if let Some((wake, notified)) = guard.take() {
          self.send_waiters.store(0, Ordering::Relaxed);
          if !notified.is_null() {
            unsafe { (*notified).store(true, Ordering::Release) };
          }
          Some(wake)
        } else {
          None
        }
      }
      Role::Recv => {
        let mut guard = self.consumer_waiter.lock();
        if let Some((wake, notified)) = guard.take() {
          self.recv_waiters.store(0, Ordering::Relaxed);
          if !notified.is_null() {
            unsafe { (*notified).store(true, Ordering::Release) };
          }
          Some(wake)
        } else {
          None
        }
      }
    };
    if let Some(w) = wake {
      w.wake();
    }
  }

  #[inline]
  pub(crate) fn notify_receivers(&self) {
    fence(Ordering::SeqCst);
    if self.recv_waiters.load(Ordering::Relaxed) != 0 {
      self.wake_one(Role::Recv);
    }
  }

  #[inline]
  pub(crate) fn notify_senders(&self) {
    fence(Ordering::SeqCst);
    if self.send_waiters.load(Ordering::Relaxed) != 0 {
      self.wake_one(Role::Send);
    }
  }

  #[inline]
  pub(crate) fn pre_park_fence(&self) {
    fence(Ordering::SeqCst);
  }

  // --- Core APIs -----------------------------------------------------------

  pub(crate) fn write_batch<I: Iterator<Item = T>>(&self, iter: &mut I, limit: usize) -> usize {
    if limit == 0 {
      return 0;
    }
    let mut sent = 0;
    while sent < limit {
      let Some(item) = iter.next() else { break };
      match self.ring.push(item) {
        Ok(()) => {
          sent += 1;
          self.notify_receivers();
        }
        Err(_) => break,
      }
    }
    sent
  }

  pub(crate) fn read_batch(&self, out: &mut Vec<T>, max: usize) -> usize {
    if max == 0 {
      return 0;
    }
    let mut got = 0;
    while got < max {
      match self.ring.pop() {
        Some(item) => {
          out.push(item);
          got += 1;
          self.notify_senders();
        }
        None => break,
      }
    }
    got
  }

  pub(crate) fn poll_recv_internal(
    &self,
    cx: &mut Context<'_>,
    is_registered: &mut bool,
  ) -> Poll<Result<T, RecvError>> {
    loop {
      if let Some(item) = self.ring.pop() {
        if *is_registered {
          self.unregister(Role::Recv);
          *is_registered = false;
        }
        self.notify_senders();
        return Poll::Ready(Ok(item));
      }
      if !self.senders_alive() {
        if let Some(item) = self.ring.pop() {
          if *is_registered {
            self.unregister(Role::Recv);
            *is_registered = false;
          }
          self.notify_senders();
          return Poll::Ready(Ok(item));
        }
        if *is_registered {
          self.unregister(Role::Recv);
          *is_registered = false;
        }
        return Poll::Ready(Err(RecvError::Disconnected));
      }

      self.register(
        Role::Recv,
        WakeRef::Waker(cx.waker().clone()),
        std::ptr::null(),
      );
      *is_registered = true;
      self.pre_park_fence();

      if let Some(item) = self.ring.pop() {
        self.unregister(Role::Recv);
        *is_registered = false;
        self.notify_senders();
        return Poll::Ready(Ok(item));
      }
      if !self.senders_alive() {
        self.unregister(Role::Recv);
        *is_registered = false;
        if let Some(item) = self.ring.pop() {
          self.notify_senders();
          return Poll::Ready(Ok(item));
        }
        return Poll::Ready(Err(RecvError::Disconnected));
      }
      return Poll::Pending;
    }
  }

  // --- Close ---------------------------------------------------------------

  pub(crate) fn drop_sender(&self) {
    if self.sender_count.fetch_sub(1, Ordering::AcqRel) == 1 {
      self.wake_one(Role::Recv);
    }
  }

  pub(crate) fn drop_receiver(&self) {
    if self.receiver_count.fetch_sub(1, Ordering::AcqRel) == 1 {
      self.wake_one(Role::Send);
    }
  }

  pub(crate) fn add_sender(&self) {
    self.sender_count.fetch_add(1, Ordering::AcqRel);
  }

  pub(crate) fn add_receiver(&self) {
    self.receiver_count.fetch_add(1, Ordering::AcqRel);
  }
}

impl<T> std::fmt::Debug for SpscShared<T> {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_struct("SpscShared")
      .field("capacity", &self.capacity())
      .field("len", &self.len())
      .finish_non_exhaustive()
  }
}
