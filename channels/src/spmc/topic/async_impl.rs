// src/spmc/topic/async_impl.rs

#![cfg(feature = "topic")]

use super::core::{SpmcTopicDispatcher, SubscriberList};
use super::mailbox::{self, RecvFuture};
use crate::error::SendError;
use crate::spmc::topic::sync_impl::{TopicReceiver, TopicSender};
use crate::TryRecvError;

use std::borrow::Borrow;
use std::collections::HashSet;
use std::future::Future;
use std::hash::Hash;
use std::mem;
use std::pin::Pin;
use std::sync::{
  atomic::{AtomicBool, Ordering},
  Arc, Weak,
};
use std::task::{Context, Poll};

use futures_core::Stream;
use papaya::Equivalent;
use parking_lot::Mutex;

// --- Async Sender ---

/// The asynchronous sending-half of a topic-based SPMC channel.
#[derive(Debug)]
pub struct AsyncTopicSender<K, T>
where
  K: Eq + Hash + Clone + Send + Sync + 'static,
  T: Send + Clone + 'static,
{
  pub(crate) dispatcher: Arc<SpmcTopicDispatcher<K, T>>,
  pub(crate) closed: AtomicBool,
}

impl<K, T> AsyncTopicSender<K, T>
where
  K: Eq + Hash + Clone + Send + Sync + 'static,
  T: Send + Clone + 'static,
{
  /// Sends a message to all subscribers of the given topic.
  ///
  /// This operation is non-blocking. If a subscriber's mailbox is full, the
  /// message is dropped for that subscriber. The returned `Result` indicates
  /// only if the channel is closed (all receivers dropped).
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

  /// Converts this asynchronous `AsyncTopicSender` into a `TopicSender`.
  ///
  /// This is a zero-cost conversion. The `Drop` implementation of the
  /// original `AsyncTopicSender` is not called.
  pub fn to_sync(self) -> TopicSender<K, T> {
    let dispatcher = unsafe { std::ptr::read(&self.dispatcher) };
    let closed = unsafe { std::ptr::read(&self.closed) };
    mem::forget(self);
    TopicSender { dispatcher, closed }
  }
}

impl<K, T> Drop for AsyncTopicSender<K, T>
where
  K: Eq + Hash + Clone + Send + Sync + 'static,
  T: Send + Clone + 'static,
{
  fn drop(&mut self) {
    if !self.closed.swap(true, Ordering::AcqRel) {
      // Sender is being dropped. We must notify all active subscribers that no
      // more messages will come. We do this by iterating through all subscriber
      // lists and calling `disconnect` on their mailboxes.
      // We do NOT clear the map, as receivers need it to unsubscribe themselves.
      let pinned_map = self.dispatcher.subscriptions.pin();
      for (_topic, list_arc) in pinned_map.iter() {
        let subscribers_snapshot = list_arc.reader.enter();
        for mailbox_weak in subscribers_snapshot.iter() {
          if let Some(mailbox_strong) = mailbox_weak.upgrade() {
            mailbox_strong.disconnect();
          }
        }
      }
    }
  }
}

// --- Async Receiver ---

/// The asynchronous receiving-half of a topic-based SPMC channel.
#[derive(Debug)]
pub struct AsyncTopicReceiver<K, T>
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

impl<K, T> AsyncTopicReceiver<K, T>
where
  K: Eq + Hash + Clone + Send + Sync + 'static,
  T: Send + Clone + 'static,
{
  /// Waits asynchronously for a message on any subscribed topic.
  ///
  /// Returns a `Future` that resolves to the message or an error if the
  /// channel is disconnected.
  pub fn recv(&self) -> RecvFuture<'_, (K, T)> {
    self.consumer.recv_async()
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

      list_arc.writer.modify(|list| {
        list.retain(|w| w.upgrade().is_some());
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
      return; // Not subscribed.
    }
    drop(subs);

    if let Some(dispatcher) = self.dispatcher.upgrade() {
      if let Some(list_arc) = dispatcher.subscriptions.pin().get(topic) {
        list_arc.writer.modify(|list| {
          list.retain(|w| {
            w.upgrade()
              .map_or(false, |s| !Arc::ptr_eq(&s, &self.producer_mailbox))
          });
        });
      }
    }
  }

  /// Converts this asynchronous `AsyncTopicReceiver` into a `TopicReceiver`.
  ///
  /// This is a zero-cost conversion. The `Drop` implementation of the
  /// original `AsyncTopicReceiver` is not called.
  pub fn to_sync(self) -> TopicReceiver<K, T> {
    // Use ptr::read to move fields and mem::forget to prevent drop.
    let dispatcher = unsafe { std::ptr::read(&self.dispatcher) };
    let consumer = unsafe { std::ptr::read(&self.consumer) };
    let producer_mailbox = unsafe { std::ptr::read(&self.producer_mailbox) };
    let subscriptions = unsafe { std::ptr::read(&self.subscriptions) };
    mem::forget(self);
    TopicReceiver {
      dispatcher,
      consumer,
      producer_mailbox,
      subscriptions,
    }
  }
}

impl<K, T> Clone for AsyncTopicReceiver<K, T>
where
  K: Eq + Hash + Clone + Send + Sync + 'static,
  T: Send + Clone + 'static,
{
  fn clone(&self) -> Self {
    if let Some(dispatcher) = self.dispatcher.upgrade() {
      dispatcher.receiver_count.fetch_add(1, Ordering::Relaxed);

      let mailbox_capacity = self.consumer.capacity();
      let (p, c) = mailbox::channel(mailbox_capacity);

      let topics_to_subscribe: Vec<K> = self.subscriptions.lock().iter().cloned().collect();

      let new_receiver = Self {
        dispatcher: self.dispatcher.clone(),
        consumer: c,
        producer_mailbox: Arc::new(p),
        subscriptions: Arc::new(Mutex::new(HashSet::new())),
      };

      for topic in topics_to_subscribe {
        new_receiver.subscribe(topic);
      }
      new_receiver
    } else {
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

impl<K, T> Drop for AsyncTopicReceiver<K, T>
where
  K: Eq + Hash + Clone + Send + Sync + 'static,
  T: Send + Clone + 'static,
{
  fn drop(&mut self) {
    if let Some(dispatcher) = self.dispatcher.upgrade() {
      let topics_to_unsubscribe: Vec<K> = self.subscriptions.lock().drain().collect();

      for topic in topics_to_unsubscribe {
        self.unsubscribe(&topic);
      }

      dispatcher.receiver_count.fetch_sub(1, Ordering::Relaxed);
    }
  }
}

impl<K, T> Stream for AsyncTopicReceiver<K, T>
where
  K: Eq + Hash + Clone + Send + Sync + 'static,
  T: Send + Clone + 'static,
{
  type Item = (K, T);

  fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
    // We can use Pin::get_mut because we are not moving out of the future.
    let receiver = self.get_mut();
    match Pin::new(&mut receiver.consumer.recv_async()).poll(cx) {
      Poll::Ready(Ok(value)) => Poll::Ready(Some(value)),
      Poll::Ready(Err(_)) => Poll::Ready(None), // Disconnected
      Poll::Pending => Poll::Pending,
    }
  }
}
