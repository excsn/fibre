//! A multi-consumer "topic" or "pub/sub" channel.
//!
//! This channel allows a single producer to broadcast messages to multiple
//! consumers, where each consumer subscribes to specific "topics". A message
//! sent to a topic is delivered to all consumers subscribed to that topic.
//!
//! ## Behavior
//!
//! - **Topic-Based Filtering**: Consumers only receive messages for topics they
//!   explicitly subscribe to.
//! - **Non-Blocking Sender**: The sender is not blocked by slow consumers. If a
//!   consumer's internal message buffer (mailbox) is full, new messages for
//!   that consumer are dropped, ensuring the sender and other consumers are
//!   not impacted.
//! - **`Clone` Requirement**: Since a message can be delivered to multiple
//!   subscribers, the message type `T` and topic `K` must implement `Clone`.
//! - **Sync/Async Agnostic**: Full interoperability between sync and async code.

mod async_impl;
mod core;
mod left_right;
mod mailbox;
mod sync_impl;

use parking_lot::Mutex;
use std::collections::HashSet;
use std::hash::Hash;
use std::sync::{
  atomic::{AtomicBool, Ordering},
  Arc,
};

// --- Public Re-exports ---

pub use async_impl::{AsyncTopicReceiver, AsyncTopicSender};
pub use sync_impl::{TopicReceiver, TopicSender};

// --- Channel Constructors ---

/// Creates a new synchronous topic-based channel.
///
/// Each receiver created from this channel will have an internal mailbox
/// with the specified `mailbox_capacity`. If a receiver's mailbox is full,
/// subsequent messages sent to its subscribed topics will be dropped for that
/// receiver until it consumes messages and makes space.
pub fn channel<K, T>(mailbox_capacity: usize) -> (TopicSender<K, T>, TopicReceiver<K, T>)
where
  K: Eq + Hash + Clone + Send + Sync + 'static,
  T: Send + Clone + 'static,
{
  let dispatcher = Arc::new(core::SpmcTopicDispatcher::new());
  // Start with one receiver.
  dispatcher.receiver_count.store(1, Ordering::Relaxed);

  let (p, c) = mailbox::channel(mailbox_capacity);

  let sender = TopicSender {
    dispatcher: Arc::clone(&dispatcher),
    closed: AtomicBool::new(false),
  };

  let receiver = TopicReceiver {
    dispatcher: Arc::downgrade(&dispatcher),
    consumer: c,
    producer_mailbox: Arc::new(p),
    subscriptions: Arc::new(Mutex::new(HashSet::new())),
    closed: AtomicBool::new(false),
  };

  (sender, receiver)
}

/// Creates a new asynchronous topic-based channel.
///
/// See [`channel`] for details on `mailbox_capacity`.
pub fn channel_async<K, T>(
  mailbox_capacity: usize,
) -> (AsyncTopicSender<K, T>, AsyncTopicReceiver<K, T>)
where
  K: Eq + Hash + Clone + Send + Sync + 'static,
  T: Send + Clone + 'static,
{
  let dispatcher = Arc::new(core::SpmcTopicDispatcher::new());
  // Start with one receiver.
  dispatcher.receiver_count.store(1, Ordering::Relaxed);

  let (p, c) = mailbox::channel(mailbox_capacity);

  let sender = AsyncTopicSender {
    dispatcher: Arc::clone(&dispatcher),
    closed: AtomicBool::new(false),
  };

  let receiver = AsyncTopicReceiver {
    dispatcher: Arc::downgrade(&dispatcher),
    consumer: c,
    producer_mailbox: Arc::new(p),
    subscriptions: Arc::new(Mutex::new(HashSet::new())),
    closed: AtomicBool::new(false),
  };

  (sender, receiver)
}
