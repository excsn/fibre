// src/mpmc/async_impl.rs

//! Implementation of the asynchronous Future-based send and receive logic.

use futures_core::Stream;

use super::{AsyncReceiver, AsyncSender};
use crate::error::{SendError, TryRecvError, TrySendError};
use crate::internal::waiter::{AsyncWaiterNode, LinkedNode};
use crate::RecvError;

use core::marker::PhantomPinned;
use std::future::Future;
use std::pin::Pin;
use std::ptr::NonNull;
use std::sync::atomic::Ordering;
use std::task::{Context, Poll};

// --- SendFuture ---

/// A future that completes when a value has been sent to the MPMC channel.
#[must_use = "futures do nothing unless you .await or poll them"]
#[derive(Debug)]
pub struct SendFuture<'a, T: Send> {
  sender: &'a AsyncSender<T>,
  // The item is wrapped in an Option so it can be taken during the poll.
  item: Option<T>,
  waiter_node: AsyncWaiterNode<T>,
  _phantom: PhantomPinned,
}

impl<'a, T: Send> SendFuture<'a, T> {
  pub(super) fn new(sender: &'a AsyncSender<T>, item: T) -> Self {
    Self {
      sender,
      item: Some(item),
      waiter_node: AsyncWaiterNode::new(),
      _phantom: PhantomPinned,
    }
  }
}

impl<'a, T: Send> Future for SendFuture<'a, T> {
  type Output = Result<(), SendError>;

  fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    let this = unsafe { self.as_mut().get_unchecked_mut() };
    'poll_loop: loop {
      // Recover item from node if necessary (Rendezvous)
      if this.item.is_none() {
        match this.waiter_node.item.take() {
          Some(recovered) => this.item = Some(recovered),
          None => return Poll::Ready(Ok(())), // item was consumed, done
        }
      }

      let mut guard = this.sender.shared.internal.lock();

      if guard.receiver_count == 0 {
        return Poll::Ready(Err(SendError::Closed));
      }

      // 1. Wake waiting receiver (Direct Handoff)
      if let Some(mut waiter_ptr) = unsafe { guard.waiting_async_receivers.pop_front() } {
        let waiter = unsafe { waiter_ptr.as_mut() };
        waiter.item = this.item.take();
        drop(guard);
        waiter.wake();
        return Poll::Ready(Ok(()));
      }
      if let Some(mut waiter_ptr) = unsafe { guard.waiting_sync_receivers.pop_front() } {
        let waiter = unsafe { waiter_ptr.as_mut() };
        waiter.item = this.item.take();
        drop(guard);
        waiter.wake();
        return Poll::Ready(Ok(()));
      }

      // 2. Push to buffer
      if this.sender.shared.capacity > 0 && guard.queue.len() < this.sender.shared.capacity {
        guard.queue.push_back(this.item.take().unwrap());
        drop(guard);
        return Poll::Ready(Ok(()));
      }

      // 3. Park
      this.waiter_node.item = this.item.take();
      unsafe {
        if this.waiter_node.is_enqueued() {
          if !this.waiter_node.waker.as_ref().unwrap().will_wake(cx.waker()) {
            this.waiter_node.waker = Some(cx.waker().clone());
          }
        } else {
          this.waiter_node.waker = Some(cx.waker().clone());
          let node_ptr = NonNull::from(&mut this.waiter_node);
          guard.waiting_async_senders.push_back(node_ptr);
        }
      }
      return Poll::Pending;
    }
  }
}

impl<'a, T: Send> Drop for SendFuture<'a, T> {
  fn drop(&mut self) {
    if self.waiter_node.is_enqueued() {
      let mut guard = self.sender.shared.internal.lock();
      unsafe {
        let node_ptr = NonNull::from(&mut self.waiter_node);
        guard.waiting_async_senders.remove(node_ptr);
      }
      // If rendezvous and item is still in node, it gets dropped with the node.
      // If item is gone from node, it was sent.
    }
  }
}

/// A future that completes when a value has been received from the MPMC channel.
#[must_use = "futures do nothing unless you .await or poll them"]
#[derive(Debug)]
pub struct RecvFuture<'a, T: Send> {
  receiver: &'a AsyncReceiver<T>,
  waiter_node: AsyncWaiterNode<T>,
  _phantom: PhantomPinned,
}

impl<'a, T: Send> RecvFuture<'a, T> {
  pub(super) fn new(receiver: &'a AsyncReceiver<T>) -> Self {
    Self {
      receiver,
      waiter_node: AsyncWaiterNode::new(),
      _phantom: PhantomPinned,
    }
  }
}

impl<'a, T: Send> Future for RecvFuture<'a, T> {
  type Output = Result<T, RecvError>;

  fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    let this = unsafe { self.as_mut().get_unchecked_mut() };

