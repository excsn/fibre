#![cfg(feature = "topic")]

use crate::error::{RecvError, TryRecvError};
use crate::RecvErrorTimeout;
use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};
use std::thread::{self, Thread};
use std::time::{Duration, Instant};

// --- Waiter & Internal State ---

/// Represents either a sync or async waiter.
#[derive(Debug)]
enum Waiter {
  Sync(Thread),
  Async(Waker),
}

impl Waiter {
  /// Wakes the underlying thread or task.
  fn wake(self) {
    match self {
      Waiter::Sync(thread) => thread.unpark(),
      Waiter::Async(waker) => waker.wake(),
    }
  }
}

/// The internal state of a single mailbox, protected by a Mutex.
#[derive(Debug)]
struct MailboxInternal<T> {
  buffer: VecDeque<T>, //TODO update to use blocked vecdeque
  capacity: usize,
  /// The single consumer waiting for a message.
  consumer_waiter: Option<Waiter>,
  /// True if the `TopicSender` that delivers to this mailbox has been dropped.
  is_disconnected: bool,
  /// A count of messages dropped because the buffer was full.
  dropped_count: u64,
}

/// The shared core of the mailbox.
#[derive(Debug)]
struct MailboxShared<T> {
  internal: Mutex<MailboxInternal<T>>,
}

impl<T> MailboxShared<T> {
  /// Wakes the consumer if one is waiting. Must be called with the lock held.
  fn wake_consumer(&self, guard: &mut MailboxInternal<T>) {
    if let Some(waiter) = guard.consumer_waiter.take() {
      waiter.wake();
    }
  }
}

// --- Public Mailbox Handles ---

/// The producer-side handle for a mailbox. This is held (weakly) by the
/// `TopicSender` and used to deliver messages.
#[derive(Debug)]
pub(crate) struct MailboxProducer<T> {
  shared: Arc<MailboxShared<T>>,
}

/// The consumer-side handle for a mailbox. This is held by a `TopicReceiver`.
#[derive(Debug)]
pub(crate) struct MailboxConsumer<T> {
  shared: Arc<MailboxShared<T>>,
}

/// Creates a new mailbox channel with a given capacity.
pub(crate) fn channel<T>(capacity: usize) -> (MailboxProducer<T>, MailboxConsumer<T>) {
  let internal = MailboxInternal {
    buffer: VecDeque::with_capacity(capacity),
    capacity,
    consumer_waiter: None,
    is_disconnected: false,
    dropped_count: 0,
  };
  let shared = Arc::new(MailboxShared {
    internal: Mutex::new(internal),
  });
  (
    MailboxProducer {
      shared: shared.clone(),
    },
    MailboxConsumer { shared },
  )
}

// --- Producer Implementation ---

impl<T> MailboxProducer<T> {
  /// Delivers a message to this mailbox.
  ///
  /// This operation is **non-blocking**. If the mailbox buffer is full,
  /// the message is dropped, and the function returns immediately.
  pub(crate) fn deliver(&self, value: T) {
    let mut guard = self.shared.internal.lock().unwrap();

    // If the buffer is full, drop the message and increment the counter.
    if guard.buffer.len() >= guard.capacity {
      guard.dropped_count += 1;
      return; // Message is dropped here.
    }

    // Otherwise, push the message and wake the consumer.
    guard.buffer.push_back(value);
    self.shared.wake_consumer(&mut guard);
  }

  /// Signals to the consumer that the channel is disconnected.
  pub(crate) fn disconnect(&self) {
    let mut guard = self.shared.internal.lock().unwrap();
    if !guard.is_disconnected {
      guard.is_disconnected = true;
      self.shared.wake_consumer(&mut guard);
    }
  }

  /// Returns the capacity of the mailbox.
  pub(crate) fn capacity(&self) -> usize {
    self.shared.internal.lock().unwrap().capacity
  }

  /// Returns true if the mailbox buffer is empty.
  pub(crate) fn is_empty(&self) -> bool {
    self.shared.internal.lock().unwrap().buffer.is_empty()
  }
}

