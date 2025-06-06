// src/mpsc/mod.rs

//! A multi-producer, single-consumer (MPSC) channel.
//!
//! This channel is optimized for the MPSC pattern, offering higher performance
//! than a general-purpose MPMC channel by leveraging a lock-free design. Producers
//! can send data without blocking each other, and the single consumer can receive
//! data without any lock contention.

use std::marker::PhantomData;
use std::sync::Arc;
mod lockfree;

// Public re-exports
pub use crate::error::{RecvError, SendError, TryRecvError, TrySendError};
pub use lockfree::{Consumer, Producer};

/// Creates a new unbounded MPSC channel.
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

#[cfg(test)]
mod tests {
  use super::*;
  use std::thread;
  use std::time::Duration;

  #[test]
  fn single_producer_single_consumer() {
    let (tx, mut rx) = channel();
    tx.send(42).unwrap();
    assert_eq!(rx.recv().unwrap(), 42);
  }

  #[test]
  fn recv_blocks_until_send() {
    let (tx, mut rx) = channel::<i32>();

    let consumer_handle = thread::spawn(move || {
      // This will block until the main thread sends a value.
      let val = rx.recv().unwrap();
      assert_eq!(val, 100);
    });

    // Give the consumer thread time to call recv() and park.
    thread::sleep(Duration::from_millis(50));

    tx.send(100).unwrap();

    consumer_handle.join().expect("Consumer thread panicked");
  }

  #[test]
  fn multi_producer_test() {
    let (tx, mut rx) = channel();
    let num_producers = 4;
    let items_per_producer = 1_000;

    let mut handles = Vec::new();
    for _ in 0..num_producers {
      let tx_clone = tx.clone();
      handles.push(thread::spawn(move || {
        for j in 0..items_per_producer {
          tx_clone.send(j).unwrap();
        }
      }));
    }
    drop(tx); // Drop the original producer

    for handle in handles {
      handle.join().unwrap();
    }

    let mut received_count = 0;
    // Drain the receiver
    while let Ok(_) = rx.try_recv() {
      received_count += 1;
    }

    assert_eq!(received_count, num_producers * items_per_producer);
  }

  #[test]
  fn producer_drop_signals_consumer() {
    let (tx, mut rx) = channel::<i32>();
    drop(tx);
    assert_eq!(rx.recv(), Err(RecvError::Disconnected));
  }
}
