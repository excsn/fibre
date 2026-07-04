//! Producer handles for the bounded MPSC: `Sender` (sync) and `AsyncSender`,
//! plus their send futures. All send paths funnel through `super::shared`'s
//! pre-allocated node pool and the common `publish_run` tail.

use std::fmt;
use std::marker::PhantomPinned;
use std::pin::Pin;
use std::task::{Context, Poll};

use crate::internal::sync::{fence, thread, Arc, AtomicBool, Ordering};

use crate::error::{
  BatchSendErrorReason, CloseError, SendBatchError, SendError, TrySendBatchError, TrySendError,
};
use crate::sync_util;

#[cfg(miri)]
use super::miri;
use super::shared::*;

pub struct Sender<T: Send> {
  pub(crate) shared: Arc<BoundedQueue<T>>,
  pub(crate) closed: AtomicBool,
}

pub struct AsyncSender<T: Send> {
  pub(crate) shared: Arc<BoundedQueue<T>>,
  pub(crate) closed: AtomicBool,
}

impl<T: Send> fmt::Debug for Sender<T> {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("Sender")
      .field("capacity", &self.capacity())
      .field("len", &self.len())
      .field("closed", &self.closed.load(Ordering::Relaxed))
      .finish()
  }
}

impl<T: Send> fmt::Debug for AsyncSender<T> {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("AsyncSender")
      .field("capacity", &self.capacity())
      .field("len", &self.len())
      .field("closed", &self.closed.load(Ordering::Relaxed))
      .finish()
  }
}

unsafe impl<T: Send> Send for Sender<T> {}
unsafe impl<T: Send> Send for AsyncSender<T> {}
unsafe impl<T: Send> Sync for AsyncSender<T> {}

impl<T: Send> Sender<T> {
  #[inline]
  unsafe fn enqueue_node(&self, node: *mut Node<T>, item: T) {
    unsafe { publish_run(&self.shared, node, 1, &mut std::iter::once(item)) }
  }

  /// Blocks until a node is available. Never holds the shared bucket's lock
  /// while parked: each iteration locks just long enough to check, then drops
  /// it before potentially parking, so other senders sharing this bucket
  /// aren't frozen out for the duration of our wait.
  #[inline]
  fn allocate_node(&self) -> Result<*mut Node<T>, SendError> {
    // Cap-1 ping-pong: spin-probe BEFORE registering — the consumer's
    // recycle usually lands within the window, and skipping registration
    // skips the whole waiter round trip (register + fence here,
    // waiter-lock + flag + unpark on the consumer). This, not the park
    // itself, is where the pre-f35e867 fast path got its throughput.
    //
    // The probe pops the chunk stack DIRECTLY, bypassing the shared pool
    // mutex: at cap-1 every chunk is exactly one node (flush threshold is
    // min(cap, CACHE_FLUSH_CHUNK) = 1, initial chunk = capacity = 1), so
    // a popped chunk head IS the acquired node. N spinning producers race
    // a lock-free CAS instead of convoying on the mutex; the empty probe
    // is a single load (`pop` early-returns on the sentinel). Runs before
    // taking `park_gate` so spinners never serialize behind a parked peer.
    if self.shared.cap == 1 {
      for _ in 0..SYNC_SPIN_LIMIT {
        thread::yield_now();
        if let Some(idx) = self.shared.chunk_stack.pop(self.shared.buf_start) {
          let node = self.shared.idx_to_ptr(idx);
          debug_assert_eq!(unsafe { (*node).chunk_len }, 1);
          // The probe acquires the node directly, bypassing fill_pool — so it
          // must carry the same `cfg(miri)` ownership mark fill_pool applies
          // when it grabs a node (FREE -> PRODUCER + owner canary). Without it
          // publish_run's `shadow_publish` PRODUCER assert false-panics on this
          // node (it was never marked grabbed), which masquerades as a deadlock.
          // At cap-1 the chunk is exactly this one node (chunk_len == 1).
          #[cfg(miri)]
          miri::mark_producer_grab(&self.shared, node);
          return Ok(node);
        }
      }
    }

    // Caps 2-4: same probe economics as cap-1 (recycle chunks are tiny, the
    // consumer's flush lands within the window), but chunks hold several
    // nodes so acquisition goes through the pool. `try_lock` so contending
    // spinners skip and re-probe instead of convoying on the mutex the
    // winning senders' fast path needs.
    if self.shared.cap > 1 && self.shared.cap <= 4 {
      for _ in 0..SYNC_SPIN_LIMIT {
        if let Some(mut pool) = self.shared.cache.try_lock() {
          if let Some(node) = pool.pop_node(&self.shared) {
            return Ok(node);
          }
        }
        thread::yield_now();
      }
    }

    // Blocking path. The gate serializes parked producers at cap-1 ONLY:
    // there each release frees exactly one node, so one registered waiter is
    // ideal and the gate kills the consumer's unpark-per-item storm. At
    // caps >= 2 a release frees a whole chunk and `notify_sync_senders`
    // batch-wakes that many producers in parallel — flamegraphs (Cap-4
    // Prod-14, 2026-07-02) showed a universal gate degrades those parallel
    // wakes into a serialized pthread_cond_signal chain between producers
    // (~600 K/s, 72% of producer on-CPU time in gate lock/unlock slow paths).
    let _gate = (self.shared.cap == 1).then(|| self.shared.park_gate.lock());

    let mut is_registered = false;
    let mut my_id = None;
    let notified = AtomicBool::new(false);
    let notified_ptr = &notified as *const AtomicBool;

    loop {
      if self.closed.load(Ordering::Relaxed) || !self.shared.receivers_alive() {
        if let Some(id) = my_id {
          self.shared.finish_sync_send(id, &notified);
        }
        return Err(SendError::Closed);
      }

      {
        let mut pool = self.shared.cache.lock();
        if let Some(node) = pool.pop_node(&self.shared) {
          if let Some(id) = my_id {
            self.shared.finish_sync_send(id, &notified);
          }
          return Ok(node);
        }
      }

      if is_registered {
        spin_before_park_cap1(self.shared.cap, &notified);
        if !notified.load(Ordering::Relaxed) {
          sync_util::park_thread();
        }
        if notified.swap(false, Ordering::Acquire) {
          is_registered = false;
          my_id = None;
        }
        continue;
      }

      let id = self
        .shared
        .register_sync_send(None, thread::current(), notified_ptr);
      is_registered = true;
      my_id = Some(id);
      fence(Ordering::SeqCst);
    }
  }

