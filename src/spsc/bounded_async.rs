use super::bounded_sync::{BoundedSyncConsumer, BoundedSyncProducer, SpscShared};
use crate::error::{RecvError, SendError, TryRecvError, TrySendError};

use std::future::Future;
use std::marker::PhantomData;
use std::mem;
use std::pin::Pin;
use std::sync::atomic::{self, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};

// --- Async Producer ---
#[derive(Debug)]
pub struct AsyncBoundedSpscProducer<T> {
  pub(crate) shared: Arc<SpscShared<T>>,
  _phantom: PhantomData<T>, // T is not directly owned but logically associated
}

// --- Async Consumer ---
#[derive(Debug)]
pub struct AsyncBoundedSpscConsumer<T> {
  pub(crate) shared: Arc<SpscShared<T>>,
  _phantom: PhantomData<T>, // T is not directly owned but logically associated
}

unsafe impl<T: Send> Send for AsyncBoundedSpscProducer<T> {}
unsafe impl<T: Send> Send for AsyncBoundedSpscConsumer<T> {}
// Note: SPSC producer/consumer are typically !Sync because only one thread uses each end.
// PhantomData<*mut ()> could enforce this, but the single-threaded usage is by convention.
// If methods take `&self` and `T` is `Send`, then `Arc<SpscShared<T>>` being `Sync` is key.

impl<T> AsyncBoundedSpscProducer<T> {
  /// Crate-internal constructor for tests or other channel types.
  pub(crate) fn from_shared(shared: Arc<SpscShared<T>>) -> Self {
    Self {
      shared,
      _phantom: PhantomData,
    }
  }

  /// Converts this asynchronous SPSC producer into a synchronous one.
  pub fn to_sync(self) -> BoundedSyncProducer<T> {
    let shared = unsafe { std::ptr::read(&self.shared) };
    mem::forget(self);
    BoundedSyncProducer {
      shared,
      _phantom: PhantomData,
    }
  }
}
impl<T> AsyncBoundedSpscConsumer<T> {
  /// Crate-internal constructor for tests or other channel types.
  pub(crate) fn from_shared(shared: Arc<SpscShared<T>>) -> Self {
    Self {
      shared,
      _phantom: PhantomData,
    }
  }

  /// Converts this asynchronous SPSC consumer into a synchronous one.
  pub fn to_sync(self) -> BoundedSyncConsumer<T> {
    let shared = unsafe { std::ptr::read(&self.shared) };
    mem::forget(self);
    BoundedSyncConsumer {
      shared,
      _phantom: PhantomData,
    }
  }
}

/// Creates a new asynchronous bounded SPSC channel with the given capacity.
///
/// `capacity` must be greater than 0. Panics if capacity is 0.
pub fn bounded_async<T: Send>(
  capacity: usize,
) -> (AsyncBoundedSpscProducer<T>, AsyncBoundedSpscConsumer<T>) {
  let shared_core = SpscShared::new_internal(capacity);
  let shared_arc = Arc::new(shared_core);

  (
    AsyncBoundedSpscProducer {
      shared: Arc::clone(&shared_arc),
      _phantom: PhantomData,
    },
    AsyncBoundedSpscConsumer {
      shared: shared_arc,
      _phantom: PhantomData,
    },
  )
}

