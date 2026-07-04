use crate::error::RecvError;
use crate::internal::cache_padded::CachePadded;
use core::cell::UnsafeCell;
use core::mem::MaybeUninit;
use core::task::{Context, Poll, Waker};
use crate::internal::sync::{fence, AtomicBool, AtomicUsize, Mutex, Ordering, Thread};

// --- Lock-free ring -------------------------------------------------------

/// Producer-owned state, isolated on its own cache line.
struct ProducerSide {
  /// Next position to write. Written only by the producer.
  tail: AtomicUsize,
  /// Producer-private snapshot of `head`. A stale value only under-reports
  /// free space (`head` never moves backwards), so it is always safe to trust
  /// — but `push` must refresh it before it may report the ring full.
  cached_head: UnsafeCell<usize>,
}

/// Consumer-owned state, isolated on its own cache line.
struct ConsumerSide {
  /// Next position to read. Written only by the consumer.
  head: AtomicUsize,
  /// Consumer-private snapshot of `tail`; same staleness rules as
  /// `cached_head`, mirrored.
  cached_tail: UnsafeCell<usize>,
}

/// Owned-index SPSC ring (Lamport queue with cached counterpart indices).
///
/// Each side owns its index outright — the producer is the only writer of
/// `tail`, the consumer the only writer of `head` — so neither side ever needs
/// a CAS. Each side works against a private cached copy of the other side's
/// index and only pays a cross-core Acquire load when the cache can no longer
/// prove progress is possible. Items live in one contiguous unpadded buffer;
/// only the two index groups are cache-padded.
///
/// # Soundness
/// Producer-side methods (`push`, `producer_free_space`) require exclusive
/// producer access and consumer-side methods (`pop`) exclusive consumer
/// access: they mutate the private caches through `UnsafeCell`. Callers uphold
/// this — the sync handles are `!Sync` and not `Clone`, the async handles take
/// `&mut self` for every ring-touching operation, and the sync<->async
/// conversions consume the handle.
pub(crate) struct Ring<T> {
  buf: Box<[UnsafeCell<MaybeUninit<T>>]>,
  mask: usize,
  /// Channel capacity: the requested `n`, `<= buf.len()`. The physical buffer
  /// (`mask + 1`) is rounded up to a power of two for index masking and is a
  /// separate quantity; fullness and `capacity()` both use this logical value.
  cap: usize,
  p: CachePadded<ProducerSide>,
  c: CachePadded<ConsumerSide>,
}

impl<T> Ring<T> {
  fn new(capacity: usize) -> Self {
    assert!(capacity > 0, "Spsc capacity must be > 0");
    // Physical buffer: a power of two (>= 2) for fast index masking.
    let phys = capacity.next_power_of_two().max(2);
    let mut v = Vec::with_capacity(phys);
    for _ in 0..phys {
      v.push(UnsafeCell::new(MaybeUninit::uninit()));
    }
    Ring {
      buf: v.into_boxed_slice(),
      mask: phys - 1,
      cap: capacity,
      p: CachePadded::new(ProducerSide {
        tail: AtomicUsize::new(0),
        cached_head: UnsafeCell::new(0),
      }),
      c: CachePadded::new(ConsumerSide {
        head: AtomicUsize::new(0),
        cached_tail: UnsafeCell::new(0),
      }),
    }
  }

  #[inline]
  pub(crate) fn capacity(&self) -> usize {
    self.cap
  }

  #[inline]
  pub(crate) fn len(&self) -> usize {
    // Observer snapshot. `head` is loaded first so the later `tail` load can
    // only be at or ahead of it (no negative wrap); clamped because `tail` may
    // keep advancing between the two loads.
    let head = self.c.head.load(Ordering::Acquire);
    let tail = self.p.tail.load(Ordering::Acquire);
    tail.wrapping_sub(head).min(self.cap)
  }

  #[inline]
  pub(crate) fn is_empty(&self) -> bool {
    self.len() == 0
  }

  #[inline]
  pub(crate) fn is_full(&self) -> bool {
    self.len() >= self.cap
  }

  /// Free slots as proven to the producer, refreshing `cached_head`. The result
  /// is exact at the time of the call and can only grow until the producer
  /// pushes, so the caller may perform that many pushes infallibly.
  ///
  /// Producer-side: see the soundness notes on [`Ring`].
  #[inline]
  pub(crate) fn producer_free_space(&self) -> usize {
    let tail = self.p.tail.load(Ordering::Relaxed);
    // SAFETY: producer-exclusive method; no other reference to `cached_head`
    // can exist (soundness notes on `Ring`).
    let cached_head = unsafe { &mut *self.p.cached_head.get() };
    *cached_head = self.c.head.load(Ordering::Acquire);
    self.cap - tail.wrapping_sub(*cached_head)
  }