  pub fn send(&self, item: T) -> Result<(), SendError> {
    if self.closed.load(Ordering::Relaxed) || !self.shared.receivers_alive() {
      return Err(SendError::Closed);
    }

    // 1. Try to get a node from local cache or chunk stack. The lock is
    // held only long enough to pop the pointer; the Vyukov swap and the
    // receiver wake happen after it's released.
    let node_ptr = {
      let mut pool = self.shared.cache.lock();
      pool.pop_node(&self.shared)
    };
    if let Some(node) = node_ptr {
      unsafe { self.enqueue_node(node, item) };
      return Ok(());
    }

    // 2. Blocking Path: park immediately rather than spinning.
    let node_ptr = self.allocate_node()?;
    unsafe { self.enqueue_node(node_ptr, item) };
    Ok(())
  }

  pub fn try_send(&self, item: T) -> Result<(), TrySendError<T>> {
    if self.closed.load(Ordering::Relaxed) || !self.shared.receivers_alive() {
      return Err(TrySendError::Closed(item));
    }
    let node_ptr = {
      let mut pool = self.shared.cache.lock();
      pool.pop_node(&self.shared)
    };
    let curr = match node_ptr {
      Some(n) => n,
      None => return Err(TrySendError::Full(item)),
    };

    unsafe { self.enqueue_node(curr, item) };
    Ok(())
  }

