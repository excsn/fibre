//! A single-producer, multi-consumer (SPMC) "broadcast" channel.
//!
//! This channel is optimized for the "fan-out" or "publish-subscribe" pattern,
//! where one producer sends messages to many consumers. Every consumer that is
//! subscribed at the time of sending will see every message.
//!
//! ## Behavior
//!
//! - **Guaranteed Delivery**: This implementation guarantees that a message sent by the
//!   producer will be delivered to all active consumers. It does not drop messages.
//! - **Blocking BoundedSyncSender**: If any consumer is slow and the channel's buffer fills up
//!   from that consumer's perspective, the producer will block until the slow
//!   consumer catches up. This backpressure mechanism ensures no messages are lost.
//! - **`T: Clone` Requirement**: Since every message is delivered to every consumer, the
//!   message type `T` must implement the `Clone` trait.
//! - **Mixed Paradigms**: The channel supports full interoperability between synchronous
//!   and asynchronous code. You can have a synchronous `BoundedSyncSender` sending to an
//!   `BoundedAsyncReceiver`, or any other combination, by using the provided `to_sync()` and
//!   `to_async()` conversion methods.
//!
//! ## When to use SPMC
//!
//! - When you need to broadcast events or state updates to multiple listeners.
//! - For implementing a "fan-out" worker pattern where a central task distributes
//!   identical jobs to a pool of workers.
//! - In any situation where a single source of data needs to be multiplexed to
//!   multiple destinations without data loss.
//!
//! # Examples
//!
//! ### Basic Synchronous Usage
//!
//! ```
//! use fibre::spmc;
//! use std::thread;
//!
//! // Create a channel with a buffer capacity of 2.
//! let (mut producer, mut receiver1) = spmc::bounded(2);
//! let mut receiver2 = receiver1.clone();
//!
//! assert_eq!(producer.len(), 0);
//! assert!(producer.is_empty());
//! assert!(!producer.is_full());
//! assert_eq!(receiver1.len(), 0);
//! assert!(receiver1.is_empty());
//!
//! // The producer sends messages.
//! producer.send("hello".to_string()).unwrap();
//! assert_eq!(producer.len(), 1);
//! assert_eq!(receiver1.len(), 1);
//! assert_eq!(receiver2.len(), 1);
//!
//! producer.send("world".to_string()).unwrap();
//! assert_eq!(producer.len(), 2); // Producer sees length based on slowest consumer
//! assert!(producer.is_full());    // Capacity is 2
//! assert_eq!(receiver1.len(), 2);
//! assert_eq!(receiver2.len(), 2);
//!
//! // Both receivers get a copy of every message.
//! let handle1 = thread::spawn(move || {
//!     assert_eq!(receiver1.recv().unwrap(), "hello");
//!     assert_eq!(receiver1.recv().unwrap(), "world");
//!     assert!(receiver1.is_empty());
//! });
//!
//! let handle2 = thread::spawn(move || {
//!     assert_eq!(receiver2.recv().unwrap(), "hello");
//!     assert_eq!(receiver2.recv().unwrap(), "world");
//! });
//!
//! handle1.join().unwrap();
//! handle2.join().unwrap();
//! ```

mod ring_buffer;
#[cfg(feature = "topic")]
pub mod topic;

use std::mem;

use crate::internal::sync::{AtomicBool, Ordering};
use std::marker::PhantomData;

pub use crate::error::{
  BatchSendErrorReason, CloseError, RecvError, SendBatchError, SendError, TryRecvError,
  TrySendBatchError, TrySendError,
};
pub use ring_buffer::{
  BoundedAsyncReceiver, BoundedAsyncSender, BoundedSyncReceiver, RecvBatchFuture, RecvBatchMutFuture, RecvFuture,
  SendBatchFuture, SendBatchMutFuture, SendFuture, BoundedSyncSender,
};

// --- Constructors ---

