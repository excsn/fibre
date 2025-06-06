//! A multi-producer, single-consumer (MPSC) channel.
//!
//! This channel is optimized for the MPSC pattern, offering higher performance
//! than a general-purpose MPMC channel by leveraging a lock-free design. It supports
//! mixed sync/async operations, allowing, for example, a synchronous `Producer` to
//! send to an asynchronous `AsyncConsumer`.

use std::marker::PhantomData;
use std::mem;
use std::sync::Arc;
mod lockfree;

// Public re-exports
pub use crate::error::{RecvError, SendError, TryRecvError, TrySendError};
pub use lockfree::{AsyncConsumer, AsyncProducer, Consumer, Producer, RecvFuture, SendFuture};

// --- Sync Constructor ---
/// Creates a new unbounded synchronous MPSC channel.
pub fn channel<T: Send>() -> (Producer<T>, Consumer<T>) {
  let shared = Arc::new(lockfree::MpscShared::new());
  let producer = Producer {
    shared: Arc::clone(&shared),
  };
  let consumer = Consumer {
    shared,
    _phantom: PhantomData,
  };
  (producer, consumer)
}

// --- Async Constructor ---
/// Creates a new unbounded asynchronous MPSC channel.
pub fn channel_async<T: Send>() -> (AsyncProducer<T>, AsyncConsumer<T>) {
  let shared = Arc::new(lockfree::MpscShared::new());
  let producer = AsyncProducer {
    shared: Arc::clone(&shared),
  };
  let consumer = AsyncConsumer {
    shared,
    _phantom: PhantomData,
  };
  (producer, consumer)
}

// --- Conversion Methods ---

impl<T: Send> Producer<T> {
  /// Converts this synchronous `Producer` into an asynchronous `AsyncProducer`.
  pub fn to_async(self) -> AsyncProducer<T> {
    let shared = unsafe { std::ptr::read(&self.shared) };
    mem::forget(self);
    AsyncProducer { shared }
  }
}

impl<T: Send> Consumer<T> {
  /// Converts this synchronous `Consumer` into an asynchronous `AsyncConsumer`.
  pub fn to_async(self) -> AsyncConsumer<T> {
    let shared = unsafe { std::ptr::read(&self.shared) };
    mem::forget(self);
    AsyncConsumer {
      shared,
      _phantom: PhantomData,
    }
  }
}

impl<T: Send> AsyncProducer<T> {
  /// Converts this asynchronous `AsyncProducer` into a synchronous `Producer`.
  pub fn to_sync(self) -> Producer<T> {
    let shared = unsafe { std::ptr::read(&self.shared) };
    mem::forget(self);
    Producer { shared }
  }
}

impl<T: Send> AsyncConsumer<T> {
  /// Converts this asynchronous `AsyncConsumer` into a synchronous `Consumer`.
  pub fn to_sync(self) -> Consumer<T> {
    let shared = unsafe { std::ptr::read(&self.shared) };
    mem::forget(self);
    Consumer {
      shared,
      _phantom: PhantomData,
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::thread;
  use std::time::Duration;
  use tokio::runtime::Runtime;

  #[test]
  fn sync_to_sync_blocking() {
    let (tx, mut rx) = channel::<i32>();
    let handle = thread::spawn(move || rx.recv().unwrap());
    thread::sleep(Duration::from_millis(50));
    tx.send(123).unwrap();
    assert_eq!(handle.join().unwrap(), 123);
  }

  #[tokio::test]
  async fn async_to_async() {
    let (tx, mut rx) = channel_async::<i32>();
    let handle = tokio::spawn(async move { rx.recv().await.unwrap() });
    tokio::time::sleep(Duration::from_millis(50)).await;
    tx.send(456).await.unwrap();
    assert_eq!(handle.await.unwrap(), 456);
  }

  #[tokio::test]
  async fn sync_to_async_conversion() {
    // Create an ASYNC channel pair first.
    let (tx_async, mut rx_async) = channel_async::<i32>();

    // Convert the async producer to a sync one for the thread.
    let tx_sync = tx_async.to_sync();

    let producer_handle = thread::spawn(move || {
      thread::sleep(Duration::from_millis(50));
      // Use the sync producer to send a value.
      tx_sync.send(789).unwrap();
    });

    // Await the async receiver. This is now correct.
    assert_eq!(rx_async.recv().await.unwrap(), 789);
    producer_handle.join().unwrap();
  }

  #[test]
  fn async_to_sync_conversion() {
    let (tx_async, rx_sync) = channel_async::<i32>();
    let mut rx_sync = rx_sync.to_sync(); // Convert

    let rt = Runtime::new().unwrap();
    rt.spawn(async move {
      tokio::time::sleep(Duration::from_millis(50)).await;
      tx_async.send(101).await.unwrap();
    });

    // Blocking receive in a standard thread.
    assert_eq!(rx_sync.recv().unwrap(), 101);
  }
}