// --- Async Producer Methods & SendFuture ---
impl<T: Send> AsyncBoundedSpscProducer<T> {
  /// Sends an item into the channel asynchronously.
  ///
  /// The returned future will complete with `Ok(())` if the send was successful,
  /// or `Err(SendError::Closed)` if the consumer has been dropped.
  pub fn send(&self, item: T) -> SendFuture<'_, T> {
    SendFuture::new(&self.shared, item)
  }

  /// Attempts to send an item into the channel without blocking (asynchronously).
  ///
  /// This is a non-blocking operation that returns immediately.
  ///
  /// # Errors
  ///
  /// - `Err(TrySendError::Full(item))` if the channel is full.
  /// - `Err(TrySendError::Closed(item))` if the consumer has been dropped.
  pub fn try_send(&self, item: T) -> Result<(), TrySendError<T>> {
    let shared = &self.shared;
    if shared.consumer_dropped.load(Ordering::Acquire) {
      return Err(TrySendError::Closed(item));
    }
    let head = shared.head.load(Ordering::Relaxed);
    let tail = shared.tail.load(Ordering::Acquire);
    if shared.is_full(head, tail) {
      return Err(TrySendError::Full(item));
    }
    let slot_idx = head % shared.capacity;
    unsafe {
      (*shared.buffer[slot_idx].get()).write(item);
    }
    shared.head.store(head.wrapping_add(1), Ordering::Release);
    shared.wake_consumer();
    Ok(())
  }
}

impl<T> Drop for AsyncBoundedSpscProducer<T> {
  fn drop(&mut self) {
    self.shared.producer_dropped.store(true, Ordering::Release);
    atomic::fence(Ordering::SeqCst);
    self.shared.wake_consumer();
  }
}

#[must_use = "futures do nothing unless you .await or poll them"]
pub struct SendFuture<'a, T> {
  shared: &'a Arc<SpscShared<T>>,
  item: Option<T>, // Item to be sent, consumed by poll
}

impl<'a, T> SendFuture<'a, T> {
  fn new(shared: &'a Arc<SpscShared<T>>, item: T) -> Self {
    SendFuture {
      shared,
      item: Some(item),
    }
  }
}

impl<'a, T: Unpin + Send> Future for SendFuture<'a, T> {
  // Added Send bound for T
  type Output = Result<(), SendError>;

  fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    loop {
      let s = &mut *self;
      let shared = &*s.shared;

      if s.item.is_none() {
        return Poll::Ready(Ok(()));
      }

      if shared.consumer_dropped.load(Ordering::Acquire) {
        s.item = None;
        return Poll::Ready(Err(SendError::Closed));
      }

      let head = shared.head.load(Ordering::Relaxed);
      let tail = shared.tail.load(Ordering::Acquire);

      if !shared.is_full(head, tail) {
        let item_to_send = s.item.take().unwrap();
        let slot_idx = head % shared.capacity;
        unsafe {
          (*shared.buffer[slot_idx].get()).write(item_to_send);
        }
        shared.head.store(head.wrapping_add(1), Ordering::Release);
        shared.wake_consumer();
        return Poll::Ready(Ok(()));
      }

      shared.producer_waker_async.register(cx.waker());

      // Re-check after registration
      if shared.consumer_dropped.load(Ordering::Acquire) {
        continue;
      }
      let head_after_register = shared.head.load(Ordering::Relaxed);
      let tail_after_register = shared.tail.load(Ordering::Acquire);
      if !shared.is_full(head_after_register, tail_after_register) {
        continue;
      }

      return Poll::Pending;
    }
  }
}

impl<'a, T> Drop for SendFuture<'a, T> {
  fn drop(&mut self) {
    // If self.item is Some, it means the send did not complete.
    // The item is dropped here when Option<T> is dropped.
  }
}