/// Creates a new bounded synchronous SPMC channel.
///
/// `T` must be `Send + Clone` because each consumer gets its own copy of the message.
///
/// # Panics
///
/// Panics if `capacity` is 0.
pub fn bounded<T: Send + Clone>(capacity: usize) -> (BoundedSyncSender<T>, BoundedSyncReceiver<T>) {
  let (p, r) = ring_buffer::new_channel(capacity);
  (p, r)
}

/// Creates a new bounded asynchronous SPMC channel.
///
/// `T` must be `Send + Clone` because each consumer gets its own copy of the message.
///
/// # Panics
///
/// Panics if `capacity` is 0.
pub fn bounded_async<T: Send + Clone>(capacity: usize) -> (BoundedAsyncSender<T>, BoundedAsyncReceiver<T>) {
  let (p, r) = ring_buffer::new_channel(capacity);
  (p.to_async(), r.to_async())
}

// --- Conversion Methods ---

impl<T: Send + Clone> BoundedSyncSender<T> {
  /// Converts this synchronous `BoundedSyncSender` into an asynchronous `BoundedAsyncSender`.
  ///
  /// This is a zero-cost conversion that facilitates interoperability between
  /// synchronous and asynchronous code. The `Drop` implementation of the
  /// original `BoundedSyncSender` is not called.
  pub fn to_async(self) -> BoundedAsyncSender<T> {
    let shared = unsafe { std::ptr::read(&self.shared) };
    let closed = self.closed.load(Ordering::Relaxed);
    mem::forget(self);
    BoundedAsyncSender {
      shared,
      closed: AtomicBool::new(closed),
      _not_sync: PhantomData,
    }
  }
}

impl<T: Send + Clone> BoundedAsyncSender<T> {
  /// Converts this asynchronous `BoundedAsyncSender` into a synchronous `BoundedSyncSender`.
  ///
  /// This is a zero-cost conversion that facilitates interoperability between
  /// synchronous and asynchronous code. The `Drop` implementation of the
  /// original `BoundedAsyncSender` is not called.
  pub fn to_sync(self) -> BoundedSyncSender<T> {
    let shared = unsafe { std::ptr::read(&self.shared) };
    let closed = self.closed.load(Ordering::Relaxed);
    mem::forget(self);
    BoundedSyncSender {
      shared,
      closed: AtomicBool::new(closed),
      _not_sync: PhantomData,
    }
  }
}

impl<T: Send + Clone> BoundedSyncReceiver<T> {
  /// Converts this synchronous `BoundedSyncReceiver` into an asynchronous `BoundedAsyncReceiver`.
  ///
  /// This is a zero-cost conversion. The original `BoundedSyncReceiver`'s `Drop`
  /// implementation is not called.
  pub fn to_async(self) -> BoundedAsyncReceiver<T> {
    let shared = unsafe { std::ptr::read(&self.shared) };
    let tail = unsafe { std::ptr::read(&self.tail) };
    let closed = self.closed.load(Ordering::Relaxed);
    mem::forget(self);
    BoundedAsyncReceiver {
      shared,
      tail,
      closed: AtomicBool::new(closed),
      _not_sync: PhantomData,
    }
  }
}

