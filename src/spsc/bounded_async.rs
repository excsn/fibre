// src/spsc/bounded_async.rs

use super::bounded_sync::{BoundedSyncConsumer, BoundedSyncProducer, SpscShared}; // SpscShared is pub
use crate::async_util::AtomicWaker; // Re-exported from futures-util
use crate::error::{RecvError, RecvErrorTimeout, SendError, TryRecvError, TrySendError};

use crate::internal::cache_padded::CachePadded;
use std::cell::UnsafeCell; // For SpscShared::new_internal (if called from here)
use std::future::Future;
use std::marker::PhantomData;
use std::mem::MaybeUninit; // For SpscShared::new_internal
use std::pin::Pin;
use std::sync::atomic::{self, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll}; // Removed Waker, Context provides it
use std::time::Duration; // For SpscShared::new_internal

// --- Async Producer ---
#[derive(Debug)]
pub struct AsyncBoundedSpscProducer<T> {
  pub(crate) shared: Arc<SpscShared<T>>,
  _phantom: PhantomData<T>,
}

// --- Async Consumer ---
#[derive(Debug)]
pub struct AsyncBoundedSpscConsumer<T> {
  pub(crate) shared: Arc<SpscShared<T>>,
  _phantom: PhantomData<T>,
}

unsafe impl<T: Send> Send for AsyncBoundedSpscProducer<T> {}
unsafe impl<T: Send> Send for AsyncBoundedSpscConsumer<T> {}