impl<T> Drop for MailboxProducer<T> {
  fn drop(&mut self) {
    // When the producer is dropped, it means the main TopicSender is gone.
    // We mark the channel as disconnected and wake the consumer so it can
    // receive an error instead of waiting forever.
    self.disconnect();
  }
}

// --- Consumer Implementation ---

impl<T> MailboxConsumer<T> {
  /// Attempts to receive a message without blocking.
  pub(crate) fn try_recv(&self) -> Result<T, TryRecvError> {
    let mut guard = self.shared.internal.lock().unwrap();

    if let Some(value) = guard.buffer.pop_front() {
      Ok(value)
    } else if guard.is_disconnected {
      Err(TryRecvError::Disconnected)
    } else {
      Err(TryRecvError::Empty)
    }
  }

  /// Receives a message, blocking the current thread if the mailbox is empty.
  pub(crate) fn recv_sync(&self) -> Result<T, RecvError> {
    loop {
      let mut guard = self.shared.internal.lock().unwrap();
      match guard.buffer.pop_front() {
        Some(value) => return Ok(value),
        None => {
          if guard.is_disconnected {
            return Err(RecvError::Disconnected);
          }
          // Park the thread and wait.
          guard.consumer_waiter = Some(Waiter::Sync(thread::current()));
          drop(guard); // Unlock before parking.
          thread::park();
        }
      }
    }
  }

  /// Receives a message, blocking the current thread for at most `timeout` duration.
  pub(crate) fn recv_timeout_sync(&self, timeout: Duration) -> Result<T, RecvErrorTimeout> {
    let start_time = Instant::now();
    loop {
      let mut guard = self.shared.internal.lock().unwrap();
      match guard.buffer.pop_front() {
        Some(value) => return Ok(value),
        None => {
          if guard.is_disconnected {
            return Err(RecvErrorTimeout::Disconnected);
          }

          let elapsed = start_time.elapsed();
          if elapsed >= timeout {
            return Err(RecvErrorTimeout::Timeout);
          }
          let remaining_timeout = timeout - elapsed;

          // Park the thread and wait.
          guard.consumer_waiter = Some(Waiter::Sync(thread::current()));
          drop(guard); // Unlock before parking.
          thread::park_timeout(remaining_timeout);
        }
      }
    }
  }

  /// Receives a message asynchronously.
  pub(crate) fn recv_async(&self) -> RecvFuture<'_, T> {
    RecvFuture { consumer: self }
  }

  /// Returns the capacity of the mailbox.
  pub(crate) fn capacity(&self) -> usize {
    self.shared.internal.lock().unwrap().capacity
  }

  /// Returns true if the mailbox buffer is empty.
  pub(crate) fn is_empty(&self) -> bool {
    self.shared.internal.lock().unwrap().buffer.is_empty()
  }
}

// --- Future Implementation ---

#[must_use = "futures do nothing unless you .await or poll them"]
pub struct RecvFuture<'a, T> {
  consumer: &'a MailboxConsumer<T>,
}

impl<'a, T> Future for RecvFuture<'a, T> {
  type Output = Result<T, RecvError>;

  fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    let mut guard = self.consumer.shared.internal.lock().unwrap();

    // Try to receive a value.
    if let Some(value) = guard.buffer.pop_front() {
      return Poll::Ready(Ok(value));
    }

    // Check for disconnection.
    if guard.is_disconnected {
      return Poll::Ready(Err(RecvError::Disconnected));
    }

    // If empty and not disconnected, register the waker and pend.
    match &guard.consumer_waiter {
      Some(Waiter::Async(existing_waker)) if existing_waker.will_wake(cx.waker()) => {
        // Same waker, no need to update
      }
      _ => {
        guard.consumer_waiter = Some(Waiter::Async(cx.waker().clone()));
      }
    }

    Poll::Pending
  }
}

