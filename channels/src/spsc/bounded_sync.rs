use crate::error::{
  BatchSendErrorReason, CloseError, RecvError, RecvErrorTimeout, SendBatchError, SendError,
  TryRecvError, TrySendBatchError, TrySendError,
};
use crate::spsc::shared::{Role, SpscShared, WakeRef};
use crate::sync_util;

use std::cell::Cell;
use std::marker::PhantomData;
use std::sync::atomic::{self, AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self};
use std::time::{Duration, Instant};

use super::bounded_async::{BoundedAsyncReceiver, BoundedAsyncSender};
use std::mem;

// Adaptive spin backoff constants.
const SPIN_INITIAL: u32 = 16;
const SPIN_MIN: u32 = 2;
const SPIN_MAX: u32 = 64;

thread_local! {
  static THREAD_SPIN_LIMIT: Cell<u32> = Cell::new(SPIN_INITIAL);
}

/// The synchronous sending end of a bounded SPSC channel.
#[derive(Debug)]
pub struct BoundedSyncSender<T> {
  pub(crate) shared: Arc<SpscShared<T>>,
  pub(crate) closed: AtomicBool,
  // This PhantomData makes BoundedSyncSender<T> !Sync.
  pub(crate) _phantom: PhantomData<*mut ()>,
}

/// The synchronous receiving end of a bounded SPSC channel.
#[derive(Debug)]
pub struct BoundedSyncReceiver<T> {
  pub(crate) shared: Arc<SpscShared<T>>,
  pub(crate) closed: AtomicBool,
  // This PhantomData makes BoundedSyncReceiver<T> !Sync.
  pub(crate) _phantom: PhantomData<*mut ()>,
}

unsafe impl<T: Send> Send for BoundedSyncSender<T> {}
unsafe impl<T: Send> Send for BoundedSyncReceiver<T> {}

impl<T> BoundedSyncSender<T> {
  fn close_internal(&self) {
    self.shared.producer_dropped.store(true, Ordering::Release);
    self.shared.drop_sender();
  }
}

impl<T: Send> BoundedSyncSender<T> {
  pub(crate) fn from_shared(shared: Arc<SpscShared<T>>) -> Self {
    Self {
      shared,
      closed: AtomicBool::new(false),
      _phantom: PhantomData,
    }
  }

  pub fn to_async(self) -> BoundedAsyncSender<T> {
    let shared = unsafe { std::ptr::read(&self.shared) };
    mem::forget(self);
    BoundedAsyncSender::from_shared(shared)
  }

  pub fn try_send(&self, item: T) -> Result<(), TrySendError<T>> {
    if self.closed.load(Ordering::Relaxed) {
      return Err(TrySendError::Closed(item));
    }
    if self.shared.consumer_dropped.load(Ordering::Acquire) {
      return Err(TrySendError::Closed(item));
    }
    match self.shared.ring.push(item) {
      Ok(()) => {
        self.shared.notify_receivers();
        Ok(())
      }
      Err(returned) => Err(TrySendError::Full(returned)),
    }
  }

  pub fn send(&self, item: T) -> Result<(), SendError> {
    let mut item = item;
    let mut is_registered = false;
    let notified = AtomicBool::new(false);
    let notified_ptr = &notified as *const AtomicBool;
    let spin_limit = THREAD_SPIN_LIMIT.with(|c| c.get());

    if self.closed.load(Ordering::Relaxed) || self.shared.consumer_dropped.load(Ordering::Acquire) {
      return Err(SendError::Closed);
    }

    match self.shared.ring.push(item) {
      Ok(()) => {
        self.shared.notify_receivers();
        return Ok(());
      }
      Err(returned) => item = returned,
    }

    let mut backoff = 0;
    loop {
      if self.shared.consumer_dropped.load(Ordering::Acquire) {
        if is_registered {
          self.shared.unregister(Role::Send);
        }
        return Err(SendError::Closed);
      }

      match self.shared.ring.push(item) {
        Ok(()) => {
          if is_registered {
            self.shared.unregister(Role::Send);
          }
          self.shared.notify_receivers();
          THREAD_SPIN_LIMIT.with(|c| c.set((c.get() + 1).min(SPIN_MAX)));
          return Ok(());
        }
        Err(returned) => item = returned,
      }

      if is_registered {
        THREAD_SPIN_LIMIT.with(|c| c.set((c.get() / 2).max(SPIN_MIN)));
        sync_util::park_thread();
        if notified.swap(false, Ordering::Acquire) {
          is_registered = false;
          backoff = 0;
        }
        continue;
      }

      if backoff < spin_limit {
        std::hint::spin_loop();
        backoff += 1;
        continue;
      }

      self
        .shared
        .register(Role::Send, WakeRef::Thread(thread::current()), notified_ptr);
      is_registered = true;
      self.shared.pre_park_fence();
    }
  }