  /// Enqueue. Returns `Err(item)` only after a cache refresh proves the ring
  /// is genuinely full — the waiter protocol in `SpscShared` relies on this
  /// refresh-before-reporting-full behavior for its post-registration re-check.
  ///
  /// Producer-side: see the soundness notes on [`Ring`].
  pub(crate) fn push(&self, item: T) -> Result<(), T> {
    let tail = self.p.tail.load(Ordering::Relaxed);
    // SAFETY: producer-exclusive method; no other reference to `cached_head`
    // can exist (soundness notes on `Ring`).
    let cached_head = unsafe { &mut *self.p.cached_head.get() };
    if tail.wrapping_sub(*cached_head) >= self.cap {
      *cached_head = self.c.head.load(Ordering::Acquire);
      if tail.wrapping_sub(*cached_head) >= self.cap {
        return Err(item);
      }
    }
    // SAFETY: the slot is free (fullness check above) and unpublished (the
    // consumer never reads at or past `tail`), and only this producer writes.
    unsafe { (*self.buf[tail & self.mask].get()).write(item) };
    self.p.tail.store(tail.wrapping_add(1), Ordering::Release);
    Ok(())
  }

  /// Dequeue. Returns `None` only after a cache refresh proves the ring is
  /// genuinely empty (same waiter-protocol requirement as `push`).
  ///
  /// Consumer-side: see the soundness notes on [`Ring`].
  pub(crate) fn pop(&self) -> Option<T> {
    let head = self.c.head.load(Ordering::Relaxed);
    // SAFETY: consumer-exclusive method; no other reference to `cached_tail`
    // can exist (soundness notes on `Ring`).
    let cached_tail = unsafe { &mut *self.c.cached_tail.get() };
    if head == *cached_tail {
      *cached_tail = self.p.tail.load(Ordering::Acquire);
      if head == *cached_tail {
        return None;
      }
    }
    // SAFETY: `head < tail`, so this slot was published by the producer's
    // Release store of `tail` (paired with the Acquire refresh above) and has
    // not been consumed yet.
    let item = unsafe { (*self.buf[head & self.mask].get()).assume_init_read() };
    self.c.head.store(head.wrapping_add(1), Ordering::Release);
    Some(item)
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
    let to_send = limit.min(self.ring.producer_free_space());
    let mut sent = 0;
    for _ in 0..to_send {
      let Some(item) = iter.next() else { break };
      // Infallible: `producer_free_space` proved these slots free, and only the
      // consumer can change that — in the direction of more space.
      if self.ring.push(item).is_err() {
        unreachable!("spsc write_batch: push failed inside reserved free space");
      }
      sent += 1;
    }
    if sent > 0 {
      self.notify_receivers();
    }
    sent
  }

  pub(crate) fn read_batch(&self, out: &mut Vec<T>, max: usize) -> usize {
    if max == 0 {
      return 0;
    }
    let _ = out.try_reserve_exact(max);
    let mut got = 0;
    while got < max {
      match self.ring.pop() {
        Some(item) => {
          out.push(item);
          got += 1;
        }
        None => break,
      }
    }
    if got > 0 {
      self.notify_senders();
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
}

impl<T> std::fmt::Debug for SpscShared<T> {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_struct("SpscShared")
      .field("capacity", &self.capacity())
      .field("len", &self.len())
      .finish_non_exhaustive()
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::sync::Arc;

  #[test]
  fn ring_non_power_of_two_capacity_wraps() {
    let ring = Ring::<u32>::new(3);
    assert_eq!(ring.capacity(), 3);
    // Several full fill/drain cycles across the phys=4 buffer with cap=3.
    let mut next = 0u32;
    for _ in 0..10 {
      for _ in 0..3 {
        ring.push(next).unwrap();
        next += 1;
      }
      assert!(ring.is_full());
      assert_eq!(ring.push(999), Err(999));
      for expected in next - 3..next {
        assert_eq!(ring.pop(), Some(expected));
      }
      assert!(ring.is_empty());
      assert_eq!(ring.pop(), None);
    }
  }

  #[test]
  fn ring_stale_caches_refresh() {
    let ring = Ring::<u32>::new(2);
    ring.push(1).unwrap();
    ring.push(2).unwrap();
    // The producer's cached_head still says full after this pop; push must
    // refresh and succeed rather than report full.
    assert_eq!(ring.pop(), Some(1));
    ring.push(3).unwrap();
    // The consumer's cached_tail is behind the latest push; pop must refresh.
    assert_eq!(ring.pop(), Some(2));
    assert_eq!(ring.pop(), Some(3));
    assert_eq!(ring.pop(), None);
  }

  #[test]
  fn ring_drop_releases_unconsumed_items() {
    struct Droppable(Arc<AtomicUsize>);
    impl Drop for Droppable {
      fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::Relaxed);
      }
    }

    let drops = Arc::new(AtomicUsize::new(0));
    {
      let ring = Ring::new(4);
      for _ in 0..3 {
        assert!(ring.push(Droppable(drops.clone())).is_ok());
      }
      // Consume one so head has advanced before the drain.
      drop(ring.pop());
      assert_eq!(drops.load(Ordering::Relaxed), 1);
    }
    assert_eq!(drops.load(Ordering::Relaxed), 3);
  }

  #[test]
  fn producer_free_space_is_exact_after_refresh() {
    let shared = SpscShared::<u32>::new_internal(4);
    assert_eq!(shared.ring.producer_free_space(), 4);
    shared.ring.push(1).unwrap();
    shared.ring.push(2).unwrap();
    assert_eq!(shared.ring.producer_free_space(), 2);
    assert_eq!(shared.ring.pop(), Some(1));
    assert_eq!(shared.ring.producer_free_space(), 3);
  }
}