  pub fn send_batch(&self, items: Vec<T>) -> Result<usize, SendBatchError<T>> {
    let total = items.len();
    if total == 0 {
      return Ok(0);
    }
    if self.closed.load(Ordering::Relaxed) || !self.shared.receivers_alive() {
      return Err(SendBatchError {
        sent: 0,
        unsent: items,
      });
    }

    let mut iter = items.into_iter();
    let mut sent = 0;

    while sent < total {
      // Detach a run of nodes from the pool under the lock (pure pointer
      // bookkeeping); the lock is released before writing items or
      // publishing, so it's never held across the Vyukov swap or wake.
      let (first_node, k) = loop {
        let mut pool = self.shared.cache.lock();
        if pool.count == 0 {
          // Drop the lock before calling allocate_node, which may park: see
          // the comment on allocate_node for why it must never be held while
          // blocked.
          drop(pool);
          match self.allocate_node() {
            Ok(ptr) => {
              self.shared.cache.lock().push_node(&self.shared, ptr);
            }
            Err(_) => {
              return Err(SendBatchError {
                sent,
                unsent: iter.collect(),
              });
            }
          }
          continue;
        }

        let k = (total - sent).min(pool.count);
        let first_node = pool.head;
        let mut curr = first_node;
        unsafe {
          for _ in 0..(k - 1) {
            curr = self.shared.next_of(curr);
          }
          pool.head = self.shared.next_of(curr);
        }
        pool.count -= k;
        #[cfg(miri)]
        miri::shadow_detach(&self.shared, &mut pool, first_node, k);
        break (first_node, k);
      };

      unsafe { publish_run(&self.shared, first_node, k, &mut iter) };
      sent += k;
    }

    Ok(total)
  }

  pub fn try_send_batch(&self, items: Vec<T>) -> Result<usize, TrySendBatchError<T>> {
    let total = items.len();
    if total == 0 {
      return Ok(0);
    }
    if self.closed.load(Ordering::Relaxed) || !self.shared.receivers_alive() {
      return Err(TrySendBatchError {
        sent: 0,
        unsent: items,
        reason: BatchSendErrorReason::Closed,
      });
    }

    let mut iter = items.into_iter();
    let mut sent = 0;

    while sent < total {
      // Detach a run of nodes from the pool under the lock (pure pointer
      // bookkeeping); the lock is released before writing items or
      // publishing, so it's never held across the Vyukov swap or wake.
      let detached = {
        let mut pool = self.shared.cache.lock();
        if !pool.fill_pool(&self.shared) {
          None
        } else {
          let k = (total - sent).min(pool.count);
          let first_node = pool.head;
          let mut curr = first_node;
          unsafe {
            for _ in 0..(k - 1) {
              curr = self.shared.next_of(curr);
            }
            pool.head = self.shared.next_of(curr);
          }
          pool.count -= k;
          #[cfg(miri)]
          miri::shadow_detach(&self.shared, &mut pool, first_node, k);
          Some((first_node, k))
        }
      };

      let (first_node, k) = match detached {
        Some(v) => v,
        None => break,
      };

      unsafe { publish_run(&self.shared, first_node, k, &mut iter) };
      sent += k;
    }

    if sent == total {
      Ok(total)
    } else {
      Err(TrySendBatchError {
        sent,
        unsent: iter.collect(),
        reason: BatchSendErrorReason::Full,
      })
    }
  }

  pub fn send_batch_mut(&self, items: &mut Vec<T>) -> Result<usize, SendError> {
    if items.is_empty() {
      return Ok(0);
    }
    if self.closed.load(Ordering::Relaxed) || !self.shared.receivers_alive() {
      return Err(SendError::Closed);
    }
    let batch = std::mem::take(items);
    match self.send_batch(batch) {
      Ok(n) => Ok(n),
      Err(e) => {
        *items = e.unsent;
        Err(SendError::Closed)
      }
    }
  }

  pub fn try_send_batch_mut(&self, items: &mut Vec<T>) -> Result<usize, SendError> {
    if items.is_empty() {
      return Ok(0);
    }
    if self.closed.load(Ordering::Relaxed) || !self.shared.receivers_alive() {
      return Err(SendError::Closed);
    }
    let batch = std::mem::take(items);
    match self.try_send_batch(batch) {
      Ok(n) => Ok(n),
      Err(e) => {
        let (sent, reason) = (e.sent, e.reason);
        *items = e.unsent;
        if sent == 0 && matches!(reason, BatchSendErrorReason::Closed) {
          Err(SendError::Closed)
        } else {
          Ok(sent)
        }
      }
    }
  }

  pub fn close(&self) -> Result<(), CloseError> {
    if self
      .closed
      .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
      .is_ok()
    {
      self.shared.drop_sender();
      Ok(())
    } else {
      Err(CloseError)
    }
  }

  pub fn is_closed(&self) -> bool {
    self.closed.load(Ordering::Relaxed) || !self.shared.receivers_alive()
  }

  pub fn capacity(&self) -> usize {
    self.shared.capacity()
  }

  pub fn len(&self) -> usize {
    self.shared.len()
  }

