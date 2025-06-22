// src/spmc/topic/sync_impl.rs

#![cfg(feature = "topic")]

use super::core::{SpmcTopicDispatcher, SubscriberList};
use super::mailbox;
use crate::error::{RecvError, SendError};
use crate::spmc::topic::async_impl::{AsyncTopicReceiver, AsyncTopicSender};
use crate::TryRecvError;

use std::borrow::Borrow;
use std::collections::HashSet;
use std::hash::Hash;
use std::mem;
use std::sync::{
  atomic::{AtomicBool, Ordering},
  Arc, Weak,
};

use papaya::Equivalent;
use parking_lot::Mutex;

// --- Sync Sender ---

/// The synchronous sending-half of a topic-based SPMC channel.
///
/// The sender is cloneable. Note that there is only one logical producer, but
/// the handle can be cloned to be used from multiple threads.
#[derive(Debug)]
pub struct TopicSender<K, T>
where
  K: Eq + Hash + Clone + Send + Sync + 'static,
  T: Send + Clone + 'static,
{
  pub(crate) dispatcher: Arc<SpmcTopicDispatcher<K, T>>,
  pub(crate) closed: AtomicBool,
}

impl<K, T> TopicSender<K, T>
where
  K: Eq + Hash + Clone + Send + Sync + 'static,
  T: Send + Clone + 'static,
{
  /// Sends a message to all subscribers of the given topic.
  ///
  /// This operation is non-blocking with respect to slow consumers. If a
  /// subscriber's message queue (mailbox) is full, the message will be
  /// dropped for that specific subscriber, but delivery to other subscribers
  /// will not be affected.
  ///
  /// # Errors
  ///
  /// - `Err(SendError::Closed)`: Returned if all `TopicReceiver` handles
  ///   have been dropped, indicating that there are no subscribers left in
  ///   the system.
  pub fn send(&self, topic: K, value: T) -> Result<(), SendError> {
    if self.closed.load(Ordering::Relaxed) || self.is_closed() {
      return Err(SendError::Closed);
    }

    let pinned_map = self.dispatcher.subscriptions.pin();

    if let Some(list_arc) = pinned_map.get(&topic) {
      // Enter the lock-free read path.
      let mailboxes_snapshot = list_arc.reader.enter();

      // Clone the list to iterate over it without holding the guard.
      let subscribers = mailboxes_snapshot.clone();
      drop(mailboxes_snapshot); // Guard is dropped, no locks held.

      for mailbox_weak in subscribers.iter() {
        if let Some(mailbox_strong) = mailbox_weak.upgrade() {
          mailbox_strong.deliver((topic.clone(), value.clone()));
        }
      }
    }

    Ok(())
  }

  /// Returns `true` if all receivers for this channel have been dropped.
  pub fn is_closed(&self) -> bool {
    self.dispatcher.receiver_count.load(Ordering::Relaxed) == 0
  }

  /// Converts this synchronous `TopicSender` into an `AsyncTopicSender`.
  ///
  /// This is a zero-cost conversion. The `Drop` implementation of the
  /// original `TopicSender` is not called.
  pub fn to_async(self) -> AsyncTopicSender<K, T> {
    let dispatcher = unsafe { std::ptr::read(&self.dispatcher) };
    let closed = unsafe { std::ptr::read(&self.closed) };
    mem::forget(self);
    AsyncTopicSender { dispatcher, closed }
  }
}

impl<K, T> Drop for TopicSender<K, T>
where
  K: Eq + Hash + Clone + Send + Sync + 'static,
  T: Send + Clone + 'static,
{
  fn drop(&mut self) {
    if !self.closed.swap(true, Ordering::AcqRel) {
      // Get a guard to safely clear the map.
      let guard = self.dispatcher.subscriptions.guard();
      self.dispatcher.subscriptions.clear(&guard);
    }
  }
}

// --- Sync Receiver ---

/// The synchronous receiving-half of a topic-based SPMC channel.
#[derive(Debug)]
pub struct TopicReceiver<K, T>
where
  K: Eq + Hash + Clone + Send + Sync + 'static,
  T: Send + Clone + 'static,
{
  /// A weak reference back to the central dispatcher.
  pub(crate) dispatcher: Weak<SpmcTopicDispatcher<K, T>>,
  /// The consumer end of this receiver's private MPSC mailbox.
  pub(crate) consumer: mailbox::MailboxConsumer<(K, T)>,
  /// The producer end of the mailbox, held so we can create weak refs to it.
  pub(crate) producer_mailbox: Arc<mailbox::MailboxProducer<(K, T)>>,
  /// The set of topics this receiver is currently subscribed to.
  pub(crate) subscriptions: Arc<Mutex<HashSet<K>>>,
}