    loop {
      // Check if we have a node and if it has an item (direct handoff)
      if let Some(item) = this.waiter_node.item.take() {
        return Poll::Ready(Ok(item));
      }

      let mut guard = this.receiver.shared.internal.lock();

      // 1. Check buffer
      if let Some(item) = guard.queue.pop_front() {
        // Wake a buffered sender if we freed space
        let mut async_sender_to_wake = None;
        let mut sync_sender_to_wake = None;
        if this.receiver.shared.capacity > 0 {
          if let Some(mut sender_ptr) = unsafe { guard.waiting_async_senders.pop_front() } {
            let sender = unsafe { sender_ptr.as_mut() };
            let sender_item = sender.item.take().expect("Sender must have an item");
            guard.queue.push_back(sender_item);
            async_sender_to_wake = Some(sender_ptr);
          } else if let Some(mut sender_ptr) = unsafe { guard.waiting_sync_senders.pop_front() } {
            let sender = unsafe { sender_ptr.as_mut() };
            let sender_item = sender.item.take().expect("Sender must have an item");
            guard.queue.push_back(sender_item);
            sync_sender_to_wake = Some(sender_ptr);
          }
        }
        drop(guard);
        if let Some(sender_ptr) = async_sender_to_wake {
          unsafe { sender_ptr.as_ref().wake() };
        }
        if let Some(sender_ptr) = sync_sender_to_wake {
          unsafe { sender_ptr.as_ref().wake() };
        }
        return Poll::Ready(Ok(item));
      }

      // 2. Check Rendezvous Handoff
      if let Some(mut sender_ptr) = unsafe { guard.waiting_async_senders.pop_front() } {
        let sender = unsafe { sender_ptr.as_mut() };
        let item = sender.item.take().expect("Sender must have an item");
        drop(guard);
        sender.wake();
        return Poll::Ready(Ok(item));
      }
      if let Some(mut sender_ptr) = unsafe { guard.waiting_sync_senders.pop_front() } {
        let sender = unsafe { sender_ptr.as_mut() };
        let item = sender.item.take().expect("Sender must have an item");
        drop(guard);
        sender.wake();
        return Poll::Ready(Ok(item));
      }

      if guard.sender_count == 0 {
        return Poll::Ready(Err(RecvError::Disconnected));
      }

      // 3. Park
      unsafe {
        if this.waiter_node.is_enqueued() {
          if !this.waiter_node.waker.as_ref().unwrap().will_wake(cx.waker()) {
            this.waiter_node.waker = Some(cx.waker().clone());
          }
        } else {
          this.waiter_node.waker = Some(cx.waker().clone());
          let node_ptr = NonNull::from(&mut this.waiter_node);
          guard.waiting_async_receivers.push_back(node_ptr);
        }
      }
      return Poll::Pending;
    }
  }
}

impl<'a, T: Send> Drop for RecvFuture<'a, T> {
  fn drop(&mut self) {
    if self.waiter_node.is_enqueued() {
      let mut guard = self.receiver.shared.internal.lock();
      unsafe {
        let node_ptr = NonNull::from(&mut self.waiter_node);
        guard.waiting_async_receivers.remove(node_ptr);
      }
    }
  }
}

impl<T: Send> Stream for AsyncReceiver<T> {
  type Item = T;

  fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
    let this = unsafe { self.as_mut().get_unchecked_mut() };

    loop {
      // Check if we have a node and if it has an item (direct handoff)
      if let Some(item) = this.waiter_node.item.take() {
        return Poll::Ready(Some(item));
      }

      let mut guard = this.shared.internal.lock();

      // 1. Check buffer
      if let Some(item) = guard.queue.pop_front() {
        let mut async_sender_to_wake = None;
        let mut sync_sender_to_wake = None;
        if this.shared.capacity > 0 {
          if let Some(mut sender_ptr) = unsafe { guard.waiting_async_senders.pop_front() } {
            let sender = unsafe { sender_ptr.as_mut() };
            let sender_item = sender.item.take().expect("Sender must have an item");
            guard.queue.push_back(sender_item);
            async_sender_to_wake = Some(sender_ptr);
          } else if let Some(mut sender_ptr) = unsafe { guard.waiting_sync_senders.pop_front() } {
            let sender = unsafe { sender_ptr.as_mut() };
            let sender_item = sender.item.take().expect("Sender must have an item");
            guard.queue.push_back(sender_item);
            sync_sender_to_wake = Some(sender_ptr);
          }
        }
        drop(guard);
        if let Some(sender_ptr) = async_sender_to_wake {
          unsafe { sender_ptr.as_ref().wake() };
        }
        if let Some(sender_ptr) = sync_sender_to_wake {
          unsafe { sender_ptr.as_ref().wake() };
        }
        return Poll::Ready(Some(item));
      }

      // 2. Check Rendezvous Handoff
      if let Some(mut sender_ptr) = unsafe { guard.waiting_async_senders.pop_front() } {
        let sender = unsafe { sender_ptr.as_mut() };
        let item = sender.item.take().expect("Sender must have an item");
        drop(guard);
        sender.wake();
        return Poll::Ready(Some(item));
      }
      if let Some(mut sender_ptr) = unsafe { guard.waiting_sync_senders.pop_front() } {
        let sender = unsafe { sender_ptr.as_mut() };
        let item = sender.item.take().expect("Sender must have an item");
        drop(guard);
        sender.wake();
        return Poll::Ready(Some(item));
      }

      if guard.sender_count == 0 {
        return Poll::Ready(None);
      }

      // 3. Park
      unsafe {
        if this.waiter_node.is_enqueued() {
          if !this.waiter_node.waker.as_ref().unwrap().will_wake(cx.waker()) {
            this.waiter_node.waker = Some(cx.waker().clone());
          }
        } else {
          this.waiter_node.waker = Some(cx.waker().clone());
          let node_ptr = NonNull::from(&mut this.waiter_node);
          guard.waiting_async_receivers.push_back(node_ptr);
        }
      }
      return Poll::Pending;
    }
  }
}
