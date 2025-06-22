// channels/src/spmc/topic/core.rs

#![cfg(feature = "topic")]

use super::left_right; // Use our custom left_right module
use super::mailbox;
use papaya::HashMap;
use std::fmt;
use std::hash::Hash;
use std::sync::{
  atomic::{AtomicUsize, Ordering},
  Arc, Weak,
};

/// A highly-concurrent list of subscribers for a single topic.
/// This struct is now Send + Sync because its fields are.
pub(crate) struct SubscriberList<K, T>
where
  K: Send + 'static,
  T: Send + 'static,
{
  pub(crate) reader: left_right::ReadHandle<Vec<Weak<mailbox::MailboxProducer<(K, T)>>>>,
  pub(crate) writer: left_right::WriteHandle<Vec<Weak<mailbox::MailboxProducer<(K, T)>>>>,
}

impl<K, T> SubscriberList<K, T>
where
  K: Send + 'static,
  T: Send + 'static,
{
  /// Creates a new, empty subscriber list.
  pub(crate) fn new() -> Self {
    let (reader, writer) = left_right::new();
    Self { reader, writer }
  }
}

/// The central dispatcher shared by the Sender and all Receivers.
/// It uses a concurrent HashMap to store a subscriber list for each topic.
pub(crate) struct SpmcTopicDispatcher<K, T>
where
  K: Eq + Hash + Clone + Send + Sync + 'static,
  T: Send + Clone + 'static,
{
  pub(crate) subscriptions: HashMap<K, Arc<SubscriberList<K, T>>>,
  pub(crate) receiver_count: AtomicUsize,
}

impl<K, T> fmt::Debug for SpmcTopicDispatcher<K, T>
where
  K: Eq + Hash + Clone + Send + Sync + 'static,
  T: Send + Clone + 'static,
{
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("SpmcTopicDispatcher")
      .field(
        "subscriptions_len",
        &self.subscriptions.pin().len(), // Safely get length
      )
      .field(
        "receiver_count",
        &self.receiver_count.load(Ordering::Relaxed),
      )
      .finish()
  }
}

impl<K, T> SpmcTopicDispatcher<K, T>
where
  K: Eq + Hash + Clone + Send + Sync + 'static,
  T: Send + Clone + 'static,
{
  /// Creates a new, empty dispatcher.
  pub(crate) fn new() -> Self {
    Self {
      subscriptions: HashMap::new(),
      receiver_count: AtomicUsize::new(0),
    }
  }
}