impl<K, T> TopicReceiver<K, T>
where
  K: Eq + Hash + Clone + Send + Sync + 'static,
  T: Send + Clone + 'static,
{
  /// Blocks until a message is received on any of this receiver's subscribed topics.
  ///
  /// # Errors
  ///
  /// - `Err(RecvError::Disconnected)`: Returned if the `TopicSender` is dropped
  ///   and all messages in this receiver's mailbox have been consumed.
  pub fn recv(&self) -> Result<(K, T), RecvError> {
    self.consumer.recv_sync()
  }

  /// Attempts to receive a message without blocking.
  ///
  /// Returns `Ok((K, T))` if a message is available on a subscribed topic.
  ///
  /// # Errors
  ///
  /// - `Err(TryRecvError::Empty)`: No messages are currently available.
  /// - `Err(TryRecvError::Disconnected)`: The sender has been dropped and the
  ///   mailbox is empty.
  pub fn try_recv(&self) -> Result<(K, T), TryRecvError> {
    self.consumer.try_recv()
  }

  /// Subscribes this receiver to a new topic.
  ///
  /// If the receiver is already subscribed to this topic, this is a no-op.
  pub fn subscribe(&self, topic: K) {
    let mut subs = self.subscriptions.lock();
    if !subs.insert(topic.clone()) {
      return; // Already subscribed.
    }
    drop(subs);

    if let Some(dispatcher) = self.dispatcher.upgrade() {
      let list_arc = dispatcher
        .subscriptions
        .pin()
        .get_or_insert_with(topic, || Arc::new(SubscriberList::new()))
        .clone();

      // Use the new `modify` API to perform a copy-on-write update.
      list_arc.writer.modify(|list| {
        // Prune dead weak pointers from the list.
        list.retain(|w| w.upgrade().is_some());

        // Add self to the list if not already present.
        if !list
          .iter()
          .any(|w| w.ptr_eq(&Arc::downgrade(&self.producer_mailbox)))
        {
          list.push(Arc::downgrade(&self.producer_mailbox));
        }
      });
    }
  }

  /// Unsubscribes this receiver from a topic.
  pub fn unsubscribe<Q: ?Sized>(&self, topic: &Q)
  where
    K: Borrow<Q> + Equivalent<Q>,
    Q: Hash + Eq,
  {
    let mut subs = self.subscriptions.lock();
    if !subs.remove(topic) {
      return; // Not subscribed, nothing to do.
    }
    drop(subs);

    if let Some(dispatcher) = self.dispatcher.upgrade() {
      if let Some(list_arc) = dispatcher.subscriptions.pin().get(topic) {
        // Use the `modify` API to safely remove our mailbox.
        list_arc.writer.modify(|list| {
          list.retain(|w| {
            w.upgrade()
              .map_or(false, |s| !Arc::ptr_eq(&s, &self.producer_mailbox))
          });
        });
      }
    }
  }

  /// Converts this synchronous `TopicReceiver` into an `AsyncTopicReceiver`.
  ///
  /// This is a zero-cost conversion. The `Drop` implementation of the
  /// original `TopicReceiver` is not called.
  pub fn to_async(self) -> AsyncTopicReceiver<K, T> {
    // Use ptr::read to move fields and mem::forget to prevent drop.
    let dispatcher = unsafe { std::ptr::read(&self.dispatcher) };
    let consumer = unsafe { std::ptr::read(&self.consumer) };
    let producer_mailbox = unsafe { std::ptr::read(&self.producer_mailbox) };
    let subscriptions = unsafe { std::ptr::read(&self.subscriptions) };
    mem::forget(self);
    AsyncTopicReceiver {
      dispatcher,
      consumer,
      producer_mailbox,
      subscriptions,
    }
  }
}

impl<K, T> Clone for TopicReceiver<K, T>
where
  K: Eq + Hash + Clone + Send + Sync + 'static,
  T: Send + Clone + 'static,
{
  fn clone(&self) -> Self {
    if let Some(dispatcher) = self.dispatcher.upgrade() {
      dispatcher.receiver_count.fetch_add(1, Ordering::Relaxed);

      // A better way to get capacity would be a method on the MailboxConsumer.
      // For now, this is a placeholder. Assume a default or fetch it.
      let mailbox_capacity = 16; // TODO: Plumb capacity from original receiver.
      let (p, c) = mailbox::channel(mailbox_capacity);

      let new_receiver = Self {
        dispatcher: self.dispatcher.clone(),
        consumer: c,
        producer_mailbox: Arc::new(p),
        subscriptions: Arc::new(Mutex::new(self.subscriptions.lock().clone())),
      };

      // Re-subscribe the new clone to all the same topics.
      for topic in new_receiver.subscriptions.lock().iter() {
        new_receiver.subscribe(topic.clone());
      }

      new_receiver
    } else {
      // If the dispatcher is gone, create a dead receiver.
      let (p, c) = mailbox::channel(0);
      Self {
        dispatcher: Weak::new(),
        consumer: c,
        producer_mailbox: Arc::new(p),
        subscriptions: Arc::new(Mutex::new(HashSet::new())),
      }
    }
  }
}

impl<K, T> Drop for TopicReceiver<K, T>
where
  K: Eq + Hash + Clone + Send + Sync + 'static,
  T: Send + Clone + 'static,
{
  fn drop(&mut self) {
    if let Some(dispatcher) = self.dispatcher.upgrade() {
      // Unsubscribe from all topics this receiver was listening to.
      let subs_to_drop = self.subscriptions.lock().drain().collect::<Vec<_>>();
      for topic in subs_to_drop {
        self.unsubscribe(&topic);
      }
      // Decrement the global receiver count.
      dispatcher.receiver_count.fetch_sub(1, Ordering::Relaxed);
    }
  }
}
