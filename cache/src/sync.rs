use std::{
  cell::UnsafeCell,
  future::Future,
  ops::{Deref, DerefMut},
  pin::Pin,
  ptr,
  sync::atomic::{AtomicPtr, AtomicUsize, Ordering},
  task::{Context, Poll, Waker},
  thread::{self, Thread},
};

// ================================================================================================
// === Waiter Node and Queue ======================================================================
// ================================================================================================

// The node containing a waiter. It's part of a lock-free intrusive linked list (stack).
struct WaiterNode {
  // Can be a sync thread or an async task waker.
  waiter: Waiter,
  // Pointer to the next node in the stack.
  next: AtomicPtr<WaiterNode>,
}

// An enum abstracting over the two types of waiters we support.
enum Waiter {
  Thread(Thread),
  Task(Waker),
}

impl Waiter {
  fn wake(self) {
    match self {
      Waiter::Thread(t) => t.unpark(),
      Waiter::Task(w) => w.wake(),
    }
  }
}

// A lock-free Treiber stack to manage the queue of waiters.
struct WaitQueue {
  head: AtomicPtr<WaiterNode>,
}

impl WaitQueue {
  fn new() -> Self {
    Self {
      head: AtomicPtr::new(ptr::null_mut()),
    }
  }

  // Pushes a new waiter to the head of the stack.
  fn push(&self, node: *mut WaiterNode) {
    let mut head = self.head.load(Ordering::Relaxed);
    loop {
      // Safety: The raw pointer `node` is valid and represents an allocated box.
      unsafe { (*node).next.store(head, Ordering::Relaxed) };

      // Attempt to CAS the head pointer.
      match self
        .head
        .compare_exchange_weak(head, node, Ordering::Release, Ordering::Relaxed)
      {
        Ok(_) => break,     // Success
        Err(h) => head = h, // Contention, retry with the new head
      }
    }
  }

  // Pops all waiters from the stack and returns them in a Vec.
  // This is simpler than popping one-by-one in a highly contended scenario.
  fn pop_all(&self) -> Vec<Box<WaiterNode>> {
    let mut head = self.head.swap(ptr::null_mut(), Ordering::Acquire);
    if head.is_null() {
      return Vec::new();
    }

    let mut waiters = Vec::new();
    while !head.is_null() {
      // Safety: The raw pointer `head` is valid as it came from the atomic ptr.
      let node = unsafe { Box::from_raw(head) };
      head = node.next.load(Ordering::Relaxed);
      waiters.push(node);
    }
    // The waiters are in LIFO order, so reverse them to get FIFO.
    waiters.reverse();
    waiters
  }
}

// ================================================================================================
// === HybridMutex ================================================================================
// ================================================================================================

const MUTEX_LOCKED: usize = 1;

pub struct HybridMutex<T> {
  state: AtomicUsize,
  waiters: WaitQueue,
  data: UnsafeCell<T>,
}

unsafe impl<T: Send> Send for HybridMutex<T> {}
unsafe impl<T: Send> Sync for HybridMutex<T> {}

impl<T> HybridMutex<T> {
  pub fn new(data: T) -> Self {
    Self {
      state: AtomicUsize::new(0),
      waiters: WaitQueue::new(),
      data: UnsafeCell::new(data),
    }
  }