// --- Async Consumer Methods & ReceiveFuture ---
impl<T: Send> AsyncBoundedSpscConsumer<T> {
  // Added Send bound for T
  /// Receives an item from the channel asynchronously.
  ///
  /// The returned future will complete with `Ok(T)` when an item is received,
  /// or `Err(RecvError::Disconnected)` if the producer has been dropped and
  /// the channel is empty.
  pub fn recv(&self) -> ReceiveFuture<'_, T> {
    ReceiveFuture::new(&self.shared)
  }

  /// Attempts to receive an item from the channel without blocking (asynchronously).
  ///
  /// This is a non-blocking operation that returns immediately.
  ///
  /// # Errors
  ///
  /// - `Ok(T)` if an item was successfully received.
  /// - `Err(TryRecvError::Empty)` if the channel is currently empty but the producer is alive.
  /// - `Err(TryRecvError::Disconnected)` if the producer has been dropped and the channel is empty.
  pub fn try_recv(&self) -> Result<T, TryRecvError> {
    let shared = &self.shared;
    let tail = shared.tail.load(Ordering::Relaxed);
    let head = shared.head.load(Ordering::Acquire);

    if shared.is_empty(head, tail) {
      if shared.producer_dropped.load(Ordering::Acquire) {
        // Re-read head after confirming producer_dropped, to ensure we see any item
        // sent *just before* the producer_dropped flag was set.
        let final_head = shared.head.load(Ordering::Acquire);
        if final_head == tail {
          // Still empty after re-read
          return Err(TryRecvError::Disconnected);
        }
        // An item became visible after producer_dropped check.
        // Proceed to read it using final_head.
        let slot_idx = tail % shared.capacity;
        let item = unsafe { (*shared.buffer[slot_idx].get()).assume_init_read() };
        shared.tail.store(tail.wrapping_add(1), Ordering::Release);
        shared.wake_producer(); // Producer is dropped, but this is harmless general pattern
        return Ok(item);
      } else {
        return Err(TryRecvError::Empty);
      }
    }

    let slot_idx = tail % shared.capacity;
    let item = unsafe { (*shared.buffer[slot_idx].get()).assume_init_read() };
    shared.tail.store(tail.wrapping_add(1), Ordering::Release);
    shared.wake_producer();
    Ok(item)
  }
}

impl<T> Drop for AsyncBoundedSpscConsumer<T> {
  fn drop(&mut self) {
    self.shared.consumer_dropped.store(true, Ordering::Release);
    atomic::fence(Ordering::SeqCst);
    self.shared.wake_producer();
  }
}

#[must_use = "futures do nothing unless you .await or poll them"]
pub struct ReceiveFuture<'a, T> {
  shared: &'a Arc<SpscShared<T>>,
}

impl<'a, T> ReceiveFuture<'a, T> {
  fn new(shared: &'a Arc<SpscShared<T>>) -> Self {
    ReceiveFuture { shared }
  }
}

impl<'a, T: Unpin + Send> Future for ReceiveFuture<'a, T> {
  // Added Send bound for T
  type Output = Result<T, RecvError>;

  fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    let shared = &*self.shared;