  pub fn is_empty(&self) -> bool {
    self.shared.is_empty()
  }

  pub fn is_full(&self) -> bool {
    self.shared.is_full()
  }

  pub fn to_async(self) -> AsyncSender<T> {
    let shared = unsafe { std::ptr::read(&self.shared) };
    let closed = self.closed.load(Ordering::Relaxed);
    std::mem::forget(self);
    AsyncSender {
      shared,
      closed: AtomicBool::new(closed),
    }
  }
}

impl<T: Send> Clone for Sender<T> {
  fn clone(&self) -> Self {
    self.shared.add_sender();
    Sender {
      shared: Arc::clone(&self.shared),
      closed: AtomicBool::new(false),
    }
  }
}

impl<T: Send> Drop for Sender<T> {
  fn drop(&mut self) {
    let _ = self.close();
  }
}

impl<T: Send> AsyncSender<T> {
  pub fn send(&self, item: T) -> SendFuture<'_, T> {
    SendFuture::new(self, item)
  }

  pub fn try_send(&self, item: T) -> Result<(), TrySendError<T>> {
    if self.closed.load(Ordering::Relaxed) || !self.shared.receivers_alive() {
      return Err(TrySendError::Closed(item));
    }

    let mut pool = self.shared.cache.lock();
    let curr = match pool.pop_node(&self.shared) {
      Some(node) => node,
      None => return Err(TrySendError::Full(item)),
    };

    unsafe { publish_run(&self.shared, curr, 1, &mut std::iter::once(item)) };
    Ok(())
  }

  pub fn close(&self) -> Result<(), CloseError> {
    if self
      .closed
      .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
      .is_ok()
    {
      self.shared.drop_sender();
      Ok(())
    } else {
      Err(CloseError)
    }
  }

  pub fn is_closed(&self) -> bool {
    self.closed.load(Ordering::Relaxed) || !self.shared.receivers_alive()
  }

  pub fn capacity(&self) -> usize {
    self.shared.capacity()
  }

  pub fn len(&self) -> usize {
    self.shared.len()
  }

  pub fn is_empty(&self) -> bool {
    self.shared.is_empty()
  }

  pub fn is_full(&self) -> bool {
    self.shared.is_full()
  }

  pub fn to_sync(self) -> Sender<T> {
    let shared = unsafe { std::ptr::read(&self.shared) };
    let closed = self.closed.load(Ordering::Relaxed);
    std::mem::forget(self);
    Sender {
      shared,
      closed: AtomicBool::new(closed),
    }
  }

  pub fn try_send_batch(&self, items: Vec<T>) -> Result<usize, TrySendBatchError<T>> {
    let total = items.len();
    if total == 0 {
      return Ok(0);
    }
    if self.closed.load(Ordering::Relaxed) || !self.shared.receivers_alive() {
      return Err(TrySendBatchError {
        sent: 0,
        unsent: items,
        reason: BatchSendErrorReason::Closed,
      });
    }

    let mut iter = items.into_iter();
    let mut sent = 0;
    let mut pool = self.shared.cache.lock();

    while sent < total {
      if !pool.fill_pool(&self.shared) {
        break;
      }

      let k = (total - sent).min(pool.count);
      unsafe {
        let first_node = pool.head;
        let mut curr = first_node;
        for _ in 0..(k - 1) {
          curr = self.shared.next_of(curr);
        }
        pool.head = self.shared.next_of(curr);
        pool.count -= k;

        #[cfg(miri)]
        miri::shadow_detach(&self.shared, &mut pool, first_node, k);
        publish_run(&self.shared, first_node, k, &mut iter);
      }
      sent += k;
    }

    if sent == total {
      Ok(total)
    } else {
      Err(TrySendBatchError {
        sent,
        unsent: iter.collect(),
        reason: BatchSendErrorReason::Full,
      })
    }
  }

  pub fn try_send_batch_mut(&self, items: &mut Vec<T>) -> Result<usize, SendError> {
    if items.is_empty() {
      return Ok(0);
    }
    if self.closed.load(Ordering::Relaxed) || !self.shared.receivers_alive() {
      return Err(SendError::Closed);
    }
    let batch = std::mem::take(items);
    match self.try_send_batch(batch) {
      Ok(n) => Ok(n),
      Err(e) => {
        let (sent, reason) = (e.sent, e.reason);
        *items = e.unsent;
        if sent == 0 && matches!(reason, BatchSendErrorReason::Closed) {
          Err(SendError::Closed)
        } else {
          Ok(sent)
        }
      }
    }
  }
}