  #[inline]
  pub fn lock(&self) -> MutexGuard<'_, T> {
    // Fast path: attempt to acquire the lock if it's free.
    if self
      .state
      .compare_exchange(0, MUTEX_LOCKED, Ordering::Acquire, Ordering::Relaxed)
      .is_ok()
    {
      return MutexGuard { lock: self };
    }
    self.lock_slow()
  }

  #[cold]
  fn lock_slow(&self) -> MutexGuard<'_, T> {
    let mut spin_count = 0;
    loop {
      // Spin for a short time before parking.
      if spin_count < 100 {
        if self
          .state
          .compare_exchange(0, MUTEX_LOCKED, Ordering::Acquire, Ordering::Relaxed)
          .is_ok()
        {
          return MutexGuard { lock: self };
        }
        spin_count += 1;
        thread::yield_now();
      } else {
        // Enqueue ourselves and park the thread.
        let node = Box::into_raw(Box::new(WaiterNode {
          waiter: Waiter::Thread(thread::current()),
          next: AtomicPtr::new(ptr::null_mut()),
        }));
        self.waiters.push(node);

        // Re-check state after pushing, before parking.
        if self
          .state
          .compare_exchange(0, MUTEX_LOCKED, Ordering::Acquire, Ordering::Relaxed)
          .is_ok()
        {
          // We got the lock! Our node might still be in the queue, but that's okay.
          // `unlock` will handle it.
          return MutexGuard { lock: self };
        }

        thread::park();
      }
    }
  }

  #[inline]
  pub async fn lock_async(&self) -> MutexGuard<'_, T> {
    if self
      .state
      .compare_exchange(0, MUTEX_LOCKED, Ordering::Acquire, Ordering::Relaxed)
      .is_ok()
    {
      return MutexGuard { lock: self };
    }
    MutexFuture { lock: self }.await
  }

  pub fn try_lock(&self) -> Option<MutexGuard<'_, T>> {
    // Attempt to grab the lock in a single, non-blocking operation.
    // If the current state is 0, set it to LOCKED. If not, fail.
    if self
      .state
      .compare_exchange(0, MUTEX_LOCKED, Ordering::Acquire, Ordering::Relaxed)
      .is_ok()
    {
      Some(MutexGuard { lock: self })
    } else {
      None
    }
  }

  fn unlock(&self) {
    // Unlock the state.
    self.state.store(0, Ordering::Release);

    // Wake up all waiters. They will re-contend for the lock.
    // This is simpler and fairer than trying to wake just one.
    for node in self.waiters.pop_all() {
      node.waiter.wake();
    }
  }
}

// === Mutex Guard ===
pub struct MutexGuard<'a, T> {
  lock: &'a HybridMutex<T>,
}

impl<T> Drop for MutexGuard<'_, T> {
  fn drop(&mut self) {
    self.lock.unlock();
  }
}

impl<T> Deref for MutexGuard<'_, T> {
  type Target = T;
  fn deref(&self) -> &T {
    unsafe { &*self.lock.data.get() }
  }
}

impl<T> DerefMut for MutexGuard<'_, T> {
  fn deref_mut(&mut self) -> &mut T {
    unsafe { &mut *self.lock.data.get() }
  }
}

// === Mutex Future ===
struct MutexFuture<'a, T> {
  lock: &'a HybridMutex<T>,
}

impl<'a, T> Future for MutexFuture<'a, T> {
  type Output = MutexGuard<'a, T>;

  fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    // Fast path check.
    if self
      .lock
      .state
      .compare_exchange(0, MUTEX_LOCKED, Ordering::Acquire, Ordering::Relaxed)
      .is_ok()
    {
      return Poll::Ready(MutexGuard { lock: self.lock });
    }

    // Enqueue the waker.
    let node = Box::into_raw(Box::new(WaiterNode {
      waiter: Waiter::Task(cx.waker().clone()),
      next: AtomicPtr::new(ptr::null_mut()),
    }));
    self.lock.waiters.push(node);

    // Re-check state after pushing.
    if self
      .lock
      .state
      .compare_exchange(0, MUTEX_LOCKED, Ordering::Acquire, Ordering::Relaxed)
      .is_ok()
    {
      return Poll::Ready(MutexGuard { lock: self.lock });
    }

    Poll::Pending
  }
}

// ================================================================================================
// === HybridRwLock ===============================================================================
// ================================================================================================

// State bits for the RwLock
const RW_WRITE_LOCKED: usize = 1;
const RW_READ_LOCKED_SHIFT: usize = 1;

pub struct HybridRwLock<T> {
  state: AtomicUsize,
  waiters: WaitQueue,
  data: UnsafeCell<T>,
}

unsafe impl<T: Send + Sync> Sync for HybridRwLock<T> {}
unsafe impl<T: Send> Send for HybridRwLock<T> {}

impl<T> HybridRwLock<T> {
  pub fn new(data: T) -> Self {
    Self {
      state: AtomicUsize::new(0),
      waiters: WaitQueue::new(),
      data: UnsafeCell::new(data),
    }
  }

