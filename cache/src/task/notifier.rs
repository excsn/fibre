use crate::listener::{EvictionListener, EvictionReason};

use std::sync::Arc;
use std::thread::{self, JoinHandle};

use fibre::mpsc;

/// A message sent to the notifier task.
pub(crate) type Notification<K, V> = (K, Arc<V>, EvictionReason);

/// The background task responsible for calling user-provided eviction listeners.
pub(crate) struct Notifier<K: Send, V: Send + Sync> {
  handle: JoinHandle<()>,
  _sender: mpsc::BoundedSender<(K, Arc<V>, EvictionReason)>,
}

impl<K: Send, V: Send + Sync> Notifier<K, V> {
  /// Spawns a new notifier thread.
  pub(crate) fn spawn(
    listener: Arc<dyn EvictionListener<K, V>>,
  ) -> (Self, mpsc::BoundedSender<Notification<K, V>>)
  where
    K: Send + 'static,
    V: Send + 'static,
  {
    // A simple, bounded MPSC channel for notifications.
    const NOTIFICATION_CHANNEL_CAPACITY: usize = 128;
    let (tx, rx): (
      mpsc::BoundedSender<Notification<K, V>>,
      mpsc::BoundedReceiver<Notification<K, V>>,
    ) = mpsc::bounded(NOTIFICATION_CHANNEL_CAPACITY);

    let handle = thread::spawn(move || {
      // The main notifier loop.
      // The loop will automatically end when the channel is disconnected
      // (i.e., when all senders, including the one in `CacheShared`, are dropped).
      while let Ok((key, value, reason)) = rx.recv() {
        listener.on_evict(key, value, reason);
      }
    });

    let notifier = Self {
      handle,
      // Store the sender in a Box<dyn Any> to type-erase it,
      // since Notifier itself is not generic.
      _sender: tx.clone(),
    };

    (notifier, tx)
  }

  /// Waits for the notifier thread to finish.
  pub(crate) fn stop(self) {
    // Dropping the `_sender` inside `self` will disconnect the channel,
    // allowing the receiver thread to terminate gracefully.
    drop(self._sender);
  }
}