  pub fn try_send_batch(&self, items: Vec<T>) -> Result<usize, TrySendBatchError<T>> {
    let total = items.len();
    if total == 0 {
      return Ok(0);
    }
    let mut iter = items.into_iter();
    if self.closed.load(Ordering::Relaxed) || self.shared.consumer_dropped.load(Ordering::Acquire) {
      return Err(TrySendBatchError {
        sent: 0,
        unsent: iter.collect(),
        reason: BatchSendErrorReason::Closed,
      });
    }
    let sent = self.shared.write_batch(&mut iter, total);
    if sent == total {
      Ok(total)
    } else {
      let reason = if self.shared.consumer_dropped.load(Ordering::Acquire) {
        BatchSendErrorReason::Closed
      } else {
        BatchSendErrorReason::Full
      };
      Err(TrySendBatchError {
        sent,
        unsent: iter.collect(),
        reason,
      })
    }
  }

  pub fn send_batch(&self, items: Vec<T>) -> Result<usize, SendBatchError<T>> {
    let mut iter = items.into_iter();
    let mut sent = 0;
    let mut is_registered = false;

    while let Some(mut item) = iter.next() {
      loop {
        if self.shared.consumer_dropped.load(Ordering::Acquire) {
          if is_registered {
            self.shared.unregister(Role::Send);
          }
          let mut unsent = Vec::with_capacity(iter.len() + 1);
          unsent.push(item);
          unsent.extend(iter);
          return Err(SendBatchError { sent, unsent });
        }
        match self.shared.ring.push(item) {
          Ok(()) => {
            sent += 1;
            self.shared.notify_receivers();
            break;
          }
          Err(returned) => item = returned,
        }

        self.shared.register(
          Role::Send,
          WakeRef::Thread(thread::current()),
          std::ptr::null(),
        );
        is_registered = true;
        self.shared.pre_park_fence();

        match self.shared.ring.push(item) {
          Ok(()) => {
            sent += 1;
            self.shared.notify_receivers();
            break;
          }
          Err(returned) => item = returned,
        }
        if self.shared.consumer_dropped.load(Ordering::Acquire) {
          continue;
        }
        sync_util::park_thread();
      }
    }

    if is_registered {
      self.shared.unregister(Role::Send);
    }
    Ok(sent)
  }