  // --- Read Lock ---
  #[inline]
  pub fn read(&self) -> ReadGuard<'_, T> {
    let s = self.state.load(Ordering::Relaxed);
    // Fast path: No writer, just increment reader count.
    if s & RW_WRITE_LOCKED == 0 {
      if self
        .state
        .compare_exchange_weak(
          s,
          s + (1 << RW_READ_LOCKED_SHIFT),
          Ordering::Acquire,
          Ordering::Relaxed,
        )
        .is_ok()
      {
        return ReadGuard { lock: self };
      }
    }
    self.read_slow()
  }

  #[cold]
  fn read_slow(&self) -> ReadGuard<'_, T> {
    let mut spin_count = 0;
    loop {
      let s = self.state.load(Ordering::Relaxed);
      if s & RW_WRITE_LOCKED == 0 {
        if self
          .state
          .compare_exchange_weak(
            s,
            s + (1 << RW_READ_LOCKED_SHIFT),
            Ordering::Acquire,
            Ordering::Relaxed,
          )
          .is_ok()
        {
          return ReadGuard { lock: self };
        }
      }

      if spin_count < 100 {
        spin_count += 1;
        thread::yield_now();
      } else {
        let node = Box::into_raw(Box::new(WaiterNode {
          waiter: Waiter::Thread(thread::current()),
          next: AtomicPtr::new(ptr::null_mut()),
        }));
        self.waiters.push(node);

        // Re-check after enqueueing
        let s = self.state.load(Ordering::Relaxed);
        if s & RW_WRITE_LOCKED == 0 {
          if self
            .state
            .compare_exchange_weak(
              s,
              s + (1 << RW_READ_LOCKED_SHIFT),
              Ordering::Acquire,
              Ordering::Relaxed,
            )
            .is_ok()
          {
            return ReadGuard { lock: self };
          }
        }

        thread::park();
      }
    }
  }

  #[inline]
  pub async fn read_async(&self) -> ReadGuard<'_, T> {
    let s = self.state.load(Ordering::Relaxed);
    if s & RW_WRITE_LOCKED == 0 {
      if self
        .state
        .compare_exchange_weak(
          s,
          s + (1 << RW_READ_LOCKED_SHIFT),
          Ordering::Acquire,
          Ordering::Relaxed,
        )
        .is_ok()
      {
        return ReadGuard { lock: self };
      }
    }
    ReadFuture { lock: self }.await
  }

  // --- Write Lock ---
  #[inline]
  pub fn write(&self) -> WriteGuard<'_, T> {
    if self
      .state
      .compare_exchange(0, RW_WRITE_LOCKED, Ordering::Acquire, Ordering::Relaxed)
      .is_ok()
    {
      return WriteGuard { lock: self };
    }
    self.write_slow()
  }

  #[cold]
  fn write_slow(&self) -> WriteGuard<'_, T> {
    let mut spin_count = 0;
    loop {
      if spin_count < 100 {
        if self
          .state
          .compare_exchange(0, RW_WRITE_LOCKED, Ordering::Acquire, Ordering::Relaxed)
          .is_ok()
        {
          return WriteGuard { lock: self };
        }
        spin_count += 1;
        thread::yield_now();
      } else {
        let node = Box::into_raw(Box::new(WaiterNode {
          waiter: Waiter::Thread(thread::current()),
          next: AtomicPtr::new(ptr::null_mut()),
        }));
        self.waiters.push(node);

        if self
          .state
          .compare_exchange(0, RW_WRITE_LOCKED, Ordering::Acquire, Ordering::Relaxed)
          .is_ok()
        {
          return WriteGuard { lock: self };
        }

        thread::park();
      }
    }
  }

  #[inline]
  pub async fn write_async(&self) -> WriteGuard<'_, T> {
    if self
      .state
      .compare_exchange(0, RW_WRITE_LOCKED, Ordering::Acquire, Ordering::Relaxed)
      .is_ok()
    {
      return WriteGuard { lock: self };
    }
    WriteFuture { lock: self }.await
  }

  pub fn try_read(&self) -> Option<ReadGuard<'_, T>> {
    let s = self.state.load(Ordering::Relaxed);

    // Fail immediately if a writer holds the lock.
    if (s & RW_WRITE_LOCKED) != 0 {
      return None;
    }

    // Attempt to atomically increment the reader count.
    // This will fail if the state changed (e.g., a writer just acquired the lock).
    match self.state.compare_exchange(
      s,
      s + (1 << RW_READ_LOCKED_SHIFT),
      Ordering::Acquire,
      Ordering::Relaxed,
    ) {
      Ok(_) => Some(ReadGuard { lock: self }),
      Err(_) => None, // State changed, indicating contention.
    }
  }

  pub fn try_write(&self) -> Option<WriteGuard<'_, T>> {
    // Attempt to grab the write lock in a single operation.
    // This only succeeds if the state is 0 (no readers and no writers).
    if self
      .state
      .compare_exchange(0, RW_WRITE_LOCKED, Ordering::Acquire, Ordering::Relaxed)
      .is_ok()
    {
      Some(WriteGuard { lock: self })
    } else {
      None
    }
  }

  // --- Unlock Logic ---
  fn unlock_read(&self) {
    let old_state = self
      .state
      .fetch_sub(1 << RW_READ_LOCKED_SHIFT, Ordering::Release);
    // If we were the last reader, check if writers are waiting.
    if old_state >> RW_READ_LOCKED_SHIFT == 1 {
      self.wake_waiters();
    }
  }

  fn unlock_write(&self) {
    self.state.store(0, Ordering::Release);
    self.wake_waiters();
  }

  fn wake_waiters(&self) {
    // The robust solution is to wake all waiters and let them re-contend for the lock.
    // This is simpler and guarantees no waiter is ever "lost".
    let waiters = self.waiters.pop_all();
    for node in waiters {
      node.waiter.wake();
    }
  }
}