impl<T: Send + Clone> BoundedAsyncReceiver<T> {
  /// Converts this asynchronous `BoundedAsyncReceiver` into a synchronous `BoundedSyncReceiver`.
  ///
  /// This is a zero-cost conversion. The original `BoundedAsyncReceiver`'s `Drop`
  /// implementation is not called.
  pub fn to_sync(self) -> BoundedSyncReceiver<T> {
    let shared = unsafe { std::ptr::read(&self.shared) };
    let tail = unsafe { std::ptr::read(&self.tail) };
    let closed = self.closed.load(Ordering::Relaxed);
    mem::forget(self);
    BoundedSyncReceiver {
      shared,
      tail,
      closed: AtomicBool::new(closed),
      _not_sync: PhantomData,
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::thread;
  use std::time::Duration;

  pub const SHORT_TIMEOUT: Duration = Duration::from_millis(500);
  pub const LONG_TIMEOUT: Duration = Duration::from_secs(3);
  pub const STRESS_TIMEOUT: Duration = Duration::from_secs(15);
  pub const ITEMS_LOW: usize = 50;
  pub const ITEMS_MEDIUM: usize = 200;
  pub const ITEMS_HIGH: usize = 1000;

  #[test]
  fn spmc_single_recv() {
    let (tx, rx) = bounded(2);
    assert_eq!(tx.len(), 0);
    assert!(tx.is_empty());
    assert!(!tx.is_full());
    assert_eq!(rx.len(), 0);
    assert!(rx.is_empty());
    assert!(!rx.is_full()); // A receiver is "full" if len == capacity

    tx.send(10).unwrap();
    assert_eq!(tx.len(), 1);
    assert!(!tx.is_empty());
    assert_eq!(rx.len(), 1);
    assert!(!rx.is_empty());

    assert_eq!(rx.recv().unwrap(), 10);
    assert_eq!(tx.len(), 0);
    assert!(tx.is_empty());
    assert_eq!(rx.len(), 0);
    assert!(rx.is_empty());
  }

  #[test]
  fn spmc_multiple_receivers_len_checks() {
    let (tx, rx1) = bounded(4);
    let rx2 = rx1.clone();

    assert_eq!(tx.len(), 0);
    assert!(tx.is_empty());
    assert_eq!(rx1.len(), 0);
    assert!(rx1.is_empty());
    assert_eq!(rx2.len(), 0);
    assert!(rx2.is_empty());

    tx.send(1).unwrap(); // head = 1, min_tail = 0, tx.len = 1
    assert_eq!(tx.len(), 1);
    assert_eq!(rx1.len(), 1);
    assert_eq!(rx2.len(), 1);

    tx.send(2).unwrap(); // head = 2, min_tail = 0, tx.len = 2
    assert_eq!(tx.len(), 2);
    assert_eq!(rx1.len(), 2);
    assert_eq!(rx2.len(), 2);

    assert_eq!(rx1.recv().unwrap(), 1); // rx1.tail = 1. rx2.tail = 0. head = 2. min_tail = 0.
    assert_eq!(tx.len(), 2); // Producer len depends on slowest (rx2): 2 - 0 = 2
    assert_eq!(rx1.len(), 1); // rx1 has 1 item left (item 2)
    assert_eq!(rx2.len(), 2); // rx2 still has 2 items (1, 2)

    assert_eq!(rx2.recv().unwrap(), 1); // rx1.tail = 1. rx2.tail = 1. head = 2. min_tail = 1.
    assert_eq!(tx.len(), 1); // Both have consumed item 1: 2 - 1 = 1
    assert_eq!(rx1.len(), 1);
    assert_eq!(rx2.len(), 1);

    assert_eq!(rx1.recv().unwrap(), 2); // rx1.tail = 2. rx2.tail = 1. head = 2. min_tail = 1.
    assert_eq!(tx.len(), 1); // Producer len depends on slowest (rx2): 2 - 1 = 1
    assert_eq!(rx1.len(), 0);
    assert_eq!(rx2.len(), 1);

    assert_eq!(rx2.recv().unwrap(), 2); // rx1.tail = 2. rx2.tail = 2. head = 2. min_tail = 2.
    assert_eq!(tx.len(), 0); // All items consumed by all: 2 - 2 = 0
    assert!(tx.is_empty());
    assert_eq!(rx1.len(), 0);
    assert!(rx1.is_empty());
    assert_eq!(rx2.len(), 0);
    assert!(rx2.is_empty());
  }

  #[test]
  fn spmc_sync_try_send() {
    let (tx, rx1) = bounded(1);
    let rx2 = rx1.clone(); // Keep another receiver

    assert!(tx.try_send(10).is_ok()); // head = 1. min_tail(rx1,rx2) = 0. tx.len = 1.
    assert_eq!(tx.len(), 1);
    assert!(tx.is_full());

    // Channel is full because capacity is 1 and item 10 hasn't been read by rx2 yet.
    match tx.try_send(20) {
      Err(TrySendError::Full(val)) => assert_eq!(val, 20),
      other => panic!("Expected TrySendError::Full, got {:?}", other),
    }

    assert_eq!(rx1.recv().unwrap(), 10); // rx1.tail = 1. rx2.tail = 0. head = 1. min_tail = 0.
    assert_eq!(tx.len(), 1); // Producer still sees len 1 due to rx2: 1 - 0 = 1
    assert!(tx.is_full()); // Still full from producer's perspective because of rx2

    assert_eq!(rx2.recv().unwrap(), 10); // rx1.tail = 1. rx2.tail = 1. head = 1. min_tail = 1.
    assert_eq!(tx.len(), 0); // Now both receivers have consumed item 10: 1 - 1 = 0
    assert!(!tx.is_full());
    assert!(tx.is_empty());

    assert!(tx.try_send(30).is_ok()); // head = 2. min_tail(rx1,rx2) = 1. tx.len = 1.
    assert_eq!(tx.len(), 1);
    assert!(tx.is_full());
  }

  #[test]
  fn spmc_multiple_receivers() {
    let (tx, rx1) = bounded(ITEMS_LOW);
    let rx2 = rx1.clone();
    let rx3 = rx1.clone();

    for i in 0..ITEMS_LOW {
      tx.send(i).unwrap();
    }
    assert_eq!(tx.len(), ITEMS_LOW);
    assert!(tx.is_full());

    let h1 = thread::spawn(move || {
      for i in 0..ITEMS_LOW {
        assert_eq!(rx1.recv().unwrap(), i);
      }
    });
    let h2 = thread::spawn(move || {
      for i in 0..ITEMS_LOW {
        assert_eq!(rx2.recv().unwrap(), i);
      }
    });
    let h3 = thread::spawn(move || {
      for i in 0..ITEMS_LOW {
        assert_eq!(rx3.recv().unwrap(), i);
      }
    });

    h1.join().unwrap();
    h2.join().unwrap();
    h3.join().unwrap();
  }

  #[test]
  fn spmc_sync_slow_consumer_blocks_producer() {
    let (tx, rx_fast) = bounded(1); // Capacity of 1
    let rx_slow = rx_fast.clone();

    tx.send(1).unwrap();
    assert_eq!(tx.len(), 1);
    assert!(tx.is_full());

    assert_eq!(rx_fast.recv().unwrap(), 1);
    // After rx_fast reads, tx.len() is still 1 because rx_slow hasn't read.
    // head = 1, rx_fast.tail = 1, rx_slow.tail = 0. min_tail = 0. len = 1 - 0 = 1.
    assert_eq!(tx.len(), 1);
    assert!(tx.is_full());

    let send_handle = thread::spawn(move || {
      tx.send(2).unwrap();
    });

    thread::sleep(SHORT_TIMEOUT);
    assert!(!send_handle.is_finished(), "BoundedSyncSender should have blocked");

    assert_eq!(rx_slow.recv().unwrap(), 1);
    // After rx_slow reads, tx.len() becomes 0 relative to current head (if head was 1).
    // head = 1, rx_fast.tail = 1, rx_slow.tail = 1. min_tail = 1. len = 1 - 1 = 0.
    // But the send_handle is trying to send item 2, so head will become 2.
    // So, after send_handle unblocks: head = 2. min_tail = 1. len = 1.

    send_handle
      .join()
      .expect("BoundedSyncSender panicked or was not unblocked");

    assert_eq!(rx_fast.len(), 1); // Item 2 is available for rx_fast
    assert_eq!(rx_slow.len(), 1); // Item 2 is available for rx_slow

    assert_eq!(rx_fast.recv().unwrap(), 2);
    assert_eq!(rx_slow.recv().unwrap(), 2);

    assert!(rx_fast.is_empty());
    assert!(rx_slow.is_empty());
  }

  #[test]
  fn spmc_sync_all_receivers_drop_closes_channel() {
    let (tx, rx) = bounded(2);
    let rx2 = rx.clone();
    assert_eq!(tx.capacity(), 2);
    assert_eq!(rx.capacity(), 2);

    tx.send(1).unwrap();

    drop(rx);
    drop(rx2);

    assert!(tx.is_closed());
    assert_eq!(tx.send(2), Err(SendError::Closed));
  }

  #[test]
  fn spmc_close_and_is_closed() {
    // --- Test producer closing ---
    let (mut tx, rx) = bounded(2);
    tx.send(10).unwrap();
    assert!(!tx.is_closed());
    assert!(!rx.is_closed());

    tx.close().unwrap();
    assert_eq!(tx.close(), Err(CloseError)); // Idempotent
    assert_eq!(tx.send(20), Err(SendError::Closed));

    // After the producer is closed, the receiver can still drain the buffer.
    assert_eq!(rx.recv().unwrap(), 10);

    // The next recv will fail, confirming the channel is now disconnected.
    assert_eq!(rx.recv(), Err(RecvError::Disconnected));

    // After the failed recv, is_closed is definitively true.
    assert!(rx.is_closed()); // Now empty and producer is gone

    // --- Test receiver closing ---
    // (This part of the test would have run if the first part didn't panic)
    let (tx, rx1) = bounded(2);
    let rx2 = rx1.clone();
    tx.send(1).unwrap();

    // Close one receiver
    rx1.close().unwrap();
    assert_eq!(rx1.close(), Err(CloseError)); // Idempotent
    assert_eq!(rx1.recv(), Err(RecvError::Disconnected)); // Closed handle
    assert!(!tx.is_closed()); // Other receiver still active

    // Close the second receiver
    assert_eq!(rx2.recv().unwrap(), 1); // Can still recv
    rx2.close().unwrap();
    assert_eq!(rx2.recv(), Err(RecvError::Disconnected));

    // Now the sender should see it's closed
    assert!(tx.is_closed());
    assert_eq!(tx.send(2), Err(SendError::Closed));
  }

  #[test]
  fn test_spmc_ring_buffer_wrap_around_leak() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    static DROP_COUNT: AtomicUsize = AtomicUsize::new(0);

    #[derive(Clone)]
    struct Droppable;

    impl Drop for Droppable {
      fn drop(&mut self) {
        DROP_COUNT.fetch_add(1, Ordering::SeqCst);
      }
    }

    // Create a channel with capacity 2
    let (tx, rx) = bounded::<Droppable>(2);

    // 1. Send 2 items to fill the buffer
    tx.try_send(Droppable).unwrap();
    tx.try_send(Droppable).unwrap();

    // 2. Consume both items (this clones the items; clones are dropped, originals remain in slots)
    let d1 = rx.recv().unwrap();
    let d2 = rx.recv().unwrap();

    // Drop the received clones (increments count by 2)
    drop(d1);
    drop(d2);
    assert_eq!(DROP_COUNT.load(Ordering::SeqCst), 2);

    // 3. Send 2 MORE items. This forces the producer to wrap around
    // and overwrite the first two slots.
    tx.try_send(Droppable).unwrap();
    tx.try_send(Droppable).unwrap();

    // 4. Consume the two new items
    let d3 = rx.recv().unwrap();
    let d4 = rx.recv().unwrap();
    drop(d3);
    drop(d4);

    // 5. Drop the channel handles to trigger Slot destructors for remaining active slots
    drop(tx);
    drop(rx);

    // Total items instantiated in memory = 4 originals + 4 clones = 8 items total.
    // If the bug is present, the first 2 original items are overwritten and leaked,
    // causing the drop count to be 6 instead of 8.
    let final_drops = DROP_COUNT.load(Ordering::SeqCst);
    assert_eq!(
      final_drops, 8,
      "Memory leak detected! Only {} out of 8 items were dropped during wrap-around.",
      final_drops
    );
  }

  mod closed_receivers {
    use crate::error::TrySendError;
    use crate::spmc::{bounded, bounded_async};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    type CloneHook = Arc<Mutex<Option<Box<dyn FnOnce() + Send>>>>;

    /// A payload whose `clone` runs a hook once. The hook stands in for a
    /// producer that runs while a receiver is in the middle of cloning a slot.
    struct HookedPayload {
      dropped: Arc<AtomicBool>,
      dropped_during_clone: Arc<AtomicBool>,
      hook: CloneHook,
    }

    impl HookedPayload {
      fn new(dropped_during_clone: &Arc<AtomicBool>, hook: &CloneHook) -> Self {
        HookedPayload {
          dropped: Arc::new(AtomicBool::new(false)),
          dropped_during_clone: Arc::clone(dropped_during_clone),
          hook: Arc::clone(hook),
        }
      }
    }

    impl Clone for HookedPayload {
      fn clone(&self) -> Self {
        let dropped = Arc::clone(&self.dropped);
        let dropped_during_clone = Arc::clone(&self.dropped_during_clone);
        let hook = Arc::clone(&self.hook);
        // `self` is not read past this point: when the contract is broken the
        // hook has dropped it.
        let pending = hook.lock().unwrap().take();
        if let Some(run) = pending {
          run();
        }
        if dropped.load(Ordering::SeqCst) {
          dropped_during_clone.store(true, Ordering::SeqCst);
        }
        HookedPayload {
          dropped: Arc::new(AtomicBool::new(false)),
          dropped_during_clone,
          hook,
        }
      }
    }

    impl Drop for HookedPayload {
      fn drop(&mut self) {
        self.dropped.store(true, Ordering::SeqCst);
      }
    }

    #[test]
    fn closed_receiver_converted_to_async_never_clones_a_value_the_producer_drops() {
      let dropped_during_clone = Arc::new(AtomicBool::new(false));
      let hook: CloneHook = Arc::new(Mutex::new(None));
      let (tx, live) = bounded::<HookedPayload>(1);
      let closing = live.clone();

      assert!(tx
        .try_send(HookedPayload::new(&dropped_during_clone, &hook))
        .is_ok());
      assert!(live.try_recv().is_ok());

      closing.close().unwrap();
      let mut converted = closing.to_async();

      let next = HookedPayload::new(&dropped_during_clone, &hook);
      *hook.lock().unwrap() = Some(Box::new(move || {
        let _ = tx.try_send(next);
      }));
      let _ = converted.try_recv();
      drop(hook.lock().unwrap().take());

      assert!(
        !dropped_during_clone.load(Ordering::SeqCst),
        "the producer dropped a slot value while the converted receiver was cloning it"
      );
    }

    #[test]
    fn closed_receiver_converted_to_sync_never_clones_a_value_the_producer_drops() {
      let dropped_during_clone = Arc::new(AtomicBool::new(false));
      let hook: CloneHook = Arc::new(Mutex::new(None));
      let (mut tx, mut live) = bounded_async::<HookedPayload>(1);
      let closing = live.clone();

      assert!(tx
        .try_send(HookedPayload::new(&dropped_during_clone, &hook))
        .is_ok());
      assert!(live.try_recv().is_ok());

      closing.close().unwrap();
      let converted = closing.to_sync();

      let next = HookedPayload::new(&dropped_during_clone, &hook);
      *hook.lock().unwrap() = Some(Box::new(move || {
        let _ = tx.try_send(next);
      }));
      let _ = converted.try_recv();
      drop(hook.lock().unwrap().take());

      assert!(
        !dropped_during_clone.load(Ordering::SeqCst),
        "the producer dropped a slot value while the converted receiver was cloning it"
      );
    }

    #[test]
    fn clone_of_a_closed_receiver_does_not_wedge_the_channel() {
      let (tx, live) = bounded::<u32>(1);
      let closing = live.clone();
      closing.close().unwrap();

      tx.try_send(1).unwrap();
      assert_eq!(live.try_recv(), Ok(1));
      tx.try_send(2).unwrap();
      assert_eq!(live.try_recv(), Ok(2));

      let cloned = closing.clone();

      let producer_blocked = matches!(tx.try_send(3), Err(TrySendError::Full(_)));
      let clone_can_receive = cloned.try_recv().is_ok();
      assert!(
        !producer_blocked || clone_can_receive,
        "the clone holds the producer back and has nothing it can receive"
      );
    }
  }

  mod handle_sharing {
    use crate::spmc::{BoundedAsyncReceiver, BoundedAsyncSender, BoundedSyncReceiver, BoundedSyncSender};
    use std::marker::PhantomData;

    trait NotSync {
      const IS_SYNC: bool = false;
    }
    impl<T: ?Sized> NotSync for T {}

    #[allow(dead_code)]
    struct SyncProbe<T: ?Sized>(PhantomData<T>);
    impl<T: ?Sized + Sync> SyncProbe<T> {
      #[allow(dead_code)]
      const IS_SYNC: bool = true;
    }

    #[test]
    fn sender_handles_cannot_be_shared_between_threads() {
      assert!(!<SyncProbe<BoundedSyncSender<u32>>>::IS_SYNC);
      assert!(!<SyncProbe<BoundedAsyncSender<u32>>>::IS_SYNC);
    }

    #[test]
    fn receiver_handles_cannot_be_shared_between_threads() {
      assert!(!<SyncProbe<BoundedSyncReceiver<u32>>>::IS_SYNC);
      assert!(!<SyncProbe<BoundedAsyncReceiver<u32>>>::IS_SYNC);
    }

    trait NotSend {
      const IS_SEND: bool = false;
    }
    impl<T: ?Sized> NotSend for T {}

    #[allow(dead_code)]
    struct SendProbe<T: ?Sized>(PhantomData<T>);
    impl<T: ?Sized + Send> SendProbe<T> {
      const IS_SEND: bool = true;
    }

    /// `Send` and `Clone` but not `Sync`.
    #[derive(Clone)]
    #[allow(dead_code)]
    struct SendOnly(std::cell::Cell<u64>);

    fn is_send<F: Send>(_: &F) {}

    #[test]
    fn receivers_of_a_send_only_payload_stay_on_one_thread() {
      assert!(!<SendProbe<BoundedSyncReceiver<SendOnly>>>::IS_SEND);
      assert!(!<SendProbe<BoundedAsyncReceiver<SendOnly>>>::IS_SEND);
    }

    #[test]
    fn senders_of_a_send_only_payload_move_between_threads() {
      assert!(<SendProbe<BoundedSyncSender<SendOnly>>>::IS_SEND);
      assert!(<SendProbe<BoundedAsyncSender<SendOnly>>>::IS_SEND);
    }

    #[test]
    fn handles_of_a_sync_payload_move_between_threads() {
      assert!(<SendProbe<BoundedSyncSender<u32>>>::IS_SEND);
      assert!(<SendProbe<BoundedAsyncSender<u32>>>::IS_SEND);
      assert!(<SendProbe<BoundedSyncReceiver<u32>>>::IS_SEND);
      assert!(<SendProbe<BoundedAsyncReceiver<u32>>>::IS_SEND);
    }

    #[test]
    fn async_futures_of_a_sync_payload_are_send() {
      let (mut tx, mut rx) = crate::spmc::bounded_async::<u32>(4);
      is_send(&tx.send(1));
      is_send(&tx.send_batch(vec![1, 2]));
      let mut items = vec![1, 2];
      is_send(&tx.send_batch_mut(&mut items));
      is_send(&rx.recv());
      is_send(&rx.recv_batch(2));
      let mut out = Vec::new();
      is_send(&rx.recv_batch_mut(&mut out, 2));
    }
  }
}