// Test-only helper function to access internal state.
impl<T> MailboxConsumer<T> {
  #[cfg(test)]
  fn dropped_count(&self) -> u64 {
    self.shared.internal.lock().unwrap().dropped_count
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::time::Duration;
  use tokio::time::timeout;

  const TEST_TIMEOUT: Duration = Duration::from_millis(500);

  #[test]
  fn try_recv_empty_and_after_deliver() {
    let (producer, consumer) = channel::<i32>(5);
    assert!(matches!(consumer.try_recv(), Err(TryRecvError::Empty)));

    producer.deliver(100);
    assert_eq!(consumer.try_recv().unwrap(), 100);
    assert!(matches!(consumer.try_recv(), Err(TryRecvError::Empty)));
  }

  #[test]
  fn deliver_drops_when_full() {
    let (producer, consumer) = channel::<i32>(2);

    producer.deliver(1);
    producer.deliver(2);
    assert_eq!(consumer.dropped_count(), 0);

    // This one should be dropped.
    producer.deliver(3);
    assert_eq!(consumer.dropped_count(), 1);

    // This one should also be dropped.
    producer.deliver(4);
    assert_eq!(consumer.dropped_count(), 2);

    // Ensure the original messages are still there.
    assert_eq!(consumer.try_recv().unwrap(), 1);
    assert_eq!(consumer.try_recv().unwrap(), 2);
    assert!(matches!(consumer.try_recv(), Err(TryRecvError::Empty)));
  }

  #[test]
  fn producer_drop_with_items_in_channel() {
    let (producer, consumer) = channel::<i32>(5);
    producer.deliver(1);
    producer.deliver(2);

    drop(producer);

    assert_eq!(consumer.recv_sync().unwrap(), 1);
    assert_eq!(consumer.recv_sync().unwrap(), 2);
    assert_eq!(consumer.recv_sync(), Err(RecvError::Disconnected));
  }

  #[test]
  fn producer_drop_with_empty_channel() {
    let (producer, consumer) = channel::<i32>(5);
    drop(producer);
    assert_eq!(consumer.recv_sync(), Err(RecvError::Disconnected));
  }

  #[test]
  fn recv_sync_blocks_and_unblocks() {
    let (producer, consumer) = channel(1);

    let handle = thread::spawn(move || {
      // This will block until a message is sent.
      consumer.recv_sync()
    });

    // Give the thread time to block.
    thread::sleep(Duration::from_millis(100));

    producer.deliver(99);

    let result = handle.join().expect("Thread panicked");
    assert_eq!(result.unwrap(), 99);
  }

  #[tokio::test]
  async fn recv_async_waits_and_completes() {
    let (producer, consumer) = channel(1);

    let task = tokio::spawn(async move {
      // This will pend until a message is sent.
      consumer.recv_async().await
    });

    // Give the task time to pend.
    tokio::time::sleep(Duration::from_millis(50)).await;

    producer.deliver(123);

    let result = timeout(TEST_TIMEOUT, task)
      .await
      .expect("Task timed out or panicked")
      .unwrap();

    assert_eq!(result.unwrap(), 123);
  }

  #[tokio::test]
  async fn async_recv_wakes_on_disconnect() {
    let (producer, consumer) = channel::<()>(1);

    let task = tokio::spawn(async move {
      // Should pend, then wake with an error.
      consumer.recv_async().await
    });

    // Give the task time to pend.
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Dropping the producer disconnects the channel.
    drop(producer);

    let result = timeout(TEST_TIMEOUT, task)
      .await
      .expect("Task timed out or panicked")
      .unwrap();

    assert_eq!(result, Err(RecvError::Disconnected));
  }

  #[test]
  fn concurrent_delivery() {
    let (producer, consumer) = channel(100);
    let mut handles = Vec::new();

    for i in 0..10 {
      let p_clone = Arc::clone(&producer.shared);
      handles.push(thread::spawn(move || {
        let producer = MailboxProducer { shared: p_clone };
        for j in 0..10 {
          producer.deliver(i * 10 + j);
        }
      }));
    }

    for handle in handles {
      handle.join().unwrap();
    }
    drop(producer);

    let mut received_count = 0;
    while consumer.try_recv().is_ok() {
      received_count += 1;
    }

    assert_eq!(received_count, 100);
    assert_eq!(consumer.dropped_count(), 0);
  }
}