impl<T: Send> Clone for AsyncSender<T> {
  fn clone(&self) -> Self {
    self.shared.add_sender();
    AsyncSender {
      shared: Arc::clone(&self.shared),
      closed: AtomicBool::new(false),
    }
  }
}

impl<T: Send> Drop for AsyncSender<T> {
  fn drop(&mut self) {
    let _ = self.close();
  }
}

// --- Futures ---

#[must_use = "futures do nothing unless you .await or poll them"]
pub struct SendFuture<'a, T: Send> {
  sender: &'a AsyncSender<T>,
  item: Option<T>,
  my_id: Option<u64>,
  _phantom: PhantomPinned,
}

impl<'a, T: Send> SendFuture<'a, T> {
  fn new(sender: &'a AsyncSender<T>, item: T) -> Self {
    SendFuture {
      sender,
      item: Some(item),
      my_id: None,
      _phantom: PhantomPinned,
    }
  }

  fn unregister(&mut self) {
    if let Some(id) = self.my_id.take() {
      self.sender.shared.unregister_async_send(id);
    }
  }
}

impl<'a, T: Send> Future for SendFuture<'a, T> {
  type Output = Result<(), SendError>;

  fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    let this = unsafe { self.as_mut().get_unchecked_mut() };
    let shared = &this.sender.shared;

    let mut pool = this.sender.shared.cache.lock();

    loop {
      let item_val = match this.item.take() {
        Some(it) => it,
        None => return Poll::Ready(Ok(())),
      };

      if this.sender.closed.load(Ordering::Relaxed) || !shared.receivers_alive() {
        this.unregister();
        return Poll::Ready(Err(SendError::Closed));
      }

      if !pool.fill_pool(shared) {
        let id = shared.register_async_send(
          this.my_id,
          cx.waker().clone(),
          std::ptr::null(),
        );
        this.my_id = Some(id);
        shared.pre_park_fence();

        if this.sender.closed.load(Ordering::Relaxed) || !shared.receivers_alive() {
          shared.unregister_async_send(id);
          this.my_id = None;
          this.item = Some(item_val);
          return Poll::Ready(Err(SendError::Closed));
        }

        if !pool.fill_pool(shared) {
          this.item = Some(item_val);
          return Poll::Pending;
        }
      }

      unsafe {
        let curr = pool.pop_node(shared).unwrap();
        publish_run(shared, curr, 1, &mut std::iter::once(item_val));
      }

      if let Some(id) = this.my_id.take() {
        shared.unregister_async_send(id);
      }
      return Poll::Ready(Ok(()));
    }
  }
}

impl<'a, T: Send> Drop for SendFuture<'a, T> {
  fn drop(&mut self) {
    self.unregister();
  }
}

// --- Async Batch Futures ---

#[must_use = "futures do nothing unless you .await or poll them"]
pub struct BoundedSendBatchFuture<'a, T: Send> {
  sender: &'a AsyncSender<T>,
  iter: std::vec::IntoIter<T>,
  total: usize,
  sent: usize,
  my_id: Option<u64>,
  _phantom: PhantomPinned,
}

impl<'a, T: Send> Future for BoundedSendBatchFuture<'a, T> {
  type Output = Result<usize, SendBatchError<T>>;

  fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    let this = unsafe { self.as_mut().get_unchecked_mut() };
    let shared = &this.sender.shared;
    let mut pool = this.sender.shared.cache.lock();