    loop {
      let tail = shared.tail.load(Ordering::Relaxed);
      let head = shared.head.load(Ordering::Acquire);

      if !shared.is_empty(head, tail) {
        let slot_idx = tail % shared.capacity;
        let item = unsafe { (*shared.buffer[slot_idx].get()).assume_init_read() };
        shared.tail.store(tail.wrapping_add(1), Ordering::Release);
        shared.wake_producer();
        return Poll::Ready(Ok(item));
      }

      if shared.producer_dropped.load(Ordering::Acquire) {
        // Re-read head after producer_dropped.
        // This is crucial: if an item was sent just before producer_dropped became true,
        // and we haven't seen that head update yet, we might incorrectly return Disconnected.
        let final_head = shared.head.load(Ordering::Acquire);
        if final_head == tail {
          // Still empty after re-read head
          return Poll::Ready(Err(RecvError::Disconnected));
        }
        // Item is available (final_head > tail), loop to pick it up.
        // The waker registration below is for the *next* item if this one is consumed.
        // But since an item became available, we should re-loop to consume it.
        // This path (producer_dropped is true, but item appears on re-check)
        // means we should prioritize consuming the item.
        continue;
      }

      shared.consumer_waker_async.register(cx.waker());

      // Re-check after registration
      let head_after_register = shared.head.load(Ordering::Acquire);
      if !shared.is_empty(head_after_register, tail)
        || shared.producer_dropped.load(Ordering::Acquire)
      {
        continue;
      }

      return Poll::Pending;
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::time::Duration;
  use tokio::time::timeout;

  const TEST_TIMEOUT: Duration = Duration::from_secs(2);

  // Helper to create SpscShared for tests that mix P/C types
  fn create_test_shared_core<T: Send>(capacity: usize) -> Arc<SpscShared<T>> {
    // T: Send
    SpscShared::new_internal(capacity).into()
  }

  #[tokio::test]
  async fn create_async_channel() {
    let (p, c) = bounded_async::<i32>(5);
    drop(p);
    drop(c);
  }

  #[tokio::test]
  async fn async_send_recv_single_item() {
    let (p, c) = bounded_async(1);
    p.send(42i32).await.unwrap();
    assert_eq!(c.recv().await.unwrap(), 42);
  }

  #[tokio::test]
  async fn async_try_send_full_try_recv_empty() {
    let (p, c) = bounded_async::<i32>(1);
    p.try_send(10).unwrap();
    match p.try_send(20) {
      Err(TrySendError::Full(val)) => assert_eq!(val, 20),
      res => panic!("Expected Full error, got {:?}", res),
    }

    assert_eq!(c.try_recv().unwrap(), 10);
    match c.try_recv() {
      Err(TryRecvError::Empty) => {}
      res => panic!("Expected Empty error, got {:?}", res),
    }
  }

  #[tokio::test]
  async fn async_send_blocks_then_completes() {
    let (p, mut c) = bounded_async::<i32>(1);

    p.send(1).await.unwrap(); // Fill the channel

    let send_task = tokio::spawn(async move {
      println!("[ASYNC_SEND_BLOCKS] Send Task: Sending 2...");
      p.send(2).await.unwrap();
      println!("[ASYNC_SEND_BLOCKS] Send Task: Sent 2.");
    });

    tokio::time::sleep(Duration::from_millis(50)).await; // Give send_task time to block

    println!("[ASYNC_SEND_BLOCKS] Main Task: Receiving 1...");
    assert_eq!(c.recv().await.unwrap(), 1); // Unblock producer
    println!("[ASYNC_SEND_BLOCKS] Main Task: Received 1.");

    match timeout(TEST_TIMEOUT, send_task).await {
      Ok(Ok(())) => println!("[ASYNC_SEND_BLOCKS] Main Task: Send task completed."),
      Ok(Err(e)) => panic!("[ASYNC_SEND_BLOCKS] Send task panicked: {:?}", e),
      Err(_) => panic!("[ASYNC_SEND_BLOCKS] Send task timed out"),
    }

    println!("[ASYNC_SEND_BLOCKS] Main Task: Receiving 2...");
    assert_eq!(c.recv().await.unwrap(), 2);
    println!("[ASYNC_SEND_BLOCKS] Main Task: Received 2.");
  }

  #[tokio::test]
  async fn async_recv_blocks_then_completes() {
    let (p, mut c) = bounded_async::<i32>(1);

    let recv_task = tokio::spawn(async move {
      println!("[ASYNC_RECV_BLOCKS] Recv Task: Receiving...");
      let val = c.recv().await.unwrap();
      println!("[ASYNC_RECV_BLOCKS] Recv Task: Received {}.", val);
      assert_eq!(val, 100);
    });

    tokio::time::sleep(Duration::from_millis(50)).await; // Give recv_task time to block

    println!("[ASYNC_RECV_BLOCKS] Main Task: Sending 100...");
    p.send(100).await.unwrap();
    println!("[ASYNC_RECV_BLOCKS] Main Task: Sent 100.");

    match timeout(TEST_TIMEOUT, recv_task).await {
      Ok(Ok(())) => println!("[ASYNC_RECV_BLOCKS] Main Task: Recv task completed."),
      Ok(Err(e)) => panic!("[ASYNC_RECV_BLOCKS] Recv task panicked: {:?}", e),
      Err(_) => panic!("[ASYNC_RECV_BLOCKS] Recv task timed out"),
    }
  }

  #[tokio::test]
  async fn async_producer_drop_signals_consumer() {
    let (p, mut c) = bounded_async::<i32>(1);
    p.send(10).await.unwrap(); // Send an item first
    drop(p);
    assert_eq!(c.recv().await.unwrap(), 10); // Consumer gets the item
    match c.recv().await {
      // Then gets disconnected
      Err(RecvError::Disconnected) => {}
      Ok(v) => panic!("[DROP_P] Expected Disconnected, got Ok({:?})", v),
    }
  }

  #[tokio::test]
  async fn async_producer_drop_empty_signals_consumer() {
    let (p, mut c) = bounded_async::<i32>(1);
    drop(p);
    match c.recv().await {
      Err(RecvError::Disconnected) => {}
      Ok(v) => panic!("[DROP_P_EMPTY] Expected Disconnected, got Ok({:?})", v),
    }
  }

  #[tokio::test]
  async fn async_consumer_drop_signals_producer() {
    let (p, c) = bounded_async::<i32>(1);
    drop(c);
    match p.send(1).await {
      Err(SendError::Closed) => {}
      Ok(()) => panic!("[DROP_C] Expected Closed error"),
      Err(SendError::Sent) => panic!("[DROP_C] SPSC send should not error with Sent"),
    }
  }

  #[tokio::test]
  async fn async_select_recv_preference() {
    let (p1, mut c1) = bounded_async::<i32>(1);
    let (_p2, mut c2) = bounded_async::<i32>(1); // This consumer will never receive

    p1.send(10).await.unwrap();

    tokio::select! {
        biased; // Ensures c1 is polled first if both are ready (though only c1 is)
        Ok(val) = c1.recv() => {
            assert_eq!(val, 10);
        }
        Ok(_val) = c2.recv() => {
            panic!("[SELECT_RECV] Should not have received from empty c2");
        }
        else => { // This branch handles the case where all futures complete with errors or are exhausted
            // For recv(), an Err(Disconnected) is a valid completion.
            // If c1 completed with Ok, this 'else' isn't hit for c1.
            // If c2 completed (e.g. Disconnected), it would be caught by its pattern.
            // This 'else' would be hit if, for instance, both channels were dropped
            // and recv() returned Disconnected for both.
            // Or if more complex futures were used that didn't match Ok(val).
            // For this specific test, if c1.recv() is Ok, this branch is not taken.
            // If c1.recv() was Err, then it might fall here if c2.recv() also Err or still pending.
            // To be robust, one might check c1's outcome before panicking.
            // However, with `biased` and p1.send(), c1.recv() should yield Ok.
        }
    }
  }

  #[tokio::test]
  async fn async_select_send_blocks_other_completes() {
    let (p_full, _c_full) = bounded_async::<i32>(1);
    let (p_can_send, mut c_can_send) = bounded_async::<i32>(1);

    p_full.send(1).await.unwrap(); // Fill this channel

    let recv_task = tokio::spawn(async move {
      tokio::time::sleep(Duration::from_millis(100)).await;
      assert_eq!(c_can_send.recv().await.unwrap(), 200);
    });

    tokio::select! {
        biased;
        res_full = p_full.send(100) => {
            panic!("[SELECT_SEND] Send to full channel should not have completed immediately, got {:?}", res_full);
        }
        res_can_send = p_can_send.send(200) => {
            assert!(res_can_send.is_ok(), "[SELECT_SEND] Send to available channel failed");
        }
    }
    recv_task.await.unwrap();
  }

  // --- Mixed Sync/Async Tests ---
  #[tokio::test]
  async fn sync_producer_async_consumer() {
    const CAPACITY: usize = 2;
    let core_shared = create_test_shared_core::<String>(CAPACITY);

    let mut sync_p = BoundedSyncProducer::from_shared(core_shared.clone());
    let mut async_c = AsyncBoundedSpscConsumer::from_shared(core_shared);

    let val1 = "hello from sync".to_string();
    let val2 = "world from sync".to_string();

    println!("[MIXED_S2A] SyncP sending: {}", val1);
    sync_p.send(val1.clone()).unwrap();
    println!("[MIXED_S2A] SyncP sent: {}", val1);

    println!("[MIXED_S2A] AsyncC awaiting val1...");
    assert_eq!(async_c.recv().await.unwrap(), val1);
    println!("[MIXED_S2A] AsyncC received val1.");

    // Use tokio::task::spawn_blocking for the sync send part from an async context
    let send_task_val2 = val2.clone(); // Clone for the task
    let send_task = tokio::task::spawn_blocking(move || {
      println!("[MIXED_S2A_TASK] SyncP sending: {}", send_task_val2);
      sync_p.send(send_task_val2.clone()).unwrap(); // sync_p moved
      println!("[MIXED_S2A_TASK] SyncP sent: {}", send_task_val2);
      send_task_val2 // Return the value for assertion
    });

    println!("[MIXED_S2A] AsyncC awaiting val2...");
    assert_eq!(async_c.recv().await.unwrap(), send_task.await.unwrap());
    println!("[MIXED_S2A] AsyncC received val2.");
  }

  #[test] // This test uses std::thread for consumer, tokio runtime for producer
  fn async_producer_sync_consumer() {
    const CAPACITY: usize = 2;
    let rt = tokio::runtime::Builder::new_multi_thread() // Use a multi-thread runtime
      .worker_threads(1) // One worker thread is sufficient for the producer
      .enable_all()
      .build()
      .unwrap();
    let core_shared = create_test_shared_core::<String>(CAPACITY);

    let async_p = AsyncBoundedSpscProducer::from_shared(core_shared.clone());
    let mut sync_c = BoundedSyncConsumer::from_shared(core_shared);

    let val1_original = "hello from async".to_string();
    let val2_original = "world from async".to_string();

    // Clone values that will be moved into the async task, so originals can be used for assertion
    let val1_for_async_task = val1_original.clone();
    let val2_for_async_task = val2_original.clone();

    let producer_handle = rt.spawn(async move {
      // async_p is moved into this task
      println!("[MIXED_A2S_TASK] AsyncP sending val1...");
      // val1_for_async_task is moved here
      async_p.send(val1_for_async_task).await.unwrap();
      println!("[MIXED_A2S_TASK] AsyncP sent val1.");

      tokio::time::sleep(Duration::from_millis(50)).await;

      println!("[MIXED_A2S_TASK] AsyncP sending val2...");
      // val2_for_async_task is moved here
      async_p.send(val2_for_async_task).await.unwrap();
      println!("[MIXED_A2S_TASK] AsyncP sent val2.");
      // async_p drops when this task finishes
    });

    println!("[MIXED_A2S] SyncC receiving val1...");
    assert_eq!(sync_c.recv().unwrap(), val1_original); // Use the original for assertion
    println!("[MIXED_A2S] SyncC received val1.");

    println!("[MIXED_A2S] SyncC receiving val2...");
    assert_eq!(sync_c.recv().unwrap(), val2_original); // Use the original for assertion
    println!("[MIXED_A2S] SyncC received val2.");

    // Wait for the producer task to complete.
    // rt.block_on will execute the producer_handle future to completion.
    // This is fine since the producer is on a worker thread and sync_c runs on the main thread.
    rt.block_on(async { producer_handle.await.unwrap() });
  }

  // Test to ensure try_recv correctly returns Disconnected
  #[tokio::test]
  async fn async_try_recv_disconnected() {
    let (p, mut c) = bounded_async::<i32>(1);
    p.try_send(1).unwrap();
    assert_eq!(c.try_recv().unwrap(), 1); // Consume the item

    drop(p); // Producer is dropped

    // Now try_recv should report Disconnected because channel is empty and producer gone
    assert_eq!(c.try_recv(), Err(TryRecvError::Disconnected));
  }

  // Test to ensure recv future correctly resolves to Disconnected
  #[tokio::test]
  async fn async_recv_future_disconnected_after_item() {
    let (p, mut c) = bounded_async::<i32>(1);
    p.send(1).await.unwrap();
    assert_eq!(c.recv().await.unwrap(), 1); // Consume the item

    drop(p); // Producer is dropped

    // Now recv future should resolve to Disconnected
    assert_eq!(c.recv().await, Err(RecvError::Disconnected));
  }
}