// === RwLock Guards ===
pub struct ReadGuard<'a, T> {
  lock: &'a HybridRwLock<T>,
}

impl<T> Drop for ReadGuard<'_, T> {
  fn drop(&mut self) {
    self.lock.unlock_read();
  }
}

impl<T> Deref for ReadGuard<'_, T> {
  type Target = T;
  fn deref(&self) -> &T {
    unsafe { &*self.lock.data.get() }
  }
}

pub struct WriteGuard<'a, T> {
  lock: &'a HybridRwLock<T>,
}

impl<T> Drop for WriteGuard<'_, T> {
  fn drop(&mut self) {
    self.lock.unlock_write();
  }
}

impl<T> Deref for WriteGuard<'_, T> {
  type Target = T;
  fn deref(&self) -> &T {
    unsafe { &*self.lock.data.get() }
  }
}

impl<T> DerefMut for WriteGuard<'_, T> {
  fn deref_mut(&mut self) -> &mut T {
    unsafe { &mut *self.lock.data.get() }
  }
}

// === RwLock Futures ===
struct ReadFuture<'a, T> {
  lock: &'a HybridRwLock<T>,
}

impl<'a, T> Future for ReadFuture<'a, T> {
  type Output = ReadGuard<'a, T>;
  fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    let s = self.lock.state.load(Ordering::Relaxed);
    if s & RW_WRITE_LOCKED == 0 {
      if self
        .lock
        .state
        .compare_exchange_weak(
          s,
          s + (1 << RW_READ_LOCKED_SHIFT),
          Ordering::Acquire,
          Ordering::Relaxed,
        )
        .is_ok()
      {
        return Poll::Ready(ReadGuard { lock: self.lock });
      }
    }

    let node = Box::into_raw(Box::new(WaiterNode {
      waiter: Waiter::Task(cx.waker().clone()),
      next: AtomicPtr::new(ptr::null_mut()),
    }));
    self.lock.waiters.push(node);

    let s = self.lock.state.load(Ordering::Relaxed);
    if s & RW_WRITE_LOCKED == 0 {
      if self
        .lock
        .state
        .compare_exchange_weak(
          s,
          s + (1 << RW_READ_LOCKED_SHIFT),
          Ordering::Acquire,
          Ordering::Relaxed,
        )
        .is_ok()
      {
        return Poll::Ready(ReadGuard { lock: self.lock });
      }
    }

    Poll::Pending
  }
}

struct WriteFuture<'a, T> {
  lock: &'a HybridRwLock<T>,
}

impl<'a, T> Future for WriteFuture<'a, T> {
  type Output = WriteGuard<'a, T>;
  fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    if self
      .lock
      .state
      .compare_exchange(0, RW_WRITE_LOCKED, Ordering::Acquire, Ordering::Relaxed)
      .is_ok()
    {
      return Poll::Ready(WriteGuard { lock: self.lock });
    }

    let node = Box::into_raw(Box::new(WaiterNode {
      waiter: Waiter::Task(cx.waker().clone()),
      next: AtomicPtr::new(ptr::null_mut()),
    }));
    self.lock.waiters.push(node);

    if self
      .lock
      .state
      .compare_exchange(0, RW_WRITE_LOCKED, Ordering::Acquire, Ordering::Relaxed)
      .is_ok()
    {
      return Poll::Ready(WriteGuard { lock: self.lock });
    }

    Poll::Pending
  }
}