    loop {
      if this.sent == this.total {
        if let Some(id) = this.my_id.take() {
          shared.unregister_async_send(id);
        }
        return Poll::Ready(Ok(this.total));
      }

      if this.sender.closed.load(Ordering::Relaxed) || !shared.receivers_alive() {
        if let Some(id) = this.my_id.take() {
          shared.unregister_async_send(id);
        }
        return Poll::Ready(Err(SendBatchError {
          sent: this.sent,
          unsent: this.iter.by_ref().collect(),
        }));
      }

      if !pool.fill_pool(shared) {
        let id = shared.register_async_send(
          this.my_id,
          cx.waker().clone(),
          std::ptr::null(),
        );
        this.my_id = Some(id);
        shared.pre_park_fence();

        if this.sender.closed.load(Ordering::Relaxed) || !shared.receivers_alive() {
          shared.unregister_async_send(id);
          this.my_id = None;
          return Poll::Ready(Err(SendBatchError {
            sent: this.sent,
            unsent: this.iter.by_ref().collect(),
          }));
        }

        if !pool.fill_pool(shared) {
          return Poll::Pending;
        }
      }

      let remaining = this.total - this.sent;
      let local_available = pool.count;
      let k = remaining.min(local_available);

      unsafe {
        let first_node = pool.head;
        let mut curr = first_node;
        for _ in 0..(k - 1) {
          curr = shared.next_of(curr);
        }
        pool.head = shared.next_of(curr);
        pool.count -= k;

        #[cfg(miri)]
        miri::shadow_detach(shared, &mut pool, first_node, k);
        publish_run(shared, first_node, k, &mut this.iter);
      }
      this.sent += k;
    }
  }
}

impl<'a, T: Send> Drop for BoundedSendBatchFuture<'a, T> {
  fn drop(&mut self) {
    if let Some(id) = self.my_id.take() {
      self.sender.shared.unregister_async_send(id);
    }
  }
}

#[must_use = "futures do nothing unless you .await or poll them"]
pub struct BoundedSendBatchMutFuture<'a, T: Send> {
  sender: &'a AsyncSender<T>,
  items: &'a mut Vec<T>,
  sent: usize,
  my_id: Option<u64>,
  _phantom: PhantomPinned,
}

impl<'a, T: Send> Future for BoundedSendBatchMutFuture<'a, T> {
  type Output = Result<usize, SendError>;

  fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    let this = unsafe { self.as_mut().get_unchecked_mut() };
    let shared = &this.sender.shared;
    let mut pool = this.sender.shared.cache.lock();

    loop {
      let remaining = this.items.len();
      if remaining == 0 {
        if let Some(id) = this.my_id.take() {
          shared.unregister_async_send(id);
        }
        return Poll::Ready(Ok(this.sent));
      }

      if this.sender.closed.load(Ordering::Relaxed) || !shared.receivers_alive() {
        if let Some(id) = this.my_id.take() {
          shared.unregister_async_send(id);
        }
        return Poll::Ready(Err(SendError::Closed));
      }

      if !pool.fill_pool(shared) {
        let id = shared.register_async_send(
          this.my_id,
          cx.waker().clone(),
          std::ptr::null(),
        );
        this.my_id = Some(id);
        shared.pre_park_fence();

        if this.sender.closed.load(Ordering::Relaxed) || !shared.receivers_alive() {
          shared.unregister_async_send(id);
          this.my_id = None;
          return Poll::Ready(Err(SendError::Closed));
        }

        if !pool.fill_pool(shared) {
          return Poll::Pending;
        }
      }

      let local_available = pool.count;
      let k = remaining.min(local_available);

      unsafe {
        let first_node = pool.head;
        let mut curr = first_node;
        for _ in 0..(k - 1) {
          curr = shared.next_of(curr);
        }
        pool.head = shared.next_of(curr);
        pool.count -= k;

        #[cfg(miri)]
        miri::shadow_detach(shared, &mut pool, first_node, k);
        let mut drain = this.items.drain(..k);
        publish_run(shared, first_node, k, &mut drain);
        drop(drain);
      }
      this.sent += k;
    }
  }
}

impl<'a, T: Send> Drop for BoundedSendBatchMutFuture<'a, T> {
  fn drop(&mut self) {
    if let Some(id) = self.my_id.take() {
      self.sender.shared.unregister_async_send(id);
    }
  }
}

impl<T: Send> AsyncSender<T> {
  pub fn send_batch(&self, items: Vec<T>) -> BoundedSendBatchFuture<'_, T> {
    let total = items.len();
    BoundedSendBatchFuture {
      sender: self,
      iter: items.into_iter(),
      total,
      sent: 0,
      my_id: None,
      _phantom: PhantomPinned,
    }
  }

  pub fn send_batch_mut<'a>(&'a self, items: &'a mut Vec<T>) -> BoundedSendBatchMutFuture<'a, T> {
    BoundedSendBatchMutFuture {
      sender: self,
      items,
      sent: 0,
      my_id: None,
      _phantom: PhantomPinned,
    }
  }
}