  pub fn try_send_batch_mut(&self, items: &mut Vec<T>) -> Result<usize, SendError> {
    if items.is_empty() {
      return Ok(0);
    }
    if self.closed.load(Ordering::Relaxed) || self.shared.consumer_dropped.load(Ordering::Acquire) {
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

  pub fn send_batch_mut(&self, items: &mut Vec<T>) -> Result<usize, SendError> {
    if items.is_empty() {
      return Ok(0);
    }
    if self.closed.load(Ordering::Relaxed) {
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

  pub fn is_closed(&self) -> bool {
    self.closed.load(Ordering::Relaxed) || self.shared.consumer_dropped.load(Ordering::Acquire)
  }

  pub fn capacity(&self) -> usize {
    self.shared.capacity()
  }

  #[inline]
  pub fn len(&self) -> usize {
    self.shared.len()
  }

  #[inline]
  pub fn is_empty(&self) -> bool {
    self.shared.is_empty()
  }

  #[inline]
  pub fn is_full(&self) -> bool {
    self.shared.is_full()
  }
}

impl<T> Drop for BoundedSyncSender<T> {
  fn drop(&mut self) {
    if !self.closed.swap(true, Ordering::AcqRel) {
      self.close_internal();
    }
  }
}

// Methods that do not require T: Send
impl<T> BoundedSyncReceiver<T> {
  fn close_internal(&self) {
    self.shared.consumer_dropped.store(true, Ordering::Release);
    self.shared.drop_receiver();
  }
}

// Methods that require T: Send for the channel to be usable
impl<T: Send> BoundedSyncReceiver<T> {
  pub(crate) fn from_shared(shared: Arc<SpscShared<T>>) -> Self {
    Self {
      shared,
      closed: AtomicBool::new(false),
      _phantom: PhantomData,
    }
  }

  pub fn to_async(self) -> BoundedAsyncReceiver<T> {
    let shared = unsafe { std::ptr::read(&self.shared) };
    mem::forget(self);
    BoundedAsyncReceiver::from_shared(shared)
  }

  pub fn try_recv(&self) -> Result<T, TryRecvError> {
    if self.closed.load(Ordering::Relaxed) {
      return Err(TryRecvError::Disconnected);
    }
    match self.shared.ring.pop() {
      Some(item) => {
        self.shared.notify_senders();
        Ok(item)
      }
      None => {
        if !self.shared.senders_alive() {
          if let Some(item) = self.shared.ring.pop() {
            self.shared.notify_senders();
            return Ok(item);
          }
          Err(TryRecvError::Disconnected)
        } else {
          Err(TryRecvError::Empty)
        }
      }
    }
  }

  pub fn recv(&self) -> Result<T, RecvError> {
    let mut is_registered = false;
    let notified = AtomicBool::new(false);
    let notified_ptr = &notified as *const AtomicBool;
    let spin_limit = THREAD_SPIN_LIMIT.with(|c| c.get());

    if self.closed.load(Ordering::Relaxed) {
      return Err(RecvError::Disconnected);
    }

    if let Some(item) = self.shared.ring.pop() {
      self.shared.notify_senders();
      return Ok(item);
    }

    let mut backoff = 0;
    loop {
      if let Some(item) = self.shared.ring.pop() {
        if is_registered {
          self.shared.unregister(Role::Recv);
        }
        self.shared.notify_senders();
        THREAD_SPIN_LIMIT.with(|c| c.set((c.get() + 1).min(SPIN_MAX)));
        return Ok(item);
      }

      if !self.shared.senders_alive() {
        if let Some(item) = self.shared.ring.pop() {
          if is_registered {
            self.shared.unregister(Role::Recv);
          }
          self.shared.notify_senders();
          return Ok(item);
        }
        if is_registered {
          self.shared.unregister(Role::Recv);
        }
        return Err(RecvError::Disconnected);
      }

      if is_registered {
        THREAD_SPIN_LIMIT.with(|c| c.set((c.get() / 2).max(SPIN_MIN)));
        sync_util::park_thread();
        if notified.swap(false, Ordering::Acquire) {
          is_registered = false;
          backoff = 0;
        }
        continue;
      }

      if backoff < spin_limit {
        std::hint::spin_loop();
        backoff += 1;
        continue;
      }

      self
        .shared
        .register(Role::Recv, WakeRef::Thread(thread::current()), notified_ptr);
      is_registered = true;
      self.shared.pre_park_fence();
    }
  }

  pub fn try_recv_batch(&self, max: usize) -> Result<Vec<T>, TryRecvError> {
    let mut out = Vec::new();
    self.try_recv_batch_mut(&mut out, max)?;
    Ok(out)
  }

  pub fn try_recv_batch_mut(&self, out: &mut Vec<T>, max: usize) -> Result<usize, TryRecvError> {
    if max == 0 {
      return Ok(0);
    }
    if self.closed.load(Ordering::Relaxed) {
      return Err(TryRecvError::Disconnected);
    }
    let k = self.shared.read_batch(out, max);
    if k > 0 {
      return Ok(k);
    }
    if !self.shared.senders_alive() {
      let k = self.shared.read_batch(out, max);
      if k > 0 {
        return Ok(k);
      }
      return Err(TryRecvError::Disconnected);
    }
    Err(TryRecvError::Empty)
  }

  pub fn recv_batch(&self, max: usize) -> Result<Vec<T>, RecvError> {
    let mut out = Vec::new();
    self.recv_batch_mut(&mut out, max)?;
    Ok(out)
  }

  pub fn recv_batch_mut(&self, out: &mut Vec<T>, max: usize) -> Result<usize, RecvError> {
    if max == 0 {
      return Ok(0);
    }
    if self.closed.load(Ordering::Relaxed) {
      return Err(RecvError::Disconnected);
    }
    let first = self.recv()?;
    out.push(first);
    let mut got = 1;
    while got < max {
      match self.shared.ring.pop() {
        Some(item) => {
          out.push(item);
          got += 1;
          self.shared.notify_senders();
        }
        None => break,
      }
    }
    Ok(got)
  }

  pub fn recv_timeout(&mut self, timeout: Duration) -> Result<T, RecvErrorTimeout> {
    if self.closed.load(Ordering::Relaxed) {
      return Err(RecvErrorTimeout::Disconnected);
    }
    let deadline = Instant::now().checked_add(timeout);

    if let Some(item) = self.shared.ring.pop() {
      self.shared.notify_senders();
      return Ok(item);
    }

    let mut is_registered = false;
    let notified = AtomicBool::new(false);
    let notified_ptr = &notified as *const AtomicBool;

    loop {
      if let Some(item) = self.shared.ring.pop() {
        if is_registered {
          self.shared.unregister(Role::Recv);
        }
        self.shared.notify_senders();
        return Ok(item);
      }

      if !self.shared.senders_alive() {
        if let Some(item) = self.shared.ring.pop() {
          if is_registered {
            self.shared.unregister(Role::Recv);
          }
          self.shared.notify_senders();
          return Ok(item);
        }
        if is_registered {
          self.shared.unregister(Role::Recv);
        }
        return Err(RecvErrorTimeout::Disconnected);
      }

      let now = Instant::now();
      let remaining = match deadline {
        Some(d) if d > now => d - now,
        _ => {
          if is_registered {
            self.shared.unregister(Role::Recv);
          }
          return Err(RecvErrorTimeout::Timeout);
        }
      };

      if is_registered {
        sync_util::park_thread_timeout(remaining);
        if notified.swap(false, Ordering::Acquire) {
          is_registered = false;
        }
        continue;
      }

      self
        .shared
        .register(Role::Recv, WakeRef::Thread(thread::current()), notified_ptr);
      is_registered = true;
      self.shared.pre_park_fence();
    }
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

  pub fn is_closed(&self) -> bool {
    self.closed.load(Ordering::Relaxed) || (!self.shared.senders_alive() && self.shared.is_empty())
  }

  pub fn capacity(&self) -> usize {
    self.shared.capacity()
  }

  #[inline]
  pub fn len(&self) -> usize {
    self.shared.len()
  }

  #[inline]
  pub fn is_empty(&self) -> bool {
    self.shared.is_empty()
  }

  #[inline]
  pub fn is_full(&self) -> bool {
    self.shared.is_full()
  }
}

impl<T> Drop for BoundedSyncReceiver<T> {
  fn drop(&mut self) {
    if !self.closed.swap(true, Ordering::AcqRel) {
      self.close_internal();
    }
  }
}

/// Creates a new bounded synchronous SPSC channel with the given capacity.
///
/// `capacity` must be greater than 0. Panics if capacity is 0.
pub fn bounded_sync<T: Send>(capacity: usize) -> (BoundedSyncSender<T>, BoundedSyncReceiver<T>) {
  let shared_core = SpscShared::new_internal(capacity);
  let shared_arc = Arc::new(shared_core);

  (
    BoundedSyncSender {
      shared: Arc::clone(&shared_arc),
      closed: AtomicBool::new(false),
      _phantom: PhantomData,
    },
    BoundedSyncReceiver {
      shared: shared_arc,
      closed: AtomicBool::new(false),
      _phantom: PhantomData,
    },
  )
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::{thread, time::Duration};

  #[test]
  fn create_channel() {
    let (p, c) = bounded_sync::<i32>(1);
    assert_eq!(p.shared.capacity(), 1);
    assert_eq!(c.shared.capacity(), 1);
    assert!(p.is_empty());
    assert!(c.is_empty());
    assert!(!p.is_full());
    assert!(!c.is_full());
    assert_eq!(p.len(), 0);
    assert_eq!(c.len(), 0);
  }

  #[test]
  #[should_panic]
  fn create_channel_zero_capacity() {
    let (_, _) = bounded_sync::<i32>(0);
  }

  #[test]
  fn send_recv_single_item() {
    let (p, c) = bounded_sync(1);
    assert!(p.is_empty());
    p.send(42i32).unwrap();
    assert!(p.is_full());
    assert!(!p.is_empty());
    assert_eq!(p.len(), 1);
    assert!(c.is_full());
    assert!(!c.is_empty());
    assert_eq!(c.len(), 1);

    assert_eq!(c.recv().unwrap(), 42);
    assert!(p.is_empty());
    assert!(c.is_empty());
    assert_eq!(p.len(), 0);
    assert_eq!(c.len(), 0);
  }

  #[test]
  fn try_send_full_try_recv_empty() {
    let (p, c) = bounded_sync::<i32>(1);
    p.try_send(10).unwrap();
    assert!(p.is_full());
    assert_eq!(p.len(), 1);

    match p.try_send(20) {
      Err(TrySendError::Full(val)) => assert_eq!(val, 20),
      res => panic!("Expected Full error, got {:?}", res),
    }
    assert!(p.is_full()); // Still full

    assert_eq!(c.try_recv().unwrap(), 10);
    assert!(c.is_empty());
    assert_eq!(c.len(), 0);

    match c.try_recv() {
      Err(TryRecvError::Empty) => {}
      res => panic!("Expected Empty error, got {:?}", res),
    }
    assert!(c.is_empty());
  }

  #[test]
  fn send_blocks_until_recv() {
    let (p, c) = bounded_sync::<i32>(1);
    p.send(1).unwrap(); // Fill channel
    assert_eq!(p.len(), 1);

    let producer_thread = thread::spawn(move || {
      p.send(2).unwrap(); // This should block
      assert_eq!(p.len(), 1); // After send, it's full again before consumer gets it
      p // Return producer to drop it in main thread, ensuring its Drop runs
    });

    thread::sleep(Duration::from_millis(100)); // Give producer time to block

    assert_eq!(c.recv().unwrap(), 1); // Unblock producer
    let _p_returned = producer_thread.join().unwrap(); // Sender should now complete
    assert_eq!(c.recv().unwrap(), 2);
    assert_eq!(c.len(), 0);
  }

  #[test]
  fn recv_blocks_until_send() {
    let (p, c) = bounded_sync::<i32>(1);

    let consumer_thread = thread::spawn(move || {
      let val = c.recv().unwrap(); // This should block
      assert_eq!(val, 100);
      assert_eq!(c.len(), 0);
      c // Return consumer
    });

    thread::sleep(Duration::from_millis(100)); // Give consumer time to block

    p.send(100).unwrap(); // Unblock consumer
    let _c_returned = consumer_thread.join().unwrap();

    // NOW we know the consumer received it, so the channel is empty
    assert_eq!(p.len(), 0);
  }

  #[test]
  fn producer_drop_signals_consumer() {
    let (p, c) = bounded_sync::<i32>(1);
    p.send(7).unwrap();
    assert_eq!(p.len(), 1);
    drop(p);
    assert_eq!(c.len(), 1);
    assert_eq!(c.recv().unwrap(), 7);
    assert_eq!(c.len(), 0);
    match c.recv() {
      Err(RecvError::Disconnected) => {}
      Ok(v) => panic!("Expected Disconnected error, got Ok({:?})", v),
    }
  }

  #[test]
  fn producer_drop_empty_signals_consumer() {
    let (p, c) = bounded_sync::<i32>(1);
    assert_eq!(p.len(), 0);
    drop(p);
    assert_eq!(c.len(), 0);
    match c.recv() {
      Err(RecvError::Disconnected) => {}
      Ok(v) => panic!("Expected Disconnected error, got Ok({:?})", v),
    }
  }

  #[test]
  fn consumer_drop_signals_producer() {
    let (p, c) = bounded_sync::<i32>(1);
    assert_eq!(p.len(), 0);
    drop(c);
    match p.send(1) {
      Err(SendError::Closed) => {}
      Ok(_) => panic!("Expected Closed error after consumer drop"),
      Err(SendError::Sent) => panic!("SPSC should not produce Sent error"),
    }
  }

  #[test]
  fn stress_send_recv() {
    #[cfg(not(miri))]
    const ITEMS: usize = 100_000;
    #[cfg(miri)]
    const ITEMS: usize = 10000;
    const CAPACITY: usize = 128;
    let (p, c) = bounded_sync(CAPACITY);

    let producer_handle = thread::spawn(move || {
      for i in 0..ITEMS {
        p.send(i).unwrap();
      }
    });

    let consumer_handle = thread::spawn(move || {
      for i in 0..ITEMS {
        assert_eq!(c.recv().unwrap(), i);
      }
      assert!(c.is_empty());
    });

    producer_handle.join().unwrap();
    consumer_handle.join().unwrap();
  }

  #[test]
  fn try_send_closed_by_consumer_drop() {
    let (p, c) = bounded_sync::<i32>(5);
    p.try_send(1).unwrap();
    assert_eq!(p.len(), 1);
    drop(c);
    match p.try_send(2) {
      Err(TrySendError::Closed(val)) => assert_eq!(val, 2),
      other => panic!("Expected TrySendError::Closed, got {:?}", other),
    }
  }

  #[test]
  fn try_recv_disconnected_by_producer_drop() {
    let (p, c) = bounded_sync::<i32>(5);
    p.try_send(10).unwrap();
    assert_eq!(p.len(), 1);
    drop(p);
    assert_eq!(c.len(), 1);
    assert_eq!(c.try_recv().unwrap(), 10);
    assert_eq!(c.len(), 0);
    match c.try_recv() {
      Err(TryRecvError::Disconnected) => {}
      other => panic!("Expected TryRecvError::Disconnected, got {:?}", other),
    }
  }
  #[test]
  fn try_recv_disconnected_by_producer_drop_empty() {
    let (p, c) = bounded_sync::<i32>(5);
    drop(p);
    match c.try_recv() {
      Err(TryRecvError::Disconnected) => {}
      other => panic!("Expected TryRecvError::Disconnected, got {:?}", other),
    }
  }

  #[test]
  fn values_are_dropped() {
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
    static DROP_COUNT: AtomicUsize = AtomicUsize::new(0);
    #[derive(Debug)]
    struct Droppable(Arc<AtomicUsize>);
    impl Drop for Droppable {
      fn drop(&mut self) {
        self.0.fetch_add(1, AtomicOrdering::Relaxed);
      }
    }

    let drop_counter_arc = Arc::new(AtomicUsize::new(0));
    drop_counter_arc.store(0, AtomicOrdering::Relaxed);
    {
      let (p, c) = bounded_sync::<Droppable>(2);
      p.send(Droppable(drop_counter_arc.clone())).unwrap();
      p.send(Droppable(drop_counter_arc.clone())).unwrap();
      assert_eq!(drop_counter_arc.load(AtomicOrdering::Relaxed), 0);
      assert_eq!(p.len(), 2);
      drop(p);
      assert_eq!(c.len(), 2);

      let d1 = c.recv().unwrap();
      assert_eq!(drop_counter_arc.load(AtomicOrdering::Relaxed), 0);
      assert_eq!(c.len(), 1);
      drop(d1);
      assert_eq!(drop_counter_arc.load(AtomicOrdering::Relaxed), 1);

      let d2 = c.recv().unwrap();
      assert_eq!(drop_counter_arc.load(AtomicOrdering::Relaxed), 1);
      assert_eq!(c.len(), 0);
      drop(d2);
      assert_eq!(drop_counter_arc.load(AtomicOrdering::Relaxed), 2);
    }
    assert_eq!(drop_counter_arc.load(AtomicOrdering::Relaxed), 2);

    drop_counter_arc.store(0, AtomicOrdering::Relaxed);
    {
      let (p, c) = bounded_sync::<Droppable>(2);
      p.send(Droppable(drop_counter_arc.clone())).unwrap();
      p.send(Droppable(drop_counter_arc.clone())).unwrap();
      drop(p);
      drop(c);
    }
    assert_eq!(drop_counter_arc.load(AtomicOrdering::Relaxed), 2);

    drop_counter_arc.store(0, AtomicOrdering::Relaxed);
    {
      let (p, c) = bounded_sync::<Droppable>(1);
      p.send(Droppable(drop_counter_arc.clone())).unwrap();
      drop(p);
      drop(c);
    }
    assert_eq!(drop_counter_arc.load(AtomicOrdering::Relaxed), 1);
  }

  #[test]
  #[cfg(not(miri))]
  fn recv_timeout_empty_times_out() {
    let (_p, mut c) = bounded_sync::<i32>(1);
    let res = c.recv_timeout(Duration::from_millis(50));
    assert!(matches!(res, Err(RecvErrorTimeout::Timeout)));
  }

  #[test]
  #[cfg(not(miri))]
  fn recv_timeout_item_arrives() {
    let (p, mut c) = bounded_sync::<i32>(1);
    let val_to_send = 123;

    let consumer_handle = thread::spawn(move || c.recv_timeout(Duration::from_secs(1)).unwrap());

    thread::sleep(Duration::from_millis(50)); // Ensure consumer blocks
    p.send(val_to_send).unwrap();

    assert_eq!(consumer_handle.join().unwrap(), val_to_send);
  }

  #[test]
  fn recv_timeout_producer_drops_empty() {
    let (p, mut c) = bounded_sync::<i32>(1);
    drop(p);
    let res = c.recv_timeout(Duration::from_millis(50));
    assert!(matches!(res, Err(RecvErrorTimeout::Disconnected)));
  }

  #[test]
  #[cfg(not(miri))]
  fn recv_timeout_producer_drops_with_item() {
    let (p, mut c) = bounded_sync::<i32>(1);
    p.send(99).unwrap();
    drop(p);
    assert_eq!(c.recv_timeout(Duration::from_millis(50)).unwrap(), 99);
    assert!(matches!(
      c.recv_timeout(Duration::from_millis(50)),
      Err(RecvErrorTimeout::Disconnected)
    ));
  }

  #[test]
  #[cfg(not(miri))]
  fn new_spsc_apis_capacity_close_is_closed() {
    let (p, c) = bounded_sync::<i32>(5);
    assert_eq!(p.capacity(), 5);
    assert_eq!(c.capacity(), 5);
    assert!(!p.is_closed());
    assert!(!c.is_closed());

    // Close the sender
    p.close().unwrap();
    assert!(p.is_closed());
    assert_eq!(p.close(), Err(CloseError)); // Idempotent
    assert_eq!(p.send(1), Err(SendError::Closed)); // Cannot send on closed handle

    // Receiver should see the channel is now closed
    assert!(c.is_closed());
    assert_eq!(c.recv(), Err(RecvError::Disconnected));

    // Test receiver closing
    let (p, c) = bounded_sync::<i32>(5);
    c.close().unwrap();
    assert!(c.is_closed());
    assert_eq!(c.close(), Err(CloseError));
    assert_eq!(c.recv(), Err(RecvError::Disconnected));

    // Sender should see the channel is now closed
    assert!(p.is_closed());
    assert_eq!(p.send(1), Err(SendError::Closed));
  }

  #[test]
  fn sync_sender_unblocks_on_consumer_drop() {
    let (p, c) = bounded_sync(1);
    // Fill the channel capacity
    p.send(1).unwrap();

    let handle = thread::spawn(move || {
      // This send should block
      p.send(2)
    });

    // Give the sender thread some time to park
    thread::sleep(Duration::from_millis(50));
    assert!(!handle.is_finished(), "Sender thread should be blocked");

    // Drop the consumer to trigger unblocking
    drop(c);

    let res = handle.join().expect("Sender thread panicked");
    assert!(
      matches!(res, Err(SendError::Closed)),
      "Expected Err(SendError::Closed), got {:?}",
      res
    );
  }
}