impl<T> AsyncBoundedSpscProducer<T> {
  /// Crate-internal constructor for tests or other channel types.
  pub(crate) fn from_shared(shared: Arc<SpscShared<T>>) -> Self {
    Self {
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
}

/// Creates a new asynchronous bounded SPSC channel with the given capacity.
///
/// `capacity` must be greater than 0. Panics if capacity is 0.
pub fn bounded_async<T>(capacity: usize) -> (AsyncBoundedSpscProducer<T>, AsyncBoundedSpscConsumer<T>) {
  // Use the pub(crate) constructor from SpscShared, which is now defined in bounded_sync.rs
  // SpscShared::new_internal is pub(crate)
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
impl<T> AsyncBoundedSpscProducer<T> {
  /// Sends an item into the channel asynchronously.
  pub fn send(&self, item: T) -> SendFuture<'_, T> {
    SendFuture::new(&self.shared, item)
  }

  /// Attempts to send an item into the channel without blocking (asynchronously).
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
    // Ensure the store to producer_dropped is globally visible
    // before the wake can cause another task to read it.
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

impl<'a, T: Unpin> Future for SendFuture<'a, T> {
  type Output = Result<(), SendError>;

  fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    // Loop to handle state changes after waker registration
    loop {
      let s = &mut *self; // Get &mut SendFuture from Pin<&mut Self>
      let shared = &*s.shared;

      if s.item.is_none() {
        // Already completed or item invalidly taken
        return Poll::Ready(Ok(()));
      }

      // Check for disconnection first
      if shared.consumer_dropped.load(Ordering::Acquire) {
        s.item = None; // Item is "consumed" by the error state
        return Poll::Ready(Err(SendError::Closed));
      }

      // Attempt to send
      let head = shared.head.load(Ordering::Relaxed);
      let tail = shared.tail.load(Ordering::Acquire);

      if !shared.is_full(head, tail) {
        let item_to_send = s.item.take().unwrap(); // Consume item now
        let slot_idx = head % shared.capacity;
        unsafe {
          (*shared.buffer[slot_idx].get()).write(item_to_send);
        }
        shared.head.store(head.wrapping_add(1), Ordering::Release);
        shared.wake_consumer();
        return Poll::Ready(Ok(()));
      }

      // Channel full, consumer alive. Register waker.
      shared.producer_waker_async.register(cx.waker());

      // Re-check after registration
      if shared.consumer_dropped.load(Ordering::Acquire) {
        // Consumer dropped while registering. Loop to handle Closed state.
        continue;
      }
      let head_after_register = shared.head.load(Ordering::Relaxed);
      let tail_after_register = shared.tail.load(Ordering::Acquire);
      if !shared.is_full(head_after_register, tail_after_register) {
        // Space appeared while registering. Loop to attempt send.
        continue;
      }

      // Still full, consumer alive, waker registered. Item remains in s.item.
      return Poll::Pending;
    }
  }
}

impl<'a, T> Drop for SendFuture<'a, T> {
  fn drop(&mut self) {
    // If self.item is Some, it means the send did not complete.
    // The item will be dropped automatically when `Option<T>` is dropped.
    // If the waker was registered for this specific future instance and needs
    // explicit cleanup beyond what AtomicWaker provides (e.g. if it's part of a list),
    // it would be done here. For SPSC with a single producer_waker_async,
    // overwriting by a new SendFuture is usually fine.
  }
}

// --- Async Consumer Methods & ReceiveFuture ---
impl<T> AsyncBoundedSpscConsumer<T> {
  pub fn recv(&self) -> ReceiveFuture<'_, T> {
    ReceiveFuture::new(&self.shared)
  }

  pub fn try_recv(&self) -> Result<T, TryRecvError> {
    let shared = &self.shared;
    let tail = shared.tail.load(Ordering::Relaxed); // Load tail first for consumer
    let head = shared.head.load(Ordering::Acquire);

    if shared.is_empty(head, tail) {
      if shared.producer_dropped.load(Ordering::Acquire) {
        let final_head = shared.head.load(Ordering::Acquire);
        if final_head == tail {
          return Err(TryRecvError::Disconnected);
        }
        // An item was sent just before producer dropped. Fall through to read it.
        // Update head to final_head for the read logic.
        let head_for_read = final_head; // Use this head
        let slot_idx = tail % shared.capacity; // tail is correct
        let item = unsafe { (*shared.buffer[slot_idx].get()).assume_init_read() };
        shared.tail.store(tail.wrapping_add(1), Ordering::Release);
        shared.wake_producer();
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
    // Ensure the store to consumer_dropped is globally visible
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

impl<'a, T: Unpin> Future for ReceiveFuture<'a, T> {
  type Output = Result<T, RecvError>;

  fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    let shared = &*self.shared;

    loop {
      // Loop to re-evaluate state if a waker registration happened but state changed
      let tail = shared.tail.load(Ordering::Relaxed);
      let head = shared.head.load(Ordering::Acquire);

      if !shared.is_empty(head, tail) {
        // Path 1: Item available
        let slot_idx = tail % shared.capacity;
        let item = unsafe { (*shared.buffer[slot_idx].get()).assume_init_read() };
        shared.tail.store(tail.wrapping_add(1), Ordering::Release);
        shared.wake_producer();
        return Poll::Ready(Ok(item));
      }

      // Path 2: Channel is empty. Check producer status.
      if shared.producer_dropped.load(Ordering::Acquire) {
        // Producer is gone, and we already established it's empty.
        return Poll::Ready(Err(RecvError::Disconnected));
      }

      // Path 3: Channel is empty, producer alive. Try to register and park.
      shared.consumer_waker_async.register(cx.waker());

      // Path 3 Re-check: Critical. Did state change *during/after* registration?
      let head_after_register = shared.head.load(Ordering::Acquire);
      // Also re-check producer_dropped, as that's a terminal condition for recv.
      if !shared.is_empty(head_after_register, tail) || shared.producer_dropped.load(Ordering::Acquire) {
        // State changed (item appeared or producer dropped).
        // Don't return Pending with the old waker. Loop to re-evaluate from top.
        // The waker we just registered is fine; AtomicWaker handles overwrites/staleness.
        // By looping, we immediately re-evaluate. If an item is there, we take it.
        // If producer dropped, we error. We don't need cx.waker().wake_by_ref() here
        // because we are immediately re-processing.
        continue; // Re-evaluate state from the beginning of the poll
      }

      // Path 3c: Still empty, producer alive, waker registered correctly for this state.
      return Poll::Pending;
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::internal::cache_padded::CachePadded;
  use crate::spsc::bounded_sync;
  use std::cell::UnsafeCell; // For SpscShared field access in test helper
  use std::mem::MaybeUninit; // For SpscShared field access in test helper
  use std::sync::atomic::AtomicBool; // For SpscShared field access in test helper
  use std::thread;
  use std::time::Duration;
  use tokio::time::timeout; // For SpscShared field access in test helper

  const TEST_TIMEOUT: Duration = Duration::from_secs(2); // Increased timeout slightly for CI

  // Helper to create SpscShared for tests that mix P/C types
  // This ensures that producer_dropped and consumer_dropped start as false.
  fn create_test_shared_core<T>(capacity: usize) -> Arc<SpscShared<T>> {
    SpscShared::new_internal(capacity).into() // Use the new_internal and wrap in Arc
  }

  #[tokio::test]
  async fn create_async_channel() {
    let (p, c) = bounded_async::<i32>(5);
    drop(p);
    drop(c);
  }

  #[tokio::test]
  async fn async_send_recv_single_item() {
    let (p, mut c) = bounded_async(1);
    p.send(42i32).await.unwrap();
    assert_eq!(c.recv().await.unwrap(), 42);
  }

  #[tokio::test]
  async fn async_try_send_full_try_recv_empty() {
    let (p, mut c) = bounded_async::<i32>(1);
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

    p.send(1).await.unwrap();

    let send_task = tokio::spawn(async move {
      // p is moved here
      println!("[ASYNC_SEND_BLOCKS] Send Task: Sending 2...");
      p.send(2).await.unwrap();
      println!("[ASYNC_SEND_BLOCKS] Send Task: Sent 2.");
      // p is dropped when task finishes
    });

    tokio::time::sleep(Duration::from_millis(50)).await;

    println!("[ASYNC_SEND_BLOCKS] Main Task: Receiving 1...");
    assert_eq!(c.recv().await.unwrap(), 1);
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
      // c is dropped
    });

    tokio::time::sleep(Duration::from_millis(50)).await;

    println!("[ASYNC_RECV_BLOCKS] Main Task: Sending 100...");
    p.send(100).await.unwrap(); // p is moved if send is &mut self, not if &self.
                                // For current API (send(&self)), p is not moved.
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
    drop(p);
    match c.recv().await {
      Err(RecvError::Disconnected) => {}
      Ok(v) => panic!("[DROP_P] Expected Disconnected, got Ok({:?})", v),
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
    let (_p2, mut c2) = bounded_async::<i32>(1);

    p1.send(10).await.unwrap();

    tokio::select! {
        biased;
        Ok(val) = c1.recv() => {
            assert_eq!(val, 10);
        }
        Ok(_val) = c2.recv() => {
            panic!("[SELECT_RECV] Should not have received from empty c2");
        }
        else => {
            panic!("[SELECT_RECV] Select completed without receiving from c1");
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
        biased; // For test predictability
        res_full = p_full.send(100) => {
            panic!("[SELECT_SEND] Send to full channel should not have completed immediately, got {:?}", res_full);
        }
        res_can_send = p_can_send.send(200) => { // p_can_send is moved here
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

    let send_task = tokio::task::spawn_blocking(move || {
      println!("[MIXED_S2A_TASK] SyncP sending: {}", val2);
      sync_p.send(val2.clone()).unwrap();
      println!("[MIXED_S2A_TASK] SyncP sent: {}", val2);
      val2
    });

    println!("[MIXED_S2A] AsyncC awaiting val2...");
    assert_eq!(async_c.recv().await.unwrap(), send_task.await.unwrap());
    println!("[MIXED_S2A] AsyncC received val2.");
  }

  #[test] // This test uses std::thread for consumer, tokio runtime for producer
  fn async_producer_sync_consumer() {
    const CAPACITY: usize = 2;
    let rt = tokio::runtime::Builder::new_current_thread()
      .enable_all()
      .build()
      .unwrap();
    let core_shared = create_test_shared_core::<String>(CAPACITY);

    let mut async_p = AsyncBoundedSpscProducer::from_shared(core_shared.clone());
    let mut sync_c = BoundedSyncConsumer::from_shared(core_shared);

    let val1 = "hello from async".to_string();
    let val2 = "world from async".to_string();

    println!("[MIXED_A2S] AsyncP sending val1 (blocking on runtime)...");
    rt.block_on(async_p.send(val1.clone())).unwrap();
    println!("[MIXED_A2S] AsyncP sent val1.");

    println!("[MIXED_A2S] SyncC receiving val1...");
    assert_eq!(sync_c.recv().unwrap(), val1);
    println!("[MIXED_A2S] SyncC received val1.");

    let async_send_handle = rt.spawn(async move {
      // async_p is moved here
      println!("[MIXED_A2S_TASK] AsyncP sending val2...");
      async_p.send(val2.clone()).await.unwrap();
      println!("[MIXED_A2S_TASK] AsyncP sent val2.");
      val2
    });

    println!("[MIXED_A2S] SyncC receiving val2...");
    let sent_val2 = rt.block_on(async_send_handle).unwrap();
    assert_eq!(sync_c.recv().unwrap(), sent_val2);
    println!("[MIXED_A2S] SyncC received val2.");
  }
}
